pub(crate) mod permission;
pub(crate) mod question;
pub(crate) mod slash_popup;

use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use crate::app_event::AppEvent;
use crate::theme::Theme;

use self::permission::PermissionOverlay;
use self::question::QuestionOverlay;

/// A full-width modal overlay rendered on top of the main chat surface.
///
/// The single-overlay design is intentional for Wave 1: only one of these is
/// ever visible at a time. `SlashPopup` and `Picker` variants will be added in
/// T3D and T5D respectively.
pub(crate) enum Overlay {
    Permission(PermissionOverlay),
    Question(QuestionOverlay),
}

/// Result of handing a key event to an overlay.
pub(crate) enum OverlayResult {
    /// Overlay consumed the key and stays open.
    Consumed,
    /// Overlay is finished; the caller should remove it.
    Closed,
    /// Overlay did not handle the key; pass it through.
    #[expect(dead_code, reason = "reserved for future pass-through overlay behaviors")]
    None,
}

impl Overlay {
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> (OverlayResult, Option<AppEvent>) {
        match self {
            Overlay::Permission(p) => p.handle_key(key),
            Overlay::Question(q) => q.handle_key(key),
        }
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        match self {
            Overlay::Permission(p) => p.render(area, buf, theme),
            Overlay::Question(q) => q.render(area, buf, theme),
        }
    }

    pub(crate) fn desired_height(&self, width: u16) -> u16 {
        match self {
            Overlay::Permission(p) => p.desired_height(width),
            Overlay::Question(q) => q.desired_height(width),
        }
    }
}

/// Convert a `crossterm::style::Color` (used in `Theme`) into a `ratatui` color.
///
/// Mirrors the private helper in `theme.rs` and `footer.rs`; each module keeps
/// its own copy so it remains self-contained.
pub(crate) fn ratatui_color(color: crossterm::style::Color) -> ratatui::style::Color {
    use crossterm::style::Color as Cc;
    match color {
        Cc::Reset => ratatui::style::Color::Reset,
        Cc::Black => ratatui::style::Color::Black,
        Cc::DarkGrey => ratatui::style::Color::DarkGray,
        Cc::Red | Cc::DarkRed => ratatui::style::Color::Red,
        Cc::Green | Cc::DarkGreen => ratatui::style::Color::Green,
        Cc::Yellow | Cc::DarkYellow => ratatui::style::Color::Yellow,
        Cc::Blue | Cc::DarkBlue => ratatui::style::Color::Blue,
        Cc::Magenta | Cc::DarkMagenta => ratatui::style::Color::Magenta,
        Cc::Cyan | Cc::DarkCyan => ratatui::style::Color::Cyan,
        Cc::Grey => ratatui::style::Color::Gray,
        Cc::White => ratatui::style::Color::White,
        Cc::Rgb { r, g, b } => ratatui::style::Color::Rgb(r, g, b),
        Cc::AnsiValue(value) => ratatui::style::Color::Indexed(value),
    }
}
