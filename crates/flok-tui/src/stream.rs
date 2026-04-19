use crate::history::{ActiveItem, HistoryItem, Role};

/// Ingest an assistant text delta. Returns true if a redraw is needed.
/// Ensures there is an active assistant item; appends the delta.
pub(crate) fn ingest_assistant_delta(active: &mut Option<ActiveItem>, delta: &str) -> bool {
    if delta.is_empty() {
        return false;
    }
    let item = active.get_or_insert_with(ActiveItem::new_assistant);
    match item.role {
        Role::Assistant => {
            item.append(delta);
            true
        }
        _ => {
            // Tool-call active item or other: finalize caller handles that; ignore delta to be safe.
            false
        }
    }
}

/// Finalize an active assistant item. Consumes active, returns `HistoryItem`.
pub(crate) fn finalize_assistant(
    active: Option<ActiveItem>,
    fallback_text: String,
    cancelled: bool,
) -> HistoryItem {
    match active {
        Some(item) if matches!(item.role, Role::Assistant) => {
            let text =
                if item.streaming_text.is_empty() { fallback_text } else { item.streaming_text };
            let final_text = if cancelled && !text.is_empty() {
                format!("{text}\n\n[cancelled]")
            } else if cancelled {
                "[cancelled]".to_string()
            } else {
                text
            };
            HistoryItem::Assistant { text: final_text, markdown: true }
        }
        _ => {
            let final_text =
                if cancelled { format!("{fallback_text}\n\n[cancelled]") } else { fallback_text };
            HistoryItem::Assistant { text: final_text, markdown: true }
        }
    }
}

/// Ingest a reasoning delta (separate from assistant). For MVP we discard reasoning (don't display).
pub(crate) fn ingest_reasoning_delta(_active: &mut Option<ActiveItem>, _delta: &str) -> bool {
    false
}

/// Start a tool-call active item (for streaming tool output).
pub(crate) fn begin_tool_call(active: &mut Option<ActiveItem>, tool_name: String) -> bool {
    *active = Some(ActiveItem::new_tool(tool_name));
    true
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "tool output streaming is not yet surfaced on the bus")
)]
/// Append tool output to an active tool call.
pub(crate) fn ingest_tool_output(active: &mut Option<ActiveItem>, delta: &str) -> bool {
    if let Some(item) = active.as_mut() {
        if matches!(item.role, Role::ToolCall) {
            item.append(delta);
            return true;
        }
    }
    false
}

/// Finalize an active tool call. Returns `HistoryItem::ToolCall`.
pub(crate) fn finalize_tool_call(
    active: Option<ActiveItem>,
    is_error: bool,
    duration_ms: Option<u64>,
) -> HistoryItem {
    match active {
        Some(item) if matches!(item.role, Role::ToolCall) => HistoryItem::ToolCall {
            name: item.tool_name.unwrap_or_default(),
            preview: item.streaming_text,
            is_error,
            duration_ms,
        },
        _ => HistoryItem::ToolCall {
            name: String::new(),
            preview: String::new(),
            is_error,
            duration_ms,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_creates_assistant_item_when_none() {
        let mut active: Option<ActiveItem> = None;
        let changed = ingest_assistant_delta(&mut active, "hello");
        assert!(changed);
        let item = active.expect("active item should be created");
        assert!(matches!(item.role, Role::Assistant));
        assert_eq!(item.streaming_text, "hello");
    }

    #[test]
    fn ingest_appends_to_existing_assistant() {
        let mut active = Some(ActiveItem::new_assistant());
        assert!(ingest_assistant_delta(&mut active, "foo "));
        assert!(ingest_assistant_delta(&mut active, "bar"));
        let item = active.expect("active item present");
        assert_eq!(item.streaming_text, "foo bar");
        assert_eq!(item.revision, 2);
    }

    #[test]
    fn ingest_empty_delta_returns_false() {
        let mut active: Option<ActiveItem> = None;
        assert!(!ingest_assistant_delta(&mut active, ""));
        assert!(active.is_none());

        let mut active2 = Some(ActiveItem::new_assistant());
        assert!(!ingest_assistant_delta(&mut active2, ""));
        let item = active2.expect("still present");
        assert_eq!(item.streaming_text, "");
        assert_eq!(item.revision, 0);
    }

    #[test]
    fn finalize_returns_assistant_with_text() {
        let mut active = Some(ActiveItem::new_assistant());
        ingest_assistant_delta(&mut active, "streamed reply");
        let item = finalize_assistant(active, String::new(), false);
        match item {
            HistoryItem::Assistant { text, markdown } => {
                assert_eq!(text, "streamed reply");
                assert!(markdown);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn finalize_uses_fallback_when_streaming_empty() {
        let active = Some(ActiveItem::new_assistant());
        let item = finalize_assistant(active, "fallback".to_string(), false);
        match item {
            HistoryItem::Assistant { text, markdown: _ } => assert_eq!(text, "fallback"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn finalize_cancelled_appends_marker() {
        let mut active = Some(ActiveItem::new_assistant());
        ingest_assistant_delta(&mut active, "partial");
        let item = finalize_assistant(active, String::new(), true);
        match item {
            HistoryItem::Assistant { text, markdown: _ } => {
                assert_eq!(text, "partial\n\n[cancelled]");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn finalize_cancelled_empty_yields_marker_only() {
        let active = Some(ActiveItem::new_assistant());
        let item = finalize_assistant(active, String::new(), true);
        match item {
            HistoryItem::Assistant { text, markdown: _ } => assert_eq!(text, "[cancelled]"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn reasoning_delta_is_discarded() {
        let mut active: Option<ActiveItem> = None;
        assert!(!ingest_reasoning_delta(&mut active, "thinking..."));
        assert!(active.is_none());
    }

    #[test]
    fn tool_call_flow_begin_ingest_finalize() {
        let mut active: Option<ActiveItem> = None;
        assert!(begin_tool_call(&mut active, "bash".to_string()));
        assert!(ingest_tool_output(&mut active, "ls "));
        assert!(ingest_tool_output(&mut active, "-la"));
        let item = finalize_tool_call(active, false, Some(42));
        match item {
            HistoryItem::ToolCall { name, preview, is_error, duration_ms } => {
                assert_eq!(name, "bash");
                assert_eq!(preview, "ls -la");
                assert!(!is_error);
                assert_eq!(duration_ms, Some(42));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn ingest_tool_output_ignored_for_assistant_active() {
        let mut active = Some(ActiveItem::new_assistant());
        assert!(!ingest_tool_output(&mut active, "should be ignored"));
        let item = active.expect("assistant active stays");
        assert_eq!(item.streaming_text, "");
    }

    #[test]
    fn ingest_assistant_delta_ignored_for_tool_active() {
        let mut active = Some(ActiveItem::new_tool("bash".to_string()));
        assert!(!ingest_assistant_delta(&mut active, "text"));
        let item = active.expect("tool active stays");
        assert!(matches!(item.role, Role::ToolCall));
        assert_eq!(item.streaming_text, "");
    }

    #[test]
    fn finalize_tool_call_returns_tool_call_variant() {
        // No active item at all -> still returns a ToolCall variant with empty fields.
        let item = finalize_tool_call(None, true, None);
        match item {
            HistoryItem::ToolCall { name, preview, is_error, duration_ms } => {
                assert_eq!(name, "");
                assert_eq!(preview, "");
                assert!(is_error);
                assert!(duration_ms.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }
}
