//! The `code_review` tool — runs a structured code review on a diff.
//!
//! Invokes the built-in `ReviewEngine` with specialist reviewers
//! (correctness, style, architecture, completeness) and returns a
//! structured report with findings organized by priority tier and
//! a binary verdict.

use std::path::Path;
use std::sync::Arc;

use crate::provider::Provider;
use crate::review::ReviewEngine;

use super::{Tool, ToolContext, ToolOutput};

/// Run a structured code review on a git diff.
pub struct CodeReviewTool {
    provider: Arc<dyn Provider>,
}

impl CodeReviewTool {
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl Tool for CodeReviewTool {
    fn name(&self) -> &'static str {
        "code_review"
    }

    fn description(&self) -> &'static str {
        "Run a structured code review on a git diff. Spawns specialist \
         reviewers (correctness, style, architecture, completeness) in \
         parallel and returns a prioritized report with a binary verdict \
         (APPROVE or REQUEST_CHANGES). Use this after making changes to \
         verify code quality before committing."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "base": {
                    "type": "string",
                    "description": "Base branch or commit to diff against (default: 'main')"
                },
                "context": {
                    "type": "string",
                    "description": "Optional PR description or additional context for reviewers"
                },
                "pr_number": {
                    "type": "integer",
                    "description": "GitHub PR number — if provided, fetches diff via 'gh pr diff'"
                }
            }
        })
    }

    fn describe_invocation(&self, args: &serde_json::Value) -> String {
        if let Some(pr) = args["pr_number"].as_u64() {
            format!("Code review PR #{pr}")
        } else {
            let base = args["base"].as_str().unwrap_or("main");
            format!("Code review against {base}")
        }
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let context = args["context"].as_str();

        // Get the diff — either from a PR or from git diff
        let diff = if let Some(pr_number) = args["pr_number"].as_u64() {
            get_pr_diff(&ctx.project_root, pr_number).await?
        } else {
            let base = args["base"].as_str().unwrap_or("main");
            get_branch_diff(&ctx.project_root, base).await?
        };

        if diff.trim().is_empty() {
            return Ok(ToolOutput::success("No changes to review — diff is empty."));
        }

        let changed_lines =
            diff.lines().filter(|l| l.starts_with('+') || l.starts_with('-')).count();

        tracing::info!(changed_lines, "starting code review");

        let engine = ReviewEngine::new(Arc::clone(&self.provider));
        match engine.review(&diff, context).await {
            Ok(result) => {
                let report = result.format_report();
                // Also include machine-readable JSON for the LLM to act on
                let json = serde_json::to_string(&result).unwrap_or_default();
                Ok(ToolOutput::success(format!(
                    "{report}\n\n<review_json>\n{json}\n</review_json>"
                )))
            }
            Err(e) => Ok(ToolOutput::error(format!("Code review failed: {e}"))),
        }
    }
}

/// Get diff from a GitHub PR via `gh pr diff`.
async fn get_pr_diff(project_root: &Path, pr_number: u64) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("gh")
        .args(["pr", "diff", &pr_number.to_string()])
        .current_dir(project_root)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("gh pr diff failed: {stderr}"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Get diff between current branch and a base branch.
async fn get_branch_diff(project_root: &Path, base: &str) -> anyhow::Result<String> {
    // Try origin/<base> first, fall back to <base>
    let base_ref = format!("origin/{base}");
    let output = tokio::process::Command::new("git")
        .args(["diff", &base_ref])
        .current_dir(project_root)
        .output()
        .await?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }

    // Fall back to local base branch
    let output = tokio::process::Command::new("git")
        .args(["diff", base])
        .current_dir(project_root)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("git diff failed for base '{base}': {stderr}"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
