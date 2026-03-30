//! Session cost tracking.
//!
//! Accumulates token usage and computes costs based on model pricing.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::provider::ModelRegistry;

/// Tracks cumulative token usage and cost for a session.
#[derive(Debug)]
pub struct CostTracker {
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
    cache_read_tokens: AtomicU64,
    cache_creation_tokens: AtomicU64,
    model_id: String,
}

impl CostTracker {
    /// Create a new cost tracker for the given model.
    pub fn new(model_id: &str) -> Self {
        Self {
            input_tokens: AtomicU64::new(0),
            output_tokens: AtomicU64::new(0),
            cache_read_tokens: AtomicU64::new(0),
            cache_creation_tokens: AtomicU64::new(0),
            model_id: model_id.to_string(),
        }
    }

    /// Record token usage from a provider response.
    pub fn record(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
    ) {
        self.input_tokens.fetch_add(input_tokens, Ordering::Relaxed);
        self.output_tokens.fetch_add(output_tokens, Ordering::Relaxed);
        self.cache_read_tokens.fetch_add(cache_read_tokens, Ordering::Relaxed);
        self.cache_creation_tokens.fetch_add(cache_creation_tokens, Ordering::Relaxed);
    }

    /// Get the total input tokens.
    pub fn total_input_tokens(&self) -> u64 {
        self.input_tokens.load(Ordering::Relaxed)
    }

    /// Get the total output tokens.
    pub fn total_output_tokens(&self) -> u64 {
        self.output_tokens.load(Ordering::Relaxed)
    }

    /// Get the total cache read tokens.
    pub fn total_cache_read_tokens(&self) -> u64 {
        self.cache_read_tokens.load(Ordering::Relaxed)
    }

    /// Get the total cache creation tokens.
    pub fn total_cache_creation_tokens(&self) -> u64 {
        self.cache_creation_tokens.load(Ordering::Relaxed)
    }

    /// Calculate the estimated cost in USD.
    ///
    /// For Anthropic with prompt caching:
    /// - Cache read tokens cost 10% of normal input price
    /// - Cache creation tokens cost 125% of normal input price
    /// - Non-cached input tokens cost full input price
    pub fn estimated_cost_usd(&self) -> f64 {
        let registry = ModelRegistry::builtin();
        let Some(model) = registry.get(&self.model_id) else {
            return 0.0;
        };

        let input = self.total_input_tokens();
        let output = self.total_output_tokens();
        let cache_read = self.total_cache_read_tokens();
        let cache_creation = self.total_cache_creation_tokens();

        // Non-cached input = total input - cache read - cache creation
        let uncached_input = input.saturating_sub(cache_read).saturating_sub(cache_creation);

        let input_cost = (uncached_input as f64 / 1_000_000.0) * model.input_cost_per_m;
        let output_cost = (output as f64 / 1_000_000.0) * model.output_cost_per_m;
        let cache_read_cost = (cache_read as f64 / 1_000_000.0) * model.input_cost_per_m * 0.1;
        let cache_create_cost =
            (cache_creation as f64 / 1_000_000.0) * model.input_cost_per_m * 1.25;

        input_cost + output_cost + cache_read_cost + cache_create_cost
    }

    /// Format cost as a human-readable string.
    pub fn format_cost(&self) -> String {
        let cost = self.estimated_cost_usd();
        if cost < 0.01 {
            format!("${cost:.4}")
        } else {
            format!("${cost:.2}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_read_tokens() {
        let tracker = CostTracker::new("anthropic/claude-sonnet-4-20250514");
        tracker.record(1000, 500, 200, 100);
        assert_eq!(tracker.total_input_tokens(), 1000);
        assert_eq!(tracker.total_output_tokens(), 500);
        assert_eq!(tracker.total_cache_read_tokens(), 200);
    }

    #[test]
    fn accumulates_across_calls() {
        let tracker = CostTracker::new("anthropic/claude-sonnet-4-20250514");
        tracker.record(100, 50, 0, 0);
        tracker.record(200, 100, 0, 0);
        assert_eq!(tracker.total_input_tokens(), 300);
        assert_eq!(tracker.total_output_tokens(), 150);
    }

    #[test]
    fn cost_calculation_nonzero() {
        let tracker = CostTracker::new("anthropic/claude-sonnet-4-20250514");
        tracker.record(100_000, 10_000, 0, 0);
        let cost = tracker.estimated_cost_usd();
        // 100K input at $3/M = $0.30, 10K output at $15/M = $0.15
        assert!(cost > 0.4);
        assert!(cost < 0.5);
    }

    #[test]
    fn cost_with_cache_savings() {
        let tracker = CostTracker::new("anthropic/claude-sonnet-4-20250514");
        // 100K total input, 80K from cache read, 10K cache creation, 10K uncached
        tracker.record(100_000, 10_000, 80_000, 10_000);
        let cost = tracker.estimated_cost_usd();
        // Much cheaper than without caching
        let no_cache = CostTracker::new("anthropic/claude-sonnet-4-20250514");
        no_cache.record(100_000, 10_000, 0, 0);
        assert!(cost < no_cache.estimated_cost_usd());
    }

    #[test]
    fn unknown_model_returns_zero_cost() {
        let tracker = CostTracker::new("unknown/model");
        tracker.record(100_000, 10_000, 0, 0);
        assert!(tracker.estimated_cost_usd().abs() < f64::EPSILON);
    }

    #[test]
    fn format_cost_small() {
        let tracker = CostTracker::new("anthropic/claude-sonnet-4-20250514");
        tracker.record(1000, 100, 0, 0);
        let formatted = tracker.format_cost();
        assert!(formatted.starts_with('$'));
    }
}
