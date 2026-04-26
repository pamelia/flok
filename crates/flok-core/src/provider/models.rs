//! Built-in model registry.
//!
//! A hardcoded table of known models with context window sizes and pricing.
//! Users can override or add models via `flok.toml`.

use std::collections::HashMap;

use crate::config::FlokConfig;

/// Information about a known model.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// Full model ID (e.g., "anthropic/claude-sonnet-4").
    pub id: &'static str,
    /// Provider name (e.g., "anthropic").
    pub provider: &'static str,
    /// Human-readable display name.
    pub display_name: &'static str,
    /// Maximum context window in tokens.
    pub context_window: u64,
    /// Maximum output tokens.
    pub max_output_tokens: u32,
    /// Cost per million input tokens (USD).
    pub input_cost_per_m: f64,
    /// Cost per million output tokens (USD).
    pub output_cost_per_m: f64,
    /// Whether the model supports tool use.
    pub supports_tools: bool,
    /// Whether the model supports streaming.
    pub supports_streaming: bool,
}

/// Registry of known models.
#[derive(Debug)]
pub struct ModelRegistry {
    models: HashMap<&'static str, ModelInfo>,
}

impl ModelRegistry {
    /// Create a registry with all built-in models.
    pub fn builtin() -> Self {
        let models: Vec<ModelInfo> = vec![
            // Anthropic — current models
            // API IDs from https://docs.anthropic.com/en/docs/about-claude/models
            // The provider strips the "anthropic/" prefix before sending.
            ModelInfo {
                id: "anthropic/claude-opus-4-7",
                provider: "anthropic",
                display_name: "Claude Opus 4.7",
                context_window: 1_000_000,
                max_output_tokens: 128_000,
                input_cost_per_m: 5.0,
                output_cost_per_m: 25.0,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "anthropic/claude-opus-4-6",
                provider: "anthropic",
                display_name: "Claude Opus 4.6",
                context_window: 1_000_000,
                max_output_tokens: 128_000,
                input_cost_per_m: 5.0,
                output_cost_per_m: 25.0,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "anthropic/claude-sonnet-4-6",
                provider: "anthropic",
                display_name: "Claude Sonnet 4.6",
                context_window: 1_000_000,
                max_output_tokens: 64_000,
                input_cost_per_m: 3.0,
                output_cost_per_m: 15.0,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "anthropic/claude-haiku-4-5-20251001",
                provider: "anthropic",
                display_name: "Claude Haiku 4.5",
                context_window: 200_000,
                max_output_tokens: 64_000,
                input_cost_per_m: 1.0,
                output_cost_per_m: 5.0,
                supports_tools: true,
                supports_streaming: true,
            },
            // Legacy Anthropic models
            ModelInfo {
                id: "anthropic/claude-sonnet-4-20250514",
                provider: "anthropic",
                display_name: "Claude Sonnet 4",
                context_window: 200_000,
                max_output_tokens: 64_000,
                input_cost_per_m: 3.0,
                output_cost_per_m: 15.0,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "anthropic/claude-opus-4-20250514",
                provider: "anthropic",
                display_name: "Claude Opus 4",
                context_window: 200_000,
                max_output_tokens: 32_000,
                input_cost_per_m: 15.0,
                output_cost_per_m: 75.0,
                supports_tools: true,
                supports_streaming: true,
            },
            // OpenAI — current models
            ModelInfo {
                id: "openai/gpt-5.5",
                provider: "openai",
                display_name: "GPT-5.5",
                context_window: 1_050_000,
                max_output_tokens: 128_000,
                input_cost_per_m: 2.50,
                output_cost_per_m: 15.00,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "openai/gpt-5.5-mini",
                provider: "openai",
                display_name: "GPT-5.5 Mini",
                context_window: 400_000,
                max_output_tokens: 128_000,
                input_cost_per_m: 0.75,
                output_cost_per_m: 4.50,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "openai/gpt-5.5-nano",
                provider: "openai",
                display_name: "GPT-5.5 Nano",
                context_window: 400_000,
                max_output_tokens: 128_000,
                input_cost_per_m: 0.20,
                output_cost_per_m: 1.25,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "openai/gpt-5.4",
                provider: "openai",
                display_name: "GPT-5.4",
                context_window: 1_050_000,
                max_output_tokens: 128_000,
                input_cost_per_m: 2.50,
                output_cost_per_m: 15.00,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "openai/gpt-5.4-mini",
                provider: "openai",
                display_name: "GPT-5.4 Mini",
                context_window: 400_000,
                max_output_tokens: 128_000,
                input_cost_per_m: 0.75,
                output_cost_per_m: 4.50,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "openai/gpt-5.4-nano",
                provider: "openai",
                display_name: "GPT-5.4 Nano",
                context_window: 400_000,
                max_output_tokens: 128_000,
                input_cost_per_m: 0.20,
                output_cost_per_m: 1.25,
                supports_tools: true,
                supports_streaming: true,
            },
            // OpenAI — legacy non-reasoning models
            ModelInfo {
                id: "openai/gpt-4.1",
                provider: "openai",
                display_name: "GPT-4.1",
                context_window: 1_047_576,
                max_output_tokens: 32_768,
                input_cost_per_m: 2.0,
                output_cost_per_m: 8.0,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "openai/gpt-4.1-mini",
                provider: "openai",
                display_name: "GPT-4.1 Mini",
                context_window: 1_047_576,
                max_output_tokens: 32_768,
                input_cost_per_m: 0.40,
                output_cost_per_m: 1.60,
                supports_tools: true,
                supports_streaming: true,
            },
            // Google
            ModelInfo {
                id: "google/gemini-2.5-flash",
                provider: "google",
                display_name: "Gemini 2.5 Flash",
                context_window: 1_048_576,
                max_output_tokens: 65_536,
                input_cost_per_m: 0.15,
                output_cost_per_m: 0.60,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "google/gemini-2.5-pro",
                provider: "google",
                display_name: "Gemini 2.5 Pro",
                context_window: 1_048_576,
                max_output_tokens: 65_536,
                input_cost_per_m: 1.25,
                output_cost_per_m: 10.0,
                supports_tools: true,
                supports_streaming: true,
            },
            // MiniMax (Token Plan — subscription-based, per-request not per-token)
            ModelInfo {
                id: "minimax/MiniMax-M2.7",
                provider: "minimax",
                display_name: "MiniMax M2.7",
                context_window: 200_000,
                max_output_tokens: 128_000,
                input_cost_per_m: 0.0,
                output_cost_per_m: 0.0,
                supports_tools: true,
                supports_streaming: true,
            },
        ];

        let map = models.into_iter().map(|m| (m.id, m)).collect();
        Self { models: map }
    }

