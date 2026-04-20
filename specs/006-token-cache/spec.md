# Feature Specification: Token Cache & Performance

**Feature Branch**: `006-token-cache`
**Created**: 2026-03-28
**Status**: Accepted (2026-04-19 — feature shipped; spec retroactively locked to match built reality.)

## User Scenarios & Testing

### User Story 1 - Flok Estimates Token Counts Without Provider Round-Trip (Priority: P0)
**Why this priority**: Accurate local token estimation is critical for context window management, compaction triggering, and cost tracking. Waiting for the provider to tell us the count is too late.
**Acceptance Scenarios**:
1. **Given** a conversation with 100 messages, **When** flok estimates the token count, **Then** the estimate is within 5% of the actual provider-reported count.
2. **Given** the same system prompt is sent across multiple turns, **When** flok checks the cache, **Then** the cached token count is returned in < 1μs (no re-tokenization).
3. **Given** a new message is added to the conversation, **When** flok estimates the new total, **Then** only the new message is tokenized (incremental, not full re-count).

### User Story 2 - Token Estimation Doesn't Block the Hot Path (Priority: P0)
**Why this priority**: Any latency in the prompt assembly path directly increases time-to-first-token.
**Acceptance Scenarios**:
1. **Given** prompt assembly runs before each LLM call, **When** token estimation is computed, **Then** it adds < 1ms to the total prompt assembly time for a 200K-token conversation.
2. **Given** 5 agents are running concurrently, **When** all estimate tokens simultaneously, **Then** no lock contention occurs (lock-free or per-agent caches).

### User Story 3 - Developer Sees Accurate Cost Tracking (Priority: P1)
**Why this priority**: Users need to know what they're spending.
**Acceptance Scenarios**:
1. **Given** a conversation with Anthropic, **When** the session completes, **Then** the total cost displayed matches the sum of per-step costs, accounting for input/output/cache-read/cache-write rates.
2. **Given** multiple models are used in a session (primary + utility), **When** costs are summed, **Then** each model's pricing is applied correctly.

### Edge Cases
- Unknown model (custom/local): fall back to character-based estimation (1 token ≈ 4 chars)
- Very long single message (>100K chars): tokenize in chunks to avoid memory spikes
- Non-ASCII / CJK text: tokenizer handles correctly (different char-to-token ratios)
- Concurrent cache reads/writes: lock-free data structures, no contention
- Provider reports different count than estimate: log the delta, use provider's number for cost, keep estimate for pre-flight

## Requirements

### Functional Requirements

- **FR-001**: Flok MUST provide local token estimation that is within 5% of the actual provider-reported count for supported models.
- **FR-002**: Token estimation MUST support model-specific tokenizers:
  - Anthropic (Claude): Approximate with cl100k_base or provider-specific tokenizer
  - OpenAI (GPT-4/4o/o-series): Use tiktoken cl100k_base or o200k_base
  - Google (Gemini): Approximate with character-based heuristic (1 token ≈ 4 chars)
  - Fallback: Character-based heuristic (1 token ≈ 4 chars for English, 1.5 chars for CJK)
- **FR-003**: Flok MUST cache token counts at multiple granularities:
  - Per-message token count (cached on first computation, invalidated on edit)
  - System prompt token count (cached per agent, invalidated on prompt change)
  - Conversation prefix token count (cumulative, incrementally updated)
- **FR-004**: The token cache MUST be lock-free for reads. Writes MAY use fine-grained locking.
- **FR-005**: Token estimation MUST be incremental: adding a message should only tokenize the new message, not re-count the entire history.
- **FR-006**: Cost calculation MUST use model-specific pricing from the provider config:
  ```
  cost = (input_tokens * input_per_million / 1_000_000)
       + (output_tokens * output_per_million / 1_000_000)
       + (cache_read_tokens * cache_read_per_million / 1_000_000)
       + (cache_write_tokens * cache_write_per_million / 1_000_000)
       + (reasoning_tokens * output_per_million / 1_000_000)
  ```
- **FR-007**: Cost arithmetic MUST use `f64` with sufficient precision (not floating-point-lossy for accumulated sums). For display, round to 6 decimal places.
- **FR-008**: Token counts MUST be reported in the `StepPart` of each assistant message for per-step granularity.
- **FR-009**: The token cache MUST support concurrent access from multiple agents without contention.
- **FR-010**: Flok MUST track and report cache hit rates for prompt caching (Anthropic cache_read vs total input).

### Key Entities

