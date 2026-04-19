use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

#[derive(Debug, Default, Clone)]
pub(crate) struct FooterState {
    pub(crate) plan_mode: bool,
    pub(crate) model: String,
    pub(crate) context_pct: f32,
    pub(crate) waiting: bool,
}

const PLAN_BADGE_TEXT: &str = " PLAN ";
const BUILD_BADGE_TEXT: &str = " BUILD ";
const HINT_SEPARATOR: &str = "   ";
/// Hint priority: kept first, dropped last. So `Esc cancel` is dropped first
/// when width gets tight, and `Ctrl+B sidebar` is the final hint to remain.
const HINTS: [&str; 4] = ["Ctrl+B sidebar", "Ctrl+C quit", "Tab plan", "Esc cancel"];

pub(crate) fn render(state: &FooterState, theme: &Theme, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let (left, right) = build_spans(state, theme, area.width);
    let left_width: usize = left.iter().map(|s| s.content.width()).sum();
    let right_width: usize = right.iter().map(|s| s.content.width()).sum();
    let total_width = area.width as usize;
    let pad = total_width.saturating_sub(left_width + right_width);

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(left.len() + right.len() + 1);
    spans.extend(left);
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    spans.extend(right);

    Line::from(spans).render(area, buf);
}

fn build_spans(
    state: &FooterState,
    theme: &Theme,
    width: u16,
) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let dim_style = Style::default().fg(ratatui_color(theme.dim));
    let assistant_style = Style::default().fg(ratatui_color(theme.accent_assistant));

    let (badge_text, badge_bg) = if state.plan_mode {
        (PLAN_BADGE_TEXT, theme.plan_badge)
    } else {
        (BUILD_BADGE_TEXT, theme.build_badge)
    };
    let badge_style = Style::default()
        .fg(ratatui::style::Color::Black)
        .bg(ratatui_color(badge_bg))
        .add_modifier(Modifier::BOLD);

    let mut left: Vec<Span<'static>> = Vec::new();
    left.push(Span::styled(badge_text.to_string(), badge_style));
    left.push(Span::styled(format!(" {}", state.model), dim_style));

    if width < 40 {
        return (left, Vec::new());
    }

    if state.context_pct > 0.0 {
        left.push(Span::styled(format!(" · ctx {:.0}%", state.context_pct), dim_style));
    }

    if width < 60 {
        return (left, Vec::new());
    }

    if state.waiting {
        left.push(Span::styled(" · streaming...".to_string(), assistant_style));
    }

    let n_visible = if width >= 100 {
        HINTS.len()
    } else {
        let left_w: usize = left.iter().map(|s| s.content.width()).sum();
        let mut chosen = 0;
        for n in (1..=HINTS.len()).rev() {
            let joined = HINTS[..n].join(HINT_SEPARATOR);
            // Require at least one cell of separation between left and right.
            if left_w.saturating_add(1).saturating_add(joined.width()) <= width as usize {
                chosen = n;
                break;
            }
        }
        chosen
    };

    let right = if n_visible == 0 {
        Vec::new()
    } else {
        vec![Span::styled(HINTS[..n_visible].join(HINT_SEPARATOR), dim_style)]
    };

    (left, right)
}

