# Feature Specification: Tree-Based Session Branching

**Feature Branch**: `018-tree-sessions`
**Created**: 2026-04-01
**Status**: Draft

## Motivation

Coding conversations are inherently exploratory. A developer asks the agent to try approach A. It fails. They want to rewind to the decision point and try approach B -- without losing the context of what was tried. Today, Flok's session model is linear: undo destroys messages, and there's no way to preserve multiple exploration paths within a single session lineage.

Tree-based sessions solve this by turning conversations into navigable decision trees. Each branch preserves full context, and switching branches injects a summary of the abandoned path so the agent learns from prior attempts.

This is directly inspired by Pi's tree-based session model, adapted to Flok's SQLite-backed architecture.

**Resolves open question from spec-003**: "Should we support session branching (fork a conversation at a specific message)?" -- **Yes.**

## User Scenarios & Testing

### User Story 1 - Developer Branches at a Decision Point (Priority: P0)
**Why this priority**: This is the core interaction -- the entire feature.
**Acceptance Scenarios**:
1. **Given** a conversation with 10+ messages, **When** the user invokes `/branch` and selects message 5, **Then** a new session is created containing messages 1-5, the user's cursor is in the new session's input, and the original session is preserved untouched.
2. **Given** the user branches from message 5, **When** the new session starts, **Then** a branch summary of messages 6-10 from the original session is injected as a synthetic system context message, so the agent knows what was previously tried.
3. **Given** the branched session, **When** the user types a new message, **Then** the conversation continues from message 5 with the new direction, and the agent's response reflects awareness of the abandoned branch.

### User Story 2 - Developer Navigates Session Tree (Priority: P0)
**Why this priority**: Without tree navigation, branches are dead ends.
**Acceptance Scenarios**:
1. **Given** a session with 3 branches, **When** the user invokes `/tree`, **Then** a tree view shows the session lineage with branch points, titles, timestamps, and message counts.
2. **Given** the tree view is open, **When** the user selects a sibling branch, **Then** Flok switches to that session, restoring both conversation history and workspace files (via snapshot).
3. **Given** the tree view, **When** the user presses `/` or starts typing, **Then** the tree is filtered by title/content fuzzy match.

### User Story 3 - Automatic Branch Summarization (Priority: P0)
**Why this priority**: Summaries are what make branching useful -- otherwise the agent has amnesia about abandoned paths.
**Acceptance Scenarios**:
1. **Given** a branch from message 5 of a 10-message conversation, **When** the branch is created, **Then** an LLM-generated summary of messages 6-10 is stored in the new session, including: what was attempted, what failed, key decisions made, and files modified.
2. **Given** the summary is generated, **When** it's injected into the new session's context, **Then** it appears as a user-role message with a clear `[Branch Context]` prefix so the LLM can distinguish it from actual user input.
3. **Given** the original branch had a compaction summary, **When** summarizing, **Then** the compaction summary is included as prior context (not re-summarized).

### User Story 4 - Snapshot Integration with Branches (Priority: P1)
**Why this priority**: File state must match conversation state when switching branches.
**Acceptance Scenarios**:
1. **Given** a branch point at message 5, **When** the branch is created, **Then** the workspace snapshot from after message 5's tool executions is recorded as the branch's starting snapshot.
2. **Given** the user switches from branch A to branch B via `/tree`, **When** the switch completes, **Then** the workspace files are restored to branch B's latest snapshot state.
3. **Given** a branch switch would overwrite unsaved changes, **When** the switch is initiated, **Then** the user is prompted with a confirmation dialog showing which files would change.

### User Story 5 - Labels and Bookmarks (Priority: P2)
**Why this priority**: Nice-to-have for power users navigating deep trees.
**Acceptance Scenarios**:
1. **Given** a session, **When** the user invokes `/label "checkpoint: auth working"`, **Then** the current session is annotated with that label, visible in `/tree`.
2. **Given** a labeled session in the tree view, **When** filtering, **Then** labels are included in the search index.

