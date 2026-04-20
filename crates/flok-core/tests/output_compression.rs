use std::path::PathBuf;

use flok_core::config::OutputCompressionConfig;
use flok_core::tool::{BashTool, Tool, ToolContext};

fn tool_context(cfg: OutputCompressionConfig) -> ToolContext {
    let project_root =
        std::fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| PathBuf::from("."));
    ToolContext {
        project_root,
        session_id: "test-session".to_string(),
        agent: "integration-test".to_string(),
        cancel: tokio_util::sync::CancellationToken::new(),
        lsp: None,
        output_compression: cfg,
    }
}

fn numbered_output(count: usize) -> String {
    (0..count).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n") + "\n"
}

#[tokio::test]
async fn bash_tool_long_output_is_compressed() {
    let ctx = tool_context(OutputCompressionConfig::default());
    let command = concat!(
        "i=0; ",
        "while [ $i -lt 1000 ]; do ",
        "printf '[build] compiling foo/%s.2.3\\n' \"$i\"; ",
        "i=$((i+1)); ",
        "done"
    );

    let output = BashTool.execute(serde_json::json!({"command": command}), &ctx).await.unwrap();

    assert!(!output.is_error);
    assert!(output.content.starts_with("[compression: 1000 → "));
    assert!(output.content.contains("(filter+group") || output.content.contains("(group"));
    assert!(output.content.contains("... (× 1000 similar lines)"));
}

#[tokio::test]
async fn bash_tool_short_output_is_passthrough() {
    let ctx = tool_context(OutputCompressionConfig::default());
    let command = concat!(
        "i=0; ",
        "while [ $i -lt 20 ]; do ",
        "printf 'line %s\\n' \"$i\"; ",
        "i=$((i+1)); ",
        "done"
    );

    let output = BashTool.execute(serde_json::json!({"command": command}), &ctx).await.unwrap();

    assert!(!output.is_error);
    assert_eq!(output.content, numbered_output(20));
}

#[tokio::test]
async fn compression_disabled_bypasses_pipeline() {
    let ctx = tool_context(OutputCompressionConfig {
        enabled: false,
        ..OutputCompressionConfig::default()
    });
    let command = concat!(
        "i=0; ",
        "while [ $i -lt 50 ]; do ",
        "printf 'line %s\\n' \"$i\"; ",
        "i=$((i+1)); ",
        "done"
    );

    let output = BashTool.execute(serde_json::json!({"command": command}), &ctx).await.unwrap();

    assert!(!output.is_error);
    assert_eq!(output.content, numbered_output(50));
}
