//! Integration tests for the skill loading system.

use flok_core::provider::mock::{MockToolCall, MockTurn};
use flok_core::session::SendMessageResult;
use flok_core::testutil::TestHarness;

#[tokio::test]
async fn skill_tool_loads_code_review() {
    let mut h = TestHarness::new();

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "skill".into(),
        arguments: serde_json::json!({
            "name": "code-review",
        }),
    }]));
    h.push_turn(MockTurn::Text("Loaded the skill.".into()));

    let result = h.send_message("load code review skill").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn skill_tool_loads_spec_review() {
    let mut h = TestHarness::new();

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "skill".into(),
        arguments: serde_json::json!({
            "name": "spec-review",
        }),
    }]));
    h.push_turn(MockTurn::Text("Loaded.".into()));

    let result = h.send_message("load spec review").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn skill_tool_loads_self_review_loop() {
    let mut h = TestHarness::new();

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "skill".into(),
        arguments: serde_json::json!({
            "name": "self-review-loop",
        }),
    }]));
    h.push_turn(MockTurn::Text("Loaded.".into()));

    let result = h.send_message("load self review").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn skill_tool_loads_handle_pr_feedback() {
    let mut h = TestHarness::new();

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "skill".into(),
        arguments: serde_json::json!({
            "name": "handle-pr-feedback",
        }),
    }]));
    h.push_turn(MockTurn::Text("Loaded.".into()));

    let result = h.send_message("load pr feedback handler").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn skill_tool_loads_source_driven_development() {
    let mut h = TestHarness::new();

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "skill".into(),
        arguments: serde_json::json!({
            "name": "source-driven-development",
        }),
    }]));
    h.push_turn(MockTurn::Text("Loaded.".into()));

    let result = h.send_message("load sdd skill").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn skill_tool_rejects_path_traversal() {
    let mut h = TestHarness::new();

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "skill".into(),
        arguments: serde_json::json!({
            "name": "../../../etc/passwd",
        }),
    }]));
    // After the error, the mock gives a text response
    h.push_turn(MockTurn::Text("Skill load failed as expected.".into()));

    let result = h.send_message("try path traversal").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}

#[tokio::test]
async fn skill_tool_rejects_nonexistent_skill() {
    let mut h = TestHarness::new();

    h.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
        name: "skill".into(),
        arguments: serde_json::json!({
            "name": "definitely-not-a-real-skill",
        }),
    }]));
    h.push_turn(MockTurn::Text("Skill not found.".into()));

    let result = h.send_message("load fake skill").await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)));
}