### Edge Cases
- Branch from a session with 0 assistant messages: create branch with just the user message, no summary needed
- Branch from a sub-agent session: reject with error -- sub-agent sessions are managed by the team system
- Branch from the very first message: create a fresh session with `parent_id` pointing to original, copy only message 1
- Branch when context is 95%+ full: run compaction on the new branch before generating the summary
- Circular parent_id references: prevented by DB constraints (new sessions always have higher IDs)
- Switch branches while agent is streaming: cancel current stream, then switch
- Summary generation fails (LLM error): create the branch anyway with a fallback text summary "[Summary generation failed. Prior branch explored messages 6-10 and modified: auth.rs, config.rs]"
- Very long branch to summarize (100+ messages): apply token budget (last 40K tokens) when preparing messages for summarization, same as compaction

## Requirements

### Functional Requirements

- **FR-001**: Sessions MUST support a tree structure via the existing `parent_id` FK on the `sessions` table.
- **FR-002**: Branching MUST create a new session that contains a copy of messages from the parent session up to (and including) the branch point message.
- **FR-003**: Branch creation MUST generate an LLM-summarized `branch_summary` of the messages that exist in the parent session *after* the branch point.
- **FR-004**: Branch summaries MUST be injected as a synthetic user-role message at the start of the new session (after the copied messages), formatted so the LLM can distinguish it from real user input.
- **FR-005**: The `/tree` command MUST display the full session lineage as a navigable tree, showing: title, branch point, message count, last activity, labels, and current session indicator.
- **FR-006**: Switching sessions via `/tree` MUST restore the workspace to the target session's latest snapshot state.
- **FR-007**: Branch summaries MUST track file operations (files read and modified) from the summarized messages, appended as structured metadata.
- **FR-008**: The undo system MUST remain linear within each session (no change to existing undo/redo behavior per session).
- **FR-009**: Labels MUST be persisted in the database, associated with a session ID.
- **FR-010**: Branching MUST be a read-only operation on the parent session -- no modification to the original session's messages or state.

### Non-Functional Requirements

- **NR-001**: Branch creation (excluding summary generation) MUST complete in < 500ms for sessions with up to 200 messages.
- **NR-002**: Tree view rendering MUST be instant (< 50ms) for trees with up to 50 branches.
- **NR-003**: Session switching (snapshot restore + DB load) MUST complete in < 1s.

### Key Entities

```rust
/// A branch point records where and why a session was forked.
pub struct BranchPoint {
    /// The message ID in the parent session where the branch was taken.
    pub from_message_id: String,
    /// The snapshot hash of the workspace at the branch point.
    pub snapshot_hash: Option<String>,
}

/// A branch summary injected into the new session's context.
pub struct BranchSummary {
    /// LLM-generated summary of the abandoned branch.
    pub summary: String,
    /// Files that were read during the abandoned branch.
    pub files_read: Vec<String>,
    /// Files that were modified during the abandoned branch.
    pub files_modified: Vec<String>,
}

/// A session label for bookmarking.
pub struct SessionLabel {
    pub session_id: String,
    pub label: String,
    pub created_at: String,
}

/// A node in the session tree, used for the /tree TUI.
pub struct SessionTreeNode {
    pub session: Session,
    pub children: Vec<SessionTreeNode>,
    pub label: Option<String>,
    pub message_count: usize,
    pub is_current: bool,
    pub branch_point: Option<BranchPoint>,
}
```

## Design

### Overview

Tree-based sessions use a **session-per-branch** architecture. Each branch is a separate session row in SQLite, linked via the existing `parent_id` foreign key. When branching, messages up to the branch point are *copied* into the new session. This design was chosen over a message-level tree (adding `parent_id` to messages) because:

