//! # Provider System
//!
//! LLM provider implementations. Each provider converts the unified message
//! format into provider-specific API calls and streams responses back as
//! `StreamEvent`s.

mod anthropic;
pub mod mock;
mod models;
mod openai;
mod types;

pub use anthropic::AnthropicProvider;
pub use models::{ModelInfo, ModelRegistry};
pub use openai::OpenAiProvider;
pub use types::{
    CompletionRequest, Message, MessageContent, Provider, StreamEvent, ToolDefinition,
};
