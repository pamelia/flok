//! `ProviderRegistry`: runtime container for all configured providers.
//!
//! Unlike the binary crate's single-provider startup path, this registry keeps
//! every configured provider with credentials available at runtime so tools can
//! dispatch sub-agents across providers independently.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::bus::{Bus, BusEvent};
use crate::config::RuntimeFallbackConfig;
use crate::provider::{
    is_retriable, CompletionRequest, CooldownTracker, FallbackChain, ModelRegistry, Provider,
    StreamEvent,
};

const STREAM_TIMEOUT_SECONDS: u64 = 60;

/// Default concurrent sub-agent API calls per provider.
pub const DEFAULT_PERMITS_PER_PROVIDER: usize = 3;

/// Collected tool call emitted by a provider stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UsageTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StreamedCompletion {
    pub text: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: UsageTotals,
}

#[derive(Debug, thiserror::Error)]
#[error("operation cancelled by user")]
pub(crate) struct StreamCancelled;

pub(crate) struct FallbackStreamContext<'a> {
    pub initial_provider: &'a str,
    pub initial_model: &'a str,
    pub bus: &'a Bus,
    pub session_id: &'a str,
    pub cancel_token: Option<&'a CancellationToken>,
}

/// Runtime container for all configured providers.
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn Provider>>,
    default_models: HashMap<String, String>,
    semaphores: HashMap<String, Arc<Semaphore>>,
    cooldown: Arc<CooldownTracker>,
    runtime_fallback: RuntimeFallbackConfig,
    fallback_chains: HashMap<String, Vec<String>>,
}

