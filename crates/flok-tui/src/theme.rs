use crossterm::style::Color;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Theme {
    pub(crate) dim: Color,
    pub(crate) accent_user: Color,
    pub(crate) accent_assistant: Color,
    pub(crate) accent_tool: Color,
    pub(crate) error: Color,
    pub(crate) warn: Color,
    pub(crate) info: Color,
    pub(crate) border: Color,
    pub(crate) divider: Color,
    pub(crate) plan_badge: Color,
    pub(crate) build_badge: Color,
    pub(crate) text: Color,
    pub(crate) text_muted: Color,
    pub(crate) secondary: Color,
    pub(crate) border_active: Color,
    pub(crate) code_fg: Color,
    pub(crate) code_bg: Color,
    pub(crate) heading: Color,
    pub(crate) bold_fg: Color,
}

impl Theme {
    pub(crate) fn dark() -> Self {
        Self {
            dim: Color::Rgb { r: 128, g: 128, b: 128 },
            accent_user: Color::Rgb { r: 92, g: 156, b: 245 },
            accent_assistant: Color::Rgb { r: 250, g: 178, b: 131 },
            accent_tool: Color::Rgb { r: 157, g: 124, b: 216 },
            error: Color::Rgb { r: 224, g: 108, b: 117 },
            warn: Color::Rgb { r: 245, g: 167, b: 66 },
            info: Color::Rgb { r: 86, g: 182, b: 194 },
            border: Color::Rgb { r: 72, g: 72, b: 72 },
            divider: Color::Rgb { r: 60, g: 60, b: 60 },
            plan_badge: Color::Rgb { r: 245, g: 167, b: 66 },
            build_badge: Color::Rgb { r: 127, g: 216, b: 143 },
            text: Color::Rgb { r: 238, g: 238, b: 238 },
            text_muted: Color::Rgb { r: 128, g: 128, b: 128 },
            secondary: Color::Rgb { r: 92, g: 156, b: 245 },
            border_active: Color::Rgb { r: 250, g: 178, b: 131 },
            code_fg: Color::Rgb { r: 127, g: 216, b: 143 },
            code_bg: Color::Rgb { r: 10, g: 10, b: 10 },
            heading: Color::Rgb { r: 157, g: 124, b: 216 },
            bold_fg: Color::Rgb { r: 245, g: 167, b: 66 },
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_constructs_without_panic() {
        let theme = Theme::dark();
        assert!(matches!(theme.accent_user, Color::Rgb { .. }));
    }

    #[test]
    fn default_is_dark() {
        let theme = Theme::default();
        let dark = Theme::dark();
        assert_eq!(theme.accent_user, dark.accent_user);
        assert_eq!(theme.accent_assistant, dark.accent_assistant);
        assert_eq!(theme.accent_tool, dark.accent_tool);
        assert_eq!(theme.error, dark.error);
        assert_eq!(theme.warn, dark.warn);
        assert_eq!(theme.info, dark.info);
    }

    #[test]
    fn used_render_fields_are_stable() {
        let theme = Theme::dark();
        assert!(matches!(theme.border, Color::Rgb { .. }));
        assert!(matches!(theme.text, Color::Rgb { .. }));
        assert!(matches!(theme.code_fg, Color::Rgb { .. }));
    }
}
