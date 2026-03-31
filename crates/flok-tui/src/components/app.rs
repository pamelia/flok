//! Root application component.

use iocraft::components::TextInputHandle;
use iocraft::prelude::*;
use tokio::sync::{broadcast, mpsc};

use crate::theme::Theme;
use crate::types::{TuiChannels, UiCommand, UiEvent};

use super::footer::StatusBar;
use super::input::InputBox;
use super::messages::MessageList;
use super::permission::PermissionPrompt as PermissionPromptView;
use super::question::QuestionPromptInline as QuestionPromptView;
use super::selection::{self, PanelRects, SelectionState};
use super::selection_overlay::SelectionOverlay;
use super::sidebar::Sidebar;

/// Default lines per mouse-wheel scroll tick.
const SCROLL_STEP: i32 = 3;

/// Props for the root app. Channels are `Option` so they can be `.take()`-en
/// on the first render inside `use_future`.
#[derive(Default, Props)]
pub struct FlokAppProps {
    pub cmd_tx: Option<mpsc::UnboundedSender<UiCommand>>,
    pub ui_rx: Option<mpsc::UnboundedReceiver<UiEvent>>,
    pub bus_rx: Option<broadcast::Receiver<flok_core::bus::BusEvent>>,
    pub perm_rx: Option<mpsc::UnboundedReceiver<flok_core::tool::PermissionRequest>>,
    pub question_rx: Option<mpsc::UnboundedReceiver<flok_core::tool::QuestionRequest>>,
    pub model_name: String,
}

