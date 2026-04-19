//! Routing layer between raw keys and either the chat composer or a modal overlay.
//!
//! `BottomPane` owns the single `ChatComposer`, the input history, the optional
//! modal `Overlay`, and the optional slash-command popup. Its `handle_key` method
//! implements a priority chain:
//!
//! 1. An active modal `Overlay` captures keys first (permission prompt, question).
//! 2. An active slash popup captures Up/Down/Enter/Tab/Esc.
//! 3. On an empty or single-line composer, Up/Down recall history.
//! 4. Otherwise, the key is forwarded to the `ChatComposer`; `Submit` events are
//!    recorded in history.
//! 5. After the composer consumes the key, the popup is (de)activated based on
//!    whether the current text starts with `/`.
//!
//! Rendering stacks `[overlay][slash popup][composer]` top-to-bottom.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect};

use crate::app_event::AppEvent;
use crate::composer::history::InputHistory;
use crate::composer::ChatComposer;
use crate::overlays::slash_popup::{SlashAction, SlashPopup};
use crate::overlays::{Overlay, OverlayResult};
use crate::theme::Theme;

pub(crate) struct BottomPane {
    composer: ChatComposer,
    history: InputHistory,
    overlay: Option<Overlay>,
    slash_popup: Option<SlashPopup>,
}

impl BottomPane {
    pub(crate) fn new() -> Self {
        Self {
            composer: ChatComposer::new(),
            history: InputHistory::new(),
            overlay: None,
            slash_popup: None,
        }
    }

    pub(crate) fn set_overlay(&mut self, overlay: Overlay) {
        self.overlay = Some(overlay);
    }

    pub(crate) fn clear_overlay(&mut self) {
        self.overlay = None;
    }

    pub(crate) fn has_overlay(&self) -> bool {
        self.overlay.is_some()
    }

