//! Selection highlight overlay.
//!
//! A low-level [`Component`] that paints background color on selected cells
//! using direct canvas access. Unlike [`View`], it does NOT clear existing
//! text — it only adds background, so the underlying content stays visible.
//!
//! After painting, it reads the selected text from the canvas and writes it
//! to a shared [`Ref<String>`] so the parent can access it for clipboard copy.
//!
//! Place this as the **last child** in the element tree so it draws on top.

use iocraft::prelude::*;

use crate::components::selection::{substr_by_width, ResolvedSelection};
use crate::theme::Theme;

/// Props for [`SelectionOverlay`].
#[derive(Default, Props)]
pub struct SelectionOverlayProps {
    /// The active selection, if any.
    pub selection: Option<ResolvedSelection>,
    /// Terminal width (for sizing).
    pub width: u16,
    /// Terminal height (for sizing).
    pub height: u16,
    /// Theme (for selection colors).
    pub theme: Option<Theme>,
    /// Shared ref where the overlay writes the extracted text on each draw.
    pub extracted_text: Option<Ref<String>>,
}

/// Transparent overlay that highlights selected cells by painting only
/// background color on the canvas, then reads back the selected text.
#[derive(Default)]
pub struct SelectionOverlay {
    selection: Option<ResolvedSelection>,
    width: u16,
    height: u16,
    theme: Theme,
    extracted_text: Option<Ref<String>>,
}

impl Component for SelectionOverlay {
    type Props<'a> = SelectionOverlayProps;

    fn new(_props: &Self::Props<'_>) -> Self {
        Self::default()
    }

    fn update(
        &mut self,
        props: &mut Self::Props<'_>,
        _hooks: Hooks,
        updater: &mut ComponentUpdater,
    ) {
        self.selection = props.selection.take();
        self.width = props.width;
        self.height = props.height;
        self.theme = props.theme.unwrap_or_default();
        self.extracted_text = props.extracted_text;

        updater.set_layout_style(taffy::style::Style {
            position: taffy::style::Position::Absolute,
            inset: taffy::geometry::Rect {
                left: taffy::style::LengthPercentageAuto::Length(0.0),
                top: taffy::style::LengthPercentageAuto::Length(0.0),
                right: taffy::style::LengthPercentageAuto::Auto,
                bottom: taffy::style::LengthPercentageAuto::Auto,
            },
            size: taffy::geometry::Size {
                width: taffy::style::Dimension::Length(f32::from(self.width)),
                height: taffy::style::Dimension::Length(f32::from(self.height)),
            },
            ..Default::default()
        });
    }

    fn draw(&mut self, drawer: &mut ComponentDrawer<'_>) {
        let Some(ref sel) = self.selection else {
            if let Some(mut extracted_text) = self.extracted_text {
                extracted_text.set(String::new());
            }
            return;
        };

        let ((sc, sr), (ec, er)) = sel.normalized();
        let bg = self.theme.selection_bg;
        let mut canvas = drawer.canvas();
        let panel = sel.panel_rect;
        let panel_right = panel.x + panel.w.saturating_sub(1);
        let mut extracted = String::new();

        for row in sr..=er {
            let (col_start, col_end) = if sr == er {
                (sc, ec)
            } else if row == sr {
                (sc, panel_right)
            } else if row == er {
                (panel.x, ec)
            } else {
                (panel.x, panel_right)
            };

            let x = i32::from(col_start) as isize;
            let y = i32::from(row) as isize;
            let w = (col_end.saturating_sub(col_start) + 1) as usize;

            // Paint background only — preserves existing text.
            canvas.set_background_color(x, y, w, 1, bg);

            let row_text = canvas.get_row_text(row as usize);
            let rel_start = col_start.saturating_sub(panel.x) as usize;
            let rel_end = col_end.saturating_sub(panel.x) as usize;
            let slice = substr_by_width(&row_text, rel_start, rel_end);
            let slice = slice.trim_end();

            if !extracted.is_empty() {
                extracted.push('\n');
            }
            extracted.push_str(slice);
        }

        if let Some(mut extracted_text) = self.extracted_text {
            extracted_text.set(extracted);
        }
    }
}
