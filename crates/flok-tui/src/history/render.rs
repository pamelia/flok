use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    hash::{DefaultHasher, Hash, Hasher},
};

use crossterm::style::Color as CrosstermColor;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use textwrap::Options;

use crate::{
    history::{HistoryItem, SystemLevel, TeamEventKind},
    theme::Theme,
};

const CACHE_CAPACITY: usize = 256;
const TOOL_BODY_VISIBLE_LINES: usize = 5;
const INDENT: &str = "  ";

type CacheKey = (u64, u16);
type CachedRender = (Vec<Line<'static>>, u16);

thread_local! {
    static CACHE: RefCell<HashMap<CacheKey, CachedRender>> = RefCell::new(HashMap::new());
    static CACHE_ORDER: RefCell<VecDeque<CacheKey>> = const { RefCell::new(VecDeque::new()) };
}

pub(crate) fn lines(item: &HistoryItem, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    cached_render(item, width, theme).0
}

pub(crate) fn height(item: &HistoryItem, width: u16, theme: &Theme) -> u16 {
    cached_render(item, width, theme).1
}

fn cached_render(item: &HistoryItem, width: u16, theme: &Theme) -> (Vec<Line<'static>>, u16) {
    let key = (fingerprint(item), width);

    if let Some(cached) = CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return cached;
    }

    let rendered = render_lines(item, width, theme);
    let rendered_height = u16::try_from(rendered.len()).unwrap_or(u16::MAX);

    CACHE.with(|cache| {
        cache.borrow_mut().insert(key, (rendered.clone(), rendered_height));
    });
    CACHE_ORDER.with(|order| {
        let mut order = order.borrow_mut();
        order.push_back(key);
        if order.len() > CACHE_CAPACITY {
            if let Some(oldest) = order.pop_front() {
                CACHE.with(|cache| {
                    cache.borrow_mut().remove(&oldest);
                });
            }
        }
    });

    (rendered, rendered_height)
}

fn render_lines(item: &HistoryItem, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    match item {
        HistoryItem::User { text } => {
            render_plain_message(text, width, theme, "▌ you", theme.accent_user)
        }
        HistoryItem::Assistant { text, markdown } => {
            render_assistant_message(text, *markdown, width, theme)
        }
        HistoryItem::System { text, level } => render_system_message(text, *level, width, theme),
        HistoryItem::ToolCall { name, preview, is_error, duration_ms } => {
            render_tool_call(name, preview, *is_error, *duration_ms, width, theme)
        }
        HistoryItem::TeamEvent { kind, agent, detail } => {
            render_team_event(*kind, agent, detail, theme)
        }
        HistoryItem::Divider => render_divider(width, theme),
    }
}

fn render_plain_message(
    text: &str,
    width: u16,
    theme: &Theme,
    header: &str,
    header_color: CrosstermColor,
) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    rendered
        .push(Line::from(vec![Span::styled(header.to_string(), bold_color_style(header_color))]));
    rendered.extend(
        wrap_text(text, width.saturating_sub(4))
            .into_iter()
            .map(|segment| styled_line(format!("{INDENT}{segment}"), text_style(theme))),
    );
    rendered.push(Line::default());
    rendered
}

fn render_assistant_message(
    text: &str,
    markdown: bool,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut rendered = vec![Line::from(vec![Span::styled(
        "▌ assistant".to_string(),
        bold_color_style(theme.accent_assistant),
    )])];

    if markdown {
        rendered.extend(
            crate::markdown::render_markdown(text, width.saturating_sub(2).max(1), theme)
                .into_iter()
                .map(indent_markdown_line),
        );
    } else {
        rendered.extend(
            wrap_text(text, width.saturating_sub(4))
                .into_iter()
                .map(|segment| styled_line(format!("{INDENT}{segment}"), text_style(theme))),
        );
    }

    rendered.push(Line::default());
    rendered
}

fn render_system_message(
    text: &str,
    level: SystemLevel,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let (icon, color) = match level {
        SystemLevel::Info => ("ℹ", theme.info),
        SystemLevel::Warn => ("⚠", theme.warn),
        SystemLevel::Error => ("✖", theme.error),
    };
    let style = color_style(color);

    wrap_text(text, width.saturating_sub(2))
        .into_iter()
        .enumerate()
        .map(|(index, segment)| {
            let prefix =
                if index == 0 { format!("{icon} {segment}") } else { format!("{INDENT}{segment}") };
            styled_line(prefix, style)
        })
        .collect()
}