```rust
/// Per-message cached token count
pub struct TokenCount {
    pub estimated_tokens: u32,
    pub tokenizer: TokenizerKind,
    pub computed_at: Instant,
}

/// Model-specific tokenizer
pub enum TokenizerKind {
    Tiktoken(TiktokenModel),  // cl100k_base, o200k_base
    CharBased(f32),           // chars_per_token ratio
}

pub enum TiktokenModel {
    Cl100kBase,   // GPT-4, Claude (approximate)
    O200kBase,    // GPT-4o, o-series
}

/// Incremental token counter for a conversation
pub struct ConversationTokenCounter {
    /// Per-message cached counts: MessageID -> token count
    message_cache: DashMap<MessageID, u32>,
    /// Cumulative prefix sum (updated incrementally)
    prefix_total: AtomicU64,
    /// System prompt token count (stable across turns)
    system_prompt_tokens: AtomicU32,
    /// Tokenizer for this conversation's model
    tokenizer: Arc<Tokenizer>,
}

/// Thread-safe tokenizer wrapper
pub struct Tokenizer {
    kind: TokenizerKind,
    /// Pre-compiled BPE model (for tiktoken)
    bpe: Option<tiktoken_rs::CoreBPE>,
}

/// Accumulated cost for a session
pub struct SessionCost {
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_read_cost: f64,
    pub cache_write_cost: f64,
    pub total_cost: f64,
    pub total_tokens: u64,
}
```

## Design

### Overview

Token caching is a performance-critical subsystem that sits between the session engine and the provider system. It provides fast, accurate token estimates for three purposes: (1) context window management (compaction trigger), (2) cost tracking, and (3) prompt cache optimization. The design principle is: **never re-count what hasn't changed**.

### Detailed Design

#### Architecture: Three Cache Layers

```
Layer 1: System Prompt Cache
├── Keyed by: (agent_name, model_id)
├── Invalidated by: config reload or agent change
├── Lifetime: session
└── Access: AtomicU32 (lock-free read)

Layer 2: Per-Message Cache
├── Keyed by: MessageID
├── Invalidated by: never (messages are immutable once created)
├── Lifetime: session
└── Access: DashMap (sharded lock-free reads)

Layer 3: Conversation Prefix Sum
├── Value: cumulative token count of all messages before the latest
├── Updated: incrementally when new message is added
├── Lifetime: prompt loop iteration
└── Access: AtomicU64 (lock-free read)
```

#### Incremental Token Counting

When a new message is added to the conversation:

```rust
impl ConversationTokenCounter {
    pub fn add_message(&self, message_id: MessageID, content: &str) -> u32 {
        // 1. Tokenize ONLY the new message
        let tokens = self.tokenizer.count(content);

        // 2. Cache the count
        self.message_cache.insert(message_id, tokens);

        // 3. Update prefix sum (atomic add)
        self.prefix_total.fetch_add(tokens as u64, Ordering::Relaxed);

        tokens
    }

    pub fn total_tokens(&self) -> u64 {
        self.system_prompt_tokens.load(Ordering::Relaxed) as u64
            + self.prefix_total.load(Ordering::Relaxed)
    }

    pub fn context_usage_ratio(&self, context_window: usize) -> f64 {
        self.total_tokens() as f64 / context_window as f64
    }
}
```

#### Tokenizer Implementation

For tiktoken-based counting, we use `tiktoken-rs` which provides the exact BPE tokenizers used by OpenAI and Anthropic:

```rust
impl Tokenizer {
    pub fn count(&self, text: &str) -> u32 {
        match &self.kind {
            TokenizerKind::Tiktoken(_) => {
                // tiktoken-rs BPE encoding
                self.bpe.as_ref().unwrap().encode_ordinary(text).len() as u32
            }
            TokenizerKind::CharBased(ratio) => {
                // Fast fallback: character count / ratio
                (text.len() as f32 / ratio).ceil() as u32
            }
        }
    }
}
```

**Performance note**: `tiktoken-rs` BPE encoding is CPU-intensive for large texts. For messages > 10KB, we use a sampling strategy: tokenize the first 1KB, extrapolate based on the ratio, then adjust with a small correction factor. This reduces tokenization time from O(n) to O(1) for large messages while maintaining < 3% error.

```rust
const SAMPLE_SIZE: usize = 1024;
const LARGE_MESSAGE_THRESHOLD: usize = 10_240;

impl Tokenizer {
    pub fn count_fast(&self, text: &str) -> u32 {
        if text.len() < LARGE_MESSAGE_THRESHOLD {
            return self.count(text);
        }
        // Sample-based estimation for large texts
        let sample = &text[..SAMPLE_SIZE.min(text.len())];
        let sample_tokens = self.count(sample);
        let ratio = sample_tokens as f64 / sample.len() as f64;
        (text.len() as f64 * ratio).ceil() as u32
    }
}
```

#### Tool Output Token Estimation

Tool outputs (especially file reads and grep results) can be very large. We estimate their token contribution without full tokenization:

