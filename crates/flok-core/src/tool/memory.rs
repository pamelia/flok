//! The `agent_memory` tool — read/write persistent per-agent memory.
//!
//! Memory is stored in `.flok/memory/<agent>.md` files. Each agent has
//! independent memory scoped to the project.

use std::path::PathBuf;

use super::{Tool, ToolContext, ToolOutput};

/// Read, write, or append to persistent agent memory.
pub struct AgentMemoryTool;

#[async_trait::async_trait]
impl Tool for AgentMemoryTool {
    fn name(&self) -> &'static str {
        "agent_memory"
    }

    fn description(&self) -> &'static str {
        "Read or write persistent per-agent memory for this project. \
         Memory persists across sessions. Use 'read' to recall past learnings, \
         'write' to replace memory, 'append' to add incrementally."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["operation"],
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["read", "write", "append"],
                    "description": "The operation: read, write (replace), or append"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write or append (required for write/append)"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let operation = args["operation"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: operation"))?;

        let memory_dir = ctx.project_root.join(".flok").join("memory");
        let memory_file = memory_dir.join(format!("{}.md", ctx.agent));

        match operation {
            "read" => read_memory(&memory_file).await,
            "write" => {
                let content = args["content"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("'content' required for write operation"))?;
                write_memory(&memory_dir, &memory_file, content).await
            }
            "append" => {
                let content = args["content"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("'content' required for append operation"))?;
                append_memory(&memory_dir, &memory_file, content).await
            }
            _ => Ok(ToolOutput::error(format!(
                "Unknown operation: {operation}. Use 'read', 'write', or 'append'."
            ))),
        }
    }
}

async fn read_memory(path: &PathBuf) -> anyhow::Result<ToolOutput> {
    if !path.exists() {
        return Ok(ToolOutput::success("(no memory stored yet)"));
    }
    let content = tokio::fs::read_to_string(path).await?;
    if content.is_empty() {
        Ok(ToolOutput::success("(memory is empty)"))
    } else {
        Ok(ToolOutput::success(content))
    }
}

async fn write_memory(dir: &PathBuf, path: &PathBuf, content: &str) -> anyhow::Result<ToolOutput> {
    tokio::fs::create_dir_all(dir).await?;
    tokio::fs::write(path, content).await?;
    Ok(ToolOutput::success(format!("Memory written ({} chars)", content.len())))
}

async fn append_memory(dir: &PathBuf, path: &PathBuf, content: &str) -> anyhow::Result<ToolOutput> {
    tokio::fs::create_dir_all(dir).await?;
    let mut existing =
        if path.exists() { tokio::fs::read_to_string(path).await? } else { String::new() };
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str(content);
    tokio::fs::write(path, &existing).await?;
    Ok(ToolOutput::success(format!("Memory appended ({} chars total)", existing.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::test(dir.to_path_buf())
    }

    #[tokio::test]
    async fn read_empty_memory() {
        let dir = tempfile::tempdir().unwrap();
        let tool = AgentMemoryTool;
        let result = tool
            .execute(serde_json::json!({"operation": "read"}), &test_ctx(dir.path()))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("no memory"));
    }

    #[tokio::test]
    async fn write_and_read_memory() {
        let dir = tempfile::tempdir().unwrap();
        let tool = AgentMemoryTool;
        let ctx = test_ctx(dir.path());

        // Write
        let result = tool
            .execute(
                serde_json::json!({"operation": "write", "content": "learned: use cargo test"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        // Read back
        let result = tool.execute(serde_json::json!({"operation": "read"}), &ctx).await.unwrap();
        assert!(result.content.contains("cargo test"));
    }

    #[tokio::test]
    async fn append_memory() {
        let dir = tempfile::tempdir().unwrap();
        let tool = AgentMemoryTool;
        let ctx = test_ctx(dir.path());

        tool.execute(serde_json::json!({"operation": "write", "content": "first"}), &ctx)
            .await
            .unwrap();

        tool.execute(serde_json::json!({"operation": "append", "content": "second"}), &ctx)
            .await
            .unwrap();

        let result = tool.execute(serde_json::json!({"operation": "read"}), &ctx).await.unwrap();
        assert!(result.content.contains("first"));
        assert!(result.content.contains("second"));
    }
}