fn render_tool_call(
    name: &str,
    preview: &str,
    is_error: bool,
    duration_ms: Option<u64>,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let header_color = if is_error { theme.error } else { theme.accent_tool };
    let mut header_spans = vec![Span::styled(format!("▸ {name}"), color_style(header_color))];
    if let Some(duration_ms) = duration_ms {
        header_spans.push(Span::styled(format!(" ({duration_ms}ms)"), dim_style(theme)));
    }

    let mut rendered = vec![Line::from(header_spans)];
    let wrapped_preview = wrap_text(preview, width.saturating_sub(2));

    if wrapped_preview.len() > TOOL_BODY_VISIBLE_LINES {
        let visible_preview_lines = TOOL_BODY_VISIBLE_LINES.saturating_sub(1);
        let hidden_lines = wrapped_preview.len().saturating_sub(visible_preview_lines);
        rendered.extend(
            wrapped_preview
                .iter()
                .take(visible_preview_lines)
                .map(|segment| styled_line(format!("{INDENT}{segment}"), muted_text_style(theme))),
        );
        rendered
            .push(styled_line(format!("{INDENT}… ({hidden_lines} more lines)"), dim_style(theme)));
    } else {
        rendered.extend(
            wrapped_preview
                .into_iter()
                .map(|segment| styled_line(format!("{INDENT}{segment}"), muted_text_style(theme))),
        );
    }

    rendered
}

fn render_team_event(
    kind: TeamEventKind,
    agent: &str,
    detail: &str,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let (icon, color) = match kind {
        TeamEventKind::Created => ("◆", theme.accent_tool),
        TeamEventKind::Completed => ("✓", theme.info),
        TeamEventKind::Failed => ("✗", theme.error),
    };

    vec![Line::from(vec![
        Span::styled(format!("{icon} "), color_style(color)),
        Span::styled(format!("{agent}: {detail}"), text_style(theme)),
    ])]
}

fn render_divider(width: u16, theme: &Theme) -> Vec<Line<'static>> {
    vec![styled_line("─".repeat(usize::from(width)), color_style(theme.divider))]
}

fn wrap_text(text: &str, width: u16) -> Vec<String> {
    let wrapped = textwrap::wrap(text, Options::new(usize::from(width.max(1))).break_words(true));
    if wrapped.is_empty() {
        vec![String::new()]
    } else {
        wrapped.into_iter().map(std::borrow::Cow::into_owned).collect()
    }
}

fn fingerprint(item: &HistoryItem) -> u64 {
    let mut hasher = DefaultHasher::new();
    item.hash(&mut hasher);
    hasher.finish()
}

fn indent_markdown_line(mut line: Line<'static>) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::raw(INDENT.to_string()));
    spans.append(&mut line.spans);
    line.spans = spans;
    line
}

fn styled_line(text: String, style: Style) -> Line<'static> {
    Line::from(vec![Span::styled(text, style)])
}

fn color_style(color: CrosstermColor) -> Style {
    Style::default().fg(ratatui_color(color))
}

fn bold_color_style(color: CrosstermColor) -> Style {
    color_style(color).add_modifier(Modifier::BOLD)
}

fn text_style(theme: &Theme) -> Style {
    color_style(theme.text)
}

fn muted_text_style(theme: &Theme) -> Style {
    color_style(theme.text_muted)
}

fn dim_style(theme: &Theme) -> Style {
    color_style(theme.dim)
}

fn ratatui_color(color: CrosstermColor) -> ratatui::style::Color {
    match color {
        CrosstermColor::Reset => ratatui::style::Color::Reset,
        CrosstermColor::Black => ratatui::style::Color::Black,
        CrosstermColor::DarkGrey => ratatui::style::Color::DarkGray,
        CrosstermColor::Red | CrosstermColor::DarkRed => ratatui::style::Color::Red,
        CrosstermColor::Green | CrosstermColor::DarkGreen => ratatui::style::Color::Green,
        CrosstermColor::Yellow | CrosstermColor::DarkYellow => ratatui::style::Color::Yellow,
        CrosstermColor::Blue | CrosstermColor::DarkBlue => ratatui::style::Color::Blue,
        CrosstermColor::Magenta | CrosstermColor::DarkMagenta => ratatui::style::Color::Magenta,
        CrosstermColor::Cyan | CrosstermColor::DarkCyan => ratatui::style::Color::Cyan,
        CrosstermColor::Grey => ratatui::style::Color::Gray,
        CrosstermColor::White => ratatui::style::Color::White,
        CrosstermColor::Rgb { r, g, b } => ratatui::style::Color::Rgb(r, g, b),
        CrosstermColor::AnsiValue(value) => ratatui::style::Color::Indexed(value),
    }
}

#[cfg(test)]
fn clear_cache() {
    CACHE.with(|cache| cache.borrow_mut().clear());
    CACHE_ORDER.with(|order| order.borrow_mut().clear());
}

