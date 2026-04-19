//! `ProviderRegistry`: runtime container for all configured providers.
//!
//! Unlike the binary crate's single-provider startup path, this registry keeps
//! every configured provider with credentials available at runtime so tools can
//! dispatch sub-agents across providers independently.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::provider::{ModelRegistry, Provider};

/// Default concurrent sub-agent API calls per provider.
pub const DEFAULT_PERMITS_PER_PROVIDER: usize = 3;

/// Runtime container for all configured providers.
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn Provider>>,
    default_models: HashMap<String, String>,
    semaphores: HashMap<String, Arc<Semaphore>>,
}

impl ProviderRegistry {
    /// Create an empty provider registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            default_models: HashMap::new(),
            semaphores: HashMap::new(),
        }
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
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
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
    use crate::provider::mock::MockProvider;

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
}
