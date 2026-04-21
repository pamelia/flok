use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    widgets::{Block, Borders, Widget},
};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::app_event::AppEvent;
use crate::bottom_pane::BottomPane;
use crate::chat_view::ChatView;
use crate::clipboard::Clipboard;
use crate::footer::FooterState;
use crate::history::{ActiveItem, HistoryItem, Role, TeamEventKind};
use crate::selection::{
    extract_selection_text, paint_selection, ClickTracker, LayoutRects, PanelBuffer, PanelKind,
    SelectionMode, SelectionPoint, SelectionState,
};
use crate::sidebar::SidebarState;
use crate::tui::Tui;
use crate::types::{TuiChannels, UiCommand};
use unicode_width::UnicodeWidthStr;

const COALESCE_WINDOW_MS: u64 = 50;

#[expect(
    clippy::struct_field_names,
    reason = "Wave 1 keeps the user-specified AppEvent channel names"
)]
pub(crate) struct App {
    // Channels to engine (input)
    channels: TuiChannels,

    // Internal event bus
    app_event_rx: mpsc::UnboundedReceiver<AppEvent>,

    // UI state (mutated by handle_event; rendered by render)
    history: Vec<HistoryItem>,
    active: Option<ActiveItem>,
    bottom_pane: BottomPane,
    chat_view: ChatView,
    sidebar: SidebarState,
    footer: FooterState,
    selection: Option<SelectionState>,
    click_tracker: ClickTracker,
    clipboard: Clipboard,
    panel_buffers: Vec<PanelBuffer>,
    layout_rects: LayoutRects,
    chat_drag_lock: Option<ChatDragLock>,

    terminal_size: (u16, u16),
    waiting_for_response: bool,

    // Theme
    theme: crate::theme::Theme,

    // Lifecycle
    running: bool,
    dirty: bool,
    transcript_height_cache: Option<TranscriptHeightCache>,
    render_count: u64,
}

impl App {
    pub(crate) fn new(
        channels: TuiChannels,
        _app_event_tx: mpsc::UnboundedSender<AppEvent>,
        app_event_rx: mpsc::UnboundedReceiver<AppEvent>,
    ) -> Self {
        let model_name = channels.model_name.clone();
        let plan_mode = channels.plan_mode.is_plan();
        let theme = crate::theme::Theme::dark();
        let footer =
            FooterState { plan_mode, model: model_name.clone(), context_pct: 0.0, waiting: false };
        let sidebar = SidebarState {
            session_title: String::from("flok"),
            model: model_name,
            plan_mode,
            visible: false,
            ..Default::default()
        };

        Self {
            channels,
            app_event_rx,
            history: Vec::new(),
            active: None,
            bottom_pane: BottomPane::new(),
            chat_view: ChatView::new(),
            sidebar,
            footer,
            selection: None,
            click_tracker: ClickTracker::default(),
            clipboard: Clipboard::new(),
            panel_buffers: Vec::new(),
            layout_rects: LayoutRects::default(),
            chat_drag_lock: None,
            terminal_size: (80, 24),
            waiting_for_response: false,
            theme,
            running: true,
            dirty: true,
            transcript_height_cache: None,
            render_count: 0,
        }
    }

    pub(crate) async fn run(&mut self, tui: &mut Tui) -> Result<()> {
        let mut renderer = TuiRenderer { tui };
        self.run_with_renderer(&mut renderer).await
    }

    async fn run_with_renderer<R: AppRenderer>(&mut self, renderer: &mut R) -> Result<()> {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await;
        let mut coalescer = RenderCoalescer::default();

        self.draw(renderer)?;

        while self.running {
            let must_render_now = if let Some(deadline) = coalescer.deadline {
                tokio::select! {
                    biased;

                    Some(event) = self.app_event_rx.recv() => {
                        self.process_event(event, &mut coalescer)
                    }

                    Some(ui_event) = self.channels.ui_rx.recv() => {
                        self.process_event(
                            crate::adapter::from_ui_event(ui_event),
                            &mut coalescer,
                        )
                    }

                    result = self.channels.bus_rx.recv() => {
                        self.process_bus_result(result, &mut coalescer)
                    }

                    Some(req) = self.channels.perm_rx.recv() => {
                        self.process_event(
                            crate::adapter::from_permission_request(req),
                            &mut coalescer,
                        )
                    }

                    Some(req) = self.channels.question_rx.recv() => {
                        self.process_event(
                            crate::adapter::from_question_request(req),
                            &mut coalescer,
                        )
                    }

                    _ = tick.tick() => {
                        self.process_event(AppEvent::Tick, &mut coalescer)
                    }

                    () = tokio::time::sleep_until(deadline) => {
                        coalescer.on_timeout(self.dirty)
                    }
                }
            } else {
                tokio::select! {
                    biased;

                    Some(event) = self.app_event_rx.recv() => {
                        self.process_event(event, &mut coalescer)
                    }

                    Some(ui_event) = self.channels.ui_rx.recv() => {
                        self.process_event(
                            crate::adapter::from_ui_event(ui_event),
                            &mut coalescer,
                        )
                    }

                    result = self.channels.bus_rx.recv() => {
                        self.process_bus_result(result, &mut coalescer)
                    }

                    Some(req) = self.channels.perm_rx.recv() => {
                        self.process_event(
                            crate::adapter::from_permission_request(req),
                            &mut coalescer,
                        )
                    }

                    Some(req) = self.channels.question_rx.recv() => {
                        self.process_event(
                            crate::adapter::from_question_request(req),
                            &mut coalescer,
                        )
                    }

                    _ = tick.tick() => {
                        self.process_event(AppEvent::Tick, &mut coalescer)
                    }
                }
            };

            if must_render_now {
                self.draw(renderer)?;
            }
        }

        Ok(())
    }

    fn draw<R: AppRenderer>(&mut self, renderer: &mut R) -> Result<()> {
        renderer.draw(self)?;
        self.dirty = false;
        self.render_count = self.render_count.saturating_add(1);
        Ok(())
    }

    fn process_event(&mut self, event: AppEvent, coalescer: &mut RenderCoalescer) -> bool {
        let coalescible = Self::is_coalescible_event(&event);
        self.handle_event(event);
        coalescer.after_event(coalescible, self.dirty)
    }

