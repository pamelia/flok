//! Integration tests for undo/redo with file restoration.

use flok_core::provider::mock::{MockToolCall, MockTurn};
use flok_core::session::SendMessageResult;
use flok_core::testutil::TestHarness;

#[tokio::test]
async fn undo_removes_last_message_pair() {
    let mut h = TestHarness::new();

    // Send a message
    h.push_turn(MockTurn::Text("Hello back!".into()));
    let result = h.send_message("Hello").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));

    // Undo should remove the message pair
    let undo_result = h.engine.undo().await.unwrap();
    assert!(undo_result.is_some(), "undo should return a result");
    let undo = undo_result.unwrap();
    assert!(undo.message.contains("Undone"), "undo message should describe the action");
}

#[tokio::test]
async fn undo_restores_file_state() {
    let mut h = TestHarness::new();

    // Pre-create a file
    h.write_file("data.txt", "original content");

    // Mock: overwrite the file via write tool
    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "write".into(),
        arguments: serde_json::json!({
            "file_path": h.path("data.txt"),
            "content": "modified by agent",
        }),
    }]));
    h.push_turn(MockTurn::Text("I modified the file.".into()));

    let result = h.send_message("modify data.txt").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
    assert_eq!(h.read_file("data.txt"), "modified by agent");

    // Undo should restore the original content
    let undo_result = h.engine.undo().await.unwrap();
    assert!(undo_result.is_some());
    assert_eq!(h.read_file("data.txt"), "original content");
}

#[tokio::test]
async fn redo_restores_undone_message() {
    let mut h = TestHarness::new();

    h.write_file("data.txt", "original");

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "write".into(),
        arguments: serde_json::json!({
            "file_path": h.path("data.txt"),
            "content": "changed",
        }),
    }]));
    h.push_turn(MockTurn::Text("Changed.".into()));

    let result = h.send_message("change file").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
    assert_eq!(h.read_file("data.txt"), "changed");

    // Undo
    let undo = h.engine.undo().await.unwrap();
    assert!(undo.is_some());
    assert_eq!(h.read_file("data.txt"), "original");

    // Redo should bring back the change
    let redo = h.engine.redo().await.unwrap();
    assert!(redo.is_some());
    assert_eq!(h.read_file("data.txt"), "changed");
}

#[tokio::test]
async fn undo_nothing_returns_none() {
    let mut h = TestHarness::new();

    // No messages sent, nothing to undo
    let result = h.engine.undo().await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn undo_stack_is_lifo() {
    let mut h = TestHarness::new();

    h.write_file("counter.txt", "0");

    // Message 1: write "1"
    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "write".into(),
        arguments: serde_json::json!({
            "file_path": h.path("counter.txt"),
            "content": "1",
        }),
    }]));
    h.push_turn(MockTurn::Text("Set to 1.".into()));
    h.send_message("set to 1").await.unwrap();
    assert_eq!(h.read_file("counter.txt"), "1");

    // Message 2: write "2"
    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "write".into(),
        arguments: serde_json::json!({
            "file_path": h.path("counter.txt"),
            "content": "2",
        }),
    }]));
    h.push_turn(MockTurn::Text("Set to 2.".into()));
    h.send_message("set to 2").await.unwrap();
    assert_eq!(h.read_file("counter.txt"), "2");

    // Undo message 2 -> should restore to "1"
    h.engine.undo().await.unwrap();
    assert_eq!(h.read_file("counter.txt"), "1");

    // Undo message 1 -> should restore to "0"
    h.engine.undo().await.unwrap();
    assert_eq!(h.read_file("counter.txt"), "0");
}
