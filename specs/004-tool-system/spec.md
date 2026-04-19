# Feature Specification: Tool System

**Feature Branch**: `004-tool-system`
**Created**: 2026-03-28
**Status**: Accepted (2026-04-19 — feature shipped; spec retroactively locked to match built reality.)

## User Scenarios & Testing

### User Story 1 - LLM Invokes Built-in Tools (Priority: P0)
**Why this priority**: Tools are how the LLM interacts with the codebase.
**Acceptance Scenarios**:
1. **Given** the LLM requests a `read` tool call with a file path, **When** the tool executes, **Then** the file contents are returned within 5ms for files < 1MB.
2. **Given** the LLM requests a `bash` tool call, **When** the command executes, **Then** stdout/stderr are captured and returned, with a configurable timeout (default 120s).
3. **Given** the LLM requests a `glob` tool call, **When** pattern matching runs, **Then** results are returned sorted by modification time within 50ms for projects with < 100K files.

### User Story 2 - Permission System Guards Dangerous Operations (Priority: P0)
**Why this priority**: Unguarded file writes or shell commands are dangerous.
**Acceptance Scenarios**:
1. **Given** the `edit` tool is called on a file, **When** permissions are configured to `ask` for edits, **Then** the TUI prompts the user for confirmation before executing.
2. **Given** a `bash` tool call matches a deny pattern (e.g., `rm -rf /`), **When** permission evaluation runs, **Then** the call is blocked immediately without user prompt.
3. **Given** a sub-agent has restricted permissions, **When** it tries to call `write`, **Then** the call is denied with a clear error message explaining the restriction.

### User Story 3 - Developer Switches to Plan Mode (Priority: P0)
**Why this priority**: Users need a safe exploration mode where the LLM reasons and plans without modifying files.
**Acceptance Scenarios**:
1. **Given** the user activates plan mode (via keybind or `/plan` command), **When** the LLM tries to call `edit`, `write`, or `bash` (non-read-only), **Then** those tools are unavailable and the LLM is told to plan instead of execute.
2. **Given** plan mode is active, **When** the LLM produces a plan, **Then** it can write its plan to a dedicated plan file (`.flok/plan.md`) via a special `plan` tool that only accepts plan output.
3. **Given** the user deactivates plan mode, **When** they switch back to build mode, **Then** the full tool set is restored and the LLM can execute the plan it previously wrote.
4. **Given** the user starts flok with `--plan` flag, **When** the TUI launches, **Then** it starts in plan mode by default.

### User Story 4 - LLM Calls MCP Tools (Priority: P1)
**Why this priority**: MCP extends flok's capabilities via external servers.
**Acceptance Scenarios**:
1. **Given** an MCP server is configured with tools, **When** the LLM calls an MCP tool, **Then** the request is routed to the correct MCP server and the result returned.
2. **Given** an MCP server disconnects, **When** the LLM tries to call one of its tools, **Then** a clear error is returned indicating the server is unavailable.

### Edge Cases
- Tool call with invalid JSON arguments: return validation error, LLM retries
- Tool output exceeds 50KB: truncate with message indicating truncation
- Tool call to nonexistent tool: return error listing available tools
- Concurrent tool calls from parallel agents: each agent's tool context is isolated
- Doom loop: same tool called 3x with identical args → pause and ask user
- Tool execution panics: catch with `std::panic::catch_unwind`, return error

## Requirements

### Functional Requirements

- **FR-001**: Flok MUST provide the following built-in tools:

  | Tool | Description | Permission Default |
  |------|-------------|-------------------|
  | `read` | Read file contents with line numbers | allow |
  | `write` | Write/overwrite file contents | ask |
  | `edit` | Search-and-replace edit in a file | ask |
  | `glob` | Find files by glob pattern | allow |
  | `grep` | Search file contents by regex | allow |
  | `bash` | Execute shell commands | ask (with pattern matching) |
  | `task` | Spawn sub-agent (foreground or background) | allow |
  | `question` | Ask the user a question with options | allow |
  | `todowrite` | Write/update a todo list | allow |
  | `webfetch` | Fetch URL content | allow |
  | `skill` | Load a skill's instructions | allow |
  | `team_create` | Create an agent team | allow |
  | `team_delete` | Disband an agent team | allow |
  | `team_task` | CRUD on team task board | allow |
  | `send_message` | Send message to team member | allow |
  | `agent_memory` | Read/write persistent agent memory | allow |
  | `plan` | Write plan output (plan mode only) | allow |

- **FR-002**: Each tool MUST be defined as a Rust struct implementing a `Tool` trait:
  ```rust
  #[async_trait]
  pub trait Tool: Send + Sync {
      fn name(&self) -> &str;
      fn description(&self) -> &str;
      fn parameters_schema(&self) -> serde_json::Value; // JSON Schema
      async fn execute(
          &self,
          args: serde_json::Value,
          ctx: ToolContext,
      ) -> Result<ToolOutput>;
  }
  ```

