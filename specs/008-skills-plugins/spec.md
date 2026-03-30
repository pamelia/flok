# Feature Specification: Skills, Agents, Memory & Extensibility

**Feature Branch**: `008-skills-plugins`
**Created**: 2026-03-28
**Status**: Draft

## User Scenarios & Testing

### User Story 1 - Developer Uses Built-in Skills (Priority: P0)
**Why this priority**: Built-in skills are flok's opinionated workflows -- the reason users choose flok.
**Acceptance Scenarios**:
1. **Given** the user types `/spec-review path/to/spec.md`, **When** the skill loads, **Then** flok creates a review team, spawns specialist agents, and orchestrates a three-phase review.
2. **Given** the user types `/code-review #42`, **When** the skill loads, **Then** flok fetches the PR diff, selects appropriate reviewers based on scope, and produces a structured review.
3. **Given** the user types `/self-review-loop #42`, **When** the skill loads, **Then** flok iteratively reviews and fixes the PR until clean or max turns.

### User Story 2 - Developer Defines Custom Agents (Priority: P1)
**Why this priority**: Users need to customize agent behavior for their projects.
**Acceptance Scenarios**:
1. **Given** a `.flok/agents/my-reviewer.md` file exists with YAML frontmatter, **When** flok starts, **Then** the agent is registered and available for use via `subagent_type: "my-reviewer"`.
2. **Given** a global agent is defined in `~/.config/flok/agents/my-agent.md`, **When** a project agent has the same name, **Then** the project-level definition takes precedence.
3. **Given** an agent has `memory: local`, **When** the agent runs across multiple sessions, **Then** it accumulates project-specific knowledge in persistent memory.

### User Story 3 - Developer Creates Custom Skills (Priority: P1)
**Why this priority**: Skills are reusable workflows that save time.
**Acceptance Scenarios**:
1. **Given** a `.flok/skills/deploy/SKILL.md` file exists, **When** the user types `/deploy staging`, **Then** the skill's instructions are loaded into the agent's context with `$ARGUMENTS` replaced by "staging".
2. **Given** the skill lists available skills, **When** displayed, **Then** both built-in and custom skills are shown with their descriptions.

### User Story 4 - Agent Memory Persists Across Sessions (Priority: P1)
**Why this priority**: Without memory, agents re-learn project conventions every session.
**Acceptance Scenarios**:
1. **Given** a reviewer agent writes to memory: "This project uses protobuf for API contracts", **When** the same agent runs in a future session, **Then** it reads this memory at startup and applies the knowledge.
2. **Given** memory exceeds 100KB, **When** the agent writes, **Then** the content is truncated with a warning.
3. **Given** an agent has memory, **When** the user inspects it, **Then** they can view the memory content via a CLI command or TUI panel.

### Edge Cases
- Skill markdown has invalid YAML frontmatter: log warning, skip skill
- Agent markdown references nonexistent model: fall back to default model
- Memory write conflicts (two agents same key): last-write-wins (no CRDT needed for single-user)
- Skill file is modified while flok is running: pick up changes on next skill invocation (no hot-reload needed for skills)
- Custom agent has permission to call `team_create`: allowed if permissions grant it
- Circular skill references (skill A calls skill B calls skill A): detect and error after depth 5

## Requirements

### Functional Requirements

#### Skills

- **FR-001**: Flok MUST ship with these built-in skills (compiled into the binary):

  | Skill | Command | Description |
  |-------|---------|-------------|
  | `spec-review` | `/spec-review` | Three-phase multi-agent spec review |
  | `code-review` | `/code-review` | Parallel PR code review |
  | `self-review-loop` | `/self-review-loop` | Iterative PR improvement |
  | `handle-pr-feedback` | `/handle-pr-feedback` | Process PR review comments |
  | `issue-to-spec` | `/issue-to-spec` | Issue-to-spec pipeline |

- **FR-002**: Skills MUST be discoverable from these locations (higher takes precedence):
  1. Built-in skills (compiled)
  2. `.flok/skills/*/SKILL.md` (project-local)
  3. `~/.config/flok/skills/*/SKILL.md` (global user)
- **FR-003**: Skill files MUST follow the format:
  ```markdown
  ---
  name: my-skill
  description: "What this skill does"
  ---

  Instructions for the agent when this skill is invoked.
  Use $ARGUMENTS to reference the user's input.
  ```
