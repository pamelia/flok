//! # Token Compression Engine
//!
//! Three layers of compression to reduce token consumption:
//!
//! - **Layer 1 (Shell)**: Compress shell command output before it enters context
//! - **Layer 2 (History)**: Compress `tool_result` blocks in conversation history
//! - **Layer 3 (Compaction)**: Manage context window via pruning and summarization
//!
//! See `specs/015-token-compression/spec.md` for the full design.

pub mod filter;
pub mod history;
pub mod pruning;

pub use filter::{compress_shell_output, compress_shell_output_token_budget, CompressedOutput};
pub use history::compress_tool_result;
pub use pruning::prune_tool_outputs;
