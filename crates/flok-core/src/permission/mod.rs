//! # Permission System
//!
//! Rule-based permission evaluation for tool operations. Inspired by `OpenCode`'s
//! permission model, this system uses three action types (`Allow`, `Deny`, `Ask`)
//! with pattern matching and last-match-wins semantics.
//!
//! ## Architecture
//!
//! Permissions are evaluated against layered rulesets:
//! 1. **Default rules** — hardcoded sensible defaults (in-project = allow, external = ask)
//! 2. **Config rules** — from `flok.toml` `[permission.*]` sections
//! 3. **Session rules** — from user "Always Allow" decisions (persisted per-project)
//!
//! The evaluator flattens all rulesets and finds the **last matching rule**
//! (last-match-wins). If no rule matches, the default action is `Ask`.
//!
//! ## Permission Types
//!
//! | Permission | Description |
//! |---|---|
//! | `bash` | Shell command execution |
//! | `read` | File reading |
//! | `edit` | File editing (search-and-replace) |
//! | `write` | File creation/overwrite |
//! | `glob` | File pattern matching |
//! | `grep` | Content search |
//! | `external_directory` | Operations on paths outside the project root |
//! | `doom_loop` | Same tool+args repeated 3+ times |
//! | `webfetch` | URL fetching |
//! | `*` | Wildcard matching all permission types |

pub mod arity;
pub mod defaults;
pub mod evaluate;
pub mod path;
pub mod rule;

pub use evaluate::evaluate;
pub use rule::{PermissionAction, PermissionRule};