- **FR-003**: Tool output MUST be truncated at 50KB with a truncation notice appended.
- **FR-004**: Tool descriptions MUST be loadable from external `.txt` files (for easy editing of system prompt text without recompilation).
- **FR-005**: The permission system MUST support glob-based rules per tool:
  ```toml
  [permission]
  "*" = "ask"             # Default: ask for everything
  "read" = "allow"        # Always allow reads
  "glob" = "allow"
  "grep" = "allow"
  "bash.git *" = "allow"  # Allow read-only git commands
  "bash.rm *" = "deny"    # Never allow rm
  "edit" = "ask"
  "write" = "ask"
  ```
- **FR-006**: Sub-agents MUST receive a filtered tool set based on their agent config's permission field.
- **FR-007**: The tool registry MUST support dynamic tool registration from MCP servers.
- **FR-008**: Tool call arguments MUST be validated against the JSON Schema before execution.
- **FR-009**: The `task` tool MUST support both foreground (blocking) and background (non-blocking) modes.
- **FR-010**: The `task` tool MUST support `team_id` parameter to register the spawned agent as a team member.
- **FR-011**: Doom loop detection MUST trigger after 3 identical tool calls (same name + same args hash).
- **FR-012**: Flok MUST support a **plan mode** that restricts the tool set to read-only operations:
  - **Disabled in plan mode**: `write`, `edit`, `bash` (except read-only commands matching `bash.git *`, `bash.ls *`, `bash.cat *`, `bash.rg *`), `apply_patch`
  - **Enabled in plan mode**: `read`, `glob`, `grep`, `bash` (read-only subset), `task`, `question`, `todowrite`, `webfetch`, `skill`, `plan`, `team_create`, `team_delete`, `team_task`, `send_message`, `agent_memory`
  - **The `plan` tool**: Available only in plan mode. Accepts structured plan output (markdown) and writes it to `.flok/plan.md` in the project root.
- **FR-013**: Plan mode MUST be activatable via:
  - CLI flag: `flok --plan`
  - Keybind: configurable (default `Ctrl+Shift+P`)
  - Slash command: `/plan` toggles plan mode
  - Agent: selecting the `plan` agent automatically activates plan mode
- **FR-014**: Switching between plan mode and build mode MUST be instantaneous (< 1ms) -- it only changes which tools are presented to the LLM on the next turn, no session restart needed.
- **FR-015**: The TUI MUST display a clear visual indicator when plan mode is active (e.g., `[PLAN]` badge in the status bar, distinct accent color).

### Key Entities

```rust
pub struct ToolContext {
    pub session_id: SessionID,
    pub message_id: MessageID,
    pub agent: String,
    pub cancel: CancellationToken,
    pub state: Arc<AppState>,
    pub permissions: PermissionSet,
}

pub struct ToolOutput {
    pub content: String,
    pub metadata: serde_json::Value,
    pub attachments: Vec<Attachment>,
}

pub struct PermissionRule {
    pub tool_pattern: String,    // "bash.git *", "edit", "*"
    pub action: PermissionAction,
}

pub enum PermissionAction {
    Allow,
    Deny,
    Ask,
}

pub struct PermissionSet {
    rules: Vec<PermissionRule>,
}
```

## Design

### Overview

The tool system is a registry of capabilities that the LLM can invoke during a conversation. Tools are registered at startup (built-in + MCP-discovered), filtered per agent based on permissions, and executed with isolation guarantees. The design prioritizes: (1) type safety in tool definitions, (2) fast execution for read-only tools, (3) permission enforcement as a cross-cutting concern.

### Detailed Design

#### Tool Registry

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    mcp_tools: DashMap<String, Arc<dyn Tool>>,  // Dynamic, from MCP
}

impl ToolRegistry {
    /// Get tools available for a specific agent
    pub fn tools_for_agent(&self, agent: &AgentConfig) -> Vec<ToolDefinition> {
        self.tools.iter()
            .chain(self.mcp_tools.iter())
            .filter(|(name, _)| agent.permissions.allows(name))
            .map(|(_, tool)| tool.definition())
            .collect()
    }
}
```

#### Tool Execution Pipeline

```
LLM requests tool call
  → Validate arguments against JSON Schema
  → Check permissions (Allow / Deny / Ask)
      → Deny: return error immediately
      → Ask: publish event to TUI, block until user responds
      → Allow: proceed
  → Execute tool with ToolContext
  → Truncate output if > 50KB
  → Persist ToolCallPart (state: Completed/Error)
  → Publish BusEvent::PartUpdated
  → Return result to LLM
