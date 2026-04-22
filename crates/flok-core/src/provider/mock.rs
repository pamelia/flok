//! Mock LLM provider for integration testing.
//!
//! The `MockProvider` replays a scripted sequence of turns. Each turn is
//! either a text response or a set of tool calls. The engine's prompt loop
//! executes tools and calls `stream()` again, which pops the next turn.

use std::collections::VecDeque;
use std::sync::Mutex;

use tokio::sync::mpsc;

use super::types::{CompletionRequest, Provider, StreamEvent};

/// A scripted LLM response turn.
#[derive(Debug, Clone)]
pub enum MockTurn {
    /// Return a plain text response (no tool calls).
    Text(String),
    /// Return one or more tool calls. The engine will execute them, then
    /// call `stream()` again for the next turn.
    ToolCalls(Vec<MockToolCall>),
}

/// A scripted tool call.
#[derive(Debug, Clone)]
pub struct MockToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// A programmable mock LLM provider.
///
/// Push turns with [`push_turn`] before running the engine. Each call to
/// `stream()` pops the next turn and sends the appropriate events.
///
/// If the queue is exhausted (the engine calls `stream()` more times than
/// there are turns), the mock returns a text response saying so.
pub struct MockProvider {
    turns: Mutex<VecDeque<MockTurn>>,
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MockProvider {
    /// Create a new empty mock provider.
    pub fn new() -> Self {
        Self { turns: Mutex::new(VecDeque::new()) }
    }

    /// Push a turn onto the back of the queue.
    pub fn push_turn(&self, turn: MockTurn) {
        self.turns.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push_back(turn);
    }
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        tx: mpsc::UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<()> {
        let turn = self
            .turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| {
                MockTurn::Text("[MockProvider] No more scripted turns.".to_string())
            });

        match turn {
            MockTurn::Text(text) => {
                let _ = tx.send(StreamEvent::TextDelta(text));
            }
            MockTurn::ToolCalls(calls) => {
                for (i, call) in calls.iter().enumerate() {
                    let id = format!("mock_tc_{i}");
                    let _ = tx.send(StreamEvent::ToolCallStart {
                        index: i,
                        id,
                        name: call.name.clone(),
                    });
                    let args_json = serde_json::to_string(&call.arguments)?;
                    let _ = tx.send(StreamEvent::ToolCallDelta { index: i, delta: args_json });
                }
            }
        }

        // Always send usage and done
        let _ = tx.send(StreamEvent::Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        });
        let _ = tx.send(StreamEvent::Done);

        Ok(())
    }
}

impl std::fmt::Debug for MockProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = self.turns.lock().map_or(0, |q| q.len());
        f.debug_struct("MockProvider").field("remaining_turns", &len).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_provider_text_turn() {
        let mock = MockProvider::new();
        mock.push_turn(MockTurn::Text("Hello from mock".into()));

        let (tx, mut rx) = mpsc::unbounded_channel();
        let request = CompletionRequest {
            model: "test".into(),
            reasoning_effort: None,
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 1024,
        };

        mock.stream(request, tx).await.unwrap();

        let mut text = String::new();
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::TextDelta(t) => text.push_str(&t),
                StreamEvent::Done => break,
                _ => {}
            }
        }
        assert_eq!(text, "Hello from mock");
    }

    #[tokio::test]
    async fn mock_provider_tool_call_turn() {
        let mock = MockProvider::new();
        mock.push_turn(MockTurn::ToolCalls(vec![MockToolCall {
            name: "bash".into(),
            arguments: serde_json::json!({"command": "echo hi"}),
        }]));

        let (tx, mut rx) = mpsc::unbounded_channel();
        let request = CompletionRequest {
            model: "test".into(),
            reasoning_effort: None,
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 1024,
        };

        mock.stream(request, tx).await.unwrap();

        let mut tool_name = String::new();
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::ToolCallStart { name, .. } => tool_name = name,
                StreamEvent::Done => break,
                _ => {}
            }
        }
        assert_eq!(tool_name, "bash");
    }

    #[tokio::test]
    async fn mock_provider_exhausted_returns_fallback() {
        let mock = MockProvider::new();
        // No turns pushed

        let (tx, mut rx) = mpsc::unbounded_channel();
        let request = CompletionRequest {
            model: "test".into(),
            reasoning_effort: None,
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 1024,
        };

        mock.stream(request, tx).await.unwrap();

        let mut text = String::new();
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::TextDelta(t) => text.push_str(&t),
                StreamEvent::Done => break,
                _ => {}
            }
        }
        assert!(text.contains("No more scripted turns"));
    }
}