- **FR-004**: The `skill` tool MUST inject the skill's instructions into the agent's context when invoked.

#### Agents

- **FR-005**: Flok MUST ship with 15 built-in agent types (see spec-005 for full list).
- **FR-006**: Custom agents MUST be definable as markdown files:
  ```markdown
  ---
  name: my-agent
  description: "What this agent does"
  mode: subagent
  model: anthropic/claude-sonnet-4
  memory: local
  permission:
    "*": deny
    read: allow
    glob: allow
    grep: allow
    send_message: allow
    team_task: allow
    agent_memory: allow
  ---

  System prompt for this agent.
  ```
- **FR-007**: Agent definitions MUST be discoverable from:
  1. Built-in agents (compiled)
  2. `.flok/agents/*.md` (project-local)
  3. `~/.config/flok/agents/*.md` (global user)
- **FR-008**: Agent definitions MUST support these frontmatter fields:
  - `name` (required): Agent identifier
  - `description` (required): Human-readable description
  - `mode` (required): `primary`, `subagent`, or `internal`
  - `model` (optional): Model override (e.g., `anthropic/claude-haiku-4.5`)
  - `memory` (optional): `none` (default) or `local`
  - `permission` (optional): Tool permission overrides
  - `color` (optional): TUI display color
  - `hidden` (optional): Hide from user-facing lists

#### Agent Memory

- **FR-009**: Agent memory MUST be stored in SQLite, keyed by `(project_id, agent_name)`.
- **FR-010**: Agent memory MUST support three operations:
  - `read(agent)` → `Option<String>`
  - `write(agent, content)` → `()` (replaces entire content)
  - `append(agent, content)` → `()` (appends to existing)
- **FR-011**: Agent memory MUST be capped at 100KB per `(project_id, agent_name)` entry.
- **FR-012**: Agent memory MUST be injected into the agent's system prompt when `memory: local` is configured:
  ```
  # Agent Memory
  <content from agent_memory table>
  ```
- **FR-013**: Agent memory MUST persist across sessions, teams, and flok restarts.
- **FR-014**: Agent memory MUST be readable/writable via the `agent_memory` tool during agent execution.

#### System Prompts

- **FR-015**: System prompts MUST be assembled from these components (in order):
  1. Provider-specific base prompt (Anthropic, OpenAI, Gemini, default)
  2. Agent-specific prompt (from agent definition)
  3. Environment context (model, working directory, git status, platform, date)
  4. Loaded skills (descriptions of available skills)
  5. Agent memory (if `memory: local`)
  6. AGENTS.md content (if present in project root or `.flok/`)
  7. Active skill instructions (if a skill was just loaded)
- **FR-016**: AGENTS.md files MUST be discovered from:
  - `AGENTS.md` in project root
  - `.flok/AGENTS.md`
  - `~/.config/flok/AGENTS.md` (global)
  These are concatenated into the system prompt, providing project-wide agent instructions.

### Key Entities

```rust
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub content: String,  // Markdown body (instructions)
    pub source: SkillSource,
}

pub enum SkillSource {
    BuiltIn,
    Project(PathBuf),
    Global(PathBuf),
}

pub struct AgentMemoryEntry {
    pub project_id: ProjectID,
    pub agent: String,
    pub content: String,
    pub updated_at: DateTime<Utc>,
}

pub struct SystemPrompt {
    pub base: String,          // Provider-specific
    pub agent: String,         // Agent definition
    pub environment: String,   // Runtime context
    pub skills: String,        // Available skill descriptions
    pub memory: Option<String>,// Agent memory
    pub agents_md: Option<String>, // AGENTS.md content
    pub active_skill: Option<String>, // Currently loaded skill
}

impl SystemPrompt {
    pub fn assemble(&self) -> String {
        // Concatenate all components with clear section markers
        // Order matters for prompt cache hit optimization:
        // stable components first, dynamic components last
    }
}
```

## Design

### Overview

Flok's extensibility model is intentionally constrained compared to opencode's plugin system. Instead of a general-purpose plugin SDK with lifecycle hooks, flok provides two extension points: **agents** (markdown files with system prompts and permissions) and **skills** (markdown files with workflow instructions). This is simpler, faster to load, and easier to understand.

