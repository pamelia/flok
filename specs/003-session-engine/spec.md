# Feature Specification: Session Engine

**Feature Branch**: `003-session-engine`
**Created**: 2026-03-28
**Status**: Accepted (2026-04-19 — feature shipped; spec retroactively locked to match built reality.)

## User Scenarios & Testing

### User Story 1 - Developer Has a Multi-Turn Conversation (Priority: P0)
**Why this priority**: This is the primary interaction mode.
**Acceptance Scenarios**:
1. **Given** the user types a message and presses Enter, **When** the message is submitted, **Then** the assistant begins streaming a response within 500ms (including network round-trip).
2. **Given** a conversation has 50+ messages, **When** the user scrolls up, **Then** all messages are rendered instantly from the local database (no re-fetching).
3. **Given** the assistant calls a tool, **When** the tool executes, **Then** the tool's state transitions (`pending` → `running` → `completed`/`error`) are reflected in the TUI in real-time.

### User Story 2 - Context Gets Too Large for the Model Window (Priority: P0)
**Why this priority**: Without compaction, long conversations crash or silently truncate.
**Acceptance Scenarios**:
1. **Given** token usage reaches 80% of the model's context window, **When** the compaction threshold is hit, **Then** a background compaction task summarizes older messages while preserving recent context.
2. **Given** compaction runs, **When** it completes, **Then** the user sees no interruption -- the conversation continues seamlessly with a compacted history prefix.
3. **Given** an emergency (95%+ context usage), **When** emergency compaction triggers, **Then** old tool outputs are truncated programmatically (no LLM call) to immediately free space.

### User Story 3 - Developer Spawns a Sub-Agent Task (Priority: P1)
**Why this priority**: Sub-agents are the foundation of multi-agent orchestration.
**Acceptance Scenarios**:
1. **Given** the assistant calls the `task` tool, **When** a sub-agent is spawned, **Then** a child session is created with its own message history, linked to the parent session.
2. **Given** a background sub-agent completes, **When** it returns, **Then** the result is injected back into the parent session as a tool result.
3. **Given** the parent session is cancelled, **When** abort propagates, **Then** all child sessions are also cancelled within 1s.

### Edge Cases
- User sends empty message: reject with validation error
- LLM returns empty response: treat as error, retry once
- Tool execution takes >5 minutes: timeout with error, allow LLM to recover
- Database write fails during message persistence: retry once, then surface error
- Concurrent writes to same session (team agents): SQLite WAL handles this, but we serialize writes per session
- Session exceeds 10,000 messages: warn user, suggest starting new session

## Requirements

### Functional Requirements

- **FR-001**: Sessions MUST be persisted to SQLite with full message history recoverable across restarts.
- **FR-002**: Messages MUST have exactly two roles: `User` and `Assistant`.
- **FR-003**: Messages MUST be composed of typed **Parts**:
  - `TextPart` -- plain text content
  - `ReasoningPart` -- LLM chain-of-thought / extended thinking
  - `ToolCallPart` -- tool invocation with arguments and result
  - `SnapshotPart` -- file state checkpoint (for undo/redo)
  - `CompactionPart` -- compaction summary marker
  - `StepPart` -- LLM step metadata (tokens, cost, model used)
- **FR-004**: Tool calls MUST have a state machine: `Pending` → `Running` → `Completed` | `Error`.
- **FR-005**: Sessions MUST support parent-child relationships for sub-agent tasks.
- **FR-006**: The prompt loop MUST support automatic compaction at configurable thresholds:
  - Background compaction at 80% context usage
  - Aggressive compaction at 85%
  - Emergency truncation at 95% (programmatic, no LLM call)
- **FR-007**: The prompt loop MUST detect "doom loops" -- if the same tool is called 3x with identical arguments, pause and ask for user confirmation.
- **FR-008**: Sessions MUST support message injection from other sessions (for inter-agent communication).
- **FR-009**: All state mutations MUST go through an event sourcing layer for auditability and replay.
- **FR-010**: Token counting MUST be estimated locally (not relying solely on provider response) for pre-flight context window checks.

### Key Entities

