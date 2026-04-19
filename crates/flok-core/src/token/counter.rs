//! Token counting using `tiktoken-rs` with fallback to char-based estimation.

use std::sync::OnceLock;

use tiktoken_rs::CoreBPE;

/// A token counter that selects the right tokenizer based on model name.
pub struct TokenCounter {
    /// The BPE tokenizer (if available for this model family).
    bpe: Option<&'static CoreBPE>,
}

/// Global cl100k tokenizer instance (used for `OpenAI` GPT-4+ and Anthropic Claude).
static CL100K: OnceLock<CoreBPE> = OnceLock::new();

fn get_cl100k() -> &'static CoreBPE {
    CL100K
        .get_or_init(|| tiktoken_rs::cl100k_base().expect("failed to initialize cl100k tokenizer"))
}

impl TokenCounter {
    /// Create a token counter for the given model.
    ///
    /// Uses cl100k tokenizer for `OpenAI` and Anthropic models (close enough
    /// approximation for Claude). Falls back to char/4 for unknown models.
    pub fn for_model(model_id: &str) -> Self {
        let lower = model_id.to_lowercase();
        let bpe = if lower.contains("gpt")
            || lower.contains("claude")
            || lower.contains("openai")
            || lower.contains("anthropic")
        {
            Some(get_cl100k())
        } else {
            // For Gemini, DeepSeek, etc. — use char-based fallback
            None
        };

        Self { bpe }
    }

    /// Count the number of tokens in a string.
    pub fn count(&self, text: &str) -> usize {
        if let Some(bpe) = self.bpe {
            bpe.encode_with_special_tokens(text).len()
        } else {
            // Fallback: ~4 chars per token (conservative estimate)
            text.len().div_ceil(4)
        }
    }

    /// Whether this counter uses an exact tokenizer or char-based approximation.
    pub fn is_exact(&self) -> bool {
        self.bpe.is_some()
    }
}

/// Convenience function: count tokens for a model.
pub fn count_tokens(text: &str, model_id: &str) -> usize {
    TokenCounter::for_model(model_id).count(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_tokens_with_cl100k() {
        let counter = TokenCounter::for_model("anthropic/claude-sonnet-4");
        let count = counter.count("Hello, world!");
        assert!(count > 0);
        assert!(count < 10); // "Hello, world!" is about 4 tokens
        assert!(counter.is_exact());
    }

    #[test]
    fn count_tokens_fallback() {
        let counter = TokenCounter::for_model("deepseek/deepseek-chat");
        let count = counter.count("Hello, world!");
        assert!(count > 0);
        assert!(!counter.is_exact());
    }

    #[test]
    fn count_tokens_empty_string() {
        let counter = TokenCounter::for_model("anthropic/claude-sonnet-4");
        assert_eq!(counter.count(""), 0);
    }

    #[test]
    fn count_tokens_convenience_function() {
        let count = count_tokens("fn main() { println!(\"hello\"); }", "openai/gpt-5.4");
        assert!(count > 5);
        assert!(count < 20);
    }

    #[test]
    fn fallback_approximation_reasonable() {
        let counter = TokenCounter::for_model("unknown/model");
        // 100 chars should be ~25 tokens
        let text = "a".repeat(100);
        let count = counter.count(&text);
        assert_eq!(count, 25);
    }
}
