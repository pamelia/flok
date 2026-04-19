use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextArea {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
    kill_buffer: String,
}

impl TextArea {
    pub(crate) fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
            kill_buffer: String::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub(crate) fn text(&self) -> String {
        self.lines.join("\n")
    }

    #[cfg(test)]
    pub(crate) fn set_text(&mut self, s: &str) {
        self.lines = s.split('\n').map(str::to_owned).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }

        self.cursor_row = self.lines.len().saturating_sub(1);
        self.cursor_col = self.lines[self.cursor_row].len();
    }

    pub(crate) fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    pub(crate) fn insert_char(&mut self, c: char) {
        self.lines[self.cursor_row].insert(self.cursor_col, c);
        self.cursor_col += c.len_utf8();
    }

    pub(crate) fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }

        let row = self.cursor_row;
        let col = self.cursor_col;
        let tail = self.lines[row].split_off(col);
        let parts: Vec<&str> = s.split('\n').collect();

        self.lines[row].push_str(parts[0]);
        if parts.len() == 1 {
            self.lines[row].push_str(&tail);
            self.cursor_col = col + parts[0].len();
            return;
        }

        for (offset, part) in parts.iter().enumerate().skip(1) {
            self.lines.insert(row + offset, (*part).to_string());
        }

        let last_row = row + parts.len() - 1;
        self.lines[last_row].push_str(&tail);
        self.cursor_row = last_row;
        self.cursor_col = parts.last().map_or(0, |part| part.len());
    }

    pub(crate) fn insert_newline(&mut self) {
        let rest = self.lines[self.cursor_row].split_off(self.cursor_col);
        self.lines.insert(self.cursor_row + 1, rest);
        self.cursor_row += 1;
        self.cursor_col = 0;
    }

    pub(crate) fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let prev = prev_grapheme_boundary(&self.lines[self.cursor_row], self.cursor_col);
            self.lines[self.cursor_row].replace_range(prev..self.cursor_col, "");
            self.cursor_col = prev;
            return;
        }

        if self.cursor_row == 0 {
            return;
        }

        let current = self.lines.remove(self.cursor_row);
        self.cursor_row -= 1;
        self.cursor_col = self.lines[self.cursor_row].len();
        self.lines[self.cursor_row].push_str(&current);
    }

    pub(crate) fn delete(&mut self) {
        let line_len = self.lines[self.cursor_row].len();
        if self.cursor_col < line_len {
            let next = next_grapheme_boundary(&self.lines[self.cursor_row], self.cursor_col);
            self.lines[self.cursor_row].replace_range(self.cursor_col..next, "");
            return;
        }

        if self.cursor_row + 1 >= self.lines.len() {
            return;
        }

        let next_line = self.lines.remove(self.cursor_row + 1);
        self.lines[self.cursor_row].push_str(&next_line);
    }

    pub(crate) fn delete_prev_word(&mut self) {
        while self.cursor_col == 0 {
            if self.cursor_row == 0 {
                return;
            }

            let current = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
            self.lines[self.cursor_row].push_str(&current);
        }

        let start = prev_word_boundary(&self.lines[self.cursor_row], self.cursor_col);
        self.lines[self.cursor_row].replace_range(start..self.cursor_col, "");
        self.cursor_col = start;
    }

    pub(crate) fn kill_to_eol(&mut self) {
        let line_len = self.lines[self.cursor_row].len();
        if self.cursor_col < line_len {
            self.kill_buffer = self.lines[self.cursor_row][self.cursor_col..].to_string();
            self.lines[self.cursor_row].truncate(self.cursor_col);
            return;
        }

        if self.cursor_row + 1 < self.lines.len() {
            self.kill_buffer = "\n".to_string();
            let next_line = self.lines.remove(self.cursor_row + 1);
            self.lines[self.cursor_row].push_str(&next_line);
            return;
        }

        self.kill_buffer.clear();
    }

    pub(crate) fn yank(&mut self) {
        let kill_buffer = self.kill_buffer.clone();
        self.insert_str(&kill_buffer);
    }

    pub(crate) fn clear_input(&mut self) {
        self.clear();
    }

    pub(crate) fn move_cursor_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col = prev_grapheme_boundary(&self.lines[self.cursor_row], self.cursor_col);
            return;
        }

        if self.cursor_row == 0 {
            return;
        }

        self.cursor_row -= 1;
        self.cursor_col = self.lines[self.cursor_row].len();
    }

    pub(crate) fn move_cursor_right(&mut self) {
        let line_len = self.lines[self.cursor_row].len();
        if self.cursor_col < line_len {
            self.cursor_col = next_grapheme_boundary(&self.lines[self.cursor_row], self.cursor_col);
            return;
        }

        if self.cursor_row + 1 >= self.lines.len() {
            return;
        }

        self.cursor_row += 1;
        self.cursor_col = 0;
    }

    pub(crate) fn move_cursor_up(&mut self) {
        if self.cursor_row == 0 {
            return;
        }

        let visual_col = display_col(&self.lines[self.cursor_row], self.cursor_col);
        self.cursor_row -= 1;
        self.cursor_col = byte_offset_for_display_col(&self.lines[self.cursor_row], visual_col);
    }

    pub(crate) fn move_cursor_down(&mut self) {
        if self.cursor_row + 1 >= self.lines.len() {
            return;
        }

        let visual_col = display_col(&self.lines[self.cursor_row], self.cursor_col);
        self.cursor_row += 1;
        self.cursor_col = byte_offset_for_display_col(&self.lines[self.cursor_row], visual_col);
    }

    pub(crate) fn move_to_line_start(&mut self) {
        self.cursor_col = 0;
    }

    pub(crate) fn move_to_line_end(&mut self) {
        self.cursor_col = self.lines[self.cursor_row].len();
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let blank = " ".repeat(area.width as usize);
        for row in 0..area.height {
            buf.set_string(area.x, area.y + row, blank.as_str(), Style::default());
        }

        let row_offset = self.visible_row_offset(area.height);
        for (render_row, line) in
            self.lines.iter().skip(row_offset).take(area.height as usize).enumerate()
        {
            buf.set_stringn(
                area.x,
                area.y + render_row as u16,
                line,
                area.width as usize,
                Style::default(),
            );
        }

        let (cursor_x, cursor_y) = self.cursor_pos(area);
        if let Some(cell) = buf.cell_mut(Position::new(cursor_x, cursor_y)) {
            if cell.symbol().is_empty() {
                cell.set_symbol(" ");
            }
            cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
        }
    }

    pub(crate) fn height(&self, _width: u16) -> u16 {
        self.lines.len().min(8) as u16
    }

    pub(crate) fn cursor_pos(&self, area: Rect) -> (u16, u16) {
        let row_offset = self.visible_row_offset(area.height);
        let visual_col = display_col(&self.lines[self.cursor_row], self.cursor_col);
        let x = area.x.saturating_add(visual_col.min(area.width.saturating_sub(1) as usize) as u16);
        let y = area.y.saturating_add(
            self.cursor_row.saturating_sub(row_offset).min(area.height as usize) as u16,
        );
        (x, y)
    }

    fn visible_row_offset(&self, max_rows: u16) -> usize {
        if max_rows == 0 {
            return self.cursor_row;
        }

        self.cursor_row.saturating_add(1).saturating_sub(max_rows as usize)
    }
}