#[component]
fn FlokApp(mut hooks: Hooks, props: &mut FlokAppProps) -> impl Into<AnyElement<'static>> {
    let theme = Theme::dark();
    let (term_width, term_height) = hooks.use_terminal_size();

    // ── Core state ──────────────────────────────────────────────────────
    let mut messages: State<Vec<DisplayMessage>> = hooks.use_state(Vec::new);
    let mut input_text: State<String> = hooks.use_state(String::new);
    let mut waiting = hooks.use_state(|| false);
    let mut streaming_text: State<String> = hooks.use_state(String::new);
    let mut sidebar_open = hooks.use_state(|| true);
    let mut session_title: State<String> = hooks.use_state(|| "New Session".to_string());
    let mut is_plan = hooks.use_state(|| false);
    let mut input_tokens: State<u64> = hooks.use_state(|| 0);
    let mut output_tokens: State<u64> = hooks.use_state(|| 0);
    let mut session_cost: State<f64> = hooks.use_state(|| 0.0);
    let mut active_tool: State<Option<String>> = hooks.use_state(|| None);
    let mut streaming_reasoning: State<String> = hooks.use_state(String::new);
    let mut should_exit = hooks.use_state(|| false);
    let mut context_pct: State<f64> = hooks.use_state(|| 0.0);
    let mut paste_indicator: State<Option<String>> = hooks.use_state(|| None);
    let mut toast: State<Option<String>> = hooks.use_state(|| None);

    // TextInput handle for cursor control
    let mut input_handle = hooks.use_ref(TextInputHandle::default);

    // ── Scroll handles ──────────────────────────────────────────────────
    let mut msg_scroll = hooks.use_ref(ScrollViewHandle::default);
    let mut sidebar_scroll = hooks.use_ref(ScrollViewHandle::default);
    let msg_scrolled_up: State<bool> = hooks.use_state(|| false);

    // ── Selection state ─────────────────────────────────────────────────
    // ── Input history ────────────────────────────────────────────────────
    let mut input_history: State<Vec<String>> = hooks.use_state(Vec::new);
    // Index into history: history.len() means "not browsing" (showing draft).
    let mut history_idx: State<usize> = hooks.use_state(|| 0);
    // Saved draft text when browsing history.
    let mut history_draft: State<String> = hooks.use_state(String::new);

    let mut selection: State<Option<SelectionState>> = hooks.use_state(|| None);
    // Click tracking for double/triple-click detection (300ms threshold).
    let mut last_click_time: State<Option<u64>> = hooks.use_state(|| None); // millis since epoch
    let mut last_click_pos: State<(u16, u16)> = hooks.use_state(|| (0, 0));
    let mut click_count: State<u8> = hooks.use_state(|| 0);
    // Text extracted from the canvas by SelectionOverlay during draw().
    // Uses Ref (not State) to avoid deadlock — writing State inside draw()
    // triggers a re-render which re-enters the render pass.
    let extracted_text: Ref<String> = hooks.use_ref(String::new);

    // ── Overlay state ───────────────────────────────────────────────────
    let mut palette_visible = hooks.use_state(|| false);
    let mut palette_selected: State<usize> = hooks.use_state(|| 0);
    let mut palette_filter: State<String> = hooks.use_state(String::new);

    let mut model_picker_visible = hooks.use_state(|| false);
    let mut model_picker_selected: State<usize> = hooks.use_state(|| 0);
    let mut model_picker_filter: State<String> = hooks.use_state(String::new);

    let mut perm_tool: State<String> = hooks.use_state(String::new);
    let mut perm_desc: State<String> = hooks.use_state(String::new);
    let mut perm_always_pattern: State<String> = hooks.use_state(String::new);
    let mut perm_visible = hooks.use_state(|| false);
    let mut perm_selected: State<u8> = hooks.use_state(|| 0);
    let mut perm_response: State<
        Option<tokio::sync::oneshot::Sender<flok_core::tool::PermissionDecision>>,
    > = hooks.use_state(|| None);

    let mut question_text: State<String> = hooks.use_state(String::new);
    let mut question_options: State<Vec<String>> = hooks.use_state(Vec::new);
    let mut question_visible = hooks.use_state(|| false);
    let mut question_selected: State<usize> = hooks.use_state(|| 0);
    let mut question_response: State<Option<tokio::sync::oneshot::Sender<String>>> =
        hooks.use_state(|| None);

    // Team state for sidebar
    let mut team_name: State<String> = hooks.use_state(String::new);
    let mut team_members: State<Vec<super::sidebar::TeamMemberInfo>> = hooks.use_state(Vec::new);

    let model_name = props.model_name.clone();
    let cmd_tx = props.cmd_tx.clone();
    let show_sidebar = sidebar_open.get() && term_width >= 120;

    // ── Compute panel rects for mouse routing ───────────────────────────
    // Estimate input height (3 lines + 1 margin).
    let input_h: u16 = 4;
    let panel_rects =
        selection::compute_panel_rects(term_width, term_height, show_sidebar, input_h);

    // ── Channel polling futures ─────────────────────────────────────────
    let mut ui_rx_ref = hooks.use_ref(|| props.ui_rx.take());
    let mut bus_rx_ref = hooks.use_ref(|| props.bus_rx.take());
    let mut perm_rx_ref = hooks.use_ref(|| props.perm_rx.take());
    let mut question_rx_ref = hooks.use_ref(|| props.question_rx.take());

    // Poll UI events
    hooks.use_future(async move {
        let Some(mut ui_rx) = ui_rx_ref.write().take() else {
            return;
        };
        loop {
            match ui_rx.recv().await {
                Some(UiEvent::TextDelta(delta)) => {
                    streaming_text.write().push_str(&delta);
                }
                Some(UiEvent::AssistantDone(text)) => {
                    streaming_text.set(String::new());
                    streaming_reasoning.set(String::new());
                    messages
                        .write()
                        .push(DisplayMessage { role: MessageRole::Assistant, content: text });
                    waiting.set(false);
                }
                Some(UiEvent::Cancelled(partial)) => {
                    streaming_text.set(String::new());
                    streaming_reasoning.set(String::new());
                    if !partial.is_empty() {
                        messages.write().push(DisplayMessage {
                            role: MessageRole::Assistant,
                            content: partial,
                        });
                    }
                    messages.write().push(DisplayMessage {
                        role: MessageRole::System,
                        content: "(cancelled)".into(),
                    });
                    waiting.set(false);
                }
                Some(UiEvent::HistoryMessage { role, content }) => {
                    let msg_role = match role.as_str() {
                        "user" => MessageRole::User,
                        "assistant" => MessageRole::Assistant,
                        _ => MessageRole::System,
                    };
                    if msg_role == MessageRole::User {
                        // Populate input history from past user messages.
                        input_history.write().push(content.clone());
                        history_idx.set(input_history.read().len());

                        if *session_title.read() == "New Session" {
                            let title = if content.len() > 40 {
                                format!("{}...", &content[..37])
                            } else {
                                content.clone()
                            };
                            session_title.set(title);
                        }
                    }
                    messages.write().push(DisplayMessage { role: msg_role, content });
                }
                Some(UiEvent::Error(e)) => {
                    streaming_text.set(String::new());
                    streaming_reasoning.set(String::new());
                    messages.write().push(DisplayMessage {
                        role: MessageRole::System,
                        content: format!("Error: {e}"),
                    });
                    waiting.set(false);
                }
                None => break,
            }
        }
    });

    // Poll bus events
    hooks.use_future(async move {
        let Some(mut bus_rx) = bus_rx_ref.write().take() else {
            return;
        };
        loop {
            match bus_rx.recv().await {
                Ok(flok_core::bus::BusEvent::TextDelta { delta, .. }) => {
                    streaming_text.write().push_str(&delta);
                }
                Ok(flok_core::bus::BusEvent::ReasoningDelta { delta, .. }) => {
                    streaming_reasoning.write().push_str(&delta);
                }
                Ok(flok_core::bus::BusEvent::TokenUsage {
                    input_tokens: inp,
                    output_tokens: out,
                    ..
                }) => {
                    input_tokens.set(input_tokens.get() + inp);
                    output_tokens.set(output_tokens.get() + out);
                }
                Ok(flok_core::bus::BusEvent::CostUpdate { total_cost_usd, .. }) => {
                    session_cost.set(total_cost_usd);
                }
                Ok(flok_core::bus::BusEvent::ToolCallStarted { tool_name, .. }) => {
                    active_tool.set(Some(tool_name.clone()));
                    messages.write().push(DisplayMessage {
                        role: MessageRole::ToolCall,
                        content: format!("\u{2699} {tool_name}"),
                    });
                    if tool_name.starts_with("task:") && tool_name.matches(':').count() >= 2 {
                        let parts: Vec<&str> = tool_name.splitn(3, ':').collect();
                        if parts.len() == 3 {
                            let agent_name = parts[2].to_string();
                            let mut members = team_members.read().clone();
                            if !members.iter().any(|m| m.name == agent_name) {
                                members.push(super::sidebar::TeamMemberInfo {
                                    name: agent_name,
                                    status: super::sidebar::TeamMemberStatus::Running,
                                });
                                team_members.set(members);
                            }
                        }
                    }
                }
                Ok(flok_core::bus::BusEvent::ToolCallCompleted { tool_name, is_error, .. }) => {
                    active_tool.set(None);
                    if let Some(last) = messages.write().last_mut() {
                        if last.role == MessageRole::ToolCall && last.content.contains(&tool_name) {
                            let icon = if is_error { "\u{2717}" } else { "\u{2713}" };
                            last.content = format!("{icon} {tool_name}");
                        }
                    }
                }
                Ok(flok_core::bus::BusEvent::ContextUsage { used_tokens, max_tokens, .. }) => {
                    if max_tokens > 0 {
                        context_pct.set(used_tokens as f64 / max_tokens as f64 * 100.0);
                    }
                }
                Ok(flok_core::bus::BusEvent::Error { message, .. }) => {
                    messages.write().push(DisplayMessage {
                        role: MessageRole::System,
                        content: format!("Error: {message}"),
                    });
                }
                Ok(flok_core::bus::BusEvent::TeamCreated { team_name: ref name, .. }) => {
                    team_name.set(name.clone());
                }
                Ok(flok_core::bus::BusEvent::TeamMemberCompleted { ref agent_name, .. }) => {
                    let mut members = team_members.read().clone();
                    if let Some(m) = members.iter_mut().find(|m| m.name == *agent_name) {
                        m.status = super::sidebar::TeamMemberStatus::Completed;
                    }
                    team_members.set(members);
                }
                Ok(flok_core::bus::BusEvent::TeamMemberFailed { ref agent_name, .. }) => {
                    let mut members = team_members.read().clone();
                    if let Some(m) = members.iter_mut().find(|m| m.name == *agent_name) {
                        m.status = super::sidebar::TeamMemberStatus::Failed;
                    }
                    team_members.set(members);
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Poll permission requests
    hooks.use_future(async move {
        let Some(mut rx) = perm_rx_ref.write().take() else {
            return;
        };
        while let Some(req) = rx.recv().await {
            perm_tool.set(req.tool);
            perm_desc.set(req.description);
            perm_always_pattern.set(req.always_pattern);
            perm_visible.set(true);
            perm_selected.set(0);
            perm_response.set(Some(req.response_tx));
        }
    });

    // Poll question requests
    hooks.use_future(async move {
        let Some(mut rx) = question_rx_ref.write().take() else {
            return;
        };
        while let Some(req) = rx.recv().await {
            question_text.set(req.question);
            question_options.set(req.options);
            question_visible.set(true);
            question_selected.set(0);
            question_response.set(Some(req.response_tx));
        }
    });

    // ── Toast auto-dismiss ──────────────────────────────────────────────
    hooks.use_future(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if toast.read().is_some() {
                toast.set(None);
            }
        }
    });

    // ── Keyboard + Mouse events ─────────────────────────────────────────
    hooks.use_terminal_events({
        let cmd_tx = cmd_tx.clone();
        move |event| {
            match event {
                // ── Mouse events (selection + per-panel scroll) ──────────
                TerminalEvent::FullscreenMouse(ref mouse) => {
                    handle_mouse(
                        mouse,
                        &mut selection,
                        &extracted_text,
                        &mut msg_scroll,
                        &mut sidebar_scroll,
                        &mut toast,
                        panel_rects,
                        &mut last_click_time,
                        &mut last_click_pos,
                        &mut click_count,
                    );
                }

                // ── Keyboard events ─────────────────────────────────────
                TerminalEvent::Key(KeyEvent { code, modifiers, kind, .. }) => {
                    if kind == KeyEventKind::Release {
                        return;
                    }
                    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
                    let alt = modifiers.contains(KeyModifiers::ALT);
                    let shift = modifiers.contains(KeyModifiers::SHIFT);

                    // If there is an active selection, Ctrl+C copies it.
                    let has_selection = {
                        let guard = selection.read();
                        guard.as_ref().is_some_and(SelectionState::has_extent)
                    };
                    if has_selection {
                        if ctrl && code == KeyCode::Char('c') {
                            let text = extracted_text.read().clone();
                            selection.set(None);
                            if !text.is_empty() && selection::copy_to_clipboard(&text) {
                                toast.set(Some("Copied to clipboard".into()));
                            }
                            return;
                        }
                        if code == KeyCode::Esc {
                            selection.set(None);
                            return;
                        }
                        // Any other key clears the selection and falls through.
                        selection.set(None);
                    }

                    // Model picker
                    if model_picker_visible.get() {
                        handle_model_picker(
                            code,
                            ctrl,
                            &mut model_picker_visible,
                            &mut model_picker_selected,
                            &mut model_picker_filter,
                            cmd_tx.as_ref(),
                            &mut messages,
                        );
                        return;
                    }

                    // Command palette
                    if palette_visible.get() {
                        handle_palette(
                            code,
                            ctrl,
                            &mut palette_visible,
                            &mut palette_selected,
                            &mut palette_filter,
                            cmd_tx.as_ref(),
                            &mut messages,
                            &mut sidebar_open,
                            &mut is_plan,
                            &mut should_exit,
                        );
                        return;
                    }

                    // Permission prompt
                    if perm_visible.get() {
                        handle_permission(
                            code,
                            &mut perm_visible,
                            &mut perm_selected,
                            &mut perm_response,
                        );
                        return;
                    }

                    // Question prompt
                    if question_visible.get() {
                        handle_question(
                            code,
                            ctrl,
                            &mut question_visible,
                            &mut question_selected,
                            &mut question_options,
                            &mut question_response,
                        );
                        return;
                    }

                    // ── Message scroll keybinds (opencode-style) ────────
                    // These work regardless of what has "focus" — the input
                    // always has focus, and these keybinds scroll the message
                    // area via dedicated Ctrl+Alt combos / PageUp / PageDown.
                    if handle_scroll_keybind(code, ctrl, alt, &mut msg_scroll) {
                        return;
                    }

                    // ── Normal keybinds ─────────────────────────────────
                    match code {
                        KeyCode::Esc if waiting.get() => {
                            if let Some(ref tx) = cmd_tx {
                                let _ = tx.send(UiCommand::Cancel);
                            }
                        }
                        KeyCode::Char('c' | 'd') if ctrl => {
                            if let Some(ref tx) = cmd_tx {
                                let _ = tx.send(UiCommand::Quit);
                            }
                            should_exit.set(true);
                        }
                        KeyCode::Char('b') if ctrl => {
                            sidebar_open.set(!sidebar_open.get());
                        }
                        KeyCode::Char('u') if ctrl => {
                            input_text.set(String::new());
                        }
                        KeyCode::Char('k') if ctrl => {
                            palette_visible.set(true);
                            palette_selected.set(0);
                            palette_filter.set(String::new());
                        }
                        KeyCode::Char('m') if ctrl => {
                            model_picker_visible.set(true);
                            model_picker_selected.set(0);
                            model_picker_filter.set(String::new());
                        }
                        KeyCode::Tab => {
                            is_plan.set(!is_plan.get());
                        }
                        KeyCode::Enter if shift => {
                            if !waiting.get() {
                                input_text.write().push('\n');
                            }
                        }
                        KeyCode::Enter => {
                            let text = input_text.read().clone();
                            if !text.is_empty() && !waiting.get() {
                                // Push to input history.
                                input_history.write().push(text.clone());
                                history_idx.set(input_history.read().len());
                                history_draft.set(String::new());

                                if *session_title.read() == "New Session" {
                                    let title = if text.len() > 40 {
                                        format!("{}...", &text[..37])
                                    } else {
                                        text.clone()
                                    };
                                    session_title.set(title);
                                }

                                if text.starts_with('/') {
                                    handle_slash_command(
                                        &text,
                                        &mut messages,
                                        &mut sidebar_open,
                                        &mut is_plan,
                                        cmd_tx.as_ref(),
                                        &mut should_exit,
                                    );
                                    input_text.set(String::new());
                                    return;
                                }

                                messages.write().push(DisplayMessage {
                                    role: MessageRole::User,
                                    content: text.clone(),
                                });
                                waiting.set(true);
                                streaming_text.set(String::new());
                                streaming_reasoning.set(String::new());
                                paste_indicator.set(None);
                                if let Some(ref tx) = cmd_tx {
                                    let _ = tx.send(UiCommand::SendMessage(text));
                                }
                                input_text.set(String::new());
                            }
                        }
                        // Readline keybinds
                        KeyCode::Char('w') if ctrl => {
                            if !waiting.get() {
                                let mut text = input_text.read().clone();
                                let trimmed = text.trim_end().len();
                                text.truncate(trimmed);
                                if let Some(pos) = text.rfind(|c: char| c.is_whitespace()) {
                                    text.truncate(pos + 1);
                                } else {
                                    text.clear();
                                }
                                input_text.set(text);
                            }
                        }
                        KeyCode::Char('a') if ctrl => {
                            input_handle.write().set_cursor_offset(0);
                        }
                        KeyCode::Char('e') if ctrl => {
                            let len = input_text.read().len();
                            input_handle.write().set_cursor_offset(len);
                        }
                        // Input history: Up/Down cycle through sent messages
                        // when input is single-line.
                        KeyCode::Up if !waiting.get() && !input_text.read().contains('\n') => {
                            let hist = input_history.read();
                            if !hist.is_empty() {
                                let idx = history_idx.get();
                                if idx == hist.len() {
                                    // Entering history — save current draft.
                                    history_draft.set(input_text.read().clone());
                                }
                                let new_idx = if idx == hist.len() {
                                    hist.len() - 1
                                } else {
                                    idx.saturating_sub(1)
                                };
                                if let Some(entry) = hist.get(new_idx) {
                                    input_text.set(entry.clone());
                                    let len = entry.len();
                                    drop(hist);
                                    input_handle.write().set_cursor_offset(len);
                                    history_idx.set(new_idx);
                                }
                            }
                        }
                        KeyCode::Down if !waiting.get() && !input_text.read().contains('\n') => {
                            let hist = input_history.read();
                            let idx = history_idx.get();
                            if idx < hist.len() {
                                let new_idx = idx + 1;
                                if new_idx >= hist.len() {
                                    // Back to draft.
                                    let draft = history_draft.read().clone();
                                    let len = draft.len();
                                    input_text.set(draft);
                                    drop(hist);
                                    input_handle.write().set_cursor_offset(len);
                                    history_idx.set(new_idx);
                                } else if let Some(entry) = hist.get(new_idx) {
                                    input_text.set(entry.clone());
                                    let len = entry.len();
                                    drop(hist);
                                    input_handle.write().set_cursor_offset(len);
                                    history_idx.set(new_idx);
                                }
                            }
                        }
                        _ => {}
                    }
                }

                TerminalEvent::Resize(..) => {
                    // Clear selection on resize — coordinates become stale
                    // after reflow (same approach as tmux).
                    selection.set(None);
                }
                _ => {}
            }
        }
    });

    // Handle exit
    let mut system = hooks.use_context_mut::<SystemContext>();
    if should_exit.get() {
        system.exit();
    }

    // ── Build element tree ──────────────────────────────────────────────
    let msgs = messages.read().clone();
    let stream = streaming_text.read().clone();
    let reasoning = streaming_reasoning.read().clone();
    let input = input_text.read().clone();
    let title = session_title.read().clone();
    let is_scrolled_up = msg_scrolled_up.get();
    let sel_for_overlay = selection.read().clone();

    element! {
        View(
            flex_direction: FlexDirection::Column,
            width: term_width,
            height: term_height,
            background_color: theme.bg,
        ) {
            // Main content row
            View(flex_direction: FlexDirection::Row, flex_grow: 1.0, overflow: Overflow::Hidden) {
                // Left panel (messages + input)
                View(
                    flex_direction: FlexDirection::Column,
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    padding_left: 2u32,
                    padding_right: 1u32,
                    overflow: Overflow::Hidden,
                ) {
                    // Messages area
                    MessageList(
                        messages: msgs,
                        streaming_text: stream,
                        streaming_reasoning: reasoning,
                        is_waiting: waiting.get(),
                        theme: theme,
                        scroll_handle: msg_scroll,
                        scrolled_up: msg_scrolled_up,
                    )

                    // Scroll-to-bottom indicator
                    #(if is_scrolled_up {
                        Some(element! {
                            View(
                                position: Position::Absolute,
                                bottom: 5i32,
                                right: 4i32,
                            ) {
                                Text(
                                    content: " \u{2193} New messages ",
                                    color: theme.bg,
                                    weight: Weight::Bold,
                                )
                            }
                        })
                    } else {
                        None
                    })

                    // Bottom area — pinned, never shrunk away
                    View(flex_shrink: 0.0, margin_bottom: 1u32) {
                        #(if perm_visible.get() {
                            Some(element! {
                                PermissionPromptView(
                                    tool: perm_tool.read().clone(),
                                    description: perm_desc.read().clone(),
                                    always_pattern: perm_always_pattern.read().clone(),
                                    selected: perm_selected.get(),
                                    theme: theme,
                                )
                            })
                        } else {
                            None
                        })
                        #(if question_visible.get() && !perm_visible.get() {
                            Some(element! {
                                QuestionPromptView(
                                    question: question_text.read().clone(),
                                    options: question_options.read().clone(),
                                    selected: question_selected.get(),
                                    theme: theme,
                                )
                            })
                        } else {
                            None
                        })
                        #(if !perm_visible.get() && !question_visible.get() {
                            Some(element! {
                                InputBox(
                                    text: input,
                                    is_waiting: waiting.get(),
                                    theme: theme,
                                    handle: input_handle,
                                    paste_indicator: paste_indicator.read().clone(),
                                    on_change: move |new_val: String| {
                                        let old_len = input_text.read().len();
                                        let new_len = new_val.len();
                                        if new_len > old_len + 2 {
                                            let added = &new_val[old_len..];
                                            let line_count = added.lines().count();
                                            if line_count > 1 {
                                                paste_indicator.set(Some(format!("[Pasted ~{line_count} lines]")));
                                            }
                                        }
                                        input_text.set(new_val);
                                    },
                                )
                            })
                        } else {
                            None
                        })
                    }
                }

                #(if show_sidebar {
                    Some(element! {
                        Sidebar(
                            session_title: title,
                            model_name: model_name.clone(),
                            input_tokens: input_tokens.get(),
                            output_tokens: output_tokens.get(),
                            session_cost: session_cost.get(),
                            context_pct: context_pct.get(),
                            is_plan: is_plan.get(),
                            team_name: team_name.read().clone(),
                            team_members: team_members.read().clone(),
                            theme: theme,
                            scroll_handle: sidebar_scroll,
                        )
                    })
                } else {
                    None
                })
            }

            // Status bar
            View(flex_shrink: 0.0) {
                StatusBar(
                    model_name: model_name.clone(),
                    is_plan: is_plan.get(),
                    theme: theme,
                )
            }

            // Toast overlay (top-right)
            #(toast.read().as_ref().map(|msg| {
                element! {
                    View(
                        position: Position::Absolute,
                        top: 0i32,
                        right: 2i32,
                        background_color: theme.info,
                    ) {
                        Text(
                            content: format!(" {msg} "),
                            color: theme.bg,
                            weight: Weight::Bold,
                        )
                    }
                }
            }))

            // Command palette overlay
            #(if palette_visible.get() {
                let commands = get_palette_commands();
                let filter = palette_filter.read().to_lowercase();
                let filtered: Vec<_> = commands
                    .iter()
                    .filter(|c| filter.is_empty() || c.0.to_lowercase().contains(&filter))
                    .collect();
                let sel = palette_selected.get().min(filtered.len().saturating_sub(1));

                Some(element! {
                    View(
                        position: Position::Absolute,
                        top: 0i32,
                        left: 0i32,
                        width: term_width,
                        height: term_height,
                        align_items: AlignItems::Center,
                        padding_top: 3u32,
                    ) {
                        View(
                            width: 50u32,
                            max_height: 18u32,
                            background_color: theme.bg_panel,
                            border_style: BorderStyle::Round,
                            border_color: theme.border,
                            flex_direction: FlexDirection::Column,
                            padding: 1u32,
                            overflow: Overflow::Hidden,
                        ) {
                            View(flex_direction: FlexDirection::Row, padding_bottom: 1u32) {
                                Text(content: "Commands", color: theme.text, weight: Weight::Bold)
                                View(flex_grow: 1.0) {}
                                Text(content: "esc", color: theme.text_muted)
                            }
                            View(
                                border_style: BorderStyle::Round,
                                border_color: theme.border,
                                padding_left: 1u32,
                            ) {
                                Text(
                                    content: if palette_filter.read().is_empty() {
                                        "Search commands...".to_string()
                                    } else {
                                        palette_filter.read().clone()
                                    },
                                    color: if palette_filter.read().is_empty() {
                                        theme.text_muted
                                    } else {
                                        theme.text
                                    },
                                )
                            }
                            View(flex_direction: FlexDirection::Column, padding_top: 1u32) {
                                #(filtered.iter().enumerate().map(|(i, (title, _action))| {
                                    let is_sel = i == sel;
                                    let bg = if is_sel { theme.primary } else { theme.bg_panel };
                                    let fg = if is_sel { theme.bg } else { theme.text };
                                    element! {
                                        View(key: i, background_color: bg, padding_left: 1u32) {
                                            Text(content: *title, color: fg, weight: Weight::Bold)
                                        }
                                    }
                                }))
                            }
                        }
                    }
                })
            } else {
                None
            })

            // Model picker overlay
            #(if model_picker_visible.get() {
                let models = get_model_list();
                let filter = model_picker_filter.read().to_lowercase();
                let filtered: Vec<_> = models
                    .iter()
                    .filter(|m| {
                        filter.is_empty()
                            || m.0.to_lowercase().contains(&filter)
                            || m.1.to_lowercase().contains(&filter)
                    })
                    .collect();
                let sel = model_picker_selected.get().min(filtered.len().saturating_sub(1));
                #[allow(unused_variables)]
                let current_provider = String::new();

                Some(element! {
                    View(
                        position: Position::Absolute,
                        top: 0i32,
                        left: 0i32,
                        width: term_width,
                        height: term_height,
                        align_items: AlignItems::Center,
                        padding_top: 3u32,
                    ) {
                        View(
                            width: 55u32,
                            max_height: 22u32,
                            background_color: theme.bg_panel,
                            border_style: BorderStyle::Round,
                            border_color: theme.border,
                            flex_direction: FlexDirection::Column,
                            padding: 1u32,
                            overflow: Overflow::Hidden,
                        ) {
                            View(flex_direction: FlexDirection::Row, padding_bottom: 1u32) {
                                Text(content: "Select model", color: theme.text, weight: Weight::Bold)
                                View(flex_grow: 1.0) {}
                                Text(content: "esc", color: theme.text_muted)
                            }
                            View(
                                border_style: BorderStyle::Round,
                                border_color: theme.border,
                                padding_left: 1u32,
                            ) {
                                Text(
                                    content: if model_picker_filter.read().is_empty() {
                                        "Search models...".to_string()
                                    } else {
                                        model_picker_filter.read().clone()
                                    },
                                    color: if model_picker_filter.read().is_empty() {
                                        theme.text_muted
                                    } else {
                                        theme.text
                                    },
                                )
                            }
                            ScrollView(auto_scroll: false) {
                                View(flex_direction: FlexDirection::Column, padding_top: 1u32) {
                                    #(filtered.iter().enumerate().map(|(i, (name, provider, _id))| {
                                        let is_sel = i == sel;
                                        let bg = if is_sel { theme.primary } else { theme.bg_panel };
                                        let fg = if is_sel { theme.bg } else { theme.text };
                                        let show_provider = *provider != current_provider.as_str();
                                        element! {
                                            View(key: i, flex_direction: FlexDirection::Column) {
                                                #(if show_provider && !filter.is_empty() {
                                                    None
                                                } else if show_provider {
                                                    Some(element! {
                                                        View(padding_top: 1u32) {
                                                            Text(
                                                                content: *provider,
                                                                color: theme.text_muted,
                                                                weight: Weight::Bold,
                                                            )
                                                        }
                                                    })
                                                } else {
                                                    None
                                                })
                                                View(
                                                    flex_direction: FlexDirection::Row,
                                                    background_color: bg,
                                                    padding_left: 1u32,
                                                ) {
                                                    Text(content: *name, color: fg, weight: Weight::Bold)
                                                    View(flex_grow: 1.0) {}
                                                    Text(
                                                        content: *provider,
                                                        color: if is_sel { theme.bg } else { theme.text_muted },
                                                    )
                                                }
                                            }
                                        }
                                    }))
                                }
                            }
                        }
                    }
                })
            } else {
                None
            })

            // Selection highlight overlay — draws LAST so background
            // paints on top of all other content without clearing text.
            SelectionOverlay(
                selection: sel_for_overlay,
                width: term_width,
                height: term_height,
                theme: theme,
                extracted_text: extracted_text,
            )
        }
    }
}

