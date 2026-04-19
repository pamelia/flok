use crossterm::event::{KeyCode, KeyEvent};
use flok_core::tool::QuestionRequest;
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

pub(crate) struct QuestionOverlay {
    question: String,
    options: Vec<String>,
    #[expect(dead_code, reason = "custom-answer input UI lands in a later wave")]
    allow_custom: bool,
    response_tx: Option<oneshot::Sender<String>>,
    selected: usize,
}

impl QuestionOverlay {
    pub(crate) fn new(req: QuestionRequest) -> Self {
        Self {
            question: req.question,
            options: req.options,
            allow_custom: req.allow_custom,
            response_tx: Some(req.response_tx),
            selected: 0,
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> (OverlayResult, Option<AppEvent>) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                (OverlayResult::Consumed, None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + 1 < self.options.len() {
                    self.selected += 1;
                }
                (OverlayResult::Consumed, None)
            }
            KeyCode::Enter => {
                let answer = self.options.get(self.selected).cloned().unwrap_or_default();
                self.respond(answer);
                (OverlayResult::Closed, None)
            }
            KeyCode::Esc => {
                self.respond(String::new());
                (OverlayResult::Closed, None)
            }
            _ => (OverlayResult::Consumed, None),
        }
    }

    fn respond(&mut self, answer: String) {
        if let Some(tx) = self.response_tx.take() {
            let _ = tx.send(answer);
        }
    }

    pub(crate) fn desired_height(&self, _width: u16) -> u16 {
        // borders (2) + question row (1) + blank row (1) + one row per option
        let opts = u16::try_from(self.options.len()).unwrap_or(u16::MAX);
        opts.saturating_add(4)
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if area.width < 4 || area.height < 3 {
            return;
        }

        let border = ratatui_color(theme.accent_tool);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Question ")
            .border_style(Style::default().fg(border));

        let inner = block.inner(area);
        block.render(area, buf);

        let accent =
            Style::default().fg(ratatui_color(theme.accent_user)).add_modifier(Modifier::BOLD);

        let mut lines: Vec<Line<'_>> = Vec::with_capacity(self.options.len() + 2);
        lines.push(Line::from(self.question.as_str()));
        lines.push(Line::from(""));
        for (i, opt) in self.options.iter().enumerate() {
            let prefix = if i == self.selected { "› " } else { "  " };
            if i == self.selected {
                lines.push(Line::from(vec![
                    Span::styled(prefix, accent),
                    Span::styled(opt.as_str(), accent),
                ]));
            } else {
                lines.push(Line::from(vec![Span::raw(prefix), Span::raw(opt.as_str())]));
            }
        }

        Paragraph::new(lines).wrap(Wrap { trim: false }).render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn make_overlay() -> (QuestionOverlay, oneshot::Receiver<String>) {
        let (tx, rx) = oneshot::channel();
        let req = QuestionRequest {
            question: "Pick one".to_string(),
            options: vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
            allow_custom: false,
            response_tx: tx,
        };
        (QuestionOverlay::new(req), rx)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn enter_sends_selected_option() {
        let (mut overlay, mut rx) = make_overlay();
        overlay.selected = 1;
        let (result, _) = overlay.handle_key(key(KeyCode::Enter));
        assert!(matches!(result, OverlayResult::Closed));
        assert_eq!(rx.try_recv().unwrap(), "beta");
    }

    #[test]
    fn esc_sends_empty_string() {
        let (mut overlay, mut rx) = make_overlay();
        let (result, _) = overlay.handle_key(key(KeyCode::Esc));
        assert!(matches!(result, OverlayResult::Closed));
        assert_eq!(rx.try_recv().unwrap(), "");
    }

    #[test]
    fn arrow_down_advances_selection() {
        let (mut overlay, _rx) = make_overlay();
        assert_eq!(overlay.selected, 0);
        let (result, _) = overlay.handle_key(key(KeyCode::Down));
        assert!(matches!(result, OverlayResult::Consumed));
        assert_eq!(overlay.selected, 1);
    }

    #[test]
    fn arrow_down_at_last_does_not_overflow() {
        let (mut overlay, _rx) = make_overlay();
        overlay.selected = overlay.options.len() - 1;
        let before = overlay.selected;
        let (result, _) = overlay.handle_key(key(KeyCode::Down));
        assert!(matches!(result, OverlayResult::Consumed));
        assert_eq!(overlay.selected, before, "down at last option must not advance");
    }
}
