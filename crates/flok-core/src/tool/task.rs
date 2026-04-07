//! The `task` tool — spawns a sub-agent to handle a task.
//!
//! Creates a child session with its own prompt loop. The sub-agent runs
//! the task and returns its final text response. This is used for:
//! - Codebase exploration (explore agent)
//! - Parallel research tasks (general agent)
//! - Any work that benefits from a fresh context window

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::agent;
use crate::bus::BusEvent;
use crate::config::WorktreeConfig;
use crate::provider::{CompletionRequest, Message, MessageContent, StreamEvent};
use crate::team::{TeamMessage, TeamRegistry};
use crate::worktree::WorktreeManager;

use super::{Tool, ToolContext, ToolOutput};

/// Maximum number of prompt loop iterations for a sub-agent.
const MAX_SUBAGENT_STEPS: usize = 5;

/// Maximum concurrent background agent API calls.
///
/// Limits how many sub-agents can call `provider.stream()` simultaneously.
/// This prevents rate-limiting when spawning multiple reviewers in parallel.
const MAX_CONCURRENT_AGENTS: usize = 2;

/// Maximum retry attempts for sub-agent API calls.
const MAX_SUBAGENT_RETRIES: u32 = 3;

/// Global semaphore for throttling concurrent sub-agent API calls.
static AGENT_SEMAPHORE: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(MAX_CONCURRENT_AGENTS));

/// Spawn a sub-agent to handle a task.
pub struct TaskTool {
    /// The provider for the sub-agent (shared with parent).
    provider: Arc<dyn crate::provider::Provider>,
    /// Model ID to use for sub-agent API calls (e.g., "anthropic/claude-opus-4-6").
    model_id: String,
    /// Tool registry (sub-agents get a filtered set — no task tool to prevent recursion).
    tools: Arc<crate::tool::ToolRegistry>,
    /// Bus for emitting events.
    bus: crate::bus::Bus,
    /// Project root.
    project_root: std::path::PathBuf,
    /// Worktree manager for agent isolation (shared).
    worktree_mgr: Arc<WorktreeManager>,
    /// Worktree configuration.
    worktree_config: WorktreeConfig,
    /// Team registry for background agent coordination.
    team_registry: TeamRegistry,
}

impl TaskTool {
    /// Create a new task tool.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Arc<dyn crate::provider::Provider>,
        model_id: String,
        tools: Arc<crate::tool::ToolRegistry>,
        bus: crate::bus::Bus,
        project_root: std::path::PathBuf,
        worktree_mgr: Arc<WorktreeManager>,
        worktree_config: WorktreeConfig,
        team_registry: TeamRegistry,
    ) -> Self {
        Self {
            provider,
            model_id,
            tools,
            bus,
            project_root,
            worktree_mgr,
            worktree_config,
            team_registry,
        }
    }
}

