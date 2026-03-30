//! # Session Engine
//!
//! The session engine manages conversations between the user, the LLM, and
//! tools. It owns the prompt loop: assemble messages → call provider →
//! process response → execute tool calls → repeat.

mod engine;
mod state;

pub use engine::{SendMessageResult, SessionEngine, UndoResult};
pub use state::{AppState, PlanMode};
