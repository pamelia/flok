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
}

#[component]
pub fn MessageList(props: &MessageListProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme.unwrap_or_default();
    let messages = &props.messages;
    let streaming = &props.streaming_text;
    let reasoning = &props.streaming_reasoning;
    let is_waiting = props.is_waiting;

    element! {
        View(flex_grow: 1.0, flex_direction: FlexDirection::Column, overflow: Overflow::Hidden) {
            ScrollView(
                auto_scroll: true,
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
                                View(key, padding_left: 2u32, padding_top: 1u32) {
                                    View(flex_direction: FlexDirection::Column) {
                                        Text(content: "\u{25B6} You", color: theme.secondary, weight: Weight::Bold)
                                        View(padding_left: 2u32, padding_top: 1u32) {
                                            Text(content: content, color: theme.text)
                                        }
                                    }
                                }
                            },
                            MessageRole::Assistant => element! {
                                View(key, padding_left: 2u32, padding_top: 1u32) {
                                    View(flex_direction: FlexDirection::Column) {
                                        Text(content: "\u{25B6} Assistant", color: theme.primary, weight: Weight::Bold)
                                        View(padding_left: 2u32, padding_top: 1u32) {
                                            Markdown(content: content, theme: theme)
                                        }
                                    }
                                }
                            },
                            MessageRole::System => element! {
                                View(key, padding_left: 2u32, padding_top: 1u32) {
                                    View(flex_direction: FlexDirection::Column) {
                                        Text(content: "\u{26A0} System", color: theme.warning, weight: Weight::Bold)
                                        View(padding_left: 4u32) {
                                            Text(content: content, color: theme.text_muted)
                                        }
                                    }
                                }
                            },
                            MessageRole::ToolCall => element! {
                                View(key, padding_left: 4u32) {
                                    Text(content: content, color: theme.accent)
                                }
                            },
                        }
                    }))

                    // Reasoning/thinking display
                    #(if !reasoning.is_empty() && streaming.is_empty() {
                        Some(element! {
                            View(padding_left: 4u32, padding_top: 1u32) {
                                Text(content: "\u{1F4AD} Thinking...", color: theme.accent, italic: true)
                                View(padding_left: 2u32, padding_top: 1u32) {
                                    Text(content: reasoning.clone(), color: theme.text_muted, italic: true)
                                }
                            }
                        })
                    } else {
                        None
                    })

                    // Streaming text
                    #(if !streaming.is_empty() {
                        Some(element! {
                            View(padding_left: 2u32, padding_top: 1u32) {
                                View(flex_direction: FlexDirection::Column) {
                                    View(flex_direction: FlexDirection::Row) {
                                        Text(content: "\u{25B6} Assistant", color: theme.primary, weight: Weight::Bold)
                                        Text(content: " ...", color: theme.text_muted)
                                    }
                                    View(padding_left: 2u32, padding_top: 1u32) {
                                        Markdown(content: streaming.clone(), theme: theme)
                                    }
                                }
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
