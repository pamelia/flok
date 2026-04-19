# Feature Specification: Intelligent Model Routing

**Feature Branch**: `012-intelligent-routing`
**Created**: 2026-03-28
**Status**: Accepted

Accepted 2026-04-19: Tier 1 scope locked — provider-level fallback chains + runtime error-driven failover. Tier 2+ explicitly deferred.

## User Scenarios & Testing

### User Story 1 - Automatic Task-Based Model Selection (Priority: P0)
**Why this priority**: Intelligent routing is the primary cost optimization lever -- it can cut costs 3-5x without sacrificing quality.
**Acceptance Scenarios**:
1. **Given** the agent is performing file exploration (read, glob, grep), **When** the routing system evaluates the task, **Then** it routes to the `explore` tier model (cheap/fast, e.g., DeepSeek V3).
2. **Given** the agent is generating code for a standard feature, **When** the routing system evaluates, **Then** it routes to the `build` tier model (balanced, e.g., Claude Sonnet 4.6).
3. **Given** the agent is making an architecture decision or debugging a complex issue, **When** the routing system evaluates, **Then** it routes to the `plan` tier model (deep reasoning, e.g., Claude Opus 4.6).
4. **Given** the user explicitly selects a model, **When** the explicit override is active, **Then** routing is bypassed and the selected model is used for all requests.

### User Story 2 - Automatic Fallback on Rate Limiting (Priority: P0)
**Why this priority**: Rate limiting is the most common disruption in agentic workflows.
**Acceptance Scenarios**:
1. **Given** the primary model returns a 429 rate limit error, **When** fallback models are configured, **Then** flok automatically retries with the next fallback within 500ms.
2. **Given** a model has been rate-limited, **When** the cooldown period expires (default 60s), **Then** the primary model is tried again on the next request.
3. **Given** all models in a fallback chain are rate-limited, **When** the next request comes, **Then** flok waits for the shortest cooldown and retries, displaying a "waiting for rate limit" indicator in the TUI.

### User Story 3 - Cost-Aware Routing for Agent Teams (Priority: P1)
**Why this priority**: Agent teams can have mixed models -- expensive reasoning for the lead, cheap fast models for workers.
**Acceptance Scenarios**:
1. **Given** a team lead is orchestrating and a worker is doing file search, **When** routing evaluates, **Then** the lead uses the `plan` tier and the worker uses the `explore` tier.
2. **Given** a per-session cost budget is set, **When** the budget threshold is reached (e.g., 80%), **Then** routing automatically downgrades to cheaper models and warns the user.
3. **Given** the session cost report, **When** displayed, **Then** it shows per-model-tier breakdown (how much was spent on explore vs build vs plan).

### User Story 4 - Developer Configures Routing Tiers (Priority: P0)
**Why this priority**: Users must be able to control which models are used for which tasks.
**Acceptance Scenarios**:
1. **Given** the user configures routing tiers in `flok.toml`, **When** flok starts, **Then** the tiers are applied to all automatic routing decisions.
2. **Given** no routing config exists, **When** flok starts, **Then** sensible defaults are used based on available providers and models.
3. **Given** routing config is changed while flok is running, **When** hot-reload triggers, **Then** new routing rules are applied to the next request.

