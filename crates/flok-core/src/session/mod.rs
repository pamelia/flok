//! # Session Engine
//!
//! The session engine manages conversations between the user, the LLM, and
//! tools. It owns the prompt loop: assemble messages → call provider →
//! process response → execute tool calls → repeat.
//!
//! The session module also includes tree-based branching (spec-018): sessions
//! form a tree via `parent_id`, with branch operations creating new sessions
//! from branch points in existing conversations.

pub mod branch;
mod engine;
mod state;
pub mod tree;

pub use branch::{create_branch, BranchResult};
pub use engine::{SendMessageResult, SessionEngine, UndoResult};
pub use state::{AppState, PlanMode};
pub use tree::{build_session_tree, flatten_tree, SessionTreeNode};
