use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) mod render;

static NEXT_ACTIVE_ITEM_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Role {
    #[expect(
        dead_code,
        reason = "active transcript items are assistant/tool-only in the current cutover"
    )]
    User,
    Assistant,
    #[expect(
        dead_code,
        reason = "active transcript items are assistant/tool-only in the current cutover"
    )]
    System,
    ToolCall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SystemLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TeamEventKind {
    Created,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Hash)]
pub(crate) enum HistoryItem {
    User { text: String },
    Assistant { text: String, markdown: bool },
    System { text: String, level: SystemLevel },
    ToolCall { name: String, preview: String, is_error: bool, duration_ms: Option<u64> },
    TeamEvent { kind: TeamEventKind, agent: String, detail: String },
    ProviderFallback { from: String, to: String, reason: String },
    Divider,
}

impl HistoryItem {
    pub(crate) fn user(text: impl Into<String>) -> Self {
        Self::User { text: text.into() }
    }

    pub(crate) fn assistant(text: impl Into<String>, markdown: bool) -> Self {
        Self::Assistant { text: text.into(), markdown }
    }

    pub(crate) fn system_info(text: impl Into<String>) -> Self {
        Self::System { text: text.into(), level: SystemLevel::Info }
    }

    pub(crate) fn system_warn(text: impl Into<String>) -> Self {
        Self::System { text: text.into(), level: SystemLevel::Warn }
    }

    pub(crate) fn system_error(text: impl Into<String>) -> Self {
        Self::System { text: text.into(), level: SystemLevel::Error }
    }

    pub(crate) fn provider_fallback(
        from: impl Into<String>,
        to: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self::ProviderFallback { from: from.into(), to: to.into(), reason: reason.into() }
    }

    #[expect(dead_code, reason = "reserved transcript separator for later UX polish")]
    pub(crate) fn divider() -> Self {
        Self::Divider
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveItem {
    pub(crate) id: u64,
    pub(crate) role: Role,
    pub(crate) streaming_text: String,
    pub(crate) tool_name: Option<String>,
    pub(crate) revision: u64,
}

impl ActiveItem {
    pub(crate) fn new_assistant() -> Self {
        Self {
            id: next_active_item_id(),
            role: Role::Assistant,
            streaming_text: String::new(),
            tool_name: None,
            revision: 0,
        }
    }

    pub(crate) fn new_tool(name: String) -> Self {
        Self {
            id: next_active_item_id(),
            role: Role::ToolCall,
            streaming_text: String::new(),
            tool_name: Some(name),
            revision: 0,
        }
    }

    pub(crate) fn append(&mut self, delta: &str) {
        self.streaming_text.push_str(delta);
        self.revision = self.revision.wrapping_add(1);
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "helper reserved for future active-item finalization paths")
    )]
    pub(crate) fn into_final(self) -> HistoryItem {
        match self.role {
            Role::Assistant => HistoryItem::Assistant { text: self.streaming_text, markdown: true },
            Role::ToolCall => HistoryItem::ToolCall {
                name: self.tool_name.unwrap_or_default(),
                preview: self.streaming_text,
                is_error: false,
                duration_ms: None,
            },
            Role::User => HistoryItem::User { text: self.streaming_text },
            Role::System => {
                HistoryItem::System { text: self.streaming_text, level: SystemLevel::Info }
            }
        }
    }
}

fn next_active_item_id() -> u64 {
    NEXT_ACTIVE_ITEM_ID.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_item_constructors() {
        match HistoryItem::user("hi") {
            HistoryItem::User { text } => assert_eq!(text, "hi"),
            _ => panic!("wrong variant"),
        }
        match HistoryItem::system_error("boom") {
            HistoryItem::System { text, level } => {
                assert_eq!(text, "boom");
                assert_eq!(level, SystemLevel::Error);
            }
            _ => panic!("wrong variant"),
        }
        match HistoryItem::provider_fallback("anthropic", "openai", "HTTP 529") {
            HistoryItem::ProviderFallback { from, to, reason } => {
                assert_eq!(from, "anthropic");
                assert_eq!(to, "openai");
                assert_eq!(reason, "HTTP 529");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn active_item_append_bumps_revision() {
        let mut item = ActiveItem::new_assistant();
        assert_eq!(item.revision, 0);
        item.append("hello");
        assert_eq!(item.streaming_text, "hello");
        assert_eq!(item.revision, 1);
        item.append(" world");
        assert_eq!(item.streaming_text, "hello world");
        assert_eq!(item.revision, 2);
    }

    #[test]
    fn active_item_into_final_assistant() {
        let mut item = ActiveItem::new_assistant();
        item.append("response text");
        match item.into_final() {
            HistoryItem::Assistant { text, markdown } => {
                assert_eq!(text, "response text");
                assert!(markdown);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn active_item_into_final_tool() {
        let mut item = ActiveItem::new_tool("bash".to_string());
        item.append("ls -la");
        match item.into_final() {
            HistoryItem::ToolCall { name, preview, is_error, duration_ms } => {
                assert_eq!(name, "bash");
                assert_eq!(preview, "ls -la");
                assert!(!is_error);
                assert!(duration_ms.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }
}
