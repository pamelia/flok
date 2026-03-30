//! # flok-apply
//!
//! Fast apply engine for code edits. Handles lazy edit snippets where
//! the LLM uses markers like `// ... existing code ...` to indicate
//! unchanged regions, rather than reproducing the full file.
//!
//! ## Strategy
//!
//! The engine uses a multi-level fallback chain:
//!
//! 1. **Ellipsis merge**: Detect `... existing code ...` markers and match
//!    surrounding context lines against the original file to reconstruct
//!    the full edit.
//! 2. **Line-level fuzzy match**: If the snippet looks like a contiguous
//!    block of code (no ellipsis markers), find the best-matching region
//!    in the original file and replace it.
//! 3. **Full file replacement**: If the snippet appears to be a complete
//!    file (has similar structure/imports), replace the entire file.
//!
//! Each strategy reports which approach was used so the caller can log it.

mod apply;
mod fuzzy;

pub use apply::{apply_edit, ApplyResult, Strategy};