    /// Look up a model by its full ID.
    pub fn get(&self, id: &str) -> Option<&ModelInfo> {
        self.models.get(id)
    }

    /// List all known models.
    pub fn all(&self) -> Vec<&ModelInfo> {
        self.models.values().collect()
    }

    /// Extract the provider-local model name from a full ID.
    ///
    /// Example: `"anthropic/claude-sonnet-4"` → `"claude-sonnet-4"`
    pub fn model_name(full_id: &str) -> &str {
        full_id.split_once('/').map_or(full_id, |(_, name)| name)
    }

    /// Extract the provider name from a full ID.
    ///
    /// Example: `"anthropic/claude-sonnet-4"` → `"anthropic"`
    pub fn provider_name(full_id: &str) -> &str {
        full_id.split_once('/').map_or("unknown", |(provider, _)| provider)
    }

    /// Resolve a model name that may be a shorthand alias.
    ///
    /// Accepts:
    /// - Full ID: `"anthropic/claude-sonnet-4-20250514"` (returned as-is)
    /// - Short names: `"sonnet"`, `"opus"`, `"haiku"`, `"gpt-5.5"`, `"mini"`, `"nano"`
    /// - Partial names: `"claude-sonnet-4"`, `"claude-opus-4"`
    ///
    /// Returns the full model ID, or the input unchanged if no match.
    pub fn resolve(input: &str) -> String {
        // Already a full ID?
        if input.contains('/') {
            return input.to_string();
        }

        let lower = input.to_lowercase();

        // Shorthand aliases
        let resolved = match lower.as_str() {
            // Anthropic — current models (shorthands + explicit versions merged)
            "sonnet" | "claude-sonnet" | "sonnet-4.6" | "claude-sonnet-4.6"
            | "claude-sonnet-4-6" => "anthropic/claude-sonnet-4-6",
            "opus-4.7" | "claude-opus-4.7" | "opus-4-7" | "claude-opus-4-7" => {
                "anthropic/claude-opus-4-7"
            }
            "opus" | "claude-opus" | "opus-4.6" | "claude-opus-4.6" | "claude-opus-4-6"
            | "opus-4-6" => "anthropic/claude-opus-4-6",
            "haiku" | "claude-haiku" | "haiku-4.5" | "claude-haiku-4.5" | "claude-haiku-4-5" => {
                "anthropic/claude-haiku-4-5-20251001"
            }
            // Anthropic — legacy versions
            "sonnet-4" | "claude-sonnet-4" | "sonnet-4.0" => "anthropic/claude-sonnet-4-20250514",
            "opus-4" | "claude-opus-4" | "opus-4.0" => "anthropic/claude-opus-4-20250514",
            "gpt-5.5" | "gpt5.5" | "5.5" | "chatgpt-5.5" | "chatgpt5.5" => "openai/gpt-5.5",
            "gpt-5.5-mini" | "gpt5.5-mini" | "5.5-mini" | "mini" | "chatgpt-5.5-mini" => {
                "openai/gpt-5.5-mini"
            }
            "gpt-5.5-nano" | "gpt5.5-nano" | "5.5-nano" | "nano" | "chatgpt-5.5-nano" => {
                "openai/gpt-5.5-nano"
            }
            "gpt-5.4" | "gpt5.4" | "5.4" | "chatgpt-5.4" | "chatgpt5.4" => "openai/gpt-5.4",
            "gpt-5.4-mini" | "gpt5.4-mini" | "5.4-mini" | "chatgpt-5.4-mini" => {
                "openai/gpt-5.4-mini"
            }
            "gpt-5.4-nano" | "gpt5.4-nano" | "5.4-nano" | "chatgpt-5.4-nano" => {
                "openai/gpt-5.4-nano"
            }
            "gpt-4.1" | "gpt4.1" | "4.1" => "openai/gpt-4.1",
            "gpt-4.1-mini" | "gpt4.1-mini" | "4.1-mini" => "openai/gpt-4.1-mini",
            "flash" | "gemini-flash" | "gemini-2.5-flash" => "google/gemini-2.5-flash",
            "pro" | "gemini-pro" | "gemini-2.5-pro" => "google/gemini-2.5-pro",
            "minimax" | "minimax-m2.7" | "m2.7" => "minimax/MiniMax-M2.7",
            _ => input,
        };

        resolved.to_string()
    }
}

