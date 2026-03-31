//! Color theme for the TUI.
//!
//! Colors matched to the `OpenCode` "opencode" default dark theme for a
//! consistent, polished terminal aesthetic.

use iocraft::prelude::Color;

/// A color theme.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    // Core backgrounds
    pub bg: Color,
    pub bg_panel: Color,
    pub bg_element: Color,

    // Text
    pub text: Color,
    pub text_muted: Color,

    // Semantic colors
    pub primary: Color, // Warm peach/orange — headings in sidebar, section titles
    pub secondary: Color, // Blue — user badge, links
    pub accent: Color,  // Purple/violet — markdown headings, keywords
    pub highlight: Color, // Green — inline code, success highlights
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub info: Color,

    // Borders
    pub border: Color,
    pub border_active: Color,

    // Selection
    pub selection_bg: Color,
    pub selection_fg: Color,

    // Markdown-specific
    pub code_fg: Color, // Inline code text (green)
    pub code_bg: Color, // Code block background
    pub heading: Color, // Markdown headings (purple/violet)
    pub bold_fg: Color, // Bold text (orange/gold)
    pub table_border: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            // Backgrounds — near-black with subtle stepping
            bg: Color::Rgb { r: 10, g: 10, b: 10 }, // #0a0a0a
            bg_panel: Color::Rgb { r: 20, g: 20, b: 20 }, // #141414
            bg_element: Color::Rgb { r: 30, g: 30, b: 30 }, // #1e1e1e

            // Text — bright for readability
            text: Color::Rgb { r: 238, g: 238, b: 238 }, // #eeeeee
            text_muted: Color::Rgb { r: 128, g: 128, b: 128 }, // #808080

            // Semantic colors — matched to OpenCode's opencode.json dark mode
            primary: Color::Rgb { r: 250, g: 178, b: 131 }, // #fab283 warm peach/orange
            secondary: Color::Rgb { r: 92, g: 156, b: 245 }, // #5c9cf5 blue
            accent: Color::Rgb { r: 157, g: 124, b: 216 },  // #9d7cd8 purple/violet
            highlight: Color::Rgb { r: 127, g: 216, b: 143 }, // #7fd88f green
            error: Color::Rgb { r: 224, g: 108, b: 117 },   // #e06c75 soft red
            warning: Color::Rgb { r: 245, g: 167, b: 66 },  // #f5a742 orange/gold
            success: Color::Rgb { r: 127, g: 216, b: 143 }, // #7fd88f green
            info: Color::Rgb { r: 86, g: 182, b: 194 },     // #56b6c2 teal/cyan

            // Borders
            border: Color::Rgb { r: 72, g: 72, b: 72 }, // #484848
            border_active: Color::Rgb { r: 250, g: 178, b: 131 }, // #fab283 (same as primary)

            // Selection — inverted style for text selection
            selection_bg: Color::Rgb { r: 92, g: 156, b: 245 }, // #5c9cf5 blue
            selection_fg: Color::Rgb { r: 10, g: 10, b: 10 },   // #0a0a0a (same as bg)

            // Markdown
            code_fg: Color::Rgb { r: 127, g: 216, b: 143 }, // #7fd88f green (inline code)
            code_bg: Color::Rgb { r: 10, g: 10, b: 10 },    // #0a0a0a (same as bg)
            heading: Color::Rgb { r: 157, g: 124, b: 216 }, // #9d7cd8 purple/violet
            bold_fg: Color::Rgb { r: 245, g: 167, b: 66 },  // #f5a742 orange/gold
            table_border: Color::Rgb { r: 128, g: 128, b: 128 }, // #808080 (same as text_muted)
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}
