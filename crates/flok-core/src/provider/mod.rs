//! # Provider System
//!
//! LLM provider implementations. Each provider converts the unified message
//! format into provider-specific API calls and streams responses back as
//! `StreamEvent`s.

mod anthropic;
pub mod fallback;
mod minimax;
pub mod mock;
mod models;
mod openai;
pub mod registry;
mod types;

pub use anthropic::AnthropicProvider;
pub use fallback::{is_retriable, CooldownTracker, FallbackChain};
pub use minimax::MiniMaxProvider;
pub use models::{resolve_default_model, ModelInfo, ModelRegistry};
pub use openai::OpenAiProvider;
pub use registry::{ProviderRegistry, DEFAULT_PERMITS_PER_PROVIDER};
pub use types::{
    CompletionRequest, Message, MessageContent, Provider, StreamEvent, ToolDefinition,
};