// ── Helper functions extracted from the keyboard handler ─────────────────

/// Handle mouse events: selection tracking + per-panel scroll routing.
#[allow(clippy::too_many_arguments)]
fn handle_mouse(
    mouse: &FullscreenMouseEvent,
    selection: &mut State<Option<SelectionState>>,
    extracted_text: &Ref<String>,
    msg_scroll: &mut Ref<ScrollViewHandle>,
    sidebar_scroll: &mut Ref<ScrollViewHandle>,
    toast: &mut State<Option<String>>,
    rects: PanelRects,
    last_click_time: &mut State<Option<u64>>,
    last_click_pos: &mut State<(u16, u16)>,
    click_count: &mut State<u8>,
) {
    match mouse.kind {
        MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let pos = (mouse.column, mouse.row);

            // Detect multi-click (300ms threshold, same position).
            let is_repeat = last_click_time.get().is_some_and(|t| now.saturating_sub(t) < 300)
                && last_click_pos.get() == pos;

            let count = if is_repeat { (click_count.get() % 3) + 1 } else { 1 };
            click_count.set(count);
            last_click_time.set(Some(now));
            last_click_pos.set(pos);

            if let Some(panel) = rects.identify(mouse.column, mouse.row) {
                let mut sel = SelectionState::start(panel, mouse.column, mouse.row);
                match count {
                    2 => {
                        // Double-click: word selection mode.
                        // The overlay will expand to word boundaries in draw().
                        sel.mode = selection::SelectionMode::Word;
                    }
                    3 => {
                        // Triple-click: line selection mode.
                        let pr = match panel {
                            selection::Panel::Messages => rects.messages,
                            selection::Panel::Sidebar => rects.sidebar.unwrap_or(rects.messages),
                            selection::Panel::Input => rects.input,
                        };
                        sel.mode = selection::SelectionMode::Line;
                        sel.anchor.0 = pr.x;
                        sel.cursor.0 = pr.x + pr.w.saturating_sub(1);
                    }
                    _ => {} // Single click: normal char selection
                }
                selection.set(Some(sel));
            }
        }
        MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
            if let Some(ref mut sel) = *selection.write() {
                let (c, r) = rects.clamp_to(sel.panel, mouse.column, mouse.row);
                sel.extend(c, r);

                // Auto-scroll when dragging near panel edges.
                let rect = match sel.panel {
                    selection::Panel::Messages => rects.messages,
                    selection::Panel::Sidebar => rects.sidebar.unwrap_or_default(),
                    selection::Panel::Input => return, // No scroll for input
                };
                let scroll = match sel.panel {
                    selection::Panel::Messages => &mut *msg_scroll,
                    selection::Panel::Sidebar => &mut *sidebar_scroll,
                    selection::Panel::Input => return,
                };
                if r <= rect.y {
                    scroll.write().scroll_by(-SCROLL_STEP);
                } else if r >= rect.y + rect.h.saturating_sub(1) {
                    scroll.write().scroll_by(SCROLL_STEP);
                }
            }
        }
        MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
            // The SelectionOverlay reads the canvas during draw() and stores
            // the selected text in `extracted_text`. We read it here.
            let text = extracted_text.read().clone();
            // Clear the selection (overlay will clear extracted_text on next draw).
            selection.set(None);

            tracing::debug!(
                text_len = text.len(),
                text_preview = &text[..text.len().min(80)],
                "selection copy from canvas"
            );

            if text.is_empty() {
                toast.set(Some("Selection empty".into()));
            } else if selection::copy_to_clipboard(&text) {
                toast.set(Some("Copied to clipboard".into()));
            } else {
                toast.set(Some("Clipboard error".into()));
            }
        }
        // Per-panel mouse wheel scrolling
        MouseEventKind::ScrollUp => {
            if let Some(panel) = rects.identify(mouse.column, mouse.row) {
                match panel {
                    selection::Panel::Messages => {
                        msg_scroll.write().scroll_by(-SCROLL_STEP);
                    }
                    selection::Panel::Sidebar => {
                        sidebar_scroll.write().scroll_by(-SCROLL_STEP);
                    }
                    selection::Panel::Input => {} // No scroll for input
                }
            }
        }
        MouseEventKind::ScrollDown => {
            if let Some(panel) = rects.identify(mouse.column, mouse.row) {
                match panel {
                    selection::Panel::Messages => {
                        msg_scroll.write().scroll_by(SCROLL_STEP);
                    }
                    selection::Panel::Sidebar => {
                        sidebar_scroll.write().scroll_by(SCROLL_STEP);
                    }
                    selection::Panel::Input => {}
                }
            }
        }
        _ => {}
    }
}

