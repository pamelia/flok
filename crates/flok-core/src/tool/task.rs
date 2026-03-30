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
use crate::provider::{CompletionRequest, Message, MessageContent, StreamEvent};

use super::{Tool, ToolContext, ToolOutput};

/// Maximum number of prompt loop iterations for a sub-agent.
const MAX_SUBAGENT_STEPS: usize = 15;

/// Spawn a sub-agent to handle a task.
pub struct TaskTool {
    /// The provider for the sub-agent (shared with parent).
    provider: Arc<dyn crate::provider::Provider>,
    /// Tool registry (sub-agents get a filtered set — no task tool to prevent recursion).
    tools: Arc<crate::tool::ToolRegistry>,
    /// Bus for emitting events.
    bus: crate::bus::Bus,
    /// Project root.
    project_root: std::path::PathBuf,
}

impl TaskTool {
    /// Create a new task tool.
    pub fn new(
        provider: Arc<dyn crate::provider::Provider>,
        tools: Arc<crate::tool::ToolRegistry>,
        bus: crate::bus::Bus,
        project_root: std::path::PathBuf,
    ) -> Self {
        Self { provider, tools, bus, project_root }
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

        tracing::info!(agent = agent_type, description, "spawning sub-agent task");

        self.bus.send(BusEvent::ToolCallStarted {
            session_id: ctx.session_id.clone(),
            tool_name: format!("task:{agent_type}"),
            tool_call_id: description.to_string(),
        });

        // Build the system prompt for the sub-agent
        let system = agent_def.system_prompt.map_or_else(
            || {
                format!(
                    "You are a sub-agent ({agent_type}) helping with a specific task. \
                     Complete the task and provide a clear summary of your findings.\n\n\
                     Working directory: {}",
                    self.project_root.display()
                )
            },
            String::from,
        );

        // Run the sub-agent prompt loop
        let result = self.run_subagent(system, prompt, &ctx.session_id, description).await;

        let is_error = result.is_err();
        self.bus.send(BusEvent::ToolCallCompleted {
            session_id: ctx.session_id.clone(),
            tool_name: format!("task:{agent_type}"),
            tool_call_id: description.to_string(),
            is_error,
        });

        match result {
            Ok(response) => Ok(ToolOutput::success(response)),
            Err(e) => Ok(ToolOutput::error(format!("Sub-agent error: {e}"))),
        }
    }
}

impl TaskTool {
    /// Run a sub-agent's prompt loop and return the final text response.
    async fn run_subagent(
        &self,
        system: String,
        prompt: &str,
        parent_session_id: &str,
        description: &str,
    ) -> anyhow::Result<String> {
        let mut messages = vec![Message {
            role: "user".into(),
            content: vec![MessageContent::Text { text: prompt.to_string() }],
        }];

        let tool_defs = self.tools.tool_definitions();
        // Filter out the task tool itself to prevent infinite recursion
        let filtered_tools: Vec<_> = tool_defs.into_iter().filter(|t| t.name != "task").collect();

        let sub_ctx = ToolContext {
            project_root: self.project_root.clone(),
            session_id: format!("{parent_session_id}:sub:{description}"),
            agent: "subagent".into(),
            cancel: tokio_util::sync::CancellationToken::new(),
        };

        for step in 0..MAX_SUBAGENT_STEPS {
            tracing::debug!(step, description, "sub-agent step");

            let request = CompletionRequest {
                model: String::new(), // Will use the provider's default
                system: system.clone(),
                messages: messages.clone(),
                tools: filtered_tools.clone(),
                max_tokens: 8192,
            };

            // Stream the response
            let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
            let provider = Arc::clone(&self.provider);
            tokio::spawn(async move {
                if let Err(e) = provider.stream(request, tx).await {
                    tracing::error!("Sub-agent stream error: {e}");
                }
            });

            let mut text = String::new();
            let mut tool_calls: Vec<SubToolCall> = Vec::new();

            let timeout = std::time::Duration::from_secs(30);
            loop {
                let event = match tokio::time::timeout(timeout, rx.recv()).await {
                    Ok(Some(event)) => event,
                    Ok(None) => break,
                    Err(_) => return Err(anyhow::anyhow!("Sub-agent stream timeout")),
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
                    StreamEvent::Error(e) => return Err(anyhow::anyhow!("Sub-agent error: {e}")),
                    _ => {}
                }
            }

            // Filter empty tool calls
            let tool_calls: Vec<_> = tool_calls
                .into_iter()
                .filter(|tc| !tc.id.is_empty() && !tc.name.is_empty())
                .collect();

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
