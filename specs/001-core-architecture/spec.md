# Feature Specification: Core Architecture

**Feature Branch**: `001-core-architecture`
**Created**: 2026-03-28
**Status**: Accepted (2026-04-19 — feature shipped; spec retroactively locked to match built reality.)

## User Scenarios & Testing

### User Story 1 - Developer Installs and Runs Flok (Priority: P0)
**Why this priority**: Without a working binary, nothing else matters.
**Acceptance Scenarios**:
1. **Given** a Rust toolchain is installed, **When** the user runs `cargo install flok`, **Then** a single `flok` binary is produced with all features embedded.
2. **Given** the binary is installed, **When** the user runs `flok` in a git repository, **Then** the TUI launches, detects the project, and is ready to accept input within 200ms.
3. **Given** no prior configuration exists, **When** flok starts for the first time, **Then** it creates XDG-compliant directories (`~/.config/flok/`, `~/.local/share/flok/`, `~/.cache/flok/`) and initializes the SQLite database.

### User Story 2 - Developer Configures Providers and Preferences (Priority: P0)
**Why this priority**: Users must be able to connect to their LLM providers.
**Acceptance Scenarios**:
1. **Given** a `flok.toml` exists in the project root, **When** flok starts, **Then** it merges project config with global config (`~/.config/flok/flok.toml`), with project taking precedence.
2. **Given** `ANTHROPIC_API_KEY` is set in the environment, **When** flok starts, **Then** the Anthropic provider is automatically available without any config file.
3. **Given** the config file is modified while flok is running, **When** the file watcher detects the change, **Then** config is hot-reloaded within 100ms without restarting.

### Edge Cases
- Config file has syntax errors: log warning, keep previous valid config
- XDG directories have restrictive permissions: fail fast with clear error message
- Database is locked by another flok instance: use WAL mode, busy timeout of 5s
- Multiple flok instances in the same project: share database via WAL, separate sessions

## Requirements

### Functional Requirements

- **FR-001**: Flok MUST compile to a single static binary with no runtime dependencies beyond libc.
- **FR-002**: Flok MUST use a Cargo workspace with the following crate layout:
  - `flok` (binary crate) -- CLI entry point, TUI, command routing
  - `flok-core` (library) -- session engine, provider system, tool system, agent teams, model routing
  - `flok-db` (library) -- database layer, migrations, schema
  - `flok-tui` (library) -- terminal UI components and rendering (real-time dashboard, agent panes)
  - `flok-mcp` (library) -- MCP client implementation
  - `flok-lsp` (library) -- LSP client for semantic code intelligence (see spec-011)
  - `flok-apply` (library) -- fast apply engine (tree-sitter AST merge), smart grep, context compaction (see spec-014)
- **FR-003**: Flok MUST follow XDG Base Directory Specification for all paths:
  - Data: `$XDG_DATA_HOME/flok/` (default: `~/.local/share/flok/`)
  - Config: `$XDG_CONFIG_HOME/flok/` (default: `~/.config/flok/`)
  - Cache: `$XDG_CACHE_HOME/flok/` (default: `~/.cache/flok/`)
  - State: `$XDG_STATE_HOME/flok/` (default: `~/.local/state/flok/`)
- **FR-004**: Flok MUST support TOML configuration with the following precedence (highest to lowest):
  1. Environment variables (`FLOK_*`)
  2. Inline config (`FLOK_CONFIG_CONTENT`)
  3. Project config (`flok.toml` in project root)
  4. `.flok/` directories (`.flok/flok.toml`)
  5. Global config (`~/.config/flok/flok.toml`)
- **FR-005**: Flok MUST hot-reload configuration on file change using the `notify` crate, applying changes via `arc_swap::ArcSwap` for lock-free reads.
- **FR-006**: Flok MUST detect the project root by walking up from CWD to find `.git/`, `Cargo.toml`, `package.json`, `go.mod`, or similar markers.
- **FR-007**: Flok MUST register each project in a `project` table on first encounter, assigning a stable `ProjectID`.

### Key Entities

