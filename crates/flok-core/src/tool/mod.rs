//! # Tool System
//!
//! Tools are capabilities that the LLM can invoke during a conversation.
//! Each tool has a name, description, JSON schema for parameters, and an
//! async execute method.

mod bash;
pub(crate) mod compression;
mod edit;
mod fast_apply;
mod glob_tool;
mod grep;
mod lsp;
mod memory;
mod plan;
mod question;
mod read;
mod registry;
mod review_tool;
mod skill;
mod task;
mod team_tools;
mod todowrite;
mod webfetch;
mod write;

pub use bash::BashTool;
pub use edit::EditTool;
pub use fast_apply::FastApplyTool;
pub use glob_tool::GlobTool;
pub use grep::GrepTool;
pub use lsp::{LspDiagnosticsTool, LspFindReferencesTool, LspGotoDefinitionTool, LspSymbolsTool};
pub use memory::AgentMemoryTool;
pub use plan::PlanTool;
pub use question::{QuestionRequest, QuestionTool};
pub use read::ReadTool;
pub use registry::ToolRegistry;
pub use review_tool::CodeReviewTool;
pub use skill::SkillTool;
pub use task::TaskTool;
pub use team_tools::{SendMessageTool, TeamCreateTool, TeamDeleteTool, TeamTaskTool};
pub use todowrite::{TodoItem, TodoList, TodoWriteTool};
pub use webfetch::WebfetchTool;
pub use write::WriteTool;

use std::path::PathBuf;
use std::sync::Mutex;

use tokio::sync::{mpsc, oneshot};

use crate::lsp::LspManager;
use crate::permission::arity;
use crate::permission::rule::{PermissionAction, PermissionRule};
use crate::permission::{defaults, evaluate};

// ---------------------------------------------------------------------------
// Permission system
// ---------------------------------------------------------------------------

/// How dangerous a tool operation is.
///
/// This enum is kept for backward compatibility with tools that declare their
/// permission level. The rule-based system takes precedence when rules exist;
/// this level is used as a fallback hint when no specific rule matches.
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
    /// Tool name (permission type).
    pub tool: String,
    /// Human-readable summary of what the tool wants to do.
    pub description: String,
    /// The "always allow" pattern that will be stored if the user chooses "Always".
    /// For bash commands, this is the arity-based prefix (e.g., `"git commit *"`).
    /// For file operations, this is the tool name + `" *"`.
    pub always_pattern: String,
    /// Channel to send the user's decision back.
    pub response_tx: oneshot::Sender<PermissionDecision>,
}

/// The user's decision on a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Allow this one invocation.
    Allow,
    /// Allow all future invocations matching the `always_pattern`.
    Always,
    /// Deny this invocation.
    Deny,
}

/// Manages tool permissions for a session using rule-based evaluation.
///
/// Evaluates permission checks against three layered rulesets:
/// 1. Default rules (hardcoded — in-project allowed, external ask)
/// 2. Config rules (from `flok.toml`)
/// 3. Session rules (from user "Always Allow" decisions)
///
/// Uses last-match-wins semantics across all layers.
pub struct PermissionManager {
    /// Default permission rules (hardcoded sensible defaults).
    default_rules: Vec<PermissionRule>,
    /// Config-provided rules (from `flok.toml`).
    config_rules: Vec<PermissionRule>,
    /// Session-level rules from user "Always Allow" decisions.
    session_rules: Mutex<Vec<PermissionRule>>,
    /// Channel to send permission requests to the TUI.
    request_tx: Option<mpsc::UnboundedSender<PermissionRequest>>,
    /// Channel to notify when a new session rule is added (for persistence).
    /// The engine drains this and persists to the database.
    rule_added_tx: std::sync::mpsc::Sender<PermissionRule>,
    /// Receiver end — held by the engine for draining.
    rule_added_rx: Mutex<std::sync::mpsc::Receiver<PermissionRule>>,
}

impl PermissionManager {
    /// Create a permission manager that prompts through the given channel.
    ///
    /// Uses default rules and no config rules. Config rules can be set later
    /// via [`set_config_rules`].
    pub fn new(request_tx: mpsc::UnboundedSender<PermissionRequest>) -> Self {
        let (rule_added_tx, rule_added_rx) = std::sync::mpsc::channel();
        Self {
            default_rules: defaults::default_rules(),
            config_rules: Vec::new(),
            session_rules: Mutex::new(Vec::new()),
            request_tx: Some(request_tx),
            rule_added_tx,
            rule_added_rx: Mutex::new(rule_added_rx),
        }
    }

