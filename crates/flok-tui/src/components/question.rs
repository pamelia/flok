//! Question prompt component with tab support.
//!
//! Renders inline at the bottom of the session view, replacing the input box.
//! Supports multi-question flows with tabs and a confirm/review step.

use iocraft::prelude::*;

use crate::theme::Theme;

// Simple inline question prompt (single question, no tabs)
#[derive(Default, Props)]
pub struct QuestionPromptInlineProps {
    pub question: String,
    pub options: Vec<String>,
    pub selected: usize,
    pub theme: Option<Theme>,
}

#[component]
pub fn QuestionPromptInline(props: &QuestionPromptInlineProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme.unwrap_or_default();
    let selected = props.selected;

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
            border_color: theme.accent,
            background_color: theme.bg_panel,
            flex_direction: FlexDirection::Column,
            padding_left: 1u32,
            padding_right: 1u32,
        ) {
            // Question text
            View(padding_top: 1u32, padding_bottom: 1u32) {
                Text(content: props.question.clone(), color: theme.text, weight: Weight::Bold)
            }

            // Options
            #(props.options.iter().enumerate().map(|(i, opt)| {
                let is_sel = i == selected;
                let bg = if is_sel { theme.bg_element } else { theme.bg_panel };
                let fg = if is_sel { theme.secondary } else { theme.text };
                let marker = if is_sel { "> " } else { "  " };
                element! {
                    View(key: i, flex_direction: FlexDirection::Row, background_color: bg) {
                        Text(content: format!("{}{}.  ", marker, i + 1), color: theme.text_muted)
                        Text(content: opt.clone(), color: fg)
                    }
                }
            }))

            // Footer
            View(
                flex_direction: FlexDirection::Row,
                background_color: theme.bg_element,
                padding: 1u32,
            ) {
                Text(
                    content: "\u{2191}\u{2193} select  enter confirm  esc dismiss  1-9 quick select",
                    color: theme.text_muted,
                )
            }
        }
    }
}

/// A single question with its options.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct QuestionData {
    pub question: String,
    pub options: Vec<String>,
    pub selected: usize,
    pub answered: Option<String>,
    pub allow_custom: bool,
}

#[allow(dead_code)]
#[derive(Default, Props)]
pub struct QuestionPromptProps {
    pub questions: Vec<QuestionData>,
    pub active_tab: usize,
    pub theme: Option<Theme>,
}

#[component]
pub fn QuestionPrompt(props: &QuestionPromptProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme.unwrap_or_default();
    let questions = &props.questions;
    let active_tab = props.active_tab;
    let has_tabs = questions.len() > 1;

    let active_q = questions.get(active_tab);

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
            border_color: theme.accent,
            background_color: theme.bg_panel,
            flex_direction: FlexDirection::Column,
            padding_left: 1u32,
            padding_right: 1u32,
        ) {
            // Tab bar (if multiple questions)
            #(if has_tabs {
                Some(element! {
                    View(
                        flex_direction: FlexDirection::Row,
                        padding_top: 1u32,
                        padding_bottom: 1u32,
                    ) {
                        #(questions.iter().enumerate().map(|(i, q)| {
                            let is_active = i == active_tab;
                            let is_answered = q.answered.is_some();
                            let tab_bg = if is_active { theme.accent } else { theme.bg_panel };
                            let tab_fg = if is_active {
                                theme.bg
                            } else if is_answered {
                                theme.text
                            } else {
                                theme.text_muted
                            };
                            let label = if i == questions.len() - 1 && q.question == "Confirm" {
                                "Confirm".to_string()
                            } else {
                                format!("Q{}", i + 1)
                            };
                            element! {
                                View(key: i, background_color: tab_bg, padding_left: 1u32, padding_right: 1u32) {
                                    Text(content: label, color: tab_fg, weight: Weight::Bold)
                                }
                            }
                        }))
                    }
                })
            } else {
                None
            })

            // Question text
            #(if let Some(q) = active_q {
                let question_text = q.question.clone();
                let options = q.options.clone();
                let selected = q.selected;

                Some(element! {
                    View(flex_direction: FlexDirection::Column, padding_top: 1u32) {
                        Text(content: question_text, color: theme.text, weight: Weight::Bold)
                        View(padding_top: 1u32, flex_direction: FlexDirection::Column) {
                            #(options.iter().enumerate().map(|(i, opt)| {
                                let is_selected = i == selected;
                                let bg = if is_selected { theme.bg_element } else { theme.bg_panel };
                                let fg = if is_selected { theme.secondary } else { theme.text };
                                let marker = if is_selected { "> " } else { "  " };
                                element! {
                                    View(key: i, flex_direction: FlexDirection::Row, background_color: bg) {
                                        Text(content: format!("{}{}.  ", marker, i + 1), color: theme.text_muted)
                                        Text(content: opt.clone(), color: fg)
                                    }
                                }
                            }))
                        }
                    }
                })
            } else {
                None
            })

            // Footer with key hints
            View(
                flex_direction: FlexDirection::Row,
                background_color: theme.bg_element,
                padding_top: 1u32,
                padding_bottom: 1u32,
                padding_left: 1u32,
            ) {
                #(if has_tabs {
                    Some(element! {
                        Text(content: "\u{21C6} tab  ", color: theme.text_muted)
                    })
                } else {
                    None
                })
                Text(content: "\u{2191}\u{2193} select  enter confirm  esc dismiss", color: theme.text_muted)
            }
        }
    }
}
