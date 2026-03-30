//! The session engine — manages the prompt loop for a single conversation.
//!
//! The loop:
//! 1. Assemble messages (system prompt + conversation history)
//! 2. Send to provider, stream response
//! 3. Accumulate text and tool calls from the stream
//! 4. If tool calls: execute them, append results, go to step 1
//! 5. If text only: done — return the assistant's response

use std::fmt::Write;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

use std::collections::HashMap;

use crate::bus::BusEvent;
use crate::provider::{CompletionRequest, Message, MessageContent, ModelRegistry, StreamEvent};
use crate::session::state::AppState;
use crate::token::TokenCounter;

/// Maximum number of tool-call rounds before we stop (doom loop protection).
const MAX_TOOL_ROUNDS: usize = 25;

/// Maximum identical tool calls before pausing (doom loop by repetition).
const MAX_IDENTICAL_CALLS: usize = 3;

/// Build the system prompt with project context.
///
/// The prompt includes:
/// - Base instructions for the coding agent
/// - Project root and current working directory
/// - AGENTS.md content if it exists in the project root
fn build_system_prompt(project_root: &std::path::Path) -> String {
    let mut prompt = String::from(
        r"You are flok, an expert AI coding agent for the terminal.

You are an interactive CLI tool that helps users with software engineering tasks. Use the instructions below and the tools available to you to assist the user.

# Tone and style
- Only use emojis if the user explicitly requests it. Avoid using emojis in all communication unless asked.
- Your output will be displayed on a command line interface. Your responses should be short and concise. You can use GitHub-flavored markdown for formatting.
- Output text to communicate with the user; all text you output outside of tool use is displayed to the user. Only use tools to complete tasks.
- NEVER create files unless they're absolutely necessary for achieving your goal. ALWAYS prefer editing an existing file to creating a new one.

# Professional objectivity
Prioritize technical accuracy and truthfulness over validating the user's beliefs. Focus on facts and problem-solving, providing direct, objective technical info without any unnecessary superlatives, praise, or emotional validation.

# Task Management
You have access to the TodoWrite tool to help you manage and plan tasks. Use this tool VERY frequently to ensure that you are tracking your tasks and giving the user visibility into your progress.
These tools are also EXTREMELY helpful for planning tasks, and for breaking down larger complex tasks into smaller steps. If you do not use this tool when planning, you may forget to do important tasks - and that is unacceptable.

It is critical that you mark todos as completed as soon as you are done with a task. Do not batch up multiple tasks before marking them as completed.

Examples:

<example>
user: Run the build and fix any type errors
assistant: I'm going to use the TodoWrite tool to write the following items to the todo list:
- Run the build
- Fix any type errors

I'm now going to run the build using Bash.

Looks like I found 10 type errors. I'm going to use the TodoWrite tool to write 10 items to the todo list.

marking the first todo as in_progress

Let me start working on the first item...

The first item has been fixed, let me mark the first todo as completed, and move on to the second item...
</example>

<example>
user: Help me write a new feature
assistant: I'll help you implement this feature. Let me first use the TodoWrite tool to plan this task.
Adding the following todos to the todo list:
1. Research existing code
2. Design the implementation
3. Implement core functionality
4. Write tests

Let me start by researching the existing codebase...
</example>

# Doing tasks
The user will primarily request you perform software engineering tasks. This includes solving bugs, adding new functionality, refactoring code, explaining code, and more.

IMPORTANT: Always use the TodoWrite tool to plan and track tasks throughout the conversation.

# Tool usage policy
- When doing file search, prefer to use the Task tool to reduce context usage.
- You can call multiple tools in a single response. If you intend to call multiple tools and there are no dependencies between them, make all independent tool calls in parallel.
- Use specialized tools instead of bash commands when possible. For file operations, use dedicated tools: Read for reading files, Edit for editing, Write for creating files.
- VERY IMPORTANT: When exploring the codebase to gather context, use the Task tool with the explore agent instead of running search commands directly.

# Code References
When referencing specific functions or pieces of code include the pattern `file_path:line_number` to allow the user to easily navigate to the source code location.
",
    );

    // Add project context
    let _ = writeln!(
        prompt,
        "\n# Environment\n\nWorking directory: {}\nPlatform: {}\nToday's date: {}",
        project_root.display(),
        std::env::consts::OS,
        chrono::Local::now().format("%Y-%m-%d"),
    );

    // Load AGENTS.md if it exists
    let agents_md = project_root.join("AGENTS.md");
    if agents_md.exists() {
        if let Ok(content) = std::fs::read_to_string(&agents_md) {
            // Truncate if very large (>20KB) to avoid blowing the context
            let content = if content.len() > 20_000 {
                format!("{}\n\n... (AGENTS.md truncated at 20KB)", &content[..20_000])
            } else {
                content
            };
            let _ = writeln!(prompt, "\n# Project Instructions (from AGENTS.md)\n\n{content}");
        }
    }

    prompt
}

/// A snapshot captured before a user message was processed.
///
/// Used by undo to restore the workspace to the state before a prompt.
#[derive(Debug, Clone)]
struct UndoEntry {
    /// The user message ID that was sent after this snapshot.
    user_message_id: String,
    /// The git tree hash captured before the user message was processed.
    snapshot_hash: String,
}

