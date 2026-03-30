# Feature Specification: Plan-Then-Execute Architecture

**Feature Branch**: `013-plan-execute`
**Created**: 2026-03-28
**Status**: Draft

## User Scenarios & Testing

### User Story 1 - Agent Creates a Structured Execution Plan (Priority: P0)
**Why this priority**: Separating planning from execution at the architecture level (not just the prompt level) is the key to reliable multi-step tasks.
**Acceptance Scenarios**:
1. **Given** the user requests a complex task ("refactor the auth module to use JWT"), **When** the plan agent analyzes the codebase, **Then** it produces a structured execution plan as a DAG with named steps, dependencies, and estimated scope.
2. **Given** a plan is produced, **When** it is serialized, **Then** it can be saved to `.flok/plans/{plan_id}.json`, diffed, and resumed later.
3. **Given** a plan with 8 steps, **When** the user reviews it, **Then** they see each step's description, affected files, dependencies, and estimated tokens/cost.

### User Story 2 - User Reviews and Approves the Plan (Priority: P0)
**Why this priority**: Human approval before execution is critical for trust.
**Acceptance Scenarios**:
1. **Given** a plan is displayed in the TUI, **When** the user presses Enter or types `/approve`, **Then** execution begins.
2. **Given** the user wants to modify the plan, **When** they edit a step or remove steps, **Then** the modified plan is used for execution.
3. **Given** the user is running in CI mode (`--auto-approve`), **When** a plan is produced, **Then** execution starts automatically without waiting for approval.

### User Story 3 - Build Agents Execute the Plan with Checkpoints (Priority: P0)
**Why this priority**: Checkpoints enable recovery from failures without restarting the entire task.
**Acceptance Scenarios**:
1. **Given** an approved plan with steps A → B → C (sequential) and D (parallel with C), **When** execution starts, **Then** A runs first, then B, then C and D run concurrently.
2. **Given** step B fails, **When** the failure is detected, **Then** execution pauses at step B, steps C and D are not started, and the agent can retry or rollback.
3. **Given** step B fails and rollback is requested, **When** rollback executes, **Then** all file changes from steps A and B are reverted to the checkpoint before step A.
4. **Given** a long plan is partially executed (steps A, B, C completed), **When** the user resumes later with `--continue`, **Then** execution picks up from step D.

### User Story 4 - Plan Mode Produces Plans Without Side Effects (Priority: P1)
**Why this priority**: Plan mode (spec-004) should integrate with the plan-execute architecture.
**Acceptance Scenarios**:
1. **Given** the user activates plan mode, **When** the agent analyzes a task, **Then** it produces a plan data structure (not just a markdown description).
2. **Given** a plan was created in plan mode, **When** the user switches to build mode, **Then** they can execute the saved plan with `/execute-plan`.
3. **Given** the agent is in build mode and the task is complex, **When** the agent determines planning is needed, **Then** it can self-initiate plan creation before executing.

### Edge Cases
- Plan with circular dependencies: reject with error showing the cycle
- Plan step has no clear success criteria: warn, allow execution with manual confirmation per step
- Agent deviates from plan during execution: detect via file-change monitoring, warn user
- Plan becomes stale (files changed since planning): detect modified files, warn and re-plan affected steps
- Empty plan (no steps): return immediately with "nothing to do"
- Very large plan (>20 steps): warn user about cost/time estimate, require explicit confirmation
- Concurrent plan execution (two plans running): reject, only one plan can execute at a time per session

## Requirements

### Functional Requirements

- **FR-001**: Plans MUST be first-class data structures, not chat messages:
  ```rust
  pub struct ExecutionPlan {
      pub id: PlanID,
      pub title: String,
      pub description: String,
      pub steps: Vec<PlanStep>,
      pub dependencies: Vec<(StepID, StepID)>,  // (prerequisite, dependent)
      pub created_at: DateTime<Utc>,
      pub status: PlanStatus,
  }
  ```

- **FR-002**: Plans MUST be serializable to JSON and persistable to `.flok/plans/{plan_id}.json`.

- **FR-003**: Plans MUST support a DAG structure for step dependencies:
  - Sequential steps: A → B → C (B depends on A, C depends on B)
  - Parallel steps: A → [B, C] → D (B and C depend on A, D depends on both B and C)
  - Independent steps: [A, B] (no dependencies, can run concurrently)

