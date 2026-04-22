//! The `fast_apply` tool — applies code edits using lazy snippets.
//!
//! Unlike the `edit` tool which requires exact string matching, `fast_apply`
//! handles snippets with `// ... existing code ...` markers and fuzzy
//! line matching. This is the preferred tool for multi-line code changes.

use super::path_security::resolve_write_path;
use super::{PermissionLevel, Tool, ToolContext, ToolOutput};

/// Apply code edits using lazy snippets with ellipsis markers.
pub struct FastApplyTool;

fn format_attempt_trace(result: &flok_apply::ApplyResult) -> String {
    if result.attempts.len() <= 1 {
        return String::new();
    }

    let mut parts = Vec::new();
    for attempt in &result.attempts {
        let label = if attempt.success {
            format!("{} succeeded", attempt.strategy)
        } else if let Some(reason) = &attempt.reason {
            format!("{} failed ({reason})", attempt.strategy)
        } else {
            format!("{} failed", attempt.strategy)
        };
        parts.push(label);
    }

    format!(" [attempts: {}]", parts.join(" -> "))
}

#[async_trait::async_trait]
impl Tool for FastApplyTool {
    fn name(&self) -> &'static str {
        "fast_apply"
    }

    fn description(&self) -> &'static str {
        "Apply a code edit to a file using a snippet that may contain ellipsis markers \
         like `// ... existing code ...` to indicate unchanged regions. The tool \
         intelligently matches the snippet against the original file and merges the \
         changes. Preferred over `edit` for multi-line changes or when you want to \
         show only the changed parts of a function/block."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["file_path", "snippet"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The path to the file to edit (relative to project root or absolute)"
                },
                "snippet": {
                    "type": "string",
                    "description": "The code snippet to apply. May contain `// ... existing code ...` markers to indicate unchanged regions that should be preserved from the original file."
                }
            }
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn describe_invocation(&self, args: &serde_json::Value) -> String {
        let path = args["file_path"].as_str().unwrap_or("unknown");
        format!("Fast-apply edit to {path}")
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let file_path = args["file_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: file_path"))?;
        let snippet = args["snippet"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: snippet"))?;

        let resolved = match resolve_write_path(&ctx.project_root, file_path) {
            Ok(path) => path,
            Err(error) => return Ok(ToolOutput::error(error.to_string())),
        };

        // Read the original file
        let original = match tokio::fs::read_to_string(&resolved).await {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // File doesn't exist — treat snippet as a new file
                if let Some(parent) = resolved.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&resolved, snippet).await?;
                if let Some(lsp) = &ctx.lsp {
                    if let Err(error) = lsp.track_write(&resolved, snippet.to_string()).await {
                        tracing::debug!(path = %resolved.display(), %error, "failed to sync fast_apply create with lsp");
                    }
                }
                let lines = snippet.lines().count();
                return Ok(ToolOutput::success(format!(
                    "Created new file {file_path} ({lines} lines) [strategy: new-file]"
                )));
            }
            Err(e) => {
                return Ok(ToolOutput::error(format!("Failed to read {file_path}: {e}")));
            }
        };

        // Apply the edit
        match flok_apply::apply_edit(&original, snippet) {
            Ok(result) => {
                tokio::fs::write(&resolved, &result.content).await?;
                if let Some(lsp) = &ctx.lsp {
                    if let Err(error) = lsp.track_write(&resolved, result.content.clone()).await {
                        tracing::debug!(path = %resolved.display(), %error, "failed to sync fast_apply edit with lsp");
                    }
                }
                let lines = result.content.lines().count();
                Ok(ToolOutput::success(format!(
                    "Applied edit to {file_path} ({lines} lines) [strategy: {}]{}",
                    result.strategy,
                    format_attempt_trace(&result),
                )))
            }
            Err(e) => Ok(ToolOutput::error(format!(
                "Failed to apply edit to {file_path}: {e}. \
                 Try using the `edit` tool with exact string matching, \
                 or the `write` tool to replace the entire file."
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fast_apply_creates_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());

        let args = serde_json::json!({
            "file_path": "new_file.rs",
            "snippet": "fn hello() {\n    println!(\"hello\");\n}"
        });

        let result = FastApplyTool.execute(args, &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Created new file"));
        assert!(dir.path().join("new_file.rs").exists());
    }

    #[tokio::test]
    async fn fast_apply_blocks_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());

        let args = serde_json::json!({
            "file_path": "../escape.rs",
            "snippet": "fn main() {}"
        });

        let result = FastApplyTool.execute(args, &ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("escapes project root"));
    }

    #[tokio::test]
    async fn fast_apply_with_ellipsis() {
        let dir = tempfile::tempdir().unwrap();
        let original =
            "fn main() {\n    let x = 1;\n    let y = 2;\n    println!(\"{}\", x + y);\n}";
        tokio::fs::write(dir.path().join("test.rs"), original).await.unwrap();

        let ctx = ToolContext::test(dir.path().to_path_buf());
        let args = serde_json::json!({
            "file_path": "test.rs",
            "snippet": "fn main() {\n    let x = 42;\n    // ... existing code ...\n    println!(\"{}\", x + y);\n}"
        });

        let result = FastApplyTool.execute(args, &ctx).await.unwrap();
        assert!(!result.is_error, "error: {}", result.content);
        assert!(result.content.contains("ellipsis-merge"));

        let content = tokio::fs::read_to_string(dir.path().join("test.rs")).await.unwrap();
        assert!(content.contains("let x = 42;"));
        assert!(content.contains("let y = 2;"));
    }

    #[test]
    fn format_attempt_trace_reports_failures_before_success() {
        let trace = format_attempt_trace(&flok_apply::ApplyResult {
            content: "fn main() {}".to_string(),
            strategy: flok_apply::Strategy::FullFile,
            attempts: vec![
                flok_apply::StrategyAttempt {
                    strategy: flok_apply::Strategy::EllipsisMerge,
                    success: false,
                    reason: Some(
                        "could not match snippet context against the original file".into(),
                    ),
                },
                flok_apply::StrategyAttempt {
                    strategy: flok_apply::Strategy::FullFile,
                    success: true,
                    reason: None,
                },
            ],
        });

        assert!(trace.contains("ellipsis-merge failed"));
        assert!(trace.contains("full-file succeeded"));
    }

    #[tokio::test]
    async fn fast_apply_unmatched_returns_error_message() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());

        // Write a substantial file, then give a snippet that can't match
        let original = "fn alpha() {\n    let a = 1;\n}\n\nfn beta() {\n    let b = 2;\n}\n\nfn gamma() {\n    let g = 3;\n}\n";
        tokio::fs::write(dir.path().join("test.rs"), original).await.unwrap();

        let args = serde_json::json!({
            "file_path": "test.rs",
            "snippet": "completely\nunrelated\ncontent"
        });

        let result = FastApplyTool.execute(args, &ctx).await.unwrap();
        assert!(result.is_error, "expected error but got: {}", result.content);
        assert!(result.content.contains("Failed to apply"));
    }

    #[tokio::test]
    async fn fast_apply_blocks_dotflok_internal_paths() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());

        let args = serde_json::json!({
            "file_path": ".flok/blocked.rs",
            "snippet": "fn main() {}"
        });

        let result = FastApplyTool.execute(args, &ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains(".flok internals"));
    }
}
