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

- **Multi-provider support** -- Anthropic Claude (4.6/4), OpenAI GPT, DeepSeek
- **Rich terminal UI** -- iocraft-based declarative TUI with sidebar stats, markdown rendering, dark theme
- **14 built-in tools** -- read, write, edit, bash, grep, glob, webfetch, question, todowrite, skill, agent_memory, plan, task
- **Sub-agent system** -- explore agent for codebase search, general agent for parallel tasks
- **Session persistence** -- SQLite-backed conversation storage with resume (`--resume`)
- **Plan/Build modes** -- read-only plan mode restricts tools; build mode allows full access
- **Streaming responses** -- real-time text and reasoning delta streaming
- **Prompt caching** -- Anthropic cache_control breakpoints for cost savings
- **Token counting & cost tracking** -- real-time cost estimation with tiktoken-rs
- **Context window management** -- multi-tier compression (tool output pruning, shell output compression, emergency truncation)
- **Interactive permissions** -- three-tier system: Safe (auto), Write (prompt once), Dangerous (always prompt)
- **AGENTS.md injection** -- project-specific instructions injected into the system prompt
- **Model shorthand aliases** -- `sonnet`, `opus`, `haiku`, `gpt-4.1`, `mini`, `deepseek`, `r1`, etc.
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
export ANTHROPIC_API_KEY="sk-ant-..."

# Launch the TUI
./target/release/flok

# Or with cargo
cargo run --release
```

### Environment Variables

| Variable | Provider |
|----------|----------|
| `ANTHROPIC_API_KEY` | Anthropic (Claude) |
| `OPENAI_API_KEY` | OpenAI (GPT) |
| `DEEPSEEK_API_KEY` | DeepSeek |

## Usage

### Interactive Mode (TUI)

```bash
# Launch with default model (Claude Sonnet 4.6)
flok

# Use a specific model
flok --model opus
flok -m gpt-4.1

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
| `Tab` | Toggle Plan/Build mode |
| `Ctrl+K` | Command palette |
| `Ctrl+M` | Model picker |
| `Ctrl+B` | Toggle sidebar |
| `Ctrl+W` | Delete last word |
| `Ctrl+A` / `Ctrl+E` | Start / end of input |
| `Ctrl+U` | Clear input |
| `Ctrl+C` / `Ctrl+D` | Quit |

### Slash Commands

| Command | Description |
|---------|-------------|
| `/new`, `/clear` | Start a new session |
| `/plan` | Switch to plan mode (read-only) |
| `/build` | Switch to build mode |
| `/model` | Show current model |
| `/sessions` | List past sessions |
| `/sidebar` | Toggle sidebar |
| `/help` | Show available commands |
| `/quit`, `/q` | Exit |

### Permission Prompts

When the LLM requests a write or dangerous operation, you'll see a permission dialog:

- `y` / `Enter` -- Allow once
- `a` -- Always allow this tool for the session
- `n` / `Esc` -- Deny

## Models

| Shorthand | Model | Provider | Context |
|-----------|-------|----------|---------|
| `sonnet` (default) | Claude Sonnet 4.6 | Anthropic | 1M |
| `opus` | Claude Opus 4.6 | Anthropic | 1M |
| `haiku` | Claude Haiku 4.5 | Anthropic | 200K |
| `gpt-4.1` | GPT-4.1 | OpenAI | ~1M |
| `mini` | GPT-4.1 Mini | OpenAI | ~1M |
| `flash` | Gemini 2.5 Flash | Google | ~1M |
| `pro` | Gemini 2.5 Pro | Google | ~1M |
| `deepseek` | DeepSeek V3 | DeepSeek | 128K |
| `r1` | DeepSeek R1 | DeepSeek | 128K |

Legacy: `sonnet-4`, `opus-4` for Claude Sonnet 4 / Opus 4 (previous generation).

## Tools

Flok provides 14 built-in tools:

| Tool | Permission | Description |
|------|-----------|-------------|
| `read` | Safe | Read file contents with line numbers |
| `glob` | Safe | Find files by glob pattern |
| `grep` | Safe | Regex content search across files |
| `webfetch` | Safe | Fetch URL content (SSRF protected) |
| `question` | Safe | Ask the user a question with options |
| `todowrite` | Safe | Manage a task list |
| `skill` | Safe | Load skill instructions from .flok/skills/ |
| `agent_memory` | Safe | Read/write persistent per-agent memory |
| `plan` | Safe | Write structured plans to .flok/plan.md |
| `task` | Safe | Spawn sub-agent (explore, general) |
| `write` | Write | Create or overwrite files |
| `edit` | Write | Search-and-replace in files |
| `bash` | Dangerous | Execute shell commands |

In **plan mode**, only Safe tools are available. Switch to **build mode** (`Tab` or `/build`) to enable Write and Dangerous tools.

## Configuration

Flok looks for configuration in this order:

1. Environment variables (highest priority)
2. `.flok/flok.toml` in project root
3. `flok.toml` in project root
4. `~/.config/flok/flok.toml` (global)

Example `flok.toml`:

```toml
[provider.anthropic]
# api_key = "sk-ant-..."   # Or set ANTHROPIC_API_KEY env var

[provider.openai]
# api_key = "sk-..."       # Or set OPENAI_API_KEY env var
# base_url = "https://api.openai.com/v1"

[provider.deepseek]
# api_key = "sk-..."       # Or set DEEPSEEK_API_KEY env var
# base_url = "https://api.deepseek.com/v1"
```

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
│   ├── flok-core/     # Library: session engine, providers, tools, config
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
