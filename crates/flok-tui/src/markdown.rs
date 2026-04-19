use crossterm::style::Color as CrosstermColor;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

const BLOCKQUOTE_PREFIX: &str = "│ ";
const BLOCKQUOTE_PREFIX_WIDTH: usize = 2;
const CODE_INDENT: &str = "  ";

pub(crate) fn render_markdown(text: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    if text.is_empty() {
        return Vec::new();
    }
    let parser = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS);
    let mut renderer = Renderer::new(theme, width);
    for event in parser {
        renderer.handle(event);
    }
    renderer.finish()
}

#[derive(Clone, Copy, Debug)]
enum ListKind {
    Unordered,
    Ordered(u64),
}

struct Renderer<'a> {
    theme: &'a Theme,
    width: u16,
    out: Vec<Line<'static>>,
    inline: Vec<Span<'static>>,
    bold_depth: usize,
    italic_depth: usize,
    strikethrough_depth: usize,
    link_depth: usize,
    heading: Option<HeadingLevel>,
    blockquote_depth: usize,
    list_stack: Vec<ListKind>,
    pending_item_marker: Option<String>,
    item_indent_stack: Vec<usize>,
    in_code_block: bool,
    code_buffer: String,
}

impl<'a> Renderer<'a> {
    fn new(theme: &'a Theme, width: u16) -> Self {
        Self {
            theme,
            width,
            out: Vec::new(),
            inline: Vec::new(),
            bold_depth: 0,
            italic_depth: 0,
            strikethrough_depth: 0,
            link_depth: 0,
            heading: None,
            blockquote_depth: 0,
            list_stack: Vec::new(),
            pending_item_marker: None,
            item_indent_stack: Vec::new(),
            in_code_block: false,
            code_buffer: String::new(),
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        while self.out.last().is_some_and(is_blank_line) {
            self.out.pop();
        }
        self.out
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(&tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(s) => self.handle_text(s.to_string()),
            Event::Code(s) => self.push_inline_span(
                s.to_string(),
                Style::default()
                    .fg(ratatui_color(self.theme.code_fg))
                    .bg(ratatui_color(self.theme.code_bg)),
            ),
            Event::SoftBreak => self.push_inline_text(" ".to_string()),
            Event::HardBreak => self.flush_inline_block(false),
            Event::Rule => self.emit_rule(),
            Event::TaskListMarker(checked) => {
                let marker = if checked { "[x] " } else { "[ ] " };
                self.push_inline_text(marker.to_string());
            }
            Event::Html(_)
            | Event::InlineHtml(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::FootnoteReference(_) => {}
        }
    }

    fn start_tag(&mut self, tag: &Tag<'_>) {
        if is_block_tag(tag) && self.has_pending_inline() {
            self.flush_inline_block(false);
        }

        match tag {
            Tag::Heading { level, .. } => {
                self.ensure_blank_separator();
                self.heading = Some(*level);
            }
            Tag::CodeBlock(_) => {
                self.ensure_blank_separator();
                self.in_code_block = true;
                self.code_buffer.clear();
            }
            Tag::BlockQuote(_) => {
                self.blockquote_depth += 1;
            }
            Tag::List(start) => {
                let kind = (*start).map_or(ListKind::Unordered, ListKind::Ordered);
                self.list_stack.push(kind);
            }
            Tag::Item => {
                let marker = match self.list_stack.last_mut() {
                    Some(ListKind::Unordered) => "  • ".to_string(),
                    Some(ListKind::Ordered(n)) => {
                        let m = format!("  {n}. ");
                        *n += 1;
                        m
                    }
                    None => String::new(),
                };
                let marker_width = UnicodeWidthStr::width(marker.as_str());
                self.item_indent_stack.push(marker_width);
                self.pending_item_marker = Some(marker);
            }
            Tag::Strong => self.bold_depth += 1,
            Tag::Emphasis => self.italic_depth += 1,
            Tag::Strikethrough => self.strikethrough_depth += 1,
            Tag::Link { .. } => self.link_depth += 1,
            Tag::Image { .. } => self.push_inline_text("[img: ".to_string()),
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush_inline_block(true),
            TagEnd::Heading(_) => {
                self.flush_inline_block(false);
                self.heading = None;
            }
            TagEnd::CodeBlock => {
                self.emit_code_block();
                self.in_code_block = false;
                self.out.push(Line::default());
            }
            TagEnd::BlockQuote(_) => {
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
            }
            TagEnd::Item => {
                if self.has_pending_inline() {
                    self.flush_inline_block(false);
                }
                self.item_indent_stack.pop();
            }
            TagEnd::Strong => self.bold_depth = self.bold_depth.saturating_sub(1),
            TagEnd::Emphasis => self.italic_depth = self.italic_depth.saturating_sub(1),
            TagEnd::Strikethrough => {
                self.strikethrough_depth = self.strikethrough_depth.saturating_sub(1);
            }
            TagEnd::Link => self.link_depth = self.link_depth.saturating_sub(1),
            TagEnd::Image => self.push_inline_text("]".to_string()),
            _ => {}
        }
    }

    fn has_pending_inline(&self) -> bool {
        !self.inline.is_empty() || self.pending_item_marker.is_some()
    }

    fn handle_text(&mut self, text: String) {
        if self.in_code_block {
            self.code_buffer.push_str(&text);
            return;
        }
        self.push_inline_text(text);
    }

    fn push_inline_text(&mut self, text: String) {
        let style = self.text_style();
        self.inline.push(Span::styled(text, style));
    }

    fn push_inline_span(&mut self, text: String, style: Style) {
        self.inline.push(Span::styled(text, style));
    }

    fn text_style(&self) -> Style {
        if let Some(level) = self.heading {
            return heading_style(level, self.theme);
        }
        self.inline_style()
    }

    fn inline_style(&self) -> Style {
        let mut modifier = Modifier::empty();
        if self.bold_depth > 0 {
            modifier |= Modifier::BOLD;
        }
        if self.italic_depth > 0 {
            modifier |= Modifier::ITALIC;
        }
        if self.strikethrough_depth > 0 {
            modifier |= Modifier::CROSSED_OUT;
        }
        if self.link_depth > 0 {
            modifier |= Modifier::UNDERLINED;
        }

        let fg = if self.link_depth > 0 {
            Some(ratatui_color(self.theme.secondary))
        } else if self.bold_depth > 0 {
            Some(ratatui_color(self.theme.bold_fg))
        } else {
            None
        };

        let mut style = Style::default().add_modifier(modifier);
        if let Some(fg) = fg {
            style = style.fg(fg);
        }
        style
    }

    fn ensure_blank_separator(&mut self) {
        if !self.out.is_empty() && !self.out.last().is_some_and(is_blank_line) {
            self.out.push(Line::default());
        }
    }

    fn emit_rule(&mut self) {
        let line = "─".repeat(usize::from(self.width.max(1)));
        let style = Style::default().fg(ratatui_color(self.theme.divider));
        self.out.push(Line::from(Span::styled(line, style)));
    }

    fn emit_code_block(&mut self) {
        let buf = std::mem::take(&mut self.code_buffer);
        let trimmed = buf.strip_suffix('\n').unwrap_or(buf.as_str());
        let style = Style::default()
            .fg(ratatui_color(self.theme.code_fg))
            .bg(ratatui_color(self.theme.code_bg));

        for line in trimmed.split('\n') {
            let content = format!("{CODE_INDENT}{line}");
            self.out.push(Line::from(Span::styled(content, style)));
        }
    }

    fn build_prefix_first(&self) -> (Vec<Span<'static>>, usize) {
        let mut spans = Vec::new();
        let mut width = 0;

        if self.blockquote_depth > 0 {
            let bq = BLOCKQUOTE_PREFIX.repeat(self.blockquote_depth);
            let bw = BLOCKQUOTE_PREFIX_WIDTH * self.blockquote_depth;
            spans.push(Span::styled(bq, Style::default().fg(ratatui_color(self.theme.dim))));
            width += bw;
        }

        if let Some(marker) = &self.pending_item_marker {
            let parents_len = self.item_indent_stack.len().saturating_sub(1);
            let parent_indent: usize = self.item_indent_stack.iter().take(parents_len).sum();
            if parent_indent > 0 {
                spans.push(Span::raw(" ".repeat(parent_indent)));
                width += parent_indent;
            }
            let marker_width = UnicodeWidthStr::width(marker.as_str());
            spans.push(Span::raw(marker.clone()));
            width += marker_width;
        } else {
            let total: usize = self.item_indent_stack.iter().sum();
            if total > 0 {
                spans.push(Span::raw(" ".repeat(total)));
                width += total;
            }
        }

        (spans, width)
    }

    fn build_prefix_cont(&self) -> (Vec<Span<'static>>, usize) {
        let mut spans = Vec::new();
        let mut width = 0;

        if self.blockquote_depth > 0 {
            let bq = BLOCKQUOTE_PREFIX.repeat(self.blockquote_depth);
            let bw = BLOCKQUOTE_PREFIX_WIDTH * self.blockquote_depth;
            spans.push(Span::styled(bq, Style::default().fg(ratatui_color(self.theme.dim))));
            width += bw;
        }

        let total: usize = self.item_indent_stack.iter().sum();
        if total > 0 {
            spans.push(Span::raw(" ".repeat(total)));
            width += total;
        }

        (spans, width)
    }

    fn flush_inline_block(&mut self, trailing_blank: bool) {
        let spans = std::mem::take(&mut self.inline);
        let (first_prefix, first_width) = self.build_prefix_first();
        let (cont_prefix, cont_width) = self.build_prefix_cont();
        self.pending_item_marker = None;

        let total_width = usize::from(self.width.saturating_sub(2).max(1));
        let first_text_width = total_width.saturating_sub(first_width).max(1);
        let cont_text_width = total_width.saturating_sub(cont_width).max(1);

        if spans.is_empty() {
            if !first_prefix.is_empty() {
                self.out.push(Line::from(first_prefix));
            }
        } else {
            let wrapped = wrap_spans(spans, first_text_width, cont_text_width);
            for (i, mut line_spans) in wrapped.into_iter().enumerate() {
                let mut full = if i == 0 { first_prefix.clone() } else { cont_prefix.clone() };
                full.append(&mut line_spans);
                self.out.push(Line::from(full));
            }
        }

        if trailing_blank {
            self.out.push(Line::default());
        }
    }
}

fn is_block_tag(tag: &Tag<'_>) -> bool {
    matches!(tag, Tag::List(_) | Tag::BlockQuote(_) | Tag::CodeBlock(_) | Tag::Heading { .. })
}

fn heading_style(level: HeadingLevel, theme: &Theme) -> Style {
    let mut modifier = Modifier::BOLD;
    match level {
        HeadingLevel::H1 => modifier |= Modifier::UNDERLINED,
        HeadingLevel::H2 => {}
        HeadingLevel::H3 | HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => {
            modifier |= Modifier::ITALIC;
        }
    }
    Style::default().fg(ratatui_color(theme.heading)).add_modifier(modifier)
}

fn is_blank_line(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.is_empty())
}

/// Greedy word-aware wrapping. Words longer than the budget overflow rather
/// than being broken so tokens are never split mid-character.
fn wrap_spans(
    spans: Vec<Span<'static>>,
    first_width: usize,
    cont_width: usize,
) -> Vec<Vec<Span<'static>>> {
    if spans.is_empty() {
        return vec![Vec::new()];
    }

    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_width: usize = 0;

