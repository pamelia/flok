# Feature Specification: Terminal UI

**Feature Branch**: `007-tui`
**Created**: 2026-03-28
**Status**: Draft

## User Scenarios & Testing

### User Story 1 - Developer Interacts with Flok in the Terminal (Priority: P0)
**Why this priority**: The TUI is the primary user interface.
**Acceptance Scenarios**:
1. **Given** the user runs `flok`, **When** the TUI launches, **Then** it displays a prompt input area, session history (if resuming), and status bar within 200ms.
2. **Given** the user types a message and presses Enter, **When** the LLM streams a response, **Then** text appears token-by-token with < 5ms rendering latency per chunk.
3. **Given** the LLM calls tools, **When** tool execution progresses, **Then** the TUI shows tool name, status (running/completed/error), and a collapsible output preview.

### User Story 2 - Developer Monitors Agent Team Activity (Priority: P0)
**Why this priority**: Visibility into multi-agent activity is critical for trust and debugging.
**Acceptance Scenarios**:
1. **Given** an agent team is active, **When** the user views the sidebar, **Then** they see the team name, each member's agent type, and status indicator (working/completed/failed/cancelled).
2. **Given** a team member completes, **When** its status changes, **Then** the sidebar updates within 100ms (reactive, no polling).
3. **Given** 5+ agents are running concurrently, **When** multiple agents produce output, **Then** the TUI remains responsive (< 16ms frame time, 60fps).

### User Story 3 - Developer Navigates Sessions and History (Priority: P1)
**Why this priority**: Session management is essential for long-running projects.
**Acceptance Scenarios**:
1. **Given** the user presses the session list keybind, **When** the session picker opens, **Then** it shows recent sessions sorted by last activity, with fuzzy search.
2. **Given** the user selects a previous session, **When** it loads, **Then** the full message history is rendered from the database within 100ms.
3. **Given** the user wants to manage models, **When** they press the model picker keybind, **Then** available models are listed with provider, name, and cost info.

### User Story 4 - Developer Uses Keyboard Shortcuts (Priority: P1)
**Why this priority**: Power users need efficient navigation.
**Acceptance Scenarios**:
1. **Given** the user presses `Ctrl+C` during a streaming response, **When** the cancel signal fires, **Then** the stream is aborted and the LLM stops within 500ms.
2. **Given** the user presses `Ctrl+P` (or configured keybind), **When** the command palette opens, **Then** all available actions are listed with fuzzy search.
3. **Given** the user presses `Escape`, **When** a dialog is open, **Then** the dialog closes without side effects.

### Edge Cases
- Terminal resize during streaming: re-layout immediately, no text loss
- Very long tool output (>1000 lines): virtualized scrolling, only render visible lines
- Non-UTF8 terminal output: escape invalid bytes, don't crash
- Terminal doesn't support 256 colors: fall back to basic 16 colors
- SSH session with high latency: batch rendering updates, don't flood the terminal
- Screen reader / accessibility: ensure text-based output is linear and parseable

## Requirements

### Functional Requirements

- **FR-001**: Flok MUST use `iocraft` as the TUI framework (declarative React-like component model with flexbox layout via taffy).
- **FR-002**: The TUI MUST have the following layout areas:
  - **Header**: Model name, session title, token count, cost
  - **Main area**: Scrollable message history with markdown rendering
  - **Input area**: Multi-line text input with history
  - **Sidebar** (togglable): Active teams, agent statuses, todo list
  - **Status bar**: Connection status, keybind hints
- **FR-003**: The TUI MUST render at 60fps (< 16ms per frame) even during active streaming.
- **FR-004**: Message rendering MUST support:
  - Markdown formatting (headings, bold, italic, code blocks, lists, links)
  - Syntax highlighting in code blocks (tree-sitter or syntect)
  - Tool call blocks with expandable/collapsible output
  - Reasoning/thinking blocks (visually distinct, collapsible)
  - File diffs (unified diff format with +/- coloring)
- **FR-005**: The TUI MUST support the following keybinds (configurable):

  | Default Keybind | Action |
  |-----------------|--------|
  | `Enter` | Send message |
  | `Shift+Enter` | New line in input |
  | `Ctrl+C` | Cancel current operation / Exit |
  | `Ctrl+P` | Command palette |
  | `Ctrl+N` | New session |
  | `Ctrl+L` | Clear screen |
  | `Ctrl+S` | Toggle sidebar |
  | `Ctrl+T` | Toggle theme (light/dark) |
  | `Up/Down` | Scroll message history |
  | `Tab` | Accept suggested completion |
  | `Escape` | Close dialog / Cancel input |

- **FR-006**: The TUI MUST support permission prompts inline -- when a tool requires user confirmation, a dialog appears with Allow/Deny/Always options.
- **FR-007**: The TUI MUST support a question dialog when the LLM asks the user a question (via the `question` tool), showing options as selectable items.
- **FR-008**: The TUI MUST display a todo list (from the `todowrite` tool) as a persistent panel, updated in real-time.
- **FR-009**: The TUI MUST support theming:
  - Built-in dark theme (default)
  - Built-in light theme
  - Configurable color scheme via `flok.toml`
