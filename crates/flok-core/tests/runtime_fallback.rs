use std::sync::{Arc, Mutex};

use flok_core::bus::{Bus, BusEvent};
use flok_core::config::{AgentConfig, RuntimeFallbackConfig, WorktreeConfig};
use flok_core::provider::{CompletionRequest, Provider, ProviderRegistry, StreamEvent};
use flok_core::team::TeamRegistry;
use flok_core::tool::{TaskTool, Tool, ToolContext, ToolRegistry};
use flok_core::worktree::WorktreeManager;
use tokio::sync::mpsc;

#[derive(Debug)]
struct StatusProvider {
    name: &'static str,
    status: u16,
    calls: Mutex<u32>,
}

impl StatusProvider {
    fn call_count(&self) -> u32 {
        *self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait::async_trait]
impl Provider for StatusProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        tx: mpsc::UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<()> {
        let mut calls = self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *calls += 1;
        let _ = tx.send(StreamEvent::Error(format!("HTTP {}: synthetic failure", self.status)));
        Ok(())
    }
}

#[derive(Debug)]
struct TextProvider {
    name: &'static str,
    text: &'static str,
    calls: Mutex<u32>,
}

impl TextProvider {
    fn call_count(&self) -> u32 {
        *self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait::async_trait]
impl Provider for TextProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        tx: mpsc::UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<()> {
        let mut calls = self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *calls += 1;
        let _ = tx.send(StreamEvent::TextDelta(self.text.to_string()));
        let _ = tx.send(StreamEvent::Done);
        Ok(())
    }
}

fn request(model: &str) -> CompletionRequest {
    CompletionRequest {
        model: model.to_string(),
        system: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
        max_tokens: 512,
    }
}

fn registry_with_chain(
    primary: Arc<dyn Provider>,
    secondary: Arc<dyn Provider>,
) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.set_runtime_fallback(RuntimeFallbackConfig {
        cooldown_seconds: 300,
        ..RuntimeFallbackConfig::default()
    });
    registry.set_fallback_chains(
        [("anthropic".to_string(), vec!["openai".to_string()])].into_iter().collect(),
    );
    registry.insert("anthropic", primary, Some("anthropic/claude-sonnet-4-6".into()), 3);
    registry.insert("openai", secondary, Some("openai/gpt-5.4".into()), 3);
    registry
}

#[tokio::test]
async fn fallback_on_429_succeeds_on_next_provider() {
    let primary = Arc::new(StatusProvider { name: "anthropic", status: 429, calls: Mutex::new(0) });
    let secondary =
        Arc::new(TextProvider { name: "openai", text: "secondary success", calls: Mutex::new(0) });
    let registry = registry_with_chain(primary.clone(), secondary.clone());

    let (text, tool_calls) = registry
        .stream_with_fallback(
            "anthropic",
            "anthropic/claude-sonnet-4-6",
            request("anthropic/claude-sonnet-4-6"),
            &Bus::new(16),
            "session-1",
        )
        .await
        .expect("fallback should succeed");

    assert_eq!(text, "secondary success");
    assert!(tool_calls.is_empty());
    assert_eq!(primary.call_count(), 1);
    assert_eq!(secondary.call_count(), 1);
}

#[tokio::test]
async fn fallback_chain_exhausted_returns_error() {
    let primary = Arc::new(StatusProvider { name: "anthropic", status: 429, calls: Mutex::new(0) });
    let secondary = Arc::new(StatusProvider { name: "openai", status: 503, calls: Mutex::new(0) });
    let registry = registry_with_chain(primary.clone(), secondary.clone());

    let error = registry
        .stream_with_fallback(
            "anthropic",
            "anthropic/claude-sonnet-4-6",
            request("anthropic/claude-sonnet-4-6"),
            &Bus::new(16),
            "session-1",
        )
        .await
        .expect_err("all attempts should fail");

    let message = error.to_string();
    assert!(message.contains("All fallback attempts failed"));
    assert!(message.contains("anthropic"));
    assert!(message.contains("openai"));
    assert!(message.contains("HTTP 429"));
    assert!(message.contains("HTTP 503"));
}

