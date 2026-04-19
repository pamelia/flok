use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers, MouseEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    widgets::{Block, Borders},
};
use tokio::sync::mpsc;

use crate::app_event::AppEvent;
use crate::bottom_pane::BottomPane;
use crate::chat_view::ChatView;
use crate::footer::FooterState;
use crate::history::{ActiveItem, HistoryItem, Role, TeamEventKind};
use crate::sidebar::SidebarState;
use crate::tui::Tui;
use crate::types::{TuiChannels, UiCommand};

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

    terminal_size: (u16, u16),
    waiting_for_response: bool,

    // Theme
    theme: crate::theme::Theme,

    // Lifecycle
    running: bool,
    dirty: bool,
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
            terminal_size: (80, 24),
            waiting_for_response: false,
            theme,
            running: true,
            dirty: true,
        }
    }

    pub(crate) async fn run(&mut self, tui: &mut Tui) -> Result<()> {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        tui.draw(|frame| self.render(frame))?;
        self.dirty = false;

        while self.running {
            tokio::select! {
                biased;

                Some(event) = self.app_event_rx.recv() => {
                    self.handle_event(event);
                }

                Some(ui_event) = self.channels.ui_rx.recv() => {
                    self.handle_event(crate::adapter::from_ui_event(ui_event));
                }

                result = self.channels.bus_rx.recv() => {
                    match result {
                        Ok(bus_event) => {
                            if let Some(event) = crate::adapter::from_bus_event(bus_event) {
                                self.handle_event(event);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("bus broadcast lagged, skipped {n} events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            self.running = false;
                        }
                    }
                }

                Some(req) = self.channels.perm_rx.recv() => {
                    self.handle_event(crate::adapter::from_permission_request(req));
                }

                Some(req) = self.channels.question_rx.recv() => {
                    self.handle_event(crate::adapter::from_question_request(req));
                }

                _ = tick.tick() => {
                    self.handle_event(AppEvent::Tick);
                }
            }

            while let Ok(event) = self.app_event_rx.try_recv() {
                self.handle_event(event);
            }

            if self.dirty {
                tui.draw(|frame| self.render(frame))?;
                self.dirty = false;
            }
        }

        Ok(())
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
                    && matches!(key.code, KeyCode::Char('b'))
                {
                    self.sidebar.visible = !self.sidebar.visible;
                } else if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char('c' | 'd'))
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
            AppEvent::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.scroll_chat(-3);
                    self.dirty = true;
                }
                MouseEventKind::ScrollDown => {
                    self.scroll_chat(3);
                    self.dirty = true;
                }
                _ => {}
            },
            AppEvent::Paste(s) => {
                self.bottom_pane.handle_paste(&s);
                self.dirty = true;
            }
            AppEvent::UiEvent(ui_event) => match ui_event {
                crate::types::UiEvent::TextDelta(text) => {
                    self.dirty |= crate::stream::ingest_assistant_delta(&mut self.active, &text);
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
                    self.dirty = true;
                }
                crate::types::UiEvent::Error(message) => {
                    self.history.push(HistoryItem::system_error(message));
                    self.chat_view.on_new_content();
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
                    self.dirty = true;
                }
            },
            AppEvent::BusEvent(bus_event) => match bus_event {
                flok_core::bus::BusEvent::TextDelta { delta, .. } => {
                    self.dirty |= crate::stream::ingest_assistant_delta(&mut self.active, &delta);
                }
                flok_core::bus::BusEvent::ReasoningDelta { delta, .. } => {
                    self.dirty |= crate::stream::ingest_reasoning_delta(&mut self.active, &delta);
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
                flok_core::bus::BusEvent::TeamCreated { team_name, .. } => {
                    self.history.push(HistoryItem::TeamEvent {
                        kind: TeamEventKind::Created,
                        agent: team_name,
                        detail: "created".to_string(),
                    });
                    self.chat_view.on_new_content();
                    self.dirty = true;
                }
                flok_core::bus::BusEvent::TeamMemberCompleted { agent_name, .. } => {
                    self.history.push(HistoryItem::TeamEvent {
                        kind: TeamEventKind::Completed,
                        agent: agent_name,
                        detail: "completed".to_string(),
                    });
                    self.chat_view.on_new_content();
                    self.dirty = true;
                }
                flok_core::bus::BusEvent::TeamMemberFailed { agent_name, error, .. } => {
                    self.history.push(HistoryItem::TeamEvent {
                        kind: TeamEventKind::Failed,
                        agent: agent_name,
                        detail: error,
                    });
                    self.chat_view.on_new_content();
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
                            } else {
                                let _ = self.channels.cmd_tx.send(UiCommand::SetLabel(rest));
                            }
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
                                "Slash: /new /clear /undo /redo /tree /branch /label /plan /build /sidebar /sessions /help /quit",
                            ));
                            self.chat_view.on_new_content();
                        }
                        "" => {}
                        _ => {
                            self.history.push(HistoryItem::system_warn(format!(
                                "Unknown command: /{cmdline}"
                            )));
                            self.chat_view.on_new_content();
                        }
                    }
                } else if !trimmed.is_empty() {
                    self.history.push(HistoryItem::user(text.clone()));
                    self.waiting_for_response = true;
                    self.footer.waiting = true;
                    self.bottom_pane.set_waiting(true);
                    self.chat_view.on_new_content();
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

    fn render(&self, frame: &mut ratatui::Frame<'_>) {
        let area = frame.area();
        if area.width == 0 || area.height == 0 {
            return;
        }

        let layout = self.compute_layout(area);

        if let Some(sidebar) = layout.sidebar {
            crate::sidebar::render(&self.sidebar, sidebar, frame.buffer_mut(), &self.theme);
        }

        let chat_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ratatui_color(self.theme.border)))
            .title(format!(" flok — {} ", self.footer.model));
        let chat_inner = chat_block.inner(layout.chat);
        frame.render_widget(chat_block, layout.chat);

        self.chat_view.render(
            &self.history,
            self.active.as_ref(),
            &self.theme,
            chat_inner,
            frame.buffer_mut(),
        );
        self.bottom_pane.render(layout.bottom, frame.buffer_mut(), &self.theme);
        crate::footer::render(&self.footer, &self.theme, layout.footer, frame.buffer_mut());
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

        AppLayout { chat: rows[0], bottom: rows[1], footer: rows[2], sidebar }
    }

    fn finish_assistant(&mut self, fallback_text: String, cancelled: bool) {
        let item = crate::stream::finalize_assistant(self.active.take(), fallback_text, cancelled);
        self.push_history_if_not_duplicate(item);
        self.waiting_for_response = false;
        self.footer.waiting = false;
        self.bottom_pane.set_waiting(false);
        self.chat_view.on_new_content();
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
                    .chat
                    .height;
                let delta = -i32::from(viewport.max(2) / 2);
                self.scroll_chat(delta);
                true
            }
            (KeyCode::PageDown, _) => {
                let viewport = self
                    .compute_layout(Rect::new(0, 0, self.terminal_size.0, self.terminal_size.1))
                    .chat
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
                    .chat
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
        self.chat_view.handle_scroll(delta, layout.chat.height, self.transcript_height());
    }

    fn transcript_height(&self) -> usize {
        let layout =
            self.compute_layout(Rect::new(0, 0, self.terminal_size.0, self.terminal_size.1));
        let width = layout.chat.width.saturating_sub(2).max(1);
        let mut total = self
            .history
            .iter()
            .map(|item| usize::from(crate::history::render::height(item, width, &self.theme)))
            .sum::<usize>();
        if let Some(active) = &self.active {
            let synthetic = match active.role {
                Role::ToolCall => HistoryItem::ToolCall {
                    name: active.tool_name.clone().unwrap_or_default(),
                    preview: active.streaming_text.clone(),
                    is_error: false,
                    duration_ms: None,
                },
                _ => HistoryItem::assistant(active.streaming_text.clone(), true),
            };
            total += usize::from(crate::history::render::height(&synthetic, width, &self.theme));
        }
        total
    }
}

#[derive(Clone, Copy)]
struct AppLayout {
    chat: Rect,
    bottom: Rect,
    footer: Rect,
    sidebar: Option<Rect>,
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
    use tokio::sync::{broadcast, mpsc};

    use flok_core::session::PlanMode;
    use flok_core::tool::{PermissionRequest, QuestionRequest, TodoList};

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
}
