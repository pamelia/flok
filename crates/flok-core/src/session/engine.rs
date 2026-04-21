//! The session engine — manages the prompt loop for a single conversation.
//!
//! The loop:
//! 1. Assemble messages (system prompt + conversation history)
//! 2. Send to provider, stream response
//! 3. Accumulate text and tool calls from the stream
//! 4. If tool calls: execute them, append results, go to step 1
//! 5. If text only: done — return the assistant's response

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write;
use std::sync::Arc;

use chrono::Utc;
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

use crate::bus::BusEvent;
use crate::plan::{
    summarize_plan, Checkpoint, CheckpointData, ExecutionPlan, PlanPatch, PlanStatus, PlanStore,
    StepStatus,
};
use crate::provider::{CompletionRequest, Message, MessageContent, ModelRegistry, StreamEvent};
use crate::session::state::AppState;
use crate::token::TokenCounter;
use crate::verification::{
    detect_command_with_preference as detect_verification_command,
    run_command as run_verification_command, RetryChangeRelevance, VerificationFailureSummary,
    VerificationPreference,
};

/// Maximum number of tool-call rounds before we stop (doom loop protection).
const MAX_TOOL_ROUNDS: usize = 25;

/// Maximum identical tool calls before pausing (doom loop by repetition).
const MAX_IDENTICAL_CALLS: usize = 3;

/// Maximum automatic self-fix rounds after verification failure.
const MAX_VERIFICATION_RETRIES: usize = 1;

/// Additional automatic self-fix rounds granted when a relevant retry exposes
/// a different verification failure instead of churning on the same one.
const MAX_VERIFICATION_BONUS_RETRIES: usize = 1;