### Edge Cases
- Only one model available: route everything to it, no complexity scoring needed
- Model not available in configured tier: fall through to next tier, then default
- Complexity scoring is wrong (routes a hard task to a cheap model): LLM produces poor output, agent retries with tool errors, and doom loop detection kicks in -- could add auto-upgrade on repeated failures
- All providers down: queue requests, retry with exponential backoff, surface clear error after timeout
- Very long prompt (>100K tokens): only models with sufficient context window are eligible
- Model pricing not configured: use zero cost (don't break routing, just skip cost tracking)

## Tier 1 Scope (This Sprint)

**Goal**: When a provider errors on a retriable HTTP status, automatically fail over to a configured fallback provider without user intervention. Deliver provider-level fallback chains (not per-agent).

### Config additions to `flok.toml`

```toml
# Global fallback policy (new top-level section)
[runtime_fallback]
enabled = true
retry_on_errors = [429, 500, 502, 503, 529]  # Anthropic uses 529 for overloaded
max_attempts = 3           # total retry budget across chain
cooldown_seconds = 120     # per-provider cooldown after failure
notify_on_fallback = true  # emit UI bus event on each failover

# Per-provider fallback chain (new field on existing ProviderConfig)
[provider.anthropic]
api_key = "..."
default_model = "opus-4.7"
fallback = ["openai", "minimax"]  # NEW — ordered list of provider names

[provider.openai]
api_key = "..."
default_model = "gpt-5.4"
fallback = ["anthropic"]
```

### Runtime behavior

- `ProviderRegistry::stream_with_fallback(provider_name, request)` wraps `provider.stream()`:
  1. Attempt `provider.stream(request)`
  2. On error OR HTTP status in `retry_on_errors`:
     - Mark provider in cooldown (in-memory `HashMap<String, Instant>`)
     - Pop next provider from chain (skip any in cooldown)
     - If next is None OR `max_attempts` exceeded → surface error with list of tried providers
     - Else: construct new request using next provider's `default_model`, emit `BusEvent::ProviderFallback`, retry
- When a provider is in cooldown, `ProviderRegistry::get()` skips it in lookups during the retry chain (not for direct user requests)
- Cooldown is checked at each fallback attempt; expires naturally after `cooldown_seconds`

### Scope of integration

- Main session engine `prompt_loop` uses fallback for primary provider streams
- `task` tool's `stream_with_retry` uses fallback for sub-agent streams
- BOTH surface `ProviderFallback` events to the bus for UI toast

## Requirements

### Functional Requirements

- **FR-001**: Flok MUST support three routing tiers with configurable model assignments:
  ```toml
  [routing]
  explore = "deepseek/deepseek-chat"          # Cheap, fast: file reads, grep, exploration
  build   = "anthropic/claude-sonnet-4.6"     # Balanced: code gen, refactoring, standard tasks
  plan    = "anthropic/claude-opus-4.6"       # Deep reasoning: architecture, planning, debugging
  utility = "anthropic/claude-haiku-4.5"      # Internal: title gen, compaction, summaries
  ```

- **FR-002**: Flok MUST support per-agent model overrides that take precedence over routing tiers:
  ```toml
  # In agent definition frontmatter:
  model: anthropic/claude-haiku-4.5   # This agent always uses Haiku
  ```

- **FR-003**: Flok MUST support fallback chains per tier:
  ```toml
  [routing.fallbacks]
  explore = ["openai/gpt-5.4-mini", "google/gemini-2.5-flash"]
  build = ["openai/gpt-5.4", "google/gemini-2.5-pro"]
  plan = ["openai/o3", "google/gemini-2.5-pro"]
  ```

- **FR-004**: Flok MUST implement a prompt complexity scorer that classifies requests into tiers:
  - **Explore**: Low complexity. The prompt is a continuation of tool-result processing (file reads, grep output). No significant reasoning required.
  - **Build**: Medium complexity. The prompt involves code generation, editing, or refactoring with clear requirements.
  - **Plan**: High complexity. The prompt involves multi-step reasoning, architecture decisions, debugging with multiple hypotheses, or planning.
  - Classification is based on heuristics (not another LLM call):
    - Ratio of tool results to new user content
    - Presence of planning keywords ("design", "architect", "debug", "investigate", "analyze", "compare", "trade-off")
    - Number of files referenced in context
    - Whether the last assistant message requested more information vs proposing a solution
    - Agent type (explore agents always get explore tier)

- **FR-005**: Flok MUST implement retry with exponential backoff per model:
  - Max 3 retries per model
  - Base delay: 500ms, doubling each retry (500ms, 1s, 2s)
  - On exhausting retries for a model, move to the next fallback
  - Max 3 fallback attempts across the chain

- **FR-006**: Flok MUST track rate-limited models in a `DashMap<String, Instant>` and skip them until cooldown expires (configurable, default 60s).

- **FR-007**: Flok MUST support a per-session cost budget:
  ```toml
  [session]
  cost_budget = 5.00  # USD per session
  cost_warning = 0.80 # Warn at 80% of budget
  ```
  - At warning threshold: log warning, display TUI notification
  - At budget limit: downgrade to cheapest available model, prompt user to continue or stop

- **FR-008**: Flok MUST support context window-aware routing: models with insufficient context window for the current prompt are excluded from tier selection.

- **FR-009**: Routing decisions MUST be logged (at debug level) for transparency:
  ```
  [routing] task=build complexity=medium model=claude-sonnet-4.6 reason="code generation, 3 files referenced"
  ```

- **FR-010**: Flok MUST support an explicit model override that bypasses routing entirely:
  - Per-session: user selects model via TUI model picker
  - Per-message: `/model claude-opus-4.6` slash command
  - The override persists until changed or session ends
- **FR-012**: Automatic routing MUST be opt-in via `routing.auto = true` (default `false`). When `routing.auto = false`, the user's explicitly selected model (or the tier's configured model) is used for all requests without complexity scoring. This is the v1.0 starting point; the `ComplexityScorer` is layered on top.
- **FR-013**: Flok MUST support auto-upgrade on failure: if a lower-tier model produces repeated failures (3+ consecutive tool errors or doom loop detection), automatically retry the request with the next tier up. Configurable via `routing.auto_upgrade_on_failure = true` (default `true`).
- **FR-014**: Routing decisions MUST be visible in the TUI status bar: show the current model name and routing tier. Full routing reason is logged at debug level.