    pub(crate) fn set_waiting(&mut self, waiting: bool) {
        self.composer.set_disabled(waiting);
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Option<AppEvent> {
        // 1. Modal overlay captures first. `map` scopes the `&mut` borrow so we
        // can freely mutate `self.overlay` afterwards (avoids NLL borrow issues
        // that arise from a naive `if let Some(o) = self.overlay.as_mut()` +
        // match arms that reassign `self.overlay`).
        if let Some((result, event)) = self.overlay.as_mut().map(|o| o.handle_key(key)) {
            if matches!(result, OverlayResult::Closed) {
                self.overlay = None;
            }
            return event;
        }

        // 2. Slash popup captures if active. Same borrow-scoping trick.
        if let Some(action) = self.slash_popup.as_mut().map(|p| p.handle_key(key)) {
            match action {
                SlashAction::Move => return None,
                SlashAction::Select(cmd) => {
                    let full = format!("/{cmd}");
                    self.composer.clear();
                    self.slash_popup = None;
                    self.history.push(full.clone());
                    return Some(AppEvent::Submit(full));
                }
                SlashAction::Cancel => {
                    self.slash_popup = None;
                    return None;
                }
                SlashAction::None => {
                    // Fall through so typed characters (and backspace) reach the
                    // composer; `sync_slash_popup` below updates the filter.
                }
            }
        }

        // 3. Intercept history recall on empty or single-line input.
        if self.composer.is_empty() || self.composer_is_single_line() {
            if matches!(key.code, KeyCode::Up) && key.modifiers == KeyModifiers::NONE {
                let cur = self.composer.text();
                if let Some(text) = self.history.recall_prev(&cur) {
                    self.composer.clear();
                    self.composer.handle_paste(&text);
                }
                return None;
            }
            if matches!(key.code, KeyCode::Down)
                && key.modifiers == KeyModifiers::NONE
                && self.history.is_browsing()
            {
                let cur = self.composer.text();
                if let Some(text) = self.history.recall_next(&cur) {
                    self.composer.clear();
                    self.composer.handle_paste(&text);
                }
                return None;
            }
        }

        // 4. Forward to composer; record submits in history.
        let evt = self.composer.handle_key(key);
        if let Some(AppEvent::Submit(ref text)) = evt {
            self.history.push(text.clone());
        }

        // 5. (De)activate the slash popup based on the new composer state.
        self.sync_slash_popup();

        evt
    }

    /// Activates the slash popup when the composer starts with `/`, updates its
    /// filter when it stays active, and tears it down when the slash has been
    /// erased (or the composer was cleared by a submit).
    fn sync_slash_popup(&mut self) {
        let text = self.composer.text();
        let has_slash = !text.is_empty() && text.starts_with('/');
        match (&mut self.slash_popup, has_slash) {
            (None, true) => {
                let mut popup = SlashPopup::new();
                popup.update_filter(&text);
                self.slash_popup = Some(popup);
            }
            (Some(popup), true) => popup.update_filter(&text),
            (Some(_), false) => self.slash_popup = None,
            (None, false) => {}
        }
    }

    fn composer_is_single_line(&self) -> bool {
        !self.composer.text().contains('\n')
    }

    pub(crate) fn handle_paste(&mut self, s: &str) {
        self.composer.handle_paste(s);
        // Pasted text can start with `/` or remove one — keep popup in sync.
        self.sync_slash_popup();
    }

    pub(crate) fn compute_height(&self, width: u16) -> u16 {
        let composer_h = self.composer.height(width);
        let overlay_h = self.overlay.as_ref().map_or(0, |o| o.desired_height(width));
        let popup_h = self.slash_popup.as_ref().map_or(0, SlashPopup::desired_height);
        composer_h.saturating_add(overlay_h).saturating_add(popup_h)
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let mut y = area.y;
        let width = area.width;

        if let Some(overlay) = &self.overlay {
            let remaining = area.height.saturating_sub(y.saturating_sub(area.y));
            let h = overlay.desired_height(width).min(remaining);
            if h > 0 {
                let sub = Rect { x: area.x, y, width, height: h };
                overlay.render(sub, buf, theme);
                y = y.saturating_add(h);
            }
        }

        if let Some(popup) = &self.slash_popup {
            let remaining = area.height.saturating_sub(y.saturating_sub(area.y));
            let h = popup.desired_height().min(remaining);
            if h > 0 {
                let sub = Rect { x: area.x, y, width, height: h };
                popup.render(sub, buf, theme);
                y = y.saturating_add(h);
            }
        }

        let composer_h = area.height.saturating_sub(y.saturating_sub(area.y));
        if composer_h > 0 {
            let sub = Rect { x: area.x, y, width, height: composer_h };
            self.composer.render(theme, sub, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlays::permission::PermissionOverlay;
    use flok_core::tool::{PermissionDecision, PermissionRequest};
    use tokio::sync::oneshot;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn char_key(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
    }

    fn type_string(bp: &mut BottomPane, s: &str) {
        for ch in s.chars() {
            let _ = bp.handle_key(char_key(ch));
        }
    }

    fn make_permission_overlay() -> (Overlay, oneshot::Receiver<PermissionDecision>) {
        let (tx, rx) = oneshot::channel();
        let req = PermissionRequest {
            tool: "read".to_string(),
            description: "read /etc/hosts".to_string(),
            always_pattern: "read *".to_string(),
            response_tx: tx,
        };
        (Overlay::Permission(PermissionOverlay::new(req)), rx)
    }

    #[test]
    fn overlay_captures_keys_before_composer() {
        let mut bp = BottomPane::new();
        let (overlay, _rx) = make_permission_overlay();
        bp.set_overlay(overlay);
        assert!(bp.has_overlay());

        // 'x' is ignored by PermissionOverlay (falls into the default Consumed arm)
        // but must NOT reach the composer.
        let evt = bp.handle_key(char_key('x'));
        assert!(evt.is_none());
        assert!(bp.composer.is_empty(), "composer saw the key: {:?}", bp.composer.text());
        assert!(bp.has_overlay(), "overlay should still be open after non-terminal key");
    }

    #[test]
    fn overlay_closes_on_terminal_key() {
        let mut bp = BottomPane::new();
        let (overlay, mut rx) = make_permission_overlay();
        bp.set_overlay(overlay);

        let _ = bp.handle_key(char_key('y'));
        assert!(!bp.has_overlay());
        assert_eq!(rx.try_recv().ok(), Some(PermissionDecision::Allow));
    }

    #[test]
    fn typing_slash_activates_popup() {
        let mut bp = BottomPane::new();
        assert!(bp.slash_popup.is_none());

        let _ = bp.handle_key(char_key('/'));

        assert!(bp.slash_popup.is_some());
        assert_eq!(bp.composer.text(), "/");
    }

    #[test]
    fn erasing_slash_deactivates_popup() {
        let mut bp = BottomPane::new();
        let _ = bp.handle_key(char_key('/'));
        assert!(bp.slash_popup.is_some());

        let _ = bp.handle_key(key(KeyCode::Backspace));
        assert!(bp.composer.is_empty());
        assert!(bp.slash_popup.is_none());
    }

    #[test]
    fn popup_select_submits_slash_command() {
        let mut bp = BottomPane::new();
        type_string(&mut bp, "/help");
        assert!(bp.slash_popup.is_some());

        let evt = bp.handle_key(key(KeyCode::Enter));
        match evt {
            Some(AppEvent::Submit(s)) => assert_eq!(s, "/help"),
            other => panic!("expected Submit(\"/help\"), got {other:?}"),
        }
        assert!(bp.slash_popup.is_none());
        assert!(bp.composer.is_empty());
    }

    #[test]
    fn up_on_empty_composer_recalls_history() {
        let mut bp = BottomPane::new();
        bp.history.push("past message".to_string());

        let evt = bp.handle_key(key(KeyCode::Up));

        assert!(evt.is_none());
        assert_eq!(bp.composer.text(), "past message");
    }

    #[test]
    fn down_past_newest_restores_draft() {
        let mut bp = BottomPane::new();
        bp.history.push("older".to_string());
        // Start with a draft in the composer.
        type_string(&mut bp, "draft");

        // Up saves draft and shows the history entry.
        let _ = bp.handle_key(key(KeyCode::Up));
        assert_eq!(bp.composer.text(), "older");

        // Down past newest restores the saved draft.
        let _ = bp.handle_key(key(KeyCode::Down));
        assert_eq!(bp.composer.text(), "draft");
    }

    #[test]
    fn submit_adds_to_history() {
        let mut bp = BottomPane::new();
        type_string(&mut bp, "hi");

        let evt = bp.handle_key(key(KeyCode::Enter));
        assert!(matches!(&evt, Some(AppEvent::Submit(s)) if s == "hi"));
        assert!(bp.composer.is_empty());

        // Pressing Up must recall the just-submitted entry.
        let _ = bp.handle_key(key(KeyCode::Up));
        assert_eq!(bp.composer.text(), "hi");
    }

    #[test]
    fn set_waiting_disables_composer() {
        let mut bp = BottomPane::new();
        bp.set_waiting(true);
        // Disabled composer should swallow typed keys.
        let _ = bp.handle_key(char_key('x'));
        assert!(bp.composer.is_empty());

        bp.set_waiting(false);
        let _ = bp.handle_key(char_key('x'));
        assert_eq!(bp.composer.text(), "x");
    }

    #[test]
    fn compute_height_grows_with_popup() {
        let mut bp = BottomPane::new();
        let base = bp.compute_height(80);
        let _ = bp.handle_key(char_key('/'));
        let with_popup = bp.compute_height(80);
        assert!(with_popup > base, "expected popup to add height: {base} -> {with_popup}");
    }

    #[test]
    fn render_does_not_panic_on_small_area() {
        let mut bp = BottomPane::new();
        let (overlay, _rx) = make_permission_overlay();
        bp.set_overlay(overlay);
        let _ = bp.handle_key(char_key('/'));

        let theme = Theme::dark();
        let area = Rect::new(0, 0, 20, 8);
        let mut buf = Buffer::empty(area);
        bp.render(area, &mut buf, &theme);
    }
}