    fn process_bus_result(
        &mut self,
        result: std::result::Result<
            flok_core::bus::BusEvent,
            tokio::sync::broadcast::error::RecvError,
        >,
        coalescer: &mut RenderCoalescer,
    ) -> bool {
        match result {
            Ok(bus_event) => crate::adapter::from_bus_event(bus_event)
                .is_some_and(|event| self.process_event(event, coalescer)),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("bus broadcast lagged, skipped {n} events");
                false
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                self.running = false;
                coalescer.after_event(false, self.dirty)
            }
        }
    }

    fn is_coalescible_event(event: &AppEvent) -> bool {
        matches!(
            event,
            AppEvent::UiEvent(crate::types::UiEvent::TextDelta(_))
                | AppEvent::BusEvent(
                    flok_core::bus::BusEvent::TextDelta { .. }
                        | flok_core::bus::BusEvent::ReasoningDelta { .. }
                )
        )
    }

    fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Quit => {
                self.running = false;
                self.dirty = true;
            }
            AppEvent::Resize(width, height) => {
                self.terminal_size = (width, height);
                self.dirty = true;
            }
            AppEvent::Tick => {
                let todos = self
                    .channels
                    .todo_list
                    .items()
                    .into_iter()
                    .map(|item| format!("[{}] {}", item.status, item.content))
                    .collect::<Vec<_>>();
                if self.sidebar.todos != todos {
                    self.sidebar.todos = todos;
                    self.dirty = true;
                }
            }
            AppEvent::Key(key) => {
                if matches!(key.code, KeyCode::Tab) {
                    self.handle_event(AppEvent::TogglePlanMode);
                } else if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char('c'))
                {
                    // Ctrl+C only copies when there is an active text selection.
                    // It does NOT quit — use Ctrl+D to exit. Ctrl+B is unbound.
                    if self.selection.as_ref().is_some_and(SelectionState::has_extent) {
                        self.copy_active_selection();
                        self.selection = None;
                    }
                } else if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char('d'))
                {
                    self.running = false;
                } else if self.waiting_for_response
                    && !self.bottom_pane.has_overlay()
                    && matches!(key.code, KeyCode::Esc)
                {
                    self.handle_event(AppEvent::Cancel);
                } else if self.handle_chat_scroll_key(key) {
                    // handled by scroll helper
                } else if let Some(next_event) = self.bottom_pane.handle_key(key) {
                    self.handle_event(next_event);
                }
                self.dirty = true;
            }
            AppEvent::Mouse(mouse) => self.handle_mouse(mouse),
            AppEvent::Paste(s) => {
                self.bottom_pane.handle_paste(&s);
                self.dirty = true;
            }
            AppEvent::UiEvent(ui_event) => match ui_event {
                crate::types::UiEvent::TextDelta(text) => {
                    self.dirty |= crate::stream::ingest_assistant_delta(&mut self.active, &text);
                    self.maintain_chat_drag_lock();
                }
                crate::types::UiEvent::AssistantDone(text) => {
                    self.finish_assistant(text, false);
                }
                crate::types::UiEvent::Cancelled(text) => {
                    self.finish_assistant(text, true);
                }
                crate::types::UiEvent::HistoryMessage { role, content } => {
                    let item = match role.as_str() {
                        "user" => HistoryItem::user(content),
                        "assistant" => HistoryItem::assistant(content, true),
                        "system" => HistoryItem::system_info(content),
                        other => {
                            tracing::debug!(
                                role = other,
                                "unknown history role; treating as system"
                            );
                            HistoryItem::system_info(content)
                        }
                    };
                    self.history.push(item);
                    self.chat_view.on_new_content();
                    self.maintain_chat_drag_lock();
                    self.dirty = true;
                }
                crate::types::UiEvent::Error(message) => {
                    self.history.push(HistoryItem::system_error(message));
                    self.chat_view.on_new_content();
                    self.maintain_chat_drag_lock();
                    self.dirty = true;
                }
                crate::types::UiEvent::SessionSwitched { messages } => {
                    self.history.clear();
                    self.active = None;
                    self.waiting_for_response = false;
                    self.footer.waiting = false;
                    self.bottom_pane.set_waiting(false);
                    for (role, content) in messages {
                        let item = match role.as_str() {
                            "user" => HistoryItem::user(content),
                            "assistant" => HistoryItem::assistant(content, true),
                            "system" => HistoryItem::system_info(content),
                            other => {
                                tracing::debug!(
                                    role = other,
                                    "unknown switched-session role; treating as system"
                                );
                                HistoryItem::system_info(content)
                            }
                        };
                        self.history.push(item);
                    }
                    self.chat_view.scroll_offset = 0;
                    self.chat_view.follow_bottom = true;
                    self.chat_view.on_new_content();
                    self.selection = None;
                    self.release_chat_drag_lock();
                    self.dirty = true;
                }
                crate::types::UiEvent::BranchPoints(points) => {
                    let body = if points.is_empty() {
                        "No branch points available.".to_string()
                    } else {
                        points
                            .into_iter()
                            .map(|(message_id, number, preview)| {
                                format!("{number}: {preview} ({message_id})")
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    self.history.push(HistoryItem::system_info(body));
                    self.chat_view.on_new_content();
                    self.maintain_chat_drag_lock();
                    self.dirty = true;
                }
            },
            AppEvent::BusEvent(bus_event) => match bus_event {
                flok_core::bus::BusEvent::TextDelta { delta, .. } => {
                    self.dirty |= crate::stream::ingest_assistant_delta(&mut self.active, &delta);
                    self.maintain_chat_drag_lock();
                }
                flok_core::bus::BusEvent::ReasoningDelta { delta, .. } => {
                    self.dirty |= crate::stream::ingest_reasoning_delta(&mut self.active, &delta);
                    self.maintain_chat_drag_lock();
                }
                flok_core::bus::BusEvent::TokenUsage { input_tokens, output_tokens, .. } => {
                    self.sidebar.input_tokens = input_tokens;
                    self.sidebar.output_tokens = output_tokens;
                    self.dirty = true;
                }
                flok_core::bus::BusEvent::CostUpdate { total_cost_usd, .. } => {
                    self.sidebar.session_cost_usd = total_cost_usd;
                    self.dirty = true;
                }
                flok_core::bus::BusEvent::ContextUsage { used_tokens, max_tokens, .. } => {
                    let pct = if max_tokens == 0 {
                        0.0
                    } else {
                        (used_tokens as f32 / max_tokens as f32) * 100.0
                    };
                    self.footer.context_pct = pct;
                    self.sidebar.context_pct = pct;
                    self.dirty = true;
                }
                flok_core::bus::BusEvent::ToolCallStarted { tool_name, .. } => {
                    self.dirty |= crate::stream::begin_tool_call(&mut self.active, tool_name);
                }
                flok_core::bus::BusEvent::ToolCallCompleted { is_error, .. } => {
                    let item =
                        crate::stream::finalize_tool_call(self.active.take(), is_error, None);
                    self.history.push(item);
                    self.chat_view.on_new_content();
                    self.maintain_chat_drag_lock();
                    self.dirty = true;
                }
                flok_core::bus::BusEvent::StreamingComplete { .. } => {
                    if self.active.as_ref().is_some_and(|item| item.role == Role::Assistant) {
                        let item = crate::stream::finalize_assistant(
                            self.active.take(),
                            String::new(),
                            false,
                        );
                        self.push_history_if_not_duplicate(item);
                        self.chat_view.on_new_content();
                        self.maintain_chat_drag_lock();
                        self.dirty = true;
                    }
                }
                flok_core::bus::BusEvent::Cancelled { .. } => {
                    self.waiting_for_response = false;
                    self.footer.waiting = false;
                    self.bottom_pane.set_waiting(false);
                    self.dirty = true;
                }
                flok_core::bus::BusEvent::CompressionStats { t1_pruned, l2_compressed, .. } => {
                    tracing::debug!(t1_pruned, l2_compressed, "compression stats updated");
                }
                flok_core::bus::BusEvent::VerificationStarted { command, .. } => {
                    self.history
                        .push(HistoryItem::system_info(format!("Verification running: {command}")));
                    self.chat_view.on_new_content();
                    self.maintain_chat_drag_lock();
                    self.dirty = true;
                }
                flok_core::bus::BusEvent::VerificationCompleted { success, summary, .. } => {
                    let item = if success {
                        HistoryItem::system_info(summary)
                    } else {
                        HistoryItem::system_error(summary)
                    };
                    self.history.push(item);
                    self.chat_view.on_new_content();
                    self.maintain_chat_drag_lock();
                    self.dirty = true;
                }
                flok_core::bus::BusEvent::TeamCreated { team_name, .. } => {
                    self.history.push(HistoryItem::TeamEvent {
                        kind: TeamEventKind::Created,
                        agent: team_name,
                        detail: "created".to_string(),
                    });
                    self.chat_view.on_new_content();
                    self.maintain_chat_drag_lock();
                    self.dirty = true;
                }
                flok_core::bus::BusEvent::TeamMemberCompleted { agent_name, .. } => {
                    self.history.push(HistoryItem::TeamEvent {
                        kind: TeamEventKind::Completed,
                        agent: agent_name,
                        detail: "completed".to_string(),
                    });
                    self.chat_view.on_new_content();
                    self.maintain_chat_drag_lock();
                    self.dirty = true;
                }
                flok_core::bus::BusEvent::TeamMemberFailed { agent_name, error, .. } => {
                    self.history.push(HistoryItem::TeamEvent {
                        kind: TeamEventKind::Failed,
                        agent: agent_name,
                        detail: error,
                    });
                    self.chat_view.on_new_content();
                    self.maintain_chat_drag_lock();
                    self.dirty = true;
                }
                flok_core::bus::BusEvent::ProviderFallback {
                    from_provider,
                    to_provider,
                    reason,
                    ..
                } => {
                    self.history.push(HistoryItem::provider_fallback(
                        from_provider,
                        to_provider,
                        reason,
                    ));
                    self.chat_view.on_new_content();
                    self.maintain_chat_drag_lock();
                    self.dirty = true;
                }
                flok_core::bus::BusEvent::ModelRouted { from_model, to_model, reason, .. } => {
                    self.history.push(HistoryItem::system_info(format!(
                        "Model routed: {from_model} -> {to_model} ({reason})"
                    )));
                    self.chat_view.on_new_content();
                    self.maintain_chat_drag_lock();
                    self.dirty = true;
                }
                other => {
                    tracing::debug!(event = ?other, "ignoring unsupported bus event in MVP");
                }
            },
            AppEvent::Submit(text) => {
                let trimmed = text.trim();
                if let Some(cmdline) = trimmed.strip_prefix('/') {
                    let mut parts = cmdline.split_whitespace();
                    let name = parts.next().unwrap_or("");
                    let rest = parts.collect::<Vec<_>>().join(" ");
                    match name {
                        "quit" | "exit" | "q" => {
                            self.running = false;
                        }
                        "clear" | "new" => {
                            self.history.clear();
                            self.active = None;
                            self.waiting_for_response = false;
                            self.footer.waiting = false;
                            self.bottom_pane.set_waiting(false);
                            tracing::debug!(
                                "no NewSession UiCommand variant; cleared local transcript only"
                            );
                        }
                        "undo" => {
                            let _ = self.channels.cmd_tx.send(UiCommand::Undo);
                        }
                        "redo" => {
                            let _ = self.channels.cmd_tx.send(UiCommand::Redo);
                        }
                        "tree" => {
                            let _ = self.channels.cmd_tx.send(UiCommand::ShowTree);
                        }
                        "branch" => {
                            if rest.is_empty() {
                                let _ = self.channels.cmd_tx.send(UiCommand::ListBranchPoints);
                            } else {
                                let _ = self.channels.cmd_tx.send(UiCommand::BranchAt(rest));
                            }
                        }
                        "label" => {
                            if rest.is_empty() {
                                self.history.push(HistoryItem::system_warn("Usage: /label <text>"));
                                self.chat_view.on_new_content();
                                self.maintain_chat_drag_lock();
                            } else {
                                let _ = self.channels.cmd_tx.send(UiCommand::SetLabel(rest));
                            }
                        }
                        "plans" => {
                            let _ = self.channels.cmd_tx.send(UiCommand::ListPlans);
                        }
                        "show-plan" => {
                            let plan_id = (!rest.is_empty()).then_some(rest);
                            let _ = self.channels.cmd_tx.send(UiCommand::ShowPlan(plan_id));
                        }
                        "approve" => {
                            let plan_id = (!rest.is_empty()).then_some(rest);
                            let _ = self.channels.cmd_tx.send(UiCommand::ApprovePlan(plan_id));
                        }
                        "execute-plan" => {
                            let plan_id = (!rest.is_empty()).then_some(rest);
                            let _ = self.channels.cmd_tx.send(UiCommand::ExecutePlan(plan_id));
                        }
                        "plan" => {
                            self.channels.plan_mode.set(true);
                            self.footer.plan_mode = true;
                            self.sidebar.plan_mode = true;
                        }
                        "build" => {
                            self.channels.plan_mode.set(false);
                            self.footer.plan_mode = false;
                            self.sidebar.plan_mode = false;
                        }
                        "sidebar" => {
                            self.sidebar.visible = !self.sidebar.visible;
                        }
                        "sessions" => {
                            let _ = self.channels.cmd_tx.send(UiCommand::ListSessions);
                        }
                        "help" => {
                            self.history.push(HistoryItem::system_info(
                                "Slash: /new /clear /undo /redo /tree /branch /label /plans /show-plan /approve /execute-plan /plan /build /sidebar /sessions /help /quit",
                            ));
                            self.chat_view.on_new_content();
                            self.maintain_chat_drag_lock();
                        }
                        "" => {}
                        _ => {
                            self.history.push(HistoryItem::system_warn(format!(
                                "Unknown command: /{cmdline}"
                            )));
                            self.chat_view.on_new_content();
                            self.maintain_chat_drag_lock();
                        }
                    }
                } else if !trimmed.is_empty() {
                    self.history.push(HistoryItem::user(text.clone()));
                    self.waiting_for_response = true;
                    self.footer.waiting = true;
                    self.bottom_pane.set_waiting(true);
                    self.chat_view.on_new_content();
                    self.maintain_chat_drag_lock();
                    let _ = self.channels.cmd_tx.send(UiCommand::SendMessage(text));
                }
                self.dirty = true;
            }
            AppEvent::PermissionRequest(req) => {
                let overlay = crate::overlays::Overlay::Permission(
                    crate::overlays::permission::PermissionOverlay::new(req),
                );
                self.bottom_pane.set_overlay(overlay);
                self.dirty = true;
            }
            AppEvent::QuestionRequest(req) => {
                let overlay = crate::overlays::Overlay::Question(
                    crate::overlays::question::QuestionOverlay::new(req),
                );
                self.bottom_pane.set_overlay(overlay);
                self.dirty = true;
            }
            AppEvent::Cancel => {
                let _ = self.channels.cmd_tx.send(UiCommand::Cancel);
                if self.active.is_some() {
                    let item = match self.active.as_ref().map(|active| active.role) {
                        Some(Role::ToolCall) => {
                            crate::stream::finalize_tool_call(self.active.take(), true, None)
                        }
                        _ => crate::stream::finalize_assistant(
                            self.active.take(),
                            String::new(),
                            true,
                        ),
                    };
                    self.push_history_if_not_duplicate(item);
                    self.chat_view.on_new_content();
                    self.maintain_chat_drag_lock();
                }
                self.waiting_for_response = false;
                self.footer.waiting = false;
                self.bottom_pane.set_waiting(false);
                self.dirty = true;
            }
            AppEvent::ToggleSidebar => {
                self.sidebar.visible = !self.sidebar.visible;
                self.dirty = true;
            }
            AppEvent::TogglePlanMode => {
                let plan_mode = self.channels.plan_mode.toggle();
                self.footer.plan_mode = plan_mode;
                self.sidebar.plan_mode = plan_mode;
                self.dirty = true;
            }
            AppEvent::ShowOverlay(kind) => {
                tracing::debug!(?kind, "ShowOverlay without payload is unsupported; ignoring");
                self.dirty = true;
            }
            AppEvent::HideOverlay => {
                self.bottom_pane.clear_overlay();
                self.dirty = true;
            }
        }
    }

    fn render(&mut self, frame: &mut ratatui::Frame<'_>) {
        let area = frame.area();
        if area.width == 0 || area.height == 0 {
            return;
        }

        let layout = self.compute_layout(area);
        self.layout_rects = LayoutRects {
            chat: layout.chat_inner,
            sidebar: layout.sidebar,
            composer: layout.bottom,
        };
        self.panel_buffers.clear();

        if let Some(sidebar) = layout.sidebar {
            crate::sidebar::render(&self.sidebar, sidebar, frame.buffer_mut(), &self.theme);
            self.panel_buffers.push(PanelBuffer {
                rect: sidebar,
                rows: crate::sidebar::visible_rows(&self.sidebar, sidebar, &self.theme),
            });
        }

        let chat_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ratatui_color(self.theme.border)))
            .title(format!(" flok — {} ", self.footer.model));
        frame.render_widget(chat_block, layout.chat);

        let (chat_lines, chat_rows) = self.chat_view.visible_lines_and_rows(
            &self.history,
            self.active.as_ref(),
            &self.theme,
            layout.chat_inner,
        );
        for (index, line) in chat_lines.iter().enumerate() {
            let row_y = layout.chat_inner.y.saturating_add(index as u16);
            if row_y >= layout.chat_inner.y.saturating_add(layout.chat_inner.height) {
                break;
            }
            let row = Rect {
                x: layout.chat_inner.x,
                y: row_y,
                width: layout.chat_inner.width,
                height: 1,
            };
            line.clone().render(row, frame.buffer_mut());
        }
        self.panel_buffers.push(PanelBuffer { rect: layout.chat_inner, rows: chat_rows });
        self.bottom_pane.render(layout.bottom, frame.buffer_mut(), &self.theme);
        self.panel_buffers.push(PanelBuffer {
            rect: layout.bottom,
            rows: self.bottom_pane.visible_rows(layout.bottom, &self.theme),
        });
        crate::footer::render(&self.footer, &self.theme, layout.footer, frame.buffer_mut());

        if let Some(selection) = &self.selection {
            paint_selection(frame.buffer_mut(), selection, &self.panel_buffers);
        }
    }

    fn compute_layout(&self, area: Rect) -> AppLayout {
        let (main, sidebar) = if self.sidebar.visible && area.width > 32 {
            let cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(32)]).split(area);
            (cols[0], Some(cols[1]))
        } else {
            (area, None)
        };

        let content_height = main.height.saturating_sub(1);
        let max_bottom = content_height.saturating_mul(3) / 5;
        let computed_bottom = self.bottom_pane.compute_height(main.width).max(3);
        let bottom_height = if content_height <= 1 {
            0
        } else {
            computed_bottom.min(max_bottom.max(1)).min(content_height.saturating_sub(1))
        };

        let rows = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(bottom_height),
            Constraint::Length(1),
        ])
        .split(main);

        let chat = rows[0];
        let chat_inner = Block::default().borders(Borders::ALL).inner(chat);

        AppLayout { chat, chat_inner, bottom: rows[1], footer: rows[2], sidebar }
    }

    fn finish_assistant(&mut self, fallback_text: String, cancelled: bool) {
        let item = crate::stream::finalize_assistant(self.active.take(), fallback_text, cancelled);
        self.push_history_if_not_duplicate(item);
        self.waiting_for_response = false;
        self.footer.waiting = false;
        self.bottom_pane.set_waiting(false);
        self.chat_view.on_new_content();
        self.maintain_chat_drag_lock();
        self.dirty = true;
    }

    fn push_history_if_not_duplicate(&mut self, item: HistoryItem) {
        let is_duplicate = match (&item, self.history.last()) {
            (
                HistoryItem::Assistant { text: next, .. },
                Some(HistoryItem::Assistant { text: prev, .. }),
            ) => prev == next,
            (
                HistoryItem::ToolCall {
                    name: next_name,
                    preview: next_preview,
                    is_error: next_error,
                    duration_ms: next_duration,
                },
                Some(HistoryItem::ToolCall {
                    name: prev_name,
                    preview: prev_preview,
                    is_error: prev_error,
                    duration_ms: prev_duration,
                }),
            ) => {
                prev_name == next_name
                    && prev_preview == next_preview
                    && prev_error == next_error
                    && prev_duration == next_duration
            }
            _ => false,
        };

        if !is_duplicate {
            self.history.push(item);
        }
    }

    fn handle_chat_scroll_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        match (key.code, key.modifiers) {
            (KeyCode::PageUp, _) => {
                let viewport = self
                    .compute_layout(Rect::new(0, 0, self.terminal_size.0, self.terminal_size.1))
                    .chat_inner
                    .height;
                let delta = -i32::from(viewport.max(2) / 2);
                self.scroll_chat(delta);
                true
            }
            (KeyCode::PageDown, _) => {
                let viewport = self
                    .compute_layout(Rect::new(0, 0, self.terminal_size.0, self.terminal_size.1))
                    .chat_inner
                    .height;
                let delta = i32::from(viewport.max(2) / 2);
                self.scroll_chat(delta);
                true
            }
            (KeyCode::Home, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                let total = self.transcript_height();
                let viewport = usize::from(
                    self.compute_layout(Rect::new(
                        0,
                        0,
                        self.terminal_size.0,
                        self.terminal_size.1,
                    ))
                    .chat_inner
                    .height,
                );
                self.chat_view.scroll_offset = total.saturating_sub(viewport);
                self.chat_view.follow_bottom = false;
                true
            }
            (KeyCode::End, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                self.chat_view.scroll_offset = 0;
                self.chat_view.follow_bottom = true;
                true
            }
            _ => false,
        }
    }

    fn scroll_chat(&mut self, delta: i32) {
        let layout =
            self.compute_layout(Rect::new(0, 0, self.terminal_size.0, self.terminal_size.1));
        let transcript_height = self.transcript_height();
        self.chat_view.handle_scroll(delta, layout.chat_inner.height, transcript_height);
    }

    fn transcript_height(&mut self) -> usize {
        let layout =
            self.compute_layout(Rect::new(0, 0, self.terminal_size.0, self.terminal_size.1));
        let width = layout.chat_inner.width.max(1);
        let active_id = self.active.as_ref().map(|active| active.id);
        let active_revision = self.active.as_ref().map_or(0, |active| active.revision);

        if let Some(cache) = self.transcript_height_cache {
            if cache.history_len == self.history.len()
                && cache.active_id == active_id
                && cache.active_revision == active_revision
                && cache.width == width
            {
                return cache.height;
            }
        }

        let mut total = self
            .history
            .iter()
            .map(|item| usize::from(crate::history::render::height(item, width, &self.theme)))
            .sum::<usize>();
        if let Some(active) = &self.active {
            total += usize::from(crate::history::render::active_height(active, width, &self.theme));
        }

        self.transcript_height_cache = Some(TranscriptHeightCache {
            history_len: self.history.len(),
            active_id,
            active_revision,
            width,
            height: total,
        });

        total
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.bottom_pane.has_overlay() {
            return;
        }
        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
            return;
        }

        let pos = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(panel) = PanelKind::identify(pos, &self.layout_rects) else {
                    if let Some(selection) = self.selection.as_mut() {
                        selection.clear();
                    }
                    self.selection = None;
                    self.release_chat_drag_lock();
                    return;
                };
                let Some(point) = self.clamped_point(panel, pos.0, pos.1) else {
                    return;
                };
                let count = self.click_tracker.register(Instant::now(), pos.0, pos.1);
                let mode = SelectionMode::from_click_count(count);
                let mut selection = SelectionState::start(point).with_mode(mode);
                if mode != SelectionMode::Char {
                    if let Some((anchor, head)) = self.expand_selection(point, mode) {
                        selection.anchor = Some(anchor);
                        selection.head = Some(head);
                    }
                }
                self.selection = Some(selection);
                if panel == PanelKind::Chat {
                    self.acquire_chat_drag_lock();
                } else {
                    self.release_chat_drag_lock();
                }
                self.dirty = true;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some((panel, _)) = self
                    .selection
                    .as_ref()
                    .and_then(|selection| selection.anchor.map(|anchor| (anchor.panel, anchor)))
                else {
                    return;
                };
                let Some(point) = self.clamped_point(panel, pos.0, pos.1) else {
                    return;
                };
                if let Some(selection) = self.selection.as_mut() {
                    selection.extend(point);
                }
                self.maintain_chat_drag_lock();
                self.dirty = true;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(selection) = self.selection.as_mut() {
                    if selection.has_extent() {
                        let text = extract_selection_text(&self.panel_buffers, selection);
                        let _copied = self.clipboard.copy(&text);
                    }
                    selection.clear();
                    self.selection = None;
                }
                self.release_chat_drag_lock();
                self.dirty = true;
            }
            MouseEventKind::ScrollUp => {
                if self.selection.as_ref().is_some_and(SelectionState::is_dragging) {
                    return;
                }
                self.scroll_chat(-3);
                self.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                if self.selection.as_ref().is_some_and(SelectionState::is_dragging) {
                    return;
                }
                self.scroll_chat(3);
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn clamped_point(&self, panel: PanelKind, col: u16, row: u16) -> Option<SelectionPoint> {
        let rect = self.layout_rects.rect_for(panel)?;
        let last_col = rect.right().saturating_sub(1);
        let last_row = rect.bottom().saturating_sub(1);
        Some(SelectionPoint {
            panel,
            col: col.clamp(rect.x, last_col),
            row: row.clamp(rect.y, last_row),
        })
    }

    fn expand_selection(
        &self,
        point: SelectionPoint,
        mode: SelectionMode,
    ) -> Option<(SelectionPoint, SelectionPoint)> {
        let target_rect = self.layout_rects.rect_for(point.panel)?;
        let panel = self.panel_buffers.iter().find(|buffer| buffer.rect == target_rect)?;
        let row_index = usize::from(point.row.saturating_sub(panel.rect.y));
        let line = panel.rows.get(row_index)?;
        let relative_col = usize::from(point.col.saturating_sub(panel.rect.x));

        match mode {
            SelectionMode::Char => None,
            SelectionMode::Word => {
                let (start_col, end_col) =
                    crate::selection::expand_word_by_width(line, relative_col, relative_col);
                Some((
                    SelectionPoint {
                        panel: point.panel,
                        row: point.row,
                        col: panel.rect.x + start_col as u16,
                    },
                    SelectionPoint {
                        panel: point.panel,
                        row: point.row,
                        col: panel.rect.x + end_col as u16,
                    },
                ))
            }
            SelectionMode::Line => Self::expand_line_selection(point, line, panel.rect),
            SelectionMode::Paragraph => Self::expand_paragraph_selection(point, panel),
        }
    }

    fn expand_line_selection(
        point: SelectionPoint,
        line: &str,
        rect: Rect,
    ) -> Option<(SelectionPoint, SelectionPoint)> {
        let line_width = line.width();
        if line_width == 0 {
            return None;
        }
        Some((
            SelectionPoint { panel: point.panel, row: point.row, col: rect.x },
            SelectionPoint {
                panel: point.panel,
                row: point.row,
                col: rect.x + u16::try_from(line_width.saturating_sub(1)).ok()?,
            },
        ))
    }

    fn expand_paragraph_selection(
        point: SelectionPoint,
        panel: &PanelBuffer,
    ) -> Option<(SelectionPoint, SelectionPoint)> {
        let row_index = usize::from(point.row.saturating_sub(panel.rect.y));
        let mut start_row = row_index;
        let mut end_row = row_index;

        while start_row > 0 && !panel.rows[start_row - 1].trim().is_empty() {
            start_row -= 1;
        }
        while end_row + 1 < panel.rows.len() && !panel.rows[end_row + 1].trim().is_empty() {
            end_row += 1;
        }

        let end_width = panel.rows.get(end_row)?.width();
        if end_width == 0 {
            return None;
        }

        Some((
            SelectionPoint {
                panel: point.panel,
                row: panel.rect.y + u16::try_from(start_row).ok()?,
                col: panel.rect.x,
            },
            SelectionPoint {
                panel: point.panel,
                row: panel.rect.y + u16::try_from(end_row).ok()?,
                col: panel.rect.x + u16::try_from(end_width.saturating_sub(1)).ok()?,
            },
        ))
    }

    fn copy_active_selection(&mut self) {
        if let Some(selection) = self.selection.as_ref().filter(|selection| selection.has_extent())
        {
            let text = extract_selection_text(&self.panel_buffers, selection);
            let _copied = self.clipboard.copy(&text);
        }
    }

    fn acquire_chat_drag_lock(&mut self) {
        self.chat_drag_lock = Some(ChatDragLock {
            transcript_height: self.transcript_height(),
            scroll_offset: self.chat_view.scroll_offset,
        });
        self.chat_view.follow_bottom = false;
    }

    fn maintain_chat_drag_lock(&mut self) {
        if let Some(lock) = self.chat_drag_lock {
            let current_height = self.transcript_height();
            let growth = current_height.saturating_sub(lock.transcript_height);
            self.chat_view.scroll_offset = lock.scroll_offset.saturating_add(growth);
            self.chat_view.follow_bottom = false;
        }
    }

    fn release_chat_drag_lock(&mut self) {
        self.chat_drag_lock = None;
    }

    pub(crate) fn test_handle_event(&mut self, event: AppEvent) {
        self.handle_event(event);
    }

    pub(crate) fn test_render(&mut self, frame: &mut ratatui::Frame<'_>) {
        self.render(frame);
    }

    pub(crate) fn test_push_history_item(&mut self, item: HistoryItem) {
        self.history.push(item);
        self.chat_view.on_new_content();
    }

    pub(crate) fn test_set_sidebar_visible(&mut self, visible: bool) {
        self.sidebar.visible = visible;
    }

    pub(crate) fn test_set_composer_text(&mut self, text: &str) {
        self.bottom_pane.handle_paste(text);
    }

    pub(crate) fn test_set_permission_overlay(
        &mut self,
        request: flok_core::tool::PermissionRequest,
    ) {
        let overlay = crate::overlays::Overlay::Permission(
            crate::overlays::permission::PermissionOverlay::new(request),
        );
        self.bottom_pane.set_overlay(overlay);
    }

    pub(crate) fn test_layout_rects(&self) -> LayoutRects {
        self.layout_rects
    }

    pub(crate) fn test_copied_text(&self) -> Option<&str> {
        self.clipboard.last_copied_text.as_deref()
    }

    pub(crate) fn test_is_running(&self) -> bool {
        self.running
    }

    pub(crate) fn test_has_selection(&self) -> bool {
        self.selection.as_ref().is_some_and(SelectionState::has_extent)
    }

    pub(crate) fn test_chat_scroll_offset(&self) -> usize {
        self.chat_view.scroll_offset
    }

    pub(crate) fn test_render_count(&self) -> u64 {
        self.render_count
    }

    pub(crate) async fn test_run_with_renderer<R: AppRenderer>(
        &mut self,
        renderer: &mut R,
    ) -> Result<()> {
        self.run_with_renderer(renderer).await
    }
}