/// Handle keyboard scroll commands (opencode-style keybinds).
/// Returns `true` if the event was consumed.
fn handle_scroll_keybind(
    code: KeyCode,
    ctrl: bool,
    alt: bool,
    handle: &mut Ref<ScrollViewHandle>,
) -> bool {
    // PageUp / PageDown (no modifiers needed)
    match code {
        KeyCode::PageUp => {
            let mut h = handle.write();
            let vh = i32::from(h.viewport_height());
            h.scroll_by(-vh.max(1) / 2);
            return true;
        }
        KeyCode::PageDown => {
            let mut h = handle.write();
            let vh = i32::from(h.viewport_height());
            h.scroll_by(vh.max(1) / 2);
            return true;
        }
        KeyCode::Home if ctrl => {
            handle.write().scroll_to_top();
            return true;
        }
        KeyCode::End if ctrl => {
            handle.write().scroll_to_bottom();
            return true;
        }
        _ => {}
    }

    // Ctrl+Alt combos (matching opencode defaults)
    if ctrl && alt {
        match code {
            // Ctrl+Alt+Y — line up
            KeyCode::Char('y') => {
                handle.write().scroll_by(-1);
                return true;
            }
            // Ctrl+Alt+E — line down
            KeyCode::Char('e') => {
                handle.write().scroll_by(1);
                return true;
            }
            // Ctrl+Alt+U — half page up
            KeyCode::Char('u') => {
                let mut h = handle.write();
                let vh = i32::from(h.viewport_height());
                h.scroll_by(-vh.max(1) / 4);
                return true;
            }
            // Ctrl+Alt+D — half page down
            KeyCode::Char('d') => {
                let mut h = handle.write();
                let vh = i32::from(h.viewport_height());
                h.scroll_by(vh.max(1) / 4);
                return true;
            }
            // Ctrl+Alt+B — page up
            KeyCode::Char('b') => {
                let mut h = handle.write();
                let vh = i32::from(h.viewport_height());
                h.scroll_by(-vh.max(1) / 2);
                return true;
            }
            // Ctrl+Alt+F — page down
            KeyCode::Char('f') => {
                let mut h = handle.write();
                let vh = i32::from(h.viewport_height());
                h.scroll_by(vh.max(1) / 2);
                return true;
            }
            // Ctrl+Alt+G — scroll to bottom
            KeyCode::Char('g') => {
                handle.write().scroll_to_bottom();
                return true;
            }
            _ => {}
        }
    }

    // Ctrl+G — scroll to top (opencode: messages_first)
    if ctrl && !alt && code == KeyCode::Char('g') {
        handle.write().scroll_to_top();
        return true;
    }

    false
}