fn prev_grapheme_boundary(s: &str, cursor_col: usize) -> usize {
    let cursor_col = cursor_col.min(s.len());
    UnicodeSegmentation::grapheme_indices(s, true)
        .take_while(|(idx, _)| *idx < cursor_col)
        .map(|(idx, _)| idx)
        .last()
        .unwrap_or(0)
}

fn next_grapheme_boundary(s: &str, cursor_col: usize) -> usize {
    let cursor_col = cursor_col.min(s.len());
    s[cursor_col..].graphemes(true).next().map_or(s.len(), |grapheme| cursor_col + grapheme.len())
}

fn display_col(s: &str, cursor_col: usize) -> usize {
    let cursor_col = cursor_col.min(s.len());
    s[..cursor_col].graphemes(true).map(UnicodeWidthStr::width).sum()
}

fn byte_offset_for_display_col(s: &str, target_col: usize) -> usize {
    let mut visual_col = 0;
    for (idx, grapheme) in s.grapheme_indices(true) {
        let next_visual = visual_col + UnicodeWidthStr::width(grapheme);
        if next_visual > target_col {
            return idx;
        }
        if next_visual == target_col {
            return idx + grapheme.len();
        }
        visual_col = next_visual;
    }

    s.len()
}

fn prev_word_boundary(s: &str, cursor_col: usize) -> usize {
    let mut cursor_col = cursor_col.min(s.len());
    while cursor_col > 0 {
        let prev = prev_grapheme_boundary(s, cursor_col);
        if classify_grapheme(&s[prev..cursor_col]) != GraphemeClass::Whitespace {
            break;
        }
        cursor_col = prev;
    }

    if cursor_col == 0 {
        return 0;
    }

    let mut boundary = cursor_col;
    let first_prev = prev_grapheme_boundary(s, boundary);
    let target_class = classify_grapheme(&s[first_prev..boundary]);
    while boundary > 0 {
        let prev = prev_grapheme_boundary(s, boundary);
        if classify_grapheme(&s[prev..boundary]) != target_class {
            break;
        }
        boundary = prev;
    }

    boundary
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GraphemeClass {
    Whitespace,
    Word,
    Other,
}

fn classify_grapheme(grapheme: &str) -> GraphemeClass {
    if grapheme.chars().all(char::is_whitespace) {
        GraphemeClass::Whitespace
    } else if grapheme.chars().any(|ch| ch.is_alphanumeric() || ch == '_') {
        GraphemeClass::Word
    } else {
        GraphemeClass::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    #[test]
    fn insert_char_insert_str_and_newline_round_trip_text() {
        let mut textarea = TextArea::new();
        textarea.insert_char('h');
        textarea.insert_char('i');
        textarea.insert_newline();
        textarea.insert_str("there");

        assert_eq!(textarea.text(), "hi\nthere");
        assert_eq!(textarea.cursor_row, 1);
        assert_eq!(textarea.cursor_col, "there".len());
    }

    #[test]
    fn insert_str_handles_multiline_paste() {
        let mut textarea = TextArea::new();
        textarea.insert_str("alpha\nbeta\ngamma");

        assert_eq!(textarea.text(), "alpha\nbeta\ngamma");
        assert_eq!(textarea.cursor_row, 2);
        assert_eq!(textarea.cursor_col, "gamma".len());
    }

    #[test]
    fn backspace_at_start_of_line_joins_lines() {
        let mut textarea = TextArea::new();
        textarea.set_text("hello\nworld");
        textarea.move_to_line_start();

        textarea.backspace();

        assert_eq!(textarea.text(), "helloworld");
        assert_eq!(textarea.cursor_row, 0);
        assert_eq!(textarea.cursor_col, "hello".len());
    }

    #[test]
    fn delete_joins_with_next_line_at_end_of_line() {
        let mut textarea = TextArea::new();
        textarea.set_text("hello\nworld");
        textarea.cursor_row = 0;
        textarea.cursor_col = textarea.lines[0].len();

        textarea.delete();

        assert_eq!(textarea.text(), "helloworld");
        assert_eq!(textarea.cursor_row, 0);
        assert_eq!(textarea.cursor_col, "hello".len());
    }

    #[test]
    fn delete_prev_word_respects_utf8_word_boundaries() {
        let mut textarea = TextArea::new();
        textarea.set_text("héllo 世界");

        textarea.delete_prev_word();
        assert_eq!(textarea.text(), "héllo ");

        textarea.delete_prev_word();
        assert_eq!(textarea.text(), "");
        assert!(textarea.is_empty());
    }

    #[test]
    fn kill_to_eol_captures_text_and_yank_restores_it() {
        let mut textarea = TextArea::new();
        textarea.set_text("alpha beta gamma");
        textarea.cursor_col = "alpha ".len();

        textarea.kill_to_eol();
        assert_eq!(textarea.text(), "alpha ");
        assert_eq!(textarea.kill_buffer, "beta gamma");

        textarea.yank();
        assert_eq!(textarea.text(), "alpha beta gamma");
    }

    #[test]
    fn cursor_left_and_right_are_grapheme_aware() {
        let mut textarea = TextArea::new();
        let emoji = "👩‍💻";
        textarea.set_text(&format!("a{emoji}b"));
        textarea.move_to_line_start();

        textarea.move_cursor_right();
        assert_eq!(textarea.cursor_col, 1);

        textarea.move_cursor_right();
        assert_eq!(textarea.cursor_col, 1 + emoji.len());

        textarea.move_cursor_right();
        assert_eq!(textarea.cursor_col, textarea.lines[0].len());

        textarea.move_cursor_left();
        assert_eq!(textarea.cursor_col, 1 + emoji.len());

        textarea.move_cursor_left();
        assert_eq!(textarea.cursor_col, 1);
    }

    #[test]
    fn cursor_up_and_down_preserve_visual_column_when_possible() {
        let mut textarea = TextArea::new();
        textarea.set_text("abcd\nxy\n12345");
        textarea.cursor_row = 0;
        textarea.cursor_col = 3;

        textarea.move_cursor_down();
        assert_eq!(textarea.cursor_row, 1);
        assert_eq!(textarea.cursor_col, 2);

        textarea.move_cursor_down();
        assert_eq!(textarea.cursor_row, 2);
        assert_eq!(textarea.cursor_col, 2);

        textarea.move_cursor_up();
        assert_eq!(textarea.cursor_row, 1);
        assert_eq!(textarea.cursor_col, 2);
    }

    #[test]
    fn multi_line_height_is_capped_at_eight() {
        let mut textarea = TextArea::new();
        textarea.set_text("0\n1\n2\n3\n4\n5\n6\n7\n8\n9");

        assert_eq!(textarea.height(80), 8);
    }

    #[test]
    fn render_draws_text_and_highlights_cursor_cell() {
        let mut textarea = TextArea::new();
        textarea.set_text("hello");
        textarea.move_to_line_start();
        textarea.move_cursor_right();
        textarea.move_cursor_right();

        let area = Rect::new(0, 0, 6, 1);
        let mut buf = Buffer::empty(area);
        textarea.render(area, &mut buf);

        let rendered: String = (0..area.width)
            .map(|x| buf.cell(Position::new(x, 0)).expect("cell in range").symbol())
            .collect();
        assert_eq!(rendered, "hello ");

        let cursor_cell = buf.cell(Position::new(2, 0)).expect("cursor cell in range");
        assert!(cursor_cell.modifier.contains(Modifier::REVERSED));
    }
}
