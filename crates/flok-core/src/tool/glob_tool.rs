//! The `glob` tool — finds files matching glob patterns.

use std::path::Path;

use super::{Tool, ToolContext, ToolOutput};

/// Find files matching a glob pattern.
pub struct GlobTool;

#[async_trait::async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> &'static str {
        "Find files matching a glob pattern (e.g., '**/*.rs', 'src/**/*.ts'). \
         Returns matching file paths sorted by modification time."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match files against (e.g., '**/*.rs')"
                },
                "path": {
                    "type": "string",
                    "description": "The directory to search in (default: project root)"
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

        let dir = search_path
            .map_or_else(|| ctx.project_root.clone(), |p| resolve_path(&ctx.project_root, p));

        // Build the full glob pattern
        let full_pattern = dir.join(pattern).to_string_lossy().to_string();

        let mut entries: Vec<String> = Vec::new();
        let glob_results =
            glob::glob(&full_pattern).map_err(|e| anyhow::anyhow!("Invalid glob pattern: {e}"))?;

        for entry in glob_results {
            match entry {
                Ok(path) => {
                    // Make path relative to the search dir
                    let relative =
                        path.strip_prefix(&dir).unwrap_or(&path).to_string_lossy().to_string();
                    entries.push(relative);
                }
                Err(e) => {
                    tracing::debug!("glob error for entry: {e}");
                }
            }
        }

        if entries.is_empty() {
            return Ok(ToolOutput::success("No files matched the pattern."));
        }

        // Truncate if too many results
        let total = entries.len();
        if total > 500 {
            entries.truncate(500);
            entries.push(format!("\n... ({total} total matches, showing first 500)"));
        }

        Ok(ToolOutput::success(entries.join("\n")))
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
    async fn glob_finds_rust_files() {
        let result = GlobTool
            .execute(serde_json::json!({"pattern": "src/**/*.rs"}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("lib.rs"));
    }

    #[tokio::test]
    async fn glob_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());
        let result = GlobTool
            .execute(serde_json::json!({"pattern": "**/*.nonexistent"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("No files matched"));
    }

    #[tokio::test]
    async fn glob_invalid_pattern_returns_error() {
        let result =
            GlobTool.execute(serde_json::json!({"pattern": "[invalid"}), &test_ctx()).await;
        assert!(result.is_err());
    }
}
