# Spec 015 — Token Compression Engine

**Status:** Accepted
**Priority:** P0 — Critical path for cost/quality
**Depends on:** 003 (Session Engine), 004 (Tool System), 006 (Token Cache)

> **Accepted note (2026-04-19):** MVP (shell output 4-stage pipeline) shipped 2026-04-19.
> Conversation history compression remains Future Work.

## Summary

Three compression layers that reduce token consumption by 60-90%. All built natively
in Rust, running in-process, zero external API dependencies.

```
Layer 1: Shell Output Compression
  Agent runs `git status` → Flok intercepts → strips noise → 70-95% smaller

Layer 2: Conversation History Compression
  Tool_result blocks in API requests → compress before sending → 30-50% savings per turn

Layer 3: Context Window Management (Tiered Compaction)
  Session grows toward limit → prune tool outputs (no LLM) → summarize (LLM) → truncate
```

## Research Findings

Analysis of OpenCode's production compaction, Cursor's published self-summarization
research (March 2026), and Anthropic's prompt caching API reveals:

1. **Most context bloat comes from tool responses** — OpenCode's pruning targets only
   `tool_result` blocks and achieves sufficient savings without LLM summarization in
   most sessions.

2. **Recency-based pruning beats percentage-based triggers** — OpenCode protects the
   last 40K tokens of tool output, regardless of context percentage. Cursor triggers at
   a fixed token count (40K-80K), not a percentage.

3. **Compaction breaks prompt caching** — Restructuring the conversation invalidates
   the entire Anthropic cache prefix. T1 pruning (clearing old tool outputs in-place)
   preserves the prefix. T2 summarization should be a last resort.

