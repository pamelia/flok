//! Input box component with blinking cursor via `TextInput`.

use iocraft::prelude::*;

use crate::theme::Theme;

#[derive(Default, Props)]
pub struct InputBoxProps {
    pub text: String,
    pub is_waiting: bool,
    pub on_change: HandlerMut<'static, String>,
    pub handle: Option<Ref<TextInputHandle>>,
    pub paste_indicator: Option<String>,
    pub theme: Option<Theme>,
}

#[component]
pub fn InputBox(props: &mut InputBoxProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme.unwrap_or_default();
    let border_color = if props.is_waiting { theme.border } else { theme.primary };
    let has_focus = !props.is_waiting;

    element! {
        View(
            flex_direction: FlexDirection::Column,
            width: 100pct,
        ) {
            // Paste indicator above the input
            #(props.paste_indicator.as_ref().map(|indicator| element! {
                View(padding_left: 1u32) {
                    Text(content: indicator.clone(), color: theme.warning, weight: Weight::Bold)
                }
            }))

            View(
                border_style: BorderStyle::Single,
                border_color: border_color,
                background_color: theme.bg,
                min_height: 3u32,
                max_height: 8u32,
                width: 100pct,
                padding_left: 1u32,
                padding_right: 1u32,
            ) {
                TextInput(
                    value: props.text.clone(),
                    has_focus: has_focus,
                    color: theme.text,
                    cursor_color: theme.primary,
                    on_change: props.on_change.take(),
                    handle: props.handle,
                )
            }
        }
    }
}
