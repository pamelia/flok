//! Branch engine — creates session branches and generates summaries.
//!
//! A branch is a new session that copies messages up to a branch point from
//! a parent session, then injects an LLM-generated summary of the abandoned
//! tail as a synthetic user message.

use std::fmt::Write;
use std::sync::Arc;

use tokio::sync::mpsc;
use ulid::Ulid;

use crate::bus::BusEvent;
use crate::provider::{CompletionRequest, Message, MessageContent, Provider, StreamEvent};
use crate::session::state::AppState;

/// The result of a branch operation.
#[derive(Debug)]
pub struct BranchResult {
    /// The newly created session ID.
    pub session_id: String,
    /// Number of messages copied from the parent.
    pub messages_copied: usize,
    /// Whether a summary was generated (false if no messages after the branch point).
    pub summary_generated: bool,
}

/// Create a branch session from an existing session at a specific message.
///
/// This is the top-level branch operation that:
/// 1. Validates the branch point
/// 2. Creates a new session with parent linkage
/// 3. Copies messages up to the branch point
/// 4. Generates and injects a summary of the abandoned tail
///
/// # Errors
///
/// Returns an error if the branch point message doesn't exist, the session
/// is a sub-agent session, or any DB/provider operation fails.
pub async fn create_branch(
    state: &AppState,
    source_session_id: &str,
    from_message_id: &str,
    snapshot_hash: Option<String>,
) -> anyhow::Result<BranchResult> {
    // 1. Validate source session exists and is not a sub-agent session
    let source_session = state.db.get_session(source_session_id)?;

    // Check if this is a sub-agent session (has parent_id but no branch_from_message_id,
    // meaning it was created by the team system, not by user branching)
    // Sub-agent sessions have parent_id set but are managed by team infrastructure.
    // User branches also have parent_id, but they additionally have branch_from_message_id.
    // Root sessions (parent_id=None) are always branchable.
    // We allow branching from existing branch sessions (they have branch_from_message_id).
    // We reject branching from sub-agent sessions (parent_id set, no branch_from_message_id).
    if source_session.parent_id.is_some() && source_session.branch_from_message_id.is_none() {
        anyhow::bail!(
            "Cannot branch from a sub-agent session. \
             Branch from the parent session instead."
        );
    }

    // 2. Validate the branch point message exists in the source session
    let _branch_msg = state.db.get_message(from_message_id)?;

    // 3. Create the new session
    let new_session_id = Ulid::new().to_string();
    let title = if source_session.title.is_empty() {
        "(branch)".to_string()
    } else {
        format!("{} (branch)", source_session.title)
    };

    state.db.create_branch_session(
        &new_session_id,
        &source_session.project_id,
        source_session_id,
        &source_session.model_id,
        &title,
        from_message_id,
        snapshot_hash.as_deref(),
    )?;

    // 4. Copy messages up to the branch point
    let messages_copied = state.db.copy_messages_to_session(
        source_session_id,
        &new_session_id,
        from_message_id,
        &|| Ulid::new().to_string(),
    )?;

    tracing::info!(
        source = %source_session_id,
        branch = %new_session_id,
        from_message = %from_message_id,
        messages_copied,
        "branch session created"
    );

    // 5. Generate summary of the abandoned tail (async, non-blocking on failure)
    let tail_messages = state.db.list_messages_after(source_session_id, from_message_id)?;
    let summary_generated = if tail_messages.is_empty() {
        false
    } else {
        match generate_branch_summary(&state.provider, &source_session.model_id, &tail_messages)
            .await
        {
            Ok(summary) => {
                inject_branch_summary(state, &new_session_id, &summary)?;
                true
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "branch summary generation failed, injecting fallback"
                );
                // Fallback: inject a minimal summary listing modified files
                let fallback = build_fallback_summary(&tail_messages);
                inject_branch_summary(state, &new_session_id, &fallback)?;
                true
            }
        }
    };

    // 6. Emit bus event
    state.bus.send(BusEvent::SessionBranched {
        parent_session_id: source_session_id.to_string(),
        new_session_id: new_session_id.clone(),
        from_message_id: from_message_id.to_string(),
    });

    Ok(BranchResult { session_id: new_session_id, messages_copied, summary_generated })
}

