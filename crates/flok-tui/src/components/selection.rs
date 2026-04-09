//! Panel-scoped text selection.
//!
//! Tracks mouse-drag selection within a single panel, extracts the selected
//! text, and copies it to the system clipboard via `arboard`.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::components::app::{DisplayMessage, MessageRole};
use crate::components::sidebar::{TeamMemberInfo, TeamMemberStatus};

/// Which panel a selection belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Messages,
    Sidebar,
    Input,
}

/// Selection granularity (single-click, double-click, triple-click).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    /// Normal character-level drag selection.
    #[default]
    Char,
    /// Double-click: expand to word boundaries.
    Word,
    /// Triple-click: expand to full line.
    Line,
}

/// Rectangular region on screen (in absolute terminal coordinates).
#[derive(Debug, Clone, Copy, Default)]
pub struct PanelRect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl PanelRect {
    pub fn contains(self, col: u16, row: u16) -> bool {
        col >= self.x && col < self.x + self.w && row >= self.y && row < self.y + self.h
    }
}

/// Rects of each panel, computed from layout dimensions.
#[derive(Debug, Clone, Copy, Default)]
pub struct PanelRects {
    pub messages: PanelRect,
    pub sidebar: Option<PanelRect>,
    pub input: PanelRect,
}

impl PanelRects {
    /// Identify which panel a terminal coordinate falls in.
    pub fn identify(&self, col: u16, row: u16) -> Option<Panel> {
        if let Some(sb) = self.sidebar {
            if sb.contains(col, row) {
                return Some(Panel::Sidebar);
            }
        }
        if self.input.contains(col, row) {
            return Some(Panel::Input);
        }
        if self.messages.contains(col, row) {
            return Some(Panel::Messages);
        }
        None
    }

    /// Clamp a coordinate to a specific panel's bounds.
    pub fn clamp_to(&self, panel: Panel, col: u16, row: u16) -> (u16, u16) {
        let rect = match panel {
            Panel::Messages => &self.messages,
            Panel::Sidebar => {
                if let Some(ref sb) = self.sidebar {
                    sb
                } else {
                    return (col, row);
                }
            }
            Panel::Input => &self.input,
        };
        let c = col.clamp(rect.x, rect.x + rect.w.saturating_sub(1));
        let r = row.clamp(rect.y, rect.y + rect.h.saturating_sub(1));
        (c, r)
    }
}

/// Visible text lines for a panel in the current frame.
#[derive(Debug, Clone, Default)]
pub struct VisiblePaneLines {
    pub rect: PanelRect,
    pub lines: Vec<String>,
}

impl VisiblePaneLines {
    pub fn new(rect: PanelRect, mut lines: Vec<String>) -> Self {
        lines.truncate(rect.h as usize);
        while lines.len() < rect.h as usize {
            lines.push(String::new());
        }
        Self { rect, lines }
    }

    fn resolve_selection(&self, selection: &SelectionState) -> ResolvedSelection {
        let ((sc, sr), (ec, er)) = selection.normalized();
        let max_col = self.rect.w.saturating_sub(1);
        let max_row = self.rect.h.saturating_sub(1);

        let mut start_row = sr.saturating_sub(self.rect.y).min(max_row);
        let mut end_row = er.saturating_sub(self.rect.y).min(max_row);
        let mut start_col = sc.saturating_sub(self.rect.x).min(max_col) as usize;
        let mut end_col = ec.saturating_sub(self.rect.x).min(max_col) as usize;

        if selection.mode == SelectionMode::Line {
            start_col = 0;
            end_col = max_col as usize;
        } else if selection.mode == SelectionMode::Word && start_row == end_row {
            let line = self.lines.get(start_row as usize).map_or("", String::as_str);
            (start_col, end_col) = expand_word_by_width(line, start_col, end_col);
        }

        if start_row > end_row {
            std::mem::swap(&mut start_row, &mut end_row);
        }

        ResolvedSelection {
            panel_rect: self.rect,
            anchor: (self.rect.x + start_col as u16, self.rect.y + start_row),
            cursor: (self.rect.x + end_col as u16, self.rect.y + end_row),
        }
    }
}

