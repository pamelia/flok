//! Permission prompt component.
//!
//! Renders inline at the bottom of the session view, replacing the input box.
//! Uses a colored left border (amber/warning) like opencode.

use iocraft::prelude::*;

use crate::theme::Theme;

#[derive(Default, Props)]
pub struct PermissionPromptProps {
    pub tool: String,
    pub description: String,
    /// The pattern that "Allow always" will approve (e.g., `"git commit *"`).
    pub always_pattern: String,
    pub selected: u8, // 0=Allow, 1=Always, 2=Deny
    pub theme: Option<Theme>,
}

#[component]
pub fn PermissionPrompt(props: &PermissionPromptProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme.unwrap_or_default();
    let selected = props.selected;

    // Button colors based on selection
    let allow_bg = if selected == 0 { theme.warning } else { theme.bg_element };
    let always_bg = if selected == 1 { theme.warning } else { theme.bg_element };
    let deny_bg = if selected == 2 { theme.error } else { theme.bg_element };

    let allow_fg = if selected == 0 { theme.bg } else { theme.text };
    let always_fg = if selected == 1 { theme.bg } else { theme.text };
    let deny_fg = if selected == 2 { theme.bg } else { theme.text };

    element! {
        View(
            width: 100pct,
            border_style: BorderStyle::Custom(BorderCharacters {
                top_left: ' ',
                top_right: ' ',
                bottom_left: ' ',
                bottom_right: ' ',
                left: '\u{2503}',
                right: ' ',
                top: ' ',
                bottom: ' ',
            }),
            border_edges: Edges::Left,
            border_color: theme.warning,
            background_color: theme.bg_panel,
            flex_direction: FlexDirection::Column,
            padding_left: 1u32,
            padding_right: 1u32,
        ) {
            // Header
            View(padding_top: 1u32, padding_bottom: 1u32) {
                Text(
                    content: format!("\u{25B3} Permission required"),
                    color: theme.warning,
                    weight: Weight::Bold,
                )
            }

            // Description
            View(padding_bottom: 1u32) {
                Text(content: props.description.clone(), color: theme.text)
            }

            // Option buttons bar
            View(
                flex_direction: FlexDirection::Row,
                background_color: theme.bg_element,
                padding_top: 1u32,
                padding_bottom: 1u32,
                padding_left: 1u32,
            ) {
                View(background_color: allow_bg, padding_left: 1u32, padding_right: 1u32) {
                    Text(content: "Allow once", color: allow_fg, weight: Weight::Bold)
                }
                Text(content: "  ", color: theme.bg_element)
                View(background_color: always_bg, padding_left: 1u32, padding_right: 1u32) {
                    Text(
                        content: if props.always_pattern.is_empty() {
                            "Allow always".to_string()
                        } else {
                            format!("Allow always: {}", props.always_pattern)
                        },
                        color: always_fg,
                        weight: Weight::Bold,
                    )
                }
                Text(content: "  ", color: theme.bg_element)
                View(background_color: deny_bg, padding_left: 1u32, padding_right: 1u32) {
                    Text(content: "Reject", color: deny_fg, weight: Weight::Bold)
                }
                Text(content: "       ", color: theme.bg_element)
                Text(
                    content: "\u{21C6} select  enter confirm",
                    color: theme.text_muted,
                )
            }
        }
    }
}
