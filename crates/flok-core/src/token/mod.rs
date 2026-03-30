//! # Token Counting & Cost Tracking
//!
//! Provides accurate token counting via `tiktoken-rs` for OpenAI-family models
//! and a character-based approximation for others. Also tracks cumulative
//! session costs based on model pricing from the registry.

mod cost;
mod counter;

pub use cost::CostTracker;
pub use counter::{count_tokens, TokenCounter};