1. **Isolation**: Each session has independent message history. Compression, undo, and compaction operate per-session without tree-aware logic.
2. **Simplicity**: The existing `SessionEngine`, `assemble_messages()`, and all tool execution code works unchanged. They see a flat message list per session.
3. **Existing schema support**: `sessions.parent_id` is already in the schema (spec-003 anticipated this).
4. **Performance**: No per-query tree traversal. Loading a session is still a simple `WHERE session_id = ?`.
5. **Sub-agent compatibility**: Sub-agents already use per-session isolation. Branch sessions fit naturally.

The trade-off is message duplication at branch time (O(n) copy where n is messages up to branch point). For typical conversations (< 200 messages), this is negligible.

### Detailed Design

#### Schema Changes (Migration 3)

```sql
-- Add branch metadata to sessions
ALTER TABLE sessions ADD COLUMN branch_from_message_id TEXT;
ALTER TABLE sessions ADD COLUMN branch_snapshot_hash TEXT;

-- Session labels
CREATE TABLE IF NOT EXISTS session_labels (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT NOT NULL REFERENCES sessions(id),
    label       TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(session_id)  -- one label per session
);

CREATE INDEX IF NOT EXISTS idx_session_labels_session ON session_labels(session_id);
```

The `sessions.parent_id` column already exists and is already nullable. We add two columns:
- `branch_from_message_id`: the message ID in the **parent** session where the branch was taken (NULL for root sessions)
- `branch_snapshot_hash`: the workspace snapshot hash at the branch point (NULL if snapshots are disabled)

#### Branch Creation Algorithm

```
branch(session_id, from_message_id) -> Result<Session>:

1. Validate:
   - from_message_id exists in session_id
   - session is not a sub-agent session (no team association)
   - agent is not currently streaming

2. Capture branch point snapshot:
   - snapshot_hash = current workspace snapshot (or the snapshot
     associated with from_message_id if we track per-message snapshots)

3. Create new session:
   - new_id = ULID
   - parent_id = session_id
   - branch_from_message_id = from_message_id
   - branch_snapshot_hash = snapshot_hash
   - title = parent_title + " (branch)"
   - model_id = parent's current model

4. Copy messages:
   - SELECT messages FROM parent WHERE rowid <= from_message.rowid
   - For each message: INSERT INTO new session with new IDs, preserving
     role, parts, and relative ordering
   - Use a single transaction for atomicity

5. Generate branch summary (async):
   - Collect messages AFTER from_message_id in the parent session
   - If > 0 messages:
     a. Apply token budget (last 40K tokens from the tail)
     b. Extract file operations from tool call parts
     c. Call LLM with summarization prompt (see below)
     d. Insert branch summary as a synthetic user message in the new session
   - If 0 messages (branching from the last message): skip summary

6. Emit BusEvent::SessionBranched { parent_id, new_session_id, from_message_id }

7. Switch to new session:
   - Restore workspace to snapshot_hash
   - Load new session in engine
```

#### Branch Summary Prompt

The LLM is called with a structured summarization prompt:

```
The user is branching this conversation to explore a different approach.
Summarize what happened in the abandoned branch so the assistant can
learn from it.

<conversation>
{serialized messages after branch point}
</conversation>

Produce a structured summary with:
1. **Goal**: What was the user trying to achieve?
2. **Approach**: What approach was taken?
3. **Outcome**: What happened? Did it succeed or fail? Why?
4. **Key Decisions**: Any important decisions or discoveries.
5. **Files Modified**: List of files that were changed.
6. **Lessons**: What should be avoided or considered in a new approach?

Keep the summary concise (< 500 words). Focus on actionable information
that helps the assistant take a different approach.
```

The summary is injected as a user-role message:

```
[Branch Context] The following summarizes a previous exploration path
that was abandoned. Use this to avoid repeating failed approaches.

<branch-summary>
{LLM-generated summary}
</branch-summary>

<files-read>{comma-separated list}</files-read>
<files-modified>{comma-separated list}</files-modified>
```