- **FR-015**: Config MUST support `[runtime_fallback]` section with `enabled`, `retry_on_errors`, `max_attempts`, `cooldown_seconds`, and `notify_on_fallback` fields.
- **FR-016**: `ProviderConfig` MUST support an optional `fallback` field — ordered list of provider names.
- **FR-017**: When a provider stream fails with a status in `retry_on_errors`, the runtime MUST attempt the next provider in the chain (skipping cooldowns) up to `max_attempts` total attempts.
- **FR-018**: Each fallback attempt MUST emit `BusEvent::ProviderFallback { session_id, from, to, reason }` when `notify_on_fallback` is true.
- **FR-019**: Cooldown tracking MUST be in-memory (not persisted); providers in cooldown MUST be skipped during fallback selection.
- **FR-020**: When all providers in a chain have failed or are in cooldown, the runtime MUST surface a structured error listing each attempted provider and its failure reason.
- **FR-021**: The fallback logic MUST work for both lead-session streams and sub-agent (`task` tool) streams.

- **FR-011**: Default routing MUST be inferred from available providers when no explicit config exists:
  - If only Anthropic is available: Haiku → explore, Sonnet → build, Opus → plan
  - If only OpenAI is available: GPT-5.4 mini → explore, GPT-5.4 → build, o3 → plan
  - If multiple providers: prefer Anthropic for build (best tool use), any for explore

### Key Entities

```rust
pub enum RoutingTier {
    Explore,   // Cheap, fast
    Build,     // Balanced
    Plan,      // Deep reasoning
    Utility,   // Internal (compaction, titles)
}

pub struct RoutingConfig {
    pub tiers: HashMap<RoutingTier, String>,         // Tier -> model ID
    pub fallbacks: HashMap<RoutingTier, Vec<String>>, // Tier -> fallback chain
    pub cost_budget: Option<f64>,
    pub cost_warning: f64,                           // Fraction (0.0 - 1.0)
}

pub struct RoutingDecision {
    pub tier: RoutingTier,
    pub model_id: String,
    pub reason: String,
    pub was_fallback: bool,
}

pub struct ComplexityScore {
    pub tier: RoutingTier,
    pub confidence: f64,      // 0.0 - 1.0
    pub factors: Vec<String>, // Human-readable reasons
}

pub struct ModelRouter {
    config: ArcSwap<RoutingConfig>,
    rate_limited: DashMap<String, Instant>,
    session_cost: AtomicU64,   // Fixed-point cost in microdollars
    cooldown: Duration,
}
```

