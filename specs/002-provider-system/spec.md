# Feature Specification: Provider System

**Feature Branch**: `002-provider-system`
**Created**: 2026-03-28
**Status**: Accepted (2026-04-19 — feature shipped; spec retroactively locked to match built reality.)

## User Scenarios & Testing

### User Story 1 - Developer Uses Multiple LLM Providers (Priority: P0)
**Why this priority**: The core value of flok depends on talking to LLMs.
**Acceptance Scenarios**:
1. **Given** `ANTHROPIC_API_KEY` is set, **When** the user starts a session, **Then** Anthropic models (Claude Sonnet, Claude Haiku, etc.) are available for selection.
2. **Given** both Anthropic and OpenAI keys are configured, **When** the user selects `openai/gpt-4o`, **Then** requests route to OpenAI's API with correct headers and payload format.
3. **Given** a model is streaming a response, **When** the user sees text appearing token-by-token, **Then** latency between provider SSE chunks and TUI rendering is < 5ms.

### User Story 2 - Developer Configures Model Routing Per Process Type (Priority: P1)
**Why this priority**: Different agent roles benefit from different models (fast vs capable).
**Acceptance Scenarios**:
1. **Given** routing config specifies `channel = "anthropic/claude-sonnet-4"` and `worker = "anthropic/claude-haiku-4.5"`, **When** a worker agent is spawned, **Then** it uses Haiku, not Sonnet.
2. **Given** a model returns a 429 rate limit error, **When** fallback models are configured, **Then** flok automatically retries with the next fallback within 500ms.
3. **Given** a model has been rate-limited, **When** the cooldown period (60s default) expires, **Then** the primary model is tried again.

### User Story 3 - Developer Gets Prompt Cache Hits (Priority: P0)
**Why this priority**: Cache hits reduce cost by 90% and latency by 50%+.
**Acceptance Scenarios**:
1. **Given** a conversation with a stable system prompt, **When** subsequent messages are sent, **Then** the system prompt portion achieves cache hits (verified by `cache_read_input_tokens > 0` in usage).
2. **Given** Anthropic provider with cache control, **When** assembling the request, **Then** cache breakpoints are placed at system prompt and conversation prefix boundaries.

### Edge Cases
- Provider API is unreachable: timeout after 30s per attempt, retry with exponential backoff
- API key is invalid: surface clear error immediately, don't retry
- Model returns malformed SSE: log warning, skip malformed chunk, continue stream
- Stream hangs (no data for 30s): abort, surface timeout error
- Provider returns unexpected HTTP status: map to typed error, include status code and body

## Requirements

### Functional Requirements

- **FR-001**: Flok MUST support the following provider APIs out of the box:
  - Anthropic Messages API (streaming + non-streaming)
  - OpenAI Chat Completions API (streaming + non-streaming)
  - OpenAI Responses API
  - Google Gemini API
  - Any OpenAI-compatible API (via base URL override)
- **FR-002**: Flok MUST auto-detect providers from environment variables:
  - `ANTHROPIC_API_KEY` → Anthropic provider
  - `OPENAI_API_KEY` → OpenAI provider
  - `GOOGLE_API_KEY` → Google Gemini provider
  - `OPENROUTER_API_KEY` → OpenRouter (OpenAI-compatible)
  - `XAI_API_KEY` → xAI (OpenAI-compatible)
  - Additional providers configurable in `flok.toml`
- **FR-003**: Flok MUST support intelligent model routing via the ModelRouter (see spec-012 for full details):
  ```toml
  [routing]
  explore = "deepseek/deepseek-chat"        # Cheap, fast: file reads, grep, exploration
  build   = "anthropic/claude-sonnet-4.6"   # Balanced: code gen, refactoring
  plan    = "anthropic/claude-opus-4.6"     # Deep reasoning: architecture, planning
  utility = "anthropic/claude-haiku-4.5"    # Internal: title gen, compaction, summaries
  ```
  The router scores prompt complexity and automatically selects the appropriate tier. This alone can cut costs 3-5x.