#[derive(Clone, Copy)]
struct AppLayout {
    chat: Rect,
    chat_inner: Rect,
    bottom: Rect,
    footer: Rect,
    sidebar: Option<Rect>,
}

#[derive(Clone, Copy)]
struct ChatDragLock {
    transcript_height: usize,
    scroll_offset: usize,
}

#[derive(Clone, Copy)]
struct TranscriptHeightCache {
    history_len: usize,
    active_id: Option<u64>,
    active_revision: u64,
    width: u16,
    height: usize,
}

#[derive(Default)]
struct RenderCoalescer {
    deadline: Option<tokio::time::Instant>,
}

impl RenderCoalescer {
    fn after_event(&mut self, coalescible: bool, dirty: bool) -> bool {
        if coalescible {
            if dirty && self.deadline.is_none() {
                self.deadline =
                    Some(tokio::time::Instant::now() + Duration::from_millis(COALESCE_WINDOW_MS));
            }
            false
        } else {
            self.deadline = None;
            dirty
        }
    }

    fn on_timeout(&mut self, dirty: bool) -> bool {
        self.deadline = None;
        dirty
    }
}

pub(crate) trait AppRenderer {
    fn draw(&mut self, app: &mut App) -> Result<()>;
}

struct TuiRenderer<'a> {
    tui: &'a mut Tui,
}