## Design

### Overview

The intelligent model routing system automatically selects the best model for each request based on task complexity, cost constraints, and model availability. It sits between the session engine and the provider system, intercepting each completion request and routing it to the appropriate model tier. The design prioritizes: (1) zero-latency routing decisions (heuristic, not LLM-based), (2) seamless fallback on failures, and (3) cost transparency.

### Detailed Design

#### Complexity Scoring Algorithm

The complexity scorer uses cheap heuristics (no LLM call) to classify each prompt:

```rust
impl ComplexityScorer {
    pub fn score(
        &self,
        messages: &[Message],
        agent: &AgentConfig,
        last_tool_calls: &[ToolCallPart],
    ) -> ComplexityScore {
        // Rule 1: Agent type override
        if agent.mode == AgentMode::Internal {
            return ComplexityScore::utility("internal agent");
        }
        if agent.name == "explore" {
            return ComplexityScore::explore("explore agent");
        }

        // Rule 2: If last turn was all tool results (file reads, grep),
        // and the LLM is just processing them → explore
        let last_user = messages.last_user();
        let tool_result_ratio = count_tool_results(last_user) as f64
            / last_user.parts.len() as f64;
        if tool_result_ratio > 0.8 {
            return ComplexityScore::explore("processing tool results");
        }

        // Rule 3: Planning keywords in user message
        let user_text = last_user.text_content();
        let planning_keywords = [
            "design", "architect", "debug", "investigate", "analyze",
            "compare", "trade-off", "plan", "strategy", "refactor",
            "why", "root cause", "complex",
        ];
        let keyword_hits = planning_keywords.iter()
            .filter(|k| user_text.to_lowercase().contains(*k))
            .count();
        if keyword_hits >= 2 {
            return ComplexityScore::plan(
                format!("{} planning keywords detected", keyword_hits)
            );
        }

        // Rule 4: Multiple files in context + modification intent
        let files_referenced = count_files_in_context(messages);
        if files_referenced > 5 && has_modification_intent(last_user) {
            return ComplexityScore::plan("multi-file modification");
        }

        // Rule 5: Default to build tier
        ComplexityScore::build("standard task")
    }
}
```

#### Model Selection Pipeline

```
Request → Complexity Scorer → Tier Selection
                                    ↓
                             Check explicit override? → Yes → Use override model
                                    ↓ No
                             Check agent model override? → Yes → Use agent model
                                    ↓ No
                             Get tier model from config
                                    ↓
                             Is model rate-limited? → Yes → Try fallback chain
                                    ↓ No
                             Is context window sufficient? → No → Try fallback
                                    ↓ Yes
                             Route to model
                                    ↓
                             On 429/error → Mark rate-limited → Try next fallback
```

