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
- **Multi-provider support** -- Anthropic Claude (4.6/4), OpenAI GPT-5.4 family, DeepSeek
- **Model shorthand aliases** -- `sonnet`, `opus`, `haiku`, `gpt-5.4`, `chatgpt-5.4`, `mini`, `nano`, `deepseek`, `r1`
- **Prompt caching** -- Anthropic cache_control breakpoints for cost savings
- **Streaming responses** -- real-time text and reasoning delta streaming

**Terminal UI**
- **Rich terminal UI** -- iocraft-based declarative TUI with sidebar stats, markdown rendering, dark theme
- **Command palette** (`Ctrl+K`) and model picker (`Ctrl+M`)
- **Text selection & copy** -- click-drag, double-click word, triple-click line, auto-copy to clipboard
- **Scrolling** -- mouse wheel per-panel, keyboard scroll (Page Up/Down, vim-style bindings)
- **Plan/Build modes** -- read-only plan mode restricts tools; build mode allows full access

**Tools & Agents**
- **19 built-in tools** -- file ops, search, bash, web fetch, code review, team coordination, and more
- **10 sub-agents** -- explore and general agents, plus 8 specialist reviewers for code/spec review
- **4 built-in skills** -- code-review, self-review-loop, spec-review, handle-pr-feedback
- **Code review engine** -- multi-agent parallel review with specialist reviewers, deduplication, and verdicts

**Session Management**
- **Session persistence** -- SQLite-backed conversation storage with resume (`--resume`)
- **Session branching** -- branch conversations at any message to explore alternatives; LLM-generated summaries of abandoned paths are injected so the agent learns from prior attempts
- **Session tree** -- navigate branch history with `/tree`, switch between branches, label checkpoints
- **Undo/Redo** -- revert the last message and restore workspace files to the pre-message state

**Infrastructure**
- **Workspace snapshots** -- shadow git repository tracks file state; undo restores files automatically
- **Worktree isolation** -- sub-agents get isolated git worktrees for concurrent work without conflicts
- **Token counting & cost tracking** -- real-time cost estimation with tiktoken-rs
- **Context window management** -- multi-tier compression (tool output pruning, shell output compression, emergency truncation)
- **Interactive permissions** -- three-tier system: Safe (auto), Write (prompt once), Dangerous (always prompt); decisions persist per-project
- **AGENTS.md injection** -- project-specific instructions injected into the system prompt
- **Non-interactive mode** -- `--prompt` flag for scripting and CI

## Quick Start

### Prerequisites

- Rust 1.80+ (for building from source)
- At least one LLM provider API key

### Build & Run

```bash
git clone https://github.com/opencode-dev/flok.git
cd flok
cargo build --release

# Set at least one API key
flok auth login --provider anthropic

# Launch the TUI
./target/release/flok

# Or with cargo
cargo run --release
```

### Providing API Keys

Flok reads provider API keys **only** from its config file at runtime — environment
variables are NOT consulted. The easiest way to set a key is interactively:

```bash
flok auth login --provider anthropic   # or openai, deepseek, minimax
```

Repeat for any other provider you use, for example `flok auth login --provider openai`.

This writes `~/.config/flok/flok.toml` with mode `0600` on Unix. You can also edit
the file manually — see [`flok.example.toml`](./flok.example.toml) for the schema.

## Usage

### Interactive Mode (TUI)

```bash
# Launch with default model (Claude Sonnet 4.6)
flok

# Use a specific model
flok --model opus
flok -m gpt-5.4
flok -m chatgpt-5.4

# Specify working directory
flok -d /path/to/project

# Start in plan mode (read-only)
flok --plan

# Resume a previous session
flok --resume <SESSION_ID>
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
```

### TUI Key Bindings

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Shift+Enter` | New line in input |
| `Esc` | Cancel current streaming response |
| `Tab` | Toggle Plan/Build mode |
| `Ctrl+K` | Command palette |
| `Ctrl+M` | Model picker |
| `Ctrl+B` | Toggle sidebar |
| `Ctrl+W` | Delete last word |
| `Ctrl+A` / `Ctrl+E` | Start / end of input |
| `Ctrl+U` | Clear input |
| `Up` / `Down` | Input history (when single-line) |
| `Ctrl+C` / `Ctrl+D` | Quit |

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
| `/plan` | Switch to plan mode (read-only) |
| `/build` | Switch to build mode |
| `/sessions` | List past sessions |
| `/sidebar` | Toggle sidebar |
| `/help` | Show available commands |
| `/quit`, `/q` | Exit |

### Permission Prompts

When the LLM requests a write or dangerous operation, you'll see a permission dialog:

- `y` / `Enter` -- Allow once
- `a` -- Always allow this tool for the session (persisted to DB)
- `n` / `Esc` -- Deny

## Models

| Shorthand | Model | Provider | Context |
|-----------|-------|----------|---------|
| `sonnet` (default) | Claude Sonnet 4.6 | Anthropic | 1M |
| `opus-4.7` | Claude Opus 4.7 | Anthropic | 1M |
| `opus` | Claude Opus 4.6 | Anthropic | 1M |
| `haiku` | Claude Haiku 4.5 | Anthropic | 200K |
| `gpt-5.4` | GPT-5.4 | OpenAI | 1.05M |
| `chatgpt-5.4` | GPT-5.4 | OpenAI | 1.05M |
| `mini` | GPT-5.4 Mini | OpenAI | 400K |
| `nano` | GPT-5.4 Nano | OpenAI | 400K |
| `gpt-4.1` | GPT-4.1 | OpenAI (legacy) | ~1M |
| `deepseek` | DeepSeek V3 | DeepSeek | 128K |
| `r1` | DeepSeek R1 | DeepSeek | 128K |

Legacy: `sonnet-4`, `opus-4` for Claude Sonnet 4 / Opus 4, plus explicit `gpt-4.1` / `gpt-4.1-mini` for older OpenAI non-reasoning models.

DeepSeek uses the OpenAI-compatible API format with a different base URL.

## Tools

Flok provides 19 built-in tools:

| Tool | Permission | Description |
|------|-----------|-------------|
| `read` | Safe | Read file contents with line numbers |
| `glob` | Safe | Find files by glob pattern |
| `grep` | Safe | Regex content search across files |
| `webfetch` | Safe | Fetch URL content (SSRF protected) |
| `question` | Safe | Ask the user a question with options |
| `todowrite` | Safe | Manage a task list |
| `skill` | Safe | Load skill instructions from `.flok/skills/` |
| `agent_memory` | Safe | Read/write persistent per-agent memory |
| `plan` | Safe | Write structured plans to `.flok/plan.md` |
| `task` | Safe | Spawn sub-agent (explore, general, reviewers) |
| `code_review` | Safe | Run structured multi-agent code review on a diff |
| `team_create` | Safe | Create a named agent team for coordination |
| `team_delete` | Safe | Disband an agent team |
| `team_task` | Safe | Manage tasks on a team's shared task board |
| `send_message` | Safe | Send messages between agents in a team |
| `write` | Write | Create or overwrite files |
| `edit` | Write | Search-and-replace in files |
| `fast_apply` | Write | Apply code edits using lazy snippet markers (`// ... existing code ...`) |
| `bash` | Dangerous | Execute shell commands |