/// Build the system prompt with project context.
///
/// The prompt includes:
/// - Base instructions for the coding agent
/// - Project root and current working directory
/// - AGENTS.md content if it exists in the project root
/// - Available multi-provider runtime options for cross-coverage sub-agent dispatch
fn build_system_prompt(
    project_root: &std::path::Path,
    provider_registry: &crate::provider::ProviderRegistry,
) -> String {
    let mut prompt = String::from(
        r"You are flok, an expert AI coding agent for the terminal.

You are an interactive CLI tool that helps users with software engineering tasks. Use the instructions below and the tools available to you to assist the user.

# Tone and style
- Only use emojis if the user explicitly requests it. Avoid using emojis in all communication unless asked.
- Your output will be displayed on a command line interface. Your responses should be short and concise. You can use GitHub-flavored markdown for formatting.
- Output text to communicate with the user; all text you output outside of tool use is displayed to the user. Only use tools to complete tasks.
- NEVER create files unless they're absolutely necessary for achieving your goal. ALWAYS prefer editing an existing file to creating a new one.

# Professional objectivity
Prioritize technical accuracy and truthfulness over validating the user's beliefs. Focus on facts and problem-solving, providing direct, objective technical info without any unnecessary superlatives, praise, or emotional validation. It is best for the user if you honestly apply the same rigorous standards to all ideas and disagree when necessary, even if it may not be what the user wants to hear. Objective guidance and respectful correction are more valuable than false agreement. Whenever there is uncertainty, investigate to find the truth first rather than instinctively confirming the user's beliefs.

# Scope discipline
Touch only what the task requires.

- Do NOT 'clean up' code adjacent to your change.
- Do NOT refactor imports in files you are not modifying.
- Do NOT remove comments you don't fully understand.
- Do NOT add features not in the spec because they 'seem useful'.
- Do NOT modernize syntax in files you are only reading.

If you notice something worth improving outside your task scope, note it -- don't fix it:

```
NOTICED BUT NOT TOUCHING:
- src/utils/format.rs has an unused import (unrelated to this task)
- The auth middleware could use better error messages (separate task)
```

# Handling ambiguity and confusion
When requirements are ambiguous, conflicting, or incomplete, do NOT silently pick one interpretation. Surface the conflict explicitly and ask the user.

When context conflicts (e.g., a spec says one thing but existing code does another), present the options clearly:

```
AMBIGUITY:
The spec calls for X, but the existing codebase does Y.

Options:
A) Follow the spec -- implement X
B) Follow existing patterns -- do Y, update the spec
C) Ask -- this seems like an intentional decision I shouldn't override

Which approach should I take?
```

When requirements are incomplete, check existing code for precedent. If no precedent exists, stop and ask. Don't invent requirements.

# Stop-the-line rule
When tests fail, builds break, or behavior is unexpected: STOP. Do not push past a failure to work on the next feature. Errors compound.

1. STOP adding features or making changes
2. PRESERVE evidence (error output, logs, repro steps)
3. DIAGNOSE the root cause (not just symptoms)
4. FIX the underlying issue
5. GUARD against recurrence (add a regression test)
6. RESUME only after verification passes

A bug in Step 3 that goes unfixed makes Steps 4-10 wrong. Fix failures immediately.

# Assumptions
Before starting non-trivial work, state your assumptions explicitly:

```
ASSUMPTIONS:
1. The database is PostgreSQL (based on existing schema)
2. We're targeting the stable Rust toolchain
3. The existing test patterns use tokio::test for async
Correct me now or I'll proceed with these.
```

Don't silently fill in ambiguous requirements. Surface misunderstandings before code gets written.

# Change summaries
After any non-trivial modification, provide a structured summary:

```
CHANGES MADE:
- src/routes/tasks.rs: Added validation to the POST endpoint
- src/lib/validation.rs: Added TaskCreateSchema

NOT TOUCHED (intentionally):
- src/routes/auth.rs: Has similar validation gap but out of scope

POTENTIAL CONCERNS:
- The schema is strict -- rejects extra fields. Confirm this is desired.
```

This surfaces unintended changes and shows scope discipline.

# Common shortcuts to avoid
- 'I'll add tests later' -- Write tests with the code, not after. Tests written after the fact test implementation, not behavior.
- 'I'll clean it up later' -- Later never comes. Do it now or file a separate task.
- 'This is too simple to test' -- Simple code gets complicated. The test documents expected behavior.
- 'It works, that's good enough' -- Working code that's unreadable, insecure, or architecturally wrong creates debt.
- 'These changes are too small to commit separately' -- Small commits are free. Large commits hide bugs.

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

For multi-step tasks, follow this approach:
1. Understand the request and surface assumptions
2. Plan the work using TodoWrite (break into small, verifiable steps)
3. Implement one slice at a time -- implement, test, verify, then move on
4. After each slice, confirm tests pass and the build is clean
5. Summarize what changed when done

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

    let configured_providers = provider_registry.configured_providers();
    let _ = writeln!(prompt, "\n## Available Providers\n");
    if configured_providers.len() <= 1 {
        if let Some(provider_name) = configured_providers.first() {
            let default_model = provider_registry
                .display_default_model(provider_name)
                .unwrap_or_else(|| "not set".to_string());
            let _ = writeln!(prompt, "Available providers: {}.", provider_registry.describe());
            let _ = writeln!(
                prompt,
                "Only {provider_name} is configured (default model: {default_model}) — cross-coverage review will run specialists once."
            );
        } else {
            let _ = writeln!(prompt, "No providers are configured for sub-agent dispatch.");
        }
    } else {
        let _ = writeln!(
            prompt,
            "You have these LLM providers configured and can dispatch sub-agents to each via the `task` tool's `model` parameter:"
        );
        for provider_name in configured_providers {
            let default_model = provider_registry
                .display_default_model(provider_name)
                .unwrap_or_else(|| "not set".to_string());
            let _ = writeln!(prompt, "- {provider_name} (default model: {default_model})");
        }
        let _ = writeln!(
            prompt,
            "\nFor multi-model review (code review, spec review), use cross-coverage: spawn each specialist ONCE PER PROVIDER by calling `task(...)` multiple times with different `model` values. This ensures every finding is stress-tested by every available model."
        );
    }

    // List available skills (built-in + project-local)
    let _ = writeln!(prompt, "\n# Available Skills\n");
    let _ =
        writeln!(prompt, "Use the `skill` tool to load detailed instructions for these workflows:");
    for skill in crate::skills::BUILTIN_SKILLS {
        let _ = writeln!(prompt, "- **{}**: {}", skill.name, skill.description);
    }
    // Check for project-local skills
    let local_skills_dir = project_root.join(".flok").join("skills");
    if local_skills_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&local_skills_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let name = name.strip_suffix(".md").unwrap_or(&name);
                // Skip if it shadows a built-in (already listed)
                if crate::skills::get_builtin_skill(name).is_none() {
                    let _ = writeln!(prompt, "- **{name}** (project-local)");
                }
            }
        }
    }

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

    /// List saved execution plans for this session.
    ///
    /// # Errors
    ///
    /// Returns an error if plan files cannot be read.
    pub fn list_plans_text(&self) -> anyhow::Result<String> {
        let plans = self.session_plans()?;
        if plans.is_empty() {
            return Ok("No saved plans found for this session.".to_string());
        }

        let mut text = String::from("Saved plans:\n\n");
        for (index, plan) in plans.iter().enumerate() {
            let completed_steps = plan
                .steps
                .iter()
                .filter(|step| matches!(step.status, StepStatus::Completed))
                .count();
            let _ = writeln!(
                text,
                "{}. {} [{}] steps {}/{}  [{:.8}]",
                index + 1,
                plan.title,
                plan_status_label(&plan.status),
                completed_steps,
                plan.steps.len(),
                plan.id,
            );
        }
        let _ = write!(text, "\nUse /show-plan [ID], /approve [ID], or /execute-plan [ID].");
        Ok(text)
    }

    /// Show a persisted execution plan for this session.
    ///
    /// # Errors
    ///
    /// Returns an error if the requested plan cannot be loaded.
    pub fn show_plan_text(&self, plan_id: Option<&str>) -> anyhow::Result<String> {
        let plan = self.resolve_plan(plan_id)?;
        let store = self.plan_store();
        Ok(format_plan_details(&plan, &store))
    }

    /// Approve a persisted execution plan so it can be executed.
    ///
    /// # Errors
    ///
    /// Returns an error if the requested plan cannot be loaded or updated.
    pub fn approve_plan(&self, plan_id: Option<&str>) -> anyhow::Result<String> {
        let plan = self.resolve_plan(plan_id)?;
        if matches!(plan.status, PlanStatus::Executing) {
            anyhow::bail!("cannot approve a plan while it is executing");
        }

        let updated = self.plan_store().apply_patch(
            &plan.id,
            PlanPatch { plan_status: Some(PlanStatus::Approved), ..PlanPatch::default() },
        )?;

        Ok(format!("Plan approved.\n\n{}", format_plan_details(&updated, &self.plan_store())))
    }

    /// Execute an approved plan step-by-step by sending each step as a prompt.
    ///
    /// # Errors
    ///
    /// Returns an error if execution fails, is cancelled, or the plan is invalid.
    pub async fn execute_plan(&mut self, plan_id: Option<&str>) -> anyhow::Result<String> {
        if self.state.plan_mode.is_plan() {
            anyhow::bail!("cannot execute a plan in PLAN mode; switch to BUILD mode first");
        }

        let store = self.plan_store();
        let mut plan = self.resolve_plan(plan_id)?;
        if matches!(plan.status, PlanStatus::Draft) {
            anyhow::bail!("plan '{}' is still a draft; approve it before execution", plan.id);
        }
        if matches!(plan.status, PlanStatus::Completed) {
            return Ok(format!(
                "Plan already completed.\n\n{}",
                format_plan_details(&plan, &store)
            ));
        }

        plan.status = PlanStatus::Executing;
        plan.updated_at = Utc::now();
        store.save_plan(&plan)?;

        loop {
            if self.cancel_token.is_cancelled() {
                plan.status = PlanStatus::Cancelled;
                plan.updated_at = Utc::now();
                store.save_plan(&plan)?;
                anyhow::bail!("plan execution cancelled");
            }

            if let Some(step_index) = next_ready_step_index(&plan) {
                let step_id = plan.steps[step_index].id.clone();
                let checkpoint = match self.state.snapshot.track().await {
                    Ok(Some(hash)) => Some(Checkpoint {
                        step_id: step_id.clone(),
                        snapshot: CheckpointData::WorkspaceSnapshot { hash },
                        created_at: Utc::now(),
                    }),
                    Ok(None) => None,
                    Err(error) => {
                        tracing::warn!(%error, step_id, "failed to capture plan checkpoint");
                        None
                    }
                };

                plan = store.apply_patch(
                    &plan.id,
                    PlanPatch {
                        plan_status: Some(PlanStatus::Executing),
                        step_id: Some(step_id.clone()),
                        step_status: Some(StepStatus::Running),
                        checkpoint,
                    },
                )?;

                let step = plan
                    .steps
                    .iter()
                    .find(|candidate| candidate.id == step_id)
                    .cloned()
                    .expect("running step must exist in plan");
                let prompt = build_plan_step_prompt(&plan, &step);

                match self.send_message(&prompt).await {
                    Ok(SendMessageResult::Complete(_)) => {
                        plan = store.apply_patch(
                            &plan.id,
                            PlanPatch {
                                step_id: Some(step_id),
                                step_status: Some(StepStatus::Completed),
                                ..PlanPatch::default()
                            },
                        )?;
                    }
                    Ok(SendMessageResult::Cancelled { .. }) => {
                        let detail =
                            rollback_plan_step(&self.state.snapshot, &step).await.unwrap_or_else(
                                |error| format!("cancellation rollback failed: {error}"),
                            );
                        mark_cancelled_plan(&store, &plan, &step.id, &detail)?;
                        anyhow::bail!("plan execution cancelled during step '{}'", step.title);
                    }
                    Err(error) => {
                        let rollback_detail = rollback_plan_step(&self.state.snapshot, &step)
                            .await
                            .unwrap_or_else(|rollback_error| {
                                format!("rollback failed after step error: {rollback_error}")
                            });
                        mark_failed_plan(
                            &store,
                            &plan,
                            &step.id,
                            &error.to_string(),
                            &rollback_detail,
                        )?;
                        return Err(anyhow::anyhow!(
                            "plan execution failed at step '{}': {error}",
                            step.title
                        ));
                    }
                }
                continue;
            }

            if plan.steps.iter().all(|step| matches!(step.status, StepStatus::Completed)) {
                plan.status = PlanStatus::Completed;
                plan.updated_at = Utc::now();
                store.save_plan(&plan)?;
                return Ok(format!(
                    "Plan executed successfully.\n\n{}",
                    format_plan_details(&plan, &store)
                ));
            }

            if plan.steps.iter().any(|step| matches!(step.status, StepStatus::Failed(_))) {
                anyhow::bail!("plan '{}' has failed steps and cannot continue", plan.id);
            }

            let blocked_steps =
                plan.steps.iter().filter(|step| matches!(step.status, StepStatus::Pending)).count();
            anyhow::bail!(
                "plan '{}' is blocked; {blocked_steps} pending step(s) have unsatisfied dependencies",
                plan.id
            );
        }
    }

    /// Create a branch from the current session at the given message ID.
    ///
    /// Captures the current workspace snapshot, creates a new session with
    /// messages copied up to the branch point, and generates a summary of
    /// the abandoned tail.
    ///
    /// # Errors
    ///
    /// Returns an error if the message doesn't exist or branch creation fails.
    pub async fn branch_at_message(
        &self,
        from_message_id: &str,
    ) -> anyhow::Result<super::branch::BranchResult> {
        // Capture current snapshot for the branch point
        let snapshot_hash = match self.state.snapshot.track().await {
            Ok(hash) => hash,
            Err(e) => {
                tracing::warn!("branch snapshot failed: {e}");
                None
            }
        };

        super::branch::create_branch(&self.state, &self.session_id, from_message_id, snapshot_hash)
            .await
    }

    /// List messages in the current session suitable for selecting a branch point.
    ///
    /// Returns a list of `(message_id, index, role, text_preview)` for each
    /// user message in the session (only user messages are valid branch points).
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails.
    pub fn list_branch_points(&self) -> anyhow::Result<Vec<(String, usize, String)>> {
        let rows = self.state.db.list_messages(&self.session_id)?;
        let mut points = Vec::new();

        for (i, row) in rows.iter().enumerate() {
            // Only user messages are valid branch points
            if row.role != "user" {
                continue;
            }
            let parts: Vec<crate::provider::MessageContent> =
                serde_json::from_str(&row.parts).unwrap_or_default();
            let preview = parts
                .iter()
                .find_map(|p| match p {
                    crate::provider::MessageContent::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let preview =
                if preview.len() > 80 { format!("{}...", &preview[..77]) } else { preview };
            points.push((row.id.clone(), i + 1, preview));
        }

        Ok(points)
    }

    /// Build the session tree for display.
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails.
    pub fn session_tree(&self) -> anyhow::Result<Vec<super::tree::SessionTreeNode>> {
        super::tree::build_session_tree(&self.state.db, &self.state.project_id, &self.session_id)
    }

    /// Build a formatted text representation of the session tree.
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails.
    pub fn session_tree_text(&self) -> anyhow::Result<String> {
        use std::fmt::Write;
        let tree = self.session_tree()?;
        let flat = super::tree::flatten_tree(&tree);

        if flat.is_empty() {
            return Ok("No sessions found.".to_string());
        }

        let mut text = String::from("Session Tree\n\n");
        for (depth, node) in &flat {
            let indent = "  ".repeat(*depth);
            let prefix = if *depth == 0 {
                ""
            } else {
                "\u{251C}\u{2500} " // ├─
            };
            let marker = if node.is_current { "\u{25CF} " } else { "  " }; // ●
            let title =
                if node.session.title.is_empty() { "(untitled)" } else { &node.session.title };
            let count = node.message_count;
            let _ = write!(text, "{indent}{prefix}{marker}{title}  ({count} msgs)");
            if let Some(label) = &node.label {
                let _ = write!(text, "  [{label}]");
            }
            let _ = writeln!(text, "  [{:.8}]", node.session.id);
        }

        Ok(text)
    }

    /// Set or update a label on the current session.
    ///
    /// # Errors
    ///
    /// Returns an error if the database operation fails.
    pub fn set_label(&self, label: &str) -> anyhow::Result<()> {
        self.state.db.upsert_session_label(&self.session_id, label)?;
        Ok(())
    }

    /// Switch to a different session by ID.
    ///
    /// Restores the workspace to the target session's snapshot state and
    /// returns the display messages for the target session.
    ///
    /// # Errors
    ///
    /// Returns an error if the session doesn't exist, snapshot restore fails,
    /// or the session belongs to a different project.
    pub async fn switch_session(
        &mut self,
        target_session_id: &str,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let target = self.state.db.get_session(target_session_id)?;

        // Safety: don't switch to a session from a different project
        if target.project_id != self.state.project_id {
            anyhow::bail!("Cannot switch to a session from a different project");
        }

        let old_session_id = self.session_id.clone();

        // Restore snapshot if the target has one
        if let Some(ref hash) = target.branch_snapshot_hash {
            if let Err(e) = self.state.snapshot.restore(hash).await {
                tracing::warn!(
                    error = %e,
                    snapshot = %hash,
                    "snapshot restore failed during session switch"
                );
            } else {
                self.state.bus.send(BusEvent::SnapshotRestored {
                    session_id: target_session_id.to_string(),
                    snapshot_hash: hash.clone(),
                });
            }
        }

        // Switch the engine to the new session
        self.session_id = target_session_id.to_string();
        self.model_id = target.model_id;
        self.undo_stack.clear();
        self.redo_stack.clear();

        self.state.bus.send(BusEvent::SessionSwitched {
            from_session_id: old_session_id,
            to_session_id: target_session_id.to_string(),
        });

        // Load display messages for the new session
        self.load_display_messages()
    }

    fn plan_store(&self) -> PlanStore {
        PlanStore::new(self.state.project_root.clone())
    }

    fn session_plans(&self) -> anyhow::Result<Vec<ExecutionPlan>> {
        let mut plans = self.plan_store().list_plans()?;
        plans.retain(|plan| plan.session_id == self.session_id);
        Ok(plans)
    }

    fn resolve_plan(&self, plan_id: Option<&str>) -> anyhow::Result<ExecutionPlan> {
        match plan_id {
            Some(id) => {
                let plan = self.plan_store().load_plan(id)?;
                if plan.session_id != self.session_id {
                    anyhow::bail!("plan '{id}' does not belong to the current session");
                }
                Ok(plan)
            }
            None => self
                .session_plans()?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no saved plans found for this session")),
        }
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
        let mut verification_retries = 0usize;
        let mut verification_bonus_retry_granted = false;
        let mut verification_skipped_rounds_since_failure = 0usize;
        let mut verification_failure_summary: Option<VerificationFailureSummary> = None;
        let mut verification_preference: Option<VerificationPreference> = None;
        let mut call_history: HashMap<String, usize> = HashMap::new();
        let token_counter = TokenCounter::for_model(&self.model_id);
        let context_window =
            ModelRegistry::builtin().get(&self.model_id).map_or(200_000, |m| m.context_window);

        loop {
            rounds += 1;
            tracing::debug!(
                round = rounds,
                session_id = %self.session_id,
                "prompt loop round start"
            );
            if rounds > MAX_TOOL_ROUNDS {
                return Err(anyhow::anyhow!(
                    "Tool call loop exceeded {MAX_TOOL_ROUNDS} rounds — possible doom loop"
                ));
            }

            let mut messages = self.assemble_messages()?;
            let system =
                build_system_prompt(&self.state.project_root, &self.state.provider_registry);

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
                                | "lsp_diagnostics"
                                | "lsp_goto_definition"
                                | "lsp_find_references"
                                | "lsp_symbols"
                                | "webfetch"
                                | "question"
                                | "todowrite"
                                | "plan"
                                | "plan_create"
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

            // Separate streaming text between rounds so the TUI doesn't
            // concatenate consecutive round outputs on a single line.
            if rounds > 1 {
                self.state.bus.send(BusEvent::TextDelta {
                    session_id: self.session_id.clone(),
                    message_id: String::new(),
                    delta: "\n\n".to_string(),
                });
            }

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
            tracing::debug!(
                round = rounds,
                tool_call_count = tool_calls.len(),
                text_len = text.len(),
                "LLM response received"
            );
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
                    let allowed =
                        self.state.permissions.check("doom_loop", &tc.name, &description).await;

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
            let mut changed_files = Vec::new();

            // Persist any new "Always Allow" permission rules to the database
            for rule in self.state.permissions.drain_new_rules() {
                let action_str = match rule.action {
                    crate::permission::PermissionAction::Allow => "allow",
                    crate::permission::PermissionAction::Deny => "deny",
                    crate::permission::PermissionAction::Ask => "ask",
                };
                if let Err(e) = self.state.db.upsert_permission_rule(
                    &self.state.project_id,
                    &rule.permission,
                    &rule.pattern,
                    action_str,
                ) {
                    tracing::warn!(
                        permission = %rule.permission,
                        pattern = %rule.pattern,
                        error = %e,
                        "failed to persist permission rule"
                    );
                }
            }

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
                                changed_files = patch.files;
                            }
                            Ok(_) => {} // No files changed
                            Err(e) => tracing::warn!("snapshot patch failed: {e}"),
                        }
                    }
                    Ok(None) => {} // Snapshots disabled
                    Err(e) => tracing::warn!("post-tool snapshot failed: {e}"),
                }
            }
            let verification_scope_files = effective_verification_scope_files(
                &self.state.project_root,
                &changed_files,
                &tool_calls,
                &tool_results,
            );

            let mut result_parts: Vec<MessageContent> = tool_results
                .iter()
                .map(|r| MessageContent::ToolResult {
                    tool_use_id: r.tool_call_id.clone(),
                    content: r.content.clone(),
                    is_error: r.is_error,
                })
                .collect();

            if let Some(error) = verification_retry_scope_stop_error(
                &self.state.project_root,
                verification_preference.as_ref(),
                &verification_scope_files,
            ) {
                result_parts.push(MessageContent::Text { text: error.clone() });
                let result_msg_id = Ulid::new().to_string();
                let result_json = serde_json::to_string(&result_parts)?;
                self.state.db.insert_message(
                    &result_msg_id,
                    &self.session_id,
                    "user",
                    &result_json,
                )?;
                return Err(anyhow::anyhow!(error));
            }

            let verification_outcome = maybe_run_automatic_verification(
                &self.state.project_root,
                &self.state.bus,
                &self.session_id,
                &tool_calls,
                &tool_results,
                &verification_scope_files,
                verification_preference.as_ref(),
            )
            .await?;
            let mut verification_retry_limit =
                MAX_VERIFICATION_RETRIES + usize::from(verification_bonus_retry_granted);

            match &verification_outcome {
                AutomaticVerificationOutcome::Failed(report) => {
                    let previous_failure_summary = verification_failure_summary.clone();
                    let previous_verification_preference = verification_preference.clone();
                    verification_retries += 1;
                    verification_preference =
                        Some(report.retry_preference(&verification_scope_files));
                    let current_failure_summary = report.failure_summary();
                    let retry_scope_relevance = verification_retry_scope_relevance(
                        &self.state.project_root,
                        previous_verification_preference.as_ref(),
                        &verification_scope_files,
                    );
                    result_parts.push(MessageContent::Text {
                        text: build_verification_feedback(
                            report,
                            &verification_scope_files,
                            previous_failure_summary.as_ref(),
                            verification_skipped_rounds_since_failure,
                            retry_scope_relevance,
                            previous_verification_preference.as_ref(),
                        ),
                    });
                    verification_failure_summary = current_failure_summary;
                    verification_skipped_rounds_since_failure = 0;

                    if let Some(error) = verification_retry_stop_error(
                        previous_failure_summary.as_ref(),
                        verification_failure_summary.as_ref(),
                    ) {
                        let result_msg_id = Ulid::new().to_string();
                        let result_json = serde_json::to_string(&result_parts)?;
                        self.state.db.insert_message(
                            &result_msg_id,
                            &self.session_id,
                            "user",
                            &result_json,
                        )?;
                        return Err(anyhow::anyhow!(error));
                    }

                    let retry_budget = verification_retry_budget(
                        previous_failure_summary.as_ref(),
                        verification_failure_summary.as_ref(),
                        retry_scope_relevance,
                        verification_bonus_retry_granted,
                    );
                    verification_bonus_retry_granted = retry_budget.bonus_retry_granted;
                    verification_retry_limit = retry_budget.retry_limit;
                }
                AutomaticVerificationOutcome::Passed => {
                    verification_retries = 0;
                    verification_bonus_retry_granted = false;
                    verification_skipped_rounds_since_failure = 0;
                    verification_failure_summary = None;
                    verification_preference = None;
                }
                AutomaticVerificationOutcome::Skipped => {
                    if let Some(note) = verification_context_preserved_note(
                        verification_failure_summary.as_ref(),
                        verification_preference.as_ref(),
                        verification_skipped_rounds_since_failure,
                    ) {
                        result_parts.push(MessageContent::Text { text: note });
                        verification_skipped_rounds_since_failure += 1;
                    }
                }
            }

            let result_msg_id = Ulid::new().to_string();
            let result_json = serde_json::to_string(&result_parts)?;
            self.state.db.insert_message(&result_msg_id, &self.session_id, "user", &result_json)?;

            if matches!(verification_outcome, AutomaticVerificationOutcome::Failed(_)) {
                if verification_retries > verification_retry_limit {
                    return Err(anyhow::anyhow!(
                        "Automatic verification failed after {verification_retry_limit} retry attempt(s)."
                    ));
                }
                continue;
            }

            // Wait for background agents: if tool calls spawned background team
            // agents, pause and collect their results before the next LLM call.
            // This prevents the LLM from manually polling with team_task/sleep.
            let background_agent_count = tool_calls
                .iter()
                .filter(|tc| {
                    tc.name == "task" && {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.arguments).unwrap_or_default();
                        args.get("background").and_then(serde_json::Value::as_bool).unwrap_or(false)
                            && args.get("team_id").and_then(serde_json::Value::as_str).is_some()
                    }
                })
                .count();

            if background_agent_count > 0 {
                self.wait_for_team_agents(background_agent_count).await?;
            }

            // Check cancellation after tool execution — don't start another round
            if self.cancel_token.is_cancelled() {
                self.state.bus.send(BusEvent::Cancelled { session_id: self.session_id.clone() });
                return Ok(SendMessageResult::Cancelled { partial_text: text });
            }
        }
    }

    /// Wait for background team agents to complete and inject their results.
    ///
    /// Subscribes to bus events and waits for `MessageInjected` events
    /// targeting this session. Each injected message is persisted as a
    /// synthetic user message so the LLM sees the agent's findings.
    ///
    /// Waits until all `expected_count` agents have reported back,
    /// or times out after 5 minutes.
    async fn wait_for_team_agents(&self, expected_count: usize) -> anyhow::Result<()> {
        use tokio::time::{timeout, Duration};

        const AGENT_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

        tracing::info!(
            session_id = %self.session_id,
            expected_count,
            "waiting for background team agents to report back"
        );

        let mut bus_rx = self.state.bus.subscribe();
        let mut received_count = 0u32;
        let mut completed_count = 0usize;

        // Wait for injected messages until timeout or cancellation
        loop {
            if self.cancel_token.is_cancelled() {
                tracing::info!("wait_for_team_agents: cancelled");
                break;
            }

            let event = match timeout(AGENT_TIMEOUT, bus_rx.recv()).await {
                Ok(Ok(event)) => event,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                    tracing::warn!(lagged = n, "bus receiver lagged during agent wait");
                    continue;
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    tracing::warn!("bus closed during agent wait");
                    break;
                }
                Err(_) => {
                    tracing::warn!(
                        "timed out waiting for background agents ({}s)",
                        AGENT_TIMEOUT.as_secs()
                    );
                    // Inject a timeout notice so the LLM knows
                    let timeout_msg = format!(
                        "[System: Background agents timed out after {}s. \
                         Proceed with whatever results have been received so far ({} messages).]",
                        AGENT_TIMEOUT.as_secs(),
                        received_count
                    );
                    let msg_id = Ulid::new().to_string();
                    let parts =
                        serde_json::to_string(&vec![MessageContent::Text { text: timeout_msg }])?;
                    self.state.db.insert_message(&msg_id, &self.session_id, "user", &parts)?;
                    break;
                }
            };

            match event {
                BusEvent::MessageInjected { ref session_id, ref from_agent, ref content }
                    if session_id == &self.session_id =>
                {
                    tracing::info!(
                        from = %from_agent,
                        content_len = content.len(),
                        "received injected message from background agent"
                    );

                    // Persist the injected message as a synthetic user message
                    // so the LLM sees the agent's findings in conversation history.
                    let injected_text = format!("[Message from @{from_agent}]\n\n{content}");
                    let msg_id = Ulid::new().to_string();
                    let parts =
                        serde_json::to_string(&vec![MessageContent::Text { text: injected_text }])?;
                    self.state.db.insert_message(&msg_id, &self.session_id, "user", &parts)?;

                    received_count += 1;
                }
                BusEvent::TeamMemberCompleted { ref session_id, ref agent_name, .. }
                    if session_id == &self.session_id =>
                {
                    completed_count += 1;
                    tracing::info!(
                        agent = %agent_name,
                        completed_count,
                        expected_count,
                        "team member completed"
                    );
                    if completed_count >= expected_count {
                        tracing::info!(
                            received = received_count,
                            completed = completed_count,
                            "all background agents have completed"
                        );
                        break;
                    }
                }
                BusEvent::TeamMemberFailed { ref session_id, ref agent_name, .. }
                    if session_id == &self.session_id =>
                {
                    completed_count += 1;
                    tracing::warn!(
                        agent = %agent_name,
                        completed_count,
                        expected_count,
                        "team member failed"
                    );
                    if completed_count >= expected_count {
                        tracing::info!(
                            received = received_count,
                            completed = completed_count,
                            "all background agents have finished (some failed)"
                        );
                        break;
                    }
                }
                BusEvent::Cancelled { ref session_id } if session_id == &self.session_id => {
                    tracing::info!("wait_for_team_agents: session cancelled");
                    break;
                }
                _ => {} // Ignore unrelated events
            }
        }

        if received_count > 0 {
            tracing::info!(
                received = received_count,
                "background agent wait complete, resuming prompt loop"
            );
        }

        Ok(())
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
        let initial_provider = ModelRegistry::provider_name(&request.model).to_string();
        let initial_model = request.model.clone();
        let mut partial_text = String::new();
        let mut partial_reasoning = String::new();
        let bus = self.state.bus.clone();
        let session_id = self.session_id.clone();
        let cost_tracker = &self.state.cost_tracker;

        let result = self
            .state
            .provider_registry
            .stream_with_fallback_internal(
                crate::provider::registry::FallbackStreamContext {
                    initial_provider: &initial_provider,
                    initial_model: &initial_model,
                    explicit_chain: None,
                    bus: &bus,
                    session_id: &session_id,
                    cancel_token: Some(&self.cancel_token),
                },
                request,
                |event, _completion| match event {
                    StreamEvent::TextDelta(delta) => {
                        if !delta.is_empty() {
                            partial_text.push_str(delta);
                            bus.send(BusEvent::TextDelta {
                                session_id: session_id.clone(),
                                message_id: String::new(),
                                delta: delta.clone(),
                            });
                        }
                    }
                    StreamEvent::ReasoningDelta(delta) => {
                        if !delta.is_empty() {
                            partial_reasoning.push_str(delta);
                            bus.send(BusEvent::ReasoningDelta {
                                session_id: session_id.clone(),
                                delta: delta.clone(),
                            });
                        }
                    }
                    StreamEvent::Usage {
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                        cache_creation_tokens,
                    } => {
                        tracing::debug!(
                            input_tokens,
                            output_tokens,
                            cache_read_tokens,
                            "token usage"
                        );
                        cost_tracker.record(
                            *input_tokens,
                            *output_tokens,
                            *cache_read_tokens,
                            *cache_creation_tokens,
                        );
                        bus.send(BusEvent::TokenUsage {
                            session_id: session_id.clone(),
                            input_tokens: *input_tokens,
                            output_tokens: *output_tokens,
                        });
                        bus.send(BusEvent::CostUpdate {
                            session_id: session_id.clone(),
                            total_cost_usd: cost_tracker.estimated_cost_usd(),
                        });
                    }
                    StreamEvent::ToolCallStart { .. }
                    | StreamEvent::ToolCallDelta { .. }
                    | StreamEvent::Done
                    | StreamEvent::Error(_) => {}
                },
            )
            .await;

        match result {
            Ok(response) => Ok((
                response.text,
                response.reasoning,
                response
                    .tool_calls
                    .into_iter()
                    .map(|tool_call| ToolCall {
                        id: tool_call.id,
                        name: tool_call.name,
                        arguments: tool_call.arguments,
                    })
                    .collect(),
            )),
            Err(err)
                if err.downcast_ref::<crate::provider::registry::StreamCancelled>().is_some() =>
            {
                tracing::info!("stream cancelled by user");
                Err(CancelledError { partial_text, partial_reasoning }.into())
            }
            Err(err) => Err(err),
        }
    }

    /// Stream a completion with retry on transient errors (429, 529, 500, overloaded).
    ///
    /// Uses exponential backoff: 1s, 2s, 4s (max 3 retries).
    /// Cancellation errors are never retried.
    async fn stream_completion_with_retry(
        &self,
        request: CompletionRequest,
    ) -> anyhow::Result<(String, String, Vec<ToolCall>)> {
        self.stream_completion(request).await
    }

    /// Execute a batch of tool calls.
    ///
    /// Read-only (`Safe`) tool calls run concurrently for better latency.
    /// Write/Dangerous tool calls run sequentially to avoid filesystem races.
    /// Permission checks happen sequentially before execution since they
    /// involve user interaction.
    ///
    /// Checks the cancellation token before each execution phase.
    async fn execute_tool_calls(&self, tool_calls: &[ToolCall]) -> Vec<ToolResult> {
        if self.cancel_token.is_cancelled() {
            return tool_calls
                .iter()
                .map(|tc| ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: "Cancelled by user.".into(),
                    is_error: true,
                })
                .collect();
        }

        let ctx = self.state.tool_context(&self.session_id, self.cancel_token.clone());

        // Phase 1: Pre-validate all tool calls (sequential — permission prompts).
        let mut pre_results: Vec<Option<ToolResult>> = vec![None; tool_calls.len()];
        let mut approved: Vec<(usize, Arc<dyn crate::tool::Tool>, serde_json::Value)> = Vec::new();

        for (i, tc) in tool_calls.iter().enumerate() {
            let Some(tool) = self.state.tools.get(&tc.name) else {
                pre_results[i] = Some(ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: format!(
                        "Unknown tool: {}. Available tools: {}",
                        tc.name,
                        self.state.tools.names().join(", ")
                    ),
                    is_error: true,
                });
                continue;
            };

            let args: serde_json::Value = serde_json::from_str(&tc.arguments).unwrap_or_default();

            if self.state.plan_mode.is_plan()
                && tool.permission_level() != crate::tool::PermissionLevel::Safe
            {
                pre_results[i] = Some(ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: format!(
                        "Tool '{}' blocked: currently in PLAN mode (read-only). \
                         Switch to BUILD mode to make changes.",
                        tc.name
                    ),
                    is_error: true,
                });
                continue;
            }

            let schema = tool.parameters_schema();
            if let Err(e) = validate_tool_args(&args, &schema) {
                pre_results[i] = Some(ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: format!("Invalid arguments: {e}"),
                    is_error: true,
                });
                continue;
            }

            let description = tool.describe_invocation(&args);
            tracing::debug!(
                tool = %tc.name,
                description = %description,
                "permission check for tool call"
            );

            // For bash commands, use the actual command as the pattern for
            // fine-grained permission matching. For other tools, use "*".
            let pattern = if tc.name == "bash" {
                args.get("command").and_then(serde_json::Value::as_str).unwrap_or("*")
            } else {
                "*"
            };

            let allowed = if tool.permission_level() == crate::tool::PermissionLevel::Safe {
                true
            } else {
                // Check the tool-level permission first
                let tool_allowed =
                    self.state.permissions.check(&tc.name, pattern, &description).await;
                if !tool_allowed {
                    false
                } else if tc.name == "bash" {
                    // For bash commands, additionally check for external directory access
                    let command =
                        args.get("command").and_then(serde_json::Value::as_str).unwrap_or("");
                    if crate::permission::path::command_touches_external_paths(
                        command,
                        &self.state.project_root,
                    ) {
                        // Command references paths outside the project — check external_directory permission
                        let ext_desc =
                            format!("Command accesses paths outside the project: {}", &description);
                        self.state.permissions.check("external_directory", command, &ext_desc).await
                    } else {
                        true
                    }
                } else {
                    true
                }
            };

            if !allowed {
                pre_results[i] = Some(ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: format!("Permission denied for tool '{}'", tc.name),
                    is_error: true,
                });
                continue;
            }

            approved.push((i, Arc::clone(tool), args));
        }

        let mut safe_batch: Vec<(usize, Arc<dyn crate::tool::Tool>, serde_json::Value)> =
            Vec::new();

        for (i, tool, args) in approved {
            if tool.permission_level() == crate::tool::PermissionLevel::Safe {
                safe_batch.push((i, tool, args));
                continue;
            }

            execute_safe_batch(
                &mut safe_batch,
                tool_calls,
                &ctx,
                &self.session_id,
                &self.state.bus,
                &mut pre_results,
                self.cancel_token.is_cancelled(),
            )
            .await;

            if self.cancel_token.is_cancelled() {
                pre_results[i] = Some(ToolResult {
                    tool_call_id: tool_calls[i].id.clone(),
                    content: "Cancelled by user.".into(),
                    is_error: true,
                });
                for (j, _, _) in safe_batch.drain(..) {
                    pre_results[j] = Some(ToolResult {
                        tool_call_id: tool_calls[j].id.clone(),
                        content: "Cancelled by user.".into(),
                        is_error: true,
                    });
                }
                continue;
            }

            let tc = &tool_calls[i];
            self.state.bus.send(BusEvent::ToolCallStarted {
                session_id: self.session_id.clone(),
                tool_name: tc.name.clone(),
                tool_call_id: tc.id.clone(),
            });

            let result = execute_single_tool(&*tool, args, &ctx, &tc.id, &tc.name).await;

            self.state.bus.send(BusEvent::ToolCallCompleted {
                session_id: self.session_id.clone(),
                tool_name: tc.name.clone(),
                tool_call_id: tc.id.clone(),
                is_error: result.is_error,
            });

            pre_results[i] = Some(truncate_result(result));
        }

        execute_safe_batch(
            &mut safe_batch,
            tool_calls,
            &ctx,
            &self.session_id,
            &self.state.bus,
            &mut pre_results,
            self.cancel_token.is_cancelled(),
        )
        .await;

        pre_results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                r.unwrap_or_else(|| ToolResult {
                    tool_call_id: tool_calls[i].id.clone(),
                    content: "Internal error: tool result not populated".into(),
                    is_error: true,
                })
            })
            .collect()
    }
}