The key design decision: **built-in beats configurable**. The 8 specialist reviewer agents, 5 workflow skills, and multi-agent team orchestration are all compiled into the binary. Users can override or extend, but the defaults work out of the box.

### Detailed Design

#### Skill Discovery and Loading

```rust
pub struct SkillRegistry {
    skills: HashMap<String, SkillDefinition>,
}

impl SkillRegistry {
    pub fn discover(project_root: &Path) -> Self {
        let mut skills = HashMap::new();

        // 1. Built-in skills (highest precedence for names, but custom can add new ones)
        for builtin in BuiltInSkills::all() {
            skills.insert(builtin.name.clone(), builtin);
        }

        // 2. Global skills
        let global_dir = dirs::config_dir().join("flok/skills");
        discover_skills_from_dir(&global_dir, SkillSource::Global, &mut skills);

        // 3. Project skills (override globals)
        let project_dir = project_root.join(".flok/skills");
        discover_skills_from_dir(&project_dir, SkillSource::Project, &mut skills);

        Self { skills }
    }
}

fn discover_skills_from_dir(
    dir: &Path,
    source: SkillSource,
    skills: &mut HashMap<String, SkillDefinition>,
) {
    // Walk dir, find */SKILL.md files
    // Parse YAML frontmatter + markdown body
    // Insert into map (overwrites lower-precedence entries with same name)
}
```

#### Built-in Skill Compilation

Built-in skills are embedded as string constants via `include_str!`:

```rust
pub struct BuiltInSkills;

impl BuiltInSkills {
    pub fn all() -> Vec<SkillDefinition> {
        vec![
            SkillDefinition {
                name: "spec-review".into(),
                description: "Orchestrates a three-phase parallel spec review...".into(),
                content: include_str!("skills/spec-review.md").into(),
                source: SkillSource::BuiltIn,
            },
            // ... code-review, self-review-loop, handle-pr-feedback, issue-to-spec
        ]
    }
}
```

#### Agent Definition Loading

```rust
pub struct AgentRegistry {
    agents: HashMap<String, AgentConfig>,
}

impl AgentRegistry {
    pub fn discover(project_root: &Path) -> Self {
        let mut agents = HashMap::new();

        // 1. Built-in agents
        for builtin in BuiltInAgents::all() {
            agents.insert(builtin.name.clone(), builtin);
        }

        // 2. Global agents
        discover_agents_from_dir(&dirs::config_dir().join("flok/agents"), &mut agents);

        // 3. Project agents (override globals and built-ins)
        discover_agents_from_dir(&project_root.join(".flok/agents"), &mut agents);

        Self { agents }
    }
}
```

Agent markdown files are parsed with `gray_matter` (YAML frontmatter) or a simple custom YAML parser to avoid heavy dependencies.

#### System Prompt Assembly Order (Cache Optimization)

The system prompt is assembled with **stable components first** to maximize Anthropic prompt cache hits:

```
┌─────────────────────────────────────┐
│ 1. Provider base prompt             │ ← STABLE (changes only on flok update)
│ 2. Agent system prompt              │ ← STABLE (changes only on agent edit)
│ 3. AGENTS.md content                │ ← STABLE (changes rarely)
│ 4. Agent memory                     │ ← SEMI-STABLE (changes between sessions)
├─ cache_control: ephemeral ──────────┤
│ 5. Environment context              │ ← DYNAMIC (changes each session)
│ 6. Available skills list            │ ← SEMI-STABLE
│ 7. Active skill instructions        │ ← DYNAMIC (changes per invocation)
└─────────────────────────────────────┘
```

By placing stable components at the top, we ensure the first ~80% of the system prompt is cache-eligible across turns.

#### Agent Memory Persistence

```sql
CREATE TABLE agent_memory (
    project_id TEXT NOT NULL,
    agent TEXT NOT NULL,
    content TEXT NOT NULL,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (project_id, agent)
);
```

The memory is injected into the system prompt as a clearly delineated section:

```
## Agent Memory

The following is your persistent memory for this project. It contains patterns,
conventions, and learnings from previous sessions. Use it to inform your work.

<memory content here>
```

#### No General Plugin System at v1.0 (Intentional, WASM Deferred)

