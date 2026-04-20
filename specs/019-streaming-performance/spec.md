# Spec 019 — Streaming Performance

**Status**: Accepted (2026-04-20 — MVP fixes shipped in same sprint as spec authoring.)

## Problem

Streaming responses in the flok TUI currently suffer from significant performance degradation as the conversation grows. The investigation identified that the TUI triggers a full render cycle for every incoming text delta, which can occur at rates of 10-100Hz during fast LLM output. Each render cycle involves a full markdown re-parse of the entire message history, which is an O(N) operation where N is the total length of the transcript.

Furthermore, the render cache is effectively bypassed during streaming because the cache key includes the streaming text itself, leading to a 0% hit rate for the active item. Redundant history walks for calculating visible lines and transcript height further compound the issue, resulting in high CPU usage and a sluggish user interface that struggles to keep pace with the incoming stream.

## Non-Goals
- Incremental markdown parsing (explicit Future Work)
- Streaming text over the wire faster (network is not the bottleneck)
- Changes to provider-level streaming behavior

## Tier 1 Scope (This Sprint)

### Delta coalescing

When a `BusEvent::TextDelta` arrives, do NOT render immediately. Instead, accumulate incoming deltas in a 50ms `tokio::select!` window, then render once. Non-coalescible events (mouse, key, Quit, ToolResult) break the window and force immediate render.

Config-free — 50ms is hardcoded (can be tuned later). Applies only to the TUI event loop in `crates/flok-tui/src/app.rs`.

### Revision-based active-item cache

The active streaming item gains a `revision: u64` counter that increments on every delta append. The render cache in `history/render.rs` separates:
- **Static items** keyed by `(content_fingerprint, width)` — unchanged
- **Active item** keyed by `(item_id, revision, width)` — NEW

During coalesced rendering, revision increments exactly once per frame (not per delta), so the active cache hits whenever width is stable.

### Single visible_lines pass per frame

`chat_view.rs` gets a new method `visible_lines_and_rows(...) -> (Vec<Line<'static>>, Vec<String>)` that returns both the styled render AND the plain-text rows for selection tracking in a single history walk. `app.rs::render()` calls it once and passes both to the downstream consumers instead of invoking `render()` + `visible_rows()` separately.

### transcript_height memoization

Cache `transcript_height` result by `(history_len, active_revision, width)`. Invalidate on history append or active item completion. Prevents O(N) walk of full history on every frame.

## Functional Requirements

FR-001: The TUI event loop MUST accumulate `BusEvent::TextDelta` events in a 50ms coalescing window before triggering `render()`. Non-delta events MUST force immediate render.

FR-002: The active streaming item MUST carry a monotonically increasing `revision: u64` that increments on every `ingest_assistant_delta()` call.

FR-003: The render cache in `history/render.rs` MUST use `(item_id, revision, width)` as the key for the active item, and `(content_fingerprint, width)` for static items.

FR-004: `ChatView` MUST provide a `visible_lines_and_rows` method that returns styled lines and plain text rows in a single history walk. `App::render` MUST use this single-pass API instead of calling `visible_lines` + `visible_rows` separately.

FR-005: `transcript_height` MUST be memoized by `(history_len, active_revision, width)` and invalidated only on history growth or active item completion.

FR-006: Mouse events and key events MUST bypass the coalescing window to preserve selection responsiveness.

FR-007: The coalescing window MUST be 50ms as a hardcoded constant. Config-tunable values are Future Work.

## Success Criteria

SC-001: A 10KB streaming response completes rendering with < 100ms of cumulative CPU time spent in markdown parsing (down from ~500ms baseline).

SC-002: Render frame rate during streaming is capped at ~20 FPS (50ms window); no wasted renders.

SC-003: Active-item cache hit rate during streaming is > 80% (up from 0%). (Measure: a debug counter in `history/render.rs` tracks hits vs misses over a 10KB stream.)

SC-004: End-to-end "time to first visible character" is under 50ms from the first SSE chunk arrival. Before this sprint, the value is dominated by the synchronous markdown parse; after, it's bounded by the coalescing window's race against the first event.

SC-005: Mouse-drag text selection during an active stream does not visibly lag (mouse event forces immediate render, bypassing the 50ms window).

## Future Work (deferred)

- **Incremental markdown parsing**: parse only the new suffix on each render; merge into a cached AST. Expected: 2-3× additional speedup. Deferred because it's complex and the MVP already achieves the headline goal.
- **Streaming-only tick rate**: dynamically adjust the coalescing window based on delta arrival rate. Deferred because 50ms is already within human perception threshold.
- **Tree-sitter-assisted code block rendering**: spec 014 adjacency — code blocks in streaming markdown could use tree-sitter for faster highlighting. Deferred to spec 014.
- **String allocation reduction in markdown renderer**: microopt after macro wins.

## Testing

- Unit test: coalescing drain logic given a stream of 10 deltas within 50ms → 1 render triggered.
- Unit test: revision-based cache key changes on append, stable across repeated same-revision queries.
- Integration test: simulate a streaming response of 100 deltas, measure render count and confirm ≤ 20 per 1000ms.

## Reference

Pattern: https://github.com/sigoden/aichat/blob/HEAD/src/render/stream.rs#L165-L190 — `gather_events` with 50ms `tokio::select!` window, exactly the pattern flok ports.
