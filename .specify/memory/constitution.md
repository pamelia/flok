# Flok Constitution

## Vision

Flok is an opinionated, high-performance AI coding agent written in Rust. It is a spiritual successor to opencode, rebuilt from scratch to be **blazingly fast**, **natively multi-agent**, and **zero-compromise on performance**.

## Core Principles

### 1. Performance Is Non-Negotiable
Every hot path must be benchmarked. Token caching, message serialization, database writes, and LLM streaming must operate at the limits of what the hardware allows. We chose Rust specifically because TypeScript couldn't keep up. If a design decision trades performance for convenience, we reject it.

### 2. Multi-Agent Is a First-Class Citizen
Teams of agents are not a plugin or afterthought. The session engine, tool system, and TUI are all designed around the assumption that multiple agents run concurrently, communicate asynchronously, and coordinate via shared task boards. This is built-in, not bolted-on.

### 3. Opinionated Defaults, Escape Hatches Where Needed
Flok ships with strong defaults: agent definitions, review workflows, skill compositions, and team orchestration patterns are built-in. Users shouldn't need a plugin ecosystem to get a spec-review team running. Configuration exists for provider keys and model preferences, not for reimagining the architecture.

### 4. Single Binary, Zero Runtime Dependencies
Flok compiles to a single static binary. No Node.js, no Python, no Bun, no JVM. SQLite is embedded. The embedding model runs locally via ONNX. MCP servers are the only external processes.

### 5. Steal Shamelessly, Credit Generously
Flok draws from opencode (session model, tool system, provider abstraction), Spacebot (multi-process architecture, memory system, hot-reload config), claude-plugins and opencode-plugins (agent team orchestration, spec-review workflows). We take the best ideas and make them fast.

## Governance

- **Language**: Rust (edition 2024)
- **Async Runtime**: Tokio (multi-threaded)
- **Database**: SQLite (via rusqlite/sqlx) + redb (key-value) + LanceDB (vector)
- **TUI**: iocraft (declarative React-like component model)
- **HTTP**: axum
- **CLI**: clap (derive)
- **Error Handling**: thiserror for library errors, anyhow for application context
- **Serialization**: serde + serde_json
- **Logging**: tracing

## Versioning

Semantic versioning. Pre-1.0, breaking changes are expected and documented in specs.
