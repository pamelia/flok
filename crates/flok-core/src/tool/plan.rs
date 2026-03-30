//! The `plan` tool — writes structured plan output.
//!
//! Available in plan mode only. Writes a structured markdown plan
//! to `.flok/plan.md` in the project root.

use super::{Tool, ToolContext, ToolOutput};

/// Write a structured plan to `.flok/plan.md`.
pub struct PlanTool;

#[async_trait::async_trait]
impl Tool for PlanTool {
    fn name(&self) -> &'static str {
        "plan"
    }

    fn description(&self) -> &'static str {
        "Write a structured plan to .flok/plan.md. Use this in plan mode to document \
         your analysis and proposed changes before switching to build mode."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["content"],
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The plan content in markdown format"
                },
                "append": {
                    "type": "boolean",
                    "description": "If true, append to existing plan instead of replacing (default: false)"
                }
            }
        })
    }

    fn permission_level(&self) -> super::PermissionLevel {
        // Plan tool is safe — it only writes to .flok/plan.md
        super::PermissionLevel::Safe
    }

    fn describe_invocation(&self, _args: &serde_json::Value) -> String {
        "plan: write to .flok/plan.md".to_string()
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: content"))?;
        let append = args["append"].as_bool().unwrap_or(false);

        let plan_dir = ctx.project_root.join(".flok");
        let plan_file = plan_dir.join("plan.md");

        tokio::fs::create_dir_all(&plan_dir).await?;

        if append {
            let mut existing = if plan_file.exists() {
                tokio::fs::read_to_string(&plan_file).await?
            } else {
                String::new()
            };
            if !existing.is_empty() && !existing.ends_with('\n') {
                existing.push('\n');
            }
            existing.push_str(content);
            tokio::fs::write(&plan_file, &existing).await?;
            Ok(ToolOutput::success(format!(
                "Plan appended to .flok/plan.md ({} chars total)",
                existing.len()
            )))
        } else {
            tokio::fs::write(&plan_file, content).await?;
            Ok(ToolOutput::success(format!(
                "Plan written to .flok/plan.md ({} chars)",
                content.len()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            project_root: dir.to_path_buf(),
            session_id: "test".into(),
            agent: "test".into(),
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn write_plan() {
        let dir = tempfile::tempdir().unwrap();
        let tool = PlanTool;
        let result = tool
            .execute(
                serde_json::json!({"content": "# Plan\n1. Do the thing"}),
                &test_ctx(dir.path()),
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        let content = std::fs::read_to_string(dir.path().join(".flok/plan.md")).unwrap();
        assert!(content.contains("# Plan"));
    }

    #[tokio::test]
    async fn append_plan() {
        let dir = tempfile::tempdir().unwrap();
        let tool = PlanTool;
        let ctx = test_ctx(dir.path());

        tool.execute(serde_json::json!({"content": "Step 1"}), &ctx).await.unwrap();
        tool.execute(serde_json::json!({"content": "Step 2", "append": true}), &ctx).await.unwrap();

        let content = std::fs::read_to_string(dir.path().join(".flok/plan.md")).unwrap();
        assert!(content.contains("Step 1"));
        assert!(content.contains("Step 2"));
    }
}
