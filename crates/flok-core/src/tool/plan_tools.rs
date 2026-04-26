//! Typed execution plan tools.

use std::path::PathBuf;

use serde::Deserialize;

use crate::plan::{
    summarize_plan, Checkpoint, CheckpointData, Dependency, NewExecutionPlan, NewPlanStep,
    PlanPatch, PlanStatus, PlanStore, StepStatus,
};

use super::{Tool, ToolContext, ToolOutput};

/// Create a structured execution plan persisted to flok's generated state directory.
pub struct PlanCreateTool;

/// Update plan-level or step-level status for an existing plan.
pub struct PlanUpdateTool;

#[derive(Debug, Deserialize)]
struct CreatePlanArgs {
    title: String,
    #[serde(default)]
    description: String,
    steps: Vec<CreatePlanStep>,
    #[serde(default)]
    dependencies: Vec<Dependency>,
}

#[derive(Debug, Deserialize)]
struct CreatePlanStep {
    #[serde(default)]
    id: Option<String>,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    affected_files: Vec<String>,
    agent_type: String,
    #[serde(default)]
    estimated_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct UpdatePlanArgs {
    plan_id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    step_id: Option<String>,
    #[serde(default)]
    step_status: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    checkpoint_hash: Option<String>,
}

#[async_trait::async_trait]
impl Tool for PlanCreateTool {
    fn name(&self) -> &'static str {
        "plan_create"
    }

    fn description(&self) -> &'static str {
        "Create a typed execution plan and persist it under flok's generated per-project state directory. \
         Use this in plan mode when a task is complex enough to require explicit steps, \
         dependencies, and later approval/execution."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["title", "steps"],
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short title for the plan"
                },
                "description": {
                    "type": "string",
                    "description": "Longer description of the plan"
                },
                "steps": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "required": ["title", "agent_type"],
                        "properties": {
                            "id": {"type": "string"},
                            "title": {"type": "string"},
                            "description": {"type": "string"},
                            "affected_files": {
                                "type": "array",
                                "items": {"type": "string"}
                            },
                            "agent_type": {"type": "string"},
                            "estimated_tokens": {"type": "integer"}
                        }
                    }
                },
                "dependencies": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["prerequisite", "dependent"],
                        "properties": {
                            "prerequisite": {"type": "string"},
                            "dependent": {"type": "string"}
                        }
                    }
                }
            }
        })
    }

    fn describe_invocation(&self, args: &serde_json::Value) -> String {
        let title = args["title"].as_str().unwrap_or("untitled plan");
        format!("plan_create: {title}")
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let parsed: CreatePlanArgs = serde_json::from_value(args)?;
        let store = PlanStore::new(ctx.project_root.clone());
        let plan = store.create_plan(NewExecutionPlan {
            session_id: ctx.session_id.clone(),
            title: parsed.title,
            description: parsed.description,
            steps: parsed
                .steps
                .into_iter()
                .map(|step| NewPlanStep {
                    id: step.id,
                    title: step.title,
                    description: step.description,
                    affected_files: step.affected_files.into_iter().map(PathBuf::from).collect(),
                    agent_type: step.agent_type,
                    estimated_tokens: step.estimated_tokens,
                })
                .collect(),
            dependencies: parsed.dependencies,
        })?;

        let summary = summarize_plan(&plan);
        Ok(ToolOutput::success(format!(
            "{summary}\n\nPlan file: {}\n\n<plan_json>\n{}\n</plan_json>",
            store.plan_path(&plan.id).display(),
            serde_json::to_string_pretty(&plan)?
        )))
    }
}