    for span in spans {
        let style = span.style;
        let content = span.content.into_owned();

        let mut buf = String::new();
        let mut buf_width: usize = 0;

        for piece in content.split_inclusive(' ') {
            let piece_width = UnicodeWidthStr::width(piece);
            let max = if lines.is_empty() { first_width } else { cont_width };

            if current_width + buf_width + piece_width > max
                && (current_width > 0 || !buf.is_empty())
            {
                if !buf.is_empty() {
                    let taken = std::mem::take(&mut buf);
                    current.push(Span::styled(taken, style));
                    buf_width = 0;
                }
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            buf.push_str(piece);
            buf_width += piece_width;
        }

        if !buf.is_empty() {
            current.push(Span::styled(buf, style));
            current_width += buf_width;
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(Vec::new());
    }

    lines
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
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::dark()
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn find_span<'a>(lines: &'a [Line<'static>], substr: &str) -> Option<&'a Span<'static>> {
        lines.iter().flat_map(|l| l.spans.iter()).find(|s| s.content.contains(substr))
    }

    #[test]
    fn empty_input_returns_empty() {
        let lines = render_markdown("", 80, &theme());
        assert!(lines.is_empty());
    }

    #[test]
    fn single_paragraph_wraps_at_width() {
        let lines = render_markdown("aaa bbb ccc ddd eee", 10, &theme());
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert!(lines.len() >= 2, "width 10 should wrap 19-char paragraph; got lines: {texts:?}");
        let combined = texts.join(" ");
        assert!(combined.contains("aaa"), "got {texts:?}");
        assert!(combined.contains("eee"), "got {texts:?}");
    }

    #[test]
    fn heading_h1_is_bold_underlined() {
        let lines = render_markdown("# Hello", 80, &theme());
        let span = find_span(&lines, "Hello").expect("Hello span present");
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        assert!(span.style.add_modifier.contains(Modifier::UNDERLINED));
        assert!(!span.style.add_modifier.contains(Modifier::ITALIC));
        assert_eq!(span.style.fg, Some(ratatui_color(theme().heading)));
    }

    #[test]
    fn heading_h2_is_bold_not_underlined() {
        let lines = render_markdown("## Hello", 80, &theme());
        let span = find_span(&lines, "Hello").expect("Hello span present");
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        assert!(!span.style.add_modifier.contains(Modifier::UNDERLINED));
        assert!(!span.style.add_modifier.contains(Modifier::ITALIC));
        assert_eq!(span.style.fg, Some(ratatui_color(theme().heading)));
    }

    #[test]
    fn code_block_renders_each_line_indented() {
        let input = "```\nfoo\nbar\n```";
        let lines = render_markdown(input, 80, &theme());
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert!(texts.iter().any(|t| t == "  foo"), "expected '  foo' line in {texts:?}");
        assert!(texts.iter().any(|t| t == "  bar"), "expected '  bar' line in {texts:?}");
        let foo_span = find_span(&lines, "foo").expect("foo span present");
        assert_eq!(foo_span.style.fg, Some(ratatui_color(theme().code_fg)));
        assert_eq!(foo_span.style.bg, Some(ratatui_color(theme().code_bg)));
    }

    #[test]
    fn inline_code_has_code_color() {
        let lines = render_markdown("`hello`", 80, &theme());
        let span = find_span(&lines, "hello").expect("inline code span present");
        assert_eq!(span.style.fg, Some(ratatui_color(theme().code_fg)));
        assert_eq!(span.style.bg, Some(ratatui_color(theme().code_bg)));
    }

    #[test]
    fn bold_text_has_bold_modifier() {
        let lines = render_markdown("**bold**", 80, &theme());
        let span = find_span(&lines, "bold").expect("bold span present");
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(span.style.fg, Some(ratatui_color(theme().bold_fg)));
    }

    #[test]
    fn italic_text_has_italic_modifier() {
        let lines = render_markdown("*italic*", 80, &theme());
        let span = find_span(&lines, "italic").expect("italic span present");
        assert!(span.style.add_modifier.contains(Modifier::ITALIC));
        assert!(!span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn unordered_list_uses_bullet() {
        let lines = render_markdown("- item one\n- item two", 80, &theme());
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert!(texts.iter().any(|t| t.contains("• ") && t.contains("item one")), "got {texts:?}");
        assert!(texts.iter().any(|t| t.contains("• ") && t.contains("item two")), "got {texts:?}");
    }

    #[test]
    fn ordered_list_numbers_items_starting_from_start_value() {
        let lines = render_markdown("3. first\n4. second", 80, &theme());
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert!(texts.iter().any(|t| t.contains("3. ") && t.contains("first")), "got {texts:?}");
        assert!(texts.iter().any(|t| t.contains("4. ") && t.contains("second")), "got {texts:?}");
    }

    #[test]
    fn blockquote_prefixes_lines_with_bar() {
        let lines = render_markdown("> quoted text", 80, &theme());
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert!(texts.iter().any(|t| t.starts_with("│ ") && t.contains("quoted")), "got {texts:?}");
        let quoted_line =
            lines.iter().find(|l| line_text(l).starts_with("│ ")).expect("blockquote line present");
        assert_eq!(
            quoted_line.spans[0].style.fg,
            Some(ratatui_color(theme().dim)),
            "blockquote bar prefix should use theme.dim",
        );
    }
}
