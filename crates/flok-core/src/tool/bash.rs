//! The `bash` tool — executes shell commands.

use std::fmt::Write;
use std::time::Duration;

use super::compression::CompressionPipeline;
use super::{Tool, ToolContext, ToolOutput};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const STRIPPED_ENV_VARS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "NODE_OPTIONS",
    "PYTHONPATH",
    "PYTHONHOME",
    "RUBYOPT",
    "RUBYLIB",
    "PERL5OPT",
    "PERLLIB",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteMode {
    Single,
    Double,
}

fn parse_command(command: &str) -> anyhow::Result<(String, Vec<String>)> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote = None;

    while let Some(ch) = chars.next() {
        match quote {
            None => match ch {
                ' ' | '\t' => {
                    if !current.is_empty() {
                        parts.push(std::mem::take(&mut current));
                    }
                }
                '\n' | '\r' | '|' | '&' | ';' | '>' | '<' | '(' | ')' | '$' | '`' => {
                    anyhow::bail!("unsupported shell syntax: '{ch}'");
                }
                '\'' => quote = Some(QuoteMode::Single),
                '"' => quote = Some(QuoteMode::Double),
                '\\' => {
                    let escaped = chars
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("dangling escape at end of command"))?;
                    current.push(escaped);
                }
                _ => current.push(ch),
            },
            Some(QuoteMode::Single) => {
                if ch == '\'' {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            Some(QuoteMode::Double) => match ch {
                '"' => quote = None,
                '\\' => {
                    let escaped = chars
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("dangling escape at end of command"))?;
                    current.push(escaped);
                }
                '$' | '`' => anyhow::bail!("unsupported shell syntax: '{ch}'"),
                _ => current.push(ch),
            },
        }
    }

    if quote.is_some() {
        anyhow::bail!("unterminated quoted string");
    }

    if !current.is_empty() {
        parts.push(current);
    }

    let Some((program, args)) = parts.split_first() else {
        anyhow::bail!("command is empty");
    };

    if program.contains('=') {
        anyhow::bail!("environment variable prefixes are not supported");
    }

    Ok((program.clone(), args.to_vec()))
}

fn scrub_child_env(cmd: &mut tokio::process::Command) {
    for env_var in STRIPPED_ENV_VARS {
        cmd.env_remove(env_var);
    }
}

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
        let (program, program_args) = match parse_command(command) {
            Ok(parsed) => parsed,
            Err(error) => {
                return Ok(ToolOutput::error(format!("Unsupported bash command: {error}")));
            }
        };

        let mut cmd = tokio::process::Command::new(&program);
        cmd.args(&program_args)
            .current_dir(&ctx.project_root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        scrub_child_env(&mut cmd);

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
                    let cfg = &ctx.output_compression;
                    if !cfg.enabled || !cfg.apply_to_tools.iter().any(|tool| tool == self.name()) {
                        return Ok(ToolOutput::success(content));
                    }

                    let compressed = CompressionPipeline::new(cfg).compress(&content);
                    if compressed.stages_applied.is_empty() {
                        return Ok(ToolOutput::success(compressed.output));
                    }

                    tracing::debug!(
                        tool = self.name(),
                        program,
                        original_lines = compressed.original_lines,
                        final_lines = compressed.final_lines,
                        original_chars = compressed.original_chars,
                        final_chars = compressed.final_chars,
                        stages = ?compressed.stages_applied,
                        "tool output compression applied"
                    );

                    let header = format!(
                        "[compression: {} → {} lines ({})]\n",
                        compressed.original_lines,
                        compressed.final_lines,
                        compressed.stages_applied.join("+")
                    );
                    Ok(ToolOutput::success(format!("{header}{}", compressed.output)))
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
            .execute(serde_json::json!({"command": "echo 'hello world'"}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("hello world"));
    }

    #[tokio::test]
    async fn bash_nonzero_exit_is_error() {
        let result =
            BashTool.execute(serde_json::json!({"command": "false"}), &test_ctx()).await.unwrap();
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

    #[test]
    fn parse_command_supports_quoted_arguments() {
        let (program, args) =
            parse_command(r#"echo "hello world" 'from test'"#).expect("parsed command");
        assert_eq!(program, "echo");
        assert_eq!(args, vec!["hello world", "from test"]);
    }

    #[test]
    fn parse_command_rejects_shell_operators() {
        let error = parse_command("echo hello | cat").expect_err("expected shell syntax error");
        assert!(error.to_string().contains("unsupported shell syntax"));
    }

    #[test]
    fn parse_command_rejects_env_prefixes() {
        let error =
            parse_command("PYTHONPATH=/tmp python3 script.py").expect_err("expected env prefix");
        assert!(error.to_string().contains("environment variable prefixes"));
    }

    #[tokio::test]
    async fn bash_rejects_pipe_syntax() {
        let result = BashTool
            .execute(serde_json::json!({"command": "echo hello | cat"}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Unsupported bash command"));
    }
}