fn handle_model_picker(
    code: KeyCode,
    ctrl: bool,
    visible: &mut State<bool>,
    selected: &mut State<usize>,
    filter: &mut State<String>,
    cmd_tx: Option<&mpsc::UnboundedSender<UiCommand>>,
    messages: &mut State<Vec<DisplayMessage>>,
) {
    let models = get_model_list();
    let filt = filter.read().to_lowercase();
    let filtered: Vec<_> = models
        .iter()
        .filter(|m| {
            filt.is_empty()
                || m.0.to_lowercase().contains(&filt)
                || m.1.to_lowercase().contains(&filt)
        })
        .collect();
    let max_idx = filtered.len().saturating_sub(1);

    match code {
        KeyCode::Up => selected.set(selected.get().saturating_sub(1)),
        KeyCode::Down => selected.set((selected.get() + 1).min(max_idx)),
        KeyCode::Enter => {
            let sel = selected.get().min(max_idx);
            if let Some(&&(_, _, id)) = filtered.get(sel) {
                visible.set(false);
                filter.set(String::new());
                if let Some(tx) = cmd_tx {
                    let _ = tx.send(UiCommand::SwitchModel(id.to_string()));
                }
                messages.write().push(DisplayMessage {
                    role: MessageRole::System,
                    content: format!("Switched model to {id}"),
                });
            }
        }
        KeyCode::Esc => {
            visible.set(false);
            filter.set(String::new());
        }
        KeyCode::Backspace => {
            filter.write().pop();
        }
        KeyCode::Char(c) if !ctrl => {
            filter.write().push(c);
            selected.set(0);
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_palette(
    code: KeyCode,
    ctrl: bool,
    visible: &mut State<bool>,
    selected: &mut State<usize>,
    filter: &mut State<String>,
    cmd_tx: Option<&mpsc::UnboundedSender<UiCommand>>,
    messages: &mut State<Vec<DisplayMessage>>,
    sidebar_open: &mut State<bool>,
    is_plan: &mut State<bool>,
    should_exit: &mut State<bool>,
) {
    match code {
        KeyCode::Up => selected.set(selected.get().saturating_sub(1)),
        KeyCode::Down => selected.set(selected.get() + 1),
        KeyCode::Enter => {
            let sel = selected.get();
            let commands = get_palette_commands();
            let filt = filter.read().to_lowercase();
            let filtered: Vec<_> = commands
                .iter()
                .filter(|c| filt.is_empty() || c.0.to_lowercase().contains(&filt))
                .collect();
            if let Some(&&(_, action)) = filtered.get(sel) {
                visible.set(false);
                filter.set(String::new());
                match action {
                    "quit" => {
                        if let Some(tx) = cmd_tx {
                            let _ = tx.send(UiCommand::Quit);
                        }
                        should_exit.set(true);
                    }
                    "new" => {
                        messages.write().clear();
                        messages.write().push(DisplayMessage {
                            role: MessageRole::System,
                            content: "Session cleared.".into(),
                        });
                    }
                    "sidebar" => sidebar_open.set(!sidebar_open.get()),
                    "plan" => is_plan.set(true),
                    "build" => is_plan.set(false),
                    "sessions" => {
                        if let Some(tx) = cmd_tx {
                            let _ = tx.send(UiCommand::ListSessions);
                        }
                    }
                    "help" => {
                        messages.write().push(DisplayMessage {
                            role: MessageRole::System,
                            content: "Commands: /quit /new /plan /build /sidebar /sessions /help\n\
                                     Keys: Ctrl+C quit  Ctrl+K commands  Tab plan/build  Ctrl+B sidebar  Ctrl+U clear".into(),
                        });
                    }
                    _ => {}
                }
            }
        }
        KeyCode::Esc => {
            visible.set(false);
            filter.set(String::new());
        }
        KeyCode::Backspace => {
            filter.write().pop();
        }
        KeyCode::Char(c) if !ctrl => {
            filter.write().push(c);
            selected.set(0);
        }
        _ => {}
    }
}

fn handle_permission(
    code: KeyCode,
    visible: &mut State<bool>,
    selected: &mut State<u8>,
    response: &mut State<Option<tokio::sync::oneshot::Sender<flok_core::tool::PermissionDecision>>>,
) {
    match code {
        KeyCode::Left | KeyCode::Char('h') => {
            selected.set(selected.get().saturating_sub(1));
        }
        KeyCode::Right | KeyCode::Char('l') => {
            selected.set((selected.get() + 1).min(2));
        }
        KeyCode::Enter => {
            use flok_core::tool::PermissionDecision;
            let decision = match selected.get() {
                0 => PermissionDecision::Allow,
                1 => PermissionDecision::Always,
                _ => PermissionDecision::Deny,
            };
            if let Some(tx) = response.write().take() {
                let _ = tx.send(decision);
            }
            visible.set(false);
        }
        KeyCode::Esc => {
            if let Some(tx) = response.write().take() {
                let _ = tx.send(flok_core::tool::PermissionDecision::Deny);
            }
            visible.set(false);
        }
        _ => {}
    }
}

fn handle_question(
    code: KeyCode,
    ctrl: bool,
    visible: &mut State<bool>,
    selected: &mut State<usize>,
    options: &mut State<Vec<String>>,
    response: &mut State<Option<tokio::sync::oneshot::Sender<String>>>,
) {
    let opt_count = options.read().len();
    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            selected.set(selected.get().saturating_sub(1));
        }
        KeyCode::Down | KeyCode::Char('j') => {
            selected.set((selected.get() + 1).min(opt_count.saturating_sub(1)));
        }
        KeyCode::Enter => {
            let answer = options.read().get(selected.get()).cloned().unwrap_or_default();
            if let Some(tx) = response.write().take() {
                let _ = tx.send(answer);
            }
            visible.set(false);
        }
        KeyCode::Esc => {
            if let Some(tx) = response.write().take() {
                let _ = tx.send("(dismissed)".to_string());
            }
            visible.set(false);
        }
        KeyCode::Char(c @ '1'..='9') if !ctrl => {
            let idx = (c as usize) - ('1' as usize);
            if idx < opt_count {
                selected.set(idx);
                let answer = options.read().get(idx).cloned().unwrap_or_default();
                if let Some(tx) = response.write().take() {
                    let _ = tx.send(answer);
                }
                visible.set(false);
            }
        }
        _ => {}
    }
}

fn handle_slash_command(
    input: &str,
    messages: &mut State<Vec<DisplayMessage>>,
    sidebar_open: &mut State<bool>,
    is_plan: &mut State<bool>,
    cmd_tx: Option<&mpsc::UnboundedSender<UiCommand>>,
    should_exit: &mut State<bool>,
) {
    let trimmed = input.trim();
    match trimmed {
        "/quit" | "/exit" | "/q" => {
            if let Some(tx) = cmd_tx {
                let _ = tx.send(UiCommand::Quit);
            }
            should_exit.set(true);
        }
        "/help" => {
            messages.write().push(DisplayMessage {
                role: MessageRole::System,
                content: "Commands: /quit /new /undo /redo /model /plan /build /sidebar /sessions /help\n\
                         Keys: Ctrl+C quit  Tab plan/build  Ctrl+B sidebar  Ctrl+U clear input"
                    .into(),
            });
        }
        "/new" | "/clear" => {
            messages.write().clear();
            messages.write().push(DisplayMessage {
                role: MessageRole::System,
                content: "Session cleared.".into(),
            });
        }
        "/sessions" => {
            if let Some(tx) = cmd_tx {
                let _ = tx.send(UiCommand::ListSessions);
            }
        }
        "/undo" => {
            if let Some(tx) = cmd_tx {
                let _ = tx.send(UiCommand::Undo);
            }
        }
        "/redo" => {
            if let Some(tx) = cmd_tx {
                let _ = tx.send(UiCommand::Redo);
            }
        }
        "/sidebar" => sidebar_open.set(!sidebar_open.get()),
        "/plan" => is_plan.set(true),
        "/build" => is_plan.set(false),
        "/model" => {
            messages.write().push(DisplayMessage {
                role: MessageRole::System,
                content: "Model switching not yet implemented.".into(),
            });
        }
        _ => {
            messages.write().push(DisplayMessage {
                role: MessageRole::System,
                content: format!("Unknown command: {trimmed}. Type /help"),
            });
        }
    }
}

/// Available models for the model picker.
fn get_model_list() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("Claude Opus 4.6", "Anthropic", "anthropic/claude-opus-4-6"),
        ("Claude Sonnet 4.6", "Anthropic", "anthropic/claude-sonnet-4-6"),
        ("Claude Haiku 4.5", "Anthropic", "anthropic/claude-haiku-4-5-20251001"),
        ("Claude Sonnet 4 (legacy)", "Anthropic", "anthropic/claude-sonnet-4-20250514"),
        ("Claude Opus 4 (legacy)", "Anthropic", "anthropic/claude-opus-4-20250514"),
        ("GPT-4.1", "OpenAI", "openai/gpt-4.1"),
        ("GPT-4.1 Mini", "OpenAI", "openai/gpt-4.1-mini"),
        ("Gemini 2.5 Flash", "Google", "google/gemini-2.5-flash"),
        ("Gemini 2.5 Pro", "Google", "google/gemini-2.5-pro"),
        ("DeepSeek V3", "DeepSeek", "deepseek/deepseek-chat"),
        ("DeepSeek R1", "DeepSeek", "deepseek/deepseek-reasoner"),
    ]
}

