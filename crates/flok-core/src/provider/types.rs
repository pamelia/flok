//! Unified types for the provider system.
//!
//! These types are provider-agnostic. Each provider implementation converts
//! them to/from the provider's native API format.

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

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

    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: serde_json::Value },

    #[serde(rename = "tool_result")]
    ToolResult { tool_use_id: String, content: String, is_error: bool },
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
