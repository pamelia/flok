//! Status bar component (bottom bar).
//!
//! Left: mode indicator + model name + provider
//! Right: keybinding hints (key in cyan, label in muted)

use iocraft::prelude::*;

use crate::theme::Theme;

#[derive(Default, Props)]
pub struct StatusBarProps {
    pub model_name: String,
    pub is_plan: bool,
    pub theme: Option<Theme>,
}

#[component]
pub fn StatusBar(props: &StatusBarProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme.unwrap_or_default();

    let mode_text = if props.is_plan { "Plan" } else { "Build" };
    let mode_color = if props.is_plan { theme.accent } else { theme.primary };

    // Extract provider from model name (e.g., "claude-sonnet-4-20250514" -> "Anthropic")
    let provider = if props.model_name.contains("claude") {
        "Anthropic"
    } else if props.model_name.contains("gpt") {
        "OpenAI"
    } else if props.model_name.contains("gemini") {
        "Google"
    } else if props.model_name.contains("deepseek") {
        "DeepSeek"
    } else {
        ""
    };

    element! {
        View(
            width: 100pct,
            background_color: theme.bg_element,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding_left: 1u32,
            padding_right: 1u32,
        ) {
            // Left: mode + model + provider
            Text(content: "\u{2503} ", color: mode_color)
            Text(content: mode_text, color: mode_color, weight: Weight::Bold)
            Text(content: "  ", color: theme.bg_element)
            Text(content: props.model_name.clone(), color: theme.text)
            Text(content: "  ", color: theme.bg_element)
            Text(content: provider, color: theme.text_muted)

            // Spacer
            View(flex_grow: 1.0) {}

            // Right: keybinding hints
            Text(content: "ctrl+k", color: theme.primary)
            Text(content: " commands  ", color: theme.text_muted)
            Text(content: "tab", color: theme.primary)
            Text(content: " plan/build  ", color: theme.text_muted)
            Text(content: "ctrl+m", color: theme.primary)
            Text(content: " models  ", color: theme.text_muted)
            Text(content: "ctrl+c", color: theme.primary)
            Text(content: " quit", color: theme.text_muted)
        }
    }
}
