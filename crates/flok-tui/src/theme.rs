//! Color theme for the TUI.
//!
//! Inspired by the opencode TUI aesthetic: dark background, bright cyan
//! headings, green highlights, orange feature names, muted gray for secondary.

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
    pub primary: Color,   // Bright cyan — headings, table headers, section titles
    pub secondary: Color, // Blue — user badge, links
    pub accent: Color,    // Orange/amber — feature names, plan mode badge
    pub highlight: Color, // Green — field names, new items, success highlights
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub info: Color,

    // Borders
    pub border: Color,

    // Markdown-specific
    pub code_fg: Color, // Inline code text
    pub code_bg: Color, // Code block background
    pub heading: Color, // Section headings (cyan)
    pub bold_fg: Color, // Bold text
    pub table_border: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            // Backgrounds
            bg: Color::Rgb { r: 13, g: 13, b: 13 }, // #0d0d0d
            bg_panel: Color::Rgb { r: 22, g: 22, b: 22 }, // #161616
            bg_element: Color::Rgb { r: 30, g: 30, b: 30 }, // #1e1e1e

            // Text
            text: Color::Rgb { r: 204, g: 204, b: 204 }, // #cccccc
            text_muted: Color::Rgb { r: 100, g: 100, b: 100 }, // #646464

            // Semantic colors
            primary: Color::Rgb { r: 0, g: 212, b: 212 }, // #00d4d4 bright cyan
            secondary: Color::Rgb { r: 92, g: 156, b: 245 }, // #5c9cf5 blue
            accent: Color::Rgb { r: 255, g: 140, b: 0 },  // #ff8c00 orange
            highlight: Color::Rgb { r: 0, g: 255, b: 136 }, // #00ff88 green
            error: Color::Rgb { r: 255, g: 85, b: 85 },   // #ff5555
            warning: Color::Rgb { r: 255, g: 200, b: 50 }, // #ffc832
            success: Color::Rgb { r: 0, g: 255, b: 136 }, // #00ff88 (same as highlight)
            info: Color::Rgb { r: 0, g: 212, b: 212 },    // #00d4d4 (same as primary)

            // Borders
            border: Color::Rgb { r: 50, g: 50, b: 50 }, // #323232

            // Markdown
            code_fg: Color::Rgb { r: 0, g: 212, b: 212 }, // cyan for inline code
            code_bg: Color::Rgb { r: 25, g: 25, b: 30 },  // slightly blue-tinted dark
            heading: Color::Rgb { r: 0, g: 212, b: 212 }, // cyan headings
            bold_fg: Color::Rgb { r: 255, g: 255, b: 255 }, // pure white for bold
            table_border: Color::Rgb { r: 60, g: 60, b: 60 }, // dark gray table borders
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}