- **FR-004**: Each plan step MUST include:
  ```rust
  pub struct PlanStep {
      pub id: StepID,
      pub title: String,
      pub description: String,
      pub affected_files: Vec<PathBuf>,  // Estimated files to be modified
      pub agent_type: String,             // Which agent executes this step
      pub estimated_tokens: Option<u64>,
      pub status: StepStatus,
      pub checkpoint: Option<Checkpoint>,
  }
  
  pub enum StepStatus {
      Pending,
      Running,
      Completed,
      Failed(String),
      Skipped,
      RolledBack,
  }
  ```

- **FR-005**: Plan execution MUST create checkpoints before each step:
  - Checkpoint = git stash or snapshot of modified files
  - On failure, rollback restores the checkpoint
  - Checkpoints are cleaned up after successful plan completion

- **FR-006**: Plan execution MUST respect the DAG:
  - Steps with no unmet dependencies can run concurrently
  - A step starts only when all prerequisites are `Completed`
  - If a prerequisite fails, dependent steps are marked `Skipped`

- **FR-007**: Plans MUST support human approval gates:
  - **Default**: Show plan in TUI, wait for `/approve` or Enter
  - **Auto-approve**: `--auto-approve` flag or `plan.auto_approve = true` in config
  - **Per-step approval**: `plan.step_approval = true` -- confirm before each step

- **FR-008**: Plans MUST be diffable: changing a plan and re-running should show what changed.

- **FR-009**: Plans MUST be resumable: partial execution state is persisted, and `--continue` picks up where it left off.

- **FR-010**: The plan agent (read-only) MUST be distinct from build agents (read-write):
  - Plan agent: uses `plan` tier model (reasoning), read-only tools, produces the plan
  - Build agent(s): use `build` tier model, full tools, execute plan steps
  - This separation prevents the plan agent from accidentally modifying files

- **FR-011**: Flok MUST provide these plan-related tools/commands:

  | Tool/Command | Description |
  |-------------|-------------|
  | `/plan` | Enter plan mode (plan agent with read-only tools) |
  | `/approve` | Approve the current plan and start execution |
  | `/execute-plan [plan_id]` | Execute a saved plan |
  | `/show-plan [plan_id]` | Display a plan's steps and status |
  | `/rollback [step_id]` | Rollback to checkpoint before a step |
  | `plan_create` (tool) | Create a plan data structure (available to plan agent) |
  | `plan_update` (tool) | Update plan step status during execution |

- **FR-012**: Plan execution MUST emit events for TUI rendering:
  - `PlanCreated { plan_id }`
  - `StepStarted { plan_id, step_id }`
  - `StepCompleted { plan_id, step_id }`
  - `StepFailed { plan_id, step_id, error }`
  - `PlanCompleted { plan_id }`

### Key Entities

```rust
pub type PlanID = ULID;
pub type StepID = ULID;

pub struct ExecutionPlan {
    pub id: PlanID,
    pub session_id: SessionID,
    pub title: String,
    pub description: String,
    pub steps: Vec<PlanStep>,
    pub dependencies: Vec<Dependency>,
    pub status: PlanStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct Dependency {
    pub prerequisite: StepID,
    pub dependent: StepID,
}

pub enum PlanStatus {
    Draft,         // Created, not yet approved
    Approved,      // Approved, ready to execute
    Executing,     // Currently running
    Completed,     // All steps completed
    Failed,        // One or more steps failed
    Cancelled,     // User cancelled
}

pub struct Checkpoint {
    pub step_id: StepID,
    pub snapshot: CheckpointData,
    pub created_at: DateTime<Utc>,
}

pub enum CheckpointData {
    GitStash(String),            // Stash reference
    FileSnapshots(Vec<FileSnapshot>),
}

pub struct FileSnapshot {
    pub path: PathBuf,
    pub content: Vec<u8>,        // Original content before step
    pub existed: bool,           // Whether the file existed before
}
```

## Design

### Overview

The plan-then-execute architecture separates reasoning from action at the system level. A read-only plan agent analyzes the codebase and produces a structured execution plan (DAG). After human review, build agents execute each step with checkpoints for rollback. This design ensures: (1) the user understands what will happen before it happens, (2) failures can be recovered from without restarting, and (3) complex tasks are decomposed into verifiable units.

### Detailed Design

#### Plan Creation Flow

