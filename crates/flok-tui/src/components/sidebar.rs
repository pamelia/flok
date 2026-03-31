//! Sidebar component.
//!
//! Uses a thin separator column instead of a box border so that the sidebar
//! is visually distinct without breaking terminal text-selection UX.

use iocraft::prelude::*;

use crate::theme::Theme;

/// A team member's status for display in the sidebar.
#[derive(Debug, Clone, Default)]
pub struct TeamMemberInfo {
    /// Agent name (e.g., "feasibility-reviewer-01HRK3...")
    pub name: String,
    /// Current status.
    pub status: TeamMemberStatus,
}

impl TeamMemberInfo {
    /// Human-readable display name — strips the ULID suffix.
    ///
    /// `"feasibility-reviewer-01HRKAB3"` -> `"feasibility-reviewer"`
    pub fn display_name(&self) -> &str {
        if let Some(pos) = self.name.rfind('-') {
            let suffix = &self.name[pos + 1..];
            if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_alphanumeric()) {
                return &self.name[..pos];
            }
        }
        &self.name
    }

    /// Human-readable status word.
    pub fn status_word(&self) -> &'static str {
        match self.status {
            TeamMemberStatus::Running => "working",
            TeamMemberStatus::Completed => "done",
            TeamMemberStatus::Failed => "failed",
        }
    }
}

/// Status of a team member.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TeamMemberStatus {
    /// Agent is running.
    #[default]
    Running,
    /// Agent completed successfully.
    Completed,
    /// Agent failed.
    Failed,
}

#[derive(Default, Props)]
pub struct SidebarProps {
    pub session_title: String,
    pub model_name: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub session_cost: f64,
    pub context_pct: f64,
    pub is_plan: bool,
    /// Active team name (e.g., "code-review-pr-42").
    pub team_name: String,
    /// Team member statuses.
    pub team_members: Vec<TeamMemberInfo>,
    pub theme: Option<Theme>,
    /// Handle for imperative scroll control from the parent.
    pub scroll_handle: Option<Ref<ScrollViewHandle>>,
}

#[component]
pub fn Sidebar(mut hooks: Hooks, props: &mut SidebarProps) -> impl Into<AnyElement<'static>> {
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

    // Team summary
    let active_count =
        props.team_members.iter().filter(|m| m.status == TeamMemberStatus::Running).count();
    let total_count = props.team_members.len();

    // Internal handle if parent doesn't provide one.
    let internal_handle = hooks.use_ref(ScrollViewHandle::default);
    let handle = props.scroll_handle.unwrap_or(internal_handle);

    element! {
        View(
            flex_direction: FlexDirection::Row,
            height: 100pct,
        ) {
            // Thin separator — a single column of space instead of a box border.
            View(width: 1u32, height: 100pct) {
                Text(content: " ", color: theme.border)
            }

            // Sidebar content
            View(
                width: 38u32,
                flex_shrink: 0.0,
                height: 100pct,
                flex_direction: FlexDirection::Column,
                padding_left: 2u32,
                padding_right: 1u32,
            ) {
                // Scrollable body
                ScrollView(
                    auto_scroll: false,
                    keyboard_scroll: Some(false),
                    scroll_step: Some(0u16),
                    handle: handle,
                    scrollbar: Some(false),
                ) {
                    View(flex_direction: FlexDirection::Column, padding_top: 1u32) {
                        // Session title
                        Text(content: title, color: theme.primary, weight: Weight::Bold)
                        Text(content: props.model_name.clone(), color: theme.text_muted)

                        // Context
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

                        // Agents section
                        #(if props.team_name.is_empty() {
                            None
                        } else {
                            let team_name = props.team_name.clone();
                            let members = props.team_members.clone();
                            let summary = if active_count > 0 {
                                format!("{active_count} active, {total_count} total")
                            } else {
                                format!("{total_count} agents")
                            };
                            Some(element! {
                                View(padding_top: 1u32) {
                                    // Header
                                    Text(
                                        content: format!("AGENTS ({summary})"),
                                        color: theme.warning,
                                        weight: Weight::Bold,
                                    )
                                    // Team name on its own line
                                    Text(content: team_name, color: theme.text_muted)
                                    // Each agent on its own row
                                    #(members.iter().map(|m| {
                                        let (icon, color) = match m.status {
                                            TeamMemberStatus::Running => ("~", theme.warning),
                                            TeamMemberStatus::Completed => ("*", theme.success),
                                            TeamMemberStatus::Failed => ("x", theme.error),
                                        };
                                        let name = m.display_name();
                                        let status = m.status_word();
                                        element! {
                                            View(key: m.name.clone()) {
                                                Text(
                                                    content: format!("  {icon} @{name} {status}"),
                                                    color: color,
                                                )
                                            }
                                        }
                                    }))
                                }
                            })
                        })
                    }
                }

                // Version pinned to bottom (outside scroll)
                View(flex_shrink: 0.0) {
                    Text(content: version, color: theme.text_muted)
                }
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
