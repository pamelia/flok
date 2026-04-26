//! Shared types for communication between TUI and the session engine.

use tokio::sync::{mpsc, oneshot};

/// TUI-side MCP add request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAddCommand {
    pub name: String,
    pub url: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub bearer_token_env_var: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub disabled: bool,
}

/// Messages from the TUI to the background session task.
#[derive(Debug)]
pub enum UiCommand {
    /// User submitted a prompt.
    SendMessage(String),
    /// List saved execution plans.
    ListPlans,
    /// Show a saved execution plan. `None` means "latest".
    ShowPlan(Option<String>),
    /// Approve a saved execution plan. `None` means "latest".
    ApprovePlan(Option<String>),
    /// Execute a saved execution plan. `None` means "latest".
    ExecutePlan(Option<String>),
    /// Roll back a saved execution plan to a step checkpoint.
    RollbackPlan { plan_id: Option<String>, step_id: Option<String> },
    /// User wants to list sessions.
    ListSessions,
    /// List configured MCP servers from user config.
    ListMcpServers,
    /// Add or update an MCP server in user config.
    AddMcpServer(McpAddCommand),
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
    /// Show the session tree.
    ShowTree,
    /// Branch at a specific message ID.
    BranchAt(String),
    /// Switch to a different session by ID.
    SwitchSession(String),
    /// Set a label on the current session.
    SetLabel(String),
    /// List available branch points (user messages) in the current session.
    ListBranchPoints,
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
    /// Session switched — TUI should reload conversation display.
    /// Contains display messages for the new session.
    SessionSwitched { messages: Vec<(String, String)> },
    /// Branch points (user messages) for the /branch picker.
    /// Each entry: (`message_id`, `message_number`, `text_preview`)
    BranchPoints(Vec<(String, usize, String)>),
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
    pub alternate_screen: bool,
}
