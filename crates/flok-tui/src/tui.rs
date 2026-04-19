//! Terminal wrapper used by the ratatui-based TUI.
//!
//! Owns the `Terminal<CrosstermBackend<Stdout>>` handle, installs the panic hook
//! that restores the terminal on unwind, and spawns a forwarder task that pipes
//! `crossterm::event::Event`s into the `AppEvent` channel consumed by the App
//! event loop.

use std::io::{stdout, Stdout};
use std::sync::Once;

use anyhow::{Context, Result};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::{backend::CrosstermBackend, Frame, Terminal};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;

use crate::app_event::AppEvent;

/// Ensures the panic hook is installed exactly once across all `Tui` instances.
static PANIC_HOOK: Once = Once::new();

/// Owning wrapper for the ratatui terminal plus the background forwarder task
/// that translates crossterm input events into `AppEvent`s.
pub(crate) struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    /// Handle to the forwarder task spawned on the current `LocalSet`.
    /// Aborted on drop so the task does not outlive the `Tui`.
    event_task: Option<JoinHandle<()>>,
}

impl Tui {
    /// Enter the alternate screen, enable raw mode, mouse capture, and bracketed
    /// paste, then spawn the crossterm `EventStream` → `AppEvent` forwarder on
    /// the current `tokio::task::LocalSet`.
    pub(crate) fn new(app_event_tx: mpsc::UnboundedSender<AppEvent>) -> Result<Self> {
        install_panic_hook();

        enable_raw_mode().context("enable raw mode")?;
        execute!(stdout(), EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste,)
            .context("enter alternate screen / enable mouse capture / bracketed paste")?;

        let backend = CrosstermBackend::new(stdout());
        let terminal = Terminal::new(backend).context("construct ratatui terminal")?;

        let event_task = tokio::task::spawn_local(forward_events(app_event_tx));

        Ok(Self { terminal, event_task: Some(event_task) })
    }

    /// Draw one frame via the supplied closure.
    pub(crate) fn draw<F: FnOnce(&mut Frame)>(&mut self, f: F) -> Result<()> {
        self.terminal.draw(f).context("draw frame")?;
        Ok(())
    }

    /// Leave the alternate screen, disable mouse capture / bracketed paste, and
    /// disable raw mode. Safe to call multiple times and without prior init —
    /// individual crossterm commands are best-effort so a partially-initialized
    /// terminal can still be cleaned up.
    ///
    /// The `Result<()>` return type is part of the contract used by panic handlers
    /// and future cleanup paths, even though the current implementation swallows
    /// individual crossterm errors.
    #[expect(clippy::unnecessary_wraps, reason = "Result kept for panic-hook / caller contract")]
    pub(crate) fn restore() -> Result<()> {
        // Best-effort cleanup: ignore individual failures so that later steps
        // still run. A partially-initialised terminal (e.g. raw mode enabled
        // but mouse capture not yet armed) must still end up usable.
        let _ =
            execute!(stdout(), DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen,);
        let _ = disable_raw_mode();
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        if let Some(handle) = self.event_task.take() {
            handle.abort();
        }
        let _ = Self::restore();
    }
}

fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Ignore errors — we are already panicking.
            let _ = Tui::restore();
            prev(info);
        }));
    });
}

/// Translate a crossterm event into an `AppEvent`. Returns `None` for events
/// that should be dropped (focus changes, key Release/Repeat for MVP).
fn map_event(event: Event) -> Option<AppEvent> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => Some(AppEvent::Key(key)),
        Event::Key(_) | Event::FocusGained | Event::FocusLost => None,
        Event::Mouse(mouse) => Some(AppEvent::Mouse(mouse)),
        Event::Paste(s) => Some(AppEvent::Paste(s)),
        Event::Resize(w, h) => Some(AppEvent::Resize(w, h)),
    }
}

/// Forwarder task body. Reads crossterm events, maps them, and pushes them
/// into the `AppEvent` channel. Exits cleanly when the stream closes, when a
/// stream error occurs, or when the receiver is dropped.
async fn forward_events(tx: mpsc::UnboundedSender<AppEvent>) {
    let mut stream = EventStream::new();
    while let Some(next) = stream.next().await {
        let event = match next {
            Ok(event) => event,
            Err(err) => {
                tracing::warn!(error = %err, "crossterm event stream error; stopping forwarder");
                break;
            }
        };
        let Some(app_event) = map_event(event) else {
            continue;
        };
        if tx.send(app_event).is_err() {
            // Receiver dropped: no more consumers, shut down the forwarder.
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn restore_is_safe_to_call_without_setup() {
        // Should not panic or error when called without prior init.
        let _ = Tui::restore();
    }

    #[tokio::test]
    async fn forwarder_stops_when_receiver_dropped() {
        // Construct a channel, drop receiver, send via forwarder logic — no
        // panic expected and `send` must report the error so the forwarder
        // can break out of its loop.
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        drop(rx);
        let result = tx.send(AppEvent::Tick);
        assert!(result.is_err());
    }

    #[test]
    fn map_event_filters_key_release_and_repeat() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

        let press = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        let release = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };
        let repeat = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Repeat,
            state: KeyEventState::NONE,
        };

        assert!(matches!(map_event(Event::Key(press)), Some(AppEvent::Key(_))));
        assert!(map_event(Event::Key(release)).is_none());
        assert!(map_event(Event::Key(repeat)).is_none());
    }

    #[test]
    fn map_event_ignores_focus_events() {
        assert!(map_event(Event::FocusGained).is_none());
        assert!(map_event(Event::FocusLost).is_none());
    }

    #[test]
    fn map_event_forwards_resize_paste_mouse() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 2,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        assert!(matches!(map_event(Event::Mouse(mouse)), Some(AppEvent::Mouse(_))));
        assert!(matches!(
            map_event(Event::Paste("hi".into())),
            Some(AppEvent::Paste(s)) if s == "hi"
        ));
        assert!(matches!(map_event(Event::Resize(80, 24)), Some(AppEvent::Resize(80, 24))));
    }
}
