//! Adapter: translates external (flok-core) events into the internal
//! [`AppEvent`] vocabulary consumed by the App event loop.
//!
//! # Design principle
//!
//! **`UiEvent` owns the transcript. `BusEvent` owns telemetry / progress.**
//!
//! - [`from_ui_event`]: transcript-shaped events (text deltas, assistant done,
//!   history messages, session switched, branch points, errors, cancellation)
//!   are forwarded 1:1. The App dispatches on the `UiEvent` variant in Wave 5.
//!
//! - [`from_bus_event`]: telemetry (token usage, cost updates, compression
//!   stats, snapshot activity, tool lifecycle, team events) is wrapped into
//!   [`AppEvent::BusEvent`]. MVP forwards every variant and lets the App
//!   decide which it cares about. Returns [`Option<AppEvent>`] to leave the
//!   door open for future filtering without breaking callers.
//!
//! - [`from_permission_request`] / [`from_question_request`]: one-shot
//!   interactive prompts are carried through untouched so the App can surface
//!   an overlay and send the decision back via the embedded `oneshot::Sender`.
//!
//! Keeping this translation in one tiny module means every other part of the
//! TUI depends on [`AppEvent`] alone and stays decoupled from `flok-core`'s
//! event shapes.

use flok_core::bus::BusEvent;
use flok_core::tool::{PermissionRequest, QuestionRequest};

use crate::app_event::AppEvent;
use crate::types::UiEvent;

/// Wrap a transcript-shaped [`UiEvent`] into an [`AppEvent`].
///
/// The App's event loop inspects the inner `UiEvent` variant to decide whether
/// to append to the active item, push a new history item, or update session
/// state.
pub(crate) fn from_ui_event(e: UiEvent) -> AppEvent {
    AppEvent::UiEvent(e)
}

/// Wrap a telemetry-shaped [`BusEvent`] into an [`AppEvent`].
///
/// Returns `Option` so future revisions can drop events that don't affect the
/// UI (e.g. `MessageCreated` when the TUI already rendered the message) without
/// changing callers. MVP forwards every event; the App filters downstream.
#[expect(
    clippy::unnecessary_wraps,
    reason = "Option return type is intentional — Wave 5 will drop UI-irrelevant BusEvents here without a breaking signature change."
)]
pub(crate) fn from_bus_event(e: BusEvent) -> Option<AppEvent> {
    Some(AppEvent::BusEvent(e))
}

/// Wrap a [`PermissionRequest`] into an [`AppEvent`].
///
/// The request carries a `oneshot::Sender<PermissionDecision>` that the App
/// must resolve exactly once after the user interacts with the permission
/// overlay.
pub(crate) fn from_permission_request(r: PermissionRequest) -> AppEvent {
    AppEvent::PermissionRequest(r)
}

/// Wrap a [`QuestionRequest`] into an [`AppEvent`].
///
/// The request carries a `oneshot::Sender<String>` that the App must resolve
/// exactly once with the user's chosen option.
pub(crate) fn from_question_request(r: QuestionRequest) -> AppEvent {
    AppEvent::QuestionRequest(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    #[test]
    fn from_ui_event_wraps_unchanged() {
        let ev = UiEvent::TextDelta("hello".to_string());
        match from_ui_event(ev) {
            AppEvent::UiEvent(UiEvent::TextDelta(s)) => assert_eq!(s, "hello"),
            other => panic!("expected AppEvent::UiEvent(TextDelta), got {other:?}"),
        }
    }

    #[test]
    fn from_bus_event_wraps_unchanged() {
        let ev = BusEvent::CostUpdate { session_id: "sess-1".to_string(), total_cost_usd: 0.42 };
        match from_bus_event(ev) {
            Some(AppEvent::BusEvent(BusEvent::CostUpdate { session_id, total_cost_usd })) => {
                assert_eq!(session_id, "sess-1");
                assert!((total_cost_usd - 0.42).abs() < f64::EPSILON);
            }
            other => panic!("expected Some(AppEvent::BusEvent(CostUpdate)), got {other:?}"),
        }
    }

    #[test]
    fn from_permission_request_wraps_unchanged() {
        let (tx, _rx) = oneshot::channel();
        let req = PermissionRequest {
            tool: "bash".to_string(),
            description: "run: ls".to_string(),
            always_pattern: "bash *".to_string(),
            response_tx: tx,
        };
        match from_permission_request(req) {
            AppEvent::PermissionRequest(r) => {
                assert_eq!(r.tool, "bash");
                assert_eq!(r.description, "run: ls");
                assert_eq!(r.always_pattern, "bash *");
            }
            other => panic!("expected AppEvent::PermissionRequest, got {other:?}"),
        }
    }

    #[test]
    fn from_question_request_wraps_unchanged() {
        let (tx, _rx) = oneshot::channel();
        let req = QuestionRequest {
            question: "Proceed?".to_string(),
            options: vec!["yes".to_string(), "no".to_string()],
            allow_custom: false,
            response_tx: tx,
        };
        match from_question_request(req) {
            AppEvent::QuestionRequest(r) => {
                assert_eq!(r.question, "Proceed?");
                assert_eq!(r.options, vec!["yes".to_string(), "no".to_string()]);
                assert!(!r.allow_custom);
            }
            other => panic!("expected AppEvent::QuestionRequest, got {other:?}"),
        }
    }
}