#### Session Tree Construction

```rust
pub fn build_session_tree(
    db: &Db,
    project_id: &str,
    current_session_id: &str,
) -> Result<Vec<SessionTreeNode>> {
    // 1. Load all sessions for project
    let sessions = db.list_sessions(project_id)?;

    // 2. Load labels (batch)
    let labels = db.list_session_labels_for_project(project_id)?;
    let label_map: HashMap<String, String> = labels.into_iter()
        .map(|l| (l.session_id, l.label))
        .collect();

    // 3. Load message counts (batch query)
    let counts = db.count_messages_batch(project_id)?;

    // 4. Build tree from flat list
    let mut nodes: HashMap<String, SessionTreeNode> = HashMap::new();
    let mut children_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut roots: Vec<String> = Vec::new();

    for session in &sessions {
        let node = SessionTreeNode {
            session: session.clone(),
            children: Vec::new(),
            label: label_map.get(&session.id).cloned(),
            message_count: counts.get(&session.id).copied().unwrap_or(0),
            is_current: session.id == current_session_id,
            branch_point: session.branch_from_message_id.as_ref().map(|msg_id| {
                BranchPoint {
                    from_message_id: msg_id.clone(),
                    snapshot_hash: session.branch_snapshot_hash.clone(),
                }
            }),
        };
        nodes.insert(session.id.clone(), node);

        match &session.parent_id {
            Some(pid) => {
                children_map.entry(pid.clone()).or_default().push(session.id.clone());
            }
            None => roots.push(session.id.clone()),
        }
    }

    // 5. Recursively build tree (depth-first)
    fn attach_children(
        node_id: &str,
        nodes: &mut HashMap<String, SessionTreeNode>,
        children_map: &HashMap<String, Vec<String>>,
    ) -> Option<SessionTreeNode> {
        let child_ids = children_map.get(node_id).cloned().unwrap_or_default();
        let mut children: Vec<SessionTreeNode> = Vec::new();
        for cid in &child_ids {
            if let Some(child) = attach_children(cid, nodes, children_map) {
                children.push(child);
            }
        }
        // Sort children by created_at (oldest first)
        children.sort_by(|a, b| a.session.created_at.cmp(&b.session.created_at));

        let mut node = nodes.remove(node_id)?;
        node.children = children;
        Some(node)
    }

    let tree: Vec<SessionTreeNode> = roots.iter()
        .filter_map(|rid| attach_children(rid, &mut nodes, &children_map))
        .collect();

    Ok(tree)
}
```

#### Tree View TUI Component

A new `TreeView` component in `flok-tui` renders the session tree:

```
Session Tree
────────────────────────────────────────────────────────────
 ▼ Fix authentication bug           12 msgs  $0.42  2m ago
   ├─ ▼ Fix auth bug (branch)        8 msgs  $0.31  1m ago
   │     └─ Try JWT approach          5 msgs  $0.18  30s ago
   └─ ● Fix auth bug - v2            6 msgs  $0.22  10s ago
         [label: "working approach"]

 ▼ Add dark mode                      3 msgs  $0.12  1h ago

 Enter=switch  b=branch  l=label  /=search  q=close
```

- `▼` = expanded, `▶` = collapsed
- `●` = current session
- Labels shown inline in `[brackets]`
- Navigate with arrow keys, Enter to switch
- `b` to branch from the selected session
- `l` to add/edit a label
- `/` to search/filter

#### Integration with Existing Systems

**Undo/Redo**: Unchanged. Each session has its own independent undo stack. Branching creates a fresh session with an empty undo stack.

**Compaction**: Unchanged. Compaction operates per-session on the flat message list. Branch summaries participate in compaction like any other user message.

**Snapshots**: When branching, the snapshot hash at the branch point is recorded. When switching sessions via `/tree`, the target session's latest snapshot is restored. If the target has no snapshots (e.g., just created), the branch point snapshot is used.