/// State captured when the user undoes a message, enabling redo.
#[derive(Debug, Clone)]
struct RedoEntry {
    /// Snapshot of the workspace right before undo was applied.
    pre_undo_snapshot: String,
    /// The user message ID that was removed by undo.
    user_message_id: String,
    /// The user's original prompt text (preserved for potential future re-send).
    #[expect(dead_code, reason = "reserved for future redo-and-resend feature")]
    user_text: String,
}

/// The result of an undo or redo operation.
#[derive(Debug)]
pub struct UndoResult {
    /// Human-readable description of what happened.
    pub message: String,
    /// Number of files that were modified.
    pub files_changed: usize,
}

/// The result of `send_message` — either a complete response or a cancelled partial.
#[derive(Debug)]
pub enum SendMessageResult {
    /// The assistant completed its response normally.
    Complete(String),
    /// The operation was cancelled by the user. Contains any partial text
    /// generated before cancellation.
    Cancelled {
        /// Partial response text accumulated before cancellation.
        partial_text: String,
    },
}

/// The session engine manages a single conversation.
pub struct SessionEngine {
    state: AppState,
    session_id: String,
    model_id: String,
    /// Stack of undo entries: snapshot hashes captured before each user message.
    undo_stack: Vec<UndoEntry>,
    /// Stack of redo entries: state captured before each undo, enabling redo.
    redo_stack: Vec<RedoEntry>,
    /// Cancellation token for the current operation. Triggered by `cancel()`,
    /// reset at the start of each `send_message()` call.
    cancel_token: CancellationToken,
}

impl SessionEngine {
    /// Create a new session engine.
    ///
    /// Creates the session in the database and returns the engine.
    ///
    /// # Errors
    ///
    /// Returns an error if the database operation fails.
    pub fn new(state: AppState, model_id: String) -> anyhow::Result<Self> {
        let session_id = Ulid::new().to_string();

        state.db.create_session(&session_id, &state.project_id, &model_id)?;

        state.bus.send(BusEvent::SessionCreated { session_id: session_id.clone() });

        Ok(Self {
            state,
            session_id,
            model_id,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            cancel_token: CancellationToken::new(),
        })
    }

    /// Resume an existing session.
    ///
    /// # Errors
    ///
    /// Returns an error if the session doesn't exist.
    pub fn resume(state: AppState, session_id: String) -> anyhow::Result<Self> {
        let session = state.db.get_session(&session_id)?;
        Ok(Self {
            state,
            session_id,
            model_id: session.model_id,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            cancel_token: CancellationToken::new(),
        })
    }

    /// The session ID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Cancel the current streaming or tool execution operation.
    ///
    /// Safe to call multiple times — cancelling an already-cancelled token is a no-op.
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// Reset the cancellation token for a new operation.
    ///
    /// Must be called before `send_message()` + `cancel_token()` to ensure
    /// the cloned token corresponds to the current operation.
    pub fn reset_cancel(&mut self) {
        self.cancel_token = CancellationToken::new();
    }

