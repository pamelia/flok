//! Anthropic Claude API provider.
//!
//! Implements streaming via the Messages API with SSE (Server-Sent Events).
//! Supports tool use, prompt caching, and extended thinking.

use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::models::ModelRegistry;
use super::types::{CompletionRequest, MessageContent, Provider, StreamEvent};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

/// Anthropic Claude provider.
pub struct AnthropicProvider {
    api_key: SecretString,
    base_url: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider.
    pub fn new(api_key: SecretString, base_url: Option<String>) -> Self {
        Self {
            api_key,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            client: reqwest::Client::new(),
        }
    }

    /// Build the Anthropic API request body from a `CompletionRequest`.
    fn build_request_body(request: &CompletionRequest) -> AnthropicRequest {
        let mut messages: Vec<AnthropicMessage> = request
            .messages
            .iter()
            .map(|msg| {
                let content: Vec<AnthropicContent> = msg
                    .content
                    .iter()
                    .map(|c| match c {
                        MessageContent::Text { text } => {
                            AnthropicContent::Text { text: text.clone(), cache_control: None }
                        }
                        MessageContent::ToolUse { id, name, input } => AnthropicContent::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        },
                        MessageContent::ToolResult { tool_use_id, content, is_error } => {
                            AnthropicContent::ToolResult {
                                tool_use_id: tool_use_id.clone(),
                                content: content.clone(),
                                is_error: *is_error,
                                cache_control: None,
                            }
                        }
                        // Thinking blocks are internal — don't send back to API
                        MessageContent::Thinking { .. } => {
                            AnthropicContent::Text { text: String::new(), cache_control: None }
                        }
                    })
                    .filter(
                        |c| !matches!(c, AnthropicContent::Text { text, .. } if text.is_empty()),
                    )
                    .collect();
                AnthropicMessage { role: msg.role.clone(), content }
            })
            .collect();

