# Spec 017 — iocraft TUI Migration

**Status:** In Progress
**Priority:** P0 — Architecture improvement
**Depends on:** 007 (TUI)

## Summary

Replace `ratatui` + `crossterm` with `iocraft` as the TUI framework. This is a
complete rewrite of `flok-tui` internals while keeping the exact same user-facing
behavior and the exact same `flok-core` / `flok-db` interfaces.

## Why

1. **Declarative component model** — React-like hooks, state, and `element!` macro
   replace imperative render-to-buffer code. Much easier to maintain and extend.
2. **Automatic flexbox layout** — taffy handles all layout math. No more manual `Rect`
   calculations.
3. **Built-in components** — `ScrollView`, `TextInput`, `Button` come free.
4. **Component-local events** — `use_local_terminal_events` gives mouse events scoped
   to a component's bounding box, which fundamentally solves panel-scoped selection.
5. **Canvas diffing** — only re-renders what changed.

## What Changes

| Layer | Impact |
|-------|--------|
| `flok-tui` | **Complete rewrite** |
| `flok` (binary) | Minor changes to initialization |
| `flok-core` | **Zero changes** |
| `flok-db` | **Zero changes** |

## Migration Phases

### Phase 1: Core Layout + Minimal Working TUI
- Replace `App` with iocraft `FlokApp` component
- Header bar component
- Message list with `ScrollView`
- Input box with `TextInput`
- Footer bar component
- Wire to `flok-core` session engine (same channels)

### Phase 2: Sidebar
- Sidebar component with session info, context, tasks, tools
- Toggle with Ctrl+B / `Tab`
- Plan/Build mode badge in header

### Phase 3: Message Rendering
- Markdown rendering via `MixedTextContent`
- Tool call inline display
- Thinking/reasoning blocks
- User/assistant/system message badges

### Phase 4: Dialogs + Selection
- Permission dialog (overlay with `Position::Absolute`)
- Question dialog
- Panel-scoped text selection using `use_local_terminal_events`
- Clipboard copy + "Copied!" toast

### Phase 5: Polish
- Paste detection
- Slash commands
- All keybindings
- Theme support (dark/light toggle)

## Architecture

```
FlokApp (root component)
├── Header
├── View (flex_direction: Row)
│   ├── View (flex_grow: 1, flex_direction: Column)
│   │   ├── ScrollView (messages)
│   │   │   └── MessageList
│   │   │       ├── UserMessage
│   │   │       ├── AssistantMessage (with markdown)
│   │   │       ├── ToolCallMessage
│   │   │       └── SystemMessage
│   │   └── InputBox (TextInput)
│   └── Sidebar (width: 42, conditional)
│       ├── SessionInfo
│       ├── ContextInfo
│       ├── TaskList
│       └── ToolList
├── Footer
└── PermissionDialog (Position::Absolute, conditional)
```

## Dependencies

Remove: `ratatui`, `tui-textarea`
Add: `iocraft` (path dependency from `../iocraft`)
Keep: `crossterm` (used by iocraft internally), `arboard`