/// Visible text buffers for the selectable panels.
#[derive(Debug, Clone, Default)]
pub struct VisiblePanelBuffers {
    pub messages: VisiblePaneLines,
    pub sidebar: Option<VisiblePaneLines>,
    pub input: VisiblePaneLines,
}

impl VisiblePanelBuffers {
    fn pane(&self, panel: Panel) -> Option<&VisiblePaneLines> {
        match panel {
            Panel::Messages => Some(&self.messages),
            Panel::Sidebar => self.sidebar.as_ref(),
            Panel::Input => Some(&self.input),
        }
    }

    pub fn resolve_selection(&self, selection: &SelectionState) -> Option<ResolvedSelection> {
        selection
            .has_extent()
            .then(|| self.pane(selection.panel))
            .flatten()
            .map(|pane| pane.resolve_selection(selection))
    }
}

/// Selection after expanding against a pane's visible text.
#[derive(Debug, Clone)]
pub struct ResolvedSelection {
    pub panel_rect: PanelRect,
    pub anchor: (u16, u16),
    pub cursor: (u16, u16),
}

impl ResolvedSelection {
    pub fn normalized(&self) -> ((u16, u16), (u16, u16)) {
        let (a, b) = (self.anchor, self.cursor);
        if a.1 < b.1 || (a.1 == b.1 && a.0 <= b.0) {
            (a, b)
        } else {
            (b, a)
        }
    }
}

/// Tracks an in-progress or completed text selection.
#[derive(Debug, Clone)]
pub struct SelectionState {
    /// Panel where the selection started.
    pub panel: Panel,
    /// Anchor point (mouse-down position, absolute terminal coords).
    pub anchor: (u16, u16),
    /// Current drag point (absolute terminal coords).
    pub cursor: (u16, u16),
    /// Selection granularity.
    pub mode: SelectionMode,
}

impl SelectionState {
    /// Start a new selection at the given screen position.
    pub fn start(panel: Panel, col: u16, row: u16) -> Self {
        Self { panel, anchor: (col, row), cursor: (col, row), mode: SelectionMode::Char }
    }

    /// Extend the selection to a new position.
    pub fn extend(&mut self, col: u16, row: u16) {
        self.cursor = (col, row);
    }

    /// Whether the selection has nonzero extent.
    /// Word and Line modes always have extent (expanded by the overlay).
    pub fn has_extent(&self) -> bool {
        self.mode != SelectionMode::Char || self.anchor != self.cursor
    }

    /// Return `(start, end)` in normalized order (top-left to bottom-right).
    pub fn normalized(&self) -> ((u16, u16), (u16, u16)) {
        let (a, b) = (self.anchor, self.cursor);
        if a.1 < b.1 || (a.1 == b.1 && a.0 <= b.0) {
            (a, b)
        } else {
            (b, a)
        }
    }

    /// Check if a screen cell (col, row) is within the selection range.
    #[allow(dead_code)]
    pub fn contains(&self, col: u16, row: u16) -> bool {
        let ((sc, sr), (ec, er)) = self.normalized();
        if row < sr || row > er {
            return false;
        }
        if sr == er {
            return col >= sc && col <= ec;
        }
        if row == sr {
            return col >= sc;
        }
        if row == er {
            return col <= ec;
        }
        true
    }

    /// Extract the selected text from a line buffer.
    ///
    /// `lines` contains one string per visual row in the panel.
    /// `panel_rect` is the panel's screen rectangle.
    /// `scroll_offset` is the scroll offset in rows (0 for unscrolled).
    /// Coordinates in `self` are absolute terminal coords.
    #[cfg(test)]
    pub fn extract_text(&self, lines: &[String], panel_rect: PanelRect) -> String {
        if !self.has_extent() {
            return String::new();
        }

        let ((sc, sr), (ec, er)) = self.normalized();

        let first_row = sr.saturating_sub(panel_rect.y);
        let last_row = er.saturating_sub(panel_rect.y);

        // Column offsets relative to the panel's left edge.
        let start_col = sc.saturating_sub(panel_rect.x) as usize;
        let end_col = ec.saturating_sub(panel_rect.x) as usize;

        tracing::debug!(
            first_row,
            last_row,
            start_col,
            end_col,
            total_lines = lines.len(),
            "extract_text row/col mapping"
        );

        let mut result = String::new();
        for row_idx in first_row..=last_row {
            let line = if let Some(l) = lines.get(row_idx as usize) {
                l.as_str()
            } else {
                tracing::debug!(row_idx, "extract_text: row out of bounds");
                ""
            };

            let (col_start, col_end) = if first_row == last_row {
                // Single-row selection
                (start_col, end_col)
            } else if row_idx == first_row {
                (start_col, line.width())
            } else if row_idx == last_row {
                (0, end_col)
            } else {
                // Middle rows: full line
                (0, line.width())
            };

            // Extract the substring by character widths.
            let extracted = substr_by_width(line, col_start, col_end);
            tracing::debug!(
                row_idx,
                line_len = line.len(),
                line_preview = &line[..line.len().min(40)],
                col_start,
                col_end,
                extracted_len = extracted.len(),
                "extract_text: row"
            );

            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(&extracted);
        }
        result
    }
}