- **FR-003a**: Flok MUST support mixed-model agent teams: each agent within a team can use a different model/provider. A lead agent can be Claude Opus while workers use GPT-5.4 or Gemini -- all in the same session. Per-agent model overrides take precedence over routing tiers.
- **FR-004**: Flok MUST support fallback chains per routing tier:
  ```toml
  [routing.fallbacks]
  explore = ["openai/gpt-5.4-mini", "google/gemini-2.5-flash"]
  build = ["openai/gpt-5.4", "google/gemini-2.5-pro"]
  plan = ["openai/o3", "google/gemini-2.5-pro"]
  ```
- **FR-005**: Flok MUST implement retry with exponential backoff:
  - Max 3 retries per model
  - Base delay: 500ms, doubling each retry
  - Max 3 fallback attempts across the chain
  - Rate-limited models enter cooldown (configurable, default 60s)
- **FR-006**: Flok MUST track rate-limited models in a `DashMap<String, Instant>` and skip them until cooldown expires.
- **FR-007**: Flok MUST support provider-specific prompt caching strategies:
  - Anthropic: `cache_control` breakpoints on system prompt and conversation prefix
  - OpenAI: Automatic caching (no explicit markers needed)
  - Others: No caching (passthrough)
- **FR-008**: Flok MUST parse and normalize token usage from all providers into a unified `Usage` struct.
- **FR-009**: Flok MUST support hot-reloading of API keys and routing config via `ArcSwap`.
- **FR-010**: Flok MUST support OAuth token refresh for Anthropic, OpenAI, and GitHub Copilot.

### Key Entities

```rust
pub struct ProviderConfig {
    pub id: String,            // e.g., "anthropic", "openai", "openrouter"
    pub api_type: ApiType,     // Anthropic, OpenAiChat, OpenAiResponses, Gemini
    pub base_url: String,
    pub api_key: KeySource,    // Env, Config, Secret, OAuth
    pub models: Vec<ModelConfig>,
    pub headers: HashMap<String, String>,
    pub timeout: Duration,     // default 120s
}

pub enum ApiType {
    Anthropic,
    OpenAiChat,
    OpenAiResponses,
    Gemini,
    OpenAiCompatible,
}

pub enum KeySource {
    Env(String),              // Environment variable name
    Config(String),           // Direct value from config
    Secret(String),           // Encrypted in redb
    OAuth(OAuthConfig),       // OAuth token refresh
}

pub struct ModelConfig {
    pub id: String,            // e.g., "claude-sonnet-4-20250514"
    pub name: String,          // e.g., "Claude Sonnet 4"
    pub context_window: usize, // e.g., 200_000
    pub max_output: usize,     // e.g., 64_000
    pub cost: ModelCost,
    pub capabilities: ModelCapabilities,
}

pub struct ModelCost {
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cache_read_per_million: f64,
    pub cache_write_per_million: f64,
}

pub struct ModelCapabilities {
    pub streaming: bool,
    pub tool_use: bool,
    pub vision: bool,
    pub reasoning: bool,      // Extended thinking / chain-of-thought
    pub cache_control: bool,  // Explicit cache breakpoints
}

pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,        // Computed from model pricing
}
```

## Design

### Overview

The provider system is a thin, high-performance abstraction over heterogeneous LLM APIs. It translates flok's internal message format into provider-specific wire formats, handles streaming, retries, fallbacks, and normalizes responses back into a unified type.

### Detailed Design

#### Provider Registry

```rust
pub struct ProviderRegistry {
    providers: HashMap<String, ProviderConfig>,
    routing: RoutingConfig,
    rate_limited: DashMap<String, Instant>,
    http_client: reqwest::Client,  // Shared, connection-pooled
}
```

The registry is wrapped in `ArcSwap` for hot-reload. The `http_client` is constructed once at startup with:
- Connection pooling (keep-alive)
- 120s total timeout
- gzip decompression
- HTTP/2 with adaptive window size

#### Streaming Architecture

All LLM responses flow through a unified streaming interface:

