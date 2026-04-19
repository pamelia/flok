pub(crate) mod history;
pub(crate) mod textarea;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};

use crate::app_event::AppEvent;
use crate::theme::Theme;

use self::textarea::TextArea;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatComposer {
    textarea: TextArea,
    disabled: bool,
}

impl ChatComposer {
    pub(crate) fn new() -> Self {
        Self { textarea: TextArea::new(), disabled: false }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.textarea.is_empty()
    }

    pub(crate) fn text(&self) -> String {
        self.textarea.text()
    }

    pub(crate) fn clear(&mut self) {
        self.textarea.clear();
    }

    pub(crate) fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
    }

    pub(crate) fn height(&self, width: u16) -> u16 {
        self.textarea.height(width) + 2
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Option<AppEvent> {
        if self.disabled {
            return None;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match (key.code, ctrl, shift) {
            (KeyCode::Enter, _, true) => {
                self.textarea.insert_newline();
                None
            }
            (KeyCode::Enter, _, false) => {
                let text = self.textarea.text();
                self.textarea.clear();
                Some(AppEvent::Submit(text))
            }
            (KeyCode::Char('u'), true, _) => {
                self.textarea.clear_input();
                None
            }
            (KeyCode::Char('w'), true, _) => {
                self.textarea.delete_prev_word();
                None
            }
            (KeyCode::Char('a'), true, _) | (KeyCode::Home, _, _) => {
                self.textarea.move_to_line_start();
                None
            }
            (KeyCode::Char('e'), true, _) | (KeyCode::End, _, _) => {
                self.textarea.move_to_line_end();
                None
            }
            (KeyCode::Char('k'), true, _) => {
                self.textarea.kill_to_eol();
                None
            }
            (KeyCode::Char('y'), true, _) => {
                self.textarea.yank();
                None
            }
            (KeyCode::Char(ch), false, _) => {
                self.textarea.insert_char(ch);
                None
            }
            (KeyCode::Backspace, _, _) => {
                self.textarea.backspace();
                None
            }
            (KeyCode::Delete, _, _) => {
                self.textarea.delete();
                None
            }
            (KeyCode::Left, _, _) => {
                self.textarea.move_cursor_left();
                None
            }
            (KeyCode::Right, _, _) => {
                self.textarea.move_cursor_right();
                None
            }
            (KeyCode::Up, _, _) => {
                self.textarea.move_cursor_up();
                None
            }
            (KeyCode::Down, _, _) => {
                self.textarea.move_cursor_down();
                None
            }
            _ => None,
        }
    }

    pub(crate) fn handle_paste(&mut self, s: &str) {
        if self.disabled {
            return;
        }

        self.textarea.insert_str(s);
    }

    pub(crate) fn render(&self, theme: &Theme, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let border_style = if self.disabled {
            Style::default().fg(ratatui_color(theme.dim))
        } else {
            Style::default().fg(ratatui_color(theme.border_active))
        };
        let title_style = if self.disabled {
            Style::default().fg(ratatui_color(theme.dim))
        } else {
            Style::default().fg(ratatui_color(theme.text)).add_modifier(Modifier::BOLD)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Line::from(vec![Span::styled("> ".to_string(), title_style)]));
        let inner = block.inner(area);
        block.render(area, buf);
        self.textarea.render(inner, buf);
    }
}

fn ratatui_color(color: crossterm::style::Color) -> ratatui::style::Color {
    use crossterm::style::Color as Cc;

    match color {
        Cc::Reset => ratatui::style::Color::Reset,
        Cc::Black => ratatui::style::Color::Black,
        Cc::DarkGrey => ratatui::style::Color::DarkGray,
        Cc::Red | Cc::DarkRed => ratatui::style::Color::Red,
        Cc::Green | Cc::DarkGreen => ratatui::style::Color::Green,
        Cc::Yellow | Cc::DarkYellow => ratatui::style::Color::Yellow,
        Cc::Blue | Cc::DarkBlue => ratatui::style::Color::Blue,
        Cc::Magenta | Cc::DarkMagenta => ratatui::style::Color::Magenta,
        Cc::Cyan | Cc::DarkCyan => ratatui::style::Color::Cyan,
        Cc::Grey => ratatui::style::Color::Gray,
        Cc::White => ratatui::style::Color::White,
        Cc::Rgb { r, g, b } => ratatui::style::Color::Rgb(r, g, b),
        Cc::AnsiValue(value) => ratatui::style::Color::Indexed(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{buffer::Buffer, layout::Position};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    fn shift_enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
    }

    #[test]
    fn enter_submits_and_clears_textarea() {
        let mut composer = ChatComposer::new();
        composer.handle_paste("hello world");

        let event = composer.handle_key(key(KeyCode::Enter));

        match event {
            Some(AppEvent::Submit(text)) => assert_eq!(text, "hello world"),
            other => panic!("expected submit event, got {other:?}"),
        }
        assert!(composer.is_empty());
    }

    #[test]
    fn shift_enter_inserts_newline_without_submitting() {
        let mut composer = ChatComposer::new();
        composer.handle_paste("hello");

        let event = composer.handle_key(shift_enter());

        assert!(event.is_none());
        assert_eq!(composer.text(), "hello\n");
    }

    #[test]
    fn ctrl_u_clears_input() {
        let mut composer = ChatComposer::new();
        composer.handle_paste("hello");

        let event = composer.handle_key(ctrl('u'));

        assert!(event.is_none());
        assert!(composer.is_empty());
    }

    #[test]
    fn ctrl_a_and_ctrl_e_move_to_line_start_and_end() {
        let mut composer = ChatComposer::new();
        composer.handle_paste("hello");

        assert!(composer.handle_key(ctrl('a')).is_none());
        assert!(composer.handle_key(key(KeyCode::Char('X'))).is_none());
        assert_eq!(composer.text(), "Xhello");

        assert!(composer.handle_key(ctrl('e')).is_none());
        assert!(composer.handle_key(key(KeyCode::Char('Y'))).is_none());
        assert_eq!(composer.text(), "XhelloY");
    }

    #[test]
    fn disabled_state_swallows_keys_without_mutation() {
        let mut composer = ChatComposer::new();
        composer.handle_paste("hello");
        composer.set_disabled(true);

        let event = composer.handle_key(key(KeyCode::Char('x')));

        assert!(event.is_none());
        assert_eq!(composer.text(), "hello");
    }

    #[test]
    fn paste_inserts_multiline_content() {
        let mut composer = ChatComposer::new();

        composer.handle_paste("alpha\nbeta");

        assert_eq!(composer.text(), "alpha\nbeta");
        assert_eq!(composer.height(80), 4);
    }

    #[test]
    fn disabled_state_swallows_paste_without_mutation() {
        let mut composer = ChatComposer::new();
        composer.handle_paste("hello");
        composer.set_disabled(true);

        composer.handle_paste(" world");

        assert_eq!(composer.text(), "hello");
    }

    #[test]
    fn render_draws_block_and_textarea_content() {
        let mut composer = ChatComposer::new();
        composer.handle_paste("hi");

        let theme = Theme::dark();
        let area = Rect::new(0, 0, 12, 3);
        let mut buf = Buffer::empty(area);
        composer.render(&theme, area, &mut buf);

        let top_row: String = (0..area.width)
            .map(|x| buf.cell(Position::new(x, 0)).expect("cell in range").symbol())
            .collect();
        let input_row: String = (0..area.width)
            .map(|x| buf.cell(Position::new(x, 1)).expect("cell in range").symbol())
            .collect();

        assert!(top_row.contains('>'), "top row: {top_row:?}");
        assert!(input_row.contains("hi"), "input row: {input_row:?}");
    }
}