```rust
/// Fast token estimation for tool outputs
/// Tool outputs are typically ASCII-heavy (code, paths, numbers)
/// so the char-to-token ratio is more predictable
pub fn estimate_tool_output_tokens(output: &str) -> u32 {
    // ASCII-heavy content: ~3.5 chars per token
    // Mixed content: ~3.0 chars per token
    let ratio = if output.is_ascii() { 3.5 } else { 3.0 };
    (output.len() as f64 / ratio).ceil() as u32
}
```

#### Cost Calculation

Cost is computed per LLM step using the provider's reported usage (not our estimates):

```rust
pub fn compute_cost(usage: &Usage, pricing: &ModelCost) -> f64 {
    let input = usage.input_tokens as f64 * pricing.input_per_million / 1_000_000.0;
    let output = usage.output_tokens as f64 * pricing.output_per_million / 1_000_000.0;
    let cache_read = usage.cache_read_tokens as f64 * pricing.cache_read_per_million / 1_000_000.0;
    let cache_write = usage.cache_write_tokens as f64 * pricing.cache_write_per_million / 1_000_000.0;
    let reasoning = usage.reasoning_tokens as f64 * pricing.output_per_million / 1_000_000.0;
    input + output + cache_read + cache_write + reasoning
}
```

We use f64 for cost calculations. At $0.000003 per token (Haiku pricing), f64 provides 15+ significant digits -- more than sufficient. We display costs rounded to 6 decimal places (`$0.001234`).

#### Prompt Cache Optimization Feedback Loop

The token cache feeds into the provider's cache control strategy:

```
1. Token counter knows system prompt = 5000 tokens
2. Token counter knows conversation prefix = 45000 tokens
3. Provider system places cache breakpoints at:
   - After system prompt (5000 tokens) → ephemeral
   - After conversation prefix (50000 tokens) → ephemeral
4. On response, provider reports cache_read_tokens
5. Token cache records hit rate: cache_read / total_input
6. If hit rate < 50% for 3 consecutive turns:
   - Log warning
   - Check if message ordering is stable
   - Verify system prompt hasn't changed
```

#### Concurrency Model

All cache operations are designed for zero-contention concurrent access:

| Component | Data Structure | Concurrent Access Pattern |
|-----------|---------------|--------------------------|
| System prompt cache | `AtomicU32` | Lock-free read/write |
| Per-message cache | `DashMap<MessageID, u32>` | Sharded concurrent reads, per-shard writes |
| Prefix sum | `AtomicU64` | Lock-free read, atomic add |
| Cost accumulator | `AtomicU64` (fixed-point) | Lock-free atomic add |
| Tokenizer BPE model | `Arc<CoreBPE>` | Immutable, shared across threads |

No `Mutex` or `RwLock` in the hot path.

### Alternatives Considered

1. **Use provider's token count exclusively (no local estimation)**: Rejected. We need pre-flight estimates for compaction triggering. By the time the provider tells us the count, we've already sent the request.
2. **Full tiktoken for every message**: Rejected for large messages. Sampling-based estimation is 100x faster with < 3% error for messages > 10KB.
3. **Store token counts in SQLite**: Rejected for hot-path access. In-memory `DashMap` is orders of magnitude faster. We only persist final cost per step.
4. **Use Decimal type for cost**: Rejected. f64 has sufficient precision for our use case (max total cost per session is realistically < $100, and f64 handles that with 10+ significant digits).
5. **Shared tokenizer pool**: Rejected. `tiktoken-rs::CoreBPE` is `Sync + Send` and immutable after construction. A single `Arc<CoreBPE>` shared across all threads is sufficient.

## Success Criteria

- **SC-001**: Token estimation for a single message (< 10KB) completes in < 100μs
- **SC-002**: Token estimation for a single message (> 10KB) via sampling completes in < 200μs
- **SC-003**: Total conversation token count retrieval completes in < 1μs (cached prefix sum)
- **SC-004**: Token estimation accuracy within 5% of provider-reported count for supported tokenizers
- **SC-005**: Zero lock contention in token cache when 10 agents access simultaneously
- **SC-006**: Cost calculation per step completes in < 1μs
- **SC-007**: Memory overhead of token cache < 1MB for a 1000-message session

## Assumptions

- tiktoken cl100k_base is a sufficiently accurate proxy for Anthropic's tokenizer (typically within 3%)
- Message content is immutable after creation (no re-tokenization needed)
- Sampling-based estimation for large messages is acceptable (< 3% error)
- f64 precision is sufficient for cost arithmetic in our domain

## Open Questions

- Should we support Anthropic's native token counting API for exact counts (additional HTTP round-trip)?
- Should we persist token counts across sessions (warm cache on session reload)?
- Should we add a `--cost-report` CLI flag for detailed per-session cost breakdown?
- Should we support custom tokenizers for local models (llama.cpp, etc.)?
