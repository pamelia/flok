//! The `read` tool — reads file contents.

use std::fmt::Write;

use super::{Tool, ToolContext, ToolOutput};

/// Read a file's contents.
pub struct ReadTool;

#[async_trait::async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read the contents of a file. Returns the file content with line numbers."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["file_path"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The path to the file to read (relative to project root or absolute)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-indexed, default: 1)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read (default: 2000)"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let file_path = args["file_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: file_path"))?;
        let offset = args["offset"].as_u64().unwrap_or(1).max(1) as usize;
        let limit = args["limit"].as_u64().unwrap_or(2000) as usize;

        let full_path = resolve_path(&ctx.project_root, file_path);

        if !full_path.exists() {
            return Ok(ToolOutput::error(format!("File not found: {}", full_path.display())));
        }

        if full_path.is_dir() {
            // List directory contents
            let mut entries: Vec<String> = Vec::new();
            let mut dir = tokio::fs::read_dir(&full_path).await?;
            while let Some(entry) = dir.next_entry().await? {
                let name = entry.file_name().to_string_lossy().to_string();
                let suffix = if entry.file_type().await?.is_dir() { "/" } else { "" };
                entries.push(format!("{name}{suffix}"));
            }
            entries.sort();
            return Ok(ToolOutput::success(entries.join("\n")));
        }

        let content = tokio::fs::read_to_string(&full_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", full_path.display(), e))?;

        if let Some(lsp) = &ctx.lsp {
            if let Err(error) = lsp.track_read(&full_path, content.clone()).await {
                tracing::debug!(path = %full_path.display(), %error, "failed to sync read with lsp");
            }
        }

        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();
        let start = (offset - 1).min(total);
        let end = (start + limit).min(total);
        let selected = &lines[start..end];

        let mut output = String::new();
        for (i, line) in selected.iter().enumerate() {
            let line_num = start + i + 1;
            let _ = writeln!(output, "{line_num}: {line}");
        }

        if end < total {
            let _ = write!(
                output,
                "\n(Showing lines {}-{} of {}. Use offset={} to continue.)",
                start + 1,
                end,
                total,
                end + 1
            );
        }

        Ok(ToolOutput::success(output))
    }
}

/// Resolve a file path relative to the project root.
fn resolve_path(project_root: &std::path::Path, file_path: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(file_path);
    let resolved = if path.is_absolute() { path.to_path_buf() } else { project_root.join(path) };
    match std::fs::canonicalize(&resolved) {
        Ok(canonical) if canonical.starts_with(project_root) => canonical,
        _ => resolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn resolve_path_blocks_traversal() {
        let root = PathBuf::from("/project");
        let resolved = resolve_path(&root, "/project/../../../etc/passwd");
        assert_eq!(resolved, PathBuf::from("/project/../../../etc/passwd"));
    }

    #[test]
    fn resolve_path_allows_inside_root() {
        let root = PathBuf::from("/project");
        let resolved = resolve_path(&root, "src/main.rs");
        assert_eq!(resolved, PathBuf::from("/project/src/main.rs"));
    }

    #[tokio::test]
    async fn read_nonexistent_file_returns_error() {
        let tool = ReadTool;
        let ctx = ToolContext::test(PathBuf::from("/nonexistent"));
        let args = serde_json::json!({"file_path": "nope.txt"});
        let result = tool.execute(args, &ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn read_existing_file() {
        let tool = ReadTool;
        let ctx = ToolContext::test(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let args = serde_json::json!({"file_path": "Cargo.toml"});
        let result = tool.execute(args, &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("flok-core"));
    }
}
