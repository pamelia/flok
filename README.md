# flok

<div align="center">

[![CI Badge]][CI] [![License Badge]][License] [![Rust Badge]][Rust] [![Deps Badge]][Deps]

</div>

An AI coding agent for the terminal, built in Rust.

Flok connects to multiple LLM providers and gives you an interactive TUI for
AI-assisted coding workflows. Single binary, no runtime dependencies.

> **Status:** Early development (v0.0.1). Core features work. APIs may change.

[CI Badge]: https://img.shields.io/github/actions/workflow/status/pamelia/flok/rust.yml?style=flat-square&logo=github&label=CI
[CI]: https://github.com/pamelia/flok/actions/workflows/rust.yml
[License Badge]: https://img.shields.io/badge/license-MIT-blue?style=flat-square
[License]: ./LICENSE
[Rust Badge]: https://img.shields.io/badge/rust-1.80%2B-orange?style=flat-square&logo=rust
[Rust]: https://www.rust-lang.org
[Deps Badge]: https://deps.rs/repo/github/pamelia/flok/status.svg?style=flat-square
[Deps]: https://deps.rs/repo/github/pamelia/flok

## Features

**Providers & Models**
- **Multi-provider support** — Anthropic Claude 4.x, OpenAI GPT-5.4/4.1, MiniMax M2.7
- **Automatic provider fallback** — fail over across configured providers on retriable 429/5xx/529 errors
- **Intelligent model routing** — heuristic complexity detection upgrades complex turns to stronger models automatically
- **Model shorthand aliases** — `sonnet`, `opus`, `haiku`, `gpt-5.4`, `mini`, `nano`, `minimax`
- **Prompt caching** — Anthropic cache_control breakpoints for cost savings
- **Streaming responses** — real-time text and reasoning delta streaming

**Terminal UI**
- **Rich terminal UI** — iocraft-based declarative TUI with sidebar stats, markdown rendering, dark theme
- **Command palette** (`Ctrl+K`) and model picker (`Ctrl+M`)
- **Text selection & copy** — click-drag, double-click word, triple-click line, auto-copy to clipboard
- **Scrolling** — mouse wheel per-panel, keyboard scroll (Page Up/Down)
- **Plan/Build modes** — read-only plan mode restricts tools; build mode allows full access
- **Slash command popup** — type `/` to get fuzzy-filtered command suggestions

**Tools & Agents**
- **26 built-in tools** — file ops, search, LSP, bash, web fetch, execution plans, code review, team coordination, and more
- **10 sub-agents** — explore and general agents, plus 8 specialist reviewers for code/spec review
- **5 built-in skills** — code-review, self-review-loop, spec-review, handle-pr-feedback, source-driven-development
- **Code review engine** — multi-agent parallel review with specialist reviewers, deduplication, and verdicts
- **LSP integration** — diagnostics, go-to-definition, find-references, and symbol search via rust-analyzer

**Session Management**
- **Session persistence** — SQLite-backed conversation storage with resume (`--resume`)
- **Session branching** — branch conversations at any message to explore alternatives; LLM-generated summaries of abandoned paths are injected so the agent learns from prior attempts
- **Session tree** — navigate branch history with `/tree`, switch between branches, label checkpoints
- **Undo/Redo** — revert the last message and restore workspace files to the pre-message state

**Infrastructure**
- **Workspace snapshots** — shadow git repository tracks file state; undo restores files automatically
- **Worktree isolation** — sub-agents get isolated git worktrees for concurrent work without conflicts
- **Execution plans** — structured multi-step plans with DAG dependencies, step-level checkpoints, and rollback
- **Automatic verification** — detects project language and runs build/test after file changes; retries on failure
- **Token counting & cost tracking** — real-time cost estimation with tiktoken-rs, cache-aware pricing
- **Context window management** — three-tier compression (tool output filtering, history compression, recency-based pruning)
- **Output compression** — shell output deduplication, progress bar stripping, command-specific smart filters
- **Interactive permissions** — rule-based system with config-driven patterns, session persistence, and command arity awareness
- **AGENTS.md injection** — project-specific instructions injected into the system prompt
- **Non-interactive mode** — `--prompt` flag for scripting and CI

