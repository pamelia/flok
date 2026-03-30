//! The `edit` tool — performs search-and-replace edits on files.

use std::path::Path;

use super::{Tool, ToolContext, ToolOutput};

/// Edit a file by replacing a string with another.
pub struct EditTool;

#[async_trait::async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Perform an exact string replacement in a file. The old_string must match exactly \
         (including whitespace and indentation). Use this for surgical edits."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["file_path", "old_string", "new_string"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact string to find and replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement string"
                }
            }
        })
    }

    fn permission_level(&self) -> super::PermissionLevel {
        super::PermissionLevel::Write
    }

    fn describe_invocation(&self, args: &serde_json::Value) -> String {
        let path = args["file_path"].as_str().unwrap_or("(unknown)");
        format!("edit: {path}")
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let file_path = args["file_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: file_path"))?;
        let old_string = args["old_string"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: old_string"))?;
        let new_string = args["new_string"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: new_string"))?;

        let full_path = resolve_path(&ctx.project_root, file_path);

        if !full_path.exists() {
            return Ok(ToolOutput::error(format!("File not found: {}", full_path.display())));
        }

        let content = tokio::fs::read_to_string(&full_path).await?;

        // Count occurrences
        let count = content.matches(old_string).count();

        if count == 0 {
            return Ok(ToolOutput::error("old_string not found in file content.".to_string()));
        }

        if count > 1 {
            return Ok(ToolOutput::error(format!(
                "Found {count} matches for old_string. Provide more surrounding context \
                 to make the match unique."
            )));
        }

        let new_content = content.replacen(old_string, new_string, 1);
        tokio::fs::write(&full_path, &new_content).await?;

        Ok(ToolOutput::success(format!("Applied edit to {}", full_path.display())))
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
    async fn edit_replaces_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");
        std::fs::write(&file, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

        let ctx = ToolContext::test(dir.path().to_path_buf());
        let args = serde_json::json!({
            "file_path": "test.rs",
            "old_string": "println!(\"hello\")",
            "new_string": "println!(\"world\")"
        });

        let result = EditTool.execute(args, &ctx).await.unwrap();
        assert!(!result.is_error);

        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.contains("println!(\"world\")"));
        assert!(!content.contains("println!(\"hello\")"));
    }

    #[tokio::test]
    async fn edit_not_found_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        let ctx = ToolContext::test(dir.path().to_path_buf());
        let args = serde_json::json!({
            "file_path": "test.rs",
            "old_string": "this does not exist",
            "new_string": "replacement"
        });

        let result = EditTool.execute(args, &ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn edit_multiple_matches_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");
        std::fs::write(&file, "aaa\naaa\naaa").unwrap();

        let ctx = ToolContext::test(dir.path().to_path_buf());
        let args = serde_json::json!({
            "file_path": "test.rs",
            "old_string": "aaa",
            "new_string": "bbb"
        });

        let result = EditTool.execute(args, &ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("3 matches"));
    }
}
