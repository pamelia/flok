//! Panel-scoped text selection.
//!
//! Tracks mouse-drag selection within a single panel, extracts the selected
//! text, and copies it to the system clipboard via `arboard`.

#[cfg(test)]
use unicode_width::UnicodeWidthStr;

#[cfg(test)]
use crate::components::app::{DisplayMessage, MessageRole};

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
    pub fn extract_text(
        &self,
        lines: &[String],
        panel_rect: PanelRect,
        scroll_offset: u16,
    ) -> String {
        if !self.has_extent() {
            return String::new();
        }

        let ((sc, sr), (ec, er)) = self.normalized();

        // Convert absolute screen coords to panel-relative row indices,
        // accounting for scroll offset.
        let first_row = sr.saturating_sub(panel_rect.y) + scroll_offset;
        let last_row = er.saturating_sub(panel_rect.y) + scroll_offset;

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
#[cfg(test)]
fn substr_by_width(s: &str, start: usize, end: usize) -> String {
    let mut result = String::new();
    let mut col = 0;
    for ch in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
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

// ── Line buffer construction (used by tests) ────────────────────────────

/// Build a flat list of visual lines from the message list.
#[cfg(test)]
pub fn build_message_lines(
    messages: &[DisplayMessage],
    streaming: &str,
    reasoning: &str,
    is_waiting: bool,
    panel_width: u16,
) -> Vec<String> {
    let pw = panel_width as usize;
    // Left padding in the messages panel is 2.
    let content_w = pw.saturating_sub(3); // 2 left pad + 1 right pad
    let mut lines: Vec<String> = Vec::new();

    for msg in messages {
        // Each message has padding_top: 1 (blank line before header)
        lines.push(String::new());

        match msg.role {
            MessageRole::User => {
                // "  > You" (2 padding_left + header)
                lines.push("  \u{25B6} You".to_string());
                // blank line (padding_top: 1 on content)
                lines.push(String::new());
                // Content with 4 indent (2 msg padding + 2 content padding)
                let indent = 4;
                let wrap_w = content_w.saturating_sub(indent);
                for wl in wrap_text(&msg.content, wrap_w) {
                    lines.push(format!("{:indent$}{wl}", "", indent = indent));
                }
            }
            MessageRole::Assistant => {
                lines.push("  \u{25B6} Assistant".to_string());
                lines.push(String::new());
                let indent = 4;
                let wrap_w = content_w.saturating_sub(indent);
                // Strip markdown formatting for plain text extraction.
                let plain = strip_markdown(&msg.content);
                for wl in wrap_text(&plain, wrap_w) {
                    lines.push(format!("{:indent$}{wl}", "", indent = indent));
                }
            }
            MessageRole::System => {
                lines.push("  \u{26A0} System".to_string());
                let indent = 4;
                let wrap_w = content_w.saturating_sub(indent);
                for wl in wrap_text(&msg.content, wrap_w) {
                    lines.push(format!("{:indent$}{wl}", "", indent = indent));
                }
            }
            MessageRole::ToolCall => {
                let indent = 4;
                let wrap_w = content_w.saturating_sub(indent);
                for wl in wrap_text(&msg.content, wrap_w) {
                    lines.push(format!("{:indent$}{wl}", "", indent = indent));
                }
            }
        }
    }

    // Reasoning
    if !reasoning.is_empty() && streaming.is_empty() {
        lines.push(String::new());
        lines.push("    \u{1F4AD} Thinking...".to_string());
        lines.push(String::new());
        let indent = 6;
        let wrap_w = content_w.saturating_sub(indent);
        for wl in wrap_text(reasoning, wrap_w) {
            lines.push(format!("{:indent$}{wl}", "", indent = indent));
        }
    }

    // Streaming
    if !streaming.is_empty() {
        lines.push(String::new());
        lines.push("  \u{25B6} Assistant ...".to_string());
        lines.push(String::new());
        let indent = 4;
        let wrap_w = content_w.saturating_sub(indent);
        let plain = strip_markdown(streaming);
        for wl in wrap_text(&plain, wrap_w) {
            lines.push(format!("{:indent$}{wl}", "", indent = indent));
        }
    } else if is_waiting {
        lines.push(String::new());
        lines.push("    Thinking...".to_string());
    }

    lines
}

/// Build visual lines for the sidebar.
#[cfg(test)]
#[allow(dead_code, clippy::too_many_arguments)]
pub fn build_sidebar_lines(
    title: &str,
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cost: f64,
    context_pct: f64,
    team_name: &str,
    team_members: &[(String, &str)], // (name, status_word)
) -> Vec<String> {
    let mut lines = Vec::new();

    lines.push(title.to_string());
    lines.push(model.to_string());
    lines.push(String::new());
    lines.push("CONTEXT".to_string());
    lines.push(format!("In  {}", format_tokens(input_tokens)));
    lines.push(format!("Out {}", format_tokens(output_tokens)));
    if cost > 0.0 {
        if cost < 0.01 {
            lines.push(format!("Cost ${cost:.4}"));
        } else {
            lines.push(format!("Cost ${cost:.2}"));
        }
    }
    if context_pct > 0.0 {
        lines.push(format!("Ctx  {:.0}%", context_pct.min(100.0)));
    }

    if !team_name.is_empty() {
        lines.push(String::new());
        lines.push(format!("AGENTS ({} agents)", team_members.len()));
        lines.push(team_name.to_string());
        for (name, status) in team_members {
            lines.push(format!("  @{name} {status}"));
        }
    }

    lines
}

/// Wrap text into lines that fit within `max_width` display columns.
#[cfg(test)]
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
#[cfg(test)]
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

#[cfg(test)]
fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
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
        assert_eq!(sel.extract_text(&lines, rect, 0), "Hello");
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
        let text = sel.extract_text(&lines, rect, 0);
        assert_eq!(text, "one\nLine two\nLine");
    }

    #[test]
    fn extract_text_with_scroll_offset() {
        let lines = vec![
            "Row 0".to_string(),
            "Row 1".to_string(),
            "Row 2 visible".to_string(),
            "Row 3 visible".to_string(),
        ];
        let rect = PanelRect { x: 0, y: 0, w: 80, h: 2 };
        // Screen row 0 with scroll_offset=2 maps to lines[2]
        let sel = SelectionState {
            panel: Panel::Messages,
            anchor: (0, 0),
            cursor: (4, 0),
            mode: SelectionMode::Char,
        };
        assert_eq!(sel.extract_text(&lines, rect, 2), "Row 2");
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
        assert_eq!(sel.extract_text(&lines, rect, 0), "llo w");
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
        // Should contain user header and assistant header
        assert!(lines.iter().any(|l| l.contains("You")));
        assert!(lines.iter().any(|l| l.contains("Assistant")));
        assert!(lines.iter().any(|l| l.contains("hi")));
        assert!(lines.iter().any(|l| l.contains("hello")));
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