```

#### Per-Process Tool Isolation (Inspired by Spacebot)

Different agent types get different tool sets. This is not just permission-based filtering -- certain tools are only *registered* for certain process types:

| Agent Type | Available Tools |
|------------|----------------|
| Primary – build mode | All tools (except `plan`) |
| Primary – plan mode | Read-only tools + plan + task + question + todowrite + team tools |
| Sub-agent (general) | All tools minus team management |
| Sub-agent (explore) | read, glob, grep, bash (read-only git only) |
| Team member | Agent's registered tools + send_message + team_task + agent_memory |
| Compaction (internal) | read only (summarization) |
| Utility (internal) | read, glob, grep |

#### Plan Mode: Safe Exploration

Plan mode is implemented as a tool filter applied at the `tools_for_agent` level, not as a separate agent type. This means any session can toggle between plan and build mode mid-conversation:

```rust
pub struct ToolFilter {
    mode: AgentMode,
    plan_mode: bool,
}

/// Tools blocked in plan mode
const PLAN_MODE_BLOCKED: &[&str] = &["write", "edit", "apply_patch"];

/// Bash commands allowed in plan mode (read-only subset)
const PLAN_MODE_BASH_ALLOW: &[&str] = &[
    "git ", "ls ", "cat ", "head ", "tail ", "rg ", "find ", "wc ",
    "tree ", "file ", "stat ", "du ", "df ", "env ", "echo ",
    "which ", "type ", "uname ", "pwd ", "date ",
];

impl ToolFilter {
    pub fn allows(&self, tool_name: &str, args: Option<&Value>) -> bool {
        if !self.plan_mode {
            return true; // Build mode: everything allowed
        }

        // Block write tools entirely
        if PLAN_MODE_BLOCKED.contains(&tool_name) {
            return false;
        }

        // Bash: only allow read-only commands
        if tool_name == "bash" {
            if let Some(args) = args {
                let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                return PLAN_MODE_BASH_ALLOW.iter().any(|prefix| cmd.starts_with(prefix));
            }
            return false;
        }

        true // All other tools allowed
    }
}
```

The `plan` tool itself is simple -- it writes structured plan output to a file:

```rust
pub struct PlanTool;

impl Tool for PlanTool {
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let content: String = serde_json::from_value(args["content"].clone())?;
        let plan_path = ctx.state.project.root.join(".flok/plan.md");
        tokio::fs::create_dir_all(plan_path.parent().unwrap()).await?;
        tokio::fs::write(&plan_path, &content).await?;
        Ok(ToolOutput {
            content: format!("Plan written to {}", plan_path.display()),
            ..Default::default()
        })
    }
}
```

When the user switches back to build mode, the LLM can read `.flok/plan.md` via the `read` tool and execute the plan step by step with the full tool set restored.

#### The `task` Tool: Sub-Agent Spawning

The `task` tool is the most complex built-in tool. It creates a child session and runs a prompt loop:

```rust
pub struct TaskTool;

impl Tool for TaskTool {
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: TaskParams = serde_json::from_value(args)?;

        // Resolve agent config
        let agent = ctx.state.agents.get(&params.subagent_type)?;

        // Create child session
        let child_session = Session::create(
            &ctx.state.db,
            ctx.session_id, // parent
            &agent,
        ).await?;

        if params.background {
            // Register as team member if team_id provided
            if let Some(team_id) = &params.team_id {
                let member_name = Team::add_member(
                    &ctx.state.db,
                    team_id,
                    child_session.id,
                    &agent.name,
                ).await?;
                // member_name may be disambiguated (e.g., "general-2")
            }

            // Spawn background task
            let state = ctx.state.clone();
            let cancel = ctx.cancel.child_token();
            tokio::spawn(async move {
                let result = run_prompt_loop(
                    state.clone(), child_session.id, params.prompt, &agent, cancel
                ).await;
                // On completion/failure, update team member status
                // Auto-send result to lead if team member
            });

            return Ok(ToolOutput {
                content: format!("Background task started: {}", child_session.id),
                ..Default::default()
            });
        }

        // Foreground: run and wait
        let result = run_prompt_loop(
            ctx.state.clone(), child_session.id, params.prompt, &agent, ctx.cancel.clone()
        ).await?;

        Ok(ToolOutput { content: result.summary, ..Default::default() })
    }
}
```

#### Concurrency Limiting for Background Agents

A global `Arc<Semaphore>` limits concurrent background agents:

```rust
static MAX_CONCURRENT_AGENTS: usize = 10; // Configurable via config.team.max_agents

pub struct BackgroundAgentLimiter {
    semaphore: Arc<Semaphore>,
}
```

Background `task` calls acquire a permit before spawning. If the limit is reached, the call blocks until a slot opens.

#### The `bash` Tool: Sandboxed Shell Execution

```rust
pub struct BashTool {
    timeout: Duration,      // Default 120s
    sandbox: SandboxMode,
}

