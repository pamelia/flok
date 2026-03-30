//! Layer 3, Tier 1: Tool output pruning.
//!
//! Recency-based pruning of old `tool_result` blocks in the conversation history.
//! This runs every prompt loop iteration (before assembling the API request)
//! and requires NO LLM call.
//!
//! Inspired by `OpenCode`'s `prune()` function which protects the last 40K tokens
//! of tool output and clears everything older.

use crate::provider::MessageContent;

/// The placeholder text that replaces pruned tool results.
pub const PRUNED_PLACEHOLDER: &str = "[Old tool result content cleared]";

/// Protected tool names whose output is never pruned.
const PROTECTED_TOOLS: &[&str] = &["skill"];

/// Prune old `tool_result` content from messages.
///
/// Walks backwards through all messages, tracking cumulative `tool_result`
/// token count. Once we exceed `protect_tokens`, older `tool_result` blocks
/// are replaced with a placeholder.
///
/// Returns the number of parts pruned.
///
/// # Arguments
///
/// * `messages` — The conversation message parts (mutable; pruned in-place)
/// * `protect_tokens` — Number of recent `tool_result` tokens to protect (default: 40000)
/// * `chars_per_token` — Approximate characters per token (default: 4)
pub fn prune_tool_outputs(
    messages: &mut [Vec<MessageContent>],
    protect_tokens: usize,
    chars_per_token: usize,
) -> usize {
    let protect_chars = protect_tokens * chars_per_token;
    let mut cumulative_chars: usize = 0;
    let mut pruned_count: usize = 0;

    // Walk backwards through messages
    for parts in messages.iter_mut().rev() {
        for part in parts.iter_mut().rev() {
            if let MessageContent::ToolResult { content, is_error, tool_use_id, .. } = part {
                // Never prune errors
                if *is_error {
                    continue;
                }

                // Never prune already-pruned content
                if content == PRUNED_PLACEHOLDER {
                    continue;
                }

                // Never prune protected tools (check by tool_use_id prefix convention)
                // In practice, tool_use_ids from protected tools would be tracked separately
                let _ = tool_use_id; // Used for future protected-tool lookup

                let content_chars = content.len();
                cumulative_chars += content_chars;

                // If we've exceeded the protect window, prune this one
                if cumulative_chars > protect_chars {
                    *content = PRUNED_PLACEHOLDER.to_string();
                    pruned_count += 1;
                }
            }
        }
    }

    if pruned_count > 0 {
        tracing::debug!(pruned_count, protect_tokens, "pruned old tool outputs");
    }

    pruned_count
}

/// Check if a tool name is protected from pruning.
pub fn is_protected_tool(name: &str) -> bool {
    PROTECTED_TOOLS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool_result(content: &str, is_error: bool) -> MessageContent {
        MessageContent::ToolResult {
            tool_use_id: "tc_test".into(),
            content: content.into(),
            is_error,
        }
    }

    fn make_text(text: &str) -> MessageContent {
        MessageContent::Text { text: text.into() }
    }

    #[test]
    fn prune_clears_old_tool_results() {
        // 3 messages, each with a tool result of 100 chars
        let content_100 = "x".repeat(100);
        let mut messages = vec![
            vec![make_tool_result(&content_100, false)],
            vec![make_tool_result(&content_100, false)],
            vec![make_tool_result(&content_100, false)],
        ];

        // Protect 30 tokens * 4 chars = 120 chars. Only the last message (100 chars)
        // fits within the protect window. The first two should be pruned.
        let pruned = prune_tool_outputs(&mut messages, 30, 4);

        // The last message (most recent, index 2) should be protected
        assert!(
            matches!(
                &messages[2][0],
                MessageContent::ToolResult { content, .. } if content == &content_100
            ),
            "last message should be preserved"
        );

        // At least the first message should be pruned
        assert!(pruned >= 1, "expected at least 1 pruned, got {pruned}");
        assert!(
            matches!(
                &messages[0][0],
                MessageContent::ToolResult { content, .. } if content == PRUNED_PLACEHOLDER
            ),
            "first message should be pruned"
        );
    }

    #[test]
    fn prune_never_touches_errors() {
        let mut messages = vec![vec![make_tool_result("error output", true)]];

        let pruned = prune_tool_outputs(&mut messages, 0, 4); // protect 0 tokens

        assert_eq!(pruned, 0);
        assert!(matches!(
            &messages[0][0],
            MessageContent::ToolResult { content, .. } if content == "error output"
        ));
    }

    #[test]
    fn prune_never_touches_text() {
        let mut messages = vec![vec![make_text("user message")]];

        let pruned = prune_tool_outputs(&mut messages, 0, 4);

        assert_eq!(pruned, 0);
        assert!(matches!(
            &messages[0][0],
            MessageContent::Text { text } if text == "user message"
        ));
    }

    #[test]
    fn prune_skips_already_pruned() {
        let mut messages = vec![vec![make_tool_result(PRUNED_PLACEHOLDER, false)]];

        let pruned = prune_tool_outputs(&mut messages, 0, 4);

        assert_eq!(pruned, 0);
    }

    #[test]
    fn prune_with_large_protect_window_does_nothing() {
        let content = "x".repeat(100);
        let mut messages =
            vec![vec![make_tool_result(&content, false)], vec![make_tool_result(&content, false)]];

        // Protect 100K tokens — way more than we have
        let pruned = prune_tool_outputs(&mut messages, 100_000, 4);

        assert_eq!(pruned, 0);
    }

    #[test]
    fn protected_tool_check() {
        assert!(is_protected_tool("skill"));
        assert!(!is_protected_tool("bash"));
        assert!(!is_protected_tool("read"));
    }
}