        let mut tools: Vec<AnthropicTool> = request
            .tools
            .iter()
            .map(|t| AnthropicTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
                cache_control: None,
            })
            .collect();

        // Add cache_control to the last tool for prompt caching
        // (Anthropic caches everything up to and including the marked block)
        if let Some(last_tool) = tools.last_mut() {
            last_tool.cache_control = Some(CacheControl { r#type: "ephemeral" });
        }

        // Add cache_control breakpoint to the last content block of the
        // second-to-last user message. This caches the conversation prefix
        // so only the latest turn incurs input token costs.
        if messages.len() >= 2 {
            // Find the second-to-last user message (the one before the current turn)
            let last_user_idx = messages.iter().rposition(|m| m.role == "user");
            let second_last_user_idx = last_user_idx
                .and_then(|last| messages[..last].iter().rposition(|m| m.role == "user"));

            if let Some(idx) = second_last_user_idx {
                if let Some(
                    AnthropicContent::Text { cache_control, .. }
                    | AnthropicContent::ToolResult { cache_control, .. },
                ) = messages[idx].content.last_mut()
                {
                    *cache_control = Some(CacheControl { r#type: "ephemeral" });
                }
            }
        }

        // System prompt as a structured block with cache_control
        let system = vec![SystemBlock {
            r#type: "text",
            text: request.system.clone(),
            cache_control: Some(CacheControl { r#type: "ephemeral" }),
        }];

        AnthropicRequest {
            model: ModelRegistry::model_name(&request.model).to_string(),
            max_tokens: request.max_tokens,
            system,
            messages,
            tools: if tools.is_empty() { None } else { Some(tools) },
            stream: true,
        }
    }
}

#[async_trait::async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        tx: mpsc::UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<()> {
        let body = Self::build_request_body(&request);
        let url = format!("{}/v1/messages", self.base_url);

        tracing::debug!(
            model = %body.model,
            url = %url,
            "sending Anthropic request"
        );

        let req = self
            .client
            .post(&url)
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", API_VERSION)
            .header("anthropic-beta", "prompt-caching-2024-07-31")
            .header("content-type", "application/json")
            .json(&body);

        let mut es = EventSource::new(req)?;

        while let Some(event) = es.next().await {
            match event {
                Ok(Event::Open) => {}
                Ok(Event::Message(msg)) => {
                    let stream_event = parse_sse_event(&msg.event, &msg.data);
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
                    // Extract error body for better diagnostics
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

/// Parse an Anthropic SSE event into a `StreamEvent`.
fn parse_sse_event(event_type: &str, data: &str) -> StreamEvent {
    match event_type {
        "content_block_start" => {
            if let Ok(ev) = serde_json::from_str::<ContentBlockStart>(data) {
                match ev.content_block {
                    ContentBlock::Text { .. } => StreamEvent::TextDelta(String::new()),
                    ContentBlock::Thinking { .. } => StreamEvent::ReasoningDelta(String::new()),
                    ContentBlock::ToolUse { id, name } => {
                        StreamEvent::ToolCallStart { index: ev.index, id, name }
                    }
                }
            } else {
                StreamEvent::Error(format!("failed to parse content_block_start: {data}"))
            }
        }
        "content_block_delta" => {
            if let Ok(ev) = serde_json::from_str::<ContentBlockDelta>(data) {
                match ev.delta {
                    Delta::TextDelta { text } => StreamEvent::TextDelta(text),
                    Delta::ThinkingDelta { thinking } => StreamEvent::ReasoningDelta(thinking),
                    Delta::InputJsonDelta { partial_json } => {
                        StreamEvent::ToolCallDelta { index: ev.index, delta: partial_json }
                    }
                }
            } else {
                StreamEvent::Error(format!("failed to parse content_block_delta: {data}"))
            }
        }
        "message_delta" => {
            if let Ok(ev) = serde_json::from_str::<MessageDelta>(data) {
                StreamEvent::Usage {
                    input_tokens: 0,
                    output_tokens: ev.usage.output_tokens,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                }
            } else {
                StreamEvent::Done
            }
        }
        "message_start" => {
            if let Ok(ev) = serde_json::from_str::<MessageStart>(data) {
                StreamEvent::Usage {
                    input_tokens: ev.message.usage.input_tokens,
                    output_tokens: 0,
                    cache_read_tokens: ev.message.usage.cache_read_input_tokens.unwrap_or(0),
                    cache_creation_tokens: ev
                        .message
                        .usage
                        .cache_creation_input_tokens
                        .unwrap_or(0),
                }
            } else {
                StreamEvent::Error(format!("failed to parse message_start: {data}"))
            }
        }
        "message_stop" => StreamEvent::Done,
        "content_block_stop" | "ping" => {
            // Ignored events — not errors, just no-ops
            StreamEvent::TextDelta(String::new())
        }
        "error" => {
            if let Ok(ev) = serde_json::from_str::<ErrorEvent>(data) {
                StreamEvent::Error(format!("{}: {}", ev.error.r#type, ev.error.message))
            } else {
                StreamEvent::Error(format!("unknown error: {data}"))
            }
        }
        _ => StreamEvent::TextDelta(String::new()),
    }
}

// ---------------------------------------------------------------------------
// Anthropic API types (private, for serialization only)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    system: Vec<SystemBlock>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    stream: bool,
}

#[derive(Serialize)]
struct SystemBlock {
    r#type: &'static str,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Serialize)]
struct CacheControl {
    r#type: &'static str,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContent>,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum AnthropicContent {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: serde_json::Value },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

#[derive(Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
    input_schema: serde_json::Value,
}

// -- SSE event parsing types --

#[derive(Deserialize)]
struct ContentBlockStart {
    index: usize,
    content_block: ContentBlock,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text {
        #[allow(dead_code)]
        text: String,
    },
    #[serde(rename = "thinking")]
    Thinking {
        #[allow(dead_code)]
        thinking: String,
    },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Deserialize)]
struct ContentBlockDelta {
    index: usize,
    delta: Delta,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::enum_variant_names)]
enum Delta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Deserialize)]
struct MessageDelta {
    usage: MessageDeltaUsage,
}

#[derive(Deserialize)]
struct MessageDeltaUsage {
    output_tokens: u64,
}

#[derive(Deserialize)]
struct MessageStart {
    message: MessageStartMessage,
}

#[derive(Deserialize)]
struct MessageStartMessage {
    usage: MessageStartUsage,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)]
struct MessageStartUsage {
    input_tokens: u64,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct ErrorEvent {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    r#type: String,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_text_delta_event() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let event = parse_sse_event("content_block_delta", data);
        assert!(matches!(event, StreamEvent::TextDelta(text) if text == "Hello"));
    }

    #[test]
    fn parse_tool_use_start_event() {
        let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_123","name":"read"}}"#;
        let event = parse_sse_event("content_block_start", data);
        assert!(
            matches!(event, StreamEvent::ToolCallStart { index: 1, id, name } if id == "toolu_123" && name == "read")
        );
    }

    #[test]
    fn parse_input_json_delta_event() {
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\""}}"#;
        let event = parse_sse_event("content_block_delta", data);
        assert!(
            matches!(event, StreamEvent::ToolCallDelta { index: 1, delta } if delta == "{\"path\"")
        );
    }

    #[test]
    fn parse_message_stop_event() {
        let event = parse_sse_event("message_stop", "{}");
        assert!(matches!(event, StreamEvent::Done));
    }

    #[test]
    fn parse_error_event() {
        let data =
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"too many requests"}}"#;
        let event = parse_sse_event("error", data);
        assert!(matches!(event, StreamEvent::Error(msg) if msg.contains("rate_limit_error")));
    }

    #[test]
    fn parse_message_start_with_usage() {
        let data = r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4-20250514","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1234,"output_tokens":0,"cache_read_input_tokens":500,"cache_creation_input_tokens":100}}}"#;
        let event = parse_sse_event("message_start", data);
        assert!(matches!(
            event,
            StreamEvent::Usage {
                input_tokens: 1234,
                cache_read_tokens: 500,
                cache_creation_tokens: 100,
                ..
            }
        ));
    }

    #[test]
    fn build_request_body_serializes() {
        let _provider = AnthropicProvider::new(SecretString::from("test-key".to_string()), None);
        let request = CompletionRequest {
            model: "anthropic/claude-sonnet-4-6".into(),
            system: "You are a helpful assistant.".into(),
            messages: vec![super::super::types::Message {
                role: "user".into(),
                content: vec![MessageContent::Text { text: "hello".into() }],
            }],
            tools: vec![],
            max_tokens: 4096,
        };

        let body = AnthropicProvider::build_request_body(&request);
        assert_eq!(body.model, "claude-sonnet-4-6");
        assert_eq!(body.max_tokens, 4096);
        assert!(body.stream);
        assert!(body.tools.is_none());

        // System prompt is a structured block with cache_control
        assert_eq!(body.system.len(), 1);
        assert_eq!(body.system[0].r#type, "text");
        assert!(body.system[0].cache_control.is_some());

        // Verify it serializes to valid JSON
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("claude-sonnet-4"));
        assert!(json.contains("cache_control"));
        assert!(json.contains("ephemeral"));
    }

    #[test]
    fn opus_4_7_request_omits_fields_rejected_by_api() {
        let request = CompletionRequest {
            model: "anthropic/claude-opus-4-7".into(),
            system: "You are helpful.".into(),
            messages: vec![super::super::types::Message {
                role: "user".into(),
                content: vec![MessageContent::Text { text: "hi".into() }],
            }],
            tools: vec![],
            max_tokens: 128,
        };

        let body = AnthropicProvider::build_request_body(&request);
        let json = serde_json::to_string(&body).unwrap();

        assert!(json.contains("claude-opus-4-7"));
        assert!(
            !json.contains("\"temperature\""),
            "Opus 4.7 rejects temperature; it must not be serialized. Body was: {json}"
        );
        assert!(
            !json.contains("\"top_p\""),
            "Opus 4.7 rejects top_p; it must not be serialized. Body was: {json}"
        );
        assert!(
            !json.contains("\"top_k\""),
            "Opus 4.7 rejects top_k; it must not be serialized. Body was: {json}"
        );
        assert!(
            !json.contains("\"thinking\""),
            "Opus 4.7 rejects legacy extended thinking; thinking field must not be serialized. Body was: {json}"
        );
    }
}
