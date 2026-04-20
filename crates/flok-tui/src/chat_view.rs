use ratatui::{layout::Rect, text::Line};

use crate::{history::ActiveItem, theme::Theme};

pub(crate) struct ChatView {
    /// Lines from the bottom of the transcript. `0` = pinned to newest line.
    pub(crate) scroll_offset: usize,
    pub(crate) follow_bottom: bool,
}

impl ChatView {
    pub(crate) fn new() -> Self {
        Self { scroll_offset: 0, follow_bottom: true }
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

    pub(crate) fn visible_lines_and_rows(
        &self,
        history: &[crate::history::HistoryItem],
        active: Option<&ActiveItem>,
        theme: &Theme,
        area: Rect,
    ) -> (Vec<Line<'static>>, Vec<String>) {
        self.visible_lines_and_rows_with_observer(history, active, theme, area, || {})
    }

    fn visible_lines_and_rows_with_observer<F>(
        &self,
        history: &[crate::history::HistoryItem],
        active: Option<&ActiveItem>,
        theme: &Theme,
        area: Rect,
        mut observe_history_item: F,
    ) -> (Vec<Line<'static>>, Vec<String>)
    where
        F: FnMut(),
    {
        if area.width == 0 || area.height == 0 {
            return (Vec::new(), Vec::new());
        }

        let mut all_lines: Vec<Line<'static>> = Vec::new();
        let mut all_rows: Vec<String> = Vec::new();
        for item in history {
            observe_history_item();
            extend_lines_and_rows(
                &mut all_lines,
                &mut all_rows,
                crate::history::render::lines(item, area.width, theme),
            );
        }
        if let Some(active) = active {
            extend_lines_and_rows(
                &mut all_lines,
                &mut all_rows,
                crate::history::render::active_lines(active, area.width, theme),
            );
        }

        let viewport = area.height as usize;
        let total_lines = all_lines.len();
        let max_offset = total_lines.saturating_sub(viewport);
        let scroll_offset = self.scroll_offset.min(max_offset);
        let start = total_lines.saturating_sub(viewport + scroll_offset);
        let end = total_lines.saturating_sub(scroll_offset);

        let mut visible = all_lines[start..end].to_vec();
        let mut visible_rows = all_rows[start..end].to_vec();
        while visible.len() < viewport {
            visible.push(Line::default());
            visible_rows.push(String::new());
        }
        (visible, visible_rows)
    }
}

fn extend_lines_and_rows(
    all_lines: &mut Vec<Line<'static>>,
    all_rows: &mut Vec<String>,
    lines: Vec<Line<'static>>,
) {
    for line in lines {
        all_rows.push(line.spans.iter().map(|span| span.content.as_ref()).collect());
        all_lines.push(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        buffer::Buffer,
        layout::{Position, Rect},
        widgets::Widget,
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
        let (lines, _) = v.visible_lines_and_rows(&[], None, &crate::theme::Theme::dark(), area);
        for (index, line) in lines.iter().enumerate() {
            line.clone().render(
                Rect { x: area.x, y: area.y + index as u16, width: area.width, height: 1 },
                &mut buf,
            );
        }
    }

    #[test]
    fn visible_rows_matches_render_output_length() {
        let v = ChatView::new();
        let area = Rect::new(0, 0, 20, 4);
        let theme = crate::theme::Theme::dark();
        let history = vec![crate::history::HistoryItem::user("hello world")];
        let mut buf = Buffer::empty(area);

        let (lines, rows) = v.visible_lines_and_rows(&history, None, &theme, area);
        for (index, line) in lines.iter().enumerate() {
            line.clone().render(
                Rect { x: area.x, y: area.y + index as u16, width: area.width, height: 1 },
                &mut buf,
            );
        }

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

    #[test]
    fn visible_lines_and_rows_single_walk() {
        use std::cell::Cell;

        let view = ChatView::new();
        let area = Rect::new(0, 0, 20, 4);
        let theme = crate::theme::Theme::dark();
        let history = vec![
            crate::history::HistoryItem::user("one"),
            crate::history::HistoryItem::user("two"),
        ];
        let walks = Cell::new(0usize);

        let (_lines, rows) =
            view.visible_lines_and_rows_with_observer(&history, None, &theme, area, || {
                walks.set(walks.get().saturating_add(1));
            });

        assert_eq!(walks.get(), history.len());
        assert_eq!(rows.len(), usize::from(area.height));
    }
}
