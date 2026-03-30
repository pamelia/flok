//! Shared types for communication between TUI and the session engine.

use tokio::sync::{mpsc, oneshot};

/// Messages from the TUI to the background session task.
#[derive(Debug)]
pub enum UiCommand {
    /// User submitted a prompt.
    SendMessage(String),
    /// User wants to list sessions.
    ListSessions,
    /// User selected a model from the picker. Value is the full model ID.
    SwitchModel(String),
    /// Undo the last user message and restore files.
    Undo,
    /// Redo the last undone message and restore files.
    Redo,
    /// Cancel the current streaming response or tool execution.
    Cancel,
    /// User wants to quit.
    Quit,
}

/// Messages from the background session task to the TUI.
#[derive(Debug)]
pub enum UiEvent {
    /// A text delta arrived from streaming.
    TextDelta(String),
    /// The assistant finished responding.
    AssistantDone(String),
    /// The assistant's response was cancelled by the user.
    /// Contains any partial text generated before cancellation.
    Cancelled(String),
    /// A historical message loaded from a resumed session.
    /// (role: "user" | "assistant" | "system", content)
    HistoryMessage { role: String, content: String },
    /// An error occurred.
    Error(String),
}

/// A permission prompt from the engine, needing user approval.
pub struct PermissionPrompt {
    /// Tool name.
    pub tool: String,
    /// What the tool wants to do.
    pub description: String,
    /// Send the decision back.
    pub response_tx: oneshot::Sender<flok_core::tool::PermissionDecision>,
}

impl std::fmt::Debug for PermissionPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionPrompt")
            .field("tool", &self.tool)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

/// A question dialog from the engine, needing user selection.
pub struct QuestionDialog {
    /// The question text.
    pub question: String,
    /// Available options.
    pub options: Vec<String>,
    /// Whether the user can type a custom answer.
    pub allow_custom: bool,
    /// Send the answer back.
    pub response_tx: oneshot::Sender<String>,
}

impl std::fmt::Debug for QuestionDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuestionDialog")
            .field("question", &self.question)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

/// Channels bundle passed to the TUI app.
pub struct TuiChannels {
    pub cmd_tx: mpsc::UnboundedSender<UiCommand>,
    pub ui_rx: mpsc::UnboundedReceiver<UiEvent>,
    pub bus_rx: tokio::sync::broadcast::Receiver<flok_core::bus::BusEvent>,
    pub perm_rx: mpsc::UnboundedReceiver<flok_core::tool::PermissionRequest>,
    pub question_rx: mpsc::UnboundedReceiver<flok_core::tool::QuestionRequest>,
    pub todo_list: flok_core::tool::TodoList,
    pub plan_mode: flok_core::session::PlanMode,
    pub model_name: String,
}