- **FR-010**: The TUI MUST support clipboard operations (copy selected text, paste into input) using OS clipboard integration.

### Key Entities

```rust
pub enum Route {
    Home,           // Session list / welcome screen
    Session(SessionID),  // Active conversation
}

pub enum Dialog {
    Permission(PermissionRequest),
    Question(QuestionRequest),
    SessionPicker,
    ModelPicker,
    CommandPalette,
    Help,
    Confirm(String),
}

pub struct TuiState {
    pub route: Route,
    pub dialog: Option<Dialog>,
    pub sidebar_visible: bool,
    pub input: TextArea,
    pub scroll_offset: usize,
    pub theme: Theme,
    pub teams: Vec<TeamWithMembers>,
    pub todos: Vec<Todo>,
    pub streaming: bool,
}

pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub muted: Color,
    pub border: Color,
    pub code_bg: Color,
    pub selection: Color,
}
```

## Design

### Overview

The TUI is built on `iocraft`, a declarative React-like component framework for terminals with flexbox layout (via taffy). Components use hooks (`use_state`, `use_future`, `use_terminal_events`) for state management and async channel polling. State flows from the event bus through `use_future` hooks that update `State<T>` variables, triggering automatic re-renders. The design prioritizes: (1) sub-frame rendering latency for streaming, (2) canvas-diffing for minimal redraws, and (3) clean component boundaries (messages, sidebar, input are separate components).

### Detailed Design

#### Architecture: Event Loop

```rust
pub async fn run_tui(state: Arc<AppState>) -> Result<()> {
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut tui_state = TuiState::new();
    let mut bus_rx = state.bus.subscribe();

    // Event sources:
    // 1. Terminal input events (keyboard, mouse, resize)
    // 2. Bus events (session, team, streaming)
    // 3. Tick timer (for animations, cursor blink)

    let tick_rate = Duration::from_millis(16); // 60fps

    loop {
        // Render current state
        terminal.draw(|frame| render(frame, &tui_state, &state))?;

        // Wait for next event (with tick timeout)
        tokio::select! {
            // Terminal input
            event = crossterm_event_stream.next() => {
                handle_input(&mut tui_state, &state, event?).await?;
            }
            // Bus events (streaming deltas, team updates, etc.)
            event = bus_rx.recv() => {
                handle_bus_event(&mut tui_state, event?);
            }
            // Tick (for smooth animations)
            _ = tokio::time::sleep(tick_rate) => {
                // Cursor blink, spinner animation
            }
        }

        if tui_state.should_quit {
            break;
        }
    }

    Ok(())
}
```

#### Streaming Text Rendering

When the LLM is streaming, text deltas arrive as `BusEvent::PartUpdated` events. The TUI appends each delta to the current message's render buffer:

```rust
fn handle_bus_event(tui: &mut TuiState, event: BusEvent) {
    match event {
        BusEvent::PartUpdated { part_id, .. } => {
            // Append text delta to the active message's buffer
            // Re-render only the last message block (partial redraw)
            tui.mark_dirty(DirtyRegion::LastMessage);
        }
        BusEvent::MemberUpdated { team_id, agent, status } => {
            // Update sidebar team panel
            tui.update_team_member(team_id, agent, status);
            tui.mark_dirty(DirtyRegion::Sidebar);
        }
        _ => {}
    }
}
```

**Partial redraw optimization**: Instead of re-rendering the entire screen on each delta, we track dirty regions and only redraw the affected area. For streaming, this means only the last message block is re-rendered per delta.

#### Markdown Rendering

Messages are rendered with rich formatting using a custom markdown-to-iocraft component tree converter:

```rust
pub fn render_markdown(text: &str, width: u16, theme: &Theme) -> Vec<Line<'_>> {
    // Parse markdown with pulldown-cmark
    // Convert to iocraft elements with:
    //   - Bold: Style::new().bold()
    //   - Italic: Style::new().italic()
    //   - Code inline: Style::new().bg(theme.code_bg)
    //   - Code block: Syntax highlighted via syntect
    //   - Headings: Bold + accent color
    //   - Lists: Indented with bullet/number
    //   - Links: Underlined + accent color
}
```

Code block syntax highlighting uses `syntect` with a bundled set of common language definitions (Rust, Python, TypeScript, Go, Java, C, C++, Shell, YAML, JSON, TOML, Markdown, SQL).

#### Team Sidebar Panel

The sidebar displays active teams and their agents:

```
┌─ Teams ─────────────┐
│ ◆ spec-review-auth  │
│   ◌ clarity-reviewer│  ← yellow: working
│   ● completeness-re │  ← green: completed
│   ◌ product-reviewer│
│   ✕ api-reviewer    │  ← red: failed
│   ○ scope-reviewer  │  ← muted: cancelled
│                     │
│ ◆ code-review-pr-42 │
│   ● general         │
│   ● explore         │
└─────────────────────┘
```

