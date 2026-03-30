# Feature Specification: Agent Teams

**Feature Branch**: `005-agent-teams`
**Created**: 2026-03-28
**Status**: Draft

## User Scenarios & Testing

### User Story 1 - LLM Orchestrates a Multi-Agent Spec Review (Priority: P0)
**Why this priority**: Multi-agent orchestration is flok's key differentiator. Built-in, not bolted-on.
**Acceptance Scenarios**:
1. **Given** the lead agent calls `team_create("spec-review-auth")`, **When** the team is created, **Then** a team record is persisted with the calling session registered as "lead" member.
2. **Given** the lead spawns 5 background agents via `task(background: true, team_id: "...")`, **When** all agents start, **Then** they run concurrently (limited by `max_agents` config), each in their own session.
3. **Given** agents complete their work and call `send_message(recipient: "lead", ...)`, **When** the lead receives messages, **Then** it processes them in order and synthesizes a final report.
4. **Given** all agents have completed, **When** the lead calls `team_delete`, **Then** the team is marked disbanded, all remaining active members are cancelled, and pending tasks are failed.

### User Story 2 - Agent Team Member Communicates with Lead (Priority: P0)
**Why this priority**: Without inter-agent messaging, teams can't coordinate.
**Acceptance Scenarios**:
1. **Given** a team member calls `send_message(team_id, recipient: "lead", content: "findings...")`, **When** the message is sent, **Then** a synthetic user message is injected into the lead's session within 10ms.
2. **Given** the lead calls `send_message(team_id, recipient: "clarity-reviewer", content: "challenge...")`, **When** the message is sent, **Then** the clarity-reviewer's session receives the challenge as an injected message.
3. **Given** a team member finishes naturally (LLM stop), **When** auto-send triggers, **Then** the member's last assistant response is automatically forwarded to the lead.

### User Story 3 - Team Lead Waits for Background Agents (Priority: P0)
**Why this priority**: Without wait-for-agents, the lead exits before agents finish.
**Acceptance Scenarios**:
1. **Given** the lead's LLM returns "stop" but active team members exist, **When** the prompt loop checks, **Then** it blocks and waits for injected messages or member status changes.
2. **Given** a team member crashes, **When** the failure is detected, **Then** the lead receives an `[AGENT FAILURE]` notification with the error message.
3. **Given** all team members have completed or failed, **When** the lead re-evaluates, **Then** it exits the wait loop and continues processing.

### User Story 4 - Built-in Spec Review Workflow (Priority: P1)
**Why this priority**: Flok ships with opinionated review workflows, no plugins needed.
**Acceptance Scenarios**:
1. **Given** the user invokes `/spec-review path/to/spec.md`, **When** the skill loads, **Then** it classifies risk (L0/L1/L2), selects appropriate specialist agents, creates a team, and spawns reviewers in parallel.
2. **Given** reviewers complete Phase 1, **When** the lead identifies cross-review opportunities, **Then** it routes challenges between agents using the predefined routing table.
3. **Given** all phases complete, **When** the lead synthesizes, **Then** a structured review with priority tiers and binary verdict (APPROVED/REVISIONS NEEDED) is produced.

### User Story 5 - Shared Team Task Board (Priority: P1)
**Why this priority**: Tasks provide coordination state visible to all team members.
**Acceptance Scenarios**:
1. **Given** the lead calls `team_task(operation: "create", subject: "Review auth module")`, **When** the task is created, **Then** all team members can see it via `team_task(operation: "list")`.
2. **Given** a member updates a task status to `completed`, **When** other members list tasks, **Then** they see the updated status.

### Edge Cases
- Duplicate agent names in a team: auto-disambiguate (e.g., "general", "general-2", "general-3")
- Team member spawned after team is disbanded: reject with clear error
- Lead session is cancelled while agents are running: disband team, cancel all agent sessions
- Agent sends message to nonexistent recipient: return error listing valid recipients
- Max agents limit reached: block `task` tool until a slot opens (semaphore)
- Member timeout (default 5min): wake lead, let it decide whether to continue waiting
- Network partition between SQLite writes: WAL mode handles gracefully

## Requirements

### Functional Requirements

- **FR-001**: Flok MUST support named agent teams with a lead-member hierarchy.
- **FR-002**: Teams MUST be persisted in SQLite with the following tables:
  - `team` (id, session_id, name, status: active/disbanded, created_at, updated_at)
  - `team_member` (team_id, session_id, agent, role: lead/member, status: active/completed/failed/cancelled, created_at, updated_at)
  - `team_task` (id, team_id, subject, description, owner, status: pending/in_progress/completed/failed, metadata: JSON, created_at, updated_at)