#[async_trait::async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &'static str {
        "task"
    }

    fn description(&self) -> &'static str {
        // This gets dynamically enriched with agent list at registration time
        "Launch a sub-agent to handle a task autonomously. Available agent types: explore, general. \
         The agent runs with its own context and returns results when done."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        let agent_list = agent::format_agent_list();
        serde_json::json!({
            "type": "object",
            "required": ["description", "prompt", "subagent_type"],
            "properties": {
                "description": {
                    "type": "string",
                    "description": "A short (3-5 words) description of the task"
                },
                "prompt": {
                    "type": "string",
                    "description": "The detailed task for the agent to perform"
                },
                "subagent_type": {
                    "type": "string",
                    "description": format!("The type of agent to use:\n{agent_list}")
                },
                "background": {
                    "type": "boolean",
                    "description": "If true, the agent runs in the background and returns immediately with a task_id. Use with team_id for parallel multi-agent workflows."
                },
                "team_id": {
                    "type": "string",
                    "description": "The team ID to register this agent as a member. Required when background is true for team-based workflows."
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let description = args["description"].as_str().unwrap_or("task");
        let prompt = args["prompt"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: prompt"))?;
        let agent_type = args["subagent_type"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: subagent_type"))?;

        // Look up the agent definition
        let agent_def = agent::get_subagent(agent_type).ok_or_else(|| {
            let available: Vec<&str> = agent::subagents().iter().map(|a| a.name).collect();
            anyhow::anyhow!("Unknown agent type: {agent_type}. Available: {}", available.join(", "))
        })?;

        let background = args["background"].as_bool().unwrap_or(false);
        let team_id = args["team_id"].as_str().map(String::from);

        tracing::info!(agent = agent_type, description, background, "spawning sub-agent task");

        // Background mode: spawn the agent and return immediately
        if background {
            return self
                .execute_background(description, prompt, agent_type, &agent_def, team_id, ctx)
                .await;
        }

        self.bus.send(BusEvent::ToolCallStarted {
            session_id: ctx.session_id.clone(),
            tool_name: format!("task:{agent_type}"),
            tool_call_id: description.to_string(),
        });

        // Determine if this agent needs worktree isolation.
        // Only non-explore agents that modify files need isolation.
        let use_worktree = self.worktree_config.enabled
            && self.worktree_mgr.is_enabled()
            && agent_type != "explore";

        // Create worktree if needed, determining the effective project root
        let session_suffix = format!("{}:{description}", ctx.session_id);
        let worktree_info = if use_worktree {
            match self.worktree_mgr.create(&session_suffix).await {
                Ok(info) => {
                    tracing::info!(
                        agent = agent_type,
                        path = %info.path.display(),
                        "sub-agent using isolated worktree"
                    );
                    Some(info)
                }
                Err(e) => {
                    tracing::warn!(
                        agent = agent_type,
                        error = %e,
                        "worktree creation failed, falling back to shared directory"
                    );
                    None
                }
            }
        } else {
            None
        };

        let effective_root =
            worktree_info.as_ref().map_or_else(|| self.project_root.clone(), |wt| wt.path.clone());

        // Build the system prompt for the sub-agent
        let system = agent_def.system_prompt.map_or_else(
            || {
                format!(
                    "You are a sub-agent ({agent_type}) helping with a specific task. \
                     Complete the task and provide a clear summary of your findings.\n\n\
                     Working directory: {}",
                    effective_root.display()
                )
            },
            String::from,
        );

        // Run the sub-agent prompt loop with the effective project root
        let result =
            self.run_subagent(system, prompt, &ctx.session_id, description, &effective_root).await;

        // Merge worktree changes back if applicable
        let mut merge_info = String::new();
        if let Some(ref wt_info) = worktree_info {
            if result.is_ok() && self.worktree_config.auto_merge {
                match self.worktree_mgr.merge(wt_info).await {
                    Ok(crate::worktree::MergeResult::Clean { files_applied }) => {
                        tracing::info!(files_applied, "worktree merge: clean");
                        if files_applied > 0 {
                            merge_info =
                                format!("\n\n[{files_applied} file(s) merged into workspace]");
                        }
                    }
                    Ok(crate::worktree::MergeResult::Conflict { files_applied, conflicts }) => {
                        tracing::warn!(?conflicts, "worktree merge: conflicts");
                        merge_info = format!(
                            "\n\n[{files_applied} file(s) merged. CONFLICTS in: {}]",
                            conflicts.join(", ")
                        );
                    }
                    Ok(crate::worktree::MergeResult::NothingToMerge) => {}
                    Err(e) => {
                        tracing::error!(error = %e, "worktree merge failed");
                        merge_info = format!("\n\n[Worktree merge failed: {e}]");
                    }
                }
            }

            // Clean up worktree
            if self.worktree_config.cleanup_on_complete {
                if let Err(e) = self.worktree_mgr.remove(wt_info).await {
                    tracing::warn!(error = %e, "worktree cleanup failed");
                }
            }
        }

        let is_error = result.is_err();
        self.bus.send(BusEvent::ToolCallCompleted {
            session_id: ctx.session_id.clone(),
            tool_name: format!("task:{agent_type}"),
            tool_call_id: description.to_string(),
            is_error,
        });

        match result {
            Ok(response) => Ok(ToolOutput::success(format!("{response}{merge_info}"))),
            Err(e) => Ok(ToolOutput::error(format!("Sub-agent error: {e}"))),
        }
    }
}

impl TaskTool {
    /// Execute a background sub-agent that runs asynchronously and reports
    /// results via team messaging.
    ///
    /// Returns immediately with the agent's `task_id`.
    async fn execute_background(
        &self,
        description: &str,
        prompt: &str,
        agent_type: &str,
        agent_def: &agent::AgentDef,
        team_id: Option<String>,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let task_id = ulid::Ulid::new().to_string();
        let agent_name = format!("{agent_type}-{}", &task_id[..8]);

        // Register as team member if team_id provided
        if let Some(ref tid) = team_id {
            if let Some((_, mut team_arc)) = self.team_registry.teams_mut().remove(tid) {
                if let Some(team_mut) = Arc::get_mut(&mut team_arc) {
                    team_mut.add_member(&agent_name).await;
                }
                self.team_registry.reinsert(tid.clone(), team_arc);
            }
        }

        let effective_root = self.project_root.clone();
        let system = agent_def.system_prompt.as_ref().map_or_else(
            || {
                format!(
                    "You are a sub-agent ({agent_type}) helping with a specific task. \
                         Complete the task and provide a clear summary of your findings.\n\n\
                         Working directory: {}",
                    effective_root.display()
                )
            },
            std::string::ToString::to_string,
        );

        // Clone everything needed for the spawned task
        let provider = Arc::clone(&self.provider);
        let model_id = self.model_id.clone();
        let tools = Arc::clone(&self.tools);
        let bus = self.bus.clone();
        let session_id = ctx.session_id.clone();
        let prompt = prompt.to_string();
        let description = description.to_string();
        let agent_type = agent_type.to_string();
        let agent_name_clone = agent_name.clone();
        let task_id_clone = task_id.clone();
        let team_registry = self.team_registry.clone();
        let team_id_clone = team_id.clone();

        tokio::spawn(async move {
            tracing::debug!(
                agent = %agent_name_clone,
                agent_type = %agent_type,
                task_id = %task_id_clone,
                team_id = ?team_id_clone,
                "background agent task spawned"
            );

            bus.send(BusEvent::ToolCallStarted {
                session_id: session_id.clone(),
                tool_name: format!("task:{agent_type}:{agent_name_clone}"),
                tool_call_id: task_id_clone.clone(),
            });

            let result = run_subagent_standalone(
                provider,
                &model_id,
                tools,
                &system,
                &prompt,
                &session_id,
                &description,
                &effective_root,
            )
            .await;

            let (response, is_error) = match &result {
                Ok(text) => {
                    tracing::debug!(
                        agent = %agent_name_clone,
                        response_len = text.len(),
                        "background agent completed successfully"
                    );
                    (text.clone(), false)
                }
                Err(e) => {
                    tracing::warn!(
                        agent = %agent_name_clone,
                        error = %e,
                        "background agent failed"
                    );
                    (format!("Sub-agent error: {e}"), true)
                }
            };

            // Inject result into the lead's session via bus event.
            // The engine's wait_for_team_agents() loop will persist this
            // as a synthetic user message in the lead's DB session.
            if let Some(ref tid) = team_id_clone {
                // Also send via team channel for backwards compatibility
                if let Some(team) = team_registry.get(tid) {
                    let _ = team.send_message(TeamMessage {
                        from: agent_name_clone.clone(),
                        to: "lead".into(),
                        content: response.clone(),
                    });
                }

                // Inject into lead session — this is the primary mechanism
                bus.send(BusEvent::MessageInjected {
                    session_id: session_id.clone(),
                    from_agent: agent_name_clone.clone(),
                    content: response.clone(),
                });

                if is_error {
                    bus.send(BusEvent::TeamMemberFailed {
                        session_id: session_id.clone(),
                        team_id: tid.clone(),
                        agent_name: agent_name_clone.clone(),
                        error: response.clone(),
                    });
                } else {
                    bus.send(BusEvent::TeamMemberCompleted {
                        session_id: session_id.clone(),
                        team_id: tid.clone(),
                        agent_name: agent_name_clone.clone(),
                    });
                }
            }

            bus.send(BusEvent::ToolCallCompleted {
                session_id,
                tool_name: format!("task:{agent_type}:{agent_name_clone}"),
                tool_call_id: task_id_clone,
                is_error,
            });
        });

        tracing::info!(task_id = %task_id, agent_name = %agent_name, "background agent spawned");

        Ok(ToolOutput::success(
            serde_json::json!({
                "task_id": task_id,
                "agent_name": agent_name,
                "status": "running",
                "team_id": team_id,
            })
            .to_string(),
        ))
    }

    /// Run a sub-agent's prompt loop and return the final text response.
    async fn run_subagent(
        &self,
        system: String,
        prompt: &str,
        parent_session_id: &str,
        description: &str,
        effective_root: &std::path::Path,
    ) -> anyhow::Result<String> {
        let mut messages = vec![Message {
            role: "user".into(),
            content: vec![MessageContent::Text { text: prompt.to_string() }],
        }];

        let tool_defs = self.tools.tool_definitions();
        // Filter out the task tool itself to prevent infinite recursion
        let filtered_tools: Vec<_> = tool_defs.into_iter().filter(|t| t.name != "task").collect();

        let sub_ctx = ToolContext {
            project_root: effective_root.to_path_buf(),
            session_id: format!("{parent_session_id}:sub:{description}"),
            agent: "subagent".into(),
            cancel: tokio_util::sync::CancellationToken::new(),
            lsp: None,
        };

        for step in 0..MAX_SUBAGENT_STEPS {
            tracing::debug!(step, description, "sub-agent step");

            let request = CompletionRequest {
                model: self.model_id.clone(),
                system: system.clone(),
                messages: messages.clone(),
                tools: filtered_tools.clone(),
                max_tokens: 8192,
            };

            // Use retry + concurrency-limited streaming
            let (text, tool_calls) =
                stream_with_retry(&self.provider, request, description).await?;

            // Store the assistant message
            let mut parts: Vec<MessageContent> = Vec::new();
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
            messages.push(Message { role: "assistant".into(), content: parts });

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                return Ok(text);
            }

            // Execute tool calls
            let mut results: Vec<MessageContent> = Vec::new();
            for tc in &tool_calls {
                if let Some(tool) = self.tools.get(&tc.name) {
                    let args: serde_json::Value =
                        serde_json::from_str(&tc.arguments).unwrap_or_default();
                    match tool.execute(args, &sub_ctx).await {
                        Ok(output) => results.push(MessageContent::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: output.content,
                            is_error: output.is_error,
                        }),
                        Err(e) => results.push(MessageContent::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: format!("Error: {e}"),
                            is_error: true,
                        }),
                    }
                } else {
                    results.push(MessageContent::ToolResult {
                        tool_use_id: tc.id.clone(),
                        content: format!("Unknown tool: {}", tc.name),
                        is_error: true,
                    });
                }
            }
            messages.push(Message { role: "user".into(), content: results });
        }

        Err(anyhow::anyhow!("Sub-agent exceeded {MAX_SUBAGENT_STEPS} steps"))
    }
}

#[derive(Debug, Clone, Default)]
struct SubToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Stream a completion with retry and backoff for sub-agents.
///
/// Acquires the global concurrency semaphore before each attempt,
/// throttling parallel API calls to prevent rate limiting.
async fn stream_with_retry(
    provider: &Arc<dyn crate::provider::Provider>,
    request: CompletionRequest,
    description: &str,
) -> anyhow::Result<(String, Vec<SubToolCall>)> {
    let mut last_error = None;

    for attempt in 0..MAX_SUBAGENT_RETRIES {
        if attempt > 0 {
            // Exponential backoff: 1s, 2s, 4s
            let delay = std::time::Duration::from_secs(1 << (attempt - 1));
            tracing::info!(
                attempt,
                delay_secs = delay.as_secs(),
                description,
                "sub-agent retrying after backoff"
            );
            tokio::time::sleep(delay).await;
        }

        // Acquire the concurrency semaphore — blocks if too many agents are active
        let available = AGENT_SEMAPHORE.available_permits();
        if available == 0 {
            tracing::debug!(
                description,
                attempt,
                "waiting for agent semaphore (all {MAX_CONCURRENT_AGENTS} permits in use)"
            );
        }
        let _permit = AGENT_SEMAPHORE
            .acquire()
            .await
            .map_err(|e| anyhow::anyhow!("agent semaphore closed: {e}"))?;
        tracing::debug!(description, attempt, "acquired agent semaphore, streaming LLM request");

        let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
        let prov = Arc::clone(provider);
        let req = request.clone();
        let desc_owned = description.to_string();
        tokio::spawn(async move {
            if let Err(e) = prov.stream(req, tx).await {
                tracing::error!(description = %desc_owned, "sub-agent stream error: {e}");
            }
        });

        let mut text = String::new();
        let mut tool_calls: Vec<SubToolCall> = Vec::new();
        let timeout_duration = std::time::Duration::from_secs(60);
        let mut had_error = false;

        loop {
            let event = match tokio::time::timeout(timeout_duration, rx.recv()).await {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(_) => {
                    last_error = Some(anyhow::anyhow!(
                        "sub-agent stream timeout (60s, attempt {}/{})",
                        attempt + 1,
                        MAX_SUBAGENT_RETRIES
                    ));
                    had_error = true;
                    break;
                }
            };
            match event {
                StreamEvent::TextDelta(delta) => text.push_str(&delta),
                StreamEvent::ToolCallStart { index, id, name } => {
                    while tool_calls.len() <= index {
                        tool_calls.push(SubToolCall::default());
                    }
                    tool_calls[index].id = id;
                    tool_calls[index].name = name;
                }
                StreamEvent::ToolCallDelta { index, delta } => {
                    if let Some(tc) = tool_calls.get_mut(index) {
                        tc.arguments.push_str(&delta);
                    }
                }
                StreamEvent::Done => break,
                StreamEvent::Error(e) => {
                    last_error = Some(anyhow::anyhow!(
                        "sub-agent API error (attempt {}/{}): {e}",
                        attempt + 1,
                        MAX_SUBAGENT_RETRIES
                    ));
                    had_error = true;
                    break;
                }
                _ => {}
            }
        }

        if !had_error {
            // Filter empty tool calls
            let tool_calls: Vec<_> = tool_calls
                .into_iter()
                .filter(|tc| !tc.id.is_empty() && !tc.name.is_empty())
                .collect();
            return Ok((text, tool_calls));
        }
        // else: retry
    }

    Err(last_error.unwrap_or_else(|| {
        anyhow::anyhow!("sub-agent failed after {MAX_SUBAGENT_RETRIES} attempts")
    }))
}

/// Standalone sub-agent runner for background tasks (no `&self` needed).
#[allow(clippy::too_many_arguments)]
async fn run_subagent_standalone(
    provider: Arc<dyn crate::provider::Provider>,
    model_id: &str,
    tools: Arc<crate::tool::ToolRegistry>,
    system: &str,
    prompt: &str,
    parent_session_id: &str,
    description: &str,
    effective_root: &std::path::Path,
) -> anyhow::Result<String> {
    let mut messages = vec![Message {
        role: "user".into(),
        content: vec![MessageContent::Text { text: prompt.to_string() }],
    }];

    let tool_defs = tools.tool_definitions();
    let filtered_tools: Vec<_> = tool_defs.into_iter().filter(|t| t.name != "task").collect();

    let sub_ctx = ToolContext {
        project_root: effective_root.to_path_buf(),
        session_id: format!("{parent_session_id}:bg:{description}"),
        agent: "background".into(),
        cancel: tokio_util::sync::CancellationToken::new(),
        lsp: None,
    };

    for step in 0..MAX_SUBAGENT_STEPS {
        tracing::debug!(step, description, "background sub-agent step");

        let request = CompletionRequest {
            model: model_id.to_string(),
            system: system.to_string(),
            messages: messages.clone(),
            tools: filtered_tools.clone(),
            max_tokens: 8192,
        };

        // Use retry + concurrency-limited streaming
        let (text, tool_calls) = stream_with_retry(&provider, request, description).await?;

        let mut parts: Vec<MessageContent> = Vec::new();
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
        messages.push(Message { role: "assistant".into(), content: parts });

        if tool_calls.is_empty() {
            return Ok(text);
        }

        let mut results: Vec<MessageContent> = Vec::new();
        for tc in &tool_calls {
            if let Some(tool) = tools.get(&tc.name) {
                let args: serde_json::Value =
                    serde_json::from_str(&tc.arguments).unwrap_or_default();
                match tool.execute(args, &sub_ctx).await {
                    Ok(output) => results.push(MessageContent::ToolResult {
                        tool_use_id: tc.id.clone(),
                        content: output.content,
                        is_error: output.is_error,
                    }),
                    Err(e) => results.push(MessageContent::ToolResult {
                        tool_use_id: tc.id.clone(),
                        content: format!("Error: {e}"),
                        is_error: true,
                    }),
                }
            } else {
                results.push(MessageContent::ToolResult {
                    tool_use_id: tc.id.clone(),
                    content: format!("Unknown tool: {}", tc.name),
                    is_error: true,
                });
            }
        }
        messages.push(Message { role: "user".into(), content: results });
    }

    Err(anyhow::anyhow!("Background sub-agent exceeded {MAX_SUBAGENT_STEPS} steps"))
}