/// Generate an LLM summary of the abandoned branch tail.
async fn generate_branch_summary(
    provider: &Arc<dyn Provider>,
    model_id: &str,
    tail_messages: &[flok_db::MessageRow],
) -> anyhow::Result<String> {
    // Serialize the tail messages into a readable format for the LLM
    let mut conversation = String::new();
    for msg in tail_messages {
        let parts: Vec<MessageContent> = serde_json::from_str(&msg.parts).unwrap_or_default();
        let _ = writeln!(conversation, "[{role}]", role = msg.role);
        for part in &parts {
            match part {
                MessageContent::Text { text } => {
                    let _ = writeln!(conversation, "{text}");
                }
                MessageContent::Thinking { thinking } => {
                    let preview = &thinking[..thinking.len().min(200)];
                    let _ = writeln!(conversation, "(thinking: {preview}...)");
                }
                MessageContent::ToolUse { name, input, .. } => {
                    let args_preview = input.to_string();
                    let args_preview = &args_preview[..args_preview.len().min(200)];
                    let _ = writeln!(conversation, "Tool call: {name}({args_preview})");
                }
                MessageContent::ToolResult { content, is_error, .. } => {
                    let preview = &content[..content.len().min(500)];
                    let error_marker = if *is_error { " [ERROR]" } else { "" };
                    let _ = writeln!(conversation, "Tool result{error_marker}: {preview}");
                }
            }
        }
        let _ = writeln!(conversation);
    }

    // Apply a rough token budget: keep at most ~40K tokens worth (~160K chars)
    let conversation = if conversation.len() > 160_000 {
        let truncated = &conversation[conversation.len() - 160_000..];
        format!("[...earlier messages truncated...]\n{truncated}")
    } else {
        conversation
    };

    let system = "You are a conversation summarizer. Your job is to produce a concise, \
                  actionable summary of an abandoned conversation branch so that an AI \
                  coding assistant can learn from what was tried and avoid repeating mistakes."
        .to_string();

    let prompt = format!(
        "The user is branching this conversation to explore a different approach.\n\
         Summarize what happened in the abandoned branch so the assistant can learn from it.\n\n\
         <conversation>\n{conversation}</conversation>\n\n\
         Produce a structured summary with:\n\
         1. **Goal**: What was the user trying to achieve?\n\
         2. **Approach**: What approach was taken?\n\
         3. **Outcome**: What happened? Did it succeed or fail? Why?\n\
         4. **Key Decisions**: Any important decisions or discoveries.\n\
         5. **Files Modified**: List of files that were changed.\n\
         6. **Lessons**: What should be avoided or considered in a new approach?\n\n\
         Keep the summary concise (< 500 words). Focus on actionable information \
         that helps the assistant take a different approach."
    );

    let request = CompletionRequest {
        model: model_id.to_string(),
        system,
        messages: vec![Message {
            role: "user".into(),
            content: vec![MessageContent::Text { text: prompt }],
        }],
        tools: Vec::new(),
        max_tokens: 2048,
    };

    // Stream and collect the response
    let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
    let provider = Arc::clone(provider);
    tokio::spawn(async move {
        if let Err(e) = provider.stream(request, tx).await {
            tracing::error!("branch summary stream error: {e}");
        }
    });

    let timeout = std::time::Duration::from_secs(30);
    let mut text = String::new();

    loop {
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(StreamEvent::TextDelta(delta))) => text.push_str(&delta),
            Ok(Some(StreamEvent::Done) | None) => break,
            Ok(Some(StreamEvent::Error(e))) => {
                anyhow::bail!("summary generation error: {e}");
            }
            Ok(Some(_)) => {} // Ignore Usage, ReasoningDelta, etc.
            Err(_) => {
                anyhow::bail!("summary generation timed out after {timeout:?}");
            }
        }
    }

    if text.is_empty() {
        anyhow::bail!("summary generation returned empty response");
    }

    Ok(text)
}

/// Inject the branch summary as a synthetic user message in the new session.
fn inject_branch_summary(state: &AppState, session_id: &str, summary: &str) -> anyhow::Result<()> {
    let msg_id = Ulid::new().to_string();
    let text = format!(
        "[Branch Context] The following summarizes a previous exploration path \
         that was abandoned. Use this to avoid repeating failed approaches.\n\n\
         <branch-summary>\n{summary}\n</branch-summary>"
    );

    let parts = serde_json::to_string(&[MessageContent::Text { text }])?;
    state.db.insert_message(&msg_id, session_id, "user", &parts)?;

    Ok(())
}

/// Build a fallback summary when LLM summarization fails.
///
/// Extracts file paths from tool calls to give the agent minimal context.
fn build_fallback_summary(tail_messages: &[flok_db::MessageRow]) -> String {
    let mut files_modified = Vec::new();

    for msg in tail_messages {
        let parts: Vec<MessageContent> = serde_json::from_str(&msg.parts).unwrap_or_default();
        for part in &parts {
            if let MessageContent::ToolUse { name, input, .. } = part {
                // Extract file paths from common tool calls
                if matches!(name.as_str(), "edit" | "write" | "bash") {
                    if let Some(path) = input.get("filePath").or(input.get("file_path")) {
                        if let Some(path_str) = path.as_str() {
                            if !files_modified.contains(&path_str.to_string()) {
                                files_modified.push(path_str.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    let mut summary = format!(
        "[Summary generation failed. Prior branch explored {} message(s)",
        tail_messages.len()
    );

    if !files_modified.is_empty() {
        let _ = write!(summary, " and modified: {}", files_modified.join(", "));
    }

    summary.push_str(".]");
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_summary_extracts_file_paths() {
        let parts = serde_json::to_string(&[MessageContent::ToolUse {
            id: "tc1".into(),
            name: "edit".into(),
            input: serde_json::json!({"filePath": "/src/main.rs", "oldString": "a", "newString": "b"}),
        }])
        .unwrap();

        let messages = vec![flok_db::MessageRow {
            id: "m1".into(),
            session_id: "s1".into(),
            role: "assistant".into(),
            parts,
            created_at: "2026-01-01".into(),
        }];

        let summary = build_fallback_summary(&messages);
        assert!(summary.contains("/src/main.rs"));
        assert!(summary.contains("1 message(s)"));
    }

    #[test]
    fn fallback_summary_deduplicates_files() {
        let parts = serde_json::to_string(&[
            MessageContent::ToolUse {
                id: "tc1".into(),
                name: "edit".into(),
                input: serde_json::json!({"filePath": "/src/main.rs"}),
            },
            MessageContent::ToolUse {
                id: "tc2".into(),
                name: "edit".into(),
                input: serde_json::json!({"filePath": "/src/main.rs"}),
            },
        ])
        .unwrap();

        let messages = vec![flok_db::MessageRow {
            id: "m1".into(),
            session_id: "s1".into(),
            role: "assistant".into(),
            parts,
            created_at: "2026-01-01".into(),
        }];

        let summary = build_fallback_summary(&messages);
        // Should only appear once
        assert_eq!(summary.matches("/src/main.rs").count(), 1);
    }
}