    /// Get a clone of the current cancellation token.
    ///
    /// Useful when the caller needs to trigger cancellation from a separate
    /// async branch (e.g., `tokio::select!`) without borrowing the engine.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Load historical messages for display (used when resuming a session).
    ///
    /// Returns a list of `(role, text_content)` pairs for display in the TUI.
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails.
    pub fn load_display_messages(&self) -> anyhow::Result<Vec<(String, String)>> {
        let rows = self.state.db.list_messages(&self.session_id)?;
        let mut display = Vec::new();

        for row in &rows {
            let parts: Vec<MessageContent> = serde_json::from_str(&row.parts)?;
            // Extract text content from the parts
            let mut text = String::new();
            for part in &parts {
                match part {
                    MessageContent::Text { text: t } => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(t);
                    }
                    MessageContent::Thinking { thinking } => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        let preview = &thinking[..thinking.len().min(100)];
                        let _ = write!(text, "(thinking: {preview}...)");
                    }
                    MessageContent::ToolUse { name, .. } => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        let _ = write!(text, "\u{2713} {name}");
                    }
                    MessageContent::ToolResult { .. } => {
                        // Skip tool results in display
                    }
                }
            }
            if !text.is_empty() {
                display.push((row.role.clone(), text));
            }
        }

        Ok(display)
    }

    /// List recent sessions as formatted text (for TUI display).
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails.
    pub fn list_sessions_text(&self) -> anyhow::Result<String> {
        use std::fmt::Write;
        let sessions = self.state.db.list_sessions(&self.state.project_id)?;
        if sessions.is_empty() {
            return Ok("No sessions found.".to_string());
        }

        let mut text = String::from("Recent sessions:\n\n");
        for (i, session) in sessions.iter().take(10).enumerate() {
            let title = if session.title.is_empty() { "(untitled)" } else { &session.title };
            let marker = if session.id == self.session_id { " \u{25B6}" } else { "  " };
            let _ = writeln!(text, "{marker} {}. {title}  [{:.8}]", i + 1, session.id);
        }
        let _ = write!(text, "\nResume with: flok --session <ID>");
        Ok(text)
    }

    /// Undo the last user message: restore workspace files and remove
    /// the message (and its responses) from conversation history.
    ///
    /// Returns a description of what was undone, or `None` if there's
    /// nothing to undo.
    ///
    /// # Errors
    ///
    /// Returns an error if snapshot restoration or DB operations fail.
    pub async fn undo(&mut self) -> anyhow::Result<Option<UndoResult>> {
        let Some(entry) = self.undo_stack.pop() else {
            return Ok(None);
        };

        // Capture current state for redo before modifying anything
        let pre_undo_snapshot = match self.state.snapshot.track().await {
            Ok(Some(hash)) => hash,
            Ok(None) => {
                // Snapshots disabled — still do the DB rollback
                String::new()
            }
            Err(e) => {
                tracing::warn!("pre-undo snapshot failed: {e}");
                String::new()
            }
        };

        // Recover the user's original text from the message (for redo)
        let user_text = match self.state.db.get_message(&entry.user_message_id) {
            Ok(msg) => {
                let parts: Vec<MessageContent> =
                    serde_json::from_str(&msg.parts).unwrap_or_default();
                parts
                    .into_iter()
                    .find_map(|p| match p {
                        MessageContent::Text { text } => Some(text),
                        _ => None,
                    })
                    .unwrap_or_default()
            }
            Err(_) => String::new(),
        };

        // Restore workspace files to the pre-message snapshot
        let files_changed = if entry.snapshot_hash.is_empty() {
            0
        } else {
            // Get the patch (files changed) before restoring
            let patch = self.state.snapshot.patch(&entry.snapshot_hash).await.unwrap_or_else(|e| {
                tracing::warn!("patch before undo restore failed: {e}");
                crate::snapshot::Patch { hash: entry.snapshot_hash.clone(), files: vec![] }
            });
            let count = patch.files.len();

            self.state.snapshot.restore(&entry.snapshot_hash).await?;

            self.state.bus.send(BusEvent::SnapshotRestored {
                session_id: self.session_id.clone(),
                snapshot_hash: entry.snapshot_hash.clone(),
            });
            count
        };

        // Delete the user message and all subsequent messages from DB
        let deleted =
            self.state.db.delete_messages_from(&self.session_id, &entry.user_message_id)?;
        tracing::info!(
            deleted_messages = deleted,
            files_changed,
            "undo: reverted to snapshot {}",
            &entry.snapshot_hash[..8.min(entry.snapshot_hash.len())]
        );

        // Push redo entry
        if !pre_undo_snapshot.is_empty() {
            self.redo_stack.push(RedoEntry {
                pre_undo_snapshot,
                user_message_id: entry.user_message_id,
                user_text,
            });
        }

        Ok(Some(UndoResult {
            message: format!(
                "Undone. {deleted} message(s) removed, {files_changed} file(s) restored."
            ),
            files_changed,
        }))
    }

    /// Redo the last undone message: restore workspace files to the state
    /// before undo was applied.
    ///
    /// Note: this restores file state but does NOT re-send the message to the
    /// LLM. The user can re-send manually if desired.
    ///
    /// Returns a description of what was redone, or `None` if there's
    /// nothing to redo.
    ///
    /// # Errors
    ///
    /// Returns an error if snapshot restoration fails.
    pub async fn redo(&mut self) -> anyhow::Result<Option<UndoResult>> {
        let Some(entry) = self.redo_stack.pop() else {
            return Ok(None);
        };

        // Capture current state so this redo can be undone again
        let pre_redo_snapshot = match self.state.snapshot.track().await {
            Ok(Some(hash)) => hash,
            _ => String::new(),
        };

        // Restore workspace files to the state before undo
        let files_changed = if entry.pre_undo_snapshot.is_empty() {
            0
        } else {
            let patch =
                self.state.snapshot.patch(&entry.pre_undo_snapshot).await.unwrap_or_else(|e| {
                    tracing::warn!("patch before redo restore failed: {e}");
                    crate::snapshot::Patch { hash: entry.pre_undo_snapshot.clone(), files: vec![] }
                });
            let count = patch.files.len();

            self.state.snapshot.restore(&entry.pre_undo_snapshot).await?;

            self.state.bus.send(BusEvent::SnapshotRestored {
                session_id: self.session_id.clone(),
                snapshot_hash: entry.pre_undo_snapshot.clone(),
            });
            count
        };

        // Push an undo entry so the user can undo this redo
        if !pre_redo_snapshot.is_empty() {
            self.undo_stack.push(UndoEntry {
                user_message_id: entry.user_message_id,
                snapshot_hash: pre_redo_snapshot,
            });
        }

        Ok(Some(UndoResult {
            message: format!("Redone. {files_changed} file(s) restored."),
            files_changed,
        }))
    }

    /// Send a user message and run the prompt loop until the assistant
    /// responds with text only (no tool calls).
    ///
    /// Returns `SendMessageResult::Complete` with the final text, or
    /// `SendMessageResult::Cancelled` with any partial text if the user
    /// cancelled via ESC.
    ///
    /// # Errors
    ///
    /// Returns an error if the provider fails or tool execution fails
    /// unrecoverably.
    pub async fn send_message(&mut self, user_text: &str) -> anyhow::Result<SendMessageResult> {
        // Capture workspace snapshot BEFORE processing this user message.
        // This is the undo point — if the user does /undo, we restore to here.
        let pre_snapshot = match self.state.snapshot.track().await {
            Ok(hash) => hash,
            Err(e) => {
                tracing::warn!("pre-message snapshot failed: {e}");
                None
            }
        };

        // Clear the redo stack when a new message is sent (new branch of history)
        self.redo_stack.clear();

        // Store the user message
        let user_msg_id = Ulid::new().to_string();
        let user_parts =
            serde_json::to_string(&[MessageContent::Text { text: user_text.to_string() }])?;
        self.state.db.insert_message(&user_msg_id, &self.session_id, "user", &user_parts)?;

        // Push undo entry if we got a snapshot
        if let Some(hash) = pre_snapshot {
            self.undo_stack
                .push(UndoEntry { user_message_id: user_msg_id.clone(), snapshot_hash: hash });
        }

        self.state.bus.send(BusEvent::MessageCreated {
            session_id: self.session_id.clone(),
            message_id: user_msg_id,
        });

        // Auto-generate session title from first user message
        let existing_messages = self.state.db.list_messages(&self.session_id)?;
        if existing_messages.len() <= 1 {
            // First message — set the title
            let title = if user_text.len() > 60 {
                format!("{}...", &user_text[..57])
            } else {
                user_text.to_string()
            };
            let _ = self.state.db.update_session_title(&self.session_id, &title);
        }

        // Run the prompt loop
        let mut rounds = 0;
        let mut call_history: HashMap<String, usize> = HashMap::new();
        let token_counter = TokenCounter::for_model(&self.model_id);
        let context_window =
            ModelRegistry::builtin().get(&self.model_id).map_or(200_000, |m| m.context_window);

        loop {
            rounds += 1;
            if rounds > MAX_TOOL_ROUNDS {
                return Err(anyhow::anyhow!(
                    "Tool call loop exceeded {MAX_TOOL_ROUNDS} rounds — possible doom loop"
                ));
            }

            let mut messages = self.assemble_messages()?;
            let system = build_system_prompt(&self.state.project_root);

            // Pre-flight token count: estimate context usage
            let estimated_tokens = estimate_message_tokens(&messages, &system, &token_counter);
            let usage_pct = estimated_tokens as f64 / context_window as f64;

            // Emit context usage to TUI sidebar
            self.state.bus.send(BusEvent::ContextUsage {
                session_id: self.session_id.clone(),
                used_tokens: estimated_tokens,
                max_tokens: context_window,
            });

            let messages = if usage_pct > 0.95 {
                // T3 Emergency truncation: keep only the last 3 turns
                tracing::warn!(
                    estimated_tokens,
                    context_window,
                    "context at {:.0}% — T3 emergency truncation",
                    usage_pct * 100.0
                );
                self.state.bus.send(BusEvent::Error {
                    message: format!(
                        "Context at {:.0}% — emergency truncation applied (keeping last 3 turns).",
                        usage_pct * 100.0
                    ),
                });

                // Keep last 6 message entries (3 turns = 3 user + 3 assistant)
                let keep = messages.len().min(6);
                let truncated = messages.split_off(messages.len() - keep);

                // Prepend a summary marker so the model knows context was lost
                let mut result = vec![Message {
                    role: "user".into(),
                    content: vec![MessageContent::Text {
                        text: "[Earlier conversation was truncated due to context window limits. \
                               The last few turns are preserved below.]"
                            .into(),
                    }],
                }];
                result.extend(truncated);
                result
            } else {
                if usage_pct > 0.80 {
                    tracing::info!(
                        estimated_tokens,
                        context_window,
                        "context at {:.0}%",
                        usage_pct * 100.0
                    );
                }
                messages
            };

            // Filter tools based on plan/build mode
            let is_plan = self.state.plan_mode.is_plan();
            let tools = if is_plan {
                // Plan mode: only read-only tools
                self.state
                    .tools
                    .tool_definitions()
                    .into_iter()
                    .filter(|t| {
                        matches!(
                            t.name.as_str(),
                            "read"
                                | "glob"
                                | "grep"
                                | "webfetch"
                                | "question"
                                | "todowrite"
                                | "plan"
                                | "skill"
                                | "agent_memory"
                        )
                    })
                    .collect()
            } else {
                self.state.tools.tool_definitions()
            };

            // Append plan mode instruction to system prompt
            let system = if is_plan {
                format!(
                    "{system}\n\n## Mode: PLAN\n\n\
                     You are in PLAN mode (read-only). You can read files, search code, \
                     and browse the web, but you CANNOT modify files, run commands, or make \
                     any changes. Focus on understanding the codebase, analyzing the problem, \
                     and creating a plan. When the user is ready to execute, they will switch \
                     to BUILD mode."
                )
            } else {
                system
            };

            let request = CompletionRequest {
                model: self.model_id.clone(),
                system,
                messages,
                tools,
                max_tokens: 16_384,
            };

            let (text, reasoning, tool_calls) =
                match self.stream_completion_with_retry(request).await {
                    Ok(result) => result,
                    Err(e) => {
                        // Check if this was a cancellation
                        if let Some(cancelled) = e.downcast_ref::<CancelledError>() {
                            let partial = cancelled.partial_text.clone();

                            // Persist partial response so conversation history stays coherent
                            if !partial.is_empty() {
                                let partial_msg_id = Ulid::new().to_string();
                                let mut parts: Vec<MessageContent> = Vec::new();
                                if !cancelled.partial_reasoning.is_empty() {
                                    parts.push(MessageContent::Thinking {
                                        thinking: cancelled.partial_reasoning.clone(),
                                    });
                                }
                                parts.push(MessageContent::Text {
                                    text: format!("{partial}\n\n_(cancelled by user)_"),
                                });
                                let parts_json = serde_json::to_string(&parts)?;
                                self.state.db.insert_message(
                                    &partial_msg_id,
                                    &self.session_id,
                                    "assistant",
                                    &parts_json,
                                )?;
                            }

                            self.state
                                .bus
                                .send(BusEvent::Cancelled { session_id: self.session_id.clone() });

                            return Ok(SendMessageResult::Cancelled { partial_text: partial });
                        }
                        return Err(e);
                    }
                };

            // Store the assistant message
            let assistant_msg_id = Ulid::new().to_string();
            let mut parts: Vec<MessageContent> = Vec::new();

            // Store reasoning/thinking first (if any)
            if !reasoning.is_empty() {
                parts.push(MessageContent::Thinking { thinking: reasoning });
            }

            if !text.is_empty() {
                parts.push(MessageContent::Text { text: text.clone() });
            }
            for tc in &tool_calls {
                parts.push(MessageContent::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: serde_json::from_str(&tc.arguments).unwrap_or_default(),
                });
            }

            let parts_json = serde_json::to_string(&parts)?;
            self.state.db.insert_message(
                &assistant_msg_id,
                &self.session_id,
                "assistant",
                &parts_json,
            )?;

            self.state.bus.send(BusEvent::StreamingComplete {
                session_id: self.session_id.clone(),
                message_id: assistant_msg_id,
            });

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                return Ok(SendMessageResult::Complete(text));
            }

            // Check cancellation before starting tool execution
            if self.cancel_token.is_cancelled() {
                // Persist what we have
                self.state.bus.send(BusEvent::Cancelled { session_id: self.session_id.clone() });
                return Ok(SendMessageResult::Cancelled { partial_text: text });
            }

            // Snapshot: capture workspace state BEFORE tool execution
            let pre_snapshot = match self.state.snapshot.track().await {
                Ok(Some(hash)) => {
                    self.state.bus.send(BusEvent::SnapshotCreated {
                        session_id: self.session_id.clone(),
                        snapshot_hash: hash.clone(),
                    });
                    Some(hash)
                }
                Ok(None) => None,
                Err(e) => {
                    tracing::warn!("pre-tool snapshot failed: {e}");
                    None
                }
            };

            // Doom loop detection: check for identical tool calls
            for tc in &tool_calls {
                let key = format!("{}:{}", tc.name, tc.arguments);
                let count = call_history.entry(key).or_insert(0);
                *count += 1;
                if *count >= MAX_IDENTICAL_CALLS {
                    tracing::warn!(
                        tool = %tc.name,
                        count = *count,
                        "doom loop detected: identical tool call repeated {MAX_IDENTICAL_CALLS} times"
                    );

                    // Ask user whether to continue instead of hard error
                    let description = format!(
                        "Tool '{}' called with identical arguments {} times (possible doom loop). Continue?",
                        tc.name, count
                    );
                    let allowed = self
                        .state
                        .permissions
                        .check(
                            "doom_loop_continue",
                            crate::tool::PermissionLevel::Dangerous,
                            &description,
                        )
                        .await;

                    if !allowed {
                        return Err(anyhow::anyhow!(
                            "Doom loop stopped by user: tool '{}' repeated {} times.",
                            tc.name,
                            count
                        ));
                    }
                    // User chose to continue — reset the counter for this key
                    *count = 0;
                }
            }

            // Execute tool calls and store results
            let tool_results = self.execute_tool_calls(&tool_calls).await;

            // Snapshot: capture workspace state AFTER tool execution and compute patch
            if let Some(ref pre_hash) = pre_snapshot {
                match self.state.snapshot.track().await {
                    Ok(Some(post_hash)) => {
                        self.state.bus.send(BusEvent::SnapshotCreated {
                            session_id: self.session_id.clone(),
                            snapshot_hash: post_hash,
                        });
                        // Compute which files changed during tool execution
                        match self.state.snapshot.patch(pre_hash).await {
                            Ok(patch) if !patch.files.is_empty() => {
                                tracing::debug!(
                                    files = patch.files.len(),
                                    "snapshot: {} files changed during tool execution",
                                    patch.files.len()
                                );
                                self.state.bus.send(BusEvent::SnapshotPatch {
                                    session_id: self.session_id.clone(),
                                    snapshot_hash: pre_hash.clone(),
                                    files_changed: patch.files.len(),
                                });
                            }
                            Ok(_) => {} // No files changed
                            Err(e) => tracing::warn!("snapshot patch failed: {e}"),
                        }
                    }
                    Ok(None) => {} // Snapshots disabled
                    Err(e) => tracing::warn!("post-tool snapshot failed: {e}"),
                }
            }

            let result_parts: Vec<MessageContent> = tool_results
                .into_iter()
                .map(|r| MessageContent::ToolResult {
                    tool_use_id: r.tool_call_id,
                    content: r.content,
                    is_error: r.is_error,
                })
                .collect();

            let result_msg_id = Ulid::new().to_string();
            let result_json = serde_json::to_string(&result_parts)?;
            self.state.db.insert_message(&result_msg_id, &self.session_id, "user", &result_json)?;

            // Check cancellation after tool execution — don't start another round
            if self.cancel_token.is_cancelled() {
                self.state.bus.send(BusEvent::Cancelled { session_id: self.session_id.clone() });
                return Ok(SendMessageResult::Cancelled { partial_text: text });
            }
        }
    }

    /// Assemble the message history from the database.
    ///
    /// Runs T1 pruning (recency-based tool output clearing) before assembly
    /// to keep context within bounds.
    fn assemble_messages(&self) -> anyhow::Result<Vec<Message>> {
        let rows = self.state.db.list_messages(&self.session_id)?;
        let mut all_parts: Vec<Vec<MessageContent>> = Vec::with_capacity(rows.len());

        for row in &rows {
            let content: Vec<MessageContent> = serde_json::from_str(&row.parts)?;
            all_parts.push(content);
        }

        // T1 pruning: clear old tool outputs to manage context size.
        // Protects the last 40K tokens (~160K chars) of tool output.
        let pruned = crate::compress::prune_tool_outputs(&mut all_parts, 40_000, 4);
        if pruned > 0 {
            tracing::info!(pruned, "T1 pruning cleared old tool outputs");
        }

        // Layer 2: compress tool_result content (JSON minify, TOON encoding)
        let mut l2_compressed = 0u32;
        for parts in &mut all_parts {
            for part in parts.iter_mut() {
                if let MessageContent::ToolResult { content, is_error, .. } = part {
                    if *is_error || content == crate::compress::pruning::PRUNED_PLACEHOLDER {
                        continue;
                    }
                    let original_len = content.len();
                    let compressed = crate::compress::compress_tool_result(content, false);
                    if compressed.len() < original_len {
                        l2_compressed += 1;
                        *content = compressed;
                    }
                }
            }
        }
        if l2_compressed > 0 {
            tracing::debug!(l2_compressed, "Layer 2 compressed tool results");
        }

        // Emit compression stats
        if pruned > 0 || l2_compressed > 0 {
            self.state.bus.send(BusEvent::CompressionStats {
                session_id: self.session_id.clone(),
                t1_pruned: pruned as u32,
                l2_compressed,
            });
        }

        // Reassemble into messages
        let messages: Vec<Message> = rows
            .iter()
            .zip(all_parts)
            .map(|(row, content)| Message { role: row.role.clone(), content })
            .collect();

        Ok(messages)
    }

    /// Stream a completion request and collect the response.
    ///
    /// Returns `(text, reasoning, tool_calls)`. If the cancellation token
    /// fires mid-stream, returns whatever was accumulated so far as a
    /// `CancelledError`.
    async fn stream_completion(
        &self,
        request: CompletionRequest,
    ) -> anyhow::Result<(String, String, Vec<ToolCall>)> {
        let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();

        // Only move the provider Arc into the spawn (Db is !Send).
        // Keep the JoinHandle so we can abort on cancellation.
        let provider = Arc::clone(&self.state.provider);
        let provider_handle = tokio::spawn(async move {
            if let Err(e) = provider.stream(request, tx).await {
                tracing::error!("Provider stream error: {e}");
            }
        });

        let mut text = String::new();
        let mut reasoning = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let stream_timeout = std::time::Duration::from_secs(30);

        loop {
            tokio::select! {
                biased;

                // Cancellation takes priority
                () = self.cancel_token.cancelled() => {
                    provider_handle.abort();
                    tracing::info!("stream cancelled by user");
                    return Err(CancelledError {
                        partial_text: text,
                        partial_reasoning: reasoning,
                    }.into());
                }

                result = tokio::time::timeout(stream_timeout, rx.recv()) => {
                    let event = match result {
                        Ok(Some(event)) => event,
                        Ok(None) => break, // Channel closed
                        Err(_) => {
                            // 30s with no event — stream is hung
                            tracing::warn!("stream timeout: no event for 30s, aborting");
                            provider_handle.abort();
                            return Err(anyhow::anyhow!(
                                "Stream timeout: no response from provider for 30 seconds. \
                                 The model may be overloaded. Try again."
                            ));
                        }
                    };
                    match event {
                        StreamEvent::TextDelta(delta) => {
                            if !delta.is_empty() {
                                text.push_str(&delta);
                                self.state.bus.send(BusEvent::TextDelta {
                                    session_id: self.session_id.clone(),
                                    message_id: String::new(),
                                    delta,
                                });
                            }
                        }
                        StreamEvent::ReasoningDelta(delta) => {
                            if !delta.is_empty() {
                                reasoning.push_str(&delta);
                                self.state.bus.send(BusEvent::ReasoningDelta {
                                    session_id: self.session_id.clone(),
                                    delta,
                                });
                            }
                        }
                        StreamEvent::ToolCallStart { index, id, name } => {
                            while tool_calls.len() <= index {
                                tool_calls.push(ToolCall::default());
                            }
                            tool_calls[index].id = id;
                            tool_calls[index].name = name;
                        }
                        StreamEvent::ToolCallDelta { index, delta } => {
                            if let Some(tc) = tool_calls.get_mut(index) {
                                tc.arguments.push_str(&delta);
                            }
                        }
                        StreamEvent::Usage {
                            input_tokens,
                            output_tokens,
                            cache_read_tokens,
                            cache_creation_tokens,
                        } => {
                            tracing::debug!(input_tokens, output_tokens, cache_read_tokens, "token usage");
                            self.state.cost_tracker.record(
                                input_tokens,
                                output_tokens,
                                cache_read_tokens,
                                cache_creation_tokens,
                            );
                            self.state.bus.send(BusEvent::TokenUsage {
                                session_id: self.session_id.clone(),
                                input_tokens,
                                output_tokens,
                            });
                            self.state.bus.send(BusEvent::CostUpdate {
                                session_id: self.session_id.clone(),
                                total_cost_usd: self.state.cost_tracker.estimated_cost_usd(),
                            });
                        }
                        StreamEvent::Done => break,
                        StreamEvent::Error(e) => {
                            return Err(anyhow::anyhow!("Provider error: {e}"));
                        }
                    }
                }
            }
        }

        // Filter out incomplete tool calls (padding entries from content_block indexing)
        let tool_calls: Vec<ToolCall> =
            tool_calls.into_iter().filter(|tc| !tc.id.is_empty() && !tc.name.is_empty()).collect();

        Ok((text, reasoning, tool_calls))
    }

    /// Stream a completion with retry on transient errors (429, 529, 500, overloaded).
    ///
    /// Uses exponential backoff: 1s, 2s, 4s (max 3 retries).
    /// Cancellation errors are never retried.
    async fn stream_completion_with_retry(
        &self,
        request: CompletionRequest,
    ) -> anyhow::Result<(String, String, Vec<ToolCall>)> {
        let max_retries = 3u32;
        let mut attempt = 0u32;

        loop {
            let result = self.stream_completion(request.clone()).await;

            match &result {
                Ok(_) => return result,
                Err(e) => {
                    // Never retry cancellations
                    if e.downcast_ref::<CancelledError>().is_some() {
                        return result;
                    }

                    let err_msg = e.to_string();
                    let is_retryable = err_msg.contains("429")
                        || err_msg.contains("529")
                        || err_msg.contains("500")
                        || err_msg.contains("overloaded")
                        || err_msg.contains("rate_limit")
                        || err_msg.contains("capacity");

                    attempt += 1;
                    if !is_retryable || attempt > max_retries {
                        return result;
                    }

                    // Try to extract retry-after from error message (Anthropic includes it)
                    let retry_after = extract_retry_after(&err_msg);
                    let backoff_secs: u64 = 1 << (attempt - 1);
                    let delay_secs = retry_after.unwrap_or(backoff_secs);
                    let delay = std::time::Duration::from_secs(delay_secs);
                    tracing::warn!(
                        attempt,
                        max_retries,
                        delay_secs = delay.as_secs(),
                        error = %err_msg,
                        "retrying after transient error"
                    );

                    self.state.bus.send(BusEvent::Error {
                        message: format!(
                            "Rate limited — retrying in {}s (attempt {attempt}/{max_retries})",
                            delay.as_secs()
                        ),
                    });

                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    /// Execute a batch of tool calls.
    ///
    /// Checks the cancellation token between calls. If cancelled, remaining
    /// tool calls are skipped and returned as cancelled errors.
    async fn execute_tool_calls(&self, tool_calls: &[ToolCall]) -> Vec<ToolResult> {
        let mut results = Vec::with_capacity(tool_calls.len());
        let ctx = self.state.tool_context(&self.session_id, self.cancel_token.clone());

        for tc in tool_calls {
            // Check cancellation before each tool call
            if self.cancel_token.is_cancelled() {
                tracing::info!(tool = %tc.name, "skipping tool call — cancelled by user");
                results.push(ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: "Cancelled by user.".into(),
                    is_error: true,
                });
                continue;
            }

            tracing::debug!(tool = %tc.name, id = %tc.id, "executing tool call");

            self.state.bus.send(BusEvent::ToolCallStarted {
                session_id: self.session_id.clone(),
                tool_name: tc.name.clone(),
                tool_call_id: tc.id.clone(),
            });

            let result = if let Some(tool) = self.state.tools.get(&tc.name) {
                let args: serde_json::Value =
                    serde_json::from_str(&tc.arguments).unwrap_or_default();

                // Plan mode safety net: block write/dangerous tools
                if self.state.plan_mode.is_plan()
                    && tool.permission_level() != crate::tool::PermissionLevel::Safe
                {
                    ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: format!(
                            "Tool '{}' blocked: currently in PLAN mode (read-only). \
                             Switch to BUILD mode to make changes.",
                            tc.name
                        ),
                        is_error: true,
                    }
                } else {
                    // Validate args against the tool's JSON schema
                    let schema = tool.parameters_schema();
                    if let Err(e) = validate_tool_args(&args, &schema) {
                        ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: format!("Invalid arguments: {e}"),
                            is_error: true,
                        }
                    } else {
                        // Check permissions before execution
                        let description = tool.describe_invocation(&args);
                        let allowed = self
                            .state
                            .permissions
                            .check(&tc.name, tool.permission_level(), &description)
                            .await;

                        if allowed {
                            // Execute with panic safety
                            let exec_result =
                                std::panic::AssertUnwindSafe(tool.execute(args, &ctx));
                            match futures::FutureExt::catch_unwind(exec_result).await {
                                Ok(Ok(output)) => ToolResult {
                                    tool_call_id: tc.id.clone(),
                                    content: output.content,
                                    is_error: output.is_error,
                                },
                                Ok(Err(e)) => ToolResult {
                                    tool_call_id: tc.id.clone(),
                                    content: format!("Tool execution error: {e}"),
                                    is_error: true,
                                },
                                Err(_panic) => ToolResult {
                                    tool_call_id: tc.id.clone(),
                                    content: format!(
                                        "Tool '{}' panicked during execution",
                                        tc.name
                                    ),
                                    is_error: true,
                                },
                            }
                        } else {
                            ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: format!("Permission denied for tool '{}'", tc.name),
                                is_error: true,
                            }
                        }
                    } // close schema validation else
                } // close plan mode else
            } else {
                ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: format!(
                        "Unknown tool: {}. Available tools: {}",
                        tc.name,
                        self.state.tools.names().join(", ")
                    ),
                    is_error: true,
                }
            };

            // Truncate very large outputs
            let truncated = if result.content.len() > 50_000 {
                let mut s = result.content[..25_000].to_string();
                let _ = write!(
                    s,
                    "\n\n... [truncated {} bytes] ...\n\n",
                    result.content.len() - 50_000
                );
                s.push_str(&result.content[result.content.len() - 25_000..]);
                ToolResult { content: s, ..result }
            } else {
                result
            };

            self.state.bus.send(BusEvent::ToolCallCompleted {
                session_id: self.session_id.clone(),
                tool_name: tc.name.clone(),
                tool_call_id: tc.id.clone(),
                is_error: truncated.is_error,
            });

            results.push(truncated);
        }

        results
    }
}