fn plan_status_label(status: &PlanStatus) -> &'static str {
    match status {
        PlanStatus::Draft => "draft",
        PlanStatus::Approved => "approved",
        PlanStatus::Executing => "executing",
        PlanStatus::Completed => "completed",
        PlanStatus::Failed => "failed",
        PlanStatus::Cancelled => "cancelled",
    }
}

fn format_plan_details(plan: &ExecutionPlan, store: &PlanStore) -> String {
    let mut text = summarize_plan(plan);
    let _ = writeln!(text, "\nPlan file: {}", store.plan_path(&plan.id).display());
    if !plan.dependencies.is_empty() {
        let _ = writeln!(text, "\nDependencies:");
        for dependency in &plan.dependencies {
            let _ = writeln!(text, "- {} -> {}", dependency.prerequisite, dependency.dependent);
        }
    }
    text.trim_end().to_string()
}

fn next_ready_step_index(plan: &ExecutionPlan) -> Option<usize> {
    plan.steps.iter().enumerate().find_map(|(index, step)| {
        if !matches!(step.status, StepStatus::Pending) {
            return None;
        }

        let ready =
            plan.dependencies.iter().filter(|dependency| dependency.dependent == step.id).all(
                |dependency| {
                    plan.steps
                        .iter()
                        .find(|candidate| candidate.id == dependency.prerequisite)
                        .is_some_and(|candidate| matches!(candidate.status, StepStatus::Completed))
                },
            );

        ready.then_some(index)
    })
}

