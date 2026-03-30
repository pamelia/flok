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
use super::sidebar::Sidebar;

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

    // State
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
    let toast: State<Option<String>> = hooks.use_state(|| None);

    // TextInput handle for cursor control
    let mut input_handle = hooks.use_ref(TextInputHandle::default);

    // Command palette state
    let mut palette_visible = hooks.use_state(|| false);
    let mut palette_selected: State<usize> = hooks.use_state(|| 0);
    let mut palette_filter: State<String> = hooks.use_state(String::new);

    // Model picker state
    let mut model_picker_visible = hooks.use_state(|| false);
    let mut model_picker_selected: State<usize> = hooks.use_state(|| 0);
    let mut model_picker_filter: State<String> = hooks.use_state(String::new);

    // Permission prompt state
    let mut perm_tool: State<String> = hooks.use_state(String::new);
    let mut perm_desc: State<String> = hooks.use_state(String::new);
    let mut perm_visible = hooks.use_state(|| false);
    let mut perm_selected: State<u8> = hooks.use_state(|| 0);
    let mut perm_response: State<
        Option<tokio::sync::oneshot::Sender<flok_core::tool::PermissionDecision>>,
    > = hooks.use_state(|| None);

    // Question prompt state
    let mut question_text: State<String> = hooks.use_state(String::new);
    let mut question_options: State<Vec<String>> = hooks.use_state(Vec::new);
    let mut question_visible = hooks.use_state(|| false);
    let mut question_selected: State<usize> = hooks.use_state(|| 0);
    let mut question_response: State<Option<tokio::sync::oneshot::Sender<String>>> =
        hooks.use_state(|| None);

    let model_name = props.model_name.clone();
    let cmd_tx = props.cmd_tx.clone();
    let show_sidebar = sidebar_open.get() && term_width >= 120;

    // --- Channel polling futures ---
    // Use use_ref to stash receivers (taken on first render only).
    // use_future is ALWAYS called (hooks ordering rule), but the receiver
    // may be None after the first render — the future handles that.

    // Stash receivers in refs (take from props on first render)
    let mut ui_rx_ref = hooks.use_ref(|| props.ui_rx.take());
    let mut bus_rx_ref = hooks.use_ref(|| props.bus_rx.take());
    let mut perm_rx_ref = hooks.use_ref(|| props.perm_rx.take());
    let mut question_rx_ref = hooks.use_ref(|| props.question_rx.take());

    // Poll UI events — always call use_future (hooks ordering)
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
                Some(UiEvent::HistoryMessage { role, content }) => {
                    let msg_role = match role.as_str() {
                        "user" => MessageRole::User,
                        "assistant" => MessageRole::Assistant,
                        _ => MessageRole::System,
                    };
                    // Set session title from first user message
                    if msg_role == MessageRole::User && *session_title.read() == "New Session" {
                        let title = if content.len() > 40 {
                            format!("{}...", &content[..37])
                        } else {
                            content.clone()
                        };
                        session_title.set(title);
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

    // --- Keyboard events ---
    hooks.use_terminal_events({
        let cmd_tx = cmd_tx.clone();
        move |event| {
            if let TerminalEvent::Key(KeyEvent { code, modifiers, kind, .. }) = event {
                if kind == KeyEventKind::Release {
                    return;
                }
                let ctrl = modifiers.contains(KeyModifiers::CONTROL);
                let shift = modifiers.contains(KeyModifiers::SHIFT);

                // Model picker keyboard handling
                if model_picker_visible.get() {
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
                    let max_idx = filtered.len().saturating_sub(1);

                    match code {
                        KeyCode::Up => {
                            model_picker_selected
                                .set(model_picker_selected.get().saturating_sub(1));
                        }
                        KeyCode::Down => {
                            model_picker_selected
                                .set((model_picker_selected.get() + 1).min(max_idx));
                        }
                        KeyCode::Enter => {
                            let sel = model_picker_selected.get().min(max_idx);
                            if let Some(&&(_, _, id)) = filtered.get(sel) {
                                model_picker_visible.set(false);
                                model_picker_filter.set(String::new());
                                if let Some(ref tx) = cmd_tx {
                                    let _ =
                                        tx.send(UiCommand::SwitchModel(id.to_string()));
                                }
                                messages.write().push(DisplayMessage {
                                    role: MessageRole::System,
                                    content: format!("Switched model to {id}"),
                                });
                            }
                        }
                        KeyCode::Esc => {
                            model_picker_visible.set(false);
                            model_picker_filter.set(String::new());
                        }
                        KeyCode::Backspace => {
                            model_picker_filter.write().pop();
                        }
                        KeyCode::Char(c) if !ctrl => {
                            model_picker_filter.write().push(c);
                            model_picker_selected.set(0);
                        }
                        _ => {}
                    }
                    return;
                }

                // Command palette keyboard handling
                if palette_visible.get() {
                    match code {
                        KeyCode::Up => {
                            palette_selected.set(palette_selected.get().saturating_sub(1));
                        }
                        KeyCode::Down => {
                            palette_selected.set(palette_selected.get() + 1);
                            // Clamping happens at render time based on filtered count
                        }
                        KeyCode::Enter => {
                            let sel = palette_selected.get();
                            let commands = get_palette_commands();
                            let filter = palette_filter.read().to_lowercase();
                            let filtered: Vec<_> = commands
                                .iter()
                                .filter(|c| {
                                    filter.is_empty() || c.0.to_lowercase().contains(&filter)
                                })
                                .collect();
                            if let Some(&&(_, action)) = filtered.get(sel) {
                                palette_visible.set(false);
                                palette_filter.set(String::new());
                                // Execute the command
                                match action {
                                    "quit" => {
                                        if let Some(ref tx) = cmd_tx {
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
                                        if let Some(ref tx) = cmd_tx {
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
                            palette_visible.set(false);
                            palette_filter.set(String::new());
                        }
                        KeyCode::Backspace => {
                            palette_filter.write().pop();
                        }
                        KeyCode::Char(c) if !ctrl => {
                            palette_filter.write().push(c);
                            palette_selected.set(0);
                        }
                        _ => {}
                    }
                    return;
                }

                // Permission prompt keyboard handling
                if perm_visible.get() {
                    match code {
                        KeyCode::Left | KeyCode::Char('h') => {
                            perm_selected.set(perm_selected.get().saturating_sub(1));
                        }
                        KeyCode::Right | KeyCode::Char('l') => {
                            perm_selected.set((perm_selected.get() + 1).min(2));
                        }
                        KeyCode::Enter => {
                            use flok_core::tool::PermissionDecision;
                            let decision = match perm_selected.get() {
                                0 => PermissionDecision::Allow,
                                1 => PermissionDecision::Always,
                                _ => PermissionDecision::Deny,
                            };
                            if let Some(tx) = perm_response.write().take() {
                                let _ = tx.send(decision);
                            }
                            perm_visible.set(false);
                        }
                        KeyCode::Esc => {
                            if let Some(tx) = perm_response.write().take() {
                                let _ = tx.send(flok_core::tool::PermissionDecision::Deny);
                            }
                            perm_visible.set(false);
                        }
                        _ => {}
                    }
                    return;
                }

                // Question prompt keyboard handling
                if question_visible.get() {
                    let opt_count = question_options.read().len();
                    match code {
                        KeyCode::Up | KeyCode::Char('k') => {
                            question_selected.set(question_selected.get().saturating_sub(1));
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            question_selected.set(
                                (question_selected.get() + 1).min(opt_count.saturating_sub(1)),
                            );
                        }
                        KeyCode::Enter => {
                            let answer = question_options
                                .read()
                                .get(question_selected.get())
                                .cloned()
                                .unwrap_or_default();
                            if let Some(tx) = question_response.write().take() {
                                let _ = tx.send(answer);
                            }
                            question_visible.set(false);
                        }
                        KeyCode::Esc => {
                            if let Some(tx) = question_response.write().take() {
                                let _ = tx.send("(dismissed)".to_string());
                            }
                            question_visible.set(false);
                        }
                        KeyCode::Char(c @ '1'..='9') if !ctrl => {
                            let idx = (c as usize) - ('1' as usize);
                            if idx < opt_count {
                                question_selected.set(idx);
                                let answer =
                                    question_options.read().get(idx).cloned().unwrap_or_default();
                                if let Some(tx) = question_response.write().take() {
                                    let _ = tx.send(answer);
                                }
                                question_visible.set(false);
                            }
                        }
                        _ => {}
                    }
                    return;
                }

                // Normal keyboard handling
                match code {
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
                        // Delete last word
                        if !waiting.get() {
                            let mut text = input_text.read().clone();
                            // Trim trailing spaces, then remove until next space
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
                        // Move cursor to start
                        input_handle.write().set_cursor_offset(0);
                    }
                    KeyCode::Char('e') if ctrl => {
                        // Move cursor to end
                        let len = input_text.read().len();
                        input_handle.write().set_cursor_offset(len);
                    }
                    // Char input and Backspace are handled by TextInput
                    _ => {}
                }
            }
        }
    });

    // Handle exit — must always call use_context_mut (hooks ordering rule)
    let mut system = hooks.use_context_mut::<SystemContext>();
    if should_exit.get() {
        system.exit();
    }

    // --- Build element tree (all owned data) ---
    let msgs = messages.read().clone();
    let stream = streaming_text.read().clone();
    let reasoning = streaming_reasoning.read().clone();
    let input = input_text.read().clone();
    let title = session_title.read().clone();
    element! {
        View(
            flex_direction: FlexDirection::Column,
            width: term_width,
            height: term_height,
            background_color: theme.bg,
        ) {
            // Main content row — takes all remaining space
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
                    // Messages area — grows to fill, clips overflow
                    MessageList(
                        messages: msgs,
                        streaming_text: stream,
                        streaming_reasoning: reasoning,
                        is_waiting: waiting.get(),
                        theme: theme,

                    )
                    // Bottom area — pinned, never shrunk away, small gap before footer
                    View(flex_shrink: 0.0, margin_bottom: 1u32) {
                        #(if perm_visible.get() {
                            Some(element! {
                                PermissionPromptView(
                                    tool: perm_tool.read().clone(),
                                    description: perm_desc.read().clone(),
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
                                        // Detect paste: if text grew by more than 2 chars
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
                    } // close flex_shrink: 0 View
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
                            theme: theme,
                        )
                    })
                } else {
                    None
                })
            }

            // Status bar — pinned at bottom (opencode style)
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
                    ) {
                        Text(
                            content: format!(" {} ", msg),
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
                            // Title
                            View(flex_direction: FlexDirection::Row, padding_bottom: 1u32) {
                                Text(content: "Commands", color: theme.text, weight: Weight::Bold)
                                View(flex_grow: 1.0) {}
                                Text(content: "esc", color: theme.text_muted)
                            }

                            // Search input
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

                            // Command list
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
                                        // Cannot mutate current_provider in iterator, show provider inline
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
        }
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
/// Returns `(display_name, provider, full_id)`.
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
/// Returns `(title, action_id)` pairs.
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
    .disable_mouse_capture()
    .await?;

    Ok(())
}
