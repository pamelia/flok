//! Integration tests for session branching and tree navigation.

use flok_core::provider::mock::MockTurn;
use flok_core::session::{flatten_tree, SendMessageResult};
use flok_core::testutil::TestHarness;

/// Helper: send a message and assert it completes.
async fn send_ok(h: &mut TestHarness, text: &str) {
    let result = h.send_message(text).await.unwrap();
    assert!(matches!(result, SendMessageResult::Complete(_)), "expected Complete, got {result:?}");
}

#[tokio::test]
async fn branch_copies_messages_up_to_point() {
    let mut h = TestHarness::new();

    // Send two messages (each needs a text response turn)
    h.push_turn(MockTurn::Text("Response 1".into()));
    send_ok(&mut h, "First message").await;

    h.push_turn(MockTurn::Text("Response 2".into()));
    send_ok(&mut h, "Second message").await;

    // List branch points — should have 2 user messages
    let points = h.engine.list_branch_points().unwrap();
    assert_eq!(points.len(), 2);

    // Branch at the first user message
    // The summary generation will call the mock for one more turn
    h.push_turn(MockTurn::Text("Summary of abandoned work".into()));

    let branch_result = h.engine.branch_at_message(&points[0].0).await.unwrap();
    assert_eq!(branch_result.messages_copied, 1); // Only the first user message
    assert!(branch_result.summary_generated);
}

#[tokio::test]
async fn tree_structure_reflects_branches() {
    let mut h = TestHarness::new();

    // Send a message to create some history
    h.push_turn(MockTurn::Text("Response".into()));
    send_ok(&mut h, "Hello").await;

    let points = h.engine.list_branch_points().unwrap();
    assert!(!points.is_empty());

    // Create two branches from the same point
    h.push_turn(MockTurn::Text("Summary A".into()));
    let branch_a = h.engine.branch_at_message(&points[0].0).await.unwrap();

    h.push_turn(MockTurn::Text("Summary B".into()));
    let branch_b = h.engine.branch_at_message(&points[0].0).await.unwrap();

    // Build the tree and verify structure
    let tree = h.engine.session_tree().unwrap();
    assert_eq!(tree.len(), 1, "should have one root");

    let root = &tree[0];
    assert_eq!(root.children.len(), 2, "root should have 2 branches");

    // Verify branch IDs
    let child_ids: Vec<&str> = root.children.iter().map(|c| c.session.id.as_str()).collect();
    assert!(child_ids.contains(&branch_a.session_id.as_str()));
    assert!(child_ids.contains(&branch_b.session_id.as_str()));
}

#[tokio::test]
async fn flatten_tree_depth_order() {
    let mut h = TestHarness::new();

    h.push_turn(MockTurn::Text("Response".into()));
    send_ok(&mut h, "Hello").await;

    let points = h.engine.list_branch_points().unwrap();
    h.push_turn(MockTurn::Text("Summary".into()));
    let _branch = h.engine.branch_at_message(&points[0].0).await.unwrap();

    let tree = h.engine.session_tree().unwrap();
    let flat = flatten_tree(&tree);

    assert_eq!(flat.len(), 2);
    assert_eq!(flat[0].0, 0); // root at depth 0
    assert_eq!(flat[1].0, 1); // branch at depth 1
}

#[tokio::test]
async fn session_tree_text_format() {
    let mut h = TestHarness::new();

    h.push_turn(MockTurn::Text("Response".into()));
    send_ok(&mut h, "Hello").await;

    let text = h.engine.session_tree_text().unwrap();
    assert!(text.contains("Session Tree"), "should have header");
    // Current session marker
    assert!(text.contains('\u{25CF}'), "should have current session marker");
}

#[tokio::test]
async fn label_persists_and_shows_in_tree() {
    let mut h = TestHarness::new();

    h.push_turn(MockTurn::Text("Response".into()));
    send_ok(&mut h, "Hello").await;

    h.engine.set_label("checkpoint: auth working").unwrap();

    let tree = h.engine.session_tree().unwrap();
    assert_eq!(tree[0].label.as_deref(), Some("checkpoint: auth working"));

    let text = h.engine.session_tree_text().unwrap();
    assert!(text.contains("checkpoint: auth working"));
}
