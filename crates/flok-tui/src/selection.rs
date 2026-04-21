use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Modifier,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const CLICK_WINDOW: Duration = Duration::from_millis(650);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelKind {
    Chat,
    Sidebar,
    Composer,
}

impl PanelKind {
    pub(crate) fn identify((col, row): (u16, u16), rects: &LayoutRects) -> Option<Self> {
        if rect_contains(rects.chat, col, row) {
            Some(Self::Chat)
        } else if rects.sidebar.is_some_and(|rect| rect_contains(rect, col, row)) {
            Some(Self::Sidebar)
        } else if rect_contains(rects.composer, col, row) {
            Some(Self::Composer)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LayoutRects {
    pub(crate) chat: Rect,
    pub(crate) sidebar: Option<Rect>,
    pub(crate) composer: Rect,
}

impl Default for LayoutRects {
    fn default() -> Self {
        Self { chat: Rect::new(0, 0, 0, 0), sidebar: None, composer: Rect::new(0, 0, 0, 0) }
    }
}

impl LayoutRects {
    pub(crate) fn rect_for(&self, panel: PanelKind) -> Option<Rect> {
        match panel {
            PanelKind::Chat => Some(self.chat),
            PanelKind::Sidebar => self.sidebar,
            PanelKind::Composer => Some(self.composer),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelectionPoint {
    pub(crate) panel: PanelKind,
    pub(crate) row: u16,
    pub(crate) col: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SelectionMode {
    #[default]
    Char,
    Word,
    Line,
    Paragraph,
}

impl SelectionMode {
    pub(crate) fn from_click_count(count: u8) -> Self {
        match count {
            2 => Self::Word,
            3 => Self::Line,
            4 => Self::Paragraph,
            _ => Self::Char,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SelectionState {
    pub(crate) anchor: Option<SelectionPoint>,
    pub(crate) head: Option<SelectionPoint>,
    pub(crate) mode: SelectionMode,
    dragging: bool,
}

impl SelectionState {
    pub(crate) fn start(point: SelectionPoint) -> Self {
        Self { anchor: Some(point), head: None, mode: SelectionMode::Char, dragging: true }
    }

    pub(crate) fn with_mode(mut self, mode: SelectionMode) -> Self {
        self.mode = mode;
        self
    }

    pub(crate) fn extend(&mut self, point: SelectionPoint) {
        self.head = Some(point);
    }

    pub(crate) fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
        self.mode = SelectionMode::Char;
        self.dragging = false;
    }

    pub(crate) fn has_extent(&self) -> bool {
        match (self.anchor, self.head) {
            (Some(anchor), Some(head)) => anchor != head,
            _ => false,
        }
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub(crate) fn normalized(&self) -> (SelectionPoint, SelectionPoint) {
        let anchor = self.anchor.unwrap_or_else(default_point);
        let head = self.head.unwrap_or(anchor);
        if compare_points(anchor, head).is_le() {
            (anchor, head)
        } else {
            (head, anchor)
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ClickTracker {
    pub(crate) history: VecDeque<(Instant, u16, u16)>,
}

impl ClickTracker {
    pub(crate) fn register(&mut self, now: Instant, col: u16, row: u16) -> u8 {
        while self
            .history
            .front()
            .is_some_and(|(seen_at, _, _)| now.saturating_duration_since(*seen_at) > CLICK_WINDOW)
        {
            self.history.pop_front();
        }

        let origin = self.history.front().copied();
        let count = if let Some((_, origin_col, origin_row)) = origin {
            if origin_row == row {
                let next_count = self.history.len() + 1;
                let tolerance = match next_count {
                    2 => 4,
                    3 => 8,
                    _ => u16::MAX,
                };
                if origin_col.abs_diff(col) > tolerance {
                    1
                } else {
                    u8::try_from(next_count).unwrap_or(4)
                }
            } else {
                1
            }
        } else {
            1
        };

        if count >= 5 {
            self.history.clear();
            self.history.push_back((now, col, row));
            1
        } else {
            if count == 1 {
                self.history.clear();
            }
            self.history.push_back((now, col, row));
            count.max(1)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PanelBuffer {
    pub(crate) rect: Rect,
    pub(crate) rows: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Whitespace,
    Word,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisplayChar {
    start: usize,
    end: usize,
    byte_start: usize,
    byte_end: usize,
    class: CharClass,
}

pub(crate) fn expand_word_by_width(line: &str, start_col: usize, end_col: usize) -> (usize, usize) {
    let chars = display_chars(line);
    let Some(mut left) = overlapping_span_index(&chars, start_col, end_col) else {
        return (start_col, end_col);
    };
    let mut right = left;
    let class = chars[left].class;

    while left > 0 && chars[left - 1].class == class {
        left -= 1;
    }
    while right + 1 < chars.len() && chars[right + 1].class == class {
        right += 1;
    }

    (chars[left].start, chars[right].end)
}

pub(crate) fn extract_selection_text(buffers: &[PanelBuffer], sel: &SelectionState) -> String {
    if !sel.has_extent() {
        return String::new();
    }

    let Some(panel) = selection_buffer(buffers, sel) else {
        return String::new();
    };
    let (start, end) = selection_bounds(sel, panel);

    let start_row = usize::from(start.row.saturating_sub(panel.rect.y));
    let end_row = usize::from(end.row.saturating_sub(panel.rect.y));
    let mut rows = Vec::new();

    for row_index in start_row..=end_row {
        let Some(line) = panel.rows.get(row_index) else {
            continue;
        };

        let selected = if start_row == end_row {
            extract_slice_by_width(
                line,
                usize::from(start.col.saturating_sub(panel.rect.x)),
                usize::from(end.col.saturating_sub(panel.rect.x)),
            )
        } else if row_index == start_row {
            let start_col = usize::from(start.col.saturating_sub(panel.rect.x));
            let line_width = line.width();
            if line_width == 0 || start_col >= line_width {
                String::new()
            } else {
                extract_slice_by_width(line, start_col, line_width.saturating_sub(1))
            }
        } else if row_index == end_row {
            extract_slice_by_width(line, 0, usize::from(end.col.saturating_sub(panel.rect.x)))
        } else {
            line.clone()
        };

        rows.push(selected);
    }

    rows.join("\n")
}

pub(crate) fn paint_selection(
    buf: &mut Buffer,
    state: &SelectionState,
    panel_buffers: &[PanelBuffer],
) {
    if !state.has_extent() {
        return;
    }

    let Some(panel) = selection_buffer(panel_buffers, state) else {
        return;
    };
    let (start, end) = selection_bounds(state, panel);
    let panel_left = panel.rect.x;
    let panel_right = panel.rect.right().saturating_sub(1);

    for row in start.row..=end.row {
        let (mut col_start, mut col_end) = if start.row == end.row {
            (start.col, end.col)
        } else if row == start.row {
            (start.col, panel_right)
        } else if row == end.row {
            (panel_left, end.col)
        } else {
            (panel_left, panel_right)
        };

        col_start = col_start.clamp(panel_left, panel_right);
        col_end = col_end.clamp(panel_left, panel_right);
        if col_start > col_end {
            continue;
        }

        for col in col_start..=col_end {
            if let Some(cell) = buf.cell_mut(Position::new(col, row)) {
                cell.modifier |= Modifier::REVERSED;
            }
        }
    }
}

pub(crate) fn selection_buffer<'a>(
    buffers: &'a [PanelBuffer],
    state: &SelectionState,
) -> Option<&'a PanelBuffer> {
    let anchor = state.anchor?;
    buffers.iter().find(|buffer| rect_contains(buffer.rect, anchor.col, anchor.row))
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.right() && row >= rect.y && row < rect.bottom()
}

fn selection_bounds(
    state: &SelectionState,
    panel: &PanelBuffer,
) -> (SelectionPoint, SelectionPoint) {
    let (mut start, mut end) = state.normalized();
    start.col = start.col.clamp(panel.rect.x, panel.rect.right().saturating_sub(1));
    end.col = end.col.clamp(panel.rect.x, panel.rect.right().saturating_sub(1));
    start.row = start.row.clamp(panel.rect.y, panel.rect.bottom().saturating_sub(1));
    end.row = end.row.clamp(panel.rect.y, panel.rect.bottom().saturating_sub(1));
    (start, end)
}

fn default_point() -> SelectionPoint {
    SelectionPoint { panel: PanelKind::Chat, row: 0, col: 0 }
}

fn compare_points(left: SelectionPoint, right: SelectionPoint) -> std::cmp::Ordering {
    (left.row, left.col).cmp(&(right.row, right.col))
}

fn display_chars(line: &str) -> Vec<DisplayChar> {
    let mut spans = Vec::new();
    let mut col = 0;
    for (byte_start, ch) in line.char_indices() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        let byte_end = byte_start + ch.len_utf8();
        spans.push(DisplayChar {
            start: col,
            end: col + width - 1,
            byte_start,
            byte_end,
            class: classify_char(ch),
        });
        col += width;
    }
    spans
}

fn overlapping_span_index(
    chars: &[DisplayChar],
    start_col: usize,
    end_col: usize,
) -> Option<usize> {
    chars.iter().position(|display| display.start <= end_col && display.end >= start_col)
}

fn extract_slice_by_width(line: &str, start_col: usize, end_col: usize) -> String {
    if start_col > end_col {
        return String::new();
    }

    let mut out = String::new();
    for span in display_chars(line) {
        if span.start > end_col {
            break;
        }
        if span.end >= start_col {
            out.push_str(&line[span.byte_start..span.byte_end]);
        }
    }
    out
}

fn classify_char(ch: char) -> CharClass {
    if ch.is_whitespace() {
        CharClass::Whitespace
    } else if is_punctuation(ch) {
        CharClass::Other
    } else {
        CharClass::Word
    }
}

fn is_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation()
        || matches!(
            ch,
            '，' | '。'
                | '、'
                | '！'
                | '？'
                | '：'
                | '；'
                | '（'
                | '）'
                | '【'
                | '】'
                | '《'
                | '》'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '—'
                | '…'
                | '·'
                | '「'
                | '」'
                | '『'
                | '』'
                | '〈'
                | '〉'
                | '〔'
                | '〕'
                | '［'
                | '］'
                | '｛'
                | '｝'
                | '～'
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        buffer::{Buffer, Cell},
        style::Style,
    };

    fn point(panel: PanelKind, row: u16, col: u16) -> SelectionPoint {
        SelectionPoint { panel, row, col }
    }

    fn panel_buffer(rect: Rect, rows: &[&str]) -> PanelBuffer {
        PanelBuffer { rect, rows: rows.iter().map(|row| (*row).to_string()).collect() }
    }

    #[test]
    fn selection_mode_from_click_count() {
        assert_eq!(SelectionMode::from_click_count(1), SelectionMode::Char);
        assert_eq!(SelectionMode::from_click_count(2), SelectionMode::Word);
        assert_eq!(SelectionMode::from_click_count(3), SelectionMode::Line);
        assert_eq!(SelectionMode::from_click_count(4), SelectionMode::Paragraph);
        assert_eq!(SelectionMode::from_click_count(5), SelectionMode::Char);
    }

    #[test]
    fn click_tracker_expires_after_650ms() {
        let mut tracker = ClickTracker::default();
        let now = Instant::now();

        assert_eq!(tracker.register(now, 10, 4), 1);
        assert_eq!(tracker.register(now + Duration::from_millis(651), 10, 4), 1);
    }

    #[test]
    fn click_tracker_distance_tolerance_4_then_8_then_unlimited() {
        let mut tracker = ClickTracker::default();
        let now = Instant::now();

        assert_eq!(tracker.register(now, 10, 4), 1);
        assert_eq!(tracker.register(now + Duration::from_millis(100), 14, 4), 2);
        assert_eq!(tracker.register(now + Duration::from_millis(200), 18, 4), 3);
        assert_eq!(tracker.register(now + Duration::from_millis(300), 60, 4), 4);
        assert_eq!(tracker.register(now + Duration::from_millis(400), 60, 4), 1);
    }

    #[test]
    fn expand_word_by_width_unicode() {
        let line = "A你🙂B next";
        assert_eq!(expand_word_by_width(line, 1, 4), (0, 5));
    }

    #[test]
    fn expand_word_by_width_punctuation() {
        let line = "alpha, beta!";
        assert_eq!(expand_word_by_width(line, 1, 2), (0, 4));
        assert_eq!(expand_word_by_width(line, 5, 5), (5, 5));
        assert_eq!(expand_word_by_width(line, 8, 8), (7, 10));
    }

    #[test]
    fn selection_state_normalized_reverse_drag() {
        let mut state = SelectionState::start(point(PanelKind::Chat, 4, 10));
        state.extend(point(PanelKind::Chat, 2, 3));

        assert_eq!(
            state.normalized(),
            (point(PanelKind::Chat, 2, 3), point(PanelKind::Chat, 4, 10))
        );
    }

    #[test]
    fn selection_state_head_only_set_on_drag() {
        let state = SelectionState::start(point(PanelKind::Chat, 1, 2));

        assert!(!state.has_extent());
        assert!(state.is_dragging());
    }

    #[test]
    fn extract_text_single_row() {
        let buffers = vec![panel_buffer(Rect::new(0, 0, 20, 2), &["hello world", "second row"])];
        let mut state = SelectionState::start(point(PanelKind::Chat, 0, 1));
        state.extend(point(PanelKind::Chat, 0, 4));

        assert_eq!(extract_selection_text(&buffers, &state), "ello");
    }

    #[test]
    fn extract_text_multi_row() {
        let buffers = vec![panel_buffer(
            Rect::new(0, 0, 20, 3),
            &["hello world", "second row", "third line"],
        )];
        let mut state = SelectionState::start(point(PanelKind::Chat, 0, 6));
        state.extend(point(PanelKind::Chat, 2, 4));

        assert_eq!(extract_selection_text(&buffers, &state), "world\nsecond row\nthird");
    }

    #[test]
    fn paint_selection_applies_reversed_modifier() {
        let rect = Rect::new(0, 0, 8, 2);
        let mut buffer = Buffer::filled(rect, Cell::new(" "));
        buffer.set_stringn(0, 0, "abcdefgh", 8, Style::default());
        let panel_buffers = vec![panel_buffer(rect, &["abcdefgh", ""])];
        let mut state = SelectionState::start(point(PanelKind::Chat, 0, 2));
        state.extend(point(PanelKind::Chat, 0, 4));

        paint_selection(&mut buffer, &state, &panel_buffers);

        for x in 2..=4 {
            let cell = buffer.cell(Position::new(x, 0));
            assert!(cell.is_some_and(|cell| cell.modifier.contains(Modifier::REVERSED)));
        }
    }
}