```rust
pub struct Session {
    pub id: SessionID,
    pub project_id: ProjectID,
    pub parent_id: Option<SessionID>,  // For sub-agent tasks
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: SessionStatus,         // Active, Compacting, Archived
}

pub enum SessionStatus {
    Active,
    Compacting,
    Archived,
}

pub struct Message {
    pub id: MessageID,
    pub session_id: SessionID,
    pub role: Role,
    pub agent: String,        // Which agent produced this message
    pub model_id: String,     // Which model was used (for assistant messages)
    pub provider_id: String,
    pub created_at: DateTime<Utc>,
}

pub enum Role { User, Assistant }

pub enum Part {
    Text(TextPart),
    Reasoning(ReasoningPart),
    ToolCall(ToolCallPart),
    Snapshot(SnapshotPart),
    Compaction(CompactionPart),
    Step(StepPart),
}

pub struct ToolCallPart {
    pub id: PartID,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub result: Option<String>,
    pub error: Option<String>,
    pub state: ToolCallState,
    pub metadata: serde_json::Value,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

pub enum ToolCallState { Pending, Running, Completed, Error }

pub struct StepPart {
    pub id: PartID,
    pub model_id: String,
    pub usage: Usage,
    pub duration_ms: u64,
    pub finish_reason: FinishReason,
}

pub enum FinishReason { Stop, ToolUse, Length, ContentFilter, Error }
```

## Design

### Overview

The session engine is the orchestrator at the heart of flok. It manages the conversation lifecycle: receiving user input, assembling prompts, streaming LLM responses, executing tools, persisting state, and managing compaction. It is designed for concurrent access by multiple agents operating within the same or related sessions.

### Detailed Design

#### The Prompt Loop

The core loop follows opencode's architecture but is optimized for Rust's async model:

```
User Input → Assemble Prompt → Stream LLM → Process Events → Persist → Loop/Exit
                                    ↓
                              Tool Calls?
                              ├── Yes → Execute Tools → Persist Results → Loop
                              └── No  → Check: Wait for injections? → Wait/Exit
```

```rust
pub async fn run_prompt_loop(
    state: Arc<AppState>,
    session_id: SessionID,
    initial_message: String,
    agent: &AgentConfig,
    cancel: CancellationToken,
) -> Result<LoopOutcome> {
    loop {
        // 1. Assemble system prompt + history + user message
        let prompt = assemble_prompt(state, session_id, agent).await?;

        // 2. Check context window, compact if needed
        if needs_compaction(&prompt, agent.model()) {
            compact(state, session_id, &prompt).await?;
            continue;
        }

        // 3. Stream LLM response
        let stream = state.providers.stream(agent.model(), &prompt).await?;

        // 4. Process stream events
        let outcome = process_stream(state, session_id, stream, &cancel).await?;

        match outcome {
            StreamOutcome::ToolCalls(calls) => {
                execute_tools(state, session_id, calls, &cancel).await?;
                continue; // Loop back for LLM to process tool results
            }
            StreamOutcome::Stop => {
                // Check if we should wait for injected messages (team lead)
                if should_wait_for_messages(state, session_id).await? {
                    match wait_for_injection(state, session_id, &cancel).await? {
                        Some(injected) => continue, // Process injected message
                        None => break Ok(LoopOutcome::Complete),
                    }
                }
                // Auto-send to lead if this is a team member
                auto_send_to_lead(state, session_id).await?;
                break Ok(LoopOutcome::Complete);
            }
            StreamOutcome::Error(e) => break Err(e),
            StreamOutcome::Cancelled => break Ok(LoopOutcome::Cancelled),
        }
    }
}
```

#### Event Sourcing

All state mutations go through events, inspired by opencode's `SyncEvent` system:

```rust
pub enum SessionEvent {
    SessionCreated { session: Session },
    MessageCreated { message: Message },
    PartCreated { message_id: MessageID, part: Part },
    PartUpdated { part_id: PartID, part: Part },
    SessionCompacted { session_id: SessionID, summary: String },
    SessionArchived { session_id: SessionID },
}
```

Events are persisted to an `event` table and projected into materialized read tables (`session`, `message`, `part`). This enables:
- Full audit trail
- Replay for debugging
- Event-driven reactivity (TUI subscribes to events)

#### Compaction Strategy

Three tiers with tool-response-aware compression (see spec-014 for full implementation):

1. **T1 - Tool Response Compression (60%)**: No LLM call. Compress tool responses aggressively -- deduplicate repeated file reads, truncate large outputs, extract only relevant sections. Preserve all model-generated reasoning intact. The critical insight: most context bloat comes from tool responses (file reads, grep output, shell results), not from model generation.
2. **T2 - Structured Summary (80%)**: Spawn a utility-model compaction agent that summarizes older messages into a `CompactionPart` using the Goal/Progress/TODOs/Constraints format. Keep the last ~40K tokens of recent context intact.
3. **T3 - Emergency Truncation (95%)**: No LLM call. Programmatically truncate old tool outputs to their first 200 characters. Drop old reasoning parts entirely. Must complete in < 100ms.