fn build_plan_step_prompt(plan: &ExecutionPlan, step: &crate::plan::PlanStep) -> String {
    let affected_files = if step.affected_files.is_empty() {
        "none specified".to_string()
    } else {
        step.affected_files
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let prerequisites = plan
        .dependencies
        .iter()
        .filter(|dependency| dependency.dependent == step.id)
        .map(|dependency| dependency.prerequisite.clone())
        .collect::<Vec<_>>();
    let prerequisite_text =
        if prerequisites.is_empty() { "none".to_string() } else { prerequisites.join(", ") };

    format!(
        "Execute approved plan '{title}' ({plan_id}), step '{step_title}' ({step_id}).\n\
         Focus only on this step.\n\
         Do not start later plan steps.\n\
         Description: {description}\n\
         Prerequisites already completed: {prerequisites}\n\
         Affected files: {files}\n\
         When finished, stop after reporting what you changed for this step.",
        title = plan.title,
        plan_id = plan.id,
        step_title = step.title,
        step_id = step.id,
        description = if step.description.trim().is_empty() {
            "(no additional description)"
        } else {
            &step.description
        },
        prerequisites = prerequisite_text,
        files = affected_files,
    )
}

async fn rollback_plan_step(
    snapshot: &crate::snapshot::SnapshotManager,
    step: &crate::plan::PlanStep,
) -> anyhow::Result<String> {
    let Some(checkpoint) = &step.checkpoint else {
        return Ok("no checkpoint captured; workspace left as-is".to_string());
    };

    match &checkpoint.snapshot {
        CheckpointData::WorkspaceSnapshot { hash } => {
            snapshot.restore(hash).await?;
            Ok(format!("workspace restored to checkpoint {hash}"))
        }
        CheckpointData::FileSnapshots(_) => {
            Ok("file-snapshot rollback not implemented yet; workspace left as-is".to_string())
        }
    }
}

fn mark_failed_plan(
    store: &PlanStore,
    current: &ExecutionPlan,
    failed_step_id: &str,
    error: &str,
    rollback_detail: &str,
) -> Result<ExecutionPlan, crate::plan::PlanError> {
    let failure_reason = format!("{error}; {rollback_detail}");
    let mut updated = store.apply_patch(
        &current.id,
        PlanPatch {
            plan_status: Some(PlanStatus::Failed),
            step_id: Some(failed_step_id.to_string()),
            step_status: Some(StepStatus::Failed(failure_reason)),
            ..PlanPatch::default()
        },
    )?;

    let blocked = blocked_dependents(&updated, failed_step_id);
    if !blocked.is_empty() {
        for step in &mut updated.steps {
            if blocked.contains(&step.id) && matches!(step.status, StepStatus::Pending) {
                step.status = StepStatus::Skipped;
            }
        }
        updated.updated_at = Utc::now();
        store.save_plan(&updated)?;
    }

    Ok(updated)
}

fn mark_cancelled_plan(
    store: &PlanStore,
    current: &ExecutionPlan,
    cancelled_step_id: &str,
    rollback_detail: &str,
) -> Result<ExecutionPlan, crate::plan::PlanError> {
    store.apply_patch(
        &current.id,
        PlanPatch {
            plan_status: Some(PlanStatus::Cancelled),
            step_id: Some(cancelled_step_id.to_string()),
            step_status: Some(StepStatus::Failed(format!("step cancelled; {rollback_detail}"))),
            ..PlanPatch::default()
        },
    )
}

fn blocked_dependents(plan: &ExecutionPlan, failed_step_id: &str) -> HashSet<String> {
    let mut blocked = HashSet::new();
    let mut queue = VecDeque::from([failed_step_id.to_string()]);

    while let Some(step_id) = queue.pop_front() {
        for dependency in plan.dependencies.iter().filter(|dep| dep.prerequisite == step_id) {
            if blocked.insert(dependency.dependent.clone()) {
                queue.push_back(dependency.dependent.clone());
            }
        }
    }

    blocked
}

async fn execute_safe_batch(
    safe_batch: &mut Vec<(usize, Arc<dyn crate::tool::Tool>, serde_json::Value)>,
    tool_calls: &[ToolCall],
    ctx: &crate::tool::ToolContext,
    session_id: &str,
    bus: &crate::bus::Bus,
    pre_results: &mut [Option<ToolResult>],
    cancelled: bool,
) {
    if safe_batch.is_empty() {
        return;
    }

    if cancelled {
        for (i, _, _) in safe_batch.drain(..) {
            pre_results[i] = Some(ToolResult {
                tool_call_id: tool_calls[i].id.clone(),
                content: "Cancelled by user.".into(),
                is_error: true,
            });
        }
        return;
    }

    let batch = std::mem::take(safe_batch);
    let futs: Vec<_> = batch
        .into_iter()
        .map(|(i, tool, args)| {
            let tc = &tool_calls[i];
            let ctx = ctx.clone();
            let session_id = session_id.to_string();
            let bus = bus.clone();
            let tc_name = tc.name.clone();
            let tc_id = tc.id.clone();

            async move {
                bus.send(BusEvent::ToolCallStarted {
                    session_id: session_id.clone(),
                    tool_name: tc_name.clone(),
                    tool_call_id: tc_id.clone(),
                });

                let result = execute_single_tool(&*tool, args, &ctx, &tc_id, &tc_name).await;

                bus.send(BusEvent::ToolCallCompleted {
                    session_id,
                    tool_name: tc_name,
                    tool_call_id: tc_id,
                    is_error: result.is_error,
                });

                (i, result)
            }
        })
        .collect();

    let concurrent_results = futures::future::join_all(futs).await;
    for (i, result) in concurrent_results {
        pre_results[i] = Some(truncate_result(result));
    }
}

/// Execute a single tool call with panic safety.
async fn execute_single_tool(
    tool: &dyn crate::tool::Tool,
    args: serde_json::Value,
    ctx: &crate::tool::ToolContext,
    tool_call_id: &str,
    tool_name: &str,
) -> ToolResult {
    let exec_result = std::panic::AssertUnwindSafe(tool.execute(args, ctx));
    match futures::FutureExt::catch_unwind(exec_result).await {
        Ok(Ok(output)) => ToolResult {
            tool_call_id: tool_call_id.to_string(),
            content: output.content,
            is_error: output.is_error,
        },
        Ok(Err(e)) => ToolResult {
            tool_call_id: tool_call_id.to_string(),
            content: format!("Tool execution error: {e}"),
            is_error: true,
        },
        Err(_panic) => ToolResult {
            tool_call_id: tool_call_id.to_string(),
            content: format!("Tool '{tool_name}' panicked during execution"),
            is_error: true,
        },
    }
}

async fn maybe_run_automatic_verification(
    project_root: &std::path::Path,
    bus: &crate::bus::Bus,
    session_id: &str,
    tool_calls: &[ToolCall],
    tool_results: &[ToolResult],
    changed_files: &[String],
    preference: Option<&VerificationPreference>,
) -> anyhow::Result<AutomaticVerificationOutcome> {
    if !should_run_automatic_verification(tool_calls, tool_results, changed_files) {
        return Ok(AutomaticVerificationOutcome::Skipped);
    }

    let Some(command) = detect_verification_command(project_root, changed_files, preference) else {
        return Ok(AutomaticVerificationOutcome::Skipped);
    };

    let command_text = command.display();
    bus.send(BusEvent::VerificationStarted {
        session_id: session_id.to_string(),
        command: command_text.clone(),
    });

    let report = run_verification_command(project_root, &command).await?;
    let summary = report.summary();
    bus.send(BusEvent::VerificationCompleted {
        session_id: session_id.to_string(),
        command: report.command.clone(),
        success: report.success,
        summary: summary.clone(),
    });

    if report.success {
        Ok(AutomaticVerificationOutcome::Passed)
    } else {
        Ok(AutomaticVerificationOutcome::Failed(report))
    }
}

fn should_run_automatic_verification(
    tool_calls: &[ToolCall],
    tool_results: &[ToolResult],
    changed_files: &[String],
) -> bool {
    if !changed_files.is_empty() {
        return true;
    }

    tool_calls.iter().zip(tool_results.iter()).any(|(tool_call, result)| {
        !result.is_error && matches!(tool_call.name.as_str(), "write" | "edit" | "fast_apply")
    })
}

enum AutomaticVerificationOutcome {
    Skipped,
    Passed,
    Failed(crate::verification::VerificationReport),
}

fn verification_retry_stop_error(
    previous_failure: Option<&VerificationFailureSummary>,
    current_failure: Option<&VerificationFailureSummary>,
) -> Option<String> {
    let (Some(previous), Some(current)) = (previous_failure, current_failure) else {
        return None;
    };

    if current.same_family_as(previous) {
        return Some(format!(
            "Automatic verification retry did not change the failure signature: {}.",
            current.kind.description()
        ));
    }

    None
}

fn verification_retry_scope_stop_error(
    project_root: &std::path::Path,
    preference: Option<&VerificationPreference>,
    changed_files: &[String],
) -> Option<String> {
    let preference = preference?;
    match verification_retry_scope_relevance(project_root, Some(preference), changed_files)? {
        RetryChangeRelevance::Relevant | RetryChangeRelevance::Unknown => None,
        RetryChangeRelevance::Irrelevant => Some(format!(
            "Automatic verification retry did not touch files relevant to the failing verification scope: {}.",
            preference.scope_summary()
        )),
    }
}

fn verification_retry_scope_relevance(
    project_root: &std::path::Path,
    preference: Option<&VerificationPreference>,
    changed_files: &[String],
) -> Option<RetryChangeRelevance> {
    Some(preference?.retry_change_relevance(project_root, changed_files))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VerificationRetryBudget {
    retry_limit: usize,
    bonus_retry_granted: bool,
}

fn verification_retry_budget(
    previous_failure: Option<&VerificationFailureSummary>,
    current_failure: Option<&VerificationFailureSummary>,
    retry_scope_relevance: Option<RetryChangeRelevance>,
    bonus_retry_granted: bool,
) -> VerificationRetryBudget {
    let earned_bonus_retry = !bonus_retry_granted
        && matches!(retry_scope_relevance, Some(RetryChangeRelevance::Relevant))
        && matches!(
            (previous_failure, current_failure),
            (Some(previous), Some(current)) if !current.same_family_as(previous)
        );
    let bonus_retry_granted = bonus_retry_granted || earned_bonus_retry;

    VerificationRetryBudget {
        retry_limit: MAX_VERIFICATION_RETRIES
            + usize::from(bonus_retry_granted) * MAX_VERIFICATION_BONUS_RETRIES,
        bonus_retry_granted,
    }
}

fn effective_verification_scope_files(
    project_root: &std::path::Path,
    changed_files: &[String],
    tool_calls: &[ToolCall],
    tool_results: &[ToolResult],
) -> Vec<String> {
    let mut scope_files = Vec::new();
    let mut seen = HashSet::new();

    for path in changed_files.iter().cloned().chain(infer_scope_files_from_tool_calls(
        project_root,
        tool_calls,
        tool_results,
    )) {
        if seen.insert(path.clone()) {
            scope_files.push(path);
        }
    }

    scope_files
}

fn infer_scope_files_from_tool_calls<'a>(
    project_root: &'a std::path::Path,
    tool_calls: &'a [ToolCall],
    tool_results: &'a [ToolResult],
) -> impl Iterator<Item = String> + 'a {
    tool_calls.iter().zip(tool_results.iter()).filter_map(move |(tool_call, result)| {
        if result.is_error || !matches!(tool_call.name.as_str(), "write" | "edit" | "fast_apply") {
            return None;
        }

        let args: serde_json::Value = serde_json::from_str(&tool_call.arguments).ok()?;
        let file_path = args.get("file_path")?.as_str()?;
        let path = std::path::Path::new(file_path);

        path.strip_prefix(project_root).map_or_else(
            |_| {
                Some(if path.is_absolute() {
                    file_path.to_string()
                } else {
                    project_root.join(path).display().to_string()
                })
            },
            |relative| Some(relative.to_string_lossy().replace('\\', "/")),
        )
    })
}

