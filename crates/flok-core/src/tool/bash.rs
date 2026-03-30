//! The `bash` tool — executes shell commands.

use std::fmt::Write;
use std::time::Duration;

use super::{Tool, ToolContext, ToolOutput};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Execute a shell command.
pub struct BashTool;

#[async_trait::async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Execute a bash command in the project directory. Returns stdout and stderr."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 120000)"
                }
            }
        })
    }

    fn permission_level(&self) -> super::PermissionLevel {
        super::PermissionLevel::Dangerous
    }

    fn describe_invocation(&self, args: &serde_json::Value) -> String {
        let cmd = args["command"].as_str().unwrap_or("(unknown)");
        // Show first 100 chars of command
        if cmd.len() > 100 {
            format!("bash: {}...", &cmd[..100])
        } else {
            format!("bash: {cmd}")
        }
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: command"))?;
        let timeout_ms = args["timeout"].as_u64();
        let timeout = timeout_ms.map_or(DEFAULT_TIMEOUT, Duration::from_millis);

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&ctx.project_root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let child = cmd.spawn()?;

        let result = tokio::time::timeout(timeout, child.wait_with_output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let exit_code = output.status.code().unwrap_or(-1);

                let mut content = String::new();
                if !stdout.is_empty() {
                    content.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str("STDERR:\n");
                    content.push_str(&stderr);
                }
                if content.is_empty() {
                    content.push_str("(no output)");
                }

                if exit_code != 0 {
                    let _ = write!(content, "\n\nExit code: {exit_code}");
                    // Never compress error output
                    Ok(ToolOutput::error(content))
                } else {
                    // Compress successful output through the shell pipeline
                    let compressed = crate::compress::compress_shell_output(
                        &content, command, 16_000, // ~4096 tokens * 4 chars/token
                    );
                    if compressed.ratio() > 0.05 {
                        tracing::debug!(
                            command,
                            original = compressed.original_chars,
                            compressed = compressed.compressed_chars,
                            ratio = format!("{:.1}%", compressed.ratio() * 100.0),
                            "shell output compressed"
                        );
                    }
                    Ok(ToolOutput::success(compressed.text))
                }
            }
            Ok(Err(e)) => Ok(ToolOutput::error(format!("Failed to execute command: {e}"))),
            Err(_) => {
                Ok(ToolOutput::error(format!("Command timed out after {}s", timeout.as_secs())))
            }
        }
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
    async fn bash_echo() {
        let result = BashTool
            .execute(serde_json::json!({"command": "echo hello"}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("hello"));
    }

    #[tokio::test]
    async fn bash_nonzero_exit_is_error() {
        let result =
            BashTool.execute(serde_json::json!({"command": "exit 1"}), &test_ctx()).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Exit code: 1"));
    }

    #[tokio::test]
    async fn bash_timeout() {
        let result = BashTool
            .execute(serde_json::json!({"command": "sleep 10", "timeout": 100}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("timed out"));
    }
}