/// Extract a substring from `s` between display column `start` (inclusive)
/// and `end` (inclusive), using character display widths.
pub(crate) fn substr_by_width(s: &str, start: usize, end: usize) -> String {
    let mut result = String::new();
    let mut col = 0;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if col + w > end + 1 {
            break;
        }
        if col >= start {
            result.push(ch);
        }
        col += w;
    }
    result
}

/// Copy text to the system clipboard.
///
/// Tries `arboard` first (native clipboard integration). Falls back to
/// OSC 52 escape sequence which works in most modern terminals even
/// without a windowing system.
pub fn copy_to_clipboard(text: &str) -> bool {
    // Try native clipboard first.
    match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)) {
        Ok(()) => return true,
        Err(e) => {
            tracing::debug!("arboard clipboard failed, trying OSC 52: {e}");
        }
    }

    // Fallback: OSC 52 escape sequence.
    // Format: ESC ] 52 ; c ; <base64-encoded-text> ESC \
    let encoded = base64_encode(text.as_bytes());
    let seq = format!("\x1b]52;c;{encoded}\x1b\\");
    std::io::Write::write_all(&mut std::io::stdout(), seq.as_bytes()).is_ok()
        && std::io::Write::flush(&mut std::io::stdout()).is_ok()
}

/// Minimal base64 encoder (no external dependency).
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 { u32::from(chunk[1]) } else { 0 };
        let b2 = if chunk.len() > 2 { u32::from(chunk[2]) } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Compute panel rects from layout dimensions.
///
/// The layout is:
/// ```text
/// +------------------------+----------+
/// | Messages (flex)        | Sidebar  | <- row 0..footer_y
/// |                        | (39 cols)|
/// +------------------------+          |
/// | Input (pinned bottom)  |          |
/// +------------------------+----------+
/// | Footer (1 row)                    |
/// +-----------------------------------+
/// ```
pub fn compute_panel_rects(
    term_width: u16,
    term_height: u16,
    show_sidebar: bool,
    input_height: u16,
) -> PanelRects {
    let footer_h: u16 = 1;
    let sidebar_w: u16 = if show_sidebar { 39 } else { 0 };
    let main_w = term_width.saturating_sub(sidebar_w);
    let content_h = term_height.saturating_sub(footer_h);
    let input_y = content_h.saturating_sub(input_height);
    let messages_h = input_y;

    PanelRects {
        messages: PanelRect { x: 0, y: 0, w: main_w, h: messages_h },
        sidebar: if show_sidebar {
            Some(PanelRect { x: main_w, y: 0, w: sidebar_w, h: content_h })
        } else {
            None
        },
        input: PanelRect { x: 0, y: input_y, w: main_w, h: input_height },
    }
}

pub fn compute_input_height(text: &str, paste_indicator: Option<&str>, panel_width: u16) -> u16 {
    build_input_lines(text, paste_indicator, panel_width).len() as u16
}

