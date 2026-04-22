//! Integration tests for tool execution through the engine pipeline.
//!
//! Each test scripts the mock provider to call a specific tool, then verifies
//! the tool actually executed (filesystem changes, command output, etc.).

use flok_core::provider::mock::{MockToolCall, MockTurn};
use flok_core::session::SendMessageResult;
use flok_core::testutil::TestHarness;

#[tokio::test]
async fn read_tool_returns_file_contents() {
    let mut h = TestHarness::new();
    h.write_file("src/main.rs", "fn main() { println!(\"hello\"); }");

    // Mock: call read, then respond with text
    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "read".into(),
        arguments: serde_json::json!({
            "file_path": h.path("src/main.rs"),
        }),
    }]));
    h.push_turn(MockTurn::Text("I read the file.".into()));

    let result = h.send_message("read main.rs").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn write_tool_creates_file() {
    let mut h = TestHarness::new();

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "write".into(),
        arguments: serde_json::json!({
            "file_path": h.path("output.txt"),
            "content": "hello from mock",
        }),
    }]));
    h.push_turn(MockTurn::Text("Done.".into()));

    let result = h.send_message("create output.txt").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
    assert!(h.file_exists("output.txt"));
    assert_eq!(h.read_file("output.txt"), "hello from mock");
}

#[tokio::test]
async fn edit_tool_replaces_content() {
    let mut h = TestHarness::new();
    h.write_file("src/lib.rs", "pub fn greet() -> &str { \"hello\" }");

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "edit".into(),
        arguments: serde_json::json!({
            "file_path": h.path("src/lib.rs"),
            "old_string": "\"hello\"",
            "new_string": "\"goodbye\"",
        }),
    }]));
    h.push_turn(MockTurn::Text("Updated.".into()));

    let result = h.send_message("change greeting").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
    assert_eq!(h.read_file("src/lib.rs"), "pub fn greet() -> &str { \"goodbye\" }");
}

#[tokio::test]
async fn bash_tool_executes_command() {
    let mut h = TestHarness::new();

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "bash".into(),
        arguments: serde_json::json!({
            "command": "echo 'integration test passed'",
        }),
    }]));
    h.push_turn(MockTurn::Text("Command executed.".into()));

    let result = h.send_message("run echo").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn grep_tool_finds_pattern() {
    let mut h = TestHarness::new();
    h.write_file("src/a.rs", "fn find_me_here() {}");
    h.write_file("src/b.rs", "fn nothing_special() {}");

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "grep".into(),
        arguments: serde_json::json!({
            "pattern": "find_me_here",
            "path": h.path("src"),
        }),
    }]));
    h.push_turn(MockTurn::Text("Found it.".into()));

    let result = h.send_message("search for find_me_here").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn smart_grep_tool_finds_symbol_results() {
    let mut h = TestHarness::new();
    h.write_file("src/auth.rs", "pub fn verify_token() {}\nfn refresh_token() {}\n");

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "smart_grep".into(),
        arguments: serde_json::json!({
            "pattern": "verify_*",
            "query_type": "symbol",
            "path": h.path("src"),
        }),
    }]));
    h.push_turn(MockTurn::Text("Found symbol.".into()));

    let result = h.send_message("find auth symbol").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn glob_tool_finds_files() {
    let mut h = TestHarness::new();
    h.write_file("src/main.rs", "");
    h.write_file("src/lib.rs", "");
    h.write_file("README.md", "");

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "glob".into(),
        arguments: serde_json::json!({
            "pattern": "src/**/*.rs",
            "path": h.path(""),
        }),
    }]));
    h.push_turn(MockTurn::Text("Found files.".into()));

    let result = h.send_message("find rust files").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn fast_apply_creates_new_file() {
    let mut h = TestHarness::new();

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "fast_apply".into(),
        arguments: serde_json::json!({
            "file_path": h.path("new_file.rs"),
            "snippet": "fn main() {\n    println!(\"fast apply\");\n}",
        }),
    }]));
    h.push_turn(MockTurn::Text("Created.".into()));

    let result = h.send_message("create new file with fast_apply").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
    assert!(h.file_exists("new_file.rs"));
    assert!(h.read_file("new_file.rs").contains("fast apply"));
}

#[tokio::test]
async fn multi_tool_round_sequential() {
    let mut h = TestHarness::new();

    // Round 1: write a file
    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "write".into(),
        arguments: serde_json::json!({
            "file_path": h.path("step1.txt"),
            "content": "step one",
        }),
    }]));

    // Round 2: read it back
    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "read".into(),
        arguments: serde_json::json!({
            "file_path": h.path("step1.txt"),
        }),
    }]));

    // Round 3: final text response
    h.push_turn(MockTurn::Text("Wrote and read the file.".into()));

    let result = h.send_message("write then read").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
    assert_eq!(h.read_file("step1.txt"), "step one");
}

#[tokio::test]
async fn multiple_tool_calls_in_one_round() {
    let mut h = TestHarness::new();

    // Single round with two tool calls
    h.push_turn(MockTurn::ToolCalls(vec![
        MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": h.path("a.txt"),
                "content": "file a",
            }),
        },
        MockToolCall {
            name: "write".into(),
            arguments: serde_json::json!({
                "file_path": h.path("b.txt"),
                "content": "file b",
            }),
        },
    ]));
    h.push_turn(MockTurn::Text("Both files created.".into()));

    let result = h.send_message("create two files").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
    assert_eq!(h.read_file("a.txt"), "file a");
    assert_eq!(h.read_file("b.txt"), "file b");
}
