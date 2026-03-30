//! Sidebar component.

use iocraft::prelude::*;

use crate::theme::Theme;

#[derive(Default, Props)]
pub struct SidebarProps {
    pub session_title: String,
    pub model_name: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub session_cost: f64,
    pub context_pct: f64,
    pub is_plan: bool,
    pub theme: Option<Theme>,
}

#[component]
pub fn Sidebar(props: &SidebarProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme.unwrap_or_default();
    let title = if props.session_title.is_empty() {
        "New Session".to_string()
    } else {
        props.session_title.clone()
    };

    let in_tokens = format_tokens(props.input_tokens);
    let out_tokens = format_tokens(props.output_tokens);
    let version = format!("flok v{}", env!("CARGO_PKG_VERSION"));
    let has_cost = props.session_cost > 0.0;
    let cost_str = if props.session_cost < 0.01 {
        format!("${:.4}", props.session_cost)
    } else {
        format!("${:.2}", props.session_cost)
    };

    element! {
        View(
            width: 42u32,
            min_width: 42u32,
            max_width: 42u32,
            flex_shrink: 0.0,
            height: 100pct,
            background_color: theme.bg_panel,
            border_style: BorderStyle::Single,
            border_edges: Edges::Left,
            border_color: theme.border,
            flex_direction: FlexDirection::Column,
            padding_left: 2u32,
            padding_right: 1u32,
            padding_top: 1u32,
        ) {
            // Session title
            Text(content: title, color: theme.primary, weight: Weight::Bold)
            Text(content: props.model_name.clone(), color: theme.text_muted)

            // Context section
            View(padding_top: 1u32) {
                Text(content: "CONTEXT", color: theme.primary, weight: Weight::Bold)
            }
            View(flex_direction: FlexDirection::Row) {
                Text(content: "In  ", color: theme.text_muted)
                Text(content: in_tokens, color: theme.text)
            }
            View(flex_direction: FlexDirection::Row) {
                Text(content: "Out ", color: theme.text_muted)
                Text(content: out_tokens, color: theme.text)
            }
            #(if has_cost {
                Some(element! {
                    View(flex_direction: FlexDirection::Row) {
                        Text(content: "Cost ", color: theme.text_muted)
                        Text(content: cost_str, color: theme.text)
                    }
                })
            } else {
                None
            })
            #(if props.context_pct > 0.0 {
                let pct = props.context_pct.min(100.0);
                let ctx_color = if pct > 90.0 {
                    theme.error
                } else if pct > 70.0 {
                    theme.warning
                } else {
                    theme.success
                };
                Some(element! {
                    View(flex_direction: FlexDirection::Row) {
                        Text(content: "Ctx  ", color: theme.text_muted)
                        Text(content: format!("{pct:.0}%"), color: ctx_color, weight: Weight::Bold)
                    }
                })
            } else {
                None
            })

            // Tools section
            View(padding_top: 1u32) {
                Text(content: "TOOLS", color: theme.primary, weight: Weight::Bold)
            }
            #(["read", "write", "edit", "bash", "grep", "glob", "webfetch"].iter().map(|name| {
                element! {
                    View(key: *name, flex_direction: FlexDirection::Row) {
                        Text(content: "\u{25CF} ", color: theme.success)
                        Text(content: *name, color: theme.text)
                    }
                }
            }))

            // Version at bottom
            View(flex_grow: 1.0, justify_content: JustifyContent::End) {
                Text(content: version, color: theme.text_muted)
            }
        }
    }
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
