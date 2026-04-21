use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::theme::Theme;

pub(crate) const SLASH_COMMANDS: &[&str] = &[
    "new",
    "clear",
    "undo",
    "redo",
    "tree",
    "branch",
    "label",
    "plans",
    "show-plan",
    "approve",
    "execute-plan",
    "plan",
    "build",
    "sidebar",
    "sessions",
    "help",
    "quit",
];

const MAX_VISIBLE: usize = 8;

pub(crate) struct SlashPopup {
    filter: String,
    filtered: Vec<&'static str>,
    selected: usize,
}

#[derive(Debug)]
pub(crate) enum SlashAction {
    Move,
    Select(&'static str),
    Cancel,
    None,
}

impl SlashPopup {
    pub(crate) fn new() -> Self {
        Self { filter: String::new(), filtered: SLASH_COMMANDS.to_vec(), selected: 0 }
    }

    pub(crate) fn update_filter(&mut self, query: &str) {
        self.filter = query.to_string();
        let q = query.trim_start_matches('/').to_lowercase();
        self.filtered =
            SLASH_COMMANDS.iter().copied().filter(|c| c.to_lowercase().contains(&q)).collect();
        if self.filtered.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len() - 1;
        }
    }

    // Navigation is intentionally limited to Up/Down. `j`/`k` are reserved for
    // the composer to avoid UX confusion while the user is mid-typing a slash
    // command, even though the popup would capture keys first.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> SlashAction {
        match key.code {
            KeyCode::Up => {
                if self.selected > 0 {
                    self.selected -= 1;
                    SlashAction::Move
                } else {
                    SlashAction::None
                }
            }
            KeyCode::Down => {
                if self.selected + 1 < self.filtered.len() {
                    self.selected += 1;
                    SlashAction::Move
                } else {
                    SlashAction::None
                }
            }
            KeyCode::Enter | KeyCode::Tab => self
                .filtered
                .get(self.selected)
                .copied()
                .map_or(SlashAction::None, SlashAction::Select),
            KeyCode::Esc => SlashAction::Cancel,
            _ => SlashAction::None,
        }
    }

    pub(crate) fn desired_height(&self) -> u16 {
        let rows = self.filtered.len().min(MAX_VISIBLE);
        u16::try_from(rows + 2).unwrap_or(u16::MAX)
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let accent = convert_color(theme.accent_user);
        let dim = convert_color(theme.dim);
        let border = convert_color(theme.border);

        let lines: Vec<Line<'_>> = self
            .filtered
            .iter()
            .take(MAX_VISIBLE)
            .enumerate()
            .map(|(i, cmd)| {
                if i == self.selected {
                    let style = Style::default().fg(accent).add_modifier(Modifier::BOLD);
                    Line::from(vec![Span::styled("> ", style), Span::styled(*cmd, style)])
                } else {
                    let style = Style::default().fg(dim);
                    Line::from(vec![Span::raw("  "), Span::styled(*cmd, style)])
                }
            })
            .collect();

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border))
            .title(" /commands ");

        Paragraph::new(lines).block(block).render(area, buf);
    }
}

impl Default for SlashPopup {
    fn default() -> Self {
        Self::new()
    }
}

// `Theme` stores colors as `crossterm::Color::Rgb`; non-Rgb variants are
// unreachable in practice but fall back to `Reset` instead of panicking.
fn convert_color(c: crossterm::style::Color) -> ratatui::style::Color {
    match c {
        crossterm::style::Color::Rgb { r, g, b } => ratatui::style::Color::Rgb(r, g, b),
        _ => ratatui::style::Color::Reset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn default_shows_all_commands() {
        let popup = SlashPopup::new();
        assert_eq!(popup.filtered.len(), SLASH_COMMANDS.len());
        assert_eq!(popup.filtered, SLASH_COMMANDS.to_vec());
        assert_eq!(popup.selected, 0);
    }

    #[test]
    fn filter_branch_matches_only_branch() {
        let mut popup = SlashPopup::new();
        popup.update_filter("/branch");
        assert_eq!(popup.filtered, vec!["branch"]);
        assert_eq!(popup.selected, 0);
    }

    #[test]
    fn filter_nonexistent_returns_empty() {
        let mut popup = SlashPopup::new();
        popup.update_filter("/zzznomatch");
        assert!(popup.filtered.is_empty());
        assert_eq!(popup.selected, 0);
    }

    #[test]
    fn up_down_navigates() {
        let mut popup = SlashPopup::new();
        assert!(matches!(popup.handle_key(key(KeyCode::Down)), SlashAction::Move));
        assert_eq!(popup.selected, 1);

        assert!(matches!(popup.handle_key(key(KeyCode::Up)), SlashAction::Move));
        assert_eq!(popup.selected, 0);

        assert!(matches!(popup.handle_key(key(KeyCode::Up)), SlashAction::None));
        assert_eq!(popup.selected, 0);
    }

    #[test]
    fn enter_returns_selected() {
        let mut popup = SlashPopup::new();
        match popup.handle_key(key(KeyCode::Enter)) {
            SlashAction::Select(cmd) => assert_eq!(cmd, "new"),
            other => panic!("expected Select, got {other:?}"),
        }
    }

    #[test]
    fn esc_returns_cancel() {
        let mut popup = SlashPopup::new();
        assert!(matches!(popup.handle_key(key(KeyCode::Esc)), SlashAction::Cancel));
    }

    #[test]
    fn tab_also_selects() {
        let mut popup = SlashPopup::new();
        popup.update_filter("/branch");
        match popup.handle_key(key(KeyCode::Tab)) {
            SlashAction::Select(cmd) => assert_eq!(cmd, "branch"),
            other => panic!("expected Select, got {other:?}"),
        }
    }

    #[test]
    fn substring_filter_matches_multiple() {
        let mut popup = SlashPopup::new();
        popup.update_filter("/an");
        assert!(popup.filtered.contains(&"plan"));
        assert!(popup.filtered.contains(&"branch"));
        assert!(!popup.filtered.contains(&"new"));
    }

    #[test]
    fn filter_show_matches_plan_commands() {
        let mut popup = SlashPopup::new();
        popup.update_filter("/plan");
        assert!(popup.filtered.contains(&"plan"));
        assert!(popup.filtered.contains(&"plans"));
        assert!(popup.filtered.contains(&"show-plan"));
        assert!(popup.filtered.contains(&"execute-plan"));
    }

    #[test]
    fn update_filter_clamps_selection() {
        let mut popup = SlashPopup::new();
        popup.selected = SLASH_COMMANDS.len() - 1;
        popup.update_filter("/branch");
        assert_eq!(popup.filtered, vec!["branch"]);
        assert_eq!(popup.selected, 0);
    }

    #[test]
    fn desired_height_is_min_of_visible_plus_borders() {
        let popup = SlashPopup::new();
        assert_eq!(popup.desired_height(), 10);

        let mut popup = SlashPopup::new();
        popup.update_filter("/branch");
        assert_eq!(popup.desired_height(), 3);
    }

    #[test]
    fn enter_on_empty_returns_none() {
        let mut popup = SlashPopup::new();
        popup.update_filter("/zzznomatch");
        assert!(matches!(popup.handle_key(key(KeyCode::Enter)), SlashAction::None));
    }

    #[test]
    fn render_does_not_panic_on_small_area() {
        let popup = SlashPopup::new();
        let theme = Theme::dark();
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        popup.render(area, &mut buf, &theme);
    }
}