impl AppRenderer for TuiRenderer<'_> {
    fn draw(&mut self, app: &mut App) -> Result<()> {
        self.tui.draw(|frame| app.render(frame))
    }
}

fn ratatui_color(color: crossterm::style::Color) -> ratatui::style::Color {
    match color {
        crossterm::style::Color::Reset => ratatui::style::Color::Reset,
        crossterm::style::Color::Black => ratatui::style::Color::Black,
        crossterm::style::Color::DarkGrey => ratatui::style::Color::DarkGray,
        crossterm::style::Color::Red | crossterm::style::Color::DarkRed => {
            ratatui::style::Color::Red
        }
        crossterm::style::Color::Green | crossterm::style::Color::DarkGreen => {
            ratatui::style::Color::Green
        }
        crossterm::style::Color::Yellow | crossterm::style::Color::DarkYellow => {
            ratatui::style::Color::Yellow
        }
        crossterm::style::Color::Blue | crossterm::style::Color::DarkBlue => {
            ratatui::style::Color::Blue
        }
        crossterm::style::Color::Magenta | crossterm::style::Color::DarkMagenta => {
            ratatui::style::Color::Magenta
        }
        crossterm::style::Color::Cyan | crossterm::style::Color::DarkCyan => {
            ratatui::style::Color::Cyan
        }
        crossterm::style::Color::Grey => ratatui::style::Color::Gray,
        crossterm::style::Color::White => ratatui::style::Color::White,
        crossterm::style::Color::Rgb { r, g, b } => ratatui::style::Color::Rgb(r, g, b),
        crossterm::style::Color::AnsiValue(value) => ratatui::style::Color::Indexed(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };
    use tokio::sync::{broadcast, mpsc};

    use flok_core::session::PlanMode;
    use flok_core::tool::{PermissionRequest, QuestionRequest, TodoList};
    #[derive(Clone, Default)]
    struct CountingRenderer {
        draws: Arc<AtomicU64>,
    }

    impl AppRenderer for CountingRenderer {
        fn draw(&mut self, _app: &mut App) -> Result<()> {
            self.draws.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    fn make_channels() -> (TuiChannels, mpsc::UnboundedReceiver<UiCommand>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (_ui_tx, ui_rx) = mpsc::unbounded_channel();
        let (_bus_tx, bus_rx) = broadcast::channel::<flok_core::bus::BusEvent>(16);
        let (_perm_tx, perm_rx) = mpsc::unbounded_channel::<PermissionRequest>();
        let (_question_tx, question_rx) = mpsc::unbounded_channel::<QuestionRequest>();

        let channels = TuiChannels {
            cmd_tx,
            ui_rx,
            bus_rx,
            perm_rx,
            question_rx,
            todo_list: TodoList::new(),
            plan_mode: PlanMode::new(),
            model_name: String::from("test-model"),
        };

        (channels, cmd_rx)
    }

    fn ctrl(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    fn selection_app() -> App {
        let (channels, _cmd_rx) = make_channels();
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut app = App::new(channels, tx, rx);
        app.layout_rects = LayoutRects {
            chat: Rect::new(0, 0, 20, 3),
            sidebar: None,
            composer: Rect::new(0, 3, 20, 3),
        };
        app.panel_buffers = vec![PanelBuffer {
            rect: Rect::new(0, 0, 20, 3),
            rows: vec!["hello world".to_string(), "second row".to_string(), String::new()],
        }];
        let mut selection =
            SelectionState::start(SelectionPoint { panel: PanelKind::Chat, row: 0, col: 0 });
        selection.extend(SelectionPoint { panel: PanelKind::Chat, row: 0, col: 4 });
        app.selection = Some(selection);
        app
    }

    async fn spawn_counting_app(
    ) -> (mpsc::UnboundedSender<AppEvent>, Arc<AtomicU64>, tokio::task::JoinHandle<Result<()>>)
    {
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let (ui_tx, ui_rx) = mpsc::unbounded_channel();
        let (bus_tx, bus_rx) = broadcast::channel::<flok_core::bus::BusEvent>(16);
        let (perm_tx, perm_rx) = mpsc::unbounded_channel::<PermissionRequest>();
        let (question_tx, question_rx) = mpsc::unbounded_channel::<QuestionRequest>();
        let channels = TuiChannels {
            cmd_tx,
            ui_rx,
            bus_rx,
            perm_rx,
            question_rx,
            todo_list: TodoList::new(),
            plan_mode: PlanMode::new(),
            model_name: String::from("test-model"),
        };
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut app = App::new(channels, tx.clone(), rx);
        let renderer = CountingRenderer::default();
        let draws = renderer.draws.clone();
        let join = tokio::spawn(async move {
            let _keepalive = (ui_tx, bus_tx, perm_tx, question_tx);
            let mut renderer = renderer;
            app.test_run_with_renderer(&mut renderer).await
        });

        while draws.load(Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
        (tx, draws, join)
    }

    #[tokio::test]
    async fn quit_event_stops_loop() {
        let (channels, _cmd_rx) = make_channels();
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut app = App::new(channels, tx.clone(), rx);

        tx.send(AppEvent::Quit).expect("quit event should be queued before handling");
        drop(tx);

        app.handle_event(AppEvent::Quit);

        assert!(!app.running);
    }

    #[tokio::test]
    async fn toggle_sidebar_event_flips_visibility() {
        let (channels, _cmd_rx) = make_channels();
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut app = App::new(channels, tx, rx);

        assert!(!app.sidebar.visible);

        app.handle_event(AppEvent::ToggleSidebar);

        assert!(app.sidebar.visible);
    }

    #[tokio::test]
    async fn cancel_event_sends_cancel_command() {
        let (channels, mut cmd_rx) = make_channels();
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut app = App::new(channels, tx, rx);

        app.handle_event(AppEvent::Cancel);

        let command = cmd_rx.try_recv().expect("cancel command should be queued");
        assert!(matches!(command, UiCommand::Cancel));
    }

    #[tokio::test]
    async fn submit_plans_sends_list_plans_command() {
        let (channels, mut cmd_rx) = make_channels();
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut app = App::new(channels, tx, rx);

        app.handle_event(AppEvent::Submit("/plans".to_string()));

        let command = cmd_rx.try_recv().expect("list plans command should be queued");
        assert!(matches!(command, UiCommand::ListPlans));
    }

    #[tokio::test]
    async fn submit_execute_plan_with_id_sends_execute_command() {
        let (channels, mut cmd_rx) = make_channels();
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut app = App::new(channels, tx, rx);

        app.handle_event(AppEvent::Submit("/execute-plan plan-123".to_string()));

        let command = cmd_rx.try_recv().expect("execute plan command should be queued");
        assert!(matches!(command, UiCommand::ExecutePlan(Some(plan_id)) if plan_id == "plan-123"));
    }

    #[tokio::test]
    async fn model_routed_bus_event_adds_system_history_item() {
        let (channels, _cmd_rx) = make_channels();
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut app = App::new(channels, tx, rx);

        app.handle_event(AppEvent::BusEvent(flok_core::bus::BusEvent::ModelRouted {
            session_id: "session-1".to_string(),
            from_model: "openai/gpt-5.4-mini".to_string(),
            to_model: "openai/gpt-5.4".to_string(),
            reason: "complexity score 4 (architecture or planning request)".to_string(),
        }));

        assert!(app.history.iter().any(|item| matches!(
            item,
            HistoryItem::System { text, .. }
                if text.contains("Model routed: openai/gpt-5.4-mini -> openai/gpt-5.4")
        )));
    }

    #[tokio::test]
    async fn ctrl_c_with_selection_copies_not_quit() {
        let mut app = selection_app();

        app.handle_event(AppEvent::Key(ctrl('c')));

        assert!(app.running);
        assert!(app.selection.is_none());
        assert_eq!(app.clipboard.last_copied_text.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn ctrl_c_without_selection_is_noop() {
        let (channels, _cmd_rx) = make_channels();
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut app = App::new(channels, tx, rx);

        app.handle_event(AppEvent::Key(ctrl('c')));

        // Ctrl+C must NEVER quit — only copy-on-selection. Without a
        // selection it's a no-op; the app stays running. Ctrl+D quits.
        assert!(app.running);
    }

    #[tokio::test]
    async fn ctrl_d_always_quits() {
        let mut app = selection_app();

        app.handle_event(AppEvent::Key(ctrl('d')));

        assert!(!app.running);
    }

    #[tokio::test]
    async fn coalescing_batches_deltas_within_window() {
        let (tx, draws, join) = spawn_counting_app().await;

        for index in 0..10 {
            let sent =
                tx.send(AppEvent::UiEvent(crate::types::UiEvent::TextDelta(index.to_string())));
            assert!(sent.is_ok());
        }
        tokio::task::yield_now().await;
        assert_eq!(draws.load(Ordering::Relaxed), 1);

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(draws.load(Ordering::Relaxed), 1);

        tokio::time::sleep(Duration::from_millis(COALESCE_WINDOW_MS + 20)).await;
        assert_eq!(draws.load(Ordering::Relaxed), 2);

        let sent = tx.send(AppEvent::Quit);
        assert!(sent.is_ok());
        tokio::task::yield_now().await;
        assert!(matches!(join.await, Ok(Ok(()))));
    }

    #[tokio::test]
    async fn coalescing_flushes_on_mouse_event() {
        let (tx, draws, join) = spawn_counting_app().await;

        let delta_sent = tx.send(AppEvent::UiEvent(crate::types::UiEvent::TextDelta("hi".into())));
        assert!(delta_sent.is_ok());

        tokio::time::sleep(Duration::from_millis(10)).await;
        let mouse_sent = tx.send(AppEvent::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(mouse_sent.is_ok());
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(draws.load(Ordering::Relaxed), 2);

        let quit_sent = tx.send(AppEvent::Quit);
        assert!(quit_sent.is_ok());
        tokio::task::yield_now().await;
        assert!(matches!(join.await, Ok(Ok(()))));
    }

    #[tokio::test]
    async fn coalescing_expires_after_window() {
        let (tx, draws, join) = spawn_counting_app().await;

        let delta_sent = tx.send(AppEvent::UiEvent(crate::types::UiEvent::TextDelta("hi".into())));
        assert!(delta_sent.is_ok());

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(draws.load(Ordering::Relaxed), 1);

        tokio::time::sleep(Duration::from_millis(COALESCE_WINDOW_MS + 20)).await;
        assert_eq!(draws.load(Ordering::Relaxed), 2);

        let quit_sent = tx.send(AppEvent::Quit);
        assert!(quit_sent.is_ok());
        tokio::task::yield_now().await;
        assert!(matches!(join.await, Ok(Ok(()))));
    }
}
