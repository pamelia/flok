//! Request-time model routing.
//!
//! Uses lightweight heuristics to balance quality, capability, context size,
//! latency, and cost across configured models. Simple turns stay on the
//! session model unless a budget policy forces a downgrade.

use crate::config::IntelligentRoutingConfig;
use crate::provider::{Message, MessageContent, ModelInfo, ModelRegistry, ProviderRegistry};

/// The chosen model for a single completion request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRoutingDecision {
    pub model_id: String,
    pub reason: Option<String>,
}

/// Runtime model quality tier used for routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoutingTier {
    Economy,
    Standard,
    Strong,
    Frontier,
}

/// Budget state for the active session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoutingBudgetState {
    pub spent_microusd: u64,
    pub max_session_microusd: Option<u64>,
}

impl RoutingBudgetState {
    fn is_exhausted(self) -> bool {
        self.max_session_microusd.is_some_and(|max| self.spent_microusd >= max)
    }
}

/// A model eligible for the current request.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutingCandidate {
    pub model_id: String,
    pub tier: RoutingTier,
    pub rank: u32,
    pub context_window: u64,
    pub max_output_tokens: u32,
    pub input_cost_per_m: f64,
    pub output_cost_per_m: f64,
}

/// Routing policy assembled from config and request state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutingPolicy {
    pub complexity_threshold: u32,
    pub budget: RoutingBudgetState,
}

/// Signals that request replay/escalation should use a stronger model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoutingFailureEscalation {
    pub repeated_tool_failures: bool,
    pub verification_retry: bool,
    pub repeated_identical_tool_calls: bool,
}