```
User: "Refactor auth to use JWT"
  → Session engine routes to plan agent (read-only tools + plan_create tool)
  → Plan agent reads codebase (read, glob, grep, lsp tools)
  → Plan agent creates plan:
      Step 1: "Add JWT dependencies to Cargo.toml" (affects: Cargo.toml)
      Step 2: "Create JWT token module" (affects: src/auth/jwt.rs) [depends: 1]
      Step 3: "Update auth middleware" (affects: src/middleware/auth.rs) [depends: 2]
      Step 4: "Update user routes" (affects: src/routes/users.rs) [depends: 2]
      Step 5: "Add JWT tests" (affects: tests/auth_test.rs) [depends: 3, 4]
      Step 6: "Update documentation" (affects: docs/auth.md) [depends: 3]
  → Plan persisted to .flok/plans/{plan_id}.json
  → TUI displays plan for review
  → User approves
```

#### DAG Execution Engine

```rust
pub struct PlanExecutor {
    state: Arc<AppState>,
    plan: Arc<RwLock<ExecutionPlan>>,
}

impl PlanExecutor {
    pub async fn execute(
        &self,
        cancel: CancellationToken,
    ) -> Result<PlanStatus> {
        loop {
            // Find all steps whose dependencies are met
            let ready_steps = self.ready_steps().await;

            if ready_steps.is_empty() {
                // Check if we're done or stuck
                return if self.all_completed().await {
                    Ok(PlanStatus::Completed)
                } else if self.any_failed().await {
                    Ok(PlanStatus::Failed)
                } else {
                    Err(anyhow!("Plan is stuck: no ready steps but not all completed"))
                };
            }

            // Execute ready steps concurrently
            let handles: Vec<_> = ready_steps.into_iter().map(|step| {
                let executor = self.clone();
                let cancel = cancel.child_token();
                tokio::spawn(async move {
                    executor.execute_step(step, cancel).await
                })
            }).collect();

            // Wait for all concurrent steps
            let results = futures::future::join_all(handles).await;

            // Check for failures
            for result in results {
                match result {
                    Ok(Ok(())) => continue,
                    Ok(Err(e)) => {
                        tracing::error!("Step failed: {}", e);
                        // Don't abort other running steps immediately
                        // The next iteration will skip dependent steps
                    }
                    Err(e) => {
                        tracing::error!("Step panicked: {}", e);
                    }
                }
            }

            if cancel.is_cancelled() {
                return Ok(PlanStatus::Cancelled);
            }
        }
    }

    async fn execute_step(
        &self,
        step: PlanStep,
        cancel: CancellationToken,
    ) -> Result<()> {
        // 1. Create checkpoint
        let checkpoint = self.create_checkpoint(&step).await?;

        // 2. Update status to Running
        self.update_step_status(step.id, StepStatus::Running).await?;
        self.state.bus.send(BusEvent::StepStarted {
            plan_id: self.plan.read().await.id,
            step_id: step.id,
        });

        // 3. Spawn build agent for this step
        let agent = self.state.agents.get(&step.agent_type)?;
        let session = Session::create(
            &self.state.db,
            Some(self.plan.read().await.session_id),
            &agent,
        ).await?;

        let prompt = format!(
            "Execute this plan step:\n\nTitle: {}\nDescription: {}\nAffected files: {:?}\n\n\
             Important: Only modify the files listed above. \
             Do not make changes beyond the scope of this step.",
            step.title, step.description, step.affected_files
        );

        let result = run_prompt_loop(
            self.state.clone(),
            session.id,
            prompt,
            &agent,
            cancel,
        ).await;

        match result {
            Ok(_) => {
                self.update_step_status(step.id, StepStatus::Completed).await?;
                self.state.bus.send(BusEvent::StepCompleted {
                    plan_id: self.plan.read().await.id,
                    step_id: step.id,
                });
                Ok(())
            }
            Err(e) => {
                self.update_step_status(
                    step.id,
                    StepStatus::Failed(e.to_string()),
                ).await?;
                self.state.bus.send(BusEvent::StepFailed {
                    plan_id: self.plan.read().await.id,
                    step_id: step.id,
                    error: e.to_string(),
                });

                // Rollback this step
                self.rollback_to_checkpoint(&checkpoint).await?;

                Err(e)
            }
        }
    }

    fn ready_steps(&self) -> Vec<PlanStep> {
        let plan = self.plan.blocking_read();
        plan.steps.iter()
            .filter(|s| s.status == StepStatus::Pending)
            .filter(|s| {
                plan.dependencies.iter()
                    .filter(|d| d.dependent == s.id)
                    .all(|d| {
                        plan.steps.iter()
                            .find(|s| s.id == d.prerequisite)
                            .map(|s| s.status == StepStatus::Completed)
                            .unwrap_or(false)
                    })
            })
            .cloned()
            .collect()
    }
}
```

