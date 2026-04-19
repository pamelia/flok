use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use crate::theme::Theme;

#[derive(Debug, Default, Clone)]
pub(crate) struct SidebarState {
    pub(crate) session_title: String,
    pub(crate) model: String,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) session_cost_usd: f64,
    pub(crate) context_pct: f32,
    pub(crate) plan_mode: bool,
    pub(crate) team_members: Vec<(String, String)>,
    pub(crate) todos: Vec<String>,
    pub(crate) visible: bool,
}

pub(crate) fn render(state: &SidebarState, area: Rect, buf: &mut Buffer, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", state.session_title))
        .border_style(Style::default().fg(ratatui_color(theme.border)));
    let inner = block.inner(area);
    block.render(area, buf);

    let mut lines = vec![
        styled(format!("model  {}", state.model), theme.text),
        styled(format!("mode   {}", if state.plan_mode { "plan" } else { "build" }), theme.text),
        styled(format!("ctx    {:.0}%", state.context_pct), theme.text),
        styled(format!("in     {}", state.input_tokens), theme.text),
        styled(format!("out    {}", state.output_tokens), theme.text),
        styled(format!("cost   ${:.4}", state.session_cost_usd), theme.text),
    ];

    if !state.team_members.is_empty() {
        lines.push(styled(String::new(), theme.dim));
        lines.push(styled("team".to_string(), theme.accent_tool));
        for (name, status) in state.team_members.iter().take(6) {
            lines.push(styled(format!("  {name}: {status}"), theme.text_muted));
        }
    }

    if !state.todos.is_empty() {
        lines.push(styled(String::new(), theme.dim));
        lines.push(styled("todos".to_string(), theme.accent_user));
        for todo in state.todos.iter().take(8) {
            lines.push(styled(format!("  {todo}"), theme.text_muted));
        }
    }

    Paragraph::new(lines).wrap(Wrap { trim: false }).render(inner, buf);
}

fn styled(text: String, color: crossterm::style::Color) -> Line<'static> {
    Line::from(Span::styled(text, Style::default().fg(ratatui_color(color))))
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

    #[test]
    fn render_does_not_panic() {
        let area = Rect::new(0, 0, 32, 12);
        let mut buf = Buffer::empty(area);
        render(&SidebarState::default(), area, &mut buf, &Theme::dark());
    }
}