/// An accumulated tool call from streaming.
#[derive(Debug, Clone, Default)]
struct ToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// The result of executing a tool.
#[derive(Debug, Clone)]
struct ToolResult {
    tool_call_id: String,
    content: String,
    is_error: bool,
}

/// Error returned when an operation is cancelled by the user.
///
/// Contains partial results accumulated before cancellation so the caller
/// can persist them for conversation history coherence.
#[derive(Debug, thiserror::Error)]
#[error("operation cancelled by user")]
struct CancelledError {
    partial_text: String,
    partial_reasoning: String,
}

/// Estimate total token count for a set of messages + system prompt.
fn estimate_message_tokens(messages: &[Message], system: &str, counter: &TokenCounter) -> u64 {
    let mut total = counter.count(system) as u64;

    for msg in messages {
        // Role overhead (~4 tokens per message)
        total += 4;
        for content in &msg.content {
            match content {
                MessageContent::Text { text } => {
                    total += counter.count(text) as u64;
                }
                MessageContent::Thinking { thinking } => {
                    total += counter.count(thinking) as u64;
                }
                MessageContent::ToolUse { name, input, .. } => {
                    total += counter.count(name) as u64;
                    total += counter.count(&input.to_string()) as u64;
                }
                MessageContent::ToolResult { content, .. } => {
                    total += counter.count(content) as u64;
                }
            }
        }
    }

    total
}