- **FR-003**: Agent names within a team MUST be unique. Duplicate names MUST be auto-disambiguated with a numeric suffix.
- **FR-004**: When a team is disbanded, all active members MUST be cancelled and all pending/in-progress tasks MUST be marked as failed.
- **FR-005**: Background agents MUST be limited by a configurable concurrency cap (`team.max_agents`, default 10).
- **FR-006**: When a background agent completes naturally (LLM stop), its last assistant response MUST be automatically forwarded to the team lead via message injection.
- **FR-007**: When a background agent fails (error/panic), the team lead MUST be notified with an `[AGENT FAILURE]` message including the error details.
- **FR-008**: The team lead's prompt loop MUST wait for active team members before exiting, with a configurable timeout (`team.member_timeout`, default 300s).
- **FR-009**: Message injection MUST be delivered asynchronously -- the sender does not block on the recipient processing the message.
- **FR-010**: On session cancellation, all owned teams MUST be disbanded automatically.
- **FR-011**: On startup, stale teams (active teams from crashed sessions) MUST be reconciled (marked disbanded).

### Built-in Agent Definitions

Flok ships with these agent types, inspired by opencode-plugins:

| Agent | Mode | Description | Default Tools |
|-------|------|-------------|---------------|
| `build` | primary | Default interactive agent. Full tool access (build mode). | All |
| `plan` | primary | Planning mode. Activates plan mode (see spec-004 FR-012). Read-only tools + `plan` tool for writing structured plans. Switching to this agent disables `edit`, `write`, and destructive `bash` commands. | read, glob, grep, bash (read-only), question, todowrite, plan, task, webfetch, skill, team tools, agent_memory |
| `general` | subagent | General-purpose research and multi-step tasks. | All |
| `explore` | subagent | Fast codebase exploration. Read-only. | read, glob, grep |
| `clarity-reviewer` | subagent | Clarity & precision specialist for reviews. | read, glob, grep, bash(git*), send_message, team_task, agent_memory |
| `completeness-reviewer` | subagent | Completeness & edge case specialist. | Same as clarity-reviewer |
| `complexity-reviewer` | subagent | Complexity & simplicity specialist. | Same as clarity-reviewer |
| `feasibility-reviewer` | subagent | Technical feasibility specialist. | Same as clarity-reviewer |
| `product-reviewer` | subagent | Product & value alignment specialist. | Same as clarity-reviewer |
| `operations-reviewer` | subagent | Operations & reliability specialist. | Same as clarity-reviewer |
| `api-reviewer` | subagent | API design & contract specialist. | Same as clarity-reviewer |
| `scope-reviewer` | subagent | Scope & delivery risk specialist. | Same as clarity-reviewer |
| `compaction` | internal | Context compaction. Hidden. | read |
| `title` | internal | Title generation. Hidden. | None |
| `summary` | internal | Session summary. Hidden. | None |

Each reviewer agent carries its full system prompt (review protocol, comment taxonomy, self-critique instructions) as defined in the opencode-plugins agent markdown files, but compiled into the binary.

### Key Entities

```rust
pub struct Team {
    pub id: TeamID,
    pub session_id: SessionID,  // Lead's session
    pub name: String,
    pub status: TeamStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub enum TeamStatus { Active, Disbanded }

pub struct TeamMember {
    pub team_id: TeamID,
    pub session_id: SessionID,  // Member's session
    pub agent: String,          // Disambiguated name
    pub role: TeamRole,
    pub status: MemberStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub enum TeamRole { Lead, Member }

pub enum MemberStatus {
    Active,
    Completed,
    Failed,
    Cancelled,
}

pub struct TeamTask {
    pub id: TeamTaskID,
    pub team_id: TeamID,
    pub subject: String,
    pub description: Option<String>,
    pub owner: Option<String>,
    pub status: TaskStatus,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub enum TaskStatus { Pending, InProgress, Completed, Failed }

pub struct AgentConfig {
    pub name: String,
    pub description: String,
    pub mode: AgentMode,        // Primary, Subagent, Internal
    pub model: Option<String>,  // Model override
    pub system_prompt: String,  // Full prompt text
    pub permissions: PermissionSet,
    pub memory: MemoryMode,     // None, Local
    pub color: Option<String>,  // TUI display color
}

pub enum AgentMode { Primary, Subagent, Internal }
pub enum MemoryMode { None, Local }
```

## Design

### Overview

The agent team system enables a lead agent to create named teams, spawn specialist sub-agents that work in parallel, coordinate via message injection, track progress via a shared task board, and synthesize results. This is the most architecturally significant subsystem in flok -- it transforms flok from a single-agent tool into a multi-agent orchestration platform.

### Event-Driven Architecture (Killer Feature #1)

Unlike Claude Code's polling-based JSON inbox approach, flok uses **event-driven agent communication** via tokio channels. No polling, no file-based inboxes. Agents communicate via typed message passing with zero-copy semantics.