```
ProjectID    = ULID (ascending, sortable)
SessionID    = ULID (ascending)
MessageID    = ULID (ascending)
PartID       = ULID (ascending)
TeamID       = ULID (ascending, prefix: "team_")
TeamTaskID   = ULID (ascending, prefix: "ttask_")
MemoryID     = ULID (ascending, prefix: "mem_")
```

All IDs are ULID-based for chronological sortability and uniqueness without coordination.

## Design

### Overview

Flok follows a **layered architecture** inspired by opencode's Effect-TS service pattern and Spacebot's dependency bundle pattern, adapted to idiomatic Rust:

```
┌─────────────────────────────────────────────┐
│                  CLI / TUI                   │  flok (bin)
├─────────────────────────────────────────────┤
│               HTTP API (axum)                │  flok-core
├─────────────────────────────────────────────┤
│  Session Engine │ Provider │ Tool │ Agent    │  flok-core
│     Teams       │  System  │ Sys  │ System   │
├─────────────────────────────────────────────┤
│              Database Layer                   │  flok-db
│         SQLite │ redb │ LanceDB              │
├─────────────────────────────────────────────┤
│              MCP Client                       │  flok-mcp
└─────────────────────────────────────────────┘
```

### Detailed Design

#### Dependency Injection: The `AppState` Pattern

Instead of opencode's Effect-TS service layer or Spacebot's `AgentDeps` bundle, flok uses a typed `AppState` that is constructed at startup and passed via `Arc`:

```rust
pub struct AppState {
    pub config: ArcSwap<Config>,
    pub db: Database,
    pub providers: ArcSwap<ProviderRegistry>,
    pub router: ModelRouter,          // Intelligent model routing (spec-012)
    pub tools: ToolRegistry,
    pub agents: AgentRegistry,
    pub bus: EventBus,
    pub mcp: McpManager,
    pub lsp: LspManager,             // LSP integration (spec-011)
    pub memory: MemoryStore,
    pub worktrees: WorktreeManager,   // Git worktree isolation (spec-010)
    pub fast_apply: FastApplyEngine,  // Tree-sitter fast apply (spec-014)
    pub smart_grep: SmartGrepEngine,  // AST-aware search (spec-014)
    pub compaction: CompactionEngine, // Tiered context compaction (spec-014)
    pub project: Project,
}
```

`ArcSwap` is used for any field that can be hot-reloaded. All other fields are either `Arc<T>` internally or use interior mutability (`RwLock`, `DashMap`) where concurrent mutation is required.

#### Database Bundle

Following Spacebot's three-database strategy:

```rust
pub struct Database {
    pub sqlite: SqlitePool,         // Relational data (sessions, messages, teams, events)
    pub kv: Arc<redb::Database>,    // Key-value (settings, encrypted secrets)
    pub lance: lancedb::Connection, // Vector + FTS (memory embeddings)
}
```

SQLite uses WAL mode, 64MB cache, foreign keys ON, 5s busy timeout. Migrations run at startup via embedded SQL files (`sqlx::migrate!`).

#### Event Bus

A typed pub/sub event bus using `tokio::broadcast`:

```rust
pub struct EventBus {
    sender: broadcast::Sender<BusEvent>,
}

pub enum BusEvent {
    SessionCreated { session_id: SessionID },
    MessageCreated { session_id: SessionID, message_id: MessageID },
    PartUpdated { session_id: SessionID, message_id: MessageID, part_id: PartID },
    TeamCreated { team_id: TeamID, session_id: SessionID },
    TeamDisbanded { team_id: TeamID },
    MemberUpdated { team_id: TeamID, agent: String, status: MemberStatus },
    MessageInjected { target_session_id: SessionID, from_agent: String },
    ConfigReloaded,
    // ...
}
```

Capacity: 1024. Receivers that fall behind lose events (broadcast semantics).

#### CLI Structure

Using `clap` derive:

```rust
#[derive(Parser)]
#[command(name = "flok", version, about = "The fast AI coding agent")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
}

enum Command {
    Run,          // Default: launch TUI (interactive mode)
    Serve,        // HTTP API server only
    Models,       // List available models
    Providers,    // List configured providers
    Sessions,     // List/manage sessions
    Export,       // Export session
    Mcp,          // MCP server management
    Version,      // Version info
}
```