#[async_trait::async_trait]
impl Tool for PlanUpdateTool {
    fn name(&self) -> &'static str {
        "plan_update"
    }

    fn description(&self) -> &'static str {
        "Update a typed execution plan's overall status or a specific step's status. \
         Use this during plan execution to record progress, failure, or checkpoints."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["plan_id"],
            "properties": {
                "plan_id": {
                    "type": "string",
                    "description": "The plan ID to update"
                },
                "status": {
                    "type": "string",
                    "enum": ["draft", "approved", "executing", "paused", "completed", "failed", "cancelled", "rolled_back"]
                },
                "step_id": {
                    "type": "string",
                    "description": "Target step ID when updating a step"
                },
                "step_status": {
                    "type": "string",
                    "enum": ["pending", "running", "completed", "failed", "skipped", "rolled_back"]
                },
                "error": {
                    "type": "string",
                    "description": "Failure reason when step_status is 'failed'"
                },
                "checkpoint_hash": {
                    "type": "string",
                    "description": "Optional workspace snapshot hash captured before the step"
                }
            }
        })
    }

    fn describe_invocation(&self, args: &serde_json::Value) -> String {
        let plan_id = args["plan_id"].as_str().unwrap_or("unknown");
        format!("plan_update: {plan_id}")
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let parsed: UpdatePlanArgs = serde_json::from_value(args)?;
        let checkpoint = parsed.checkpoint_hash.as_ref().map(|hash| Checkpoint {
            step_id: parsed.step_id.clone().unwrap_or_default(),
            snapshot: CheckpointData::WorkspaceSnapshot { hash: hash.clone() },
            created_at: chrono::Utc::now(),
        });

        let updated = PlanStore::new(ctx.project_root.clone()).apply_patch(
            &parsed.plan_id,
            PlanPatch {
                plan_status: parsed.status.as_deref().map(parse_plan_status).transpose()?,
                step_id: parsed.step_id,
                step_status: parsed
                    .step_status
                    .as_deref()
                    .map(|status| parse_step_status(status, parsed.error.as_deref()))
                    .transpose()?,
                checkpoint,
                ..PlanPatch::default()
            },
        )?;

        let summary = summarize_plan(&updated);
        Ok(ToolOutput::success(format!(
            "{summary}\n\n<plan_json>\n{}\n</plan_json>",
            serde_json::to_string_pretty(&updated)?
        )))
    }
}

fn parse_plan_status(value: &str) -> anyhow::Result<PlanStatus> {
    match value {
        "draft" => Ok(PlanStatus::Draft),
        "approved" => Ok(PlanStatus::Approved),
        "executing" => Ok(PlanStatus::Executing),
        "paused" => Ok(PlanStatus::Paused),
        "completed" => Ok(PlanStatus::Completed),
        "failed" => Ok(PlanStatus::Failed),
        "cancelled" => Ok(PlanStatus::Cancelled),
        "rolled_back" => Ok(PlanStatus::RolledBack),
        other => Err(anyhow::anyhow!("invalid plan status '{other}'")),
    }
}

fn parse_step_status(value: &str, error: Option<&str>) -> anyhow::Result<StepStatus> {
    match value {
        "pending" => Ok(StepStatus::Pending),
        "running" => Ok(StepStatus::Running),
        "completed" => Ok(StepStatus::Completed),
        "failed" => Ok(StepStatus::Failed(error.unwrap_or("step failed").to_string())),
        "skipped" => Ok(StepStatus::Skipped),
        "rolled_back" => Ok(StepStatus::RolledBack),
        other => Err(anyhow::anyhow!("invalid step status '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext::test(dir.path().to_path_buf());
        (dir, ctx)
    }

    #[tokio::test]
    async fn plan_create_writes_json_plan() {
        let (_dir, ctx) = ctx();
        let result = PlanCreateTool
            .execute(
                serde_json::json!({
                    "title": "Refactor auth",
                    "steps": [
                        {
                            "id": "step-1",
                            "title": "Add JWT module",
                            "agent_type": "build",
                            "affected_files": ["src/auth/jwt.rs"]
                        }
                    ]
                }),
                &ctx,
            )
            .await
            .expect("tool execution");

        assert!(!result.is_error);
        assert!(result.content.contains("Plan file:"));
        assert!(crate::config::project_state_dir(&ctx.project_root).join("plans").exists());
    }

    #[tokio::test]
    async fn plan_update_marks_step_complete() {
        let (_dir, ctx) = ctx();
        let create = PlanCreateTool
            .execute(
                serde_json::json!({
                    "title": "Refactor auth",
                    "steps": [
                        {
                            "id": "step-1",
                            "title": "Add JWT module",
                            "agent_type": "build"
                        }
                    ]
                }),
                &ctx,
            )
            .await
            .expect("create tool");
        let json = create
            .content
            .split("<plan_json>\n")
            .nth(1)
            .and_then(|rest| rest.split("\n</plan_json>").next())
            .expect("plan json block");
        let plan: serde_json::Value = serde_json::from_str(json).expect("parse plan json");
        let plan_id = plan["id"].as_str().expect("plan id");

        let update = PlanUpdateTool
            .execute(
                serde_json::json!({
                    "plan_id": plan_id,
                    "status": "executing",
                    "step_id": "step-1",
                    "step_status": "completed"
                }),
                &ctx,
            )
            .await
            .expect("update tool");

        assert!(!update.is_error);
        assert!(update.content.contains("[executing]"));
        assert!(update.content.contains("[completed]"));
    }
}