#[allow(clippy::too_many_arguments)]
pub fn build_visible_panel_buffers(
    rects: PanelRects,
    messages: &[DisplayMessage],
    streaming: &str,
    reasoning: &str,
    is_waiting: bool,
    message_scroll: u16,
    input_text: &str,
    paste_indicator: Option<&str>,
    title: &str,
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cost: f64,
    context_pct: f64,
    team_name: &str,
    team_members: &[TeamMemberInfo],
    sidebar_scroll: u16,
) -> VisiblePanelBuffers {
    let messages_lines = visible_window(
        &build_message_lines(messages, streaming, reasoning, is_waiting, rects.messages.w),
        message_scroll,
        rects.messages.h,
    );
    let input_lines = build_input_lines(input_text, paste_indicator, rects.input.w);

    VisiblePanelBuffers {
        messages: VisiblePaneLines::new(rects.messages, messages_lines),
        sidebar: rects.sidebar.map(|sidebar_rect| {
            let version = format!("flok v{}", env!("CARGO_PKG_VERSION"));
            build_sidebar_visible_lines(
                sidebar_rect,
                title,
                model,
                input_tokens,
                output_tokens,
                cost,
                context_pct,
                team_name,
                team_members,
                &version,
                sidebar_scroll,
            )
        }),
        input: VisiblePaneLines::new(rects.input, input_lines),
    }
}

// ── Line buffer construction ────────────────────────────────────────────

/// Build a flat list of visual lines from the message list.
pub fn build_message_lines(
    messages: &[DisplayMessage],
    streaming: &str,
    reasoning: &str,
    is_waiting: bool,
    panel_width: u16,
) -> Vec<String> {
    let content_w = panel_width as usize;
    let mut lines: Vec<String> = Vec::new();

    for msg in messages {
        match msg.role {
            MessageRole::User => {
                lines.push(String::new());
                lines.push("  \u{2503}".to_string());
                lines.push(String::new());
                let prefix = "  \u{2503}  ";
                let wrap_w = content_w.saturating_sub(prefix.width() + 1);
                for wl in wrap_text(&msg.content, wrap_w) {
                    lines.push(format!("{prefix}{wl}"));
                }
                lines.push("  \u{2503}".to_string());
            }
            MessageRole::Assistant => {
                lines.push(String::new());
                let prefix = "    ";
                let wrap_w = content_w.saturating_sub(prefix.width());
                let plain = strip_markdown(&msg.content);
                for wl in wrap_text(&plain, wrap_w) {
                    lines.push(format!("{prefix}{wl}"));
                }
            }
            MessageRole::System => {
                lines.push(String::new());
                lines.push("  \u{2503}".to_string());
                lines.push("  \u{2503}  \u{26A0} System".to_string());
                lines.push(String::new());
                let prefix = "  \u{2503}  ";
                let wrap_w = content_w.saturating_sub(prefix.width());
                for wl in wrap_text(&msg.content, wrap_w) {
                    lines.push(format!("{prefix}{wl}"));
                }
                lines.push("  \u{2503}".to_string());
            }
            MessageRole::ToolCall => {
                let first_prefix = "    \u{2502} ";
                let cont_prefix = "      ";
                let wrap_w = content_w.saturating_sub(first_prefix.width());
                for (idx, wl) in wrap_text(&msg.content, wrap_w).into_iter().enumerate() {
                    let prefix = if idx == 0 { first_prefix } else { cont_prefix };
                    lines.push(format!("{prefix}{wl}"));
                }
            }
        }
    }

    // Reasoning
    if !reasoning.is_empty() && streaming.is_empty() {
        lines.push(String::new());
        lines.push("    \u{2502} \u{1F4AD} Thinking...".to_string());
        lines.push(String::new());
        let prefix = "      ";
        let wrap_w = content_w.saturating_sub(prefix.width());
        for wl in wrap_text(reasoning, wrap_w) {
            lines.push(format!("{prefix}{wl}"));
        }
    }

    // Streaming
    if !streaming.is_empty() {
        lines.push(String::new());
        let prefix = "    ";
        let wrap_w = content_w.saturating_sub(prefix.width());
        let plain = strip_markdown(streaming);
        for wl in wrap_text(&plain, wrap_w) {
            lines.push(format!("{prefix}{wl}"));
        }
    } else if is_waiting {
        lines.push(String::new());
        lines.push("    Thinking...".to_string());
    }

    lines
}