Default command (no subcommand) is `Run` -- launches the TUI.

#### Startup Sequence

1. Parse CLI args (clap)
2. Initialize tracing subscriber
3. Detect project root
4. Load config (precedence chain)
5. Initialize Database bundle (SQLite + redb + LanceDB, run migrations)
6. Initialize EventBus
7. Initialize ProviderRegistry (from config + env)
8. Initialize ModelRouter (routing tiers from config, see spec-012)
9. Initialize ToolRegistry (built-in + custom)
10. Initialize AgentRegistry (built-in + config-defined)
11. Initialize McpManager (connect to configured MCP servers)
12. Initialize LspManager (auto-detect project languages, lazy-start servers, see spec-011)
13. Initialize MemoryStore
14. Initialize WorktreeManager (reconcile stale worktrees from crashed sessions, see spec-010)
15. Initialize FastApplyEngine + SmartGrepEngine + CompactionEngine (load tree-sitter grammars, see spec-014)
16. Construct AppState
17. Start file watcher for config hot-reload
18. Launch HTTP server (axum) on localhost
19. Launch TUI (iocraft) in-process (no HTTP server for v0.0.1)

Steps 7-15 run concurrently via `tokio::join!`. LSP servers are lazy-started (don't block startup).

#### Single Static Binary (Killer Feature #10)

Flok compiles to a single static binary with zero runtime dependencies:

```bash
curl -fsSL https://flok.dev/install.sh | sh
flok
```

No Node.js. No Python. No Docker. No bun. One binary, runs everywhere. The binary embeds: the TUI, LSP client, SQLite, redb, LanceDB, tree-sitter grammars, WASM runtime (post-v1.0), and all built-in agent/skill definitions. Startup in milliseconds.

Cross-compilation targets (v1.0): `linux-amd64`, `linux-arm64`, `macos-amd64`, `macos-arm64`. Windows deferred post-v1.0. Docker image for server deployments.

### Alternatives Considered

1. **Single crate**: Rejected. Workspace crates enable parallel compilation and clearer boundaries.
2. **PostgreSQL instead of SQLite**: Rejected. Single-binary philosophy. SQLite is fast enough with WAL mode and embedded in the binary.
3. **sled instead of redb**: Rejected. redb has simpler API, better maintained, lower overhead for key-value use cases.
4. **Using Effect-TS patterns in Rust (trait-based DI)**: Rejected. Rust's ownership model makes a simple `Arc<AppState>` pattern more ergonomic than complex trait-based DI frameworks.

## Success Criteria

- **SC-001**: Cold start to TUI ready in < 200ms on Apple Silicon
- **SC-002**: Config hot-reload applies in < 100ms
- **SC-003**: Single binary size < 50MB (release, stripped, thin LTO)
- **SC-004**: Zero `unsafe` blocks outside of FFI boundaries (SQLite, LanceDB)

## Assumptions

- Users have a Rust toolchain for building from source, OR we distribute prebuilt binaries
- SQLite is sufficient for single-user local storage (not multi-tenant)
- XDG specification is appropriate for macOS (opencode uses it, it works)
- ULID is preferable to UUIDv7 for our sortable ID needs (faster generation, same properties)

## Open Questions

- ~~Should we support Windows from day one, or defer?~~ **Decision: Defer.** Windows support is post-v1.0. Cross-compilation targets at v1.0: `linux-amd64`, `linux-arm64`, `macos-amd64`, `macos-arm64` only.
- ~~Should the HTTP server be optional (TUI-only mode for minimal overhead)?~~ **Decision: Yes.** For v0.0.1, the TUI communicates directly with the session engine in-process (no HTTP server, no axum). The HTTP/axum server layer is added later to support web UI, IDE extensions, and multi-client scenarios. The session engine exposes an async Rust API that both the TUI (direct) and HTTP layer (later) can call.
- What embedding model to bundle for local vector search? (fastembed's all-MiniLM-L6-v2 is 23MB)
