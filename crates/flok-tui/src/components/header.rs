//! Header/status bar component.
//!
//! Dark background, colored status indicator, model name, key hints.

use iocraft::prelude::*;

use crate::theme::Theme;

#[derive(Default, Props)]
pub struct HeaderProps {
    pub model_name: String,
    pub status_text: String,
    pub is_plan: bool,
    pub theme: Option<Theme>,
}

#[component]
pub fn Header(props: &HeaderProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme.unwrap_or_default();

    // Mode badge
    let mode_text = if props.is_plan { " Plan " } else { " Build " };
    let mode_color = if props.is_plan { theme.accent } else { theme.primary };

    // Status color
    let status_color = if props.status_text == "ready" { theme.success } else { theme.accent };

    element! {
        View(
            width: 100pct,
            background_color: theme.bg_element,
            flex_direction: FlexDirection::Row,
            padding_left: 1u32,
            padding_right: 1u32,
            align_items: AlignItems::Center,
        ) {
            // Mode indicator: colored square + mode name
            Text(content: "\u{25A0} ", color: mode_color)
            Text(content: mode_text, color: mode_color, weight: Weight::Bold)
            Text(content: " ", color: theme.bg_element)

            // Model name
            Text(content: props.model_name.clone(), color: theme.text)

            // Spacer
            View(flex_grow: 1.0) {}

            // Status
            Text(content: props.status_text.clone(), color: status_color)
        }
    }
}
