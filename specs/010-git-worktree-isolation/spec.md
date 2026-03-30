# Feature Specification: Git Worktree Isolation Per Agent

**Feature Branch**: `010-git-worktree-isolation`
**Created**: 2026-03-28
**Status**: Draft

## User Scenarios & Testing

### User Story 1 - Background Agent Gets Its Own Worktree (Priority: P0)
**Why this priority**: File conflicts between concurrent agents is the #1 practical problem with agent teams today. Two agents editing the same file causes silent overwrites.
**Acceptance Scenarios**:
1. **Given** a lead agent spawns a background agent via `task(background: true)`, **When** the agent starts, **Then** a git worktree is created from the current branch and the agent's `project.root` points to the worktree directory.
2. **Given** two background agents are editing files concurrently, **When** they both modify `src/main.rs`, **Then** each agent's edit is isolated to its own worktree -- no file conflicts.
3. **Given** a background agent completes, **When** its worktree is ready for merge, **Then** the lead agent reviews the diff and merges (or rebases) via the merge lock.
4. **Given** an agent is cancelled or fails, **When** cleanup runs, **Then** the worktree is removed and no stale worktrees remain.

### User Story 2 - Lead Agent Merges Worktree Results (Priority: P0)
**Why this priority**: Isolated edits must be reintegrated into the main working tree.
**Acceptance Scenarios**:
1. **Given** a background agent has completed its work in a worktree, **When** the lead agent reviews, **Then** it can see a diff of all changes made in the worktree relative to the base branch.
2. **Given** two agents' worktrees have non-conflicting changes, **When** merge runs, **Then** both sets of changes are applied cleanly to the main working tree.
3. **Given** two agents' worktrees have conflicting changes to the same file, **When** conflict is detected, **Then** the lead agent is notified with the conflict details and can choose which version to keep or manually resolve.
4. **Given** merge succeeds, **When** the worktree is no longer needed, **Then** it is automatically cleaned up (removed from disk and git worktree list).

### User Story 3 - Foreground Agents Reuse the Main Worktree (Priority: P1)
**Why this priority**: Not all agents need isolation -- foreground sub-agents operate sequentially.
**Acceptance Scenarios**:
1. **Given** a foreground agent is spawned via `task(background: false)`, **When** it starts, **Then** it uses the same working directory as the parent (no worktree created).
2. **Given** the user is in a solo session (no teams), **When** the primary agent edits files, **Then** no worktree isolation is applied (direct edits to the working tree).

### Edge Cases
- Not a git repository: disable worktree isolation, warn user, agents share the working directory
- Worktree creation fails (e.g., dirty submodules): fall back to shared working directory with a warning
- Agent spawns a sub-agent from within a worktree: sub-agent gets its own worktree branched from the parent agent's worktree
- Worktree directory already exists (stale from crash): clean up stale worktree before creating new one
- Large repository (>1GB): worktree creation is fast (git worktrees share the `.git` directory, only checkout files)
- Agent tries to push from worktree: block -- only the main worktree can push
- Merge conflict resolution timeout: after 60s of unresolved conflict, log the conflict diff and let the lead decide

## Requirements

### Functional Requirements

- **FR-001**: Flok MUST create an isolated git worktree for each background agent on spawn.
- **FR-002**: Worktrees MUST be created in a managed directory: `$XDG_STATE_HOME/flok/worktrees/{project_id}/{session_id}/`.
- **FR-003**: Worktrees MUST be created from the current HEAD of the main working tree (or a specified branch/commit).
- **FR-004**: Each agent's `ToolContext` MUST have its `project_root` set to the worktree path, so all file tools (`read`, `write`, `edit`, `glob`, `grep`, `bash`) operate within the isolated worktree.
- **FR-005**: Foreground agents (blocking `task` calls) MUST NOT create worktrees -- they reuse the parent's working directory.
- **FR-006**: On agent completion, flok MUST provide a merge mechanism:
  - **Auto-merge**: If no conflicts, merge changes into the main worktree automatically.
  - **Conflict escalation**: If conflicts exist, notify the lead agent with a diff and conflict markers.
  - **Manual merge**: The lead agent can invoke a `worktree_merge` tool to resolve conflicts.