impl Tool for BashTool {
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: BashParams = serde_json::from_value(args)?;

        // Permission check with command-level granularity
        // "bash.git status" matches pattern "bash.git *"
        ctx.permissions.check(&format!("bash.{}", params.command))?;

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&params.command)
            .current_dir(&params.workdir.unwrap_or(ctx.state.project.root.clone()))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // Apply sandbox restrictions based on mode
        self.sandbox.apply(&mut cmd, &ctx)?;

        let output = cmd.spawn()?;
        let result = tokio::time::timeout(self.timeout, output.wait_with_output()).await??;

        Ok(ToolOutput {
            content: format_command_output(&result),
            ..Default::default()
        })
    }
}
```

#### Sandboxed Tool Execution (Killer Feature #5)

A configurable sandbox system for controlling agent access to the system:

```toml
[sandbox]
mode = "approval"  # "permissive", "approval", or "locked"

[sandbox.locked]
filesystem_allow = ["./src", "./tests", "./docs"]  # Allowlist for file access
network = false                                      # No network access
write_outside_worktree = false                      # No writes outside project
```

```rust
pub enum SandboxMode {
    /// Full access (for solo dev, trusted environments)
    Permissive,
    /// Human-in-the-loop for destructive operations (default)
    Approval {
        destructive_patterns: Vec<String>,  // e.g., "rm", "git push", "docker rm"
    },
    /// Filesystem allowlist, network restrictions, no writes outside worktree
    Locked {
        filesystem_allow: Vec<PathBuf>,
        network: bool,
        write_outside_worktree: bool,
    },
}

impl SandboxMode {
    pub fn apply(
        &self,
        cmd: &mut tokio::process::Command,
        ctx: &ToolContext,
    ) -> Result<()> {
        match self {
            SandboxMode::Permissive => Ok(()),
            SandboxMode::Approval { destructive_patterns } => {
                // Check if command matches destructive patterns
                // If so, the permission system (Ask) will prompt the user
                Ok(())
            }
            SandboxMode::Locked { filesystem_allow, network, .. } => {
                // On macOS: use sandbox-exec with profile restricting access
                #[cfg(target_os = "macos")]
                {
                    let profile = generate_sandbox_profile(filesystem_allow, *network);
                    cmd.args(["sandbox-exec", "-p", &profile, "sh", "-c"]);
                }
                // On Linux: use seccomp/bubblewrap for process isolation
                #[cfg(target_os = "linux")]
                {
                    apply_seccomp_filter(cmd, filesystem_allow, *network)?;
                }
                Ok(())
            }
        }
    }
}
```

Rust's type system enforces sandbox boundaries at compile time. For container-level isolation, an optional Docker/container isolation mode can wrap each agent's commands in a lightweight container (similar to Agent of Empires' approach).

### Alternatives Considered

1. **Compile-time tool registration via macros**: Considered. Would enable zero-cost dispatch but prevents dynamic MCP tool registration. Rejected in favor of trait objects.
2. **WASM-based tool sandboxing**: Rejected. Adds complexity. OS-level process isolation (kill_on_drop, timeout) is sufficient for shell commands.
3. **Tool call batching (execute multiple tools in parallel)**: Deferred. OpenAI supports parallel tool calls. We'll process them sequentially initially and add parallel execution as an optimization.

## Success Criteria

- **SC-001**: `read` tool returns file contents in < 5ms for files < 1MB
- **SC-002**: `glob` tool returns results in < 50ms for projects with < 100K files
- **SC-003**: `grep` tool returns results in < 100ms using ripgrep-style search
- **SC-004**: Permission check overhead < 1μs per tool call
- **SC-005**: Tool JSON Schema validation < 100μs per call

## Assumptions

- Users accept that shell commands run in the project directory (not sandboxed by default)
- MCP tools follow the MCP specification faithfully
- Tool output truncation at 50KB is sufficient for all practical use cases
- The LLM handles tool errors gracefully and can retry or adapt

## Open Questions

- Should tool descriptions be customizable per-project (via config)?
- Should we add an `apply_patch` tool for unified diff application (like opencode does for GPT-5)?
- ~~Future: OS-level sandboxing (bubblewrap on Linux, sandbox-exec on macOS) like Spacebot?~~ **Decision: Defer.** For v0.0.1, use `SandboxMode::Permissive` (no OS-level sandboxing) with permission prompts (Allow/Deny/Always) as the safety mechanism. The `BashTool` already has a `sandbox: SandboxMode` field and `SandboxMode::apply()` method, so adding `Approval`/`Locked` modes later is a single-variant addition with zero refactoring of the tool execution path.
- Should plan mode persist across sessions, or reset to build mode on each new session?
