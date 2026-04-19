//! Internal message bus for the flok TUI.
//!
//! `AppEvent`s are produced by:
//!  - the `Tui` terminal wrapper (Key, Mouse, Paste, Resize)
//!  - the `adapter` module from `UiEvent` / `BusEvent` / `PermissionRequest` / `QuestionRequest`
//!  - internal handlers (`Submit`, `Cancel`, `ShowOverlay`, `Quit`, etc.)
//!
//! The `App` event loop consumes `AppEvent`s via `tokio::select!` and dispatches via
//! `App::handle_event`.
//!
//! # Why `AppEvent` is not `Clone`
//!
//! `PermissionRequest` and `QuestionRequest` each carry a
//! [`tokio::sync::oneshot::Sender`], which is inherently single-consumer and therefore
//! not `Clone`. The event enum is moved through the pipeline exactly once, matching
//! the "request/response" semantics of permission and question prompts.

use crossterm::event::{KeyEvent, MouseEvent};
use flok_core::bus::BusEvent;
use flok_core::tool::{PermissionRequest, QuestionRequest};

use crate::types::UiEvent;

/// Which overlay the UI should display on top of the main chat surface.
#[expect(dead_code, reason = "overlay routing variants reserved for future top-level dispatch")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverlayKind {
    Permission,
    Question,
    SlashPopup,
    Picker,
}

/// Every event that drives the top-level `App` state machine.
///
/// See the module-level documentation for the contract around `Clone`-ability.
#[expect(dead_code, reason = "some internal app events remain reserved for follow-up UI waves")]
#[derive(Debug)]
pub(crate) enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Resize(u16, u16),
    Tick,
    UiEvent(UiEvent),
    BusEvent(BusEvent),
    PermissionRequest(PermissionRequest),
    QuestionRequest(QuestionRequest),
    Submit(String),
    Cancel,
    ShowOverlay(OverlayKind),
    HideOverlay,
    ToggleSidebar,
    TogglePlanMode,
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time exhaustiveness check: if a new `AppEvent` variant is added
    /// without updating this match, compilation fails here, forcing the
    /// reviewer to update downstream dispatchers as well.
    ///
    /// Each variant is listed on its own arm (rather than a compact or-pattern)
    /// so a reviewer adding a variant sees exactly which arm to add. Hence the
    /// explicit `match_same_arms` allow.
    #[allow(dead_code, clippy::match_same_arms)]
    fn _exhaustive(e: &AppEvent) {
        match e {
            AppEvent::Key(_) => {}
            AppEvent::Mouse(_) => {}
            AppEvent::Paste(_) => {}
            AppEvent::Resize(_, _) => {}
            AppEvent::Tick => {}
            AppEvent::UiEvent(_) => {}
            AppEvent::BusEvent(_) => {}
            AppEvent::PermissionRequest(_) => {}
            AppEvent::QuestionRequest(_) => {}
            AppEvent::Submit(_) => {}
            AppEvent::Cancel => {}
            AppEvent::ShowOverlay(_) => {}
            AppEvent::HideOverlay => {}
            AppEvent::ToggleSidebar => {}
            AppEvent::TogglePlanMode => {}
            AppEvent::Quit => {}
        }
    }

    #[test]
    fn overlay_kind_is_copy_and_eq() {
        let a = OverlayKind::Permission;
        let b = a;
        assert_eq!(a, b);
        assert_ne!(OverlayKind::Permission, OverlayKind::Question);
    }

    #[test]
    fn app_event_debug_is_available() {
        // `Debug` is part of the public contract for logging; ensure it compiles
        // for every "plain data" variant we control.
        let ev = AppEvent::Submit("hi".to_string());
        let rendered = format!("{ev:?}");
        assert!(rendered.contains("Submit"));

        let ev = AppEvent::ShowOverlay(OverlayKind::SlashPopup);
        let rendered = format!("{ev:?}");
        assert!(rendered.contains("ShowOverlay"));
        assert!(rendered.contains("SlashPopup"));
    }
}