**Sub-agents**: Sub-agent sessions have `parent_id` set to the spawning session. The tree view distinguishes sub-agent sessions from user-created branches via presence of team association. Sub-agent sessions are hidden by default in `/tree` (togglable with `a` key).

**Compression**: Branch summary messages are treated as regular user messages during compression. They are NOT pruned by T1 (they are text, not tool results). They participate in T2 compaction normally.

**Permission rules**: Shared at the project level, not per-session. Branching does not affect permissions.

### Alternatives Considered

1. **Message-level tree (Pi's approach)**: Add `parent_id` to messages, build tree in-memory, walk root-to-leaf for LLM context. Rejected because:
   - Requires rewriting `assemble_messages()`, compression pipeline, and undo system to be tree-aware
   - Single session with 100+ branches would have O(n) tree traversal on every prompt loop iteration
   - More complex schema and query patterns for a SQLite backend
   - Pi uses this because it's JSONL (append-only, single file per session); Flok uses SQLite where session isolation is natural

2. **Copy-on-write with shared message references**: New sessions reference parent messages by ID instead of copying. Rejected because:
   - Adds complexity to message deletion, compaction, and undo (can't delete shared messages)
   - Cross-session message references complicate WAL transaction boundaries
   - Message duplication cost is negligible (< 1MB for 200 messages)

3. **Git-based branching (literal git branches for conversations)**: Rejected. Over-engineered for this use case. The shadow snapshot system handles file state; conversation state belongs in SQLite.

## Success Criteria

- **SC-001**: Branch creation (message copy) completes in < 200ms for 100-message sessions.
- **SC-002**: Branch summary generation completes in < 15s (LLM-dependent).
- **SC-003**: Session tree construction completes in < 20ms for 50 sessions.
- **SC-004**: Session switch (snapshot restore + message load) completes in < 500ms.
- **SC-005**: No regression in session engine prompt loop latency (branching is orthogonal to the hot path).

## Assumptions

- Typical session trees will have < 20 branches (deep trees are uncommon)
- Message duplication at branch time is acceptable (< 1MB per branch for typical conversations)
- Users will branch at most a few times per hour (not a high-frequency operation)
- Branch summary quality depends on the active model; no fallback to a cheaper model

## Open Questions

- Should `/tree` show sub-agent sessions? If so, in a separate section or inline with user branches?
- Should we support "merging" branches (combining insights from two branches into one)? Deferred to a future spec.
- Should branch summaries be editable by the user before injection?
- Should there be a configurable limit on tree depth or total branches per root session?
- Should the sidebar show the current branch's position in the tree (e.g., "branch 2/3 from 'Fix auth bug'")?

## Implementation Plan

### Phase 1: Schema & Core (P0)
1. Add migration 3: `branch_from_message_id`, `branch_snapshot_hash` columns on `sessions`, `session_labels` table
2. Add DB queries: `create_branch_session`, `list_session_labels`, `upsert_session_label`, `count_messages_batch`, `copy_messages_to_session`
3. Implement `BranchEngine` in `flok-core/src/session/branch.rs`: branch creation, message copying, summary generation
4. Add `BusEvent::SessionBranched` and `BusEvent::SessionSwitched`

### Phase 2: Tree Navigation (P0)
5. Implement `build_session_tree()` in `flok-core/src/session/tree.rs`
6. Add `/branch` slash command (opens message selector, creates branch)
7. Add `/tree` slash command (opens tree view)
8. Implement session switching with snapshot restore

### Phase 3: TUI (P1)
9. Build `TreeView` component in `flok-tui`
10. Integrate with sidebar (show current branch indicator)
11. Add keyboard shortcuts for branch/tree operations

### Phase 4: Polish (P2)
12. Add `/label` command
13. Add branch summary to system prompt preamble (so agent always knows it's on a branch)
14. Add tree depth indicator in footer
15. Integration tests for the full branch-switch-resume workflow
