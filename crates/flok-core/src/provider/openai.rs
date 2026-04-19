//! `OpenAI` API provider (also compatible with `OpenAI`-compatible APIs like `DeepSeek`).
//!
//! Implements streaming via the Chat Completions API with SSE.

use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::models::ModelRegistry;
use super::types::{CompletionRequest, MessageContent, Provider, StreamEvent};

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// OpenAI-compatible provider.
pub struct OpenAiProvider {
    api_key: SecretString,
    base_url: String,
    client: reqwest::Client,
}

impl OpenAiProvider {
    /// Create a new `OpenAI` provider.
    pub fn new(api_key: SecretString, base_url: Option<String>) -> Self {
        Self {
            api_key,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            client: reqwest::Client::new(),
        }
    }

    /// Build the `OpenAI` API request body.
    fn build_request_body(request: &CompletionRequest) -> OpenAiRequest {
        let mut messages: Vec<OpenAiMessage> = Vec::new();

        // System message
        if !request.system.is_empty() {
            messages.push(OpenAiMessage {
                role: "system".into(),
                content: Some(request.system.clone()),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        // Conversation messages
        for msg in &request.messages {
            match msg.role.as_str() {
                "user" => {
                    // Collect text and tool results
                    let mut text_parts = String::new();
                    let mut tool_results: Vec<OpenAiMessage> = Vec::new();

                    for c in &msg.content {
                        match c {
                            MessageContent::Text { text } => {
                                if !text_parts.is_empty() {
                                    text_parts.push('\n');
                                }
                                text_parts.push_str(text);
                            }
                            MessageContent::ToolResult { tool_use_id, content, .. } => {
                                tool_results.push(OpenAiMessage {
                                    role: "tool".into(),
                                    content: Some(content.clone()),
                                    tool_calls: None,
                                    tool_call_id: Some(tool_use_id.clone()),
                                });
                            }
                            MessageContent::ToolUse { .. } | MessageContent::Thinking { .. } => {}
                        }
                    }

                    if !text_parts.is_empty() {
                        messages.push(OpenAiMessage {
                            role: "user".into(),
                            content: Some(text_parts),
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }

                    messages.extend(tool_results);
                }
                "assistant" => {
                    let mut text = String::new();
                    let mut tool_calls: Vec<OpenAiToolCall> = Vec::new();

                    for c in &msg.content {
                        match c {
                            MessageContent::Text { text: t } => {
                                text.push_str(t);
                            }
                            MessageContent::ToolUse { id, name, input } => {
                                tool_calls.push(OpenAiToolCall {
                                    id: id.clone(),
                                    r#type: "function".into(),
                                    function: OpenAiFunctionCall {
                                        name: name.clone(),
                                        arguments: serde_json::to_string(input).unwrap_or_default(),
                                    },
                                });
                            }
                            MessageContent::ToolResult { .. } | MessageContent::Thinking { .. } => {
                            }
                        }
                    }

                    messages.push(OpenAiMessage {
                        role: "assistant".into(),
                        content: if text.is_empty() { None } else { Some(text) },
                        tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
                        tool_call_id: None,
                    });
                }
                _ => {}
            }
        }

        // Tools
        let tools: Vec<OpenAiTool> = request
            .tools
            .iter()
            .map(|t| OpenAiTool {
                r#type: "function".into(),
                function: OpenAiFunction {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.input_schema.clone(),
                },
            })
            .collect();

        OpenAiRequest {
            model: ModelRegistry::model_name(&request.model).to_string(),
            messages,
            tools: if tools.is_empty() { None } else { Some(tools) },
            max_completion_tokens: Some(request.max_tokens),
            stream: true,
            stream_options: Some(StreamOptions { include_usage: true }),
        }
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        tx: mpsc::UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<()> {
        let body = Self::build_request_body(&request);
        let url = format!("{}/chat/completions", self.base_url);

        tracing::debug!(model = %body.model, url = %url, "sending OpenAI request");

        let req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key.expose_secret()))
            .header("content-type", "application/json")
            .json(&body);

        let mut es = EventSource::new(req)?;

        while let Some(event) = es.next().await {
            match event {
                Ok(Event::Open) => {}
                Ok(Event::Message(msg)) => {
                    if msg.data == "[DONE]" {
                        let _ = tx.send(StreamEvent::Done);
                        break;
                    }

                    let stream_event = parse_sse_chunk(&msg.data);
                    let is_done = matches!(&stream_event, StreamEvent::Done);
                    if tx.send(stream_event).is_err() {
                        break;
                    }
                    if is_done {
                        break;
                    }
                }
                Err(reqwest_eventsource::Error::StreamEnded) => {
                    let _ = tx.send(StreamEvent::Done);
                    break;
                }
                Err(reqwest_eventsource::Error::InvalidStatusCode(status, response)) => {
                    let body_text = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "(could not read response body)".into());
                    let _ = tx.send(StreamEvent::Error(format!("HTTP {status}: {body_text}")));
                    break;
                }
                Err(e) => {
                    let _ = tx.send(StreamEvent::Error(e.to_string()));
                    break;
                }
            }
        }
        es.close();

        Ok(())
    }
}

/// Parse an `OpenAI` SSE chunk.
fn parse_sse_chunk(data: &str) -> StreamEvent {
    let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data) else {
        return StreamEvent::Error(format!("failed to parse chunk: {data}"));
    };

    // Usage info (sent in the final chunk when stream_options.include_usage = true)
    if let Some(usage) = chunk.usage {
        return StreamEvent::Usage {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
    }

    if chunk.choices.is_empty() {
        return StreamEvent::TextDelta(String::new());
    }

    let choice = &chunk.choices[0];
    let delta = &choice.delta;

    // Tool calls
    if let Some(ref tool_calls) = delta.tool_calls {
        for tc in tool_calls {
            if let Some(ref function) = tc.function {
                if function.name.is_some() {
                    // Tool call start
                    return StreamEvent::ToolCallStart {
                        index: tc.index,
                        id: tc.id.clone().unwrap_or_default(),
                        name: function.name.clone().unwrap_or_default(),
                    };
                }
                if let Some(ref args) = function.arguments {
                    return StreamEvent::ToolCallDelta { index: tc.index, delta: args.clone() };
                }
            }
        }
    }

    // Text content
    if let Some(ref content) = delta.content {
        return StreamEvent::TextDelta(content.clone());
    }

    // Finish reason
    if choice.finish_reason.is_some() {
        return StreamEvent::Done;
    }

    StreamEvent::TextDelta(String::new())
}

// ---------------------------------------------------------------------------
// OpenAI API types (private, for serialization only)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Serialize)]
struct OpenAiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct OpenAiToolCall {
    id: String,
    r#type: String,
    function: OpenAiFunctionCall,
}

#[derive(Serialize)]
struct OpenAiFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct OpenAiTool {
    r#type: String,
    function: OpenAiFunction,
}

#[derive(Serialize)]
struct OpenAiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

// -- SSE deserialization types --

#[derive(Deserialize)]
struct ChatCompletionChunk {
    choices: Vec<ChunkChoice>,
    usage: Option<ChunkUsage>,
}

#[derive(Deserialize)]
struct ChunkChoice {
    delta: ChunkDelta,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChunkDelta {
    content: Option<String>,
    tool_calls: Option<Vec<ChunkToolCall>>,
}

#[derive(Deserialize)]
struct ChunkToolCall {
    index: usize,
    id: Option<String>,
    function: Option<ChunkFunction>,
}

#[derive(Deserialize)]
struct ChunkFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct ChunkUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_constructs_with_secret_string() {
        use secrecy::SecretString;

        let _ = OpenAiProvider::new(SecretString::from("test-key".to_string()), None);
    }

    #[test]
    fn parse_text_delta_chunk() {
        let data = r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"gpt-5.4","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let event = parse_sse_chunk(data);
        assert!(matches!(event, StreamEvent::TextDelta(t) if t == "Hello"));
    }

    #[test]
    fn parse_tool_call_start_chunk() {
        let data = r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"gpt-5.4","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc","type":"function","function":{"name":"read","arguments":""}}]},"finish_reason":null}]}"#;
        let event = parse_sse_chunk(data);
        assert!(
            matches!(event, StreamEvent::ToolCallStart { index: 0, id, name } if id == "call_abc" && name == "read")
        );
    }

    #[test]
    fn parse_tool_call_delta_chunk() {
        let data = r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"gpt-5.4","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"file"}}]},"finish_reason":null}]}"#;
        let event = parse_sse_chunk(data);
        assert!(
            matches!(event, StreamEvent::ToolCallDelta { index: 0, delta } if delta == "{\"file")
        );
    }

    #[test]
    fn parse_finish_reason_chunk() {
        let data = r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"gpt-5.4","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;
        let event = parse_sse_chunk(data);
        assert!(matches!(event, StreamEvent::Done));
    }

    #[test]
    fn parse_usage_chunk() {
        let data = r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"gpt-5.4","choices":[],"usage":{"prompt_tokens":100,"completion_tokens":50,"total_tokens":150}}"#;
        let event = parse_sse_chunk(data);
        assert!(matches!(event, StreamEvent::Usage { input_tokens: 100, output_tokens: 50, .. }));
    }

    #[test]
    fn build_request_body_with_tools() {
        let request = CompletionRequest {
            model: "openai/gpt-5.4".into(),
            system: "You are a helper.".into(),
            messages: vec![super::super::types::Message {
                role: "user".into(),
                content: vec![MessageContent::Text { text: "hello".into() }],
            }],
            tools: vec![super::super::types::ToolDefinition {
                name: "read".into(),
                description: "Read a file.".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            max_tokens: 4096,
        };

        let body = OpenAiProvider::build_request_body(&request);
        assert_eq!(body.model, "gpt-5.4");
        assert!(body.stream);
        assert_eq!(body.messages.len(), 2); // system + user
        assert!(body.tools.is_some());
        assert_eq!(body.tools.as_ref().unwrap().len(), 1);

        // Verify serialization
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("gpt-5.4"));
        assert!(json.contains("function"));
    }
}
