//! Request-time model routing.
//!
//! Uses lightweight heuristics to detect obviously complex coding turns and
//! temporarily upgrade from the session model to the strongest configured
//! default model. This is intentionally conservative: simple turns stay on the
//! session model, and routing only upgrades when there is a clear gain.

use std::collections::BTreeSet;

use crate::config::IntelligentRoutingConfig;
use crate::provider::{Message, MessageContent, ModelRegistry, ProviderRegistry};

/// The chosen model for a single completion request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRoutingDecision {
    pub model_id: String,
    pub reason: Option<String>,
}

/// Runtime state that should influence request routing beyond prompt contents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoutingContext {
    pub round: usize,
    pub verification_retries: usize,
    pub consecutive_tool_error_rounds: usize,
    pub max_repeated_tool_calls: usize,
}

/// Choose the model for the next completion request.
#[must_use]
pub fn route_model(
    session_model: &str,
    messages: &[Message],
    context: RoutingContext,
    provider_registry: &ProviderRegistry,
    config: &IntelligentRoutingConfig,
) -> ModelRoutingDecision {
    if !config.enabled {
        return ModelRoutingDecision { model_id: session_model.to_string(), reason: None };
    }

    let analysis = analyze_complexity(messages, context);
    let strongest = strongest_configured_model(session_model, provider_registry);
    let session_rank = model_rank(session_model);

    let Some(candidate_model) = strongest else {
        return ModelRoutingDecision { model_id: session_model.to_string(), reason: None };
    };

    let candidate_rank = model_rank(&candidate_model);
    if analysis.score < config.complexity_threshold || candidate_rank <= session_rank {
        return ModelRoutingDecision { model_id: session_model.to_string(), reason: None };
    }

    let reasons = analysis.signals.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    ModelRoutingDecision {
        model_id: candidate_model,
        reason: Some(format!("complexity score {} ({reasons})", analysis.score)),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ComplexityAnalysis {
    score: u32,
    signals: Vec<&'static str>,
}

fn analyze_complexity(messages: &[Message], context: RoutingContext) -> ComplexityAnalysis {
    let mut analysis = ComplexityAnalysis::default();
    let recent_messages = messages.iter().rev().take(8).collect::<Vec<_>>();
    let mut total_text_chars = 0usize;
    let mut tool_events = 0usize;
    let mut normalized_text = String::new();

    for message in recent_messages {
        for content in &message.content {
            match content {
                MessageContent::Text { text } => {
                    total_text_chars += text.len();
                    normalized_text.push_str(text);
                    normalized_text.push('\n');
                }
                MessageContent::Compaction { summary } => {
                    let rendered = summary.render_for_prompt();
                    total_text_chars += rendered.len();
                    normalized_text.push_str(&rendered);
                    normalized_text.push('\n');
                }
                MessageContent::ProjectMemory { summary } => {
                    let rendered = summary.render_for_prompt();
                    total_text_chars += rendered.len();
                    normalized_text.push_str(&rendered);
                    normalized_text.push('\n');
                }
                MessageContent::MemoryRecall { summary } => {
                    let rendered = summary.render_for_prompt();
                    total_text_chars += rendered.len();
                    normalized_text.push_str(&rendered);
                    normalized_text.push('\n');
                }
                MessageContent::Step { step } => {
                    let rendered = step.render_for_prompt();
                    total_text_chars += rendered.len();
                    normalized_text.push_str(&rendered);
                    normalized_text.push('\n');
                }
                MessageContent::Thinking { thinking } => {
                    total_text_chars += thinking.len();
                }
                MessageContent::ToolUse { .. } | MessageContent::ToolResult { .. } => {
                    tool_events += 1;
                }
            }
        }
    }

    if messages.len() >= 12 {
        analysis.score += 1;
        analysis.signals.push("long conversation context");
    }

    if total_text_chars >= 6_000 {
        analysis.score += 2;
        analysis.signals.push("large recent prompt context");
    } else if total_text_chars >= 2_500 {
        analysis.score += 1;
        analysis.signals.push("moderate recent prompt context");
    }

    if tool_events >= 4 {
        analysis.score += 2;
        analysis.signals.push("tool-heavy interaction");
    } else if tool_events >= 2 {
        analysis.score += 1;
        analysis.signals.push("multi-step tool interaction");
    }

    let lower = normalized_text.to_ascii_lowercase();
    if [
        "architecture",
        "migration",
        "multi-agent",
        "distributed",
        "refactor",
        "router",
        "plan",
        "spec",
        "review",
        "killer agent",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
    {
        analysis.score += 2;
        analysis.signals.push("architecture or planning request");
    }

    if lower.contains("automatic verification failed")
        || lower.contains("verification failed")
        || lower.contains("failing verification")
    {
        analysis.score += 1;
        analysis.signals.push("active verification repair loop");
    }

    if context.verification_retries >= 1 {
        analysis.score += 4;
        analysis.signals.push("verification retry escalation");
    }

    if context.consecutive_tool_error_rounds >= 2 {
        analysis.score += 4;
        analysis.signals.push("repeated tool failure rounds");
    } else if context.consecutive_tool_error_rounds == 1 {
        analysis.score += 2;
        analysis.signals.push("recent tool failure round");
    }

    if context.max_repeated_tool_calls >= 3 {
        analysis.score += 4;
        analysis.signals.push("repeated identical tool calls");
    } else if context.max_repeated_tool_calls >= 2 {
        analysis.score += 2;
        analysis.signals.push("repeated tool calls");
    }

    if context.round >= 6 {
        analysis.score += 4;
        analysis.signals.push("extended prompt loop");
    } else if context.round >= 3 {
        analysis.score += 2;
        analysis.signals.push("multi-round prompt loop");
    }

    if analysis.signals.is_empty() {
        analysis.signals.push("simple turn");
    }

    analysis
}

fn strongest_configured_model(
    session_model: &str,
    provider_registry: &ProviderRegistry,
) -> Option<String> {
    let mut candidates = BTreeSet::new();
    candidates.insert(session_model.to_string());
    for model in provider_registry.configured_default_models() {
        candidates.insert(model);
    }

    candidates.into_iter().max_by_key(|model| {
        let registry = ModelRegistry::builtin();
        let context = registry.get(model).map_or(0, |info| info.context_window);
        let output = registry.get(model).map_or(0, |info| u64::from(info.max_output_tokens));
        (model_rank(model), context, output)
    })
}

fn model_rank(model_id: &str) -> u32 {
    match model_id {
        "openai/gpt-5.5" => 102,
        "openai/gpt-5.4" | "anthropic/claude-opus-4-7" => 100,
        "anthropic/claude-opus-4-6" => 95,
        "google/gemini-2.5-pro" => 92,
        "anthropic/claude-sonnet-4-6" => 85,
        "openai/gpt-4.1" => 82,
        "minimax/MiniMax-M2.7" => 78,
        "google/gemini-2.5-flash" => 65,
        "openai/gpt-5.5-mini" => 62,
        "openai/gpt-5.4-mini" | "openai/gpt-4.1-mini" => 60,
        "anthropic/claude-haiku-4-5-20251001" => 40,
        "openai/gpt-5.5-nano" => 32,
        "openai/gpt-5.4-nano" => 30,
        _ => 50,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::config::IntelligentRoutingConfig;
    use crate::provider::mock::MockProvider;

    fn registry_with_defaults(defaults: &[(&str, &str)]) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        for (provider, model) in defaults {
            let provider_impl: Arc<dyn crate::provider::Provider> = Arc::new(MockProvider::new());
            registry.insert((*provider).to_string(), provider_impl, Some((*model).to_string()), 3);
        }
        registry
    }

    fn text_message(text: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: vec![MessageContent::Text { text: text.to_string() }],
        }
    }

    #[test]
    fn route_model_keeps_session_model_for_simple_turn() {
        let registry = registry_with_defaults(&[("openai", "openai/gpt-5.4-mini")]);

        let decision = route_model(
            "openai/gpt-5.4-mini",
            &[text_message("rename this function")],
            RoutingContext::default(),
            &registry,
            &IntelligentRoutingConfig::default(),
        );

        assert_eq!(decision.model_id, "openai/gpt-5.4-mini");
        assert!(decision.reason.is_none());
    }

    #[test]
    fn route_model_upgrades_for_complex_turn() {
        let registry = registry_with_defaults(&[
            ("openai", "openai/gpt-5.4"),
            ("anthropic", "anthropic/claude-sonnet-4-6"),
        ]);

        let messages = vec![
            text_message(
                "Review this architecture plan and migration spec for a multi-agent router refactor. \
                 We need a detailed design review across the whole coding agent runtime.",
            ),
            Message {
                role: "assistant".to_string(),
                content: vec![
                    MessageContent::ToolUse {
                        id: "tool-1".to_string(),
                        name: "read".to_string(),
                        input: serde_json::json!({"file_path":"src/main.rs"}),
                    },
                    MessageContent::ToolResult {
                        tool_use_id: "tool-1".to_string(),
                        content: "read".to_string(),
                        is_error: false,
                    },
                    MessageContent::ToolUse {
                        id: "tool-2".to_string(),
                        name: "grep".to_string(),
                        input: serde_json::json!({"pattern":"router"}),
                    },
                    MessageContent::ToolResult {
                        tool_use_id: "tool-2".to_string(),
                        content: "grep".to_string(),
                        is_error: false,
                    },
                ],
            },
        ];

        let decision = route_model(
            "openai/gpt-5.4-mini",
            &messages,
            RoutingContext::default(),
            &registry,
            &IntelligentRoutingConfig::default(),
        );

        assert_eq!(decision.model_id, "openai/gpt-5.4");
        assert!(decision
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("complexity score")));
    }

    #[test]
    fn route_model_keeps_session_model_when_already_strongest() {
        let registry = registry_with_defaults(&[
            ("openai", "openai/gpt-5.4"),
            ("anthropic", "anthropic/claude-sonnet-4-6"),
        ]);

        let decision = route_model(
            "openai/gpt-5.4",
            &[text_message("review the architecture and migration plan")],
            RoutingContext::default(),
            &registry,
            &IntelligentRoutingConfig::default(),
        );

        assert_eq!(decision.model_id, "openai/gpt-5.4");
        assert!(decision.reason.is_none());
    }

    #[test]
    fn route_model_honors_disabled_config() {
        let registry = registry_with_defaults(&[("openai", "openai/gpt-5.4")]);
        let prompt =
            "Review this architecture plan and migration spec for a multi-agent router refactor. "
                .repeat(90);

        let decision = route_model(
            "openai/gpt-5.4-mini",
            &[text_message(&prompt)],
            RoutingContext::default(),
            &registry,
            &IntelligentRoutingConfig { enabled: false, complexity_threshold: 1 },
        );

        assert_eq!(decision.model_id, "openai/gpt-5.4-mini");
        assert!(decision.reason.is_none());
    }

    #[test]
    fn route_model_upgrades_for_verification_retry_context() {
        let registry = registry_with_defaults(&[("openai", "openai/gpt-5.4")]);

        let decision = route_model(
            "openai/gpt-5.4-mini",
            &[text_message("fix this compile error")],
            RoutingContext { round: 2, verification_retries: 1, ..RoutingContext::default() },
            &registry,
            &IntelligentRoutingConfig::default(),
        );

        assert_eq!(decision.model_id, "openai/gpt-5.4");
        assert!(decision
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("verification retry escalation")));
    }

    #[test]
    fn route_model_upgrades_for_extended_prompt_loop() {
        let registry = registry_with_defaults(&[("openai", "openai/gpt-5.4")]);

        let decision = route_model(
            "openai/gpt-5.4-mini",
            &[text_message("inspect the repo and try another approach")],
            RoutingContext { round: 6, verification_retries: 0, ..RoutingContext::default() },
            &registry,
            &IntelligentRoutingConfig::default(),
        );

        assert_eq!(decision.model_id, "openai/gpt-5.4");
        assert!(decision
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("extended prompt loop")));
    }

    #[test]
    fn route_model_upgrades_for_repeated_tool_failure_rounds() {
        let registry = registry_with_defaults(&[("openai", "openai/gpt-5.4")]);

        let decision = route_model(
            "openai/gpt-5.4-mini",
            &[text_message("try the tool again")],
            RoutingContext {
                round: 2,
                verification_retries: 0,
                consecutive_tool_error_rounds: 2,
                max_repeated_tool_calls: 0,
            },
            &registry,
            &IntelligentRoutingConfig::default(),
        );

        assert_eq!(decision.model_id, "openai/gpt-5.4");
        assert!(decision
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("repeated tool failure rounds")));
    }

    #[test]
    fn route_model_upgrades_for_repeated_identical_tool_calls() {
        let registry = registry_with_defaults(&[("openai", "openai/gpt-5.4")]);

        let decision = route_model(
            "openai/gpt-5.4-mini",
            &[text_message("inspect the same file again")],
            RoutingContext {
                round: 2,
                verification_retries: 0,
                consecutive_tool_error_rounds: 0,
                max_repeated_tool_calls: 3,
            },
            &registry,
            &IntelligentRoutingConfig::default(),
        );

        assert_eq!(decision.model_id, "openai/gpt-5.4");
        assert!(decision
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("repeated identical tool calls")));
    }
}
