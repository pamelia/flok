//! # flok-tui
//!
//! Terminal UI for flok. Built on `iocraft` with a declarative, React-like
//! component model. This crate provides the main TUI application and all
//! visual components.

mod components;
pub mod theme;
pub mod types;

pub use components::app::run_app;
pub use types::{PermissionPrompt, QuestionDialog, UiCommand, UiEvent};