- **FR-007**: A **merge lock** (tokio Mutex) MUST serialize merge operations to prevent concurrent merges from creating inconsistencies.
- **FR-008**: On agent cancellation or failure, worktrees MUST be cleaned up automatically.
- **FR-009**: On flok startup, stale worktrees (from crashed sessions) MUST be detected and removed.
- **FR-010**: Worktree creation MUST complete in < 500ms for repositories with < 100K files.
- **FR-011**: Agents MUST NOT be able to push from worktrees. Push operations are only allowed from the main working tree.
- **FR-012**: The worktree system MUST be opt-in via config (default: enabled for background agents):
  ```toml
  [worktree]
  enabled = true                    # Enable worktree isolation
  auto_merge = true                 # Auto-merge non-conflicting changes
  cleanup_on_complete = true        # Remove worktree after merge
  max_worktrees = 10                # Max concurrent worktrees per project
  ```

### Key Entities

```rust
pub struct WorktreeManager {
    project_root: PathBuf,
    worktree_base: PathBuf,      // $XDG_STATE_HOME/flok/worktrees/{project_id}/
    merge_lock: Mutex<()>,
    active: DashMap<SessionID, WorktreeInfo>,
}

pub struct WorktreeInfo {
    pub session_id: SessionID,
    pub path: PathBuf,
    pub branch: String,           // Temporary branch name: "flok/{session_id}"
    pub base_commit: String,      // Commit SHA the worktree was created from
    pub created_at: Instant,
}

pub enum MergeResult {
    Clean,                        // All changes merged successfully
    Conflict(Vec<ConflictFile>),  // Some files have conflicts
    NothingToMerge,               // No changes in worktree
}

pub struct ConflictFile {
    pub path: PathBuf,
    pub ours: String,             // Main worktree version
    pub theirs: String,           // Agent worktree version
    pub conflict_markers: String, // Standard git conflict markers
}
```

## Design

### Overview

The worktree isolation system ensures that concurrent background agents cannot interfere with each other's file operations. Each background agent works in a dedicated git worktree -- a lightweight, full checkout of the repository that shares the `.git` directory with the main working tree. On completion, changes are merged back through a serialized merge lock.

### Detailed Design

#### Worktree Lifecycle

```
1. LEAD spawns background agent
   → WorktreeManager::create(session_id, base_commit)
   → `git worktree add <path> -b flok/<session_id> <base_commit>`
   → Agent's ToolContext.project_root = worktree path

2. AGENT works in isolation
   → All file tools (read/write/edit/bash) use worktree path
   → Agent can commit to its temporary branch
   → No interference with other agents or main worktree

3. AGENT completes
   → WorktreeManager::merge(session_id)
   → Acquire merge_lock
   → Compute diff: worktree vs main
   → If clean: apply changes to main worktree (checkout files)
   → If conflicts: escalate to lead with conflict details
   → Release merge_lock

4. CLEANUP
   → `git worktree remove <path>`
   → `git branch -D flok/<session_id>`
   → Remove from active map
```

#### Worktree Creation

```rust
impl WorktreeManager {
    pub async fn create(
        &self,
        session_id: SessionID,
        base_commit: Option<&str>,
    ) -> Result<WorktreeInfo> {
        let base = base_commit.unwrap_or("HEAD");
        let branch_name = format!("flok/{}", session_id);
        let worktree_path = self.worktree_base.join(session_id.to_string());

        // Clean up stale worktree at this path if exists
        if worktree_path.exists() {
            self.cleanup(&session_id).await?;
        }

        // Create worktree with a new branch
        let output = tokio::process::Command::new("git")
            .args(["worktree", "add", "-b", &branch_name])
            .arg(&worktree_path)
            .arg(base)
            .current_dir(&self.project_root)
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow!(
                "Failed to create worktree: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let info = WorktreeInfo {
            session_id,
            path: worktree_path,
            branch: branch_name,
            base_commit: resolve_commit(&self.project_root, base).await?,
            created_at: Instant::now(),
        };

        self.active.insert(session_id, info.clone());
        Ok(info)
    }
}
```

#### Merge Strategy

The merge process applies changes from the worktree back to the main working tree without using `git merge` (which would create merge commits in a potentially dirty working tree):

