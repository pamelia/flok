# Spec 016 — Panel-Scoped Text Selection

**Status**: Accepted (2026-04-19 — feature shipped; spec retroactively locked to match built reality.)
**Priority:** P1 — Major UX issue
**Depends on:** 007 (TUI)

## Problem

When the user selects text in the terminal, the selection spans the full terminal
width, including both the main conversation area AND the sidebar. This means copying
text from a conversation always includes sidebar content mixed in, which is unusable.

This is a fundamental limitation of terminal text selection — the terminal treats the
screen as a flat character grid with no concept of panels or widgets.

opencode solved this by building a custom Zig-backed rendering engine (`opentui`) with
its own hit grid, mouse event handling, and container-scoped selection model. We need
a simpler solution that works within iocraft's component model.

## Solution: Application-Level Mouse Selection

Handle mouse events in the application, track selection state, render highlights, and
copy to the system clipboard. The terminal's native selection is effectively replaced.

### Architecture

```
Mouse down → identify panel from (x,y) → start selection in that panel
Mouse drag → extend selection within same panel only
Mouse up   → copy selected text to clipboard
Ctrl+C     → if selection active: copy + clear; else: quit
Esc        → clear selection
```

### Components

#### 1. `SelectionState` (in flok-tui)

```rust
pub struct SelectionState {
    /// Which panel the selection is in (Main, Sidebar, Input).
    panel: Panel,
    /// Anchor point (where mouse down happened).
    anchor: (u16, u16),
    /// Current point (where mouse is now).
    cursor: (u16, u16),
    /// Whether a selection is actively being dragged.
    is_dragging: bool,
}

pub enum Panel {
    Main,
    Sidebar,
    Input,
}
```

#### 2. Mouse Event Handling (in app event loop)

```rust
// In event_loop, add mouse event handling:
Event::Mouse(mouse) => {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let panel = identify_panel(mouse.column, mouse.row);
            self.selection = Some(SelectionState::start(panel, mouse.column, mouse.row));
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(ref mut sel) = self.selection {
                // Clamp to the panel where selection started
                let clamped = clamp_to_panel(sel.panel, mouse.column, mouse.row);
                sel.extend(clamped.0, clamped.1);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if let Some(sel) = self.selection.take() {
                let text = extract_selected_text(&sel, &self.rendered_lines);
                if !text.is_empty() {
                    copy_to_clipboard(&text);
                }
            }
        }
        _ => {}
    }
}
```

#### 3. Panel Identification

```rust
fn identify_panel(&self, x: u16, y: u16) -> Panel {
    // Use the stored Rect of each panel from the last render
    if self.sidebar_rect.contains(x, y) { Panel::Sidebar }
    else if self.input_rect.contains(x, y) { Panel::Input }
    else { Panel::Main }
}
```

Store the `Rect` of each panel during rendering so we can map coordinates back.

#### 4. Selection Rendering

During render, check if each cell falls within the selection bounds. If so, render
with inverted fg/bg colors:

```rust
// For each line in the message area:
if selection.is_active() && selection.panel == Panel::Main {
    if line_in_selection_range(line_y, &selection) {
        // Apply selection highlight style
        line_style = Style::default().bg(theme.selection).fg(theme.bg);
    }
}
```

#### 5. Text Extraction

```rust
fn extract_selected_text(sel: &SelectionState, lines: &[RenderedLine]) -> String {
    // Get the lines within the selection y-range
    // For each line, extract characters within the x-range
    // Join with newlines
}
```

This requires storing the rendered text content alongside position information.

#### 6. Clipboard

Use `arboard` crate for cross-platform clipboard access:

```rust
fn copy_to_clipboard(text: &str) {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set_text(text);
    }
}
```

### Implementation Plan

1. **Add dependencies**: `arboard` for clipboard
2. **Enable mouse capture**: `crossterm::event::EnableMouseCapture` in terminal setup
3. **Store panel rects**: Save `Rect` for each panel during render
4. **Add `SelectionState`**: Track selection anchor, cursor, panel
5. **Handle mouse events**: Down/Drag/Up in the event loop
6. **Render selection highlight**: Overlay inverted colors on selected cells
7. **Extract text + copy**: On mouse up, extract text and copy to clipboard
8. **Key integration**: Ctrl+C copies if selection active, Esc clears

### Rendering Approach

Two options for rendering the selection highlight:

**Option A: Post-render overlay**
After rendering all widgets, iterate over cells in the selection range and flip their
colors. This is simple but requires accessing the terminal buffer directly.

iocraft doesn't provide direct buffer access. Instead, use `use_local_terminal_events` for component-scoped mouse events.
We can iterate over cells in the selection range and modify their styles:

```rust
let buf = frame.buffer_mut();
for y in sel_start_y..=sel_end_y {
    let x_start = if y == sel_start_y { sel.anchor.0 } else { panel_rect.x };
    let x_end = if y == sel_end_y { sel.cursor.0 } else { panel_rect.right() };
    for x in x_start..=x_end {
        if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
            cell.set_style(selection_style);
        }
    }
}
```

**Option B: Style injection during render**
Pass the selection state to each render method and apply styles inline. More complex
but integrates better with the widget rendering.

**Recommendation: Option A** — simpler, less invasive, and `buffer_mut()` is the
approach using iocraft's `use_local_terminal_events` for component-scoped events.

### Edge Cases

- **Wrapped lines**: If text wraps, selection should follow the visual wrap, not the
  logical line. This means tracking selection in screen coordinates, not content coords.
- **Scrolled content**: Selection coordinates must account for the scroll offset.
- **Code blocks**: Selection inside code blocks should include the code, not the border
  characters.
- **Empty lines**: Selection should include empty lines between content.
- **Double-click**: Could select a word (future enhancement).
- **Triple-click**: Could select a line (future enhancement).

### Dependencies

- `arboard` — cross-platform clipboard (macOS, Linux, Windows)
- `crossterm` mouse events (already available, just not enabled)

### Effort Estimate

Medium (1-2 days):
- Mouse capture + event handling: 2-3 hours
- Selection state + panel identification: 1-2 hours
- Selection rendering (buffer overlay): 2-3 hours
- Text extraction: 1-2 hours
- Clipboard integration: 30 min
- Testing + edge cases: 2-3 hours

### Open Questions

- Should we disable terminal native selection entirely (some terminals allow this)?
- Should we show a "Copied!" toast when text is copied?
- Should double-click select a word? (Nice-to-have, not v1)
- Should we support shift+click to extend selection?
