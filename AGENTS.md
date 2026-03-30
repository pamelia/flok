# Flok Project Conventions

## 0. Build Gate (MANDATORY)

**After every feature, bug fix, or refactor — before moving to the next task — run:**

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

If any of these fail, fix the issue before proceeding. Do not skip this step. Do not
batch multiple features before running the gate. The gate runs after *every* change.

**Short version for development iteration:**
```bash
cargo check --workspace   # Fast type-check (~2s)
```

**Full gate (before declaring a task complete):**
```bash
cargo fmt --all --check   # Formatting
cargo clippy --workspace --all-targets -- -D warnings  # Lints
cargo test --workspace    # All tests
```

If clippy or tests reveal issues in code you just wrote, fix them immediately. If they
reveal pre-existing issues in code you didn't touch, note them but don't fix them in the
current task (file a separate issue).

---

## 0.1 Flok Workspace Layout

```
flok/
├── Cargo.toml              # Workspace root
├── AGENTS.md               # This file
├── flok.toml               # Default project config (for self-testing)
├── rustfmt.toml            # Formatting config
├── clippy.toml             # Clippy config (disallowed methods)
├── specs/                  # Feature specifications
├── crates/
│   ├── flok/               # Binary crate: CLI entry, wiring, main()
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   └── cli.rs      # CLI argument parsing (clap)
│   │   └── Cargo.toml
│   ├── flok-core/          # Library: session engine, providers, tools, routing
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── session/    # Session engine, prompt loop
│   │   │   ├── provider/   # LLM provider implementations
│   │   │   ├── tool/       # Tool registry and built-in tools
│   │   │   ├── agent/      # Agent definitions, registry
│   │   │   ├── config/     # Configuration loading, hot-reload
│   │   │   └── bus.rs      # Event bus
│   │   ├── tests/          # Integration tests
│   │   └── Cargo.toml
│   ├── flok-db/            # Library: SQLite, migrations, schema
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── schema.rs
│   │   │   └── migrations/ # SQL migration files
│   │   ├── tests/
│   │   └── Cargo.toml
│   └── flok-tui/           # Library: TUI components and rendering (iocraft)
│       ├── src/
│       │   ├── lib.rs
│       │   ├── components/ # iocraft components (app, messages, sidebar, etc.)
│       │   ├── theme.rs
│       │   └── types.rs    # Shared types (UiCommand, UiEvent, etc.)
│       ├── tests/
│       └── Cargo.toml
```

**Dependency direction (strict, no cycles):**
```
flok (binary) → flok-tui → flok-core → flok-db
```

Lower crates MUST NOT depend on higher crates. `flok-db` knows nothing about sessions
or providers. `flok-core` knows nothing about TUI rendering. The binary crate wires
everything together.

---

## 0.2 Test Organization

### Unit tests: inline `#[cfg(test)]` modules

Every `.rs` file with non-trivial logic should have a `#[cfg(test)] mod tests` block at
the bottom. Unit tests test individual functions, types, and modules in isolation.

```rust
// src/provider/anthropic.rs

pub(crate) fn parse_stream_event(data: &str) -> Result<StreamEvent> {
    // ...
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_text_delta() {
        let event = parse_stream_event(r#"{"type":"content_block_delta",...}"#).unwrap();
        assert!(matches!(event, StreamEvent::TextDelta { .. }));
    }

    #[test]
    fn parse_invalid_json_returns_error() {
        assert!(parse_stream_event("not json").is_err());
    }
}
```

### Integration tests: `crates/*/tests/` directory

Integration tests go in each crate's `tests/` directory. They test cross-module behavior
and public API contracts. Each file in `tests/` is compiled as a separate binary.

```rust
// crates/flok-core/tests/session_prompt_loop.rs

#[tokio::test]
async fn prompt_loop_handles_tool_calls() {
    let state = test_fixtures::app_state_with_mock_provider().await;
    let session = state.create_session().await.unwrap();
    // ...
}
```

### Test fixtures and helpers: `crates/*/src/testutil.rs` or `tests/common/`

Shared test utilities live in a `testutil` module (gated behind `#[cfg(test)]`) or in
`tests/common/mod.rs` for integration tests.

```rust
// crates/flok-core/src/testutil.rs
#![cfg(test)]

pub fn mock_provider() -> MockProvider { ... }
pub async fn app_state_with_mock_provider() -> AppState { ... }
```

### What to test

- **Always test**: public API functions, error paths, serialization/deserialization,
  configuration parsing, state machines, permission checks
- **Use snapshot tests (`insta`)**: for complex output formatting (markdown rendering,
  diagnostic messages, tool output formatting)
- **Use `#[tokio::test]`**: for all async tests
- **Mock external dependencies**: LLM providers, filesystem (via trait abstraction),
  MCP servers. Never make real HTTP calls in tests.

### Test naming convention

```rust
#[test]
fn <function_name>_<scenario>_<expected_behavior>() { }

// Examples:
#[test]
fn parse_config_missing_provider_returns_default() { }
#[test]
fn permission_check_deny_pattern_blocks_bash() { }
#[tokio::test]
async fn session_engine_streams_text_deltas() { }
```

---

## 0.3 Security Rules

These rules are non-negotiable. Every contributor — human or AI — must follow them.

### Golden Rules

1. **Never trust LLM output.** All tool responses, shell output, and file content from
   agents are untrusted input. Validate and sanitize before acting on it.
2. **Never store secrets in plaintext in memory longer than needed.** Wrap API keys, tokens,
   and credentials in a `Zeroizing<String>` (from the `zeroize` crate) or a `SecretString`
   type that redacts in `Debug`/`Display` (prints `[REDACTED]`, never the value).
3. **Never shell out by concatenating strings.** Use `Command::new(binary).arg(arg1).arg(arg2)`
   to prevent command injection. Never `sh -c "<user_string>"`.
4. **Never write outside the agent's workspace.** Canonicalize all file paths and reject
   traversal (`..`, symlink escapes). See Input Validation below.
5. **Treat migration files as immutable.** Never edit an existing migration — it causes
   checksum mismatches. Always add a new migration.
6. **Use `#[expect(lint)]` instead of `#[allow(lint)]`** for lint overrides — `expect`
   warns when the suppressed lint no longer fires, so dead suppressions don't accumulate.

### Secret & Credential Handling

- Config resolution order: `env var > secrets store > config file > default`. Never
  hardcode keys.
- Leak detection at egress points: scan all outbound text (agent replies, HTTP requests,
  log output) against regex patterns for API keys, tokens, PEM headers, and bearer tokens.
  Check plaintext, URL-encoded, base64, and hex encodings. Block any match. Log the event
  (never the matched secret).
- Environment variable injection blocking: the `bash` tool MUST strip dangerous env vars
  from child processes: `LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`, `NODE_OPTIONS`,
  `PYTHONPATH`, `RUBYOPT`, and similar library injection vectors.