```rust
impl WorktreeManager {
    pub async fn merge(&self, session_id: SessionID) -> Result<MergeResult> {
        let _lock = self.merge_lock.lock().await;

        let info = self.active.get(&session_id)
            .ok_or_else(|| anyhow!("No worktree for session {}", session_id))?;

        // Get list of changed files in the worktree
        let diff_output = tokio::process::Command::new("git")
            .args(["diff", "--name-status", &info.base_commit, "HEAD"])
            .current_dir(&info.path)
            .output()
            .await?;

        let changed_files = parse_diff_name_status(&diff_output.stdout)?;

        if changed_files.is_empty() {
            return Ok(MergeResult::NothingToMerge);
        }

        // Check for conflicts: has the main worktree modified these files since base_commit?
        let mut conflicts = Vec::new();
        let mut clean_files = Vec::new();

        for file in &changed_files {
            let main_changed = file_changed_since(
                &self.project_root, &info.base_commit, &file.path
            ).await?;

            if main_changed {
                conflicts.push(build_conflict_info(
                    &self.project_root, &info.path, &file.path
                ).await?);
            } else {
                clean_files.push(file.clone());
            }
        }

        // Apply clean files
        for file in &clean_files {
            copy_file_from_worktree(&info.path, &self.project_root, &file.path).await?;
        }

        if conflicts.is_empty() {
            Ok(MergeResult::Clean)
        } else {
            Ok(MergeResult::Conflict(conflicts))
        }
    }
}
```

#### Integration with Agent Teams

The worktree system hooks into the `task` tool's background agent spawning:

```rust
// In TaskTool::execute, after creating child session:
if params.background && state.config.load().worktree.enabled {
    if let Ok(worktree) = state.worktrees.create(child_session.id, None).await {
        // Override project root for this agent's session
        child_session.set_project_root(worktree.path.clone());
    } else {
        tracing::warn!("Worktree creation failed, agent will share main worktree");
    }
}
```

#### Stale Worktree Cleanup

On startup, reconcile stale worktrees from crashed sessions:

```rust
impl WorktreeManager {
    pub async fn reconcile(&self) -> Result<usize> {
        let output = tokio::process::Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&self.project_root)
            .output()
            .await?;

        let worktrees = parse_worktree_list(&output.stdout)?;
        let mut cleaned = 0;

        for wt in worktrees {
            if wt.branch.starts_with("refs/heads/flok/") {
                // This is a flok-managed worktree -- check if its session is still active
                let session_id = extract_session_id(&wt.branch);
                if !self.is_session_active(session_id).await? {
                    self.cleanup(&session_id).await?;
                    cleaned += 1;
                }
            }
        }

        Ok(cleaned)
    }
}
```

### Alternatives Considered

1. **File-level locking (flock/fcntl)**: Rejected. File locks prevent concurrent access but don't solve the "two agents need different versions of the same file" problem. Worktrees give each agent a complete, consistent view.
2. **Copy the entire project per agent**: Rejected. Wasteful for large repos. Git worktrees share the `.git` directory and are lightweight (only checkout files differ).
3. **Use libgit2 bindings instead of git CLI**: Considered. libgit2 (via `git2` crate) would be faster and avoid process spawning, but the git CLI is more reliable for worktree operations and handles edge cases (submodules, hooks) better. Revisit post-1.0.
4. **Branch per agent without worktrees**: Rejected. Branches without worktrees still share the same working directory. The whole point is filesystem isolation.
5. **In-memory virtual filesystem**: Rejected. Too complex, doesn't work with `bash` tool calls that expect real files on disk.

## Success Criteria

- **SC-001**: Worktree creation completes in < 500ms for repos with < 100K files
- **SC-002**: Clean merge (no conflicts) completes in < 200ms
- **SC-003**: Conflict detection completes in < 100ms per file
- **SC-004**: Stale worktree cleanup on startup completes in < 1s
- **SC-005**: Zero stale worktrees after graceful shutdown
- **SC-006**: No file conflicts between concurrent background agents (the entire point)

## Assumptions

- Users have `git` (>= 2.20) installed and available on PATH
- The project is a git repository (worktree isolation is disabled for non-git projects)
- Worktrees share `.git` directory efficiently (minimal disk overhead)
- Most agent tasks produce changes to < 50 files (merge is fast)
- Merge conflicts between agent outputs are rare (agents typically work on different areas)

## Open Questions

- Should we support nested worktrees (agent in a worktree spawns its own sub-agent)?
- Should the lead agent see a "merge preview" before auto-merge is applied?
- Should we support `libgit2` as an alternative backend for performance-critical operations?
- Should worktree branches be force-deleted on cleanup, or should we keep them for a configurable retention period for debugging?
- How should worktree isolation interact with tools like `bash` that might `cd` to absolute paths outside the worktree?
