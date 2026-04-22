//! Unified types for the provider system.
//!
//! These types are provider-agnostic. Each provider implementation converts
//! them to/from the provider's native API format.

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Reasoning effort hint for models that support configurable thinking depth.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

impl ReasoningEffort {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
        }
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A streamed event from an LLM provider.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of text output.
    TextDelta(String),

    /// A chunk of reasoning/thinking output (extended thinking).
    ReasoningDelta(String),

    /// The provider is starting a tool call.
    ToolCallStart { index: usize, id: String, name: String },

    /// A chunk of tool call arguments (JSON string fragment).
    ToolCallDelta { index: usize, delta: String },

    /// Usage information at the end of a response.
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
    },

    /// The stream has ended normally.
    Done,

    /// An error occurred during streaming.
    Error(String),
}

/// A message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Vec<MessageContent>,
}

/// Content within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MessageContent {
    #[serde(rename = "text")]
    Text { text: String },

    /// Chain-of-thought reasoning (from extended thinking).
    #[serde(rename = "thinking")]
    Thinking { thinking: String },

    /// Structured warm-memory summary of earlier conversation context.
    #[serde(rename = "compaction")]
    Compaction { summary: CompactionSummary },

    /// Structured project-level memory assembled from other sessions.
    #[serde(rename = "project_memory")]
    ProjectMemory { summary: ProjectMemorySummary },

    /// Targeted recall pulled from prior session summaries for the current query.
    #[serde(rename = "memory_recall")]
    MemoryRecall { summary: MemoryRecallSummary },

    /// Typed step metadata persisted alongside conversation history.
    #[serde(rename = "step")]
    Step { step: StepMetadata },

    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: serde_json::Value },

    #[serde(rename = "tool_result")]
    ToolResult { tool_use_id: String, content: String, is_error: bool },
}

/// Structured summary used for warm session memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionSummary {
    pub goal: String,
    pub progress: Vec<String>,
    pub todos: Vec<String>,
    pub constraints: Vec<String>,
    pub referenced_files: Vec<String>,
}

impl CompactionSummary {
    #[must_use]
    pub fn render_for_prompt(&self) -> String {
        self.render_section("[Compaction Summary]")
    }

    fn render_section(&self, header: &str) -> String {
        let mut text = format!("{header}\n");
        text.push_str("Goal:\n");
        text.push_str("- ");
        text.push_str(&self.goal);
        text.push('\n');

        append_section(&mut text, "Progress", &self.progress);
        append_section(&mut text, "TODOs", &self.todos);
        append_section(&mut text, "Constraints", &self.constraints);
        append_section(&mut text, "Referenced Files", &self.referenced_files);
        text.trim_end().to_string()
    }
}

/// Structured cross-session project memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMemorySummary {
    pub source_sessions: usize,
    pub summary: CompactionSummary,
}

impl ProjectMemorySummary {
    #[must_use]
    pub fn render_for_prompt(&self) -> String {
        let mut text = self.summary.render_section("[Project Memory]");
        text.push_str("\nSource Sessions:\n- ");
        text.push_str(&self.source_sessions.to_string());
        text
    }
}

/// Query-targeted recall from prior sessions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryRecallSummary {
    pub query: String,
    pub matched_sessions: usize,
    pub summary: CompactionSummary,
}

impl MemoryRecallSummary {
    #[must_use]
    pub fn render_for_prompt(&self) -> String {
        let mut text = self.summary.render_section("[Memory Recall]");
        text.push_str("\nRecall Query:\n- ");
        text.push_str(&self.query);
        text.push_str("\nMatched Sessions:\n- ");
        text.push_str(&self.matched_sessions.to_string());
        text
    }
}

/// Durable metadata about an execution step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepMetadata {
    pub kind: StepKind,
    pub status: StepStatus,
    pub summary: String,
    pub details: Vec<String>,
}

impl StepMetadata {
    #[must_use]
    pub fn routing(from_model: &str, to_model: &str, reason: &str) -> Self {
        Self {
            kind: StepKind::Routing,
            status: StepStatus::Info,
            summary: format!("Model routed: {from_model} -> {to_model}"),
            details: vec![format!("Reason: {reason}")],
        }
    }

    #[must_use]
    pub fn verification(command: &str, success: bool, summary: &str, changed_files: usize) -> Self {
        let status = if success { StepStatus::Succeeded } else { StepStatus::Failed };
        let mut details = vec![format!("Command: {command}")];
        if changed_files > 0 {
            details.push(format!("Changed files: {changed_files}"));
        }
        Self { kind: StepKind::Verification, status, summary: summary.to_string(), details }
    }

    #[must_use]
    pub fn context_usage(used_tokens: u64, max_tokens: u64, truncated: bool) -> Self {
        let pct =
            if max_tokens == 0 { 0.0 } else { used_tokens as f64 * 100.0 / max_tokens as f64 };
        let status = if truncated { StepStatus::Warning } else { StepStatus::Info };
        let mut details = vec![format!("Estimated tokens: {used_tokens}/{max_tokens}")];
        if truncated {
            details.push("Emergency truncation applied".to_string());
        }
        Self {
            kind: StepKind::ContextUsage,
            status,
            summary: format!("Context usage estimated at {pct:.0}%"),
            details,
        }
    }

    #[must_use]
    pub fn render_for_prompt(&self) -> String {
        let mut text =
            format!("[Step: {} / {}]\n{}", self.kind.as_str(), self.status.as_str(), self.summary);
        if !self.details.is_empty() {
            text.push_str("\nDetails:\n");
            for detail in &self.details {
                text.push_str("- ");
                text.push_str(detail);
                text.push('\n');
            }
        }
        text.trim_end().to_string()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Routing,
    Verification,
    ContextUsage,
}

impl StepKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Routing => "routing",
            Self::Verification => "verification",
            Self::ContextUsage => "context_usage",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Info,
    Warning,
    Succeeded,
    Failed,
}

impl StepStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

fn append_section(buffer: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }

    buffer.push_str(title);
    buffer.push_str(":\n");
    for item in items {
        buffer.push_str("- ");
        buffer.push_str(item);
        buffer.push('\n');
    }
}

/// A tool definition sent to the provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// A request to complete a conversation.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: u32,
}

/// The provider trait. Each LLM provider implements this.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// The provider's name (e.g., "anthropic", "openai").
    fn name(&self) -> &'static str;

    /// Stream a completion. Events are sent to the provided channel.
    ///
    /// The implementation should send events until `StreamEvent::Done` or
    /// `StreamEvent::Error`, then return.
    async fn stream(
        &self,
        request: CompletionRequest,
        tx: mpsc::UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_metadata_renders_structured_prompt_text() {
        let step = StepMetadata::verification(
            "cargo check --workspace",
            false,
            "Automatic verification failed.",
            2,
        );

        let rendered = step.render_for_prompt();
        assert!(rendered.contains("[Step: verification / failed]"));
        assert!(rendered.contains("Automatic verification failed."));
        assert!(rendered.contains("Command: cargo check --workspace"));
        assert!(rendered.contains("Changed files: 2"));
    }
}