/// Validate tool arguments against a JSON schema.
///
/// Returns `Ok(())` if valid, or a descriptive error string if not.
fn validate_tool_args(args: &serde_json::Value, schema: &serde_json::Value) -> Result<(), String> {
    let validator =
        jsonschema::validator_for(schema).map_err(|e| format!("invalid tool schema: {e}"))?;

    let errors: Vec<String> = validator.iter_errors(args).map(|e| e.to_string()).collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Try to extract a retry-after delay from an error message.
///
/// Anthropic's rate limit responses include `"retry_after": N` in JSON body.
/// We parse it from the error string since we don't have the raw response.
fn extract_retry_after(error_msg: &str) -> Option<u64> {
    // Look for "retry_after": N or retry-after: N patterns
    if let Some(pos) = error_msg.find("retry_after") {
        let rest = &error_msg[pos..];
        // Find the number after the colon
        if let Some(colon) = rest.find(':') {
            let after_colon = rest[colon + 1..].trim_start();
            let num_str: String = after_colon.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(secs) = num_str.parse::<u64>() {
                return Some(secs.min(60)); // Cap at 60s
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_retry_after_from_anthropic_error() {
        let msg = r#"HTTP 429: {"type":"error","error":{"type":"rate_limit_error","message":"Rate limited"},"retry_after": 15}"#;
        assert_eq!(extract_retry_after(msg), Some(15));
    }

    #[test]
    fn extract_retry_after_missing() {
        assert_eq!(extract_retry_after("some random error"), None);
    }

    #[test]
    fn extract_retry_after_capped() {
        let msg = r#""retry_after": 300"#;
        assert_eq!(extract_retry_after(msg), Some(60)); // Capped
    }
}
