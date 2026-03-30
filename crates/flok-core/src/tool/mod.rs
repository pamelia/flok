//! # Tool System
//!
//! Tools are capabilities that the LLM can invoke during a conversation.
//! Each tool has a name, description, JSON schema for parameters, and an
//! async execute method.

mod bash;
mod edit;
mod glob_tool;
mod grep;
mod memory;
mod plan;
mod question;
mod read;
mod registry;
mod skill;
mod task;
mod todowrite;
mod webfetch;
mod write;

pub use bash::BashTool;
pub use edit::EditTool;
pub use glob_tool::GlobTool;
pub use grep::GrepTool;
pub use memory::AgentMemoryTool;
pub use plan::PlanTool;
pub use question::{QuestionRequest, QuestionTool};
pub use read::ReadTool;
pub use registry::ToolRegistry;
pub use skill::SkillTool;
pub use task::TaskTool;
pub use todowrite::{TodoItem, TodoList, TodoWriteTool};
pub use webfetch::WebfetchTool;
pub use write::WriteTool;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use tokio::sync::{mpsc, oneshot};

// ---------------------------------------------------------------------------
// Permission system
// ---------------------------------------------------------------------------

/// How dangerous a tool operation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionLevel {
    /// Safe operations that only read data. Never prompts.
    Safe,
    /// Operations that modify files. Prompts on first use.
    Write,
    /// Operations that execute arbitrary commands. Always prompts (unless "Always" granted).
    Dangerous,
}

/// A permission request sent to the TUI for user approval.
#[derive(Debug)]
pub struct PermissionRequest {
    /// Tool name.
    pub tool: String,
    /// Human-readable summary of what the tool wants to do.
    pub description: String,
    /// Channel to send the user's decision back.
    pub response_tx: oneshot::Sender<PermissionDecision>,
}

/// The user's decision on a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Allow this one invocation.
    Allow,
    /// Allow all future invocations of this tool in this session.
    Always,
    /// Deny this invocation.
    Deny,
}

/// Manages tool permissions for a session.
pub struct PermissionManager {
    /// Tools that have been permanently allowed for this session.
    always_allowed: Mutex<HashSet<String>>,
    /// Channel to send permission requests to the TUI.
    request_tx: Option<mpsc::UnboundedSender<PermissionRequest>>,
}

impl PermissionManager {
    /// Create a permission manager that prompts through the given channel.
    pub fn new(request_tx: mpsc::UnboundedSender<PermissionRequest>) -> Self {
        Self { always_allowed: Mutex::new(HashSet::new()), request_tx: Some(request_tx) }
    }

    /// Create a permission manager that auto-approves everything (for non-interactive mode).
    pub fn auto_approve() -> Self {
        Self { always_allowed: Mutex::new(HashSet::new()), request_tx: None }
    }

    /// Check if a tool invocation is allowed. May block waiting for user input.
    ///
    /// Returns `true` if allowed, `false` if denied.
    pub async fn check(
        &self,
        tool_name: &str,
        permission_level: PermissionLevel,
        description: &str,
    ) -> bool {
        // Safe tools always pass
        if permission_level == PermissionLevel::Safe {
            return true;
        }

        // Auto-approve mode (non-interactive)
        let Some(ref tx) = self.request_tx else {
            return true;
        };

        // Check "Always" allowlist
        {
            let allowed =
                self.always_allowed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if allowed.contains(tool_name) {
                return true;
            }
        }

        // Send request to TUI and wait for response
        let (response_tx, response_rx) = oneshot::channel();
        let request = PermissionRequest {
            tool: tool_name.to_string(),
            description: description.to_string(),
            response_tx,
        };

        if tx.send(request).is_err() {
            // TUI gone — deny by default
            return false;
        }

        match response_rx.await {
            Ok(PermissionDecision::Allow) => true,
            Ok(PermissionDecision::Always) => {
                let mut allowed =
                    self.always_allowed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                allowed.insert(tool_name.to_string());
                true
            }
            Ok(PermissionDecision::Deny) | Err(_) => false,
        }
    }
}

impl std::fmt::Debug for PermissionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionManager")
            .field("always_allowed", &self.always_allowed)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Tool context and output
// ---------------------------------------------------------------------------

/// Context passed to every tool execution.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// The project root directory. All file paths are relative to this.
    pub project_root: PathBuf,
    /// The current session ID.
    pub session_id: String,
    /// The agent name executing this tool.
    pub agent: String,
    /// Cancellation token — tools should check this periodically for long ops.
    pub cancel: tokio_util::sync::CancellationToken,
}

impl ToolContext {
    /// Check if this tool execution has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Create a test context with defaults.
    #[cfg(test)]
    pub fn test(project_root: std::path::PathBuf) -> Self {
        Self {
            project_root,
            session_id: "test-session".into(),
            agent: "test-agent".into(),
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }
}

/// The result of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// The text content returned to the LLM.
    pub content: String,
    /// Whether this result represents an error.
    pub is_error: bool,
    /// Optional metadata (e.g., file path, line count, etc.)
    pub metadata: serde_json::Value,
}

impl ToolOutput {
    /// Create a successful output.
    pub fn success(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false, metadata: serde_json::Value::Null }
    }

    /// Create an error output.
    pub fn error(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true, metadata: serde_json::Value::Null }
    }
}

// ---------------------------------------------------------------------------
// Tool trait
// ---------------------------------------------------------------------------

/// The tool trait. Each tool implements this.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// The tool's unique name.
    fn name(&self) -> &'static str;

    /// A description of what the tool does (shown to the LLM).
    fn description(&self) -> &'static str;

    /// The JSON schema for the tool's parameters.
    fn parameters_schema(&self) -> serde_json::Value;

    /// The permission level required for this tool.
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Safe
    }

    /// Generate a human-readable description of what this specific invocation will do.
    /// Used in permission prompts.
    fn describe_invocation(&self, args: &serde_json::Value) -> String {
        let _ = args;
        format!("Execute {}", self.name())
    }

    /// Execute the tool with the given arguments.
    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput>;
}