/// Resolve the model ID to use, given a CLI override and loaded config.
///
/// Precedence (first match wins):
/// 1. `cli_model` — the `--model` CLI flag.
/// 2. `config.model` — top-level `model = "..."` in `flok.toml`.
/// 3. First `provider.<name>.default_model` (alphabetical by provider key).
/// 4. `default_fallback` — the hardcoded safety-net alias.
///
/// Each candidate is normalized through [`ModelRegistry::resolve`] so that
/// short aliases like `"opus-4.7"` become full IDs like
/// `"anthropic/claude-opus-4-7"`.
pub fn resolve_default_model(
    cli_model: Option<&str>,
    config: &FlokConfig,
    default_fallback: &str,
) -> String {
    if let Some(m) = cli_model {
        return ModelRegistry::resolve(m);
    }
    if let Some(m) = config.model.as_deref() {
        return ModelRegistry::resolve(m);
    }
    let mut names: Vec<&String> = config.provider.keys().collect();
    names.sort();
    for name in names {
        if let Some(m) = config.provider.get(name).and_then(|p| p.default_model.as_deref()) {
            return ModelRegistry::resolve(m);
        }
    }
    ModelRegistry::resolve(default_fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;

    #[test]
    fn builtin_registry_has_models() {
        let registry = ModelRegistry::builtin();
        assert!(registry.all().len() >= 10);
    }

    #[test]
    fn lookup_known_model() {
        let registry = ModelRegistry::builtin();
        let model = registry.get("anthropic/claude-opus-4-6").unwrap();
        assert_eq!(model.display_name, "Claude Opus 4.6");
        assert_eq!(model.context_window, 1_000_000);
    }

    #[test]
    fn lookup_unknown_model_returns_none() {
        let registry = ModelRegistry::builtin();
        assert!(registry.get("nonexistent/model").is_none());
    }

    #[test]
    fn model_name_extraction() {
        assert_eq!(ModelRegistry::model_name("anthropic/claude-sonnet-4"), "claude-sonnet-4");
        assert_eq!(ModelRegistry::model_name("just-a-name"), "just-a-name");
    }

    #[test]
    fn provider_name_extraction() {
        assert_eq!(ModelRegistry::provider_name("anthropic/claude-sonnet-4"), "anthropic");
        assert_eq!(ModelRegistry::provider_name("no-slash"), "unknown");
    }

    #[test]
    fn resolve_shorthand_sonnet() {
        assert_eq!(ModelRegistry::resolve("sonnet"), "anthropic/claude-sonnet-4-6");
    }

    #[test]
    fn resolve_shorthand_opus() {
        assert_eq!(ModelRegistry::resolve("opus"), "anthropic/claude-opus-4-6");
    }

    #[test]
    fn resolve_full_id_passthrough() {
        let full = "anthropic/claude-opus-4-6";
        assert_eq!(ModelRegistry::resolve(full), full);
    }

    #[test]
    fn resolve_partial_name() {
        assert_eq!(ModelRegistry::resolve("claude-sonnet-4"), "anthropic/claude-sonnet-4-20250514");
    }

    #[test]
    fn resolve_opus_4_6() {
        assert_eq!(ModelRegistry::resolve("opus-4.6"), "anthropic/claude-opus-4-6");
    }

    #[test]
    fn resolve_opus_4_7() {
        assert_eq!(ModelRegistry::resolve("opus-4.7"), "anthropic/claude-opus-4-7");
        assert_eq!(ModelRegistry::resolve("claude-opus-4-7"), "anthropic/claude-opus-4-7");
    }

    #[test]
    fn lookup_openai_gpt_5_4() {
        let registry = ModelRegistry::builtin();
        let model = registry.get("openai/gpt-5.5").unwrap();
        assert_eq!(model.display_name, "GPT-5.5");
        assert_eq!(model.context_window, 1_050_000);
        assert_eq!(model.max_output_tokens, 128_000);
        assert!(model.supports_tools);
        assert!(model.supports_streaming);
    }

    #[test]
    fn resolve_openai_current_shorthands() {
        assert_eq!(ModelRegistry::resolve("gpt-5.5"), "openai/gpt-5.5");
        assert_eq!(ModelRegistry::resolve("chatgpt-5.5"), "openai/gpt-5.5");
        assert_eq!(ModelRegistry::resolve("mini"), "openai/gpt-5.5-mini");
        assert_eq!(ModelRegistry::resolve("nano"), "openai/gpt-5.5-nano");
        assert_eq!(ModelRegistry::resolve("gpt-5.4"), "openai/gpt-5.4");
        assert_eq!(ModelRegistry::resolve("chatgpt-5.4"), "openai/gpt-5.4");
    }

    #[test]
    fn resolve_openai_previous_generation_variants() {
        assert_eq!(ModelRegistry::resolve("gpt-5.4-mini"), "openai/gpt-5.4-mini");
        assert_eq!(ModelRegistry::resolve("gpt-5.4-nano"), "openai/gpt-5.4-nano");
    }

    #[test]
    fn resolve_default_model_cli_wins() {
        let config = FlokConfig { model: Some("sonnet".into()), ..Default::default() };
        assert_eq!(
            resolve_default_model(Some("opus-4.7"), &config, "sonnet"),
            "anthropic/claude-opus-4-7",
        );
    }

    #[test]
    fn resolve_default_model_uses_top_level_config() {
        let config = FlokConfig { model: Some("opus-4.7".into()), ..Default::default() };
        assert_eq!(resolve_default_model(None, &config, "sonnet"), "anthropic/claude-opus-4-7",);
    }

    #[test]
    fn resolve_default_model_falls_back_to_first_provider_alphabetical() {
        use std::collections::HashMap;
        let mut provider = HashMap::new();
        provider.insert(
            "zeta-provider".to_string(),
            ProviderConfig {
                api_key: None,
                base_url: None,
                default_model: Some("sonnet".into()),
                fallback: Vec::new(),
            },
        );
        provider.insert(
            "anthropic".to_string(),
            ProviderConfig {
                api_key: None,
                base_url: None,
                default_model: Some("opus-4.7".into()),
                fallback: Vec::new(),
            },
        );
        let config = FlokConfig { provider, ..Default::default() };
        // Alphabetical order: "anthropic" < "zeta-provider", so opus-4.7 wins.
        assert_eq!(resolve_default_model(None, &config, "sonnet"), "anthropic/claude-opus-4-7",);
    }

    #[test]
    fn resolve_default_model_uses_fallback_when_nothing_set() {
        let config = FlokConfig::default();
        assert_eq!(resolve_default_model(None, &config, "sonnet"), "anthropic/claude-sonnet-4-6",);
    }

    #[test]
    fn resolve_default_model_skips_providers_without_default_model() {
        use std::collections::HashMap;
        let mut provider = HashMap::new();
        provider.insert(
            "anthropic".to_string(),
            ProviderConfig {
                api_key: None,
                base_url: None,
                default_model: None,
                fallback: Vec::new(),
            },
        );
        provider.insert(
            "openai".to_string(),
            ProviderConfig {
                api_key: None,
                base_url: None,
                default_model: Some("gpt-5.4".into()),
                fallback: Vec::new(),
            },
        );
        let config = FlokConfig { provider, ..Default::default() };
        // "anthropic" comes first alphabetically but has no default_model, so the
        // resolver should skip it and pick "openai"'s "gpt-5.4".
        assert_eq!(resolve_default_model(None, &config, "sonnet"), "openai/gpt-5.4");
    }
}
