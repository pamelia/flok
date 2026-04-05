//! Integration tests for context window management and compression.
//!
//! These tests verify that tool output truncation works correctly and
//! that the engine doesn't break when dealing with large outputs.

use flok_core::provider::mock::{MockToolCall, MockTurn};
use flok_core::session::SendMessageResult;
use flok_core::testutil::TestHarness;

#[tokio::test]
async fn large_bash_output_does_not_crash() {
    let mut h = TestHarness::new();

    // Generate a command that produces ~100KB of output
    // `seq 1 10000` produces ~50KB, which is around the truncation boundary
    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "bash".into(),
        arguments: serde_json::json!({
            "command": "seq 1 20000",
        }),
    }]));
    h.push_turn(MockTurn::Text("Processed the output.".into()));

    let result = h.send_message("generate large output").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn many_tool_rounds_complete_without_doom_loop() {
    let mut h = TestHarness::new();

    // 3 rounds of tool calls using bash (simpler, no path issues)
    for i in 0..3 {
        let path = h.path(&format!("file_{i}.txt"));
        h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "bash".into(),
            arguments: serde_json::json!({
                "command": format!("echo 'content {i}' > '{path}'"),
            }),
        }]));
    }
    // Final text response
    h.push_turn(MockTurn::Text("All files created.".into()));

    let result = h.send_message("create 3 files").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));

    // Verify all files were created
    for i in 0..3 {
        assert!(h.file_exists(&format!("file_{i}.txt")));
    }
}

#[tokio::test]
async fn large_file_read_does_not_crash() {
    let mut h = TestHarness::new();

    // Create a large file (~200KB)
    let large_content = (0..5000).fold(String::new(), |mut acc, i| {
        use std::fmt::Write;
        let _ = writeln!(acc, "line {i}: some content here");
        acc
    });
    h.write_file("large.txt", &large_content);

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "read".into(),
        arguments: serde_json::json!({
            "file_path": h.path("large.txt"),
        }),
    }]));
    h.push_turn(MockTurn::Text("Read the large file.".into()));

    let result = h.send_message("read the large file").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn sequential_messages_accumulate_history() {
    let mut h = TestHarness::new();

    // Send 3 messages in sequence
    for i in 0..3 {
        h.push_turn(MockTurn::Text(format!("Response {i}")));
        let result = h.send_message(&format!("Message {i}")).await.unwrap();
        assert!(matches!(result, SendMessageResult::Complete(_)), "message {i} should complete");
    }

    // Verify messages are in the DB
    let display = h.engine.load_display_messages().unwrap();
    // Each exchange has user + assistant = 2 messages, so 3 exchanges = 6
    assert_eq!(display.len(), 6, "should have 6 display messages (3 user + 3 assistant)");
}