Updated reactively via bus events -- no polling.

#### Real-Time Dashboard Features (Killer Feature #9)

The TUI provides a comprehensive dashboard for watching agents work:

**Live token usage and cost tracking:**
```
┌─ Session Stats ─────────────────────────┐
│ Tokens: 45,231 / 200,000  [██████░░ 23%]│
│ Cost:   $0.0847 (budget: $5.00)          │
│ Cache:  92% hit rate                      │
│ Model:  claude-sonnet-4.6 (build tier)   │
└──────────────────────────────────────────┘
```

**Context window usage meter:**
- Shows % full with color-coded thresholds:
  - Green (< 60%): healthy
  - Yellow (60-80%): T1 compaction active
  - Orange (80-95%): T2 compaction triggered
  - Red (> 95%): emergency compaction
- Compaction threshold indicators on the meter bar

**Agent-to-agent message flow visualization:**
```
┌─ Message Flow ──────────────────────────┐
│ lead ←── clarity-reviewer: "Findings.." │
│ lead ←── product-reviewer: "3 issues.." │
│ lead ──→ clarity-reviewer: "Challenge." │
│ lead ... waiting for scope-reviewer      │
└──────────────────────────────────────────┘
```

**File change diffs as they happen:**
- Real-time diff view showing files modified by agents
- Color-coded additions (+green) and deletions (-red)
- Grouped by agent/worktree

**Plan execution progress (spec-013):**
- Step-by-step plan display with status indicators
- DAG visualization showing parallel/sequential flow
- Current step highlighted, completed steps checked off

**One-key pane switching (like tmux but native):**
- `Ctrl+1` through `Ctrl+9` to switch between agent panes
- Each pane shows a specific agent's conversation
- `Ctrl+0` returns to the main (lead) view

The key insight from Spacebot and Agent of Empires: developers want to *watch* their agents work, not just see the final output.

#### Input Area

Multi-line text input with:
- Line wrapping at terminal width
- Cursor navigation (arrow keys, Home/End)
- History (Up/Down when on first/last line)
- Paste support (OS clipboard via `arboard` crate)
- Undo/redo within current input

#### Dialog System

Dialogs overlay the main content:

```rust
fn render_dialog(frame: &mut Frame, dialog: &Dialog, area: Rect, theme: &Theme) {
    match dialog {
        Dialog::Permission(req) => {
            // Show tool name, arguments, and Allow/Deny/Always buttons
            // User navigates with arrow keys, confirms with Enter
        }
        Dialog::Question(req) => {
            // Show question text and options as a selectable list
            // Custom text input option at bottom
        }
        Dialog::CommandPalette => {
            // Fuzzy-searchable list of all available actions
            // Powered by nucleo or similar fuzzy matcher
        }
        // ...
    }
}
```

### Communication with Backend

The TUI communicates with the session engine via the local HTTP API (same pattern as opencode). This enables:
- Decoupling TUI rendering from session processing
- Future: web UI, desktop app, IDE extensions all use the same API
- Clean separation of concerns

```
TUI ──HTTP──> axum server ──> Session Engine ──> Provider
                  ↑
                  └── SSE events for real-time updates
```

For the initial implementation, the TUI and server run in the same process. The HTTP calls are in-process (no network overhead). We use axum's `Router` directly without going through TCP.

### Alternatives Considered

1. **Ink (React for terminals) via wasm**: Rejected. Adds runtime dependency. `iocraft` provides a similar React-like model natively in Rust.
2. **TUI communicates directly with session engine (no HTTP)**: Considered but rejected. The HTTP API layer enables future UIs (web, desktop) without refactoring. The in-process HTTP overhead is negligible.
3. **Use iocraft `TextInput` for input**: Adopted. iocraft provides a built-in `TextInput` component with cursor, multiline, and scroll support.
4. **Image preview**: Deferred. Image support (for vision model responses) can be added later.

## Success Criteria

- **SC-001**: TUI startup to interactive prompt in < 200ms
- **SC-002**: Frame rendering time < 16ms (60fps) during streaming
- **SC-003**: Streaming text latency (bus event to rendered pixel) < 5ms
- **SC-004**: Session history load (1000 messages) and render < 200ms
- **SC-005**: Team sidebar update latency < 100ms on member status change
- **SC-006**: Input latency (keypress to rendered character) < 10ms

## Assumptions

- Most users have modern terminals with 256-color support (iTerm2, Alacritty, WezTerm, kitty)
- Terminal width is typically 80-200 columns
- Syntax highlighting for ~15 common languages covers 90%+ of use cases
- Users prefer keyboard-driven interaction (mouse support is secondary)

## Open Questions

- Should we support mouse interaction (click to expand tool outputs, scroll)?
- Should we support split panes (e.g., code preview alongside conversation)?
- Should we add a "compact" mode for narrow terminals (< 80 columns)?
- Should we support image rendering in terminals that support it (iTerm2 inline images, kitty graphics protocol)?
