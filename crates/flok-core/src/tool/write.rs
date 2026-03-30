//! The `write` tool — writes content to a file.

use std::path::Path;

use super::{Tool, ToolContext, ToolOutput};

/// Write content to a file, creating it if it doesn't exist.
pub struct WriteTool;

#[async_trait::async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "write"
    }

    fn description(&self) -> &'static str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["file_path", "content"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The path to the file to write (relative to project root or absolute)"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            }
        })
    }

    fn permission_level(&self) -> super::PermissionLevel {
        super::PermissionLevel::Write
    }

    fn describe_invocation(&self, args: &serde_json::Value) -> String {
        let path = args["file_path"].as_str().unwrap_or("(unknown)");
        format!("write: {path}")
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let file_path = args["file_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: file_path"))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: content"))?;

        let full_path = resolve_path(&ctx.project_root, file_path);

        // Create parent directories if needed
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(&full_path, content).await?;

        let line_count = content.lines().count();
        Ok(ToolOutput::success(format!("Wrote {} lines to {}", line_count, full_path.display())))
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

    #[tokio::test]
    async fn write_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());
        let args = serde_json::json!({
            "file_path": "test.txt",
            "content": "hello\nworld"
        });

        let result = WriteTool.execute(args, &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("2 lines"));

        let content = std::fs::read_to_string(dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "hello\nworld");
    }

    #[tokio::test]
    async fn write_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());
        let args = serde_json::json!({
            "file_path": "deep/nested/dir/file.txt",
            "content": "content"
        });

        let result = WriteTool.execute(args, &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(dir.path().join("deep/nested/dir/file.txt").exists());
    }
}