```rust
pub struct AgentChannel {
    tx: mpsc::Sender<AgentMessage>,
    rx: mpsc::Receiver<AgentMessage>,
}

pub enum AgentMessage {
    Injection { from: String, content: String },
    StatusUpdate { agent: String, status: MemberStatus },
    TaskUpdate { task_id: TeamTaskID, status: TaskStatus },
    Shutdown,
}
```

Key primitives: `team_spawn` (creates agent with channel), `team_message` (point-to-point via channel), `team_broadcast` (fan-out to all members), shared task list with dependency DAG (see spec-013), and **peer-to-peer messaging** (not just hub-and-spoke -- members can message each other directly).

### Mixed-Model Team Support (Killer Feature #1)

The killer move: let a lead agent be Claude Opus while teammates are GPT-5.3, Gemini 3, or a local model. Each agent in a team can use a different model, configured via agent definitions or routing tiers (see spec-012):

```toml
# Per-agent model overrides within a team
[agents.lead]
model = "anthropic/claude-opus-4.6"   # Deep reasoning for orchestration

[agents.explore]
model = "deepseek/deepseek-chat"      # Cheap/fast for file search

[agents.general]
model = "openai/gpt-4.1"             # Balanced for code gen
```

The `ProviderRegistry` routes each agent's requests to the correct provider, and the token cache tracks costs per-agent within the team. This enables cost-optimized teams where expensive reasoning models are used only for orchestration, while workers use cheaper models.

### Git Worktree Isolation (Killer Feature #2)

Every background agent gets its own git worktree automatically, preventing file conflicts between concurrent agents. See spec-010 for full details. The team system integrates with worktree isolation:

- On `task(background: true, team_id: ...)`: create worktree, set agent's project root to worktree path
- On agent completion: merge lock serializes worktree merges back to main working tree
- On team disband: clean up all worktrees

### Detailed Design

#### End-to-End Team Lifecycle

```
1. LEAD calls team_create("spec-review-auth")
   → Team row inserted (status: active)
   → Lead registered as member (role: lead)
   → Returns team_id

2. LEAD calls task(subagent_type: "clarity-reviewer", background: true, team_id: "...")
   → Child session created
   → Agent registered as team member (auto-disambiguated name)
   → Background tokio task spawned
   → Returns immediately with session_id

3. LEAD spawns more agents in parallel (single message, multiple tool calls)
   → Each gets its own session + member record
   → Concurrency capped by semaphore (max_agents)

4. BACKGROUND AGENTS work independently
   → Each has its own prompt loop, tools, and message history
   → Can call send_message to communicate with lead or other members
   → Can call team_task to update shared task board
   → Can call agent_memory to read/write persistent knowledge

5. AGENT completes (LLM returns "stop")
   → auto_send_to_lead() fires: last assistant text → injected to lead
   → Team::complete_member() marks member as Completed
   → BusEvent::MemberUpdated published

6. LEAD's prompt loop detects injection
   → wait_for_injection() wakes up
   → Lead processes "[Message from @clarity-reviewer]\n\nFindings: ..."
   → Lead may send cross-review challenges back to agents
   → Lead loops, waits for more, or synthesizes

7. ALL members done → LEAD calls team_delete
   → Team marked Disbanded
   → Any remaining active members cancelled
   → Pending tasks failed

8. CLEANUP on crash/cancel
   → Team::disband_by_session() auto-cleans on session cancel
   → Team::reconcile() runs at startup for stale teams
```

#### Lead Wait Mechanism

The lead's prompt loop has special behavior after the LLM returns "stop":

```rust
async fn should_wait_for_messages(
    state: &AppState,
    session_id: SessionID,
) -> Result<bool> {
    // Team leads: wait if active members exist
    if let Some(team) = Team::find_led_by(state, session_id).await? {
        let active = Team::active_members(state, &team.id).await?;
        return Ok(!active.is_empty());
    }
    // Team members: always wait (for cross-review challenges)
    if Team::is_member(state, session_id).await? {
        return Ok(true);
    }
    Ok(false)
}

async fn wait_for_injection(
    state: &AppState,
    session_id: SessionID,
    cancel: &CancellationToken,
) -> Result<Option<()>> {
    let timeout = state.config.load().team.member_timeout;
    let mut bus_rx = state.bus.subscribe();

    // Subscribe FIRST, then check for pending messages (race-safe)
    let pending = check_pending_injections(state, session_id).await?;
    if pending { return Ok(Some(())); }

    tokio::select! {
        _ = cancel.cancelled() => Ok(None),
        _ = tokio::time::sleep(timeout) => Ok(None),
        event = async {
            loop {
                match bus_rx.recv().await {
                    Ok(BusEvent::MessageInjected { target_session_id, .. })
                        if target_session_id == session_id => break,
                    Ok(BusEvent::MemberUpdated { .. }) => {
                        // Re-evaluate: maybe all members done
                        if !should_wait_for_messages(state, session_id).await.unwrap_or(false) {
                            break;
                        }
                    }
                    _ => continue,
                }
            }
        } => Ok(Some(())),
    }
}
```

