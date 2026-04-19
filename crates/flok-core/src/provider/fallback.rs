use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::provider::ProviderRegistry;

/// In-memory cooldown tracker.
#[derive(Debug, Default)]
pub struct CooldownTracker {
    cooldowns: Mutex<HashMap<String, Instant>>,
}

impl CooldownTracker {
    /// Mark a provider as unavailable until `cooldown` elapses.
    pub fn mark(&self, provider: &str, cooldown: Duration) {
        let until = Instant::now() + cooldown;
        self.cooldowns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(provider.to_string(), until);
    }

    /// Whether the provider is currently cooling down.
    #[must_use]
    pub fn is_cooldown(&self, provider: &str) -> bool {
        let now = Instant::now();
        let mut cooldowns =
            self.cooldowns.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

        match cooldowns.get(provider).copied() {
            Some(until) if until > now => true,
            Some(_) => {
                cooldowns.remove(provider);
                false
            }
            None => false,
        }
    }

    /// Remove expired cooldown entries.
    pub fn cleanup_expired(&self) {
        let now = Instant::now();
        self.cooldowns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, until| *until > now);
    }
}

/// Classify whether an error is retriable given the configured codes.
#[must_use]
pub fn is_retriable(err: &anyhow::Error, retry_on_errors: &[u16]) -> bool {
    err.chain()
        .filter_map(|cause| extract_status_code_from_message(&cause.to_string()))
        .any(|status| retry_on_errors.contains(&status))
}

fn extract_status_code_from_message(message: &str) -> Option<u16> {
    let bytes = message.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }

            let digits = &message[start..index];
            if digits.len() == 3 {
                let status = digits.parse::<u16>().ok()?;
                if (100..=599).contains(&status) {
                    return Some(status);
                }
            }
        } else {
            index += 1;
        }
    }

    None
}

/// The fallback chain resolver.
pub struct FallbackChain<'a> {
    pub primary: &'a str,
    pub fallbacks: &'a [String],
    pub cooldown: &'a CooldownTracker,
    pub registry: &'a ProviderRegistry,
}

impl<'a> FallbackChain<'a> {
    /// Return ordered list of providers to try, skipping cooldown ones.
    /// If all configured candidates are cooling down, returns the primary anyway.
    #[must_use]
    pub fn attempt_order(&self) -> Vec<&'a str> {
        let mut order = Vec::new();
        let mut cooled_candidates = 0usize;

        for provider in
            std::iter::once(self.primary).chain(self.fallbacks.iter().map(String::as_str))
        {
            if order.contains(&provider) || self.registry.get(provider).is_none() {
                continue;
            }

            if self.cooldown.is_cooldown(provider) {
                cooled_candidates += 1;
                continue;
            }

            order.push(provider);
        }

        if order.is_empty() && cooled_candidates > 0 {
            return vec![self.primary];
        }

        order
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anyhow::anyhow;

    use super::*;
    use crate::provider::{mock::MockProvider, Provider};

    #[test]
    fn cooldown_tracker_mark_then_is_cooldown_true() {
        let tracker = CooldownTracker::default();
        tracker.mark("anthropic", Duration::from_secs(60));

        assert!(tracker.is_cooldown("anthropic"));
    }

    #[test]
    fn cooldown_tracker_cooldown_expires() {
        let tracker = CooldownTracker::default();
        tracker.mark("anthropic", Duration::from_millis(10));

        std::thread::sleep(Duration::from_millis(20));

        assert!(!tracker.is_cooldown("anthropic"));
    }

    #[test]
    fn is_retriable_matches_429() {
        let err = anyhow!("HTTP 429: Rate limited");

        assert!(is_retriable(&err, &[429, 503]));
    }

    #[test]
    fn is_retriable_excludes_400() {
        let err = anyhow!("HTTP 400: Bad request");

        assert!(!is_retriable(&err, &[429, 500, 503]));
    }

    #[test]
    fn fallback_chain_skips_cooldown() {
        let tracker = CooldownTracker::default();
        tracker.mark("anthropic", Duration::from_secs(60));

        let mut registry = ProviderRegistry::new();
        let anthropic: Arc<dyn Provider> = Arc::new(MockProvider::new());
        let openai: Arc<dyn Provider> = Arc::new(MockProvider::new());
        registry.insert("anthropic", anthropic, Some("anthropic/claude-sonnet-4-6".into()), 3);
        registry.insert("openai", openai, Some("openai/gpt-5.4".into()), 3);

        let chain = FallbackChain {
            primary: "anthropic",
            fallbacks: &["openai".to_string()],
            cooldown: &tracker,
            registry: &registry,
        };

        assert_eq!(chain.attempt_order(), vec!["openai"]);
    }

    #[test]
    fn fallback_chain_returns_primary_if_all_cooldown() {
        let tracker = CooldownTracker::default();
        tracker.mark("anthropic", Duration::from_secs(60));
        tracker.mark("openai", Duration::from_secs(60));

        let mut registry = ProviderRegistry::new();
        let anthropic: Arc<dyn Provider> = Arc::new(MockProvider::new());
        let openai: Arc<dyn Provider> = Arc::new(MockProvider::new());
        registry.insert("anthropic", anthropic, Some("anthropic/claude-sonnet-4-6".into()), 3);
        registry.insert("openai", openai, Some("openai/gpt-5.4".into()), 3);

        let chain = FallbackChain {
            primary: "anthropic",
            fallbacks: &["openai".to_string()],
            cooldown: &tracker,
            registry: &registry,
        };

        assert_eq!(chain.attempt_order(), vec!["anthropic"]);
    }
}
