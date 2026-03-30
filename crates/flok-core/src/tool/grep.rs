//! The `grep` tool — searches file contents using regex.

use std::path::Path;

use super::{Tool, ToolContext, ToolOutput};

/// Search file contents with regex patterns.
pub struct GrepTool;

#[async_trait::async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> &'static str {
        "Search file contents using regular expressions. Returns matching file paths and \
         line numbers. Uses ripgrep-style search."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: project root)"
                },
                "include": {
                    "type": "string",
                    "description": "File glob pattern to include (e.g., '*.rs', '*.{ts,tsx}')"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: pattern"))?;
        let search_path = args["path"].as_str();
        let include = args["include"].as_str();

        let dir = search_path
            .map_or_else(|| ctx.project_root.clone(), |p| resolve_path(&ctx.project_root, p));

        // Use `grep -rn` as a fallback (ripgrep would be better but may not be installed)
        // For v0.0.1, we shell out to grep. Later we'll use the `grep` crate or ripgrep lib.
        let mut cmd = tokio::process::Command::new("grep");
        cmd.arg("-rn")
            .arg("--color=never")
            .arg("-E") // Extended regex
            .arg(pattern);

        if let Some(glob) = include {
            cmd.arg("--include").arg(glob);
        }

        cmd.arg(".")
            .current_dir(&dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let output = tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output())
            .await
            .map_err(|_| anyhow::anyhow!("grep timed out after 30s"))??;

        let stdout = String::from_utf8_lossy(&output.stdout);

        // grep exits with 1 when no matches found — this is not an error
        if stdout.is_empty() || output.status.code() == Some(1) {
            return Ok(ToolOutput::success("No matches found.".to_string()));
        }

        // Truncate if too large (>50KB)
        let content = if stdout.len() > 50_000 {
            let truncated = &stdout[..50_000];
            format!("{truncated}\n\n... (truncated, showing first 50KB of results)")
        } else {
            stdout.to_string()
        };

        Ok(ToolOutput::success(content))
    }
}

fn resolve_path(project_root: &Path, file_path: &str) -> std::path::PathBuf {
    let path = Path::new(file_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_ctx() -> ToolContext {
        ToolContext::test(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
    }

    #[tokio::test]
    async fn grep_finds_pattern() {
        let result = GrepTool
            .execute(serde_json::json!({"pattern": "fn name", "include": "*.rs"}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("fn name"));
    }

    #[tokio::test]
    async fn grep_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world").unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());
        let result = GrepTool
            .execute(serde_json::json!({"pattern": "zzz_definitely_not_here_zzz"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("No matches"));
    }
}
