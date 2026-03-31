//! Selection highlight overlay.
//!
//! A low-level [`Component`] that paints background color on selected cells
//! using direct canvas access. Unlike [`View`], it does NOT clear existing
//! text — it only adds background, so the underlying content stays visible.
//!
//! After painting, it reads the selected text from the canvas and writes it
//! to a shared [`State<String>`] so the parent can access it for clipboard copy.
//!
//! Place this as the **last child** in the element tree so it draws on top.

use iocraft::prelude::*;

use crate::components::selection::SelectionState;
use crate::theme::Theme;

/// Props for [`SelectionOverlay`].
#[derive(Default, Props)]
pub struct SelectionOverlayProps {
    /// The active selection, if any.
    pub selection: Option<SelectionState>,
    /// Terminal width (for sizing).
    pub width: u16,
    /// Terminal height (for sizing).
    pub height: u16,
    /// Theme (for selection colors).
    pub theme: Option<Theme>,
    /// Shared ref where the overlay writes the extracted text on each draw.
    /// Uses `Ref` instead of `State` to avoid triggering re-renders from
    /// within `draw()` (which would deadlock).
    pub extracted_text: Option<Ref<String>>,
}

/// Transparent overlay that highlights selected cells by painting only
/// background color on the canvas, then reads back the selected text.
#[derive(Default)]
pub struct SelectionOverlay {
    selection: Option<SelectionState>,
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

        // Always keep the overlay at full terminal size to avoid layout
        // changes between frames (which cause flicker). The `draw()` method
        // simply does nothing when there's no selection.
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
            // No selection — clear extracted text.
            if let Some(mut et) = self.extracted_text {
                et.set(String::new());
            }
            return;
        };
        if !sel.has_extent() {
            return;
        }

        let ((sc, sr), (ec, er)) = sel.normalized();
        let bg = self.theme.selection_bg;
        let mut canvas = drawer.canvas();

        // Paint background highlights AND read text from the canvas.
        let mut extracted = String::new();

        for row in sr..=er {
            let (col_start, col_end) = if sr == er {
                (sc, ec)
            } else if row == sr {
                (sc, self.width.saturating_sub(1))
            } else if row == er {
                (0, ec)
            } else {
                (0, self.width.saturating_sub(1))
            };

            let x = i32::from(col_start) as isize;
            let y = i32::from(row) as isize;
            let w = (col_end.saturating_sub(col_start) + 1) as usize;

            // Paint background only — preserves existing text.
            canvas.set_background_color(x, y, w, 1, bg);

            // Read the text content of this row from the canvas.
            let row_text = canvas.get_row_text(row as usize);
            let row_len = row_text.len();

            // Extract the selected portion of this row.
            let cs = (col_start as usize).min(row_len);
            let ce = ((col_end as usize) + 1).min(row_len);
            let slice = if cs < ce { &row_text[cs..ce] } else { "" };

            if !extracted.is_empty() {
                extracted.push('\n');
            }
            extracted.push_str(slice.trim_end());
        }

        // Write extracted text to shared state.
        if let Some(mut et) = self.extracted_text {
            et.set(extracted);
        }
    }
}
