//! The `task` tool — spawns a sub-agent to handle a task.
//!
//! Creates a child session with its own prompt loop. The sub-agent runs
//! the task and returns its final text response. This is used for:
//! - Codebase exploration (explore agent)
//! - Parallel research tasks (general agent)
//! - Any work that benefits from a fresh context window

use std::sync::Arc;

use crate::agent;
use crate::bus::BusEvent;
use crate::config::WorktreeConfig;
use crate::provider::{
    CompletionRequest, Message, MessageContent, ModelRegistry, ProviderRegistry,
};
use crate::team::{TeamMessage, TeamRegistry};
use crate::worktree::WorktreeManager;

use super::{Tool, ToolContext, ToolOutput};

/// Maximum number of prompt loop iterations for a sub-agent.
///
/// Matches `MAX_TOOL_ROUNDS` in the main session engine — sub-agents do
/// the same kind of work (LLM calls + tool calls) and need the same
/// headroom. This is a doom-loop safety valve, not a performance throttle.
const MAX_SUBAGENT_STEPS: usize = 25;

/// Spawn a sub-agent to handle a task.
pub struct TaskTool {
    /// Registry of all configured providers available to sub-agents.
    provider_registry: Arc<ProviderRegistry>,
    /// Provider name inherited from the caller when `model` is omitted.
    default_provider: String,
    /// Model ID inherited from the caller when `model` is omitted.
    default_model_id: String,
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
        provider_registry: Arc<ProviderRegistry>,
        default_provider: String,
        default_model_id: String,
        tools: Arc<crate::tool::ToolRegistry>,
        bus: crate::bus::Bus,
        project_root: std::path::PathBuf,
        worktree_mgr: Arc<WorktreeManager>,
        worktree_config: WorktreeConfig,
        team_registry: TeamRegistry,
    ) -> Self {
        Self {
            provider_registry,
            default_provider,
            default_model_id,
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
                "model": {
                    "type": "string",
                    "description": "Optional model alias or full ID (e.g., 'opus', 'gpt-5.4', 'anthropic/claude-opus-4-7'). If omitted, the sub-agent uses the caller's current provider/model. Use this for cross-coverage multi-model review: spawn the same specialist once per configured provider."
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
        let requested_model = args.get("model").and_then(serde_json::Value::as_str);

        // Look up the agent definition
        let agent_def = agent::get_subagent(agent_type).ok_or_else(|| {
            let available: Vec<&str> = agent::subagents().iter().map(|a| a.name).collect();
            anyhow::anyhow!("Unknown agent type: {agent_type}. Available: {}", available.join(", "))
        })?;

        let background = args["background"].as_bool().unwrap_or(false);
        let team_id = args["team_id"].as_str().map(String::from);

        tracing::info!(agent = agent_type, description, background, "spawning sub-agent task");

        let target = self.resolve_target(requested_model)?;

        // Background mode: spawn the agent and return immediately
        if background {
            return self
                .execute_background(
                    description,
                    prompt,
                    agent_type,
                    &agent_def,
                    team_id,
                    ctx,
                    target,
                )
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
        let result = self
            .run_subagent(system, prompt, &ctx.session_id, description, &effective_root, target)
            .await;

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

#[derive(Clone)]
struct SubagentTarget {
    provider_name: String,
    model_id: String,
}

impl TaskTool {
    fn resolve_target(&self, requested_model: Option<&str>) -> anyhow::Result<SubagentTarget> {
        let model_id =
            requested_model.map_or_else(|| self.default_model_id.clone(), ModelRegistry::resolve);
        let provider_name = if requested_model.is_some() {
            ModelRegistry::provider_name(&model_id).to_string()
        } else {
            self.default_provider.clone()
        };

        self.provider_registry.get(&provider_name).ok_or_else(|| {
            anyhow::anyhow!(
                "Provider '{provider_name}' is not configured for sub-agent model '{}'. Available providers: {}",
                model_id,
                self.provider_registry.describe()
            )
        })?;

        Ok(SubagentTarget { provider_name, model_id })
    }

    /// Execute a background sub-agent that runs asynchronously and reports
    /// results via team messaging.
    ///
    /// Returns immediately with the agent's `task_id`.
    #[expect(
        clippy::too_many_arguments,
        reason = "background dispatch needs execution context plus routing target"
    )]
    async fn execute_background(
        &self,
        description: &str,
        prompt: &str,
        agent_type: &str,
        agent_def: &agent::AgentDef,
        team_id: Option<String>,
        ctx: &ToolContext,
        target: SubagentTarget,
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
        let provider_registry = Arc::clone(&self.provider_registry);
        let provider_name = target.provider_name.clone();
        let model_id = target.model_id.clone();
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
                provider = %provider_name,
                model = %model_id,
                "background agent task spawned"
            );

            bus.send(BusEvent::ToolCallStarted {
                session_id: session_id.clone(),
                tool_name: format!("task:{agent_type}:{agent_name_clone}"),
                tool_call_id: task_id_clone.clone(),
            });

            let result = run_subagent_standalone(
                provider_registry,
                &provider_name,
                &model_id,
                tools,
                bus.clone(),
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
        target: SubagentTarget,
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
                model: target.model_id.clone(),
                system: system.clone(),
                messages: messages.clone(),
                tools: filtered_tools.clone(),
                max_tokens: 8192,
            };

            // Use fallback-aware concurrency-limited streaming.
            let (text, tool_calls) = stream_with_retry(
                &self.provider_registry,
                &target.provider_name,
                request,
                &self.bus,
                &sub_ctx.session_id,
            )
            .await?;

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

/// Stream a completion with runtime provider fallback for sub-agents.
async fn stream_with_retry(
    provider_registry: &ProviderRegistry,
    provider_name: &str,
    request: CompletionRequest,
    bus: &crate::bus::Bus,
    session_id: &str,
) -> anyhow::Result<(String, Vec<SubToolCall>)> {
    let initial_model = request.model.clone();
    let (text, tool_calls) = provider_registry
        .stream_with_fallback(provider_name, &initial_model, request, bus, session_id)
        .await?;

    Ok((
        text,
        tool_calls
            .into_iter()
            .map(|tool_call| SubToolCall {
                id: tool_call.id,
                name: tool_call.name,
                arguments: tool_call.arguments,
            })
            .collect(),
    ))
}

/// Standalone sub-agent runner for background tasks (no `&self` needed).
#[allow(clippy::too_many_arguments)]
async fn run_subagent_standalone(
    provider_registry: Arc<ProviderRegistry>,
    provider_name: &str,
    model_id: &str,
    tools: Arc<crate::tool::ToolRegistry>,
    bus: crate::bus::Bus,
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
        let (text, tool_calls) = stream_with_retry(
            &provider_registry,
            provider_name,
            request,
            &bus,
            &sub_ctx.session_id,
        )
        .await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use tokio::sync::mpsc;

    use crate::bus::Bus;
    use crate::config::WorktreeConfig;
    use crate::provider::{Provider, StreamEvent};
    use crate::team::TeamRegistry;

    #[derive(Debug)]
    struct RecordingProvider {
        name: &'static str,
        response: String,
        seen_models: Mutex<Vec<String>>,
    }

    impl RecordingProvider {
        fn new(name: &'static str, response: &str) -> Self {
            Self { name, response: response.to_string(), seen_models: Mutex::new(Vec::new()) }
        }

        fn seen_models(&self) -> Vec<String> {
            self.seen_models.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
        }
    }

    #[async_trait::async_trait]
    impl Provider for RecordingProvider {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn stream(
            &self,
            request: CompletionRequest,
            tx: mpsc::UnboundedSender<StreamEvent>,
        ) -> anyhow::Result<()> {
            self.seen_models
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.model);
            let _ = tx.send(StreamEvent::TextDelta(self.response.clone()));
            let _ = tx.send(StreamEvent::Done);
            Ok(())
        }
    }

    fn test_task_tool(
        provider_registry: Arc<ProviderRegistry>,
        default_provider: &str,
        default_model_id: &str,
    ) -> TaskTool {
        let project_root = std::env::temp_dir();
        TaskTool::new(
            provider_registry,
            default_provider.to_string(),
            default_model_id.to_string(),
            Arc::new(crate::tool::ToolRegistry::new()),
            Bus::new(16),
            std::fs::canonicalize(&project_root).expect("canonical project root"),
            Arc::new(WorktreeManager::new("test-project", project_root)),
            WorktreeConfig { enabled: false, ..WorktreeConfig::default() },
            TeamRegistry::new(),
        )
    }

    fn test_context() -> ToolContext {
        ToolContext {
            project_root: std::env::temp_dir(),
            session_id: "session-1".to_string(),
            agent: "lead".to_string(),
            cancel: tokio_util::sync::CancellationToken::new(),
            lsp: None,
        }
    }

    #[tokio::test]
    async fn execute_with_explicit_model_routes_to_requested_provider() {
        let anthropic = Arc::new(RecordingProvider::new("anthropic", "anthropic"));
        let openai = Arc::new(RecordingProvider::new("openai", "openai"));
        let anthropic_dyn: Arc<dyn Provider> = anthropic.clone();
        let openai_dyn: Arc<dyn Provider> = openai.clone();

        let mut registry = ProviderRegistry::new();
        registry.insert("anthropic", anthropic_dyn, Some("anthropic/claude-opus-4-7".into()), 3);
        registry.insert("openai", openai_dyn, Some("openai/gpt-5.4".into()), 3);

        let tool = test_task_tool(Arc::new(registry), "anthropic", "anthropic/claude-opus-4-7");
        let output = tool
            .execute(
                serde_json::json!({
                    "description": "cross check",
                    "prompt": "say which provider handled this",
                    "subagent_type": "general",
                    "model": "gpt-5.4"
                }),
                &test_context(),
            )
            .await
            .expect("task executes");

        assert!(!output.is_error);
        assert_eq!(output.content, "openai");
        assert!(anthropic.seen_models().is_empty());
        assert_eq!(openai.seen_models(), vec!["openai/gpt-5.4".to_string()]);
    }

    #[tokio::test]
    async fn execute_without_model_uses_default_provider_and_model() {
        let anthropic = Arc::new(RecordingProvider::new("anthropic", "anthropic"));
        let openai = Arc::new(RecordingProvider::new("openai", "openai"));
        let anthropic_dyn: Arc<dyn Provider> = anthropic.clone();
        let openai_dyn: Arc<dyn Provider> = openai.clone();

        let mut registry = ProviderRegistry::new();
        registry.insert("anthropic", anthropic_dyn, Some("anthropic/claude-opus-4-7".into()), 3);
        registry.insert("openai", openai_dyn, Some("openai/gpt-5.4".into()), 3);

        let tool = test_task_tool(Arc::new(registry), "anthropic", "anthropic/claude-opus-4-7");
        let output = tool
            .execute(
                serde_json::json!({
                    "description": "default route",
                    "prompt": "say which provider handled this",
                    "subagent_type": "general"
                }),
                &test_context(),
            )
            .await
            .expect("task executes");

        assert!(!output.is_error);
        assert_eq!(output.content, "anthropic");
        assert_eq!(anthropic.seen_models(), vec!["anthropic/claude-opus-4-7".to_string()]);
        assert!(openai.seen_models().is_empty());
    }

    #[tokio::test]
    async fn execute_with_unknown_provider_model_returns_clear_error() {
        let anthropic = Arc::new(RecordingProvider::new("anthropic", "anthropic"));
        let anthropic_dyn: Arc<dyn Provider> = anthropic.clone();

        let mut registry = ProviderRegistry::new();
        registry.insert("anthropic", anthropic_dyn, Some("anthropic/claude-opus-4-7".into()), 3);

        let tool = test_task_tool(Arc::new(registry), "anthropic", "anthropic/claude-opus-4-7");
        let error = tool
            .execute(
                serde_json::json!({
                    "description": "unknown route",
                    "prompt": "this should fail",
                    "subagent_type": "general",
                    "model": "google/gemini-2.5-pro"
                }),
                &test_context(),
            )
            .await
            .expect_err("unknown provider should error");

        assert!(error.to_string().contains("Provider 'google' is not configured"));
        assert!(anthropic.seen_models().is_empty());
    }
}
