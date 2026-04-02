//! Integration tests for the permission system.

use flok_core::provider::mock::{MockToolCall, MockTurn};
use flok_core::session::SendMessageResult;
use flok_core::testutil::TestHarness;

#[tokio::test]
async fn plan_mode_blocks_write_tools() {
    let mut h = TestHarness::new();
    h.plan_mode.set(true); // Enable plan (read-only) mode

    // Mock tries to write a file
    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "write".into(),
        arguments: serde_json::json!({
            "file_path": h.path("forbidden.txt"),
            "content": "should not exist",
        }),
    }]));
    // After the blocked tool, the mock should give a text response
    h.push_turn(MockTurn::Text("OK, write was blocked.".into()));

    let result = h.send_message("write a file").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));

    // The file should NOT exist because plan mode blocked the write
    assert!(!h.file_exists("forbidden.txt"));
}

#[tokio::test]
async fn plan_mode_allows_safe_tools() {
    let mut h = TestHarness::new();
    h.write_file("readable.txt", "hello world");
    h.plan_mode.set(true);

    // Mock calls read (a Safe tool) -- should work in plan mode
    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "read".into(),
        arguments: serde_json::json!({
            "file_path": h.path("readable.txt"),
        }),
    }]));
    h.push_turn(MockTurn::Text("Read the file successfully.".into()));

    let result = h.send_message("read readable.txt").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn plan_mode_blocks_bash_tool() {
    let mut h = TestHarness::new();
    h.plan_mode.set(true);

    // Mock tries to run bash (Dangerous)
    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "bash".into(),
        arguments: serde_json::json!({
            "command": "touch /tmp/should_not_exist_flok_test",
        }),
    }]));
    h.push_turn(MockTurn::Text("Bash was blocked.".into()));

    let result = h.send_message("run a command").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn plan_mode_blocks_edit_tool() {
    let mut h = TestHarness::new();
    h.write_file("src/keep.rs", "fn original() {}");
    h.plan_mode.set(true);

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "edit".into(),
        arguments: serde_json::json!({
            "file_path": h.path("src/keep.rs"),
            "old_string": "fn original() {}",
            "new_string": "fn modified() {}",
        }),
    }]));
    h.push_turn(MockTurn::Text("Edit blocked.".into()));

    let result = h.send_message("edit the file").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));

    // File should be unchanged
    assert_eq!(h.read_file("src/keep.rs"), "fn original() {}");
}

#[tokio::test]
async fn build_mode_allows_write_tools() {
    let mut h = TestHarness::new();
    // plan_mode defaults to false (build mode)

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "write".into(),
        arguments: serde_json::json!({
            "file_path": h.path("allowed.txt"),
            "content": "should exist",
        }),
    }]));
    h.push_turn(MockTurn::Text("Written.".into()));

    let result = h.send_message("write a file").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
    assert!(h.file_exists("allowed.txt"));
    assert_eq!(h.read_file("allowed.txt"), "should exist");
}
