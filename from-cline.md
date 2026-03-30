# Cline Features Worth Implementing in Flok

After a thorough study of both codebases, here are the most impactful features from Cline that Flok currently lacks, organized by priority:

## 1. Checkpoint System (Shadow Git)
**Cline**: Maintains an independent shadow Git repository that snapshots the workspace at each tool execution step. Users can diff between checkpoints and restore to any previous state.
**Flok**: No checkpoint/undo mechanism exists. If the agent makes a bad edit, there's no easy rollback.
**Value**: Very high. This is a safety net that makes users much more comfortable letting the agent run autonomously.

## 2. Loop Detection
**Cline**: Detects when the LLM makes identical tool calls in sequence. Soft warning at 3 repeats, hard escalation at 5. Uses canonical JSON signatures (excluding metadata fields).
**Flok**: Has basic doom loop protection (3 identical calls triggers a permission prompt), but no soft warning injection into conversation context to nudge the LLM to change approach before it becomes a hard stop.
**Value**: Medium. Adding a soft warning message injected into the conversation at repeat 2-3 would help models self-correct.

## 3. Apply Patch / Multi-File Diff Tool
**Cline**: Has a `apply_patch` tool using a V4A patch format that supports creating, deleting, updating, and moving multiple files in a single tool call with context-based matching.
**Flok**: Only has single-file `edit` (exact string replace) and `write` (full overwrite). No multi-file atomic operation.
**Value**: High. Multi-file patches reduce tool call round-trips significantly for refactoring tasks. This is especially important for reducing latency and token usage.

## 4. Duplicate File Read Detection
**Cline**: Tracks files read across the conversation. After 3 reads of the same unchanged file, returns a `[DUPLICATE READ]` warning instead of re-reading. Also replaces earlier reads with notices during context compaction.
**Flok**: Has blake3-based dedup for identical tool results, which is good, but doesn't specifically warn the LLM about re-reading files it already has in context.
**Value**: Medium. The LLM often re-reads files unnecessarily, wasting tokens. A warning helps it realize the content is already in context.

## 5. Conditional Rules (YAML Frontmatter on Rules/Instructions)
**Cline**: Rules can have YAML frontmatter with a `paths` field containing glob patterns. Rules only activate when working with matching files.
**Flok**: Has `AGENTS.md` injection but it's always-on with no conditional activation.
**Value**: Medium. Useful for large projects where different conventions apply to different parts of the codebase (e.g., different rules for frontend vs. backend).

## 6. Hooks System (Pre/Post Tool Execution)
**Cline**: Extensible hook system where users can define scripts that run before/after any tool execution. PreToolUse hooks can cancel operations or inject context.
**Flok**: No hook system.
**Value**: Medium. Enables guardrails like "run linter before accepting any file write" or "block writes to production config files."

## 7. Browser Automation
**Cline**: Full Puppeteer-based browser session with click, type, scroll, screenshot actions. The LLM can interact with web pages and see screenshots.
**Flok**: Only has `webfetch` (HTTP content fetching, no interaction).
**Value**: Medium-low for a TUI agent (more natural in a VS Code extension), but useful for testing web apps.

## 8. Focus Chain / Task Progress Tracking
**Cline**: A persistent markdown checklist file that the LLM maintains alongside its work. Survives context compaction. The LLM is periodically reminded about it.
**Flok**: Has `todowrite` tool which is similar but doesn't persist to a file or survive context compaction.
**Value**: Medium. The key innovation is that the checklist persists independently of context, so even after compression the LLM remembers what it was working on.

## 9. Plan/Act Mode with Separate Models
**Cline**: Can use a different (cheaper) model for planning and a more capable model for execution.
**Flok**: Has plan/build mode toggle but uses the same model for both.
**Value**: Low-medium. Cost optimization for users who want to use a cheap model for exploration.

## 10. User Edit Detection
**Cline**: When the user modifies the agent's proposed file changes (in VS Code diff view), the system detects the delta and reports the user's edits back to the LLM.
**Flok**: No equivalent (TUI can't show diff views).
**Value**: Low for TUI, but the concept of detecting external file modifications and alerting the LLM is valuable.

## 11. Context Window Summarization (LLM-Driven)
**Cline**: When context is nearly full, sends an explicit instruction to the LLM to produce a comprehensive summary covering all key aspects. This summary replaces the conversation history.
**Flok**: Has T3 emergency truncation (keeps last 6 messages) but no LLM-driven summarization that preserves key context intelligently.
**Value**: High. LLM-driven summarization preserves much more useful context than blind truncation. This directly impacts task success rate for long sessions.

---

## Top 3 Recommendations

1. **LLM-Driven Context Summarization** - Flok's T3 emergency truncation loses critical context. Having the LLM summarize before truncating would dramatically improve long-session success rates.

2. **Apply Patch (Multi-File Diff Tool)** - Reduces round-trips for refactoring tasks. The V4A format or something similar would let the agent edit multiple files atomically.

3. **Checkpoint System** - A shadow git repo for undo/restore gives users confidence to let the agent run with more autonomy, which is the whole point of an AI coding agent.
