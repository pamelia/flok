//! Message list component with scrolling.

use iocraft::prelude::*;

use crate::components::app::{DisplayMessage, MessageRole};
use crate::components::markdown::Markdown;
use crate::theme::Theme;

#[derive(Default, Props)]
pub struct MessageListProps {
    pub messages: Vec<DisplayMessage>,
    pub streaming_text: String,
    pub streaming_reasoning: String,
    pub is_waiting: bool,
    pub theme: Option<Theme>,
    /// Handle for imperative scroll control from the parent.
    pub scroll_handle: Option<Ref<ScrollViewHandle>>,
    /// Whether auto-scroll is disengaged (user scrolled up).
    /// Written by this component so the parent can show an indicator.
    pub scrolled_up: Option<State<bool>>,
}

#[component]
pub fn MessageList(
    mut hooks: Hooks,
    props: &mut MessageListProps,
) -> impl Into<AnyElement<'static>> {
    let theme = props.theme.unwrap_or_default();
    let messages = &props.messages;
    let streaming = &props.streaming_text;
    let reasoning = &props.streaming_reasoning;
    let is_waiting = props.is_waiting;

    // Internal handle for checking auto-scroll state.
    let mut internal_handle = hooks.use_ref(ScrollViewHandle::default);

    // Sync auto-scroll state to the parent's State.
    if let Some(mut scrolled_up) = props.scrolled_up {
        let pinned = internal_handle.write().is_auto_scroll_pinned();
        if scrolled_up.get() == pinned {
            scrolled_up.set(!pinned);
        }
    }

    // Use the parent-supplied handle if provided, otherwise use the internal one.
    let handle = props.scroll_handle.unwrap_or(internal_handle);

    element! {
        View(flex_grow: 1.0, flex_direction: FlexDirection::Column, overflow: Overflow::Hidden) {
            ScrollView(
                auto_scroll: true,
                keyboard_scroll: Some(false),
                scroll_step: Some(0u16),
                handle: handle,
                scrollbar_thumb_color: theme.text_muted,
                scrollbar_track_color: theme.bg_element,
            ) {
                View(flex_direction: FlexDirection::Column, width: 100pct) {
                    // Messages
                    #(messages.iter().enumerate().map(|(i, msg)| {
                        let key = ElementKey::new(i);
                        let content = msg.content.clone();
                        match msg.role {
                            MessageRole::User => element! {
                                // User message: contained block with left accent + bg
                                View(key, padding_left: 2u32, padding_top: 1u32, padding_right: 2u32) {
                                    View(flex_direction: FlexDirection::Row, width: 100pct) {
                                        // Left accent bar (thick ┃ in secondary/blue)
                                        View(width: 1u32) {
                                            Text(content: "\u{2503}", color: theme.secondary)
                                        }
                                        // Content area with panel background
                                        View(
                                            flex_grow: 1.0,
                                            background_color: theme.bg_panel,
                                            padding_top: 1u32,
                                            padding_bottom: 1u32,
                                            padding_left: 2u32,
                                            padding_right: 1u32,
                                        ) {
                                            Text(content: content, color: theme.text)
                                        }
                                    }
                                }
                            },
                            MessageRole::Assistant => element! {
                                // Assistant message: no container, just padding
                                View(key, padding_left: 4u32, padding_top: 1u32) {
                                    Markdown(content: content, theme: theme)
                                }
                            },
                            MessageRole::System => element! {
                                View(key, padding_left: 2u32, padding_top: 1u32, padding_right: 2u32) {
                                    View(flex_direction: FlexDirection::Row, width: 100pct) {
                                        // Left accent bar (warning/orange)
                                        View(width: 1u32) {
                                            Text(content: "\u{2503}", color: theme.warning)
                                        }
                                        View(
                                            flex_direction: FlexDirection::Column,
                                            flex_grow: 1.0,
                                            background_color: theme.bg_panel,
                                            padding_top: 1u32,
                                            padding_bottom: 1u32,
                                            padding_left: 2u32,
                                        ) {
                                            Text(content: "\u{26A0} System", color: theme.warning, weight: Weight::Bold)
                                            View(padding_top: 1u32) {
                                                Text(content: content, color: theme.text_muted)
                                            }
                                        }
                                    }
                                }
                            },
                            MessageRole::ToolCall => element! {
                                // Tool calls: subtle with left accent
                                View(key, padding_left: 4u32) {
                                    View(flex_direction: FlexDirection::Row) {
                                        View(width: 1u32) {
                                            Text(content: "\u{2502}", color: theme.border)
                                        }
                                        View(padding_left: 1u32) {
                                            Text(content: content, color: theme.accent)
                                        }
                                    }
                                }
                            },
                        }
                    }))

                    // Reasoning/thinking display
                    #(if !reasoning.is_empty() && streaming.is_empty() {
                        Some(element! {
                            View(padding_left: 4u32, padding_top: 1u32) {
                                View(flex_direction: FlexDirection::Row) {
                                    View(width: 1u32) {
                                        Text(content: "\u{2502}", color: theme.bg_element)
                                    }
                                    View(padding_left: 1u32, flex_direction: FlexDirection::Column) {
                                        Text(content: "\u{1F4AD} Thinking...", color: theme.accent, italic: true)
                                        View(padding_top: 1u32) {
                                            Text(content: reasoning.clone(), color: theme.text_muted, italic: true)
                                        }
                                    }
                                }
                            }
                        })
                    } else {
                        None
                    })

                    // Streaming text
                    #(if !streaming.is_empty() {
                        Some(element! {
                            View(padding_left: 4u32, padding_top: 1u32) {
                                Markdown(content: streaming.clone(), theme: theme)
                            }
                        })
                    } else if is_waiting {
                        Some(element! {
                            View(padding_left: 4u32, padding_top: 1u32) {
                                Text(content: "Thinking...", color: theme.text_muted, italic: true)
                            }
                        })
                    } else {
                        None
                    })
                }
            }
        }
    }
}
