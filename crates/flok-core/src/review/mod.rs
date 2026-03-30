//! # Code Review Engine
//!
//! Built-in code review system that spawns specialist reviewer sub-agents
//! in parallel, collects structured findings, deduplicates, and produces
//! a prioritized review with a binary verdict.
//!
//! This is the native implementation of the code-review pattern, replacing
//! the external skill-based approach with a tightly-integrated Rust module.

mod engine;
mod prompts;
pub mod self_review;
mod types;

pub use engine::ReviewEngine;
pub use self_review::{SelfReviewLoop, SelfReviewResult, StopReason};
pub use types::{Finding, FindingKind, FindingPriority, ReviewResult, Verdict};