Token estimation for threshold checking uses a fast local estimator (see spec-006).

#### Tiered Memory System (Killer Feature #4)

Flok implements a four-tier memory system to solve the "50 First Dates" problem -- agents forgetting everything between sessions:

- **Hot:** Current session conversation (in-memory). Managed by the session engine.
- **Warm:** Structured compaction summaries stored in SQLite (Goal/Progress/TODOs/Constraints format). Created by T2 compaction.
- **Cold:** Semantic search over past sessions via LanceDB vector embeddings. Enables `--continue` to inject relevant context from previous sessions automatically.
- **Permanent:** Per-agent memory (see spec-008 `agent_memory`). Project knowledge graph: architecture decisions, conventions, "always/never" rules.

Session resume with `--continue` injects warm + cold context automatically:

```rust
pub async fn resume_session(
    state: &AppState,
    session_id: SessionID,
) -> Result<Vec<Message>> {
    // 1. Load warm context: last compaction summary for this session
    let warm = state.db.last_compaction(session_id).await?;

    // 2. Load cold context: semantic search over past sessions
    let cold = state.memory.search_similar(
        &warm.summary,
        state.project.id,
        5,  // top 5 relevant memories
    ).await?;

    // 3. Load permanent: agent memory
    let permanent = state.memory.read_agent_memory(
        state.project.id,
        &agent.name,
    ).await?;

    // Inject as synthetic system context
    assemble_resume_context(warm, cold, permanent)
}
```

#### Message Injection (Inter-Agent Communication)

When one agent sends a message to another (via `send_message` tool):

1. Create a synthetic `User` message in the target session with `injected` metadata
2. Format: `[Message from @agent_name]\n\n<content>`
3. Publish `BusEvent::MessageInjected` to wake the target's prompt loop
4. Target prompt loop picks up the injected message on its next iteration

The injection flow ensures:
- Messages are persisted before notification (no lost messages)
- The target agent sees injected messages as regular user messages
- The `injected` metadata preserves provenance for audit

#### Concurrent Session Access

Multiple agents may write to different sessions concurrently. SQLite WAL mode handles concurrent readers and single-writer semantics. For sessions with high write contention (team lead receiving many injections), we use a per-session write queue:

```rust
pub struct SessionWriteQueue {
    queues: DashMap<SessionID, mpsc::Sender<WriteOp>>,
}
```

Each session gets a dedicated `mpsc` channel. A background task drains the channel and executes writes sequentially. This prevents SQLite BUSY errors while maintaining high throughput.

### Alternatives Considered

1. **Skip event sourcing, write directly**: Rejected. Event sourcing is essential for the TUI's reactive updates and for debugging multi-agent interactions.
2. **Use PostgreSQL for concurrent access**: Rejected. Violates single-binary principle. SQLite WAL + write queue is sufficient for local use.
3. **Store messages as a single JSON blob per session**: Rejected. Per-message rows enable efficient pagination, search, and partial reads.

## Success Criteria

- **SC-001**: Message persistence latency < 1ms (measured from event creation to SQLite write confirmation)
- **SC-002**: Compaction completes in < 10s for a 100-message conversation
- **SC-003**: Session load time (reading 1000 messages from DB) < 50ms
- **SC-004**: Message injection delivery latency < 10ms (from send to target wakeup)

## Assumptions

- Single user per flok instance (no multi-tenancy at the session level)
- Session sizes rarely exceed 1000 messages before the user starts a new one
- Compaction summaries are "good enough" -- perfect recall of compacted content is not required

## Open Questions

- Should we support session branching (fork a conversation at a specific message)?
- Should compaction be configurable per agent? (e.g., never compact a lead agent's session)
- ~~What's the right token estimation algorithm for pre-flight checks? (tiktoken-rs? char-based heuristic?)~~ **Decision: `tiktoken-rs`.** Use `tiktoken-rs` for token estimation. It provides accurate counts for OpenAI models and is a reasonable approximation for Anthropic/Gemini. The dependency cost is acceptable for the accuracy gain over char/4 heuristics, especially for context window management.