/// Build visual lines for the sidebar.
#[allow(clippy::too_many_arguments)]
pub fn build_sidebar_visible_lines(
    rect: PanelRect,
    title: &str,
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cost: f64,
    context_pct: f64,
    team_name: &str,
    team_members: &[TeamMemberInfo],
    version: &str,
    scroll_offset: u16,
) -> VisiblePaneLines {
    let mut body_lines = vec![
        String::new(),
        format!("   {title}"),
        format!("   {model}"),
        String::new(),
        "   CONTEXT".to_string(),
        format!("   In  {}", format_tokens(input_tokens)),
        format!("   Out {}", format_tokens(output_tokens)),
    ];

    if cost > 0.0 {
        if cost < 0.01 {
            body_lines.push(format!("   Cost ${cost:.4}"));
        } else {
            body_lines.push(format!("   Cost ${cost:.2}"));
        }
    }
    if context_pct > 0.0 {
        body_lines.push(format!("   Ctx  {:.0}%", context_pct.min(100.0)));
    }

    if !team_name.is_empty() {
        body_lines.push(String::new());
        body_lines.push("   AGENTS".to_string());
        let display_team = if team_name.len() > 33 {
            format!("{}...", &team_name[..30])
        } else {
            team_name.to_string()
        };
        body_lines.push(format!("   {display_team}"));
        for member in team_members {
            let icon = match member.status {
                TeamMemberStatus::Running => "\u{25CB}",
                TeamMemberStatus::Completed | TeamMemberStatus::Failed => "\u{25CF}",
            };
            body_lines.push(format!("     {icon} {}", member.display_name()));
        }
    }

    let body_height = rect.h.saturating_sub(1);
    let mut lines = visible_window(&body_lines, scroll_offset, body_height);
    lines.push(format!("   {version}"));
    VisiblePaneLines::new(rect, lines)
}

pub fn build_input_lines(
    text: &str,
    paste_indicator: Option<&str>,
    panel_width: u16,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(indicator) = paste_indicator {
        lines.push(format!("   {indicator}"));
    }

    let inner_prefix = "  \u{2502} ";
    let wrap_w = panel_width as usize;
    let content_w = wrap_w.saturating_sub(inner_prefix.width() + 2);
    let mut wrapped = wrap_text(text, content_w.max(1));
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }

    let max_inner_rows = 6usize;
    if wrapped.len() > max_inner_rows {
        let start = wrapped.len().saturating_sub(max_inner_rows);
        wrapped = wrapped[start..].to_vec();
    }

    lines.push("  \u{250C}".to_string());
    for line in wrapped {
        lines.push(format!("{inner_prefix}{line}"));
    }
    lines.push("  \u{2514}".to_string());

    while lines.len() < paste_indicator.map_or(3, |_| 4) {
        let insert_at = lines.len().saturating_sub(1);
        lines.insert(insert_at, inner_prefix.to_string());
    }

    lines.truncate(paste_indicator.map_or(8, |_| 9));
    lines
}

