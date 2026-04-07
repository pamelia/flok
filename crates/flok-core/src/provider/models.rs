//! Built-in model registry.
//!
//! A hardcoded table of known models with context window sizes and pricing.
//! Users can override or add models via `flok.toml`.

use std::collections::HashMap;

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
            // OpenAI
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
            // DeepSeek
            ModelInfo {
                id: "deepseek/deepseek-chat",
                provider: "deepseek",
                display_name: "DeepSeek V3",
                context_window: 128_000,
                max_output_tokens: 8_192,
                input_cost_per_m: 0.27,
                output_cost_per_m: 1.10,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "deepseek/deepseek-reasoner",
                provider: "deepseek",
                display_name: "DeepSeek R1",
                context_window: 128_000,
                max_output_tokens: 16_384,
                input_cost_per_m: 0.55,
                output_cost_per_m: 2.19,
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
    /// - Short names: `"sonnet"`, `"opus"`, `"haiku"`, `"gpt-4.1"`, `"flash"`, `"pro"`
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
            "opus" | "claude-opus" | "opus-4.6" | "claude-opus-4.6" | "claude-opus-4-6"
            | "opus-4-6" => "anthropic/claude-opus-4-6",
            "haiku" | "claude-haiku" | "haiku-4.5" | "claude-haiku-4.5" | "claude-haiku-4-5" => {
                "anthropic/claude-haiku-4-5-20251001"
            }
            // Anthropic — legacy versions
            "sonnet-4" | "claude-sonnet-4" | "sonnet-4.0" => "anthropic/claude-sonnet-4-20250514",
            "opus-4" | "claude-opus-4" | "opus-4.0" => "anthropic/claude-opus-4-20250514",
            "gpt-4.1" | "gpt4.1" | "4.1" => "openai/gpt-4.1",
            "gpt-4.1-mini" | "4.1-mini" | "mini" => "openai/gpt-4.1-mini",
            "flash" | "gemini-flash" | "gemini-2.5-flash" => "google/gemini-2.5-flash",
            "pro" | "gemini-pro" | "gemini-2.5-pro" => "google/gemini-2.5-pro",
            "deepseek" | "deepseek-v3" | "deepseek-chat" => "deepseek/deepseek-chat",
            "r1" | "deepseek-r1" | "deepseek-reasoner" => "deepseek/deepseek-reasoner",
            "minimax" | "minimax-m2.7" | "m2.7" => "minimax/MiniMax-M2.7",
            _ => input,
        };

        resolved.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_has_models() {
        let registry = ModelRegistry::builtin();
        assert!(registry.all().len() >= 9);
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
}