#[cfg(test)]
fn cache_len() -> usize {
    CACHE.with(|cache| cache.borrow().len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color as RColor;

    fn theme() -> Theme {
        Theme::dark()
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn user_has_header_and_body_and_trailing_blank() {
        clear_cache();
        let rendered = lines(&HistoryItem::user("hello world"), 20, &theme());

        assert_eq!(rendered.len(), 3);
        assert_eq!(rendered[0].spans.len(), 1);
        assert_eq!(line_text(&rendered[0]), "▌ you");
        assert_eq!(rendered[0].spans[0].style.fg, Some(RColor::Rgb(92, 156, 245)));
        assert!(rendered[0].spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line_text(&rendered[1]), "  hello world");
        assert_eq!(line_text(&rendered[2]), "");
    }

    #[test]
    fn assistant_nonmarkdown_wraps_long_text() {
        clear_cache();
        let item = HistoryItem::Assistant { text: "abcdefghijkl".to_string(), markdown: false };
        let rendered = lines(&item, 8, &theme());

        assert_eq!(line_text(&rendered[0]), "▌ assistant");
        assert_eq!(rendered[0].spans[0].style.fg, Some(RColor::Rgb(250, 178, 131)));
        assert_eq!(line_text(&rendered[1]), "  abcd");
        assert_eq!(line_text(&rendered[2]), "  efgh");
        assert_eq!(line_text(&rendered[3]), "  ijkl");
        assert_eq!(line_text(&rendered[4]), "");
    }

    #[test]
    fn system_error_uses_cross_icon() {
        clear_cache();
        let rendered = lines(&HistoryItem::system_error("boom"), 20, &theme());

        assert_eq!(rendered.len(), 1);
        assert_eq!(line_text(&rendered[0]), "✖ boom");
        assert_eq!(rendered[0].spans[0].style.fg, Some(RColor::Rgb(224, 108, 117)));
    }

    #[test]
    fn tool_call_truncates_to_five_body_lines_when_longer() {
        clear_cache();
        let item = HistoryItem::ToolCall {
            name: "bash".to_string(),
            preview: "111111 222222 333333 444444 555555 666666".to_string(),
            is_error: false,
            duration_ms: None,
        };
        let rendered = lines(&item, 8, &theme());

        assert_eq!(rendered.len(), 6);
        assert_eq!(line_text(&rendered[0]), "▸ bash");
        assert_eq!(line_text(&rendered[1]), "  111111");
        assert_eq!(line_text(&rendered[2]), "  222222");
        assert_eq!(line_text(&rendered[3]), "  333333");
        assert_eq!(line_text(&rendered[4]), "  444444");
        assert_eq!(line_text(&rendered[5]), "  … (2 more lines)");
    }

    #[test]
    fn tool_call_error_header_uses_error_color() {
        clear_cache();
        let item = HistoryItem::ToolCall {
            name: "bash".to_string(),
            preview: "oops".to_string(),
            is_error: true,
            duration_ms: None,
        };
        let rendered = lines(&item, 20, &theme());

        assert_eq!(rendered[0].spans.len(), 1);
        assert_eq!(line_text(&rendered[0]), "▸ bash");
        assert_eq!(rendered[0].spans[0].style.fg, Some(RColor::Rgb(224, 108, 117)));
    }

    #[test]
    fn team_event_created_completed_failed_render_correctly() {
        clear_cache();
        let created = lines(
            &HistoryItem::TeamEvent {
                kind: TeamEventKind::Created,
                agent: "agent-a".to_string(),
                detail: "started".to_string(),
            },
            40,
            &theme(),
        );
        let completed = lines(
            &HistoryItem::TeamEvent {
                kind: TeamEventKind::Completed,
                agent: "agent-b".to_string(),
                detail: "finished".to_string(),
            },
            40,
            &theme(),
        );
        let failed = lines(
            &HistoryItem::TeamEvent {
                kind: TeamEventKind::Failed,
                agent: "agent-c".to_string(),
                detail: "errored".to_string(),
            },
            40,
            &theme(),
        );

        assert_eq!(line_text(&created[0]), "◆ agent-a: started");
        assert_eq!(created[0].spans[0].style.fg, Some(RColor::Rgb(157, 124, 216)));
        assert_eq!(line_text(&completed[0]), "✓ agent-b: finished");
        assert_eq!(completed[0].spans[0].style.fg, Some(RColor::Rgb(86, 182, 194)));
        assert_eq!(line_text(&failed[0]), "✗ agent-c: errored");
        assert_eq!(failed[0].spans[0].style.fg, Some(RColor::Rgb(224, 108, 117)));
    }

    #[test]
    fn divider_fills_width() {
        clear_cache();
        let rendered = lines(&HistoryItem::Divider, 5, &theme());

        assert_eq!(rendered.len(), 1);
        assert_eq!(line_text(&rendered[0]), "─────");
        assert_eq!(rendered[0].spans[0].style.fg, Some(RColor::Rgb(60, 60, 60)));
    }

    #[test]
    fn height_matches_lines_len() {
        clear_cache();
        let item = HistoryItem::ToolCall {
            name: "bash".to_string(),
            preview: "preview output".to_string(),
            is_error: false,
            duration_ms: Some(12),
        };

        let rendered = lines(&item, 20, &theme());
        assert_eq!(height(&item, 20, &theme()), rendered.len() as u16);
    }

    #[test]
    fn cache_returns_same_result_for_same_inputs() {
        clear_cache();
        let item = HistoryItem::system_info("cached result");

        let first = lines(&item, 20, &theme());
        assert_eq!(cache_len(), 1);

        let second = lines(&item, 20, &theme());
        assert_eq!(cache_len(), 1);
        assert_eq!(line_text(&first[0]), line_text(&second[0]));
        assert_eq!(first[0].spans[0].style.fg, second[0].spans[0].style.fg);
    }
}