/// Convert a `crossterm::style::Color` (used in `Theme`) into a `ratatui` color
/// for use in `Style`. Mirrors the private helper in `theme.rs` because that
/// helper is module-private; duplicating it keeps `footer.rs` self-contained.
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
    use ratatui::layout::Position;
    use ratatui::style::Color as RColor;

    fn buffer_text(buf: &Buffer, y: u16) -> String {
        let mut s = String::new();
        for x in 0..buf.area.width {
            if let Some(cell) = buf.cell(Position::new(x, y)) {
                s.push_str(cell.symbol());
            }
        }
        s
    }

    fn make_state(plan_mode: bool, ctx: f32, waiting: bool) -> FooterState {
        FooterState { plan_mode, model: "sonnet".to_string(), context_pct: ctx, waiting }
    }

    #[test]
    fn plan_mode_renders_plan_badge() {
        let theme = Theme::dark();
        let state = make_state(true, 0.0, false);
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render(&state, &theme, area, &mut buf);

        let row = buffer_text(&buf, 0);
        assert!(row.starts_with(" PLAN "), "row: {row:?}");

        for x in 0..PLAN_BADGE_TEXT.len() as u16 {
            let cell = buf.cell(Position::new(x, 0)).expect("cell in range");
            assert_eq!(cell.bg, RColor::Rgb(245, 167, 66), "cell {x} bg");
            assert_eq!(cell.fg, RColor::Black, "cell {x} fg");
        }
    }

    #[test]
    fn build_mode_renders_build_badge() {
        let theme = Theme::dark();
        let state = make_state(false, 0.0, false);
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render(&state, &theme, area, &mut buf);

        let row = buffer_text(&buf, 0);
        assert!(row.starts_with(" BUILD "), "row: {row:?}");

        for x in 0..BUILD_BADGE_TEXT.len() as u16 {
            let cell = buf.cell(Position::new(x, 0)).expect("cell in range");
            assert_eq!(cell.bg, RColor::Rgb(127, 216, 143), "cell {x} bg");
            assert_eq!(cell.fg, RColor::Black, "cell {x} fg");
        }
    }

    #[test]
    fn narrow_width_drops_hints() {
        let theme = Theme::dark();
        let state = make_state(true, 50.0, true);
        let area = Rect::new(0, 0, 50, 1);
        let mut buf = Buffer::empty(area);
        render(&state, &theme, area, &mut buf);

        let row = buffer_text(&buf, 0);
        assert!(!row.contains("Ctrl+B"), "row: {row:?}");
        assert!(!row.contains("Ctrl+C"), "row: {row:?}");
        assert!(!row.contains("Tab plan"), "row: {row:?}");
        assert!(!row.contains("Esc cancel"), "row: {row:?}");
        assert!(!row.contains("streaming"), "row: {row:?}");
        // ctx is allowed at width 50 (>= 40).
        assert!(row.contains("ctx"), "row: {row:?}");
    }

    #[test]
    fn very_narrow_width_drops_context_and_hints() {
        let theme = Theme::dark();
        let state = make_state(true, 50.0, true);
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        render(&state, &theme, area, &mut buf);

        let row = buffer_text(&buf, 0);
        assert!(row.contains(" PLAN "), "row: {row:?}");
        assert!(row.contains("sonnet"), "row: {row:?}");
        assert!(!row.contains("ctx"), "row: {row:?}");
        assert!(!row.contains("streaming"), "row: {row:?}");
        assert!(!row.contains("Ctrl+B"), "row: {row:?}");
    }

    #[test]
    fn wide_width_shows_all_hints() {
        let theme = Theme::dark();
        let state = make_state(true, 50.0, false);
        let area = Rect::new(0, 0, 120, 1);
        let mut buf = Buffer::empty(area);
        render(&state, &theme, area, &mut buf);

        let row = buffer_text(&buf, 0);
        assert!(row.contains("Ctrl+B sidebar"), "row: {row:?}");
        assert!(row.contains("Ctrl+C quit"), "row: {row:?}");
        assert!(row.contains("Tab plan"), "row: {row:?}");
        assert!(row.contains("Esc cancel"), "row: {row:?}");
    }

    #[test]
    fn context_pct_zero_is_hidden() {
        let theme = Theme::dark();
        let state = make_state(true, 0.0, false);
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render(&state, &theme, area, &mut buf);

        let row = buffer_text(&buf, 0);
        assert!(!row.contains("ctx"), "row: {row:?}");
    }

    #[test]
    fn waiting_renders_streaming_indicator() {
        let theme = Theme::dark();
        let state = make_state(true, 0.0, true);
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render(&state, &theme, area, &mut buf);

        let row = buffer_text(&buf, 0);
        assert!(row.contains("streaming..."), "row: {row:?}");

        let start = row.find("streaming").expect("streaming present");
        let cell = buf.cell(Position::new(start as u16, 0)).expect("cell in range");
        assert_eq!(cell.fg, RColor::Rgb(250, 178, 131));
    }

    #[test]
    fn medium_width_drops_some_hints_in_priority_order() {
        let theme = Theme::dark();
        let state = make_state(false, 0.0, false);
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        render(&state, &theme, area, &mut buf);

        let row = buffer_text(&buf, 0);
        assert!(row.contains("Ctrl+B sidebar"), "row: {row:?}");
        assert!(!row.contains("Esc cancel"), "row: {row:?}");
    }

    #[test]
    fn render_with_zero_area_does_not_panic() {
        let theme = Theme::dark();
        let state = make_state(true, 0.0, false);
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        render(&state, &theme, area, &mut buf);
        // No assertions required: must not panic.
    }
}