fn build_verification_feedback(
    report: &crate::verification::VerificationReport,
    changed_files: &[String],
    previous_failure: Option<&VerificationFailureSummary>,
    skipped_rounds_since_previous_failure: usize,
    retry_scope_relevance: Option<RetryChangeRelevance>,
    retry_scope: Option<&VerificationPreference>,
) -> String {
    let scope = if changed_files.is_empty() {
        "Verification scope: unknown.".to_string()
    } else {
        format!("Verification scope: {}.", changed_files.join(", "))
    };
    let retry_status = match (report.failure_summary(), previous_failure) {
        (Some(current), Some(previous)) if current.same_family_as(previous) => {
            format!("Retry status: the same {} is still failing.", current.kind.description())
        }
        (Some(current), Some(previous)) if current.kind == previous.kind => format!(
            "Retry status: verification is still failing, but the {} signature changed.",
            current.kind.description()
        ),
        (Some(current), Some(_)) => format!(
            "Retry status: verification uncovered a different failure: {}.",
            current.kind.description()
        ),
        (Some(current), None) => format!("Failure classification: {}.", current.kind.description()),
        (None, _) => String::new(),
    };
    let retry_scope_status = match (retry_scope_relevance, retry_scope) {
        (Some(RetryChangeRelevance::Relevant), Some(scope)) => format!(
            "Retry scope assessment: the latest edits touched the failing verification scope: {}.",
            scope.scope_summary()
        ),
        (Some(RetryChangeRelevance::Irrelevant), Some(scope)) => format!(
            "Retry scope assessment: the latest edits did not touch the failing verification scope: {}.",
            scope.scope_summary()
        ),
        (Some(RetryChangeRelevance::Unknown), Some(scope)) => format!(
            "Retry scope assessment: unknown because no changed-file snapshot was available. Failing verification scope: {}.",
            scope.scope_summary()
        ),
        _ => String::new(),
    };
    let continuity_status = if skipped_rounds_since_previous_failure == 0 {
        String::new()
    } else {
        format!(
            "Retry continuity: {skipped_rounds_since_previous_failure} intermediate tool round(s) did not run verification, so this is continuing the previous failing verification thread."
        )
    };

    format!(
        "Automatic verification failed after the tool changes.\n\n\
         Failed command: {}\n\
         {}\n\n\
         {}\n\n\
         {}\n\n\
         {}\n\n\
         {}\n\n\
         Fix the verification failure with the minimum necessary changes, then stop so verification can rerun.",
        report.command,
        scope,
        retry_status,
        retry_scope_status,
        continuity_status,
        report.summary()
    )
}