4. **Prompt-based summarization loses 50% more information than trained self-summarization**
   (Cursor's CursorBench data). Since we can't train a custom model, our summarization
   prompt must be exceptionally well-designed.

5. **Continuous background pruning is critical** — OpenCode runs pruning every prompt
   loop iteration, not just at compaction thresholds.

## Design Decisions (deviations from token-compression-spec.md)

| Original Spec | Changed To | Reason |
|---|---|---|
| T1 triggers at 60% context | T1 runs every prompt loop iteration, recency-based (protect last 40K tokens of tool output) | Matches production-proven OpenCode approach; avoids premature pruning |
| T3 drops "old reasoning parts entirely" | T3 preserves reasoning parts (truncated to 500 chars) | Reasoning contains learned context; dropping it causes repeat failures |
| No replay-after-compaction | Added: replay last user message after T2 compaction | Critical for UX; prevents lost user intent |
| No protected tool types | Added: configurable protected tools (never pruned) | Some tool outputs (skill results, memory writes) carry critical context |
| No continuous pruning | Added: prune runs at start of every prompt loop | Prevents context from ever reaching T2/T3 thresholds |

---

## Layer 1: Shell Output Compression

### Architecture

```
Command execution in BashTool
       │ raw stdout + stderr
       ▼
┌─────────────────────┐
│  Compression         │  4-stage pipeline
│  Pipeline            │
└─────────┬───────────┘
          │ compressed output
          ▼
   Tool result → agent context
```

### 4-Stage Pipeline

Each stage: `fn compress(input: &str, ctx: &CmdContext) -> String`

**Stage 1 — Filter (lossless)**
- ANSI escape codes (`\x1b[...m`)
- Git hint text (`(use "git ..."`)
- Progress bars, spinners (`\r`, `[===`, percentage patterns)
- Blank line runs (collapse to single blank line)
- Decorative separators (`---`, `===`, `***` full-width lines)
- Build tool boilerplate headers/footers

**Stage 2 — Group (lossy, configurable)**

**Stage 3 — Truncate (lossy, configurable)**
- Head/tail mode: first N + last M lines with `... (X lines omitted)` marker
- Configurable max output size (default: 4096 tokens)

**Stage 4 — Deduplicate (lossless)**
- Identical consecutive lines: `{line} (×N)` format
- Repeated log patterns

### Command-Specific Filters

Priority filters (v1):

| Command | Strategy |
|---|---|
| `cargo test` / `pytest` / `jest` | Summary line + failures only |
| `git status` | Strip hints, compact format |
| `git diff` | Keep hunk headers + changed lines, strip excess context |
| `git log` | Already handled by git itself |
| `cargo build` / `npm run build` | Errors + warnings only |

### Functional Requirements (shipped MVP)

- **FR-001:** `bash` tool output MUST pass through a 4-stage compression pipeline
  (filter, group, truncate, deduplicate) before being embedded in the tool result.
- **FR-002:** Output ≤ `passthrough_threshold_lines` MUST be emitted unchanged
  (no headers, no markers).
- **FR-003:** Grouping MUST collapse exact-match runs of ≥ `group_exact_min` into
  `<line>\n... (× N times)\n`.
- **FR-004:** Truncation MUST preserve `head_lines` at the start and `tail_lines`
  at the end, replacing the middle with
  `... [N lines elided by compression] ...`.
- **FR-005:** A final character budget MUST cap total output at `max_chars` with
  head+tail preservation.
- **FR-006:** When compression is applied, the tool result MUST include a header
  line naming the stages that fired (telemetry).
- **FR-007:** Compression MUST be disableable via `enabled = false` in the
  `[output_compression]` config section.

### Success Criteria (shipped MVP)

- **SC-001:** A 1000-line repetitive bash output compresses to < 10% of original
  line count.
- **SC-002:** A 20-line bash output is emitted byte-for-byte identical to the raw
  output.
- **SC-003:** ANSI escape sequences are stripped from output sent to the LLM.

### Configuration

```toml
[compression.shell]
enabled = true
max_output_tokens = 4096
```

---

## Layer 2: Conversation History Compression *(Future Work)*

Applied to `tool_result` blocks in the message array before sending to the LLM provider.

### Stages

**Stage A — Content Classification**

```rust
pub enum ContentType {
    Json,
    JsonArray,
    CliOutput,
    ErrorResult,   // NEVER compress
    Other,
}
```

**Stage B — Type-Specific Compression**

| Content Type | Compression |
|---|---|
| `Json` | Minify (strip whitespace) |
| `JsonArray` | TOON encoding (header + values) |
| `CliOutput` | Apply Layer 1 pipeline if not already compressed |
| `ErrorResult` | Skip entirely |

**Stage C — Deduplication Cache**

LRU cache keyed by blake3 hash. If the same file was read multiple times,
replace with: `[content identical to previous read of {path} at turn {N}]`

### TOON Encoding

```
JSON array → "N col1,col2,...\nval1,val2,...\nval1,val2,..."
```

Lossless. 40-60% reduction on uniform object arrays.

---

## Layer 3: Context Window Management

### Continuous Pruning (runs every prompt loop iteration)

```rust
fn prune_tool_outputs(messages: &mut [Message], protect_tokens: usize) {
    // Walk backwards through messages
    // Track cumulative tool_result tokens from the end
    // Once we exceed protect_tokens, replace older tool_results with:
    //   "[Old tool result content cleared]"
    // Never prune: error results, protected tool types, user text, assistant text
}
```

- `protect_tokens`: default 40,000 (configurable)
- Protected tools: configurable list, default `["skill"]`
- This is the highest-ROI compaction technique

### T2: Structured Summary (LLM-based)

Triggers when context usage exceeds 80% even after pruning.

1. Find the last user message before the overflow point
2. Send a compaction prompt to the LLM (can use a cheaper model)
3. Replace all messages before that point with the summary
4. Replay the last user message

**Compaction prompt template:**

```
Summarize this conversation for session continuity. Include:
## Goal — What the user originally asked for
## Work Completed — What was done (file changes, fixes, features)
## Current State — Branch, files being modified, test status
## Remaining Tasks — Ordered list of next steps
## Constraints — Things that must NOT be violated
```

### T3: Emergency Truncation

Triggers at 95% context. Keep:
- System prompt
- Last compaction summary (if any)
- Last 3 turns (full fidelity)
- Drop everything else

Reasoning parts from older turns: truncated to first 500 characters, not dropped entirely.

### Safety Invariants

1. NEVER compress error output or stack traces
2. NEVER compress code in the current working set
3. NEVER compress system prompt or user messages
4. NEVER compress content flagged by leak detection
5. ALWAYS preserve the most recent tool outputs (protect window)

---

## Implementation Order

1. **Layer 1, Stage 1 (Filter) + Stage 4 (Dedup)** — highest ROI, simplest
2. **Continuous T1 pruning in session engine** — highest impact for long sessions
3. **Layer 1 command-specific filters** — git, cargo, npm (80% of commands)
4. **Layer 2 JSON minify + dedup cache** — lossless, immediate savings
5. **Layer 2 TOON encoding** — requires JSON array detection
6. **Layer 1, Stage 3 (Truncate)** — configurable max output
7. **Layer 3 T2 structured compaction** — needs compaction prompt + replay
8. **Layer 3 T3 emergency truncation** — safety net
9. **Compression stats tracking + `flok stats` command** — observability
10. **Compaction quality test suite** — verify summaries preserve key facts

## Open Questions

- Should we support a configurable compaction model (cheaper model for summarization)?
- Should T1 protect window be configurable per-tool (e.g., more protection for `read` results)?
- Should we add a `--no-compress` flag for debugging?

## Dependencies

- `regex` — pattern matching for filters
- `blake3` — content hashing for dedup cache
- `serde_json` — JSON classification and minification
- `tiktoken-rs` — token counting (already in workspace)
