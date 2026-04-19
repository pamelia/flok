use ratatui::{buffer::Buffer, layout::Rect, text::Line, widgets::Widget};

use crate::{
    history::{ActiveItem, HistoryItem, Role},
    theme::Theme,
};

pub(crate) struct ChatView {
    /// Lines from the bottom of the transcript. `0` = pinned to newest line.
    pub(crate) scroll_offset: usize,
    pub(crate) follow_bottom: bool,
}

impl ChatView {
    pub(crate) fn new() -> Self {
        Self { scroll_offset: 0, follow_bottom: true }
    }

    pub(crate) fn render(
        &self,
        history: &[HistoryItem],
        active: Option<&ActiveItem>,
        theme: &Theme,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        for (i, line) in self.visible_lines(history, active, theme, area).iter().enumerate() {
            let row_y = area.y.saturating_add(i as u16);
            if row_y >= area.y.saturating_add(area.height) {
                break;
            }
            let row = Rect { x: area.x, y: row_y, width: area.width, height: 1 };
            line.clone().render(row, buf);
        }
    }

    pub(crate) fn visible_rows(
        &self,
        history: &[HistoryItem],
        active: Option<&ActiveItem>,
        theme: &Theme,
        area: Rect,
    ) -> Vec<String> {
        self.visible_lines(history, active, theme, area)
            .into_iter()
            .map(|line| line.spans.into_iter().map(|span| span.content.into_owned()).collect())
            .collect()
    }

    /// Negative `delta` scrolls up (toward older lines); positive scrolls down.
    pub(crate) fn handle_scroll(&mut self, delta: i32, viewport_height: u16, total_height: usize) {
        if delta < 0 {
            let max_offset = total_height.saturating_sub(viewport_height as usize);
            let requested = self.scroll_offset.saturating_add(delta.unsigned_abs() as usize);
            self.scroll_offset = requested.min(max_offset);
            self.follow_bottom = false;
        } else if delta > 0 {
            self.scroll_offset = self.scroll_offset.saturating_sub(delta as usize);
            if self.scroll_offset == 0 {
                self.follow_bottom = true;
            }
        }
    }

    /// No-op: bottom-anchored `scroll_offset` handles growth automatically.
    pub(crate) fn on_new_content(&mut self) {
        let _ = self;
    }

    fn visible_lines(
        &self,
        history: &[HistoryItem],
        active: Option<&ActiveItem>,
        theme: &Theme,
        area: Rect,
    ) -> Vec<Line<'static>> {
        if area.width == 0 || area.height == 0 {
            return Vec::new();
        }

        let mut all_lines: Vec<Line<'static>> = Vec::new();
        for item in history {
            all_lines.extend(crate::history::render::lines(item, area.width, theme));
        }
        if let Some(active) = active {
            let synth = synthesize_active(active);
            all_lines.extend(crate::history::render::lines(&synth, area.width, theme));
        }

        let viewport = area.height as usize;
        let total_lines = all_lines.len();
        let max_offset = total_lines.saturating_sub(viewport);
        let scroll_offset = self.scroll_offset.min(max_offset);
        let start = total_lines.saturating_sub(viewport + scroll_offset);
        let end = total_lines.saturating_sub(scroll_offset);

        let mut visible = all_lines[start..end].to_vec();
        while visible.len() < viewport {
            visible.push(Line::default());
        }
        visible
    }
}

fn synthesize_active(active: &ActiveItem) -> HistoryItem {
    match active.role {
        Role::ToolCall => HistoryItem::ToolCall {
            name: active.tool_name.clone().unwrap_or_default(),
            preview: active.streaming_text.clone(),
            is_error: false,
            duration_ms: None,
        },
        _ => HistoryItem::Assistant { text: active.streaming_text.clone(), markdown: true },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        buffer::Buffer,
        layout::{Position, Rect},
    };

    #[test]
    fn fresh_view_is_at_bottom() {
        let v = ChatView::new();
        assert_eq!(v.scroll_offset, 0);
        assert!(v.follow_bottom);
    }

    #[test]
    fn scroll_up_disables_follow_bottom() {
        let mut v = ChatView::new();
        v.handle_scroll(-5, 10, 100);
        assert_eq!(v.scroll_offset, 5);
        assert!(!v.follow_bottom);
    }

    #[test]
    fn scroll_back_to_bottom_reenables_follow_bottom() {
        let mut v = ChatView::new();
        v.handle_scroll(-5, 10, 100);
        v.handle_scroll(5, 10, 100);
        assert_eq!(v.scroll_offset, 0);
        assert!(v.follow_bottom);
    }

    #[test]
    fn scroll_up_clamped_to_max() {
        let mut v = ChatView::new();
        v.handle_scroll(-1000, 10, 20);
        assert_eq!(v.scroll_offset, 10);
    }

    #[test]
    fn render_empty_history_produces_blank_area() {
        let v = ChatView::new();
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        v.render(&[], None, &crate::theme::Theme::dark(), area, &mut buf);
    }

    #[test]
    fn visible_rows_matches_render_output_length() {
        let v = ChatView::new();
        let area = Rect::new(0, 0, 20, 4);
        let theme = crate::theme::Theme::dark();
        let history = vec![HistoryItem::user("hello world")];
        let mut buf = Buffer::empty(area);

        v.render(&history, None, &theme, area, &mut buf);
        let rows = v.visible_rows(&history, None, &theme, area);

        assert_eq!(rows.len(), usize::from(area.height));
        for y in 0..area.height {
            let rendered: String = (0..area.width)
                .filter_map(|x| buf.cell(Position::new(x, y)))
                .map(|cell| cell.symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string();
            assert_eq!(rows[usize::from(y)], rendered);
        }
    }
}