```rust
impl ModelRouter {
    pub async fn route(
        &self,
        messages: &[Message],
        agent: &AgentConfig,
        explicit_override: Option<&str>,
        estimated_tokens: u64,
    ) -> Result<RoutingDecision> {
        // 1. Explicit override takes priority
        if let Some(model) = explicit_override {
            return Ok(RoutingDecision::explicit(model));
        }

        // 2. Agent model override
        if let Some(model) = &agent.model {
            return Ok(RoutingDecision::agent_override(model));
        }

        // 3. Complexity scoring
        let score = self.scorer.score(messages, agent, &[]);
        let config = self.config.load();

        // 4. Get tier model + fallbacks
        let tier_model = config.tiers.get(&score.tier)
            .ok_or_else(|| anyhow!("No model configured for tier {:?}", score.tier))?;

        let fallbacks = config.fallbacks.get(&score.tier)
            .cloned()
            .unwrap_or_default();

        let candidates = std::iter::once(tier_model.clone())
            .chain(fallbacks.into_iter())
            .collect::<Vec<_>>();

        // 5. Find first available model
        for (i, model_id) in candidates.iter().enumerate() {
            // Skip rate-limited models
            if let Some(limited_at) = self.rate_limited.get(model_id) {
                if limited_at.elapsed() < self.cooldown {
                    continue;
                }
                self.rate_limited.remove(model_id);
            }

            // Skip models with insufficient context window
            if let Some(model_config) = self.get_model_config(model_id) {
                if model_config.context_window < estimated_tokens as usize {
                    continue;
                }
            }

            return Ok(RoutingDecision {
                tier: score.tier,
                model_id: model_id.clone(),
                reason: score.factors.join(", "),
                was_fallback: i > 0,
            });
        }

        Err(anyhow!("All models in tier {:?} are unavailable", score.tier))
    }

    pub fn mark_rate_limited(&self, model_id: &str) {
        self.rate_limited.insert(model_id.to_string(), Instant::now());
    }
}
```

#### Cost Budget Enforcement

```rust
impl ModelRouter {
    pub fn record_cost(&self, cost_usd: f64) {
        let microdollars = (cost_usd * 1_000_000.0) as u64;
        self.session_cost.fetch_add(microdollars, Ordering::Relaxed);

        let config = self.config.load();
        if let Some(budget) = config.cost_budget {
            let current = self.session_cost.load(Ordering::Relaxed) as f64 / 1_000_000.0;
            let ratio = current / budget;

            if ratio >= 1.0 {
                tracing::warn!("Session cost budget exceeded: ${:.4} / ${:.2}", current, budget);
                // Downgrade routing to cheapest available model
                // Emit bus event for TUI notification
            } else if ratio >= config.cost_warning {
                tracing::info!("Session cost warning: ${:.4} / ${:.2} ({:.0}%)", current, budget, ratio * 100.0);
            }
        }
    }
}
```

#### Default Tier Inference

When no routing config is provided, flok infers tiers from available providers:

```rust
fn infer_default_routing(providers: &ProviderRegistry) -> RoutingConfig {
    let models = providers.all_models();

    // Categorize available models
    let mut cheap = Vec::new();
    let mut balanced = Vec::new();
    let mut reasoning = Vec::new();

    for model in &models {
        match categorize_model(&model.id) {
            ModelCategory::Cheap => cheap.push(model),
            ModelCategory::Balanced => balanced.push(model),
            ModelCategory::Reasoning => reasoning.push(model),
        }
    }

    // Build tiers from best available
    RoutingConfig {
        tiers: hashmap! {
            RoutingTier::Explore => cheap.first().or(balanced.first()).map(|m| m.id.clone()),
            RoutingTier::Build => balanced.first().or(reasoning.first()).map(|m| m.id.clone()),
            RoutingTier::Plan => reasoning.first().or(balanced.first()).map(|m| m.id.clone()),
            RoutingTier::Utility => cheap.first().map(|m| m.id.clone()),
        }.into_iter().flatten().collect(),
        ..Default::default()
    }
}

fn categorize_model(model_id: &str) -> ModelCategory {
    match model_id {
        id if id.contains("haiku") || id.contains("mini") || id.contains("flash") => ModelCategory::Cheap,
        id if id.contains("opus") || id.contains("o3") || id.contains("o1") => ModelCategory::Reasoning,
        _ => ModelCategory::Balanced,
    }
}
```

### Alternatives Considered