impl ProviderRegistry {
    /// Create an empty provider registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            default_models: HashMap::new(),
            semaphores: HashMap::new(),
            cooldown: Arc::new(CooldownTracker::default()),
            runtime_fallback: RuntimeFallbackConfig::default(),
            fallback_chains: HashMap::new(),
        }
    }

    /// Override runtime fallback behavior.
    pub fn set_runtime_fallback(&mut self, config: RuntimeFallbackConfig) {
        self.runtime_fallback = config;
    }

    /// Configure ordered fallback chains keyed by provider name.
    pub fn set_fallback_chains(&mut self, fallback_chains: HashMap<String, Vec<String>>) {
        self.fallback_chains = fallback_chains;
    }

    /// Insert a configured provider, its default model, and its semaphore.
    pub fn insert(
        &mut self,
        name: impl Into<String>,
        provider: Arc<dyn Provider>,
        default_model: Option<String>,
        permits: usize,
    ) {
        let name = name.into();
        self.providers.insert(name.clone(), provider);
        if let Some(default_model) = default_model {
            self.default_models.insert(name.clone(), default_model);
        }
        self.semaphores.insert(name, Arc::new(Semaphore::new(permits)));
    }

    /// Get a provider by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(name).map(Arc::clone)
    }

    /// Get the configured default model for a provider.
    #[must_use]
    pub fn default_model(&self, name: &str) -> Option<&str> {
        self.default_models.get(name).map(String::as_str)
    }

    /// Get a human-friendly display string for a provider default model.
    #[must_use]
    pub fn display_default_model(&self, name: &str) -> Option<String> {
        self.default_model(name).map(display_model)
    }

    /// Get the concurrency semaphore for a provider.
    #[must_use]
    pub fn semaphore(&self, name: &str) -> Option<Arc<Semaphore>> {
        self.semaphores.get(name).map(Arc::clone)
    }

    /// Return configured provider names in sorted order.
    #[must_use]
    pub fn configured_providers(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.providers.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Return a human-readable summary of configured providers.
    #[must_use]
    pub fn describe(&self) -> String {
        self.configured_providers()
            .into_iter()
            .map(|name| match self.default_model(name) {
                Some(default_model) => {
                    format!("{name} (default model: {})", display_model(default_model))
                }
                None => format!("{name} (default model: not set)"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Stream a completion request through the configured fallback chain.
    pub async fn stream_with_fallback(
        &self,
        initial_provider: &str,
        initial_model: &str,
        request: CompletionRequest,
        bus: &Bus,
        session_id: &str,
    ) -> anyhow::Result<(String, Vec<ToolCall>)> {
        let response = self
            .stream_with_fallback_internal(
                FallbackStreamContext {
                    initial_provider,
                    initial_model,
                    bus,
                    session_id,
                    cancel_token: None,
                },
                request,
                |_event, _completion| {},
            )
            .await?;

        Ok((response.text, response.tool_calls))
    }

    pub(crate) async fn stream_with_fallback_internal<F>(
        &self,
        context: FallbackStreamContext<'_>,
        request: CompletionRequest,
        mut handle_event: F,
    ) -> anyhow::Result<StreamedCompletion>
    where
        F: FnMut(&StreamEvent, &mut StreamedCompletion),
    {
        let attempts = self.resolve_attempts(context.initial_provider, context.initial_model);
        if attempts.is_empty() {
            return Err(anyhow::anyhow!(
                "Provider '{}' is not configured. Available providers: {}",
                context.initial_provider,
                self.describe()
            ));
        }

        let max_attempts =
            usize::try_from(self.runtime_fallback.max_attempts.max(1)).unwrap_or(usize::MAX);
        let attempts: Vec<_> = attempts.into_iter().take(max_attempts).collect();
        let cooldown_duration = Duration::from_secs(self.runtime_fallback.cooldown_seconds);
        let mut failures = Vec::new();

        for (index, attempt) in attempts.iter().enumerate() {
            let provider = self.get(&attempt.provider).ok_or_else(|| {
                anyhow::anyhow!("Provider '{}' is not configured", attempt.provider)
            })?;
            let semaphore = self.semaphore(&attempt.provider).ok_or_else(|| {
                anyhow::anyhow!(
                    "Provider '{}' is missing a concurrency semaphore",
                    attempt.provider
                )
            })?;

            let _permit = semaphore
                .acquire()
                .await
                .map_err(|e| anyhow::anyhow!("provider semaphore closed: {e}"))?;

            let attempt_request =
                CompletionRequest { model: attempt.model.clone(), ..request.clone() };
            let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
            let spawn_request = attempt_request.clone();
            let provider_task =
                tokio::spawn(async move { provider.stream(spawn_request, tx).await });
            let mut completion = StreamedCompletion::default();
            let mut attempt_error = None;

            loop {
                let timeout = tokio::time::sleep(Duration::from_secs(STREAM_TIMEOUT_SECONDS));
                tokio::pin!(timeout);

                let event = if let Some(cancel_token) = context.cancel_token {
                    tokio::select! {
                        () = cancel_token.cancelled() => {
                            provider_task.abort();
                            return Err(StreamCancelled.into());
                        }
                        () = &mut timeout => {
                            provider_task.abort();
                            attempt_error = Some(anyhow::anyhow!(
                                "Stream timeout: no response from provider for {STREAM_TIMEOUT_SECONDS} seconds. The model may be overloaded. Try again."
                            ));
                            break;
                        }
                        recv = rx.recv() => recv,
                    }
                } else {
                    tokio::select! {
                        () = &mut timeout => {
                            provider_task.abort();
                            attempt_error = Some(anyhow::anyhow!(
                                "Stream timeout: no response from provider for {STREAM_TIMEOUT_SECONDS} seconds. The model may be overloaded. Try again."
                            ));
                            break;
                        }
                        recv = rx.recv() => recv,
                    }
                };

                match event {
                    Some(StreamEvent::TextDelta(delta)) => {
                        completion.text.push_str(&delta);
                        let event = StreamEvent::TextDelta(delta);
                        handle_event(&event, &mut completion);
                    }
                    Some(StreamEvent::ReasoningDelta(delta)) => {
                        completion.reasoning.push_str(&delta);
                        let event = StreamEvent::ReasoningDelta(delta);
                        handle_event(&event, &mut completion);
                    }
                    Some(StreamEvent::ToolCallStart { index, id, name }) => {
                        while completion.tool_calls.len() <= index {
                            completion.tool_calls.push(ToolCall::default());
                        }
                        completion.tool_calls[index].id.clone_from(&id);
                        completion.tool_calls[index].name.clone_from(&name);
                        let event = StreamEvent::ToolCallStart { index, id, name };
                        handle_event(&event, &mut completion);
                    }
                    Some(StreamEvent::ToolCallDelta { index, delta }) => {
                        while completion.tool_calls.len() <= index {
                            completion.tool_calls.push(ToolCall::default());
                        }
                        completion.tool_calls[index].arguments.push_str(&delta);
                        let event = StreamEvent::ToolCallDelta { index, delta };
                        handle_event(&event, &mut completion);
                    }
                    Some(StreamEvent::Usage {
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                        cache_creation_tokens,
                    }) => {
                        completion.usage.input += input_tokens;
                        completion.usage.output += output_tokens;
                        completion.usage.cache_read += cache_read_tokens;
                        completion.usage.cache_creation += cache_creation_tokens;
                        let event = StreamEvent::Usage {
                            input_tokens,
                            output_tokens,
                            cache_read_tokens,
                            cache_creation_tokens,
                        };
                        handle_event(&event, &mut completion);
                    }
                    Some(StreamEvent::Done) => {
                        let event = StreamEvent::Done;
                        handle_event(&event, &mut completion);
                        break;
                    }
                    Some(StreamEvent::Error(message)) => {
                        attempt_error = Some(anyhow::anyhow!(message));
                        break;
                    }
                    None => break,
                }
            }

            if let Ok(Err(err)) = provider_task.await {
                if attempt_error.is_none() {
                    attempt_error = Some(err);
                }
            }

            completion
                .tool_calls
                .retain(|tool_call| !tool_call.id.is_empty() && !tool_call.name.is_empty());

            if let Some(err) = attempt_error {
                let retriable = self.runtime_fallback.enabled
                    && is_retriable(&err, &self.runtime_fallback.retry_on_errors);
                failures.push(format!("{} (model {}): {}", attempt.provider, attempt.model, err));

                if retriable {
                    self.cooldown.mark(&attempt.provider, cooldown_duration);

                    if let Some(next_attempt) = attempts.get(index + 1) {
                        if self.runtime_fallback.notify_on_fallback {
                            context.bus.send(BusEvent::ProviderFallback {
                                session_id: context.session_id.to_string(),
                                from_provider: attempt.provider.clone(),
                                to_provider: next_attempt.provider.clone(),
                                reason: err.to_string(),
                            });
                        }
                        continue;
                    }

                    continue;
                }

                return Err(err);
            }

            return Ok(completion);
        }

        Err(anyhow::anyhow!(
            "All fallback attempts failed for provider '{}': {}",
            context.initial_provider,
            failures.join(" | ")
        ))
    }

    fn resolve_attempts(&self, initial_provider: &str, initial_model: &str) -> Vec<AttemptTarget> {
        let fallback_chain =
            self.fallback_chains.get(initial_provider).map_or(&[][..], Vec::as_slice);
        let chain = FallbackChain {
            primary: initial_provider,
            fallbacks: fallback_chain,
            cooldown: &self.cooldown,
            registry: self,
        };

        chain
            .attempt_order()
            .into_iter()
            .filter_map(|provider| self.resolve_attempt(provider, initial_provider, initial_model))
            .collect()
    }

    fn resolve_attempt(
        &self,
        provider: &str,
        initial_provider: &str,
        initial_model: &str,
    ) -> Option<AttemptTarget> {
        let model = if provider == initial_provider {
            initial_model.to_string()
        } else if let Some(default_model) = self.default_model(provider) {
            default_model.to_string()
        } else if ModelRegistry::provider_name(initial_model) == provider {
            initial_model.to_string()
        } else {
            return None;
        };

        Some(AttemptTarget { provider: provider.to_string(), model })
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttemptTarget {
    provider: String,
    model: String,
}

fn display_model(model_id: &str) -> String {
    match model_id {
        "anthropic/claude-opus-4-7" => "opus-4.7".to_string(),
        "anthropic/claude-opus-4-6" => "opus".to_string(),
        "anthropic/claude-sonnet-4-6" => "sonnet".to_string(),
        "anthropic/claude-haiku-4-5-20251001" => "haiku".to_string(),
        "openai/gpt-5.4" => "gpt-5.4".to_string(),
        "openai/gpt-5.4-mini" => "mini".to_string(),
        "openai/gpt-5.4-nano" => "nano".to_string(),
        "deepseek/deepseek-chat" => "deepseek".to_string(),
        "deepseek/deepseek-reasoner" => "r1".to_string(),
        "minimax/MiniMax-M2.7" => "minimax".to_string(),
        _ => ModelRegistry::model_name(model_id).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::provider::mock::MockProvider;

    #[derive(Debug)]
    struct StaticResponseProvider {
        name: &'static str,
        response: &'static str,
    }

    #[async_trait::async_trait]
    impl Provider for StaticResponseProvider {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn stream(
            &self,
            _request: CompletionRequest,
            tx: mpsc::UnboundedSender<StreamEvent>,
        ) -> anyhow::Result<()> {
            let _ = tx.send(StreamEvent::TextDelta(self.response.to_string()));
            let _ = tx.send(StreamEvent::Done);
            Ok(())
        }
    }

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

    fn request(model: &str) -> CompletionRequest {
        CompletionRequest {
            model: model.to_string(),
            system: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            max_tokens: 256,
        }
    }

    #[test]
    fn insert_and_get_provider() {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider::new());
        let mut registry = ProviderRegistry::new();
        registry.insert(
            "anthropic",
            Arc::clone(&provider),
            Some("anthropic/claude-opus-4-7".into()),
            3,
        );

        assert!(registry.get("anthropic").is_some());
        assert!(registry.get("openai").is_none());
    }

    #[test]
    fn stores_default_model() {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider::new());
        let mut registry = ProviderRegistry::new();
        registry.insert("openai", provider, Some("openai/gpt-5.4".into()), 3);

        assert_eq!(registry.default_model("openai"), Some("openai/gpt-5.4"));
    }

    #[test]
    fn semaphores_are_isolated_per_provider() {
        let anthropic: Arc<dyn Provider> = Arc::new(MockProvider::new());
        let openai: Arc<dyn Provider> = Arc::new(MockProvider::new());
        let mut registry = ProviderRegistry::new();
        registry.insert("anthropic", anthropic, Some("anthropic/claude-sonnet-4-6".into()), 1);
        registry.insert("openai", openai, Some("openai/gpt-5.4".into()), 1);

        let anthropic_sem = registry.semaphore("anthropic").expect("anthropic semaphore");
        let openai_sem = registry.semaphore("openai").expect("openai semaphore");

        let _anthropic_permit = anthropic_sem.try_acquire().expect("anthropic permit");
        assert_eq!(anthropic_sem.available_permits(), 0);
        assert_eq!(openai_sem.available_permits(), 1);
    }

    #[test]
    fn describe_formats_sorted_provider_list() {
        let anthropic: Arc<dyn Provider> = Arc::new(MockProvider::new());
        let openai: Arc<dyn Provider> = Arc::new(MockProvider::new());
        let mut registry = ProviderRegistry::new();
        registry.insert("openai", openai, Some("openai/gpt-5.4".into()), 3);
        registry.insert("anthropic", anthropic, Some("anthropic/claude-opus-4-7".into()), 3);

        assert_eq!(
            registry.describe(),
            "anthropic (default model: opus-4.7), openai (default model: gpt-5.4)"
        );
    }

    #[tokio::test]
    async fn stream_with_fallback_uses_secondary_provider() {
        let anthropic =
            Arc::new(StatusProvider { name: "anthropic", status: 429, calls: Mutex::new(0) });
        let openai = Arc::new(StaticResponseProvider { name: "openai", response: "fallback ok" });
        let anthropic_dyn: Arc<dyn Provider> = anthropic.clone();
        let openai_dyn: Arc<dyn Provider> = openai.clone();

        let mut registry = ProviderRegistry::new();
        registry.set_fallback_chains(
            [("anthropic".to_string(), vec!["openai".to_string()])].into_iter().collect(),
        );
        registry.insert("anthropic", anthropic_dyn, Some("anthropic/claude-sonnet-4-6".into()), 3);
        registry.insert("openai", openai_dyn, Some("openai/gpt-5.4".into()), 3);

        let (text, tool_calls) = registry
            .stream_with_fallback(
                "anthropic",
                "anthropic/claude-sonnet-4-6",
                request("anthropic/claude-sonnet-4-6"),
                &Bus::new(16),
                "session-1",
            )
            .await
            .expect("fallback succeeds");

        assert_eq!(text, "fallback ok");
        assert!(tool_calls.is_empty());
        assert_eq!(anthropic.call_count(), 1);
    }

    #[tokio::test]
    async fn stream_with_fallback_stops_on_non_retriable_error() {
        let anthropic =
            Arc::new(StatusProvider { name: "anthropic", status: 400, calls: Mutex::new(0) });
        let openai = Arc::new(StatusProvider { name: "openai", status: 429, calls: Mutex::new(0) });
        let anthropic_dyn: Arc<dyn Provider> = anthropic.clone();
        let openai_dyn: Arc<dyn Provider> = openai.clone();

        let mut registry = ProviderRegistry::new();
        registry.set_fallback_chains(
            [("anthropic".to_string(), vec!["openai".to_string()])].into_iter().collect(),
        );
        registry.insert("anthropic", anthropic_dyn, Some("anthropic/claude-sonnet-4-6".into()), 3);
        registry.insert("openai", openai_dyn, Some("openai/gpt-5.4".into()), 3);

        let error = registry
            .stream_with_fallback(
                "anthropic",
                "anthropic/claude-sonnet-4-6",
                request("anthropic/claude-sonnet-4-6"),
                &Bus::new(16),
                "session-1",
            )
            .await
            .expect_err("400 should not fallback");

        assert!(error.to_string().contains("HTTP 400"));
        assert_eq!(anthropic.call_count(), 1);
        assert_eq!(openai.call_count(), 0);
    }
}