#### Checkpoint System

```rust
impl PlanExecutor {
    async fn create_checkpoint(&self, step: &PlanStep) -> Result<Checkpoint> {
        let snapshots: Vec<FileSnapshot> = futures::future::join_all(
            step.affected_files.iter().map(|path| async {
                let full_path = self.state.project.root.join(path);
                if full_path.exists() {
                    let content = tokio::fs::read(&full_path).await?;
                    Ok(FileSnapshot {
                        path: path.clone(),
                        content,
                        existed: true,
                    })
                } else {
                    Ok(FileSnapshot {
                        path: path.clone(),
                        content: Vec::new(),
                        existed: false,
                    })
                }
            })
        ).await.into_iter().collect::<Result<Vec<_>>>()?;

        Ok(Checkpoint {
            step_id: step.id,
            snapshot: CheckpointData::FileSnapshots(snapshots),
            created_at: Utc::now(),
        })
    }

    async fn rollback_to_checkpoint(&self, checkpoint: &Checkpoint) -> Result<()> {
        match &checkpoint.snapshot {
            CheckpointData::FileSnapshots(snapshots) => {
                for snapshot in snapshots {
                    let full_path = self.state.project.root.join(&snapshot.path);
                    if snapshot.existed {
                        tokio::fs::write(&full_path, &snapshot.content).await?;
                    } else if full_path.exists() {
                        tokio::fs::remove_file(&full_path).await?;
                    }
                }
            }
            CheckpointData::GitStash(stash_ref) => {
                tokio::process::Command::new("git")
                    .args(["stash", "pop", stash_ref])
                    .current_dir(&self.state.project.root)
                    .output()
                    .await?;
            }
        }

        Ok(())
    }
}
```

#### TUI Plan Display

```
┌─ Execution Plan: Refactor auth to JWT ─────────────────────┐
│                                                              │
│  ● Step 1: Add JWT dependencies            [completed]      │
│  ◌ Step 2: Create JWT token module         [running]        │
│  ○ Step 3: Update auth middleware          [pending]         │
│  ○ Step 4: Update user routes              [pending]         │
│  ○ Step 5: Add JWT tests                   [pending]         │
│  ○ Step 6: Update documentation            [pending]         │
│                                                              │
│  Dependencies: 1→2, 2→3, 2→4, 3+4→5, 3→6                  │
│  Estimated cost: ~$0.15 | Est. time: ~3 min                │
│                                                              │
│  [Approve]  [Edit]  [Cancel]                                │
└──────────────────────────────────────────────────────────────┘
```

### Alternatives Considered

1. **Plans as markdown files only**: Rejected. A data structure is diffable, resumable, and can be used programmatically. Markdown is the display format, not the storage format.
2. **No DAG, just sequential steps**: Rejected. Sequential-only plans are slower (can't parallelize independent steps) and less expressive.
3. **Git-based checkpoints only**: Rejected as the sole mechanism. Git stash works but requires a clean working tree. File snapshots are more flexible and work with dirty working trees.
4. **Let the LLM decide parallelism at runtime**: Rejected. Parallelism should be declared in the plan so the user can review it, and the executor can enforce it.
5. **No human approval (fully autonomous)**: Rejected as default. Auto-approve is opt-in for CI/trusted environments. Human review is the default for safety.

## Success Criteria

- **SC-001**: Plan creation (analysis + DAG construction) completes in < 60s for a typical task
- **SC-002**: Checkpoint creation (file snapshots) completes in < 100ms per step
- **SC-003**: Rollback restores exact file state within 50ms
- **SC-004**: Plan serialization/deserialization < 10ms
- **SC-005**: DAG execution correctly parallelizes independent steps (verified by timing)
- **SC-006**: Resumed plans correctly skip completed steps and continue from the right point

## Assumptions

- Most tasks can be decomposed into < 20 plan steps
- The plan agent can produce accurate affected-file estimates (not always perfect)
- File-level checkpoints are sufficient (we don't need line-level or AST-level snapshots)
- Users want to review plans before execution (in interactive mode)
- CI mode users are comfortable with auto-approve

## Open Questions

- Should plans support conditional steps (if step A finds X, then do B, else do C)?
- Should the plan agent be able to update the plan mid-execution (adaptive planning)?
- Should we integrate with git worktrees (spec-010) for plan steps, giving each step its own worktree?
- Should plans support "verification steps" that check the output of previous steps (e.g., run tests)?
- How should plan cost estimates be computed? (Token estimation per step based on affected file sizes?)