In **plan mode**, only Safe tools are available. Switch to **build mode** (`Tab` or `/build`) to enable Write and Dangerous tools.

## Sub-Agents

The `task` tool spawns sub-agents that run independently and report back:

| Agent | Description |
|-------|-------------|
| `explore` | Fast codebase search -- glob, grep, read files |
| `general` | General-purpose multi-step task execution |
| `feasibility-reviewer` | Technical feasibility & architecture fit |
| `complexity-reviewer` | Complexity & simplicity analysis |
| `completeness-reviewer` | Completeness & edge case coverage |
| `operations-reviewer` | Operations & reliability assessment |
| `api-reviewer` | API design & contract review |
| `clarity-reviewer` | Clarity & precision evaluation |
| `scope-reviewer` | Scope & delivery risk analysis |
| `product-reviewer` | Product & value alignment review |

The 8 specialist reviewers are used by the code review and spec review skills to provide multi-perspective analysis.

## Built-in Skills

Skills are loaded via the `skill` tool and provide structured workflows:

| Skill | Description |
|-------|-------------|
| `code-review` | Reviews a GitHub PR using parallel specialist agents. Produces prioritized findings and a verdict. |
| `self-review-loop` | Iterative review-fix-review loop. Runs until only minor feedback remains (max 5 turns). |
| `spec-review` | Three-phase parallel spec review: specialist review, cross-review, synthesized output. |
| `handle-pr-feedback` | Reads unresolved PR comments, applies fixes, replies to each comment, resolves threads. |

Project-local skills can be added in `.flok/skills/<name>.md`.

## Configuration

Configuration resolution:
- `.flok/flok.toml` in project root (highest priority)
- `flok.toml` in project root
- `~/.config/flok/flok.toml` (user global, lowest priority)

API keys are sourced from the config file only; env vars are not read.

Example `flok.toml`:

```toml
[provider.anthropic]
# api_key = "sk-ant-..."

[provider.openai]
# api_key = "sk-..."
# base_url = "https://api.openai.com/v1"

[provider.deepseek]
# api_key = "sk-..."
# base_url = "https://api.deepseek.com/v1"

[worktree]
# enabled = true           # Isolate sub-agents in git worktrees
# cleanup_on_complete = true
# auto_merge = true        # Auto-merge worktree changes back
```

### Default Model

Set a default model in your config so you don't have to pass `--model` on every run:

```toml
model = "opus-4.7"
```

Or per-provider — handy when you switch providers:

```toml
[provider.anthropic]
default_model = "opus-4.7"

[provider.openai]
default_model = "gpt-5.4"
```

`default_model` accepts any alias listed by `flok models` (or a full ID).

**Precedence** (first match wins):

1. `--model` CLI flag
2. top-level `model` in `flok.toml`
3. first `[provider.X].default_model` (alphabetical by provider name)
4. hardcoded `"sonnet"` fallback

### AGENTS.md

Drop an `AGENTS.md` file in your project root to inject project-specific instructions
into the system prompt. Flok reads this automatically (up to 20KB).

### Data Storage

- **Database:** `~/.local/share/flok/db/flok.db` (SQLite, WAL mode)
- **Config:** `~/.config/flok/`
- **Cache:** `~/.cache/flok/`

## Project Structure

```
flok/
├── crates/
│   ├── flok/          # Binary: CLI entry point, runtime wiring
│   ├── flok-core/     # Library: session engine, providers, tools, agents, review
│   ├── flok-db/       # Library: SQLite persistence, migrations
│   └── flok-tui/      # Library: iocraft TUI components and rendering
├── specs/             # Feature specifications
├── flok.toml          # Default project config
└── AGENTS.md          # Project coding conventions
```

Dependency direction (strict, no cycles):

```
flok (binary) -> flok-tui -> flok-core -> flok-db
```

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

MIT -- see [LICENSE](LICENSE) for details.