/// Wrap text into lines that fit within `max_width` display columns.
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }
    let mut result = Vec::new();
    for logical_line in text.split('\n') {
        if logical_line.is_empty() {
            result.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut col = 0;
        for word in logical_line.split_inclusive(' ') {
            let w = word.width();
            if col + w > max_width && col > 0 {
                result.push(current);
                current = String::new();
                col = 0;
            }
            current.push_str(word);
            col += w;
        }
        if !current.is_empty() {
            result.push(current);
        }
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

/// Minimal markdown stripping for plain-text extraction.
fn strip_markdown(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        let trimmed = line.trim_start();
        // Strip heading markers
        if let Some(rest) = trimmed.strip_prefix("### ") {
            out.push_str(rest);
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            out.push_str(rest);
        } else if let Some(rest) = trimmed.strip_prefix("# ") {
            out.push_str(rest);
        } else {
            // Strip inline formatting: bold, italic, code
            let cleaned =
                line.replace("**", "").replace("__", "").replace('*', "").replace('_', " ");
            out.push_str(&cleaned);
        }
        out.push('\n');
    }
    // Remove trailing newline
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn visible_window(lines: &[String], scroll_offset: u16, height: u16) -> Vec<String> {
    let start = scroll_offset as usize;
    let end = (start + height as usize).min(lines.len());
    let mut visible = lines[start..end].to_vec();
    while visible.len() < height as usize {
        visible.push(String::new());
    }
    visible
}

fn expand_word_by_width(s: &str, start_col: usize, end_col: usize) -> (usize, usize) {
    #[derive(Clone, Copy)]
    struct Span {
        start: usize,
        end: usize,
        whitespace: bool,
    }

    let mut spans = Vec::new();
    let mut col = 0;
    for ch in s.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        spans.push(Span { start: col, end: col + width - 1, whitespace: ch.is_whitespace() });
        col += width;
    }

    let Some(mut left) =
        spans.iter().position(|span| span.start <= end_col && span.end >= start_col)
    else {
        return (start_col, end_col);
    };
    let mut right = left;
    let whitespace = spans[left].whitespace;

    while left > 0 && spans[left - 1].whitespace == whitespace {
        left -= 1;
    }
    while right + 1 < spans.len() && spans[right + 1].whitespace == whitespace {
        right += 1;
    }

    (spans[left].start, spans[right].end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_rect_contains() {
        let r = PanelRect { x: 10, y: 5, w: 20, h: 10 };
        assert!(r.contains(10, 5));
        assert!(r.contains(29, 14));
        assert!(!r.contains(30, 5));
        assert!(!r.contains(10, 15));
        assert!(!r.contains(9, 5));
    }

    #[test]
    fn panel_rects_identify() {
        let rects = PanelRects {
            messages: PanelRect { x: 0, y: 0, w: 80, h: 20 },
            sidebar: Some(PanelRect { x: 80, y: 0, w: 39, h: 24 }),
            input: PanelRect { x: 0, y: 20, w: 80, h: 4 },
        };
        assert_eq!(rects.identify(5, 5), Some(Panel::Messages));
        assert_eq!(rects.identify(90, 10), Some(Panel::Sidebar));
        assert_eq!(rects.identify(5, 22), Some(Panel::Input));
        assert_eq!(rects.identify(200, 200), None);
    }

    #[test]
    fn selection_normalized_order() {
        // anchor before cursor (normal drag down)
        let sel = SelectionState {
            panel: Panel::Messages,
            anchor: (5, 2),
            cursor: (10, 5),
            mode: SelectionMode::Char,
        };
        assert_eq!(sel.normalized(), ((5, 2), (10, 5)));

        // anchor after cursor (drag up)
        let sel = SelectionState {
            panel: Panel::Messages,
            anchor: (10, 5),
            cursor: (5, 2),
            mode: SelectionMode::Char,
        };
        assert_eq!(sel.normalized(), ((5, 2), (10, 5)));
    }

    #[test]
    fn selection_contains_single_line() {
        let sel = SelectionState {
            panel: Panel::Messages,
            anchor: (5, 3),
            cursor: (10, 3),
            mode: SelectionMode::Char,
        };
        assert!(sel.contains(5, 3));
        assert!(sel.contains(7, 3));
        assert!(sel.contains(10, 3));
        assert!(!sel.contains(4, 3));
        assert!(!sel.contains(11, 3));
        assert!(!sel.contains(7, 2));
    }

    #[test]
    fn selection_contains_multi_line() {
        let sel = SelectionState {
            panel: Panel::Messages,
            anchor: (5, 2),
            cursor: (10, 4),
            mode: SelectionMode::Char,
        };
        // First row: col >= 5
        assert!(sel.contains(5, 2));
        assert!(sel.contains(100, 2));
        assert!(!sel.contains(4, 2));
        // Middle row: full
        assert!(sel.contains(0, 3));
        assert!(sel.contains(100, 3));
        // Last row: col <= 10
        assert!(sel.contains(0, 4));
        assert!(sel.contains(10, 4));
        assert!(!sel.contains(11, 4));
    }

    #[test]
    fn extract_text_single_line() {
        let lines = vec!["Hello, world!".to_string()];
        let rect = PanelRect { x: 0, y: 0, w: 80, h: 10 };
        let sel = SelectionState {
            panel: Panel::Messages,
            anchor: (0, 0),
            cursor: (4, 0),
            mode: SelectionMode::Char,
        };
        assert_eq!(sel.extract_text(&lines, rect), "Hello");
    }

    #[test]
    fn extract_text_multi_line() {
        let lines = vec!["Line one".to_string(), "Line two".to_string(), "Line three".to_string()];
        let rect = PanelRect { x: 0, y: 0, w: 80, h: 10 };
        let sel = SelectionState {
            panel: Panel::Messages,
            anchor: (5, 0),
            cursor: (3, 2),
            mode: SelectionMode::Char,
        };
        let text = sel.extract_text(&lines, rect);
        assert_eq!(text, "one\nLine two\nLine");
    }

    #[test]
    fn extract_text_with_visible_window() {
        let lines = vec!["Row 2 visible".to_string(), "Row 3 visible".to_string()];
        let rect = PanelRect { x: 0, y: 0, w: 80, h: 2 };
        let sel = SelectionState {
            panel: Panel::Messages,
            anchor: (0, 0),
            cursor: (4, 0),
            mode: SelectionMode::Char,
        };
        assert_eq!(sel.extract_text(&lines, rect), "Row 2");
    }

    #[test]
    fn extract_text_with_panel_offset() {
        let lines = vec!["Hello world".to_string()];
        // Panel starts at column 10
        let rect = PanelRect { x: 10, y: 5, w: 80, h: 10 };
        // Screen col 12 = panel col 2, screen col 16 = panel col 6
        let sel = SelectionState {
            panel: Panel::Messages,
            anchor: (12, 5),
            cursor: (16, 5),
            mode: SelectionMode::Char,
        };
        assert_eq!(sel.extract_text(&lines, rect), "llo w");
    }

    #[test]
    fn wrap_text_basic() {
        let lines = wrap_text("hello world foo bar", 12);
        assert_eq!(lines, vec!["hello world ", "foo bar"]);
    }

    #[test]
    fn wrap_text_newlines() {
        let lines = wrap_text("a\nb\nc", 80);
        assert_eq!(lines, vec!["a", "b", "c"]);
    }

    #[test]
    fn strip_markdown_headings_and_bold() {
        assert_eq!(strip_markdown("# Title"), "Title");
        assert_eq!(strip_markdown("## Sub"), "Sub");
        assert_eq!(strip_markdown("**bold**"), "bold");
    }

    #[test]
    fn build_message_lines_basic() {
        let msgs = vec![
            DisplayMessage { role: MessageRole::User, content: "hi".into() },
            DisplayMessage { role: MessageRole::Assistant, content: "hello".into() },
        ];
        let lines = build_message_lines(&msgs, "", "", false, 80);
        assert!(lines.iter().any(|l| l.contains("hi")));
        assert!(lines.iter().any(|l| l.contains("hello")));
    }

    #[test]
    fn resolve_word_selection_uses_visible_panel_lines() {
        let pane = VisiblePaneLines::new(
            PanelRect { x: 0, y: 0, w: 20, h: 1 },
            vec!["hello sidebar".to_string()],
        );
        let selection = SelectionState {
            panel: Panel::Messages,
            anchor: (1, 0),
            cursor: (1, 0),
            mode: SelectionMode::Word,
        };

        let resolved = pane.resolve_selection(&selection);
        assert_eq!(resolved.normalized(), ((0, 0), (4, 0)));
    }

    #[test]
    fn sidebar_visible_lines_keep_version_pinned() {
        let rect = PanelRect { x: 0, y: 0, w: 39, h: 4 };
        let visible = build_sidebar_visible_lines(
            rect,
            "Session",
            "model",
            10,
            20,
            0.0,
            0.0,
            "team-name",
            &[TeamMemberInfo { name: "worker-ABC12345".into(), status: TeamMemberStatus::Running }],
            "flok v0.0.1",
            3,
        );

        assert_eq!(visible.lines.len(), 4);
        assert_eq!(visible.lines.last().map(String::as_str), Some("   flok v0.0.1"));
    }

    #[test]
    fn format_tokens_formatting() {
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(2_500_000), "2.5M");
    }

    #[test]
    fn substr_by_width_ascii() {
        assert_eq!(substr_by_width("Hello, world!", 0, 4), "Hello");
        assert_eq!(substr_by_width("Hello, world!", 7, 11), "world");
    }
}