/// Commands available in the command palette.
fn get_palette_commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("New session", "new"),
        ("Switch to Plan mode", "plan"),
        ("Switch to Build mode", "build"),
        ("Toggle sidebar", "sidebar"),
        ("List sessions", "sessions"),
        ("Help", "help"),
        ("Exit", "quit"),
    ]
}

/// Display message for the conversation view.
#[derive(Debug, Clone)]
pub struct DisplayMessage {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    ToolCall,
}

/// Run the interactive TUI.
pub async fn run_app(channels: TuiChannels) -> anyhow::Result<()> {
    let cmd_tx = channels.cmd_tx;
    let ui_rx = channels.ui_rx;
    let bus_rx = channels.bus_rx;
    let model_name = channels.model_name;

    let perm_rx = channels.perm_rx;
    let question_rx = channels.question_rx;

    element! {
        FlokApp(
            cmd_tx: cmd_tx,
            ui_rx: ui_rx,
            bus_rx: bus_rx,
            perm_rx: perm_rx,
            question_rx: question_rx,
            model_name: model_name,
        )
    }
    .render_loop()
    .fullscreen()
    // Mouse capture is enabled by default in fullscreen mode.
    // This enables per-panel mouse wheel scrolling and text selection.
    .await?;

    Ok(())
}