## Quick Start

### Prerequisites

- Rust 1.80+ (for building from source)
- At least one supported LLM provider API key

### Build & Run

```bash
git clone https://github.com/opencode-dev/flok.git
cd flok
cargo build --release

# Set at least one API key
./target/release/flok auth login --provider anthropic

# Launch the TUI
./target/release/flok

# Or with cargo
cargo run --release
```

### Providing API Keys

Flok reads provider API keys **only** from its config file — environment
variables are NOT consulted. The easiest way to set a key is interactively:

```bash
flok auth login --provider anthropic   # or openai, minimax
```

Repeat for any other provider you use, for example `flok auth login --provider openai`.

The config file is written with mode `0600` on Unix. You can also edit the
file manually — see [`flok.example.toml`](./flok.example.toml) for the schema.

> **Config file location:** Flok uses the [`directories`](https://docs.rs/directories)
> crate, which follows platform conventions:
>
> | Platform | Config path |
> |----------|-------------|
> | **Linux** | `~/.config/flok/flok.toml` |
> | **macOS** | `~/Library/Application Support/flok/flok.toml` |
> | **Windows** | `{FOLDERPATH:RoamingAppData}\flok\flok.toml` |
>
> If `XDG_CONFIG_HOME` is set, it is respected on all platforms.

## Usage

### Interactive Mode (TUI)

```bash
# Launch with default model (Claude Sonnet 4.6)
flok

# Use a specific model
flok --model opus
flok -m gpt-5.4
flok -m mini

# Specify working directory
flok -d /path/to/project

# Start in plan mode (read-only)
flok --plan

# Resume a previous session
flok --resume <SESSION_ID>

# Enable debug logging to /tmp/flok.log
flok --debug
```

### Non-Interactive Mode

```bash
# Send a single prompt and get the response on stdout
flok --prompt "explain the architecture of this project"
flok -p "find all TODO comments in the codebase"
```

### Subcommands

```bash
flok models              # List available models and pricing
flok sessions            # List past sessions
flok sessions -n 50      # List more sessions
flok version             # Show version info
flok auth login          # Save an API key (interactive provider picker)
flok auth login --provider anthropic   # Save a specific provider's key
```

### TUI Key Bindings

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Shift+Enter` | New line in input |
| `Esc` | Cancel current streaming response |
| `Tab` | Toggle Plan/Build mode |
| `Ctrl+K` | Kill to end of line (in composer) |
| `Ctrl+M` | Model picker |
| `Ctrl+B` | Toggle sidebar |
| `Ctrl+W` | Delete last word |
| `Ctrl+A` / `Home` | Start of line |
| `Ctrl+E` / `End` | End of line |
| `Ctrl+U` | Clear input |
| `Ctrl+Y` | Yank (paste from kill ring) |
| `Up` / `Down` | Input history (single-line) or cursor movement (multi-line) |
| `Ctrl+D` | Quit |

**Scrolling:**

| Key | Action |
|-----|--------|
| `PageUp` / `PageDown` | Scroll messages half-page |
| `Ctrl+Home` / `Ctrl+End` | Scroll to top / bottom |
| Mouse wheel | Per-panel scrolling |

**Text Selection:**

| Action | Effect |
|--------|--------|
| Click + drag | Character selection |
| Double-click | Word selection |
| Triple-click | Line selection |
| `Ctrl+C` (with selection) | Copy to clipboard |

**Permission Prompts:**

| Key | Action |
|-----|--------|
| `Y` / `Enter` | Allow once |
| `A` | Always allow (persisted) |
| `N` / `Esc` | Deny |
| `←` / `→` / `Tab` | Cycle options |

### Slash Commands

| Command | Description |
|---------|-------------|
| `/new`, `/clear` | Start a new session |
| `/undo` | Undo last message and restore files |
| `/redo` | Redo last undone message |
| `/tree` | Show session branching tree |
| `/branch` | List messages to branch from |
| `/branch <n>` | Branch at message number `n` |
| `/label <text>` | Label the current session for tree navigation |
| `/plans` | List saved execution plans |
| `/show-plan [id]` | Show an execution plan (latest if no ID) |
| `/approve [id]` | Approve a saved execution plan |
| `/execute-plan [id]` | Execute a saved execution plan |
| `/plan` | Switch to plan mode (read-only) |
| `/build` | Switch to build mode |
| `/sidebar` | Toggle sidebar |
| `/sessions` | List past sessions |
| `/help` | Show available commands |
| `/quit`, `/exit`, `/q` | Exit |

## Models

### Current Models

| Shorthand | Model | Provider | Context | Max Output | Input $/M | Output $/M |
|-----------|-------|----------|---------|------------|-----------|------------|
| `sonnet` (default) | Claude Sonnet 4.6 | Anthropic | 1M | 64K | $3.00 | $15.00 |
| `opus-4.7` | Claude Opus 4.7 | Anthropic | 1M | 128K | $5.00 | $25.00 |
| `opus` | Claude Opus 4.6 | Anthropic | 1M | 128K | $5.00 | $25.00 |
| `haiku` | Claude Haiku 4.5 | Anthropic | 200K | 64K | $1.00 | $5.00 |
| `gpt-5.4` | GPT-5.4 | OpenAI | 1.05M | 128K | $2.50 | $15.00 |
| `mini` | GPT-5.4 Mini | OpenAI | 400K | 128K | $0.75 | $4.50 |
| `nano` | GPT-5.4 Nano | OpenAI | 400K | 128K | $0.20 | $1.25 |
| `minimax` | MiniMax M2.7 | MiniMax | 200K | 128K | subscription | subscription |

### Legacy

| Shorthand | Model | Provider | Context |
|-----------|-------|----------|---------|
| `sonnet-4` | Claude Sonnet 4 | Anthropic | 200K |
| `opus-4` | Claude Opus 4 | Anthropic | 200K |
| `gpt-4.1` | GPT-4.1 | OpenAI | ~1M |
| `gpt-4.1-mini` | GPT-4.1 Mini | OpenAI | ~1M |

Each model has many aliases — run `flok models` for the full list.

## Tools

Flok provides 26 built-in tools (plus 4 conditional LSP tools):

### Safe Tools (no permission prompt)

| Tool | Description |
|------|-------------|
| `read` | Read file contents with line numbers; also handles directory listing |
| `glob` | Find files by glob pattern, sorted by modification time |
| `grep` | Regex content search across files |
| `smart_grep` | Symbol-aware code search — supports text, symbol, reference, and semantic query modes |
| `question` | Ask the user a question with selectable options |
| `todowrite` | Manage a task list with content, status, and priority |
| `skill` | Load skill instructions from `.flok/skills/`, global skills dir, or built-ins |
| `agent_memory` | Read/write/append persistent per-agent memory (`.flok/memory/<agent>.md`) |
| `plan` | Write a structured markdown plan to `.flok/plan.md` |
| `plan_create` | Create a typed execution plan with steps, dependencies, and agent types |
| `plan_update` | Update plan or step status, record checkpoints and errors |
| `task` | Spawn a sub-agent (explore, general, or specialist reviewers) |
| `code_review` | Run structured multi-agent code review on a git diff |
| `team_create` | Create a named agent team for multi-agent coordination |
| `team_delete` | Disband an agent team |
| `team_task` | Manage tasks on a team's shared task board (create, update, get, list) |
| `send_message` | Send messages between agents in a team |

### Safe Tools — LSP (conditional, enabled when rust-analyzer is available)

| Tool | Description |
|------|-------------|
| `lsp_diagnostics` | Get LSP diagnostics for a file or directory, with severity filtering |
| `lsp_goto_definition` | Find the definition of a symbol at a given file/line/column |
| `lsp_find_references` | Find all references to a symbol, optionally including declaration |
| `lsp_symbols` | List document symbols or search workspace symbols |

### Write Tools (prompt on first use)

| Tool | Description |
|------|-------------|
| `write` | Create or overwrite files; creates parent directories |
| `edit` | Exact string search-and-replace in files |
| `fast_apply` | Apply code edits using lazy snippet markers (`// ... existing code ...`) |
| `webfetch` | Fetch URL content as text/JSON (SSRF-protected: blocks private IPs and metadata endpoints) |

### Dangerous Tools (always prompt unless "Always" granted)

| Tool | Description |
|------|-------------|
| `bash` | Execute shell commands; 120s timeout; strips dangerous env vars (`LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`, `NODE_OPTIONS`, etc.) |

In **plan mode**, only Safe tools are available. Switch to **build mode** (`Tab` or `/build`) to enable Write and Dangerous tools.

## Sub-Agents

The `task` tool spawns sub-agents that run independently with their own context window (up to 25 prompt rounds) and report back:

| Agent | Description |
|-------|-------------|
| `explore` | Fast codebase search — glob, grep, read files. Read-only by convention. Supports thoroughness levels: quick, medium, very thorough. |
| `general` | General-purpose multi-step task execution |
| `feasibility-reviewer` | Technical feasibility & architecture fit |
| `complexity-reviewer` | Complexity & simplicity analysis |
| `completeness-reviewer` | Completeness & edge case coverage |
| `operations-reviewer` | Operations & reliability assessment |
| `api-reviewer` | API design & contract review |
| `clarity-reviewer` | Clarity & precision evaluation |
| `scope-reviewer` | Scope & delivery risk analysis |
| `product-reviewer` | Product & value alignment review |

Sub-agents inherit all registered tools except `task` itself (no recursive spawning). When worktree isolation is enabled, background agents get their own git worktree so they can edit files without interfering with the main session.

The 8 specialist reviewers are used by the code review and spec review skills to provide multi-perspective analysis.

## Built-in Skills

Skills are loaded via the `skill` tool and provide structured multi-step workflows:

| Skill | Description |
|-------|-------------|
| `code-review` | Reviews a GitHub PR using parallel specialist agents. Selects 2–4 reviewers based on PR size, produces prioritized findings and an APPROVE/REQUEST_CHANGES verdict. |
| `self-review-loop` | Iterative review-fix-review loop. Runs until only minor feedback remains (max 5 turns). Includes oscillation detection. |
| `spec-review` | Three-phase parallel spec review: specialist review, cross-review for conflicts, synthesized output with verdict. |
| `handle-pr-feedback` | Reads unresolved PR review comments, triages them, applies fixes, replies to each comment via GitHub API, and pushes. |
| `source-driven-development` | Grounds framework-specific code in official documentation. Detects dependency versions, fetches relevant docs, implements with cited sources. |

**Custom skills:** Drop a `.md` file in `.flok/skills/` (project-local) or `<config_dir>/flok/skills/` (global) to add your own. Same-name files override built-ins.

## Configuration

### Config File Locations

Configuration is loaded from multiple layers, merged with higher-priority files overriding lower ones:

| Priority | Location | Description |
|----------|----------|-------------|
| 3 (highest) | `<project>/.flok/flok.toml` | Private per-project config (gitignore-able) |
| 2 | `<project>/flok.toml` | Committable project config |
| 1 (lowest) | `<config_dir>/flok/flok.toml` | Global user config |

The `<config_dir>` follows platform conventions:

| Platform | Global config path |
|----------|--------------------|
| **Linux** | `~/.config/flok/flok.toml` |
| **macOS** | `~/Library/Application Support/flok/flok.toml` |
| **Windows** | `{FOLDERPATH:RoamingAppData}\flok\flok.toml` |

If `XDG_CONFIG_HOME` is set, it is respected on all platforms.

API keys are sourced **only** from the config file — environment variables are not read.

### Model Selection

**Default model precedence** (first match wins):

1. `--model` CLI flag
2. Top-level `model` in config
3. First `[provider.X].default_model` (alphabetical by provider name)
4. Hardcoded `"sonnet"` fallback

```toml
# Set a global default model
model = "opus-4.7"

# Or set a default per-provider
[provider.anthropic]
default_model = "opus-4.7"

[provider.openai]
default_model = "gpt-5.4"
```

### Reasoning Effort

```toml
# Global reasoning effort for providers/models that support it
# Values: "none", "minimal", "low", "medium", "high", "xhigh"
reasoning_effort = "high"
```

### Provider Configuration

```toml
[provider.anthropic]
# api_key = "sk-ant-..."       # Set via: flok auth login --provider anthropic
# default_model = "opus-4.7"
# fallback = ["openai"]        # Try OpenAI on 429/5xx/529

[provider.openai]
# api_key = "sk-..."           # Set via: flok auth login --provider openai
# base_url = "https://api.openai.com/v1"   # Override for compatible APIs
# default_model = "gpt-5.4"
# fallback = ["anthropic"]

[provider.minimax]
# api_key = "..."              # Set via: flok auth login --provider minimax
# default_model = "minimax"
```

### Per-Agent Model Overrides

Any built-in sub-agent can override its preferred model, fallback chain, reasoning
effort, and system prompt extension:

```toml
[agents.explore]
model = "haiku"
reasoning_effort = "low"
fallback_models = ["minimax", "nano"]
prompt_append = "Be concise. Skip non-essential detail."

[agents.feasibility-reviewer]
model = "opus-4.7"
reasoning_effort = "high"
prompt_append = "Pay extra attention to operational risks."
```

When a sub-agent is spawned, the model is resolved as: `task(model=...)` argument
→ `[agents.<name>].model` → session default. If `fallback_models` is set for an
agent, it replaces the provider-level fallback chain for that agent.

### Runtime Fallback

```toml
[runtime_fallback]
enabled = true                          # default: true
retry_on_errors = [429, 500, 502, 503, 529]  # HTTP codes that trigger fallback
max_attempts = 3                        # Total attempts across fallback chain
cooldown_seconds = 120                  # Provider cooldown after retriable failure
notify_on_fallback = true               # Show fallback notices in the TUI
```

### Intelligent Routing

Automatically upgrades complex turns to stronger models based on conversation
complexity signals (tool density, verification retries, architecture keywords, etc.):

```toml
[intelligent_routing]
enabled = true          # default: true
complexity_threshold = 4  # Minimum complexity score before model upgrade
```

### LSP Integration

Flok ships with a built-in LSP client. Currently supports rust-analyzer for Rust
projects (auto-detected via `Cargo.toml` or `rust-toolchain.toml`):

```toml
[lsp]
enabled = true               # default: true
request_timeout_ms = 5000    # default: 5000

[lsp.rust]
command = "rust-analyzer"    # default: "rust-analyzer"
args = []                    # default: []
```

When enabled, four additional tools become available: `lsp_diagnostics`,
`lsp_goto_definition`, `lsp_find_references`, and `lsp_symbols`.

### Worktree Isolation

Sub-agents run in isolated git worktrees so they can edit files without conflicts:

```toml
[worktree]
enabled = true              # default: true
auto_merge = true           # Auto-merge non-conflicting changes on completion
cleanup_on_complete = true  # Remove worktree directory after merge
```

### Permission Rules

Configure per-tool permission rules. Each entry can be a bare action or a table
of pattern → action:

```toml
[permission]
read = "allow"       # Allow all reads without prompting
glob = "allow"

[permission.bash]
"*" = "allow"        # Allow all bash commands
"rm -rf *" = "deny"  # But deny destructive patterns

[permission.external_directory]
"*" = "ask"          # Always ask for external directory access
```

Actions: `allow`, `deny`, `ask`.

### Output Compression

Controls the tool output compression pipeline (mainly for `bash` output):

```toml
[output_compression]
enabled = true                    # default: true
passthrough_threshold_lines = 40  # Skip compression below this
max_lines = 200                   # Line budget before truncation
head_lines = 50                   # Lines kept at start
tail_lines = 50                   # Lines kept at end
max_chars = 20000                 # Hard character budget
group_exact_min = 3               # Min exact-repeat run to group
group_similar_min = 5             # Min normalized-repeat run to group
apply_to_tools = ["bash"]         # Tools that use this pipeline
```

### AGENTS.md

Drop an `AGENTS.md` file in your project root to inject project-specific
instructions into the system prompt. Flok reads this automatically (up to 20KB).

### Data Storage

Flok follows platform conventions for data storage:

| Purpose | Linux | macOS |
|---------|-------|-------|
| **Config** | `~/.config/flok/` | `~/Library/Application Support/flok/` |
| **Database** | `~/.local/share/flok/db/flok.db` | `~/Library/Application Support/flok/db/flok.db` |
| **Cache** | `~/.cache/flok/` | `~/Library/Caches/flok/` |
| **Worktrees** | `~/.local/state/flok/worktrees/` | `~/Library/Application Support/flok/worktrees/` |
| **Debug log** | `/tmp/flok.log` (when `--debug`) | `/tmp/flok.log` (when `--debug`) |

If `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, or `XDG_STATE_HOME` are
set, those are respected on all platforms.

The database uses SQLite in WAL mode with foreign keys enabled.

## Architecture

```
flok/
├── crates/
│   ├── flok/          # Binary: CLI entry point, runtime wiring
│   ├── flok-core/     # Library: session engine, providers, tools, agents,
│   │                  #   review, LSP, worktrees, snapshots, compression,
│   │                  #   permissions, routing, cost tracking
│   ├── flok-db/       # Library: SQLite persistence, migrations
│   └── flok-tui/      # Library: iocraft TUI components and rendering
├── specs/             # Feature specifications
├── flok.example.toml  # Annotated config reference
└── AGENTS.md          # Project coding conventions
```

Dependency direction (strict, no cycles):

```
flok (binary) → flok-tui → flok-core → flok-db
```

### Key Subsystems

| Module | Description |
|--------|-------------|
| `provider/` | LLM provider implementations (Anthropic, OpenAI, MiniMax) + model registry + fallback |
| `session/` | Prompt loop engine, branching, undo/redo, tree navigation |
| `tool/` | Tool registry and all 26 built-in tools |
| `agent/` | Sub-agent definitions, system prompts, routing |
| `config/` | Multi-layer config loading and merging |
| `compress/` | Three-tier context compression (filter → history → pruning) |
| `permission/` | Rule engine, arity-based canonicalization, tree-sitter path extraction |
| `lsp/` | LSP client (JSON-RPC, document tracking, diagnostics) |
| `snapshot.rs` | Shadow git repo for workspace state tracking |
| `worktree.rs` | Git worktree management for sub-agent isolation |
| `routing.rs` | Intelligent model routing with complexity heuristics |
| `verification.rs` | Post-edit automatic build/test verification |
| `review/` | Code review engine with parallel specialist agents |
| `skills/` | Compiled-in skill markdown files |
| `team.rs` | Multi-agent team coordination (registry, task board, messaging) |
| `token/` | Token counting (tiktoken) and cost tracking (atomic, cache-aware) |
| `bus.rs` | Event bus (tokio broadcast) for cross-system communication |

## Development

```bash
# Fast type-check during iteration
cargo check --workspace

# Full build gate (run before every commit)
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License

MIT — see [LICENSE](LICENSE) for details.