/// Runtime state that should influence request routing beyond prompt contents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoutingContext {
    pub round: usize,
    pub verification_retries: usize,
    pub consecutive_tool_error_rounds: usize,
    pub max_repeated_tool_calls: usize,
    pub spent_microusd: u64,
    pub max_session_microusd: Option<u64>,
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
    let candidates = routing_candidates(session_model, provider_registry, &analysis);
    let Some(session_candidate) =
        candidates.iter().find(|candidate| candidate.model_id == session_model)
    else {
        return ModelRoutingDecision { model_id: session_model.to_string(), reason: None };
    };
    let policy = RoutingPolicy {
        complexity_threshold: config.complexity_threshold,
        budget: RoutingBudgetState {
            spent_microusd: context.spent_microusd,
            max_session_microusd: context.max_session_microusd.or(config.max_session_cost_microusd),
        },
    };

    if policy.budget.is_exhausted() {
        let cheapest = candidates.iter().min_by(|left, right| candidate_cost_order(left, right));
        if let Some(cheapest) = cheapest {
            if cheapest.model_id != session_model {
                return ModelRoutingDecision {
                    model_id: cheapest.model_id.clone(),
                    reason: Some(format!(
                        "budget limit reached (${:.4}/${:.4}); downgraded to {}",
                        policy.budget.spent_microusd as f64 / 1_000_000.0,
                        policy.budget.max_session_microusd.unwrap_or_default() as f64 / 1_000_000.0,
                        cheapest.model_id
                    )),
                };
            }
        }
        return ModelRoutingDecision { model_id: session_model.to_string(), reason: None };
    }

    if analysis.score < policy.complexity_threshold {
        return ModelRoutingDecision { model_id: session_model.to_string(), reason: None };
    }

    let Some(candidate) =
        candidates.iter().max_by_key(|candidate| candidate_score(candidate, &analysis))
    else {
        return ModelRoutingDecision { model_id: session_model.to_string(), reason: None };
    };

    if candidate.rank <= session_candidate.rank {
        return ModelRoutingDecision { model_id: session_model.to_string(), reason: None };
    }

    let reasons = analysis.signals.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    ModelRoutingDecision {
        model_id: candidate.model_id.clone(),
        reason: Some(format!(
            "complexity score {} ({reasons}); selected {:?} candidate with {} token context",
            analysis.score, candidate.tier, candidate.context_window
        )),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ComplexityAnalysis {
    score: u32,
    signals: Vec<&'static str>,
    estimated_input_tokens: u64,
    requires_tools: bool,
    requires_streaming: bool,
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

    analysis.estimated_input_tokens =
        u64::try_from(total_text_chars.div_ceil(4)).unwrap_or(u64::MAX);
    analysis.requires_tools = true;
    analysis.requires_streaming = true;

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

fn routing_candidates(
    session_model: &str,
    provider_registry: &ProviderRegistry,
    analysis: &ComplexityAnalysis,
) -> Vec<RoutingCandidate> {
    let registry = ModelRegistry::builtin();
    let mut model_ids = provider_registry.configured_default_models();
    model_ids.push(session_model.to_string());
    model_ids.sort();
    model_ids.dedup();

    model_ids
        .into_iter()
        .filter_map(|model_id| routing_candidate(&registry, &model_id, analysis))
        .collect()
}

fn routing_candidate(
    registry: &ModelRegistry,
    model_id: &str,
    analysis: &ComplexityAnalysis,
) -> Option<RoutingCandidate> {
    let info = registry.get(model_id)?;
    if !candidate_is_eligible(info, analysis) {
        return None;
    }

    Some(candidate_from_model_info(info))
}

fn candidate_is_eligible(info: &ModelInfo, analysis: &ComplexityAnalysis) -> bool {
    if analysis.requires_tools && !info.supports_tools {
        return false;
    }
    if analysis.requires_streaming && !info.supports_streaming {
        return false;
    }
    info.context_window >= analysis.estimated_input_tokens.saturating_add(8_192)
}

fn candidate_from_model_info(info: &ModelInfo) -> RoutingCandidate {
    RoutingCandidate {
        model_id: info.id.to_string(),
        tier: routing_tier(info.id),
        rank: model_rank(info.id),
        context_window: info.context_window,
        max_output_tokens: info.max_output_tokens,
        input_cost_per_m: info.input_cost_per_m,
        output_cost_per_m: info.output_cost_per_m,
    }
}

fn candidate_score(candidate: &RoutingCandidate, analysis: &ComplexityAnalysis) -> u64 {
    let context_fit_bonus =
        if candidate.context_window >= analysis.estimated_input_tokens.saturating_mul(4) {
            8
        } else if candidate.context_window >= analysis.estimated_input_tokens.saturating_mul(2) {
            4
        } else {
            0
        };
    u64::from(candidate.rank) * 1_000
        + u64::from(candidate.max_output_tokens / 1_000)
        + context_fit_bonus
        - estimated_cost_penalty(candidate)
}

fn candidate_cost_order(left: &RoutingCandidate, right: &RoutingCandidate) -> std::cmp::Ordering {
    estimated_average_cost_microusd_per_m(left)
        .cmp(&estimated_average_cost_microusd_per_m(right))
        .then_with(|| left.rank.cmp(&right.rank))
        .then_with(|| left.model_id.cmp(&right.model_id))
}

fn estimated_cost_penalty(candidate: &RoutingCandidate) -> u64 {
    estimated_average_cost_microusd_per_m(candidate) / 1_000_000
}

fn estimated_average_cost_microusd_per_m(candidate: &RoutingCandidate) -> u64 {
    ((candidate.input_cost_per_m + candidate.output_cost_per_m) * 500_000.0).round() as u64
}

fn routing_tier(model_id: &str) -> RoutingTier {
    match model_rank(model_id) {
        100.. => RoutingTier::Frontier,
        80..=99 => RoutingTier::Strong,
        50..=79 => RoutingTier::Standard,
        _ => RoutingTier::Economy,
    }
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
            &IntelligentRoutingConfig {
                enabled: false,
                complexity_threshold: 1,
                ..IntelligentRoutingConfig::default()
            },
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
                ..RoutingContext::default()
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
                ..RoutingContext::default()
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

    #[test]
    fn route_model_scores_multiple_eligible_candidates() {
        let registry = registry_with_defaults(&[
            ("openai", "openai/gpt-5.4-mini"),
            ("anthropic", "anthropic/claude-sonnet-4-6"),
            ("minimax", "minimax/MiniMax-M2.7"),
        ]);

        let decision = route_model(
            "openai/gpt-5.4-mini",
            &[text_message("review the architecture plan and migration spec")],
            RoutingContext::default(),
            &registry,
            &IntelligentRoutingConfig {
                complexity_threshold: 1,
                ..IntelligentRoutingConfig::default()
            },
        );

        assert_eq!(decision.model_id, "anthropic/claude-sonnet-4-6");
        assert!(decision
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("selected Strong candidate")));
    }

    #[test]
    fn route_model_downgrades_when_session_budget_is_exhausted() {
        let registry = registry_with_defaults(&[
            ("openai", "openai/gpt-5.4-nano"),
            ("anthropic", "anthropic/claude-sonnet-4-6"),
        ]);

        let decision = route_model(
            "anthropic/claude-sonnet-4-6",
            &[text_message("review the architecture plan and migration spec")],
            RoutingContext {
                spent_microusd: 10_000,
                max_session_microusd: Some(10_000),
                ..RoutingContext::default()
            },
            &registry,
            &IntelligentRoutingConfig::default(),
        );

        assert_eq!(decision.model_id, "openai/gpt-5.4-nano");
        assert!(decision
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("budget limit reached")));
    }

    #[test]
    fn routing_candidate_excludes_models_that_do_not_fit_context() {
        let registry = ModelRegistry::builtin();
        let analysis = ComplexityAnalysis {
            estimated_input_tokens: 250_000,
            requires_tools: true,
            requires_streaming: true,
            ..ComplexityAnalysis::default()
        };

        assert!(routing_candidate(&registry, "minimax/MiniMax-M2.7", &analysis).is_none());
        assert!(routing_candidate(&registry, "openai/gpt-5.4-mini", &analysis).is_some());
    }

    #[test]
    fn routing_candidate_excludes_models_without_required_capabilities() {
        let info = ModelInfo {
            id: "test/text-only",
            provider: "test",
            display_name: "Text Only",
            context_window: 1_000_000,
            max_output_tokens: 16_000,
            input_cost_per_m: 1.0,
            output_cost_per_m: 2.0,
            supports_tools: false,
            supports_streaming: true,
        };
        let analysis = ComplexityAnalysis {
            estimated_input_tokens: 1_000,
            requires_tools: true,
            requires_streaming: true,
            ..ComplexityAnalysis::default()
        };

        assert!(!candidate_is_eligible(&info, &analysis));
    }
}