### Input Validation & Sandboxing

- All file paths from LLM output MUST be:
  - Canonicalized via `std::fs::canonicalize()` or equivalent
  - Checked against the agent's workspace root (reject if not a descendant)
  - Checked for symlink escape (resolve symlinks, then re-check containment)
- Network access from tools:
  - Block requests to cloud metadata endpoints (`169.254.169.254`,
    `metadata.google.internal`, etc.)
  - Block requests to loopback, private IP ranges, and link-local addresses (SSRF
    protection)
- Identity/config file protection: write operations to flok's own config, memory, and
  identity paths MUST be rejected with an error directing the LLM to the correct tool.
- Error messages MUST NOT leak sensitive information (file paths with user data, secret
  values, internal IP addresses).

### Logging

- Use `tracing` for structured logging. Never `println!` in library code.
- NEVER log secret values, API keys, or raw credentials at any level.
- NEVER log raw user prompts at `info` level or above — they may contain sensitive data.
- Include `agent_id`, `session_id`, and `trace_id` as structured fields on log entries
  for correlation.
- Log levels:
  - `error` — something broke, needs human attention
  - `warn` — something unexpected but recoverable
  - `info` — lifecycle events (agent spawn, session start/end, compaction triggered)
  - `debug` — tool calls, message routing, state transitions
  - `trace` — raw LLM request/response (NEVER in production — contains user data)

### Security Testing

- Security-critical paths MUST have dedicated tests:
  - Path traversal rejection
  - Secret leak detection (assert output does NOT contain known patterns)
  - Sandbox boundary enforcement
  - Command injection resistance
- Integration tests for agent workflows MUST use a temporary directory as the workspace
  root and assert no writes occurred outside it.
- Use `cargo fuzz` for parser code, deserialization, and any function that processes
  untrusted input.

### CI Security Gates

In addition to the build gate (Section 0), CI should run:

```bash
cargo audit              # RustSec vulnerability database
cargo deny check         # License compliance, duplicate crates, banned crates
cargo geiger             # Unsafe code map — review diff if count increases
```

### Release Profile Hardening

```toml
[profile.release]
overflow-checks = true   # Detect integer overflow in release builds
```

---

# Rust Code Quality and Performance Guidelines

> Distilled from the conventions and patterns used by 11 production Rust codebases:
> ripgrep, rust-analyzer, tokio, ruff, zed, wasmi, nushell, helix, regex, memchr, bstr.

---

## Table of Contents