fn verification_context_preserved_note(
    previous_failure: Option<&VerificationFailureSummary>,
    retry_scope: Option<&VerificationPreference>,
    skipped_rounds_since_failure: usize,
) -> Option<String> {
    let previous_failure = previous_failure?;
    let retry_scope = retry_scope?;
    Some(format!(
        "Automatic verification did not run after this tool round. Previous failing verification context remains active: {} in scope {}. Skipped verification rounds since that failure: {}.",
        previous_failure.kind.description(),
        retry_scope.scope_summary(),
        skipped_rounds_since_failure + 1,
    ))
}

/// Truncate very large tool outputs to keep context manageable.
fn truncate_result(result: ToolResult) -> ToolResult {
    use std::fmt::Write;
    if result.content.len() > 50_000 {
        let mut s = result.content[..25_000].to_string();
        let _ = write!(s, "\n\n... [truncated {} bytes] ...\n\n", result.content.len() - 50_000);
        s.push_str(&result.content[result.content.len() - 25_000..]);
        ToolResult { content: s, ..result }
    } else {
        result
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use flok_db::Db;
    use tempfile::TempDir;

    use crate::bus::Bus;
    use crate::config::FlokConfig;
    use crate::lsp::LspManager;
    use crate::plan::{Dependency, NewExecutionPlan, NewPlanStep, PlanStatus, StepStatus};
    use crate::provider::mock::{MockProvider, MockToolCall, MockTurn};
    use crate::provider::ProviderRegistry;
    use crate::session::PlanMode;
    use crate::snapshot::SnapshotManager;
    use crate::token::CostTracker;
    use crate::tool::{
        PermissionLevel, PermissionManager, ReadTool, Tool, ToolContext, ToolOutput, ToolRegistry,
        WriteTool,
    };

    struct RecordingTool {
        name: &'static str,
        permission: PermissionLevel,
        log: Arc<tokio::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl Tool for RecordingTool {
        fn name(&self) -> &'static str {
            self.name
        }

        fn description(&self) -> &'static str {
            self.name
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {},
            })
        }

        fn permission_level(&self) -> PermissionLevel {
            self.permission
        }

        async fn execute(
            &self,
            _args: serde_json::Value,
            _ctx: &ToolContext,
        ) -> anyhow::Result<ToolOutput> {
            self.log.lock().await.push(self.name.to_string());
            Ok(ToolOutput::success(self.name))
        }
    }

    fn test_engine_with_tools(
        temp_dir: &TempDir,
        provider: &Arc<MockProvider>,
        tools: ToolRegistry,
    ) -> SessionEngine {
        let project_root = std::fs::canonicalize(temp_dir.path()).expect("canonical project root");
        let db = Db::open_in_memory().expect("in-memory db");
        let project_id = "test-project";
        db.get_or_create_project(project_id, project_root.to_str().expect("project root utf8"))
            .expect("create project");

        let snapshot = Arc::new(SnapshotManager::new("test-session", project_root.clone()));
        let lsp = Arc::new(LspManager::disabled(project_root.clone()));
        let provider_concrete: Arc<MockProvider> = Arc::clone(provider);
        let provider_dyn: Arc<dyn crate::provider::Provider> = provider_concrete;
        let mut provider_registry = ProviderRegistry::new();
        provider_registry.insert(
            "mock",
            Arc::clone(&provider_dyn),
            Some("mock/test-model".into()),
            3,
        );
        let provider_registry = Arc::new(provider_registry);
        let state = AppState::new(
            db,
            FlokConfig::default(),
            provider_dyn,
            provider_registry,
            tools,
            Bus::new(64),
            PermissionManager::auto_approve(),
            CostTracker::new("test-model"),
            PlanMode::new(),
            project_root,
            project_id.to_string(),
            snapshot,
            lsp,
        );

        SessionEngine::new(state, "mock/test-model".to_string()).expect("create session engine")
    }

    fn write_rust_fixture(temp_dir: &TempDir) {
        std::fs::create_dir_all(temp_dir.path().join("src")).expect("create src");
        std::fs::write(
            temp_dir.path().join("Cargo.toml"),
            r#"[package]
name = "verify_fixture"
version = "0.1.0"
edition = "2021"
"#,
        )
        .expect("write cargo toml");
        std::fs::write(temp_dir.path().join("src/lib.rs"), "pub fn value() -> usize {\n    1\n}\n")
            .expect("write lib.rs");
    }

    fn write_rust_workspace_fixture(temp_dir: &TempDir) {
        std::fs::create_dir_all(temp_dir.path().join("crates/app/src")).expect("create src");
        std::fs::write(
            temp_dir.path().join("Cargo.toml"),
            r#"[workspace]
members = ["crates/app"]
"#,
        )
        .expect("write workspace cargo toml");
        std::fs::write(
            temp_dir.path().join("crates/app/Cargo.toml"),
            r#"[package]
name = "verify_fixture"
version = "0.1.0"
edition = "2021"
"#,
        )
        .expect("write crate cargo toml");
        std::fs::write(
            temp_dir.path().join("crates/app/src/lib.rs"),
            "pub fn value() -> usize {\n    1\n}\n",
        )
        .expect("write crate lib.rs");
    }

    #[tokio::test]
    async fn execute_tool_calls_preserves_write_before_following_safe_tools() {
        let temp_dir = TempDir::new().expect("temp dir");
        let log = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let provider = Arc::new(MockProvider::new());

        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(RecordingTool {
            name: "write_like",
            permission: PermissionLevel::Write,
            log: Arc::clone(&log),
        }));
        tools.register(Arc::new(RecordingTool {
            name: "safe_like",
            permission: PermissionLevel::Safe,
            log: Arc::clone(&log),
        }));

        provider.push_turn(MockTurn::ToolCalls(vec![
            MockToolCall { name: "write_like".into(), arguments: serde_json::json!({}) },
            MockToolCall { name: "safe_like".into(), arguments: serde_json::json!({}) },
        ]));
        provider.push_turn(MockTurn::Text("done".into()));

        let mut engine = test_engine_with_tools(&temp_dir, &provider, tools);
        let result = engine.send_message("run tools").await.expect("send message succeeds");

        assert!(matches!(result, SendMessageResult::Complete(ref text) if text == "done"));
        assert_eq!(log.lock().await.as_slice(), ["write_like", "safe_like"]);
    }

    #[tokio::test]
    async fn approve_plan_marks_latest_plan_approved() {
        let temp_dir = TempDir::new().expect("temp dir");
        let provider = Arc::new(MockProvider::new());
        let engine = test_engine_with_tools(&temp_dir, &provider, ToolRegistry::new());
        let store = engine.plan_store();

        let created = store
            .create_plan(NewExecutionPlan {
                session_id: engine.session_id().to_string(),
                title: "Ship plan approvals".to_string(),
                description: String::new(),
                steps: vec![NewPlanStep {
                    id: Some("step-1".to_string()),
                    title: "Add runtime command".to_string(),
                    description: String::new(),
                    affected_files: Vec::new(),
                    agent_type: "build".to_string(),
                    estimated_tokens: None,
                }],
                dependencies: Vec::new(),
            })
            .expect("create plan");

        let text = engine.approve_plan(None).expect("approve plan");
        let loaded = store.load_plan(&created.id).expect("load approved plan");

        assert!(text.contains("Plan approved."));
        assert!(matches!(loaded.status, PlanStatus::Approved));
    }

    #[tokio::test]
    async fn execute_plan_runs_steps_and_marks_completed() {
        let temp_dir = TempDir::new().expect("temp dir");
        let provider = Arc::new(MockProvider::new());
        provider.push_turn(MockTurn::Text("implemented step 1".into()));
        provider.push_turn(MockTurn::Text("implemented step 2".into()));

        let mut engine = test_engine_with_tools(&temp_dir, &provider, ToolRegistry::new());
        let store = engine.plan_store();
        let created = store
            .create_plan(NewExecutionPlan {
                session_id: engine.session_id().to_string(),
                title: "Execute plan".to_string(),
                description: "Run two sequential steps".to_string(),
                steps: vec![
                    NewPlanStep {
                        id: Some("step-1".to_string()),
                        title: "First".to_string(),
                        description: "Do the first thing".to_string(),
                        affected_files: Vec::new(),
                        agent_type: "build".to_string(),
                        estimated_tokens: None,
                    },
                    NewPlanStep {
                        id: Some("step-2".to_string()),
                        title: "Second".to_string(),
                        description: "Do the second thing".to_string(),
                        affected_files: Vec::new(),
                        agent_type: "build".to_string(),
                        estimated_tokens: None,
                    },
                ],
                dependencies: vec![Dependency {
                    prerequisite: "step-1".to_string(),
                    dependent: "step-2".to_string(),
                }],
            })
            .expect("create plan");

        engine.approve_plan(Some(&created.id)).expect("approve");
        let result = engine.execute_plan(Some(&created.id)).await.expect("execute");
        let loaded = store.load_plan(&created.id).expect("reload");

        assert!(result.contains("Plan executed successfully."));
        assert!(matches!(loaded.status, PlanStatus::Completed));
        assert!(loaded.steps.iter().all(|step| matches!(step.status, StepStatus::Completed)));
    }

    #[tokio::test]
    async fn automatic_verification_runs_after_write_tool_success() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_fixture(&temp_dir);

        let provider = Arc::new(MockProvider::new());
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    2\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::Text("done".into()));

        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(WriteTool));

        let mut engine = test_engine_with_tools(&temp_dir, &provider, tools);
        let mut bus_rx = engine.state.bus.subscribe();
        let result = engine.send_message("update value").await.expect("send message succeeds");

        assert!(matches!(result, SendMessageResult::Complete(ref text) if text == "done"));

        let mut saw_success = false;
        while let Ok(event) = bus_rx.try_recv() {
            if let BusEvent::VerificationCompleted { success, summary, .. } = event {
                saw_success = success && summary.contains("Automatic verification passed.");
            }
        }
        assert!(saw_success, "expected verification success event");
    }

    #[tokio::test]
    async fn automatic_verification_feedback_allows_single_self_fix_round() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_fixture(&temp_dir);

        let provider = Arc::new(MockProvider::new());
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    let broken = ;\n    2\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    2\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::Text("done".into()));

        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(WriteTool));

        let mut engine = test_engine_with_tools(&temp_dir, &provider, tools);
        let result = engine
            .send_message("repair after verification failure")
            .await
            .expect("self-fix succeeds");

        assert!(matches!(result, SendMessageResult::Complete(ref text) if text == "done"));
        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("src/lib.rs")).expect("read final file"),
            "pub fn value() -> usize {\n    2\n}\n"
        );
    }

    #[tokio::test]
    async fn automatic_verification_allows_bonus_retry_for_relevant_different_failure() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_fixture(&temp_dir);

        let provider = Arc::new(MockProvider::new());
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    let broken = ;\n    2\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    let still_broken: usize = \"oops\";\n    still_broken\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    2\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::Text("done".into()));

        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(WriteTool));

        let mut engine = test_engine_with_tools(&temp_dir, &provider, tools);
        let result = engine
            .send_message("fix a changed verification failure")
            .await
            .expect("bonus retry should allow a second repair round");

        assert!(matches!(result, SendMessageResult::Complete(ref text) if text == "done"));
        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("src/lib.rs")).expect("read final file"),
            "pub fn value() -> usize {\n    2\n}\n"
        );
    }

    #[tokio::test]
    async fn automatic_verification_stops_after_bonus_retry_limit() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_fixture(&temp_dir);

        let provider = Arc::new(MockProvider::new());
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    let broken = ;\n    2\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    let still_broken: usize = \"oops\";\n    still_broken\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    missing_value\n}\n"
            }),
        }]));

        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(WriteTool));

        let mut engine = test_engine_with_tools(&temp_dir, &provider, tools);
        let error = engine
            .send_message("keep failing verification after the bonus retry")
            .await
            .expect_err("verification should stop after the bonus retry budget");

        assert!(error.to_string().contains("Automatic verification failed after 2 retry attempt"));
    }

    #[tokio::test]
    async fn automatic_verification_stops_early_on_unchanged_failure_signature() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_fixture(&temp_dir);

        let provider = Arc::new(MockProvider::new());
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    let broken = ;\n    2\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    let still_broken = ;\n    3\n}\n"
            }),
        }]));

        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(WriteTool));

        let mut engine = test_engine_with_tools(&temp_dir, &provider, tools);
        let error = engine
            .send_message("keep hitting the same verification failure")
            .await
            .expect_err("verification churn should stop early");

        assert!(error
            .to_string()
            .contains("Automatic verification retry did not change the failure signature"));
    }

    #[tokio::test]
    async fn automatic_verification_skipped_round_preserves_failure_state() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_fixture(&temp_dir);

        let provider = Arc::new(MockProvider::new());
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    let broken = ;\n    2\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "read".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs"
            }),
        }]));
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    let still_broken = ;\n    3\n}\n"
            }),
        }]));

        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(WriteTool));
        tools.register(Arc::new(ReadTool));

        let mut engine = test_engine_with_tools(&temp_dir, &provider, tools);
        let error = engine
            .send_message("inspect before retrying verification")
            .await
            .expect_err("a read-only detour should not clear verification failure state");

        assert!(error
            .to_string()
            .contains("Automatic verification retry did not change the failure signature"));
        let messages = engine
            .state
            .db
            .list_messages(engine.session_id())
            .expect("list messages after skipped verification round");
        assert!(messages.iter().any(|message| {
            message.parts.contains(
                "Automatic verification did not run after this tool round. Previous failing verification context remains active"
            )
        }));
    }

    #[tokio::test]
    async fn automatic_verification_pass_resets_failure_state() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_fixture(&temp_dir);

        let provider = Arc::new(MockProvider::new());
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    let broken = ;\n    2\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    2\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    let broken = ;\n    2\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn value() -> usize {\n    2\n}\n"
            }),
        }]));
        provider.push_turn(MockTurn::Text("done".into()));

        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(WriteTool));

        let mut engine = test_engine_with_tools(&temp_dir, &provider, tools);
        let result = engine
            .send_message("fail, pass, then fail again")
            .await
            .expect("a passing verification should reset retry state");

        assert!(matches!(result, SendMessageResult::Complete(ref text) if text == "done"));
    }

    #[test]
    fn verification_retry_scope_stop_error_detects_unrelated_retry_changes() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_workspace_fixture(&temp_dir);
        let report = crate::verification::VerificationReport {
            executed_command: crate::verification::detect_command(
                temp_dir.path(),
                &[temp_dir.path().join("crates/app/src/lib.rs").display().to_string()],
            )
            .expect("verification command"),
            command: "cargo check --manifest-path crates/app/Cargo.toml".to_string(),
            success: false,
            exit_code: Some(101),
            output: "error: expected expression".to_string(),
        };
        let preference = report.retry_preference(&[temp_dir
            .path()
            .join("crates/app/src/lib.rs")
            .display()
            .to_string()]);

        let error = verification_retry_scope_stop_error(
            temp_dir.path(),
            Some(&preference),
            &[temp_dir.path().join("README.md").display().to_string()],
        )
        .expect("unrelated retry changes should stop");

        assert!(error.contains(
            "Automatic verification retry did not touch files relevant to the failing verification scope"
        ));
    }

    #[tokio::test]
    async fn automatic_verification_retry_reuses_previous_rust_scope() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_workspace_fixture(&temp_dir);
        std::fs::write(
            temp_dir.path().join("crates/app/src/lib.rs"),
            "pub fn value() -> usize {\n    let broken = ;\n    2\n}\n",
        )
        .expect("write broken file");

        let bus = Bus::new(8);
        let mut bus_rx = bus.subscribe();
        let tool_calls = vec![ToolCall {
            id: "tool-1".to_string(),
            name: "write".to_string(),
            arguments: "{}".to_string(),
        }];
        let tool_results = vec![ToolResult {
            tool_call_id: "tool-1".to_string(),
            content: "updated".to_string(),
            is_error: false,
        }];

        let first_scope = vec![temp_dir.path().join("crates/app/src/lib.rs").display().to_string()];
        let first = maybe_run_automatic_verification(
            temp_dir.path(),
            &bus,
            "session-1",
            &tool_calls,
            &tool_results,
            &first_scope,
            None,
        )
        .await
        .expect("first verification");

        let AutomaticVerificationOutcome::Failed(first_report) = first else {
            panic!("expected failed targeted verification");
        };

        std::fs::write(
            temp_dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\"]\n# retry touched root manifest\n",
        )
        .expect("touch root manifest");

        let second_scope = vec![temp_dir.path().join("Cargo.toml").display().to_string()];
        let second = maybe_run_automatic_verification(
            temp_dir.path(),
            &bus,
            "session-1",
            &tool_calls,
            &tool_results,
            &second_scope,
            Some(&first_report.retry_preference(&first_scope)),
        )
        .await
        .expect("second verification");

        assert!(matches!(second, AutomaticVerificationOutcome::Failed(_)));

        let mut commands = Vec::new();
        while let Ok(event) = bus_rx.try_recv() {
            if let BusEvent::VerificationStarted { command, .. } = event {
                commands.push(command);
            }
        }

        assert_eq!(
            commands,
            vec![
                "cargo check --manifest-path crates/app/Cargo.toml".to_string(),
                "cargo check --manifest-path crates/app/Cargo.toml".to_string(),
            ]
        );
    }

    #[test]
    fn verification_feedback_includes_command_and_scope() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_workspace_fixture(&temp_dir);
        let report = crate::verification::VerificationReport {
            executed_command: crate::verification::detect_command(
                temp_dir.path(),
                &[temp_dir.path().join("crates/app/src/lib.rs").display().to_string()],
            )
            .expect("verification command"),
            command: "cargo check --manifest-path crates/app/Cargo.toml".to_string(),
            success: false,
            exit_code: Some(101),
            output: "error: expected expression".to_string(),
        };

        let feedback = build_verification_feedback(
            &report,
            &["crates/app/src/lib.rs".to_string(), "crates/app/src/api.rs".to_string()],
            None,
            0,
            None,
            None,
        );

        assert!(
            feedback.contains("Failed command: cargo check --manifest-path crates/app/Cargo.toml")
        );
        assert!(
            feedback.contains("Verification scope: crates/app/src/lib.rs, crates/app/src/api.rs.")
        );
        assert!(feedback.contains("Failure classification: build or typecheck failure."));
        assert!(feedback.contains("error: expected expression"));
    }

    #[test]
    fn verification_feedback_mentions_same_failure_family_on_retry() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_workspace_fixture(&temp_dir);
        let report = crate::verification::VerificationReport {
            executed_command: crate::verification::detect_command(
                temp_dir.path(),
                &[temp_dir.path().join("crates/app/src/lib.rs").display().to_string()],
            )
            .expect("verification command"),
            command: "cargo check --manifest-path crates/app/Cargo.toml".to_string(),
            success: false,
            exit_code: Some(101),
            output: "error[E0308]: mismatched types\n  --> src/lib.rs:44:5".to_string(),
        };
        let previous = crate::verification::VerificationFailureSummary::new(
            crate::verification::VerificationFailureKind::Build,
            Some("error[E0308]: mismatched types".to_string()),
        );
        let preference = report.retry_preference(&["crates/app/src/lib.rs".to_string()]);

        let feedback = build_verification_feedback(
            &report,
            &["crates/app/src/lib.rs".to_string()],
            Some(&previous),
            0,
            Some(RetryChangeRelevance::Relevant),
            Some(&preference),
        );

        assert!(feedback
            .contains("Retry status: the same build or typecheck failure is still failing."));
        assert!(feedback.contains(
            "Retry scope assessment: the latest edits touched the failing verification scope: crates/app/src/lib.rs."
        ));
    }

    #[test]
    fn verification_feedback_mentions_different_failure_on_retry() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_workspace_fixture(&temp_dir);
        let report = crate::verification::VerificationReport {
            executed_command: crate::verification::detect_command(
                temp_dir.path(),
                &[temp_dir.path().join("crates/app/src/lib.rs").display().to_string()],
            )
            .expect("verification command"),
            command: "cargo check --manifest-path crates/app/Cargo.toml".to_string(),
            success: false,
            exit_code: Some(101),
            output: "error[E0425]: cannot find value `missing` in this scope".to_string(),
        };
        let previous = crate::verification::VerificationFailureSummary::new(
            crate::verification::VerificationFailureKind::Test,
            Some("--- FAIL: TestClient".to_string()),
        );
        let preference = report.retry_preference(&["crates/app/src/lib.rs".to_string()]);

        let feedback = build_verification_feedback(
            &report,
            &["crates/app/src/lib.rs".to_string()],
            Some(&previous),
            0,
            Some(RetryChangeRelevance::Irrelevant),
            Some(&preference),
        );

        assert!(feedback.contains(
            "Retry status: verification uncovered a different failure: build or typecheck failure."
        ));
        assert!(feedback.contains(
            "Retry scope assessment: the latest edits did not touch the failing verification scope: crates/app/src/lib.rs."
        ));
    }

    #[test]
    fn verification_feedback_mentions_unknown_scope_on_retry() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_workspace_fixture(&temp_dir);
        let report = crate::verification::VerificationReport {
            executed_command: crate::verification::detect_command(
                temp_dir.path(),
                &[temp_dir.path().join("crates/app/src/lib.rs").display().to_string()],
            )
            .expect("verification command"),
            command: "cargo check --manifest-path crates/app/Cargo.toml".to_string(),
            success: false,
            exit_code: Some(101),
            output: "error[E0425]: cannot find value `missing` in this scope".to_string(),
        };
        let previous = crate::verification::VerificationFailureSummary::new(
            crate::verification::VerificationFailureKind::Build,
            Some("error[E0308]: mismatched types".to_string()),
        );
        let preference = report.retry_preference(&["crates/app/src/lib.rs".to_string()]);

        let feedback = build_verification_feedback(
            &report,
            &[],
            Some(&previous),
            0,
            Some(RetryChangeRelevance::Unknown),
            Some(&preference),
        );

        assert!(feedback.contains(
            "Retry scope assessment: unknown because no changed-file snapshot was available. Failing verification scope: crates/app/src/lib.rs."
        ));
    }

    #[test]
    fn verification_retry_stop_error_detects_same_failure_family() {
        let previous = crate::verification::VerificationFailureSummary::new(
            crate::verification::VerificationFailureKind::Build,
            Some("error[E0308]: mismatched types".to_string()),
        );
        let current = crate::verification::VerificationFailureSummary::new(
            crate::verification::VerificationFailureKind::Build,
            Some("error[E0308]: mismatched types".to_string()),
        );

        let error = verification_retry_stop_error(Some(&previous), Some(&current))
            .expect("same failure family should stop");
        assert!(error.contains("Automatic verification retry did not change the failure signature"));
    }

    #[test]
    fn verification_feedback_mentions_skipped_round_continuity() {
        let temp_dir = TempDir::new().expect("temp dir");
        write_rust_workspace_fixture(&temp_dir);
        let report = crate::verification::VerificationReport {
            executed_command: crate::verification::detect_command(
                temp_dir.path(),
                &[temp_dir.path().join("crates/app/src/lib.rs").display().to_string()],
            )
            .expect("verification command"),
            command: "cargo check --manifest-path crates/app/Cargo.toml".to_string(),
            success: false,
            exit_code: Some(101),
            output: "error[E0425]: cannot find value `missing` in this scope".to_string(),
        };
        let previous = crate::verification::VerificationFailureSummary::new(
            crate::verification::VerificationFailureKind::Build,
            Some("error[E0308]: mismatched types".to_string()),
        );
        let preference = report.retry_preference(&["crates/app/src/lib.rs".to_string()]);

        let feedback = build_verification_feedback(
            &report,
            &["crates/app/src/lib.rs".to_string()],
            Some(&previous),
            2,
            Some(RetryChangeRelevance::Relevant),
            Some(&preference),
        );

        assert!(feedback.contains(
            "Retry continuity: 2 intermediate tool round(s) did not run verification, so this is continuing the previous failing verification thread."
        ));
    }

    #[test]
    fn verification_retry_budget_grants_bonus_for_relevant_different_failure() {
        let previous = crate::verification::VerificationFailureSummary::new(
            crate::verification::VerificationFailureKind::Build,
            Some("error: expected expression".to_string()),
        );
        let current = crate::verification::VerificationFailureSummary::new(
            crate::verification::VerificationFailureKind::Build,
            Some("error[E0308]: mismatched types".to_string()),
        );

        let budget = verification_retry_budget(
            Some(&previous),
            Some(&current),
            Some(RetryChangeRelevance::Relevant),
            false,
        );

        assert_eq!(budget, VerificationRetryBudget { retry_limit: 2, bonus_retry_granted: true });
    }

    #[test]
    fn effective_verification_scope_files_falls_back_to_write_tool_paths() {
        let scope_files = effective_verification_scope_files(
            std::path::Path::new("/tmp/project"),
            &[],
            &[ToolCall {
                id: "tool-1".to_string(),
                name: "write".to_string(),
                arguments:
                    r#"{"file_path":"src/lib.rs","content":"pub fn value() -> usize { 2 }"}"#
                        .to_string(),
            }],
            &[ToolResult {
                tool_call_id: "tool-1".to_string(),
                content: "updated".to_string(),
                is_error: false,
            }],
        );

        assert_eq!(scope_files, vec!["/tmp/project/src/lib.rs".to_string()]);
    }

    #[test]
    fn build_system_prompt_lists_configured_providers() {
        let anthropic: Arc<dyn crate::provider::Provider> = Arc::new(MockProvider::new());
        let openai: Arc<dyn crate::provider::Provider> = Arc::new(MockProvider::new());
        let mut provider_registry = ProviderRegistry::new();
        provider_registry.insert(
            "anthropic",
            anthropic,
            Some("anthropic/claude-opus-4-7".into()),
            3,
        );
        provider_registry.insert("openai", openai, Some("openai/gpt-5.4".into()), 3);

        let prompt = build_system_prompt(std::path::Path::new("/tmp/project"), &provider_registry);

        assert!(prompt.contains("## Available Providers"));
        assert!(prompt.contains("- anthropic (default model: opus-4.7)"));
        assert!(prompt.contains("- openai (default model: gpt-5.4)"));
        assert!(prompt.contains("task` tool's `model` parameter"));
    }
}