Unlike opencode's plugin SDK with lifecycle hooks, flok v1.0 does **not** ship with a general-purpose plugin system. The reasoning:

1. **Complexity**: Plugin lifecycle management (loading, versioning, error handling) is a significant engineering investment.
2. **Performance**: Plugin hooks add latency to every operation.
3. **Simplicity**: Markdown-based agents and skills cover 95% of extension use cases.
4. **Security**: No arbitrary code execution from third-party packages.

Users who need custom tool implementations can:
- Use MCP servers (see spec-009) for external tool definitions
- Write a custom agent prompt that uses `bash` to call their tools
- Fork flok and add built-in tools (it's open source)

#### WASM Plugin System (Post-v1.0, Killer Feature #8)

The killer-features analysis identified WASM-based plugins as a significant differentiator. While deferred from v1.0, the architecture should be WASM-ready:

**Future WASM plugin capabilities:**
- Plugins compile to `.wasm` -- language-agnostic (write in Rust, Go, Python, whatever compiles to WASM)
- Sandboxed execution via `wasmtime` -- a buggy plugin can't crash the host
- Hot-reloadable -- swap plugins without restarting flok
- Typed interface via `wit-bindgen` (WASM Interface Types)

**Planned hook points** (design the internal architecture with these in mind even at v1.0):
- `on_message` -- after each user/assistant message
- `on_tool_call` -- before/after tool execution
- `on_compaction` -- when context is compacted
- `on_session_start` / `on_session_end`
- `on_agent_spawn` -- when a new agent is created

**v1.0 preparation:** Define internal hook traits now (e.g., `trait Hook: Send + Sync`) that are used by built-in components. Post-v1.0, WASM plugins implement the same trait via `wit-bindgen` bindings. This makes the transition from "hardcoded hooks" to "WASM plugins" a non-breaking change.

```rust
// v1.0: Internal hook trait (used by built-in components)
#[async_trait]
pub trait Hook: Send + Sync {
    async fn on_message(&self, _event: &MessageEvent) -> Result<()> { Ok(()) }
    async fn on_tool_call(&self, _event: &ToolCallEvent) -> Result<()> { Ok(()) }
    async fn on_compaction(&self, _event: &CompactionEvent) -> Result<()> { Ok(()) }
    async fn on_session_start(&self, _event: &SessionEvent) -> Result<()> { Ok(()) }
    async fn on_session_end(&self, _event: &SessionEvent) -> Result<()> { Ok(()) }
    async fn on_agent_spawn(&self, _event: &AgentSpawnEvent) -> Result<()> { Ok(()) }
}

// Post-v1.0: WASM plugins implement the same trait via wit-bindgen
// This enables the community to build memory systems, notification integrations,
// custom tools, and review workflows without forking the core.
```

### Alternatives Considered

1. **Full plugin SDK (like opencode)**: Rejected. The plugin system adds significant complexity for marginal benefit. MCP servers provide the same extensibility with better isolation.
2. **Lua scripting for custom tools**: Rejected. Adds a runtime dependency and complexity. MCP or bash covers the same use cases.
3. **WASM-based plugins**: Rejected. Too complex for the benefit. Revisit post-1.0 if demand warrants.
4. **Dynamic agent loading via HTTP (remote agents)**: Deferred. Interesting for team-shared agent definitions, but adds network dependency.

## Success Criteria

- **SC-001**: All 5 built-in skills work out of the box with zero configuration
- **SC-002**: Custom agent discovery and loading < 10ms for a directory with 50 agent files
- **SC-003**: System prompt assembly < 1ms
- **SC-004**: Agent memory read/write < 5ms per operation
- **SC-005**: Skill discovery (built-in + custom) < 10ms

## Assumptions

- Markdown-based agent and skill definitions are expressive enough for most use cases
- YAML frontmatter parsing is fast enough (small files, simple schemas)
- 100KB memory cap per agent is sufficient for project-specific knowledge
- Users accept that custom tools require MCP servers (not inline code)

## Open Questions

- Should we support remote skill/agent repositories (git URLs)?
- Should agents be able to read other agents' memory?
- Should we add a `flok agents list` CLI command for discoverability?
- Should we support skill chaining (skill A invokes skill B) as a first-class concept?