#[tokio::test]
async fn non_retriable_error_does_not_fallback() {
    let primary = Arc::new(StatusProvider { name: "anthropic", status: 400, calls: Mutex::new(0) });
    let secondary =
        Arc::new(TextProvider { name: "openai", text: "should not run", calls: Mutex::new(0) });
    let registry = registry_with_chain(primary.clone(), secondary.clone());

    let error = registry
        .stream_with_fallback(
            "anthropic",
            "anthropic/claude-sonnet-4-6",
            request("anthropic/claude-sonnet-4-6"),
            &Bus::new(16),
            "session-1",
        )
        .await
        .expect_err("400 should surface immediately");

    assert!(error.to_string().contains("HTTP 400"));
    assert_eq!(primary.call_count(), 1);
    assert_eq!(secondary.call_count(), 0);
}

#[tokio::test]
async fn cooldown_skips_primary_within_window() {
    let primary = Arc::new(StatusProvider { name: "anthropic", status: 429, calls: Mutex::new(0) });
    let secondary =
        Arc::new(TextProvider { name: "openai", text: "secondary success", calls: Mutex::new(0) });
    let registry = registry_with_chain(primary.clone(), secondary.clone());

    registry
        .stream_with_fallback(
            "anthropic",
            "anthropic/claude-sonnet-4-6",
            request("anthropic/claude-sonnet-4-6"),
            &Bus::new(16),
            "session-1",
        )
        .await
        .expect("first call should fallback");

    registry
        .stream_with_fallback(
            "anthropic",
            "anthropic/claude-sonnet-4-6",
            request("anthropic/claude-sonnet-4-6"),
            &Bus::new(16),
            "session-2",
        )
        .await
        .expect("second call should skip cooled-down primary");

    assert_eq!(primary.call_count(), 1);
    assert_eq!(secondary.call_count(), 2);
}

#[tokio::test]
async fn provider_fallback_bus_event_emitted() {
    let primary = Arc::new(StatusProvider { name: "anthropic", status: 529, calls: Mutex::new(0) });
    let secondary =
        Arc::new(TextProvider { name: "openai", text: "secondary success", calls: Mutex::new(0) });
    let registry = registry_with_chain(primary, secondary);
    let bus = Bus::new(16);
    let mut rx = bus.subscribe();

    registry
        .stream_with_fallback(
            "anthropic",
            "anthropic/claude-sonnet-4-6",
            request("anthropic/claude-sonnet-4-6"),
            &bus,
            "session-42",
        )
        .await
        .expect("fallback should succeed");

    let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("event available")
        .expect("bus event received");

    assert!(matches!(
        event,
        BusEvent::ProviderFallback {
            session_id,
            from_provider,
            to_provider,
            reason,
        } if session_id == "session-42"
            && from_provider == "anthropic"
            && to_provider == "openai"
            && reason.contains("HTTP 529")
    ));
}

#[tokio::test]
async fn fallback_in_task_tool_sub_agent() {
    let primary = Arc::new(StatusProvider { name: "anthropic", status: 429, calls: Mutex::new(0) });
    let secondary =
        Arc::new(TextProvider { name: "openai", text: "task fallback ok", calls: Mutex::new(0) });
    let registry = Arc::new(registry_with_chain(primary.clone(), secondary.clone()));
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let project_root = std::fs::canonicalize(temp_dir.path()).expect("canonical project root");
    let tool = TaskTool::new(
        Arc::clone(&registry),
        "anthropic".to_string(),
        "anthropic/claude-sonnet-4-6".to_string(),
        std::collections::HashMap::<String, AgentConfig>::new(),
        Arc::new(ToolRegistry::new()),
        Bus::new(16),
        project_root.clone(),
        Arc::new(WorktreeManager::new("test-project", project_root.clone())),
        WorktreeConfig { enabled: false, ..WorktreeConfig::default() },
        TeamRegistry::new(),
    );

    let output = tool
        .execute(
            serde_json::json!({
                "description": "fallback task",
                "prompt": "respond without tools",
                "subagent_type": "general"
            }),
            &ToolContext {
                project_root,
                session_id: "session-task".to_string(),
                agent: "lead".to_string(),
                cancel: tokio_util::sync::CancellationToken::new(),
                lsp: None,
            },
        )
        .await
        .expect("task tool succeeds");

    assert!(!output.is_error);
    assert_eq!(output.content, "task fallback ok");
    assert_eq!(primary.call_count(), 1);
    assert_eq!(secondary.call_count(), 1);
}