0. [Build Gate](#0-build-gate-mandatory)
0. [Flok Workspace Layout](#01-flok-workspace-layout)
0. [Test Organization](#02-test-organization)
0. [Security Rules](#03-security-rules)
1. [Error Handling](#1-error-handling)
2. [Performance](#2-performance)
3. [Unsafe Code](#3-unsafe-code)
4. [Type System](#4-type-system)
5. [API Design](#5-api-design)
6. [Trait Design](#6-trait-design)
7. [Testing](#7-testing)
8. [Documentation](#8-documentation)
9. [Lints and Formatting](#9-lints-and-formatting)
10. [Project Structure](#10-project-structure)
11. [Concurrency](#11-concurrency)
12. [Dependencies](#12-dependencies)
13. [Feature Flags](#13-feature-flags)
14. [Macros](#14-macros)

---

## 1. Error Handling

### 1.1 Use typed errors at library boundaries, `anyhow` for application code

Library crates define domain-specific error enums with `thiserror`. Application-level code
(binaries, CLI tools) uses `anyhow::Result` for convenience.

```rust
// Library crate -- typed, structured
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unexpected token at offset {offset}")]
    UnexpectedToken { offset: usize },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

// Application crate -- ergonomic
fn main() -> anyhow::Result<()> {
    let config = load_config().context("failed to load config")?;
    Ok(())
}
```

**Sources:** ruff (thiserror at boundaries, anyhow internally), helix (typed errors in helix-lsp,
anyhow in helix-term), nushell (thiserror + miette for ShellError), zed (anyhow throughout
with thiserror for specific types like Timeout).

### 1.2 Keep error types small

Box the inner representation so `Result<T, Error>` doesn't bloat return values. A pointer-sized
error means the happy path has zero overhead from the error type.

```rust
#[derive(Debug)]
pub struct Error {
    kind: Box<ErrorKind>,
}

#[test]
fn error_is_pointer_sized() {
    assert_eq!(std::mem::size_of::<Error>(), std::mem::size_of::<*const ()>());
}
```

**Source:** wasmi (`Error` is exactly 8 bytes via `Box<ErrorKind>`).

### 1.3 Mark error-construction paths as `#[cold]`

Error paths are unlikely. Annotating `From` impls and constructors with `#[cold]` improves
branch prediction on the happy path.

```rust
impl From<IoError> for Error {
    #[inline]
    #[cold]
    fn from(err: IoError) -> Self {
        Self { kind: Box::new(ErrorKind::Io(err)) }
    }
}
```

**Source:** wasmi (all `From` impls on errors are `#[cold]`).

### 1.4 Use `#[non_exhaustive]` on public error enums

Allows adding new error variants without breaking downstream code.

```rust
#[derive(Debug)]
#[non_exhaustive]
pub enum ErrorKind {
    Syntax(SyntaxError),
    Io(std::io::Error),
}
```

**Sources:** regex (`Error` is `#[non_exhaustive]`), wasmi (`ErrorKind` is `#[non_exhaustive]`).

### 1.5 Errors should carry the failed value when useful

When a send/conversion fails, return the original value inside the error so the caller can
retry or recover without losing ownership.

```rust
pub struct SendError<T>(pub T);

pub enum TrySendError<T> {
    Full(T),
    Closed(T),
}
```

**Source:** tokio (`SendError<T>` wraps the unsent value).

### 1.6 Provide rich error context for user-facing errors

Use source spans, labels, and help text. Crates like `miette` or custom error formatters
can point the user to the exact problem location.

```rust
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
#[error("type mismatch in '{name}'")]
#[diagnostic(code(my_tool::type_mismatch))]
pub struct TypeMismatch {
    pub name: String,
    #[label("expected {expected}, found {found}")]
    pub span: SourceSpan,
    pub expected: String,
    pub found: String,
    #[help]
    pub suggestion: Option<String>,
}
```

**Source:** nushell (ShellError with miette diagnostics, span labels, help text, resolution
sections in doc comments).

### 1.7 Never use `.unwrap()` in library code

Prefer `?` for propagation. Use `.expect("reason")` only when the invariant is provably
upheld and document why. Encode constraints in the type system instead of relying on
runtime panics.

**Sources:** ruff (AGENTS.md: "avoid patterns that require `panic!`, `unreachable!`, or
`.unwrap()`"), nushell (CI enforces `clippy::unwrap_used`), zed (CLAUDE.md: "Never use
`unwrap()` -- propagate with `?`").

### 1.8 Never silently discard errors

Don't use `let _ = fallible_op();`. Either propagate with `?`, handle the error, or log it
explicitly.

```rust
// Bad
let _ = send_notification();

// Good
if let Err(e) = send_notification() {
    log::warn!("failed to send notification: {e}");
}

// Also acceptable in frameworks that provide it
send_notification_task.detach_and_log_err(cx);
```

**Source:** zed (CLAUDE.md: "Never silently discard errors with `let _ =` -- use
`.log_err()` instead").

---

## 2. Performance

### 2.1 Choose the right allocator

Use jemalloc on Linux/macOS and mimalloc on Windows. The system allocator has significant
overhead for multi-threaded workloads.

```rust
#[cfg(all(not(target_os = "windows"), not(target_env = "musl")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(target_os = "windows")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
```

**Source:** ruff (jemalloc + mimalloc selection by platform).

### 2.2 Use `FxHashMap`/`FxHashSet` for non-cryptographic hashing

The default `SipHash` is cryptographically secure but slow. For internal data structures
where DoS resistance is not needed (especially short keys like identifiers), use `rustc-hash`.

```rust
use rustc_hash::{FxHashMap, FxHashSet};
```

**Sources:** ruff, zed, rust-analyzer (all use rustc-hash/FxHash throughout).

### 2.3 Use `SmallVec` for common small collections

When most instances contain 1-3 elements, `SmallVec` avoids heap allocation entirely.

```rust
use smallvec::SmallVec;

// Single-cursor is the common case -- avoid heap allocation
pub struct Selection {
    ranges: SmallVec<[Range; 1]>,
    primary_index: usize,
}
```

**Sources:** helix (`SmallVec<[Range; 1]>` for selections), ruff (smallvec with `union`,
`const_generics`, `const_new` features).

### 2.4 Use `CompactString` / `SmartString` for short strings

Identifiers, variable names, and other short strings can be stored inline (typically up to
24 bytes on 64-bit) without a heap allocation.

```rust
pub struct Name(compact_str::CompactString);
```

**Sources:** ruff (`CompactString` for Python identifiers), helix (`SmartString` as `Tendril`
type for edit operations).

### 2.5 Use bitsets for frequently-checked boolean sets

When you frequently check membership in a set of flags/rules, a fixed-size bitset is
orders of magnitude faster than a `HashSet`.

```rust
const RULESET_SIZE: usize = 16;

#[derive(Clone, Default)]
pub struct RuleSet([u64; RULESET_SIZE]); // 1024 bits, O(1) contains/insert

impl RuleSet {
    pub fn contains(&self, rule: Rule) -> bool {
        let code = rule as u16;
        let word = (code / 64) as usize;
        let bit = code % 64;
        self.0[word] & (1 << bit) != 0
    }
}
```

**Source:** ruff (bitset-based `RuleSet` for O(1) rule enablement checks).

### 2.6 Use `NonZeroU32` / `NonZeroUsize` for index newtypes

This enables niche optimization: `Option<Id>` has the same size as `Id`.

```rust
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct NodeId(std::num::NonZeroU32);

// Proves it at compile time:
const _: () = assert!(
    std::mem::size_of::<NodeId>() == std::mem::size_of::<Option<NodeId>>()
);
```

**Sources:** ruff (`newtype_index` macro uses `NonZeroU32`), helix (`DocumentId(NonZeroUsize)`).

### 2.7 Use arena allocation for batch-allocated, batch-freed data

When many objects are created during a phase and freed together, arena allocation eliminates
per-object allocation overhead and improves cache locality.

```rust
// typed_arena for simple bump allocation
let arena: typed_arena::Arena<Annotation> = typed_arena::Arena::new();
let ann = arena.alloc(parse_annotation()?);

// Custom arena for render frames (zed's GPUI)
let mut arena = Arena::new(NonZeroUsize::new(65536).unwrap());
let element = arena.alloc(|| create_element());
arena.clear(); // Bulk-free at end of frame
```

**Sources:** ruff (typed_arena for parsed annotations), zed (custom Arena for render elements),
rust-analyzer (arena allocation for syntax trees), wasmi (allocation recycling for
translation/validation).

### 2.8 Use cache-line padding for shared concurrent data

Prevent false sharing by aligning frequently-contended fields to cache line boundaries.

```rust
#[cfg_attr(
    any(target_arch = "x86_64", target_arch = "aarch64"),
    repr(align(128))
)]
#[cfg_attr(
    any(target_arch = "arm", target_arch = "mips", target_arch = "mips64"),
    repr(align(32))
)]
pub(crate) struct CachePadded<T> {
    value: T,
}
```

**Source:** tokio (CachePadded with per-architecture alignment, sourced from Intel manuals,
Go, and Linux kernel).

### 2.9 Tune release profiles per crate

Hot crates (parsers, core engines) benefit from `codegen-units = 1` for maximum optimization.
Other crates can use higher values for faster compile times.

```toml
[profile.release]
lto = "thin"

[profile.release.package.my_parser]
codegen-units = 1

[profile.release.package.my_ast]
codegen-units = 1
```

**Sources:** ruff (`codegen-units = 1` for parser and AST crates, fat LTO for release),
zed (thin LTO, `codegen-units = 1` for release), helix (separate `opt` profile with fat LTO).

### 2.10 Add compile-time size assertions for critical types

Prevent accidental size regressions in types that appear in hot paths or large collections.

```rust
const _: () = assert!(std::mem::size_of::<Value>() <= 56);
const _: () = assert!(std::mem::size_of::<Rule>() == 2);
const _: () = assert!(std::mem::size_of::<Error>() == 8);
```

**Sources:** nushell (Value <= 56 bytes), ruff (Rule == 2 bytes), wasmi (Error == 8 bytes,
Op == 24 bytes).

### 2.11 Use `#[inline]` and `#[inline(always)]` judiciously

- `#[inline]` on small functions that cross crate boundaries
- `#[inline(always)]` only for genuinely critical hot-path functions (SIMD wrappers,
  trait method impls that must be monomorphized)
- `#[inline(never)]` on cold error paths and overflow handlers to keep the hot path compact
- Consider feature-gating aggressive inlining

```rust
#[cfg_attr(feature = "perf-inline", inline(always))]
fn next_state(&self, current: StateID, byte: u8) -> StateID { ... }

#[inline(never)]
fn push_overflow(&mut self) { ... }
```

**Sources:** regex (`perf-inline` feature gates `#[inline(always)]`), memchr (`#[inline(always)]`
on SIMD trait methods), tokio (`#[inline(never)]` on `push_overflow`).

### 2.12 Use branch hints for hot/cold paths

On stable Rust, use the `#[cold]` attribute pattern:

```rust
#[cold]
#[inline]
fn cold() {}

#[inline]
pub fn likely(condition: bool) -> bool {
    if !condition { cold() }
    condition
}

#[inline]
pub fn unlikely(condition: bool) -> bool {
    if condition { cold() }
    condition
}
```

**Source:** wasmi (custom `likely`/`unlikely`/`cold` hints for stable Rust).

### 2.13 Provide `_into` variants for allocation amortization

When a function returns an allocated buffer, also provide a variant that writes into a
caller-provided buffer, enabling reuse across calls.

```rust
pub fn to_lowercase(&self) -> Vec<u8> { ... }
pub fn to_lowercase_into(&self, buf: &mut Vec<u8>) { ... }

pub fn to_str_lossy(&self) -> Cow<'_, str> { ... }
pub fn to_str_lossy_into(&self, dest: &mut String) { ... }
```

**Source:** bstr (every allocating method has an `_into` variant).

### 2.14 Use file-level parallelism with rayon for embarrassingly parallel work

When work items are independent (linting files, processing records), use `rayon::par_iter()`
for trivial parallelism with no cross-item synchronization.

```rust
let results: Vec<_> = files
    .par_iter()
    .filter_map(|file| lint_file(file).ok())
    .collect();
```

**Sources:** ruff (par_iter over files), nushell (par-each command with configurable thread
pools).

---

## 3. Unsafe Code

### 3.1 Every `unsafe` block must have a `// SAFETY:` comment

The comment must explain **why** the safety invariants are satisfied, not just restate what
the code does.

```rust
// SAFETY: `state` is always <= 96 and `class` is always <= 11, so
// the maximum index is 107. STATES_FORWARD.len() == 108, therefore
// every index is in bounds by construction of the DFA.
unsafe {
    *STATES_FORWARD.get_unchecked(state + class as usize)
}
```

**Sources:** All 11 codebases follow this convention. tokio, memchr, and wasmi are
particularly rigorous.

### 3.2 Use `debug_assert!` inside unsafe code to catch violations during testing

Invariants that are relied upon for safety should be checked in debug builds.

```rust
pub unsafe fn get(&self, index: usize) -> &T {
    debug_assert!(index < self.len, "index {index} out of bounds (len: {})", self.len);
    &*self.ptr.add(index)
}
```

**Sources:** memchr (extensive `debug_assert!` in SIMD code), wasmi (`extra-checks` feature
adds runtime invariant checking), regex (debug-mode overflow checks in `int.rs`).

### 3.3 Document `unsafe trait` safety contracts

When a trait is `unsafe` to implement, document the exact invariants that implementors
must uphold.

```rust
/// # Safety
///
/// Implementations must guarantee that:
/// - `next_state`, given a valid state ID, always returns a valid state ID.
/// - `start_state` always returns a valid state ID or an error.
pub unsafe trait Automaton {
    fn next_state(&self, current: StateID, byte: u8) -> StateID;
    fn start_state(&self) -> Result<StateID, MatchError>;
}
```

**Sources:** regex (`unsafe trait Automaton`), tokio (`unsafe trait Link`),
memchr (`unsafe trait Vector`).

### 3.4 Use `unsafe impl Send`/`Sync` with documented safety arguments

When manually implementing `Send`/`Sync` for a type containing raw pointers, document
why it is safe.

```rust
// SAFETY: The raw pointers in `Stack` never leak outside the type and only
// point to heap memory owned by the `Stack`. The type provides no shared
// mutable access, so sending across threads is safe.
unsafe impl Send for EngineStacks {}
unsafe impl Sync for EngineStacks {}
```

**Sources:** tokio, wasmi, memchr (all have documented manual Send/Sync impls).

### 3.5 Add compile-time Send/Sync assertions

Ensure critical types maintain their expected auto-trait implementations across refactors.

```rust
#[cfg(test)]
const _: () = {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    fn assert_unwind_safe<T: std::panic::UnwindSafe + std::panic::RefUnwindSafe>() {}

    let _ = assert_send::<MyType>;
    let _ = assert_sync::<MyType>;
    let _ = assert_unwind_safe::<MyType>;
};
```

**Sources:** wasmi (compile-time assertions for Store, Inst, InstanceEntity),
bstr (oibits test for BStr, BString, Finder), memchr (Send/Sync regression test).

### 3.6 Use `#[repr(transparent)]` for zero-cost newtype casts

When a newtype wraps a single field, `#[repr(transparent)]` guarantees layout compatibility,
enabling safe pointer casts.

```rust
#[repr(transparent)]
pub struct BStr {
    bytes: [u8],
}

impl BStr {
    pub fn new(bytes: &[u8]) -> &BStr {
        // SAFETY: BStr is #[repr(transparent)] over [u8]
        unsafe { &*(bytes as *const [u8] as *const BStr) }
    }
}
```

**Sources:** bstr, regex (`StateID`, `PatternID`), wasmi (`Cell`, `Bits64`).

### 3.7 Confine unsafe to low-level infrastructure modules

Keep unsafe code concentrated in a few well-audited modules. Higher layers should be
entirely safe. Consider `#![forbid(unsafe_code)]` on safe layers.

```rust
// In the parser crate (no unsafe needed):
#![forbid(unsafe_code)]

// In the runtime/engine crate, specific modules opt out:
#![deny(unsafe_op_in_unsafe_fn)]
// Then in core.rs:
#![allow(unsafe_op_in_unsafe_fn)] // documented reason for this module
```

**Sources:** regex-syntax (`#![forbid(unsafe_code)]`), tokio (`#![deny(unsafe_op_in_unsafe_fn)]`
with per-module opt-out for readability in task infrastructure).

---

## 4. Type System

### 4.1 Use newtypes to prevent ID/index misuse

Wrap raw integers in distinct types to prevent accidental mixing of different kinds of IDs.
Use `PhantomData` for generic index types.

```rust
// Distinct types prevent mixing
pub struct ScopeId(NonZeroU32);
pub struct BindingId(NonZeroU32);

// Or with a generic marker:
pub struct Id<M, V = usize> {
    inner: V,
    _phantom: PhantomData<M>,
}
pub type VarId = Id<marker::Var>;
pub type DeclId = Id<marker::Decl>;
```

**Sources:** ruff (`newtype_index` macro), nushell (phantom-typed `Id<M, V>`),
wasmi (`for_each_index!` + `define_index!` macros), regex (`PatternID(SmallIndex)`).

### 4.2 Use `IndexVec<I, T>` for typed collections

Pair newtype indices with typed vectors to prevent indexing with the wrong ID type.

```rust
pub struct IndexVec<I, T> {
    raw: Vec<T>,
    _index: PhantomData<I>,
}

impl<I: Idx, T> std::ops::Index<I> for IndexVec<I, T> {
    type Output = T;
    fn index(&self, index: I) -> &T {
        &self.raw[index.index()]
    }
}
```

**Sources:** ruff (`IndexVec` for semantic IDs), wasmi (`Arena<Key, T>` with `ArenaKey` trait).

### 4.3 Use newtypes for physical units

Prevent accidental mixing of different unit types (pixels, rems, device pixels, bytes, chars).

```rust
#[derive(Copy, Clone)]
pub struct Pixels(pub f32);

#[derive(Copy, Clone)]
pub struct DevicePixels(pub i32);

#[derive(Copy, Clone)]
pub struct Rems(pub f32);

impl Pixels {
    pub fn scale(&self, factor: f32) -> ScaledPixels {
        ScaledPixels(self.0 * factor)
    }
}
```

**Source:** zed (distinct Pixels, DevicePixels, ScaledPixels, Rems types in GPUI).

### 4.4 Use `Cow<'_, T>` for zero-cost-when-valid conversions

When most inputs are valid and only a few require transformation, `Cow` avoids allocation
in the common case.

```rust
pub fn to_str_lossy(&self) -> Cow<'_, str> {
    match self.to_str() {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => {
            let mut buf = String::new();
            self.to_str_lossy_into(&mut buf);
            Cow::Owned(buf)
        }
    }
}
```

**Source:** bstr (Cow optimization: borrow when valid, allocate only on error).

### 4.5 Use `#[must_use]` on types and methods that should not be silently discarded

```rust
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ResponseFuture { ... }

#[must_use]
pub fn with_capacity(cap: usize) -> Self { ... }
```

**Sources:** tokio (`#[must_use]` on futures and state-transition results),
helix (on methods returning new values), ruff/tokio (`#[deny(unused_must_use)]`).

---

## 5. API Design

### 5.1 Use the Builder pattern for complex construction

Separate configuration (data) from construction (logic). Use `Option<T>` fields in config
so values can be layered without overwriting explicitly-set values.

```rust
pub struct Config {
    threads: Option<usize>,
    stack_size: Option<usize>,
}

impl Config {
    pub fn threads(mut self, n: usize) -> Self {
        self.threads = Some(n);
        self
    }
}

pub struct Builder { config: Config }

impl Builder {
    pub fn new() -> Self { ... }
    pub fn configure(mut self, config: Config) -> Self { self.config = config; self }
    pub fn build(self) -> Result<Runtime> { ... }
}
```

**Sources:** tokio (Runtime::Builder), regex (Config + Builder separation for every engine),
wasmi (Engine + Config).

### 5.2 Use sealed traits for extension methods

Prevent external implementations while keeping the trait public. This allows adding methods
to foreign types (like `[u8]`) without exposing the ability to implement the trait.

```rust
mod private {
    pub trait Sealed {}
}
impl private::Sealed for [u8] {}

pub trait ByteSlice: private::Sealed {
    fn to_str(&self) -> Result<&str, Utf8Error>;
    fn find(&self, needle: &[u8]) -> Option<usize>;
}

impl ByteSlice for [u8] { ... }
```

**Sources:** bstr (sealed `ByteSlice`/`ByteVec` extension traits),
zed (sealed `InputEvent` trait).

### 5.3 Provide layered APIs (simple and advanced)

Offer a simple, opinionated API for common use cases and an advanced API for power users.

```rust
// Simple: one function call
let re = Regex::new(r"foo(\w+)")?;

// Advanced: full control over engines and options
let re = regex_automata::meta::Builder::new()
    .configure(meta::Config::new().match_kind(MatchKind::All))
    .syntax(syntax::Config::new().unicode(false))
    .build(r"foo(\w+)")?;
```

**Sources:** regex (simple `regex` crate wrapping advanced `regex-automata`),
memchr (top-level functions vs `arch::*` SIMD types).

### 5.4 Use `AsRef`/`Into` for flexible input parameters

Accept broad input types in public APIs. Use `impl AsRef<[u8]>` for read-only parameters,
`impl Into<String>` for owned parameters.

```rust
pub fn find<B: AsRef<[u8]>>(&self, needle: B) -> Option<usize> {
    self.find_inner(needle.as_ref())
}
```

**Sources:** bstr (extensive `AsRef<[u8]>` usage), nushell (`FromValue`/`IntoValue` traits).

### 5.5 Use `impl Trait` in return position for iterator-heavy APIs

```rust
pub fn lines(&self) -> impl Iterator<Item = &[u8]> + '_ {
    self.split(b'\n')
}
```

### 5.6 Use `Context`/`AsContext` patterns for store-based architectures

When values are associated with a runtime store, use a context trait to enable ergonomic
API usage.

```rust
pub trait AsContext {
    type Data;
    fn as_context(&self) -> StoreContext<'_, Self::Data>;
}

pub fn call(&self, ctx: impl AsContextMut, args: &[Value]) -> Result<Vec<Value>> { ... }
```

**Source:** wasmi (AsContext/AsContextMut with blanket impls for `&T`, `&mut T`, `Store<T>`).

---

## 6. Trait Design

### 6.1 Use blanket impls to reduce boilerplate for implementors

Provide a simplified trait and a blanket implementation of the full trait.

```rust
pub trait AlwaysFixableViolation {
    fn message(&self) -> String;
    fn fix_title(&self) -> String; // Required -- always has a fix
}

// Blanket: any AlwaysFixableViolation automatically implements Violation
impl<V: AlwaysFixableViolation> Violation for V {
    const FIX_AVAILABILITY: FixAvailability = FixAvailability::Always;
    fn message(&self) -> String { AlwaysFixableViolation::message(self) }
    fn fix_title(&self) -> Option<String> { Some(AlwaysFixableViolation::fix_title(self)) }
}
```

**Source:** ruff (AlwaysFixableViolation blanket impl for Violation),
nushell (SimplePluginCommand blanket impl for PluginCommand).

### 6.2 Use dynamic dispatch intentionally when monomorphization is wasteful

When the code behind a trait object is too large to benefit from inlining, prefer
`Arc<dyn Trait>` or `Box<dyn Trait>` to reduce code bloat.

```rust
// The regex engine code is too large to inline -- dynamic dispatch costs are negligible
pub(super) struct Core {
    strategy: Arc<dyn Strategy>,
}
```

**Source:** regex (meta strategy uses `Arc<dyn Strategy>` with documented rationale).

### 6.3 Use `Clone`-for-`Box<dyn Trait>` helper traits

When you need `Clone` on trait objects, use a helper trait with a blanket impl.

```rust
pub trait CommandClone {
    fn clone_box(&self) -> Box<dyn Command>;
}

impl<T: 'static + Command + Clone> CommandClone for T {
    fn clone_box(&self) -> Box<dyn Command> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn Command> {
    fn clone(&self) -> Self { self.clone_box() }
}
```

**Source:** nushell (CommandClone pattern for `Box<dyn Command>`).

### 6.4 Use associated types and const generics for zero-cost abstraction

```rust
pub trait Item: Clone {
    type Summary: Summary;
    fn summary(&self) -> Self::Summary;
}

pub trait Summary: Clone {
    type Context<'a>: Copy;
    fn zero(cx: Self::Context<'_>) -> Self;
    fn add_summary(&mut self, other: &Self, cx: Self::Context<'_>);
}
```

**Source:** zed (SumTree trait hierarchy with associated types for multi-dimensional indexing).

---

## 7. Testing

### 7.1 Use snapshot testing for output-heavy assertions

Use `insta` for snapshot testing. Parameterize tests with `#[test_case]` for coverage.

```rust
#[test_case(Rule::UnusedImport, Path::new("F401_0.py"))]
#[test_case(Rule::UnusedImport, Path::new("F401_1.py"))]
fn rules(rule_code: Rule, path: &Path) -> Result<()> {
    let snapshot = format!("{}_{}", rule_code.noqa_code(), path.to_string_lossy());
    let diagnostics = lint_file(path, &settings_for(rule_code))?;
    insta::assert_snapshot!(snapshot, format_diagnostics(&diagnostics));
    Ok(())
}
```

**Source:** ruff (fixture + snapshot testing with test_case + insta).

### 7.2 Use shared TOML/data-driven test suites

Define tests as data (TOML, JSON, fixtures) and run the same suite against multiple
implementations. This ensures consistency across backends.

```rust
fn suite() -> RegexTests {
    let mut tests = RegexTests::new();
    const DATA: &[u8] = include_bytes!("../testdata/basic.toml");
    tests.load_slice("basic", DATA).unwrap();
    tests
}

// Same suite runs against PikeVM, DFA, hybrid DFA, backtracker...
```

**Source:** regex (TOML test suites shared across all engine implementations).

### 7.3 Test every command's examples automatically

If your system has commands/plugins with examples, verify them automatically.

```rust
impl Command for MyCommand {
    fn examples(&self) -> Vec<Example> {
        vec![Example {
            description: "Add two numbers",
            example: "2 + 3",
            result: Some(Value::int(5, Span::test_data())),
        }]
    }
}

#[test]
fn test_examples() {
    test_runner().examples(MyCommand).unwrap();
}
```

**Source:** nushell (every command's examples are automatically tested).

### 7.4 Use property-based testing with naive reference implementations

Test optimized implementations against trivially-correct reference versions.

```rust
quickcheck::quickcheck! {
    fn find_matches_naive(needle: u8, haystack: Vec<u8>) -> bool {
        optimized_memchr(needle, &haystack)
            == haystack.iter().position(|&b| b == needle)
    }
}
```

**Sources:** memchr (quickcheck against naive implementations), bstr (quickcheck for byteset).

### 7.5 Use loom for concurrency testing

Replace synchronization primitives with loom equivalents during testing to explore all
possible interleavings.

```rust
// In production:
use std::sync::atomic::AtomicUsize;

// Under loom:
#[cfg(loom)]
use loom::sync::atomic::AtomicUsize;

#[test]
fn concurrent_notify() {
    loom::model(|| {
        let notify = Arc::new(Notify::new());
        let notify2 = notify.clone();
        let th = thread::spawn(move || notify2.notified().await);
        notify.notify_one();
        th.join().unwrap();
    });
}
```

**Source:** tokio (entire synchronization layer is swappable via the `loom` module, reduced
queue sizes under loom for feasible state space exploration).

### 7.6 Use miri for unsafe code validation

Run tests under miri to detect undefined behavior. Provide miri-compatible code paths
where necessary.

```rust
#[cfg(miri)]
const EXPAND_LEN: usize = 6; // Reduced for miri performance

#[cfg(not(miri))]
const EXPAND_LEN: usize = 515;
```

**Sources:** tokio (miri-compatible pointer provenance via `ptr_expose`),
memchr (miri-aware test expansion), regex (`# if cfg!(miri) { return Ok(()); }`
in slow doc tests).

### 7.7 Use deterministic test scheduling for async tests

Provide a seeded RNG for task scheduling in tests, enabling reproducible failures.

```rust
#[gpui::test]
async fn test_example(cx: &TestAppContext) {
    // TestDispatcher uses a seeded PRNG for scheduling order
    // Failures print the seed for reproduction: SEED=42
    cx.run_until_parked(); // Advance until all tasks blocked
}
```

**Source:** zed (deterministic TestDispatcher with configurable seeds, `run_until_parked()`).

### 7.8 Test SIMD code at every alignment and size

Exercise all alignment and boundary conditions by iterating over both lengths and offsets.

```rust
#[test]
fn test_all_alignments() {
    for len in 0..517 {
        for align in 0..65 {
            let data = make_test_data(len, align);
            assert_eq!(
                optimized_search(&data),
                naive_search(&data),
                "failed at len={len}, align={align}"
            );
        }
    }
}
```

**Source:** memchr (alignment x size exhaustive testing), bstr (ASCII detection tests over
all alignments).

---

## 8. Documentation

### 8.1 Require docs on all public items

```rust
#![deny(missing_docs)]
#![warn(missing_debug_implementations)]
```

**Sources:** tokio, regex, regex-syntax, regex-automata (all deny missing_docs).

### 8.2 Document the "why", not just the "what"

Comments should explain design decisions, performance rationale, and safety arguments.

```rust
// We manually inline try_search_mayfail here because letting the
// compiler do it seems to produce pretty crappy codegen.

// We use f64 as the underlying type for technical reasons: using f64
// makes the compiler put values into floating point registers, freeing
// general-purpose registers for other handler parameters.
```

**Sources:** regex (inline reasoning comments), wasmi (technical rationale for register types).

### 8.3 Document module-level architecture

Use `//!` module-level docs to explain the purpose, design, and key abstractions of a module.

```rust
//! # Task Ownership Protocol
//!
//! This module implements the task state machine. The following reference
//! types exist:
//!
//! - `Task<S>`: The "spawn" reference, owned by the scheduler.
//! - `Notified`: Notification reference, used to wake the task.
//! - `JoinHandle<T>`: Owned by the user, used to await the result.
//!
//! The JOIN_WAKER protocol has 7 rules: ...
```

**Source:** tokio (120-line ownership protocol doc in task module), helix-event (module-level
architecture docs).

### 8.4 Document safety invariants for entire modules, not just functions

When a module has pervasive unsafe, document the safety model at the module level.

```rust
//! # Safety
//!
//! The functions in this module should all be considered `unsafe` but are not
//! marked as such to reduce noise. The safety invariants are:
//!
//! 1. `Ip` must always point to a valid instruction encoding.
//! 2. `Sp` must always point within the allocated stack frame.
//! 3. The `Header` pointer in a `RawTask` must remain valid for the
//!    task's lifetime.
```

**Sources:** tokio (task/core.rs module-level safety docs), wasmi (executor module safety).

### 8.5 Use doc comments as user-facing documentation that is automatically tested

For rule-based systems, derive user documentation from doc comments on rule types.

```rust
/// ## What it does
/// Checks for unused imports.
///
/// ## Why is this bad?
/// Unused imports add a performance overhead at runtime.
///
/// ## Example
/// ```python
/// import os  # unused
/// ```
///
/// Use instead:
/// ```python
/// # Remove the unused import
/// ```
#[derive(ViolationMetadata)]
pub struct UnusedImport { ... }
```

**Source:** ruff (doc comments become user-facing rule documentation, tested for presence).

---

## 9. Lints and Formatting

### 9.1 Recommended workspace-level lint configuration

```toml
[workspace.lints.rust]
unsafe_code = "warn"
unreachable_pub = "warn"
unused_must_use = "deny"

[workspace.lints.clippy]
# Enable pedantic as baseline (strict code quality)
pedantic = { level = "warn", priority = -2 }

# Selectively allow noisy pedantic lints
missing_errors_doc = "allow"
missing_panics_doc = "allow"
must_use_candidate = "allow"
too_many_lines = "allow"
module_name_repetitions = "allow"

# Deny dangerous patterns
dbg_macro = "deny"
print_stdout = "warn"
print_stderr = "warn"
todo = "deny"
get_unwrap = "warn"
```

**Source:** ruff (clippy pedantic enabled by default with selective exceptions).

### 9.2 Use `clippy.toml` to ban dangerous methods

```toml
# clippy.toml
disallowed-methods = [
    { path = "serde_json::from_reader", reason = "Use from_slice -- 10x faster" },
    { path = "std::process::Command::spawn", reason = "Blocks the thread. Use async." },
    { path = "std::env::var", reason = "Use the abstraction layer for testability" },
]
```

**Sources:** zed (bans blocking I/O and slow serde methods), ruff (bans direct env/fs access
in type checker crates).

### 9.3 Use `#[deny(unsafe_op_in_unsafe_fn)]`

Force explicit `unsafe` blocks even inside `unsafe fn`, making safety boundaries visible.

```rust
#![deny(unsafe_op_in_unsafe_fn)]

unsafe fn process(ptr: *const u8) {
    // Each unsafe operation requires its own block and SAFETY comment
    let val = unsafe {
        // SAFETY: ptr is guaranteed valid by caller contract
        *ptr
    };
}
```

**Source:** tokio (`deny(unsafe_op_in_unsafe_fn)` crate-wide, with per-module opt-out where
the density of unsafe would hurt readability).

### 9.4 Declare all custom `cfg` flags

Prevent typo-induced silent configuration bugs by declaring expected `cfg` values.

```toml
[workspace.lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = [
    'cfg(fuzzing)',
    'cfg(loom)',
    'cfg(tokio_unstable)',
] }
```

**Source:** tokio (pre-declares all custom cfg flags).

### 9.5 Use a consistent `rustfmt.toml`

```toml
edition = "2024"
max_width = 100  # or 79 for BurntSushi-style projects
use_small_heuristics = "max"
imports_granularity = "Crate"
```

**Sources:** wasmi (`imports_granularity = "Crate"`), memchr/regex/bstr (`max_width = 79`).

---

## 10. Project Structure

### 10.1 Use a multi-crate workspace with clear layering

Organize by responsibility with a strict dependency direction:

```
crate-cli/          # Binary, application-level errors (anyhow)
crate-engine/       # Core logic
crate-syntax/       # Parsing, AST
crate-ir/           # Internal representation
crate-types/        # Shared type definitions
crate-macros/       # Proc macros (separate crate required)
crate-test-support/ # Test utilities
```

Lower layers should not depend on higher layers.

**Sources:** All 11 codebases use multi-crate workspaces. ripgrep, ruff, wasmi, helix,
and nushell all demonstrate clean layering.

### 10.2 Use `pub(crate)` as the default visibility

Items should be as private as possible. Use `pub(crate)` for items shared within a crate
but not part of the public API. Use `#[warn(unreachable_pub)]` to catch items marked `pub`
that aren't actually reachable externally.

```rust
#![warn(unreachable_pub)]

pub(crate) struct InternalState { ... }
pub(crate) fn helper_function() { ... }
```

**Sources:** tokio (`warn(unreachable_pub)`), ruff (`pub(crate)` on all rule functions).

### 10.3 Use private modules with selective re-exports

Keep module internals private and control the public API surface through re-exports.

```rust
// lib.rs
mod selection;     // Private module
mod transaction;   // Private module

pub use selection::{Range, Selection};
pub use transaction::{ChangeSet, Transaction};
```

**Sources:** helix, bstr, nushell (private modules with pub use re-exports).

### 10.4 Centralize workspace dependency versions

Declare all dependency versions in the workspace `Cargo.toml` and reference them from
member crates.

```toml
# Root Cargo.toml
[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["full"] }

# Member Cargo.toml
[dependencies]
serde = { workspace = true }
tokio = { workspace = true }
```

**Sources:** All major workspaces (ruff, zed, tokio, nushell) use `workspace.dependencies`.

### 10.5 Inherit workspace lints in all crates

```toml
# Root Cargo.toml
[workspace.lints.clippy]
pedantic = { level = "warn", priority = -2 }

# Member Cargo.toml
[lints]
workspace = true
```

**Sources:** ruff, zed, nushell (all crates inherit workspace lints).

---

## 11. Concurrency

### 11.1 Pack atomic state into a single word

Combine flags, lifecycle state, and reference counts into a single `AtomicUsize` to enable
single-CAS state transitions.

```rust
// Bits 0-1: lifecycle (RUNNING, COMPLETE)
// Bit 2: NOTIFIED
// Bit 3: CANCELLED
// Bit 4: JOIN_INTEREST
// Bits 16-31: reference count
pub struct State(AtomicUsize);

impl State {
    fn transition_to_running(&self) -> TransitionResult {
        self.val.fetch_update_action(|snapshot| {
            // Single CAS for the entire state transition
        })
    }
}
```

**Source:** tokio (task state machine in a single AtomicUsize).

### 11.2 Return `#[must_use]` action enums from state transitions

Force callers to handle every possible outcome of a concurrent state transition.

```rust
#[must_use]
pub enum TransitionToRunning {
    Success,
    Cancelled,
    Failed,
    Dealloc,
}
```

**Source:** tokio (state transitions return must_use enums).

### 11.3 Use cooperative scheduling budgets

Prevent task starvation by limiting how many times a task can poll before yielding.

```rust
pub(crate) struct Budget(Option<u8>);

// Each leaf future decrements the budget
pub fn poll_proceed(cx: &mut Context<'_>) -> Poll<()> {
    // When budget exhausted, return Pending to yield
}
```

**Source:** tokio (cooperative scheduling with configurable budget).

### 11.4 Use `PhantomData` markers for thread-safety constraints

```rust
pub struct ForegroundExecutor {
    // Prevent accidentally sending across threads
    _not_send: PhantomData<Rc<()>>,
}
```

**Source:** zed (PhantomData markers on context types to enforce main-thread-only access).

---

## 12. Dependencies

### 12.1 Minimize dependencies and use `default-features = false`

Every dependency increases compile time and attack surface. Disable default features and
only enable what you need.

```toml
[dependencies]
serde = { version = "1", default-features = false, features = ["derive"] }
hashbrown = { version = "0.14", default-features = false, features = ["raw-entry", "inline-more"] }
```

### 12.2 Prefer well-known, focused crates

| Need | Crate |
|------|-------|
| Fast hashing | `rustc-hash` |
| Byte string search | `memchr` |
| Multi-pattern matching | `aho-corasick` |
| Small vectors | `smallvec` |
| Inline strings | `compact_str` or `smartstring` |
| Arena allocation | `typed-arena` or `bumpalo` |
| Error handling (libs) | `thiserror` |
| Error handling (apps) | `anyhow` |
| Error display | `miette` |
| Serialization | `serde` |
| Parallel iteration | `rayon` |
| Snapshot testing | `insta` |
| Parameterized tests | `test-case` |
| Property testing | `quickcheck` or `proptest` |

### 12.3 Use `cargo-shear` to detect unused dependencies

```toml
[workspace.metadata.cargo-shear]
ignored = ["getrandom"] # Allowlist for indirectly-needed crates
```

**Source:** ruff (cargo-shear integration).

---

## 13. Feature Flags

### 13.1 Keep default features minimal

Don't enable everything by default. Let users opt into what they need.

```toml
[features]
default = []
full = ["fs", "io-util", "net", "rt", "sync", "time"]
fs = []
net = ["dep:mio", "dep:socket2"]
```

**Source:** tokio (`default = []`, users must opt in).

### 13.2 Use `compile_error!` for invalid feature/platform combinations

```rust
#[cfg(all(not(tokio_unstable), target_family = "wasm", feature = "net"))]
compile_error!("The `net` feature is not supported on wasm.");

#[cfg(not(any(target_pointer_width = "32", target_pointer_width = "64")))]
compile_error!("This crate requires 32-bit or 64-bit pointer width.");
```

**Sources:** tokio (wasm feature guards, pointer width check),
memchr (unsupported pointer width check).

### 13.3 Use cfg macros to reduce `#[cfg(...)]` noise

When feature gates are complex, wrap them in declarative macros.

```rust
macro_rules! cfg_rt {
    ($($item:item)*) => {
        $(
            #[cfg(feature = "rt")]
            #[cfg_attr(docsrs, doc(cfg(feature = "rt")))]
            $item
        )*
    }
}

cfg_rt! {
    pub mod runtime;
    pub mod task;
}
```

**Source:** tokio (~40 cfg macros in `macros/cfg.rs`).

### 13.4 Conditionally derive serde traits

Don't force serde on all users. Gate it behind a feature.

```rust
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Config { ... }
```

**Sources:** bstr, ruff (serde behind feature flags).

---

## 14. Macros

### 14.1 Prefer functions over macros unless macros provide a clear advantage

Macros should be used for:
- Generating repetitive trait implementations across many types
- Compile-time code generation (derive macros, registration macros)
- Solving partial-borrow problems that functions cannot
- Creating DSLs that reduce boilerplate significantly

Do not use macros merely for convenience when a function would work.

### 14.2 Use macros for partial-borrow workarounds

When the borrow checker prevents splitting a struct borrow, macros can work around this
because they expand inline.

```rust
macro_rules! current {
    ($editor:expr) => {{
        let view = &mut $editor.tree.views[$editor.tree.focus];
        let doc = &mut $editor.documents[view.doc];
        (view, doc)
    }};
}
```

**Source:** helix (`current!`, `doc_mut!`, `view_mut!` macros).

### 14.3 Use proc macros for derive-based code generation

For repetitive patterns across many types, derive macros eliminate boilerplate and ensure
consistency.

```rust
#[derive(ViolationMetadata)]
#[violation_metadata(stable_since = "v0.0.18")]
pub struct UnusedImport {
    pub name: String,
}
```

**Sources:** ruff (`ViolationMetadata` derive, `map_codes` attribute macro),
nushell (`FromValue`/`IntoValue` derive macros), zed (`Action` derive macro).

### 14.4 Use declarative macros for cross-type trait implementation

When many types need the same trait implementation with minor variations, use `macro_rules!`.

```rust
macro_rules! impl_partial_eq {
    ($lhs:ty, $rhs:ty) => {
        impl PartialEq<$rhs> for $lhs {
            fn eq(&self, other: &$rhs) -> bool {
                self.as_bytes() == other.as_ref()
            }
        }
        impl PartialEq<$lhs> for $rhs {
            fn eq(&self, other: &$lhs) -> bool {
                self.as_ref() == other.as_bytes()
            }
        }
    };
}

impl_partial_eq!(BStr, [u8]);
impl_partial_eq!(BStr, str);
impl_partial_eq!(BStr, String);
```

**Source:** bstr (macro-generated bidirectional comparison matrix).

---

## Appendix: Quick Reference

### Crate-level lint template

```rust
#![warn(
    missing_docs,
    missing_debug_implementations,
    unreachable_pub,
    rust_2018_idioms,
)]
#![deny(
    unused_must_use,
    unsafe_op_in_unsafe_fn,
)]
```

### Workspace Cargo.toml template

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
edition = "2024"
rust-version = "1.80"

[workspace.lints.rust]
unsafe_code = "warn"
unreachable_pub = "warn"
unused_must_use = "deny"

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -2 }
missing_errors_doc = "allow"
must_use_candidate = "allow"
module_name_repetitions = "allow"
dbg_macro = "deny"
todo = "deny"
print_stdout = "warn"

[workspace.dependencies]
# Pin all versions here

[profile.release]
lto = "thin"
codegen-units = 1

[profile.dev.package."*"]
opt-level = 1 # Optimize deps even in dev
```

### Size assertion template

```rust
#[cfg(test)]
mod size_assertions {
    use super::*;
    const _: () = assert!(std::mem::size_of::<MyType>() <= 64);
    const _: () = assert!(
        std::mem::size_of::<MyId>() == std::mem::size_of::<Option<MyId>>()
    );
}
```

### Send/Sync assertion template

```rust
#[cfg(test)]
const _: () = {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    let _ = assert_send::<MyType>;
    let _ = assert_sync::<MyType>;
};
```
