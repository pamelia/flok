//! Dialog overlay system.
//!
//! Centered panel over a semi-transparent backdrop. Supports:
//! - `DialogSelect`: fuzzy search list picker
//! - Esc to dismiss

use iocraft::prelude::*;

use crate::theme::Theme;

/// An option in a `DialogSelect`.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SelectOption {
    pub title: String,
    pub description: String,
    pub category: String,
    pub footer: String,
    pub value: String,
}

/// Props for the select dialog overlay.
#[allow(dead_code)]
#[derive(Default, Props)]
pub struct DialogSelectProps {
    pub title: String,
    pub options: Vec<SelectOption>,
    pub selected: usize,
    pub filter: String,
    pub visible: bool,
    pub theme: Option<Theme>,
}

#[component]
pub fn DialogSelect(props: &DialogSelectProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme.unwrap_or_default();

    if !props.visible {
        return element! { View() };
    }

    // Filter options by search text
    let filter_lower = props.filter.to_lowercase();
    let filtered: Vec<(usize, &SelectOption)> = props
        .options
        .iter()
        .enumerate()
        .filter(|(_, opt)| {
            filter_lower.is_empty()
                || opt.title.to_lowercase().contains(&filter_lower)
                || opt.category.to_lowercase().contains(&filter_lower)
        })
        .collect();

    let selected = props.selected;

    // Group by category
    let current_category = String::new();

    element! {
        // Full-screen backdrop
        View(
            position: Position::Absolute,
            top: 0i32,
            left: 0i32,
            width: 100pct,
            height: 100pct,
            align_items: AlignItems::Center,
            padding_top: 4u32,
        ) {
            // Dialog panel
            View(
                width: 60u32,
                max_height: 20u32,
                background_color: theme.bg_panel,
                border_style: BorderStyle::Round,
                border_color: theme.border,
                flex_direction: FlexDirection::Column,
                padding: 1u32,
                overflow: Overflow::Hidden,
            ) {
                // Title bar
                View(flex_direction: FlexDirection::Row, padding_bottom: 1u32) {
                    Text(content: props.title.clone(), color: theme.text, weight: Weight::Bold)
                    View(flex_grow: 1.0) {}
                    Text(content: "esc", color: theme.text_muted)
                }

                // Search input
                View(
                    border_style: BorderStyle::Round,
                    border_color: theme.border,
                    padding_left: 1u32,
                    padding_right: 1u32,
                ) {
                    Text(
                        content: if props.filter.is_empty() {
                            "Search...".to_string()
                        } else {
                            props.filter.clone()
                        },
                        color: if props.filter.is_empty() {
                            theme.text_muted
                        } else {
                            theme.text
                        },
                    )
                }

                // Options list
                ScrollView() {
                    View(flex_direction: FlexDirection::Column, width: 100pct) {
                        #(filtered.iter().enumerate().map(|(visual_idx, (_, opt))| {
                            let is_selected = visual_idx == selected;
                            let bg = if is_selected { theme.primary } else { theme.bg_panel };
                            let fg = if is_selected { theme.bg } else { theme.text };
                            let show_cat = opt.category != current_category;
                            // Note: can't mutate current_category here in a clean way
                            // For now, show category on every item if it has one

                            element! {
                                View(key: visual_idx, flex_direction: FlexDirection::Column) {
                                    #(if show_cat && !opt.category.is_empty() {
                                        Some(element! {
                                            View(padding_top: 1u32) {
                                                Text(
                                                    content: opt.category.clone(),
                                                    color: theme.text_muted,
                                                    weight: Weight::Bold,
                                                )
                                            }
                                        })
                                    } else {
                                        None
                                    })
                                    View(
                                        flex_direction: FlexDirection::Row,
                                        background_color: bg,
                                        padding_left: 1u32,
                                    ) {
                                        Text(content: opt.title.clone(), color: fg, weight: Weight::Bold)
                                        #(if opt.description.is_empty() {
                                            None
                                        } else {
                                            Some(element! {
                                                Text(
                                                    content: format!("  {}", opt.description),
                                                    color: if is_selected { theme.bg } else { theme.text_muted },
                                                )
                                            })
                                        })
                                        View(flex_grow: 1.0) {}
                                        #(if opt.footer.is_empty() {
                                            None
                                        } else {
                                            Some(element! {
                                                Text(
                                                    content: opt.footer.clone(),
                                                    color: if is_selected { theme.bg } else { theme.text_muted },
                                                )
                                            })
                                        })
                                    }
                                }
                            }
                        }))
                    }
                }
            }
        }
    }
}
