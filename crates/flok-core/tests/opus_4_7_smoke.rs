//! Live API smoke test for Claude Opus 4.7.
//!
//! This test is `#[ignore]` by default because it makes a real billed
//! Anthropic API call (~$0.001 per run). To execute it:
//!
//! ```sh
//! ANTHROPIC_API_KEY=sk-... cargo test -p flok-core opus_4_7 -- --ignored --nocapture
//! ```
//!
//! Passes when the provider streams at least one text delta and a clean
//! completion event with no errors — proving that Flok's request shape
//! is accepted by Opus 4.7 end-to-end.

use flok_core::provider::{
    AnthropicProvider, CompletionRequest, Message, MessageContent, Provider, StreamEvent,
};
use secrecy::SecretString;

#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY and makes a real billed API call"]
async fn opus_4_7_accepts_floks_request_shape() {
    let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") else {
        // Skip cleanly rather than panicking — allows `cargo test -- --ignored`
        // without an API key to not blow up.
        return;
    };

    let provider = AnthropicProvider::new(SecretString::from(api_key), None);
    let request = CompletionRequest {
        model: "anthropic/claude-opus-4-7".into(),
        reasoning_effort: None,
        system: "You are a terse assistant.".into(),
        messages: vec![Message {
            role: "user".into(),
            content: vec![MessageContent::Text {
                text: "Reply with only the single word: ok".into(),
            }],
        }],
        tools: vec![],
        max_tokens: 32,
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
    let stream_task = tokio::spawn(async move { provider.stream(request, tx).await });

    let mut saw_text_delta = false;
    let mut saw_done = false;
    let mut accumulated = String::new();
    let mut errors: Vec<String> = Vec::new();

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TextDelta(delta) => {
                saw_text_delta = true;
                accumulated.push_str(&delta);
            }
            StreamEvent::Done => {
                saw_done = true;
            }
            StreamEvent::Error(message) => {
                errors.push(message);
            }
            _ => {}
        }
    }

    // Ensure the background stream task did not error out.
    let stream_result = stream_task.await.expect("stream task panicked");
    stream_result.expect("provider.stream returned Err");

    assert!(
        errors.is_empty(),
        "Opus 4.7 returned error events (this likely means our request shape is invalid for 4.7): {errors:?}"
    );
    assert!(saw_text_delta, "expected at least one TextDelta event from Opus 4.7");
    assert!(saw_done, "expected a completion event from Opus 4.7");
    assert!(!accumulated.is_empty(), "expected non-empty accumulated response from Opus 4.7");
}
