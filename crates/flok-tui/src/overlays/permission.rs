use crossterm::event::{KeyCode, KeyEvent};
use flok_core::tool::{PermissionDecision, PermissionRequest};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};
use tokio::sync::oneshot;

use crate::app_event::AppEvent;
use crate::theme::Theme;

use super::{ratatui_color, OverlayResult};

const SELECTED_ALLOW: u8 = 0;
const SELECTED_ALWAYS: u8 = 1;
const SELECTED_DENY: u8 = 2;

pub(crate) struct PermissionOverlay {
    tool: String,
    description: String,
    #[expect(dead_code, reason = "persisted by caller when user picks Always (T3E)")]
    always_pattern: String,
    response_tx: Option<oneshot::Sender<PermissionDecision>>,
    selected: u8,
}

impl PermissionOverlay {
    pub(crate) fn new(req: PermissionRequest) -> Self {
        Self {
            tool: req.tool,
            description: req.description,
            always_pattern: req.always_pattern,
            response_tx: Some(req.response_tx),
            selected: SELECTED_ALLOW,
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> (OverlayResult, Option<AppEvent>) {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                self.respond(PermissionDecision::Allow);
                (OverlayResult::Closed, None)
            }
            KeyCode::Char('a' | 'A') => {
                self.respond(PermissionDecision::Always);
                (OverlayResult::Closed, None)
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                self.respond(PermissionDecision::Deny);
                (OverlayResult::Closed, None)
            }
            KeyCode::Enter => {
                let decision = match self.selected {
                    SELECTED_ALLOW => PermissionDecision::Allow,
                    SELECTED_ALWAYS => PermissionDecision::Always,
                    _ => PermissionDecision::Deny,
                };
                self.respond(decision);
                (OverlayResult::Closed, None)
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.selected = self.selected.saturating_sub(1);
                (OverlayResult::Consumed, None)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.selected = (self.selected + 1).min(SELECTED_DENY);
                (OverlayResult::Consumed, None)
            }
            KeyCode::Tab => {
                self.selected = (self.selected + 1) % 3;
                (OverlayResult::Consumed, None)
            }
            _ => (OverlayResult::Consumed, None),
        }
    }

    fn respond(&mut self, decision: PermissionDecision) {
        if let Some(tx) = self.response_tx.take() {
            let _ = tx.send(decision);
        }
    }

    pub(crate) fn desired_height(&self, _width: u16) -> u16 {
        // borders (2) + tool/desc line(s) + blank + button row + padding
        let desc_lines = u16::try_from(self.description.lines().count()).unwrap_or(1).max(1);
        desc_lines.saturating_add(6)
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if area.width < 4 || area.height < 3 {
            return;
        }

        let border = ratatui_color(theme.warn);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Permission Required ")
            .border_style(Style::default().fg(border));

        let inner = block.inner(area);
        block.render(area, buf);

        let bold = Style::default().add_modifier(Modifier::BOLD);
        let accent =
            Style::default().fg(ratatui_color(theme.accent_user)).add_modifier(Modifier::BOLD);

        let button = |label: &'static str, idx: u8| -> Span<'static> {
            if idx == self.selected {
                Span::styled(label, accent)
            } else {
                Span::raw(label)
            }
        };

        let lines: Vec<Line<'_>> = vec![
            Line::from(vec![
                Span::styled(self.tool.as_str(), bold),
                Span::raw(": "),
                Span::raw(self.description.as_str()),
            ]),
            Line::from(""),
            Line::from(vec![
                button("[Y]es", SELECTED_ALLOW),
                Span::raw("  "),
                button("[A]lways", SELECTED_ALWAYS),
                Span::raw("  "),
                button("[N]o", SELECTED_DENY),
            ]),
        ];

        Paragraph::new(lines).wrap(Wrap { trim: false }).render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn make_overlay() -> (PermissionOverlay, oneshot::Receiver<PermissionDecision>) {
        let (tx, rx) = oneshot::channel();
        let req = PermissionRequest {
            tool: "bash".to_string(),
            description: "ls -la".to_string(),
            always_pattern: "bash ls *".to_string(),
            response_tx: tx,
        };
        (PermissionOverlay::new(req), rx)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn y_key_sends_allow() {
        let (mut overlay, mut rx) = make_overlay();
        let (result, _) = overlay.handle_key(key(KeyCode::Char('y')));
        assert!(matches!(result, OverlayResult::Closed));
        assert_eq!(rx.try_recv().unwrap(), PermissionDecision::Allow);
    }

    #[test]
    fn esc_sends_deny() {
        let (mut overlay, mut rx) = make_overlay();
        let (result, _) = overlay.handle_key(key(KeyCode::Esc));
        assert!(matches!(result, OverlayResult::Closed));
        assert_eq!(rx.try_recv().unwrap(), PermissionDecision::Deny);
    }

    #[test]
    fn arrow_nav_changes_selection() {
        let (mut overlay, _rx) = make_overlay();
        assert_eq!(overlay.selected, SELECTED_ALLOW);
        overlay.handle_key(key(KeyCode::Right));
        assert_eq!(overlay.selected, SELECTED_ALWAYS);
        overlay.handle_key(key(KeyCode::Right));
        assert_eq!(overlay.selected, SELECTED_DENY);
        overlay.handle_key(key(KeyCode::Right));
        assert_eq!(overlay.selected, SELECTED_DENY, "right at last must clamp");
        overlay.handle_key(key(KeyCode::Left));
        assert_eq!(overlay.selected, SELECTED_ALWAYS);
        overlay.handle_key(key(KeyCode::Left));
        assert_eq!(overlay.selected, SELECTED_ALLOW);
        overlay.handle_key(key(KeyCode::Left));
        assert_eq!(overlay.selected, SELECTED_ALLOW, "left at first must clamp");
    }

    #[test]
    fn enter_with_selected_always_sends_always() {
        let (mut overlay, mut rx) = make_overlay();
        overlay.selected = SELECTED_ALWAYS;
        let (result, _) = overlay.handle_key(key(KeyCode::Enter));
        assert!(matches!(result, OverlayResult::Closed));
        assert_eq!(rx.try_recv().unwrap(), PermissionDecision::Always);
    }
}
