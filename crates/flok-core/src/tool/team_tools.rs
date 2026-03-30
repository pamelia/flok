//! Team management tools — create teams, manage tasks, and send messages.
//!
//! These tools enable multi-agent coordination patterns like code review
//! and self-review loops where specialist agents work in parallel.

use crate::team::TeamRegistry;

use super::{Tool, ToolContext, ToolOutput};

// ---------------------------------------------------------------------------
// team_create
// ---------------------------------------------------------------------------

/// Create a named agent team for coordinating multiple sub-agents.
pub struct TeamCreateTool {
    registry: TeamRegistry,
}

impl TeamCreateTool {
    pub fn new(registry: TeamRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl Tool for TeamCreateTool {
    fn name(&self) -> &'static str {
        "team_create"
    }

    fn description(&self) -> &'static str {
        "Create a named agent team for coordinating multiple specialist sub-agents. \
         Returns the team ID which must be passed to team_task, send_message, and team_delete."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": "A descriptive name for the team (e.g., 'code-review-pr-42')"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let name = args["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: name"))?;

        let team = self.registry.create_team(name);
        tracing::info!(team_id = %team.id, name, "created team");

        Ok(ToolOutput::success(
            serde_json::json!({
                "team_id": team.id,
                "name": name,
            })
            .to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// team_delete
// ---------------------------------------------------------------------------

/// Disband a team and mark it as cancelled.
pub struct TeamDeleteTool {
    registry: TeamRegistry,
}

impl TeamDeleteTool {
    pub fn new(registry: TeamRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl Tool for TeamDeleteTool {
    fn name(&self) -> &'static str {
        "team_delete"
    }

    fn description(&self) -> &'static str {
        "Disband a team. Call this after all team work is done."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["team_id"],
            "properties": {
                "team_id": {
                    "type": "string",
                    "description": "The team ID returned by team_create"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let team_id = args["team_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: team_id"))?;

        if self.registry.delete(team_id) {
            tracing::info!(team_id, "disbanded team");
            Ok(ToolOutput::success(format!("Team {team_id} disbanded.")))
        } else {
            Ok(ToolOutput::error(format!("Team '{team_id}' not found.")))
        }
    }
}

// ---------------------------------------------------------------------------
// team_task
// ---------------------------------------------------------------------------

/// Manage tasks on a team's shared task board.
pub struct TeamTaskTool {
    registry: TeamRegistry,
}

impl TeamTaskTool {
    pub fn new(registry: TeamRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl Tool for TeamTaskTool {
    fn name(&self) -> &'static str {
        "team_task"
    }

    fn description(&self) -> &'static str {
        "Manage tasks on a team's shared task board. Operations: create, update, get, list."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["operation", "team_id"],
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["create", "update", "get", "list"],
                    "description": "The operation to perform"
                },
                "team_id": {
                    "type": "string",
                    "description": "The team ID"
                },
                "subject": {
                    "type": "string",
                    "description": "Task subject (required for create)"
                },
                "description": {
                    "type": "string",
                    "description": "Task description"
                },
                "task_id": {
                    "type": "string",
                    "description": "The task ID (required for get/update)"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "failed"],
                    "description": "Task status (for update)"
                },
                "owner": {
                    "type": "string",
                    "description": "Agent name that owns this task"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let team_id = args["team_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: team_id"))?;
        let operation = args["operation"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: operation"))?;

        let team = self
            .registry
            .get(team_id)
            .ok_or_else(|| anyhow::anyhow!("team '{team_id}' not found"))?;

        match operation {
            "create" => {
                let subject = args["subject"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required parameter: subject"))?;
                let description = args["description"].as_str().unwrap_or("");
                let task = team.create_task(subject.into(), description.into()).await;
                Ok(ToolOutput::success(serde_json::to_string_pretty(&task).unwrap_or_default()))
            }
            "update" => {
                let task_id = args["task_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required parameter: task_id"))?;
                let status = args["status"].as_str().map(parse_status).transpose()?;
                let owner = args["owner"].as_str().map(String::from);
                let description = args["description"].as_str().map(String::from);

                let task = team.update_task(task_id, status, owner, description).await?;
                Ok(ToolOutput::success(serde_json::to_string_pretty(&task).unwrap_or_default()))
            }
            "get" => {
                let task_id = args["task_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required parameter: task_id"))?;
                match team.get_task(task_id).await {
                    Some(task) => Ok(ToolOutput::success(
                        serde_json::to_string_pretty(&task).unwrap_or_default(),
                    )),
                    None => Ok(ToolOutput::error(format!("Task '{task_id}' not found"))),
                }
            }
            "list" => {
                let tasks = team.list_tasks().await;
                Ok(ToolOutput::success(serde_json::to_string_pretty(&tasks).unwrap_or_default()))
            }
            other => Ok(ToolOutput::error(format!(
                "Unknown operation: {other}. Use: create, update, get, list"
            ))),
        }
    }
}

fn parse_status(s: &str) -> anyhow::Result<crate::team::TaskStatus> {
    match s {
        "pending" => Ok(crate::team::TaskStatus::Pending),
        "in_progress" => Ok(crate::team::TaskStatus::InProgress),
        "completed" => Ok(crate::team::TaskStatus::Completed),
        "failed" => Ok(crate::team::TaskStatus::Failed),
        other => Err(anyhow::anyhow!(
            "invalid status '{other}'. Use: pending, in_progress, completed, failed"
        )),
    }
}

// ---------------------------------------------------------------------------
// send_message
// ---------------------------------------------------------------------------

/// Send a message to another agent in the team.
pub struct SendMessageTool {
    registry: TeamRegistry,
}

impl SendMessageTool {
    pub fn new(registry: TeamRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &'static str {
        "send_message"
    }

    fn description(&self) -> &'static str {
        "Send a message to another agent in your team. Use 'lead' as the recipient \
         to send to the team lead, or specify a specific agent name."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["team_id", "recipient", "content"],
            "properties": {
                "team_id": {
                    "type": "string",
                    "description": "The team ID"
                },
                "recipient": {
                    "type": "string",
                    "description": "Agent name to send to, or 'lead' for the team lead"
                },
                "content": {
                    "type": "string",
                    "description": "The message content"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let team_id = args["team_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: team_id"))?;
        let recipient = args["recipient"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: recipient"))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: content"))?;

        let team = self
            .registry
            .get(team_id)
            .ok_or_else(|| anyhow::anyhow!("team '{team_id}' not found"))?;

        let msg = crate::team::TeamMessage {
            from: ctx.agent.clone(),
            to: recipient.to_string(),
            content: content.to_string(),
        };

        match team.send_message(msg) {
            Ok(()) => Ok(ToolOutput::success(format!("Message sent to {recipient}."))),
            Err(e) => Ok(ToolOutput::error(format!("Failed to send message: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn team_create_returns_id() {
        let registry = TeamRegistry::new();
        let tool = TeamCreateTool::new(registry.clone());
        let ctx = ToolContext::test(std::path::PathBuf::from("/tmp"));

        let result = tool.execute(serde_json::json!({"name": "test-team"}), &ctx).await.unwrap();

        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(parsed["team_id"].is_string());
        assert_eq!(parsed["name"], "test-team");
    }

    #[tokio::test]
    async fn team_task_create_and_list() {
        let registry = TeamRegistry::new();
        let team = registry.create_team("test");
        let team_id = team.id.clone();

        let tool = TeamTaskTool::new(registry);
        let ctx = ToolContext::test(std::path::PathBuf::from("/tmp"));

        // Create a task
        let result = tool
            .execute(
                serde_json::json!({
                    "operation": "create",
                    "team_id": team_id,
                    "subject": "Review auth",
                    "description": "Check SQL injection"
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        // List tasks
        let result = tool
            .execute(
                serde_json::json!({
                    "operation": "list",
                    "team_id": team_id
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Review auth"));
    }

    #[tokio::test]
    async fn send_message_to_lead() {
        let registry = TeamRegistry::new();
        let team = registry.create_team("msg-test");
        let team_id = team.id.clone();

        let tool = SendMessageTool::new(registry.clone());
        let mut ctx = ToolContext::test(std::path::PathBuf::from("/tmp"));
        ctx.agent = "reviewer-1".into();

        let result = tool
            .execute(
                serde_json::json!({
                    "team_id": team_id,
                    "recipient": "lead",
                    "content": "Found a bug in auth.rs"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Message sent"));

        // Verify the message arrived
        let team = registry.get(&team_id).unwrap();
        let msgs = team.drain_messages("lead").await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "Found a bug in auth.rs");
        assert_eq!(msgs[0].from, "reviewer-1");
    }

    #[tokio::test]
    async fn team_delete_works() {
        let registry = TeamRegistry::new();
        let team = registry.create_team("ephemeral");
        let team_id = team.id.clone();

        let tool = TeamDeleteTool::new(registry.clone());
        let ctx = ToolContext::test(std::path::PathBuf::from("/tmp"));

        let result = tool.execute(serde_json::json!({"team_id": team_id}), &ctx).await.unwrap();

        assert!(!result.is_error);
        assert!(registry.get(&team_id).is_none());
    }
}