```rust
pub enum StreamEvent {
    TextDelta(String),
    ReasoningDelta(String),
    ToolCallStart { id: String, name: String },
    ToolCallDelta { id: String, arguments_delta: String },
    ToolCallEnd { id: String },
    Usage(Usage),
    Error(ProviderError),
    Done,
}

pub type CompletionStream = Pin<Box<dyn Stream<Item = StreamEvent> + Send>>;
```

Provider-specific SSE parsing is implemented per `ApiType`:

1. **Anthropic**: Parse `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop` events. Handle `thinking` blocks as `ReasoningDelta`.
2. **OpenAI Chat**: Parse `chat.completion.chunk` events. Accumulate tool call argument deltas.
3. **OpenAI Responses**: Parse response stream events.
4. **Gemini**: Parse `GenerateContent` stream chunks.

#### Request Assembly

Each `ApiType` has a dedicated request builder that transforms flok's internal message format:

```rust
pub trait RequestBuilder: Send + Sync {
    fn build_request(
        &self,
        config: &ProviderConfig,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Result<reqwest::RequestBuilder>;

    fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<CompletionStream>;
}
```

#### Prompt Cache Optimization (Critical Path)

For Anthropic, cache breakpoints are placed strategically:

```
[System Prompt]          <- cache_control: ephemeral (stable across turns)
[Conversation History]   <- cache_control: ephemeral (stable prefix)
[Latest User Message]    <- no cache (changes each turn)
```

This ensures that repeated turns reuse cached tokens for the system prompt and growing conversation prefix. The savings are massive:
- Cache write: 25% premium on first cache miss
- Cache read: 90% discount on hits
- Net effect: ~10x cheaper after first turn

For OpenAI, caching is automatic (prefix-based). No explicit markers needed, but we ensure message ordering is stable to maximize hit rates.

#### Hanging Stream Detection

A watchdog timer runs alongside each stream. If no SSE event arrives within 30s (configurable), the stream is aborted and the request retried or failed:

```rust
async fn stream_with_timeout(
    stream: CompletionStream,
    timeout: Duration,
) -> CompletionStream {
    // tokio::time::timeout on each .next() call
}
```

### Alternatives Considered

1. **Use the `rig` crate (like Spacebot)**: Rejected. Rig's `CompletionModel` trait requires implementing per-provider models. Direct HTTP is simpler and gives us full control over caching and streaming.
2. **Use the Vercel AI SDK via WASM**: Rejected. Adds complexity and runtime dependency. Native Rust HTTP is faster.
3. **Abstract all providers behind a single trait**: Adopted. `RequestBuilder` trait per `ApiType` is the right level of abstraction -- provider-specific where needed, unified at the stream level.

## Success Criteria

- **SC-001**: Time from user message to first streamed token < 200ms (network time excluded, measuring only local overhead)
- **SC-002**: Prompt cache hit rate > 90% for multi-turn conversations on Anthropic
- **SC-003**: Fallback from rate-limited model to backup completes in < 1s
- **SC-004**: Zero-copy SSE parsing -- stream chunks are parsed without allocating intermediate buffers where possible

## Assumptions

- Provider APIs are stable and follow their documented specifications
- Users have valid API keys with sufficient quota
- Network latency dominates total response time (local overhead is negligible by comparison)

## Open Questions

- ~~Should we bundle a model registry (like opencode's models.dev) or rely purely on config?~~ **Decision: Hardcode a small table.** Ship a built-in registry of current known models (Claude Sonnet 4/Opus 4/Haiku 4.5, GPT-5.4/5.4-mini/5.4-nano, GPT-4.1/4.1-mini for legacy compatibility, Gemini 2.5 Flash/Pro, DeepSeek V3/R1) with context window sizes, pricing, and capability flags. Users can override or add models via `flok.toml`. No external API dependency for the registry.
- ~~Should we support Amazon Bedrock and Azure OpenAI from day one?~~ **Decision: No.** Bedrock and Azure OpenAI are deferred post-v1.0. Focus on direct Anthropic, OpenAI, Google Gemini, and OpenAI-compatible APIs.
- How to handle provider-specific features (e.g., Anthropic's extended thinking) in the unified stream interface?