1. **LLM-based complexity scoring**: Rejected. Using an LLM to classify prompt complexity adds latency and cost. Heuristics are fast (< 1ms) and good enough for 80%+ of cases.
2. **User picks model per message**: Supported as override, but not the default. The routing system should make good decisions automatically.
3. **Token-count-based routing (cheap for short, expensive for long)**: Rejected. Length doesn't correlate with complexity. A short debugging question may need deep reasoning, while a long file read is simple.
4. **Learning-based routing (train on past sessions)**: Deferred. Interesting for post-1.0, but adds complexity. Static heuristics are a better starting point.
5. **Per-tool routing (different model for each tool call)**: Rejected. Too granular. The three-tier system is sufficient and simpler.

## Future Work (Tier 2+)

- **Tier 2**: Per-agent fallback chains (`[agents.oracle] model = "gpt-5.4", fallback = [...]`). Currently Tier 1 is provider-level; per-agent is richer but requires config schema for `[agents.X]` which doesn't yet exist in flok.
- **Tier 2**: Per-agent `prompt_append` for custom system-prompt extensions via config.
- **Tier 3**: Categories (visual-engineering, ultrabrain, quick, etc.) as a delegation abstraction above `subagent_type`.
- **Tier 3**: Model variants (`xhigh`/`high`/`medium`/`low`) mapping to extended-thinking or reasoning_effort.
- **Tier 3**: Per-model concurrency limits (beyond per-provider).
- **Tier 3**: Content-complexity-based routing (cheap model for simple tasks, expensive for hard).
- **Out-of-scope always**: Persistent cooldown state across restarts (ephemeral is fine).

## Success Criteria

- **SC-001**: Routing decision latency < 1ms (no LLM call, pure heuristics)
- **SC-002**: Cost reduction of 3-5x compared to using the most expensive model for everything
- **SC-003**: Fallback from rate-limited model to backup completes in < 500ms
- **SC-004**: Complexity scoring accuracy > 80% (manual evaluation on test corpus)
- **SC-005**: Zero routing-related conversation quality degradation for standard tasks

- **SC-006**: Given a configured `fallback = ["openai"]` on provider.anthropic, when Anthropic returns HTTP 529, the request completes successfully via OpenAI within 10 seconds (single retry latency).
- **SC-007**: When all providers in a chain fail, the user sees a clear error identifying which providers were tried and why each failed.
- **SC-008**: A provider in cooldown for 120s is not retried within that window even if another failure occurs.
- **SC-009**: `BusEvent::ProviderFallback` is emitted exactly once per successful failover.

## Assumptions

- Three routing tiers (explore/build/plan) are sufficient for most use cases
- Heuristic complexity scoring is accurate enough (not perfect, but good)
- Users want cost optimization by default (not forced to configure it)
- Rate limit cooldowns are predictable (60s is a reasonable default)

## Open Questions

- ~~Should we support auto-upgrade on failure (if cheap model produces poor output, retry with better model)?~~ **Decision: Yes.** If a cheap model produces poor output (repeated tool errors, doom loop detection), automatically retry with the next tier up. Add an `auto_upgrade_on_failure` flag (default `true`).
- ~~Should routing decisions be visible in the TUI (which model was selected and why)?~~ **Decision: Yes.** Show the selected model and routing tier in the TUI status bar. Log the full routing reason at debug level. The user should always know which model is being used.
- ~~Should we add a `routing.auto = false` config to disable automatic routing entirely?~~ **Decision: Yes. Start here.** The v1.0 initial implementation should ship with `routing.auto = false` as the default — users explicitly pick a model (or configure tiers manually). Automatic complexity-based routing is built on top of this foundation and enabled via `routing.auto = true`. This means the `ModelRouter` and tier config are implemented first as explicit/manual routing, and the `ComplexityScorer` heuristics are layered in as a follow-up.
- ~~Should cost budget be per-session, per-day, or configurable?~~ **Decision: Per-session for Tier 1.** Per-day/configurable deferred to Tier 2.
- ~~How should routing interact with prompt caching? (switching models invalidates cache)~~ **Decision: Switching models invalidates the cache.** This is an acceptable trade-off for reliability in Tier 1.