#### Name Disambiguation

When adding a member with a duplicate agent name:

```rust
pub async fn add_member(
    db: &Database,
    team_id: &TeamID,
    session_id: SessionID,
    agent_name: &str,
) -> Result<String> {
    // Transaction to prevent TOCTOU
    db.transaction(|tx| async {
        let existing = count_members_with_prefix(tx, team_id, agent_name).await?;
        let disambiguated = if existing == 0 {
            agent_name.to_string()
        } else {
            format!("{}-{}", agent_name, existing + 1)
        };
        insert_member(tx, team_id, session_id, &disambiguated, TeamRole::Member).await?;
        Ok(disambiguated)
    }).await
}
```

#### Built-in Skills (Opinionated, No Plugin Required)

Flok ships with these workflows compiled in:

**`/spec-review`**: Three-phase multi-agent review
- Phase 1: Parallel specialist review with self-critique (L1/L2)
- Phase 2: Lead-mediated cross-review with routing table
- Phase 3: Deduplication + synthesis + binary verdict

**`/code-review`**: Parallel PR review
- Scope-based agent selection (2-4 agents based on diff size)
- Single-phase parallel review (no cross-review)
- Binary verdict: APPROVE or REQUEST CHANGES

**`/self-review-loop`**: Iterative improvement
- Fresh agent each turn (no anchoring bias)
- Auto-detect and run test suite
- Oscillation detection (50% file overlap with 2 turns ago)
- Max 5 turns

**`/handle-pr-feedback`**: Autonomous PR feedback handling
- Fetch unresolved review threads
- Triage (address/skip)
- Fix + commit + reply + resolve

**`/issue-to-spec`**: Issue-to-spec pipeline
- Explore issue + codebase
- Interview user
- Author spec (spec-kit format if `.specify/` exists)
- Assess complexity, optionally invoke `/spec-review`

#### Comment Taxonomy (Shared Across All Reviewers)

```rust
pub enum CommentLabel {
    Blocker,    // Must resolve. Cite concrete harm.
    Risk,       // Gap to consciously accept.
    Question,   // Seeking clarification.
    Suggestion, // Concrete alternative with rationale.
    Nitpick,    // Minor preference.
    Thought,    // Observation, not a request.
}

pub enum Priority { P0, P1, P2, P3 }
```

#### Cross-Review Routing Table

```rust
fn cross_review_target(source: &str) -> Option<&str> {
    match source {
        "clarity-reviewer"      => Some("completeness-reviewer"),
        "completeness-reviewer" => Some("product-reviewer"),
        "product-reviewer"      => Some("scope-reviewer"),
        "feasibility-reviewer"  => Some("complexity-reviewer"),
        "api-reviewer"          => Some("operations-reviewer"),
        "operations-reviewer"   => Some("feasibility-reviewer"),
        "scope-reviewer"        => Some("product-reviewer"),
        "complexity-reviewer"   => Some("feasibility-reviewer"),
        _ => None,
    }
}
```

### Alternatives Considered

1. **Plugin-based agent definitions (like opencode)**: Rejected for built-in agents. Flok is opinionated -- core review workflows ship in the binary. Users can still define custom agents in `.flok/agents/`.
2. **HTTP-based inter-agent communication**: Rejected. Message injection via SQLite + event bus is faster and simpler for a local tool.
3. **Shared memory between agents (via Arc<Mutex>)**: Rejected. Each agent has its own session and message history. Communication is via explicit message injection, which is auditable and debuggable.
4. **Fixed agent count per review type**: Rejected. Dynamic agent selection based on risk/scope classification (L0/L1/L2) is more efficient.

## Success Criteria

- **SC-001**: Team creation + 5 agent spawns complete in < 500ms
- **SC-002**: Message injection latency (send to receive) < 10ms
- **SC-003**: Auto-send-to-lead fires within 100ms of agent completion
- **SC-004**: Team reconciliation on startup completes in < 100ms
- **SC-005**: Full spec-review (5 agents, 3 phases) completes in < 5 minutes for a typical spec

## Assumptions

- 8 specialist reviewer agents are sufficient for most review scenarios
- The cross-review routing table covers the most valuable domain-boundary challenges
- Auto-send-to-lead is the right default (agents shouldn't need to remember to report back)
- 5-minute member timeout is sufficient for most agent tasks

## Open Questions

- Should we support inter-team communication (agents from different teams talking)?
- Should the routing table be configurable or is the hardcoded version sufficient?
- Should agents be able to spawn their own sub-teams (nested team hierarchy)?
- How should we handle agent memory conflicts (two agents writing to the same memory key)?