    /// Create a permission manager that auto-approves everything (for non-interactive mode).
    pub fn auto_approve() -> Self {
        let (rule_added_tx, rule_added_rx) = std::sync::mpsc::channel();
        Self {
            default_rules: defaults::default_rules(),
            config_rules: Vec::new(),
            session_rules: Mutex::new(Vec::new()),
            request_tx: None,
            rule_added_tx,
            rule_added_rx: Mutex::new(rule_added_rx),
        }
    }

    /// Set config-provided permission rules (from `flok.toml`).
    pub fn set_config_rules(&mut self, rules: Vec<PermissionRule>) {
        self.config_rules = rules;
    }

    /// Load previously persisted session rules (e.g., from database).
    pub fn load_session_rules(&self, rules: Vec<PermissionRule>) {
        let mut session =
            self.session_rules.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *session = rules;
    }

    /// Drain any newly added rules from the channel.
    ///
    /// Called by the engine after permission checks to persist new rules to
    /// the database. Returns all rules added since the last drain.
    pub fn drain_new_rules(&self) -> Vec<PermissionRule> {
        let rx = self.rule_added_rx.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut rules = Vec::new();
        while let Ok(rule) = rx.try_recv() {
            rules.push(rule);
        }
        rules
    }

    /// Check if a tool invocation is allowed. May block waiting for user input.
    ///
    /// # Arguments
    ///
    /// * `permission` — The permission type (e.g., `"bash"`, `"edit"`, `"read"`)
    /// * `pattern` — The specific pattern to check (e.g., `"git commit -m fix"`, `"src/main.rs"`)
    /// * `description` — Human-readable description for the TUI prompt
    ///
    /// # Returns
    ///
    /// `true` if allowed, `false` if denied or rejected by the user.
    pub async fn check(&self, permission: &str, pattern: &str, description: &str) -> bool {
        // Auto-approve mode (non-interactive)
        let Some(ref tx) = self.request_tx else {
            return true;
        };

        // Evaluate against all rulesets
        let action = {
            let session =
                self.session_rules.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            evaluate(permission, pattern, &[&self.default_rules, &self.config_rules, &session])
        };

        tracing::debug!(
            permission,
            pattern,
            action = ?action,
            "permission check result"
        );

        match action {
            PermissionAction::Allow => return true,
            PermissionAction::Deny => return false,
            PermissionAction::Ask => {} // Fall through to TUI prompt
        }

        // Compute the "always" pattern for this invocation
        let always_pattern = if permission == "bash" {
            let tokens = arity::tokenize_command(pattern);
            arity::always_pattern(&tokens)
        } else {
            format!("{permission} *")
        };

        // Send request to TUI and wait for response
        let (response_tx, response_rx) = oneshot::channel();
        let request = PermissionRequest {
            tool: permission.to_string(),
            description: description.to_string(),
            always_pattern: always_pattern.clone(),
            response_tx,
        };

        if tx.send(request).is_err() {
            // TUI gone — deny by default
            return false;
        }

        match response_rx.await {
            Ok(PermissionDecision::Allow) => true,
            Ok(PermissionDecision::Always) => {
                let new_rule =
                    PermissionRule::new(permission, &always_pattern, PermissionAction::Allow);
                // Notify engine for persistence (non-blocking, fire-and-forget)
                let _ = self.rule_added_tx.send(new_rule.clone());
                // Add to session rules
                let mut session =
                    self.session_rules.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                session.push(new_rule);
                true
            }
            Ok(PermissionDecision::Deny) | Err(_) => false,
        }
    }

    /// Legacy compatibility: check using the old `PermissionLevel` enum.
    ///
    /// Maps the tool name and level to the new `(permission, pattern)` system:
    /// - `Safe` → always allowed (no rule check)
    /// - `Write`/`Dangerous` → evaluates rules with `(tool_name, "*")`
    pub async fn check_legacy(
        &self,
        tool_name: &str,
        permission_level: PermissionLevel,
        description: &str,
    ) -> bool {
        if permission_level == PermissionLevel::Safe {
            return true;
        }
        self.check(tool_name, "*", description).await
    }
}

impl std::fmt::Debug for PermissionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionManager")
            .field("session_rules", &self.session_rules)
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
    pub lsp: Option<std::sync::Arc<LspManager>>,
    pub output_compression: crate::config::OutputCompressionConfig,
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
            lsp: None,
            output_compression: crate::config::OutputCompressionConfig::default(),
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
