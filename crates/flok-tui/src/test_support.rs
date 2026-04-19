use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::{
    app::App,
    app_event::AppEvent,
    history::HistoryItem,
    types::{TuiChannels, UiCommand, UiEvent},
};
use flok_core::{
    session::PlanMode,
    tool::{PermissionRequest, QuestionRequest, TodoList},
};

pub struct TestAppHarness {
    app: App,
    terminal: Terminal<TestBackend>,
    _cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
}

impl TestAppHarness {
    pub fn new(width: u16, height: u16) -> Result<Self> {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (_ui_tx, ui_rx) = mpsc::unbounded_channel();
        let (_bus_tx, bus_rx) = broadcast::channel(16);
        let (_perm_tx, perm_rx) = mpsc::unbounded_channel::<PermissionRequest>();
        let (_question_tx, question_rx) = mpsc::unbounded_channel::<QuestionRequest>();
        let channels = TuiChannels {
            cmd_tx,
            ui_rx,
            bus_rx,
            perm_rx,
            question_rx,
            todo_list: TodoList::new(),
            plan_mode: PlanMode::new(),
            model_name: "test-model".to_string(),
        };
        let (app_tx, app_rx) = mpsc::unbounded_channel();
        let mut app = App::new(channels, app_tx, app_rx);
        app.test_handle_event(AppEvent::Resize(width, height));
        let backend = TestBackend::new(width, height);
        let terminal = Terminal::new(backend).context("construct test terminal")?;
        let mut harness = Self { app, terminal, _cmd_rx: cmd_rx };
        harness.render()?;
        Ok(harness)
    }

    pub fn render(&mut self) -> Result<()> {
        self.terminal.draw(|frame| self.app.test_render(frame)).context("render test app")?;
        Ok(())
    }

    pub fn add_user_message(&mut self, text: &str) -> Result<()> {
        self.app.test_push_history_item(HistoryItem::user(text.to_string()));
        self.render()
    }

    pub fn add_assistant_message(&mut self, text: &str) -> Result<()> {
        self.app.test_push_history_item(HistoryItem::assistant(text.to_string(), true));
        self.render()
    }

    pub fn set_sidebar_visible(&mut self, visible: bool) -> Result<()> {
        self.app.test_set_sidebar_visible(visible);
        self.render()
    }

    pub fn set_composer_text(&mut self, text: &str) -> Result<()> {
        self.app.test_set_composer_text(text);
        self.render()
    }

    pub fn open_permission_overlay(&mut self) -> Result<()> {
        let (response_tx, _response_rx) = oneshot::channel();
        let request = PermissionRequest {
            tool: "read".to_string(),
            description: "read file".to_string(),
            always_pattern: "read *".to_string(),
            response_tx,
        };
        self.app.test_set_permission_overlay(request);
        self.render()
    }

    pub fn ctrl_key(&mut self, ch: char) -> Result<()> {
        self.app.test_handle_event(AppEvent::Key(KeyEvent::new(
            KeyCode::Char(ch),
            KeyModifiers::CONTROL,
        )));
        self.render()
    }

    pub fn mouse_down_left(&mut self, column: u16, row: u16, shift: bool) -> Result<()> {
        self.mouse(MouseEventKind::Down(MouseButton::Left), column, row, shift)
    }

    pub fn mouse_drag_left(&mut self, column: u16, row: u16, shift: bool) -> Result<()> {
        self.mouse(MouseEventKind::Drag(MouseButton::Left), column, row, shift)
    }

    pub fn mouse_up_left(&mut self, column: u16, row: u16, shift: bool) -> Result<()> {
        self.mouse(MouseEventKind::Up(MouseButton::Left), column, row, shift)
    }

    pub fn scroll_up(&mut self, column: u16, row: u16) -> Result<()> {
        self.mouse(MouseEventKind::ScrollUp, column, row, false)
    }

    pub fn scroll_down(&mut self, column: u16, row: u16) -> Result<()> {
        self.mouse(MouseEventKind::ScrollDown, column, row, false)
    }

    pub fn chat_rect(&self) -> Rect {
        self.app.test_layout_rects().chat
    }

    pub fn sidebar_rect(&self) -> Option<Rect> {
        self.app.test_layout_rects().sidebar
    }

    pub fn composer_rect(&self) -> Rect {
        self.app.test_layout_rects().composer
    }

    pub fn copied_text(&self) -> Option<&str> {
        self.app.test_copied_text()
    }

    pub fn is_running(&self) -> bool {
        self.app.test_is_running()
    }

    pub fn has_selection(&self) -> bool {
        self.app.test_has_selection()
    }

    pub fn chat_scroll_offset(&self) -> usize {
        self.app.test_chat_scroll_offset()
    }

    pub fn cell_is_reversed(&self, column: u16, row: u16) -> bool {
        self.terminal
            .backend()
            .buffer()
            .cell((column, row))
            .is_some_and(|cell| cell.modifier.contains(ratatui::style::Modifier::REVERSED))
    }

    pub fn ui_event(&mut self, event: UiEvent) -> Result<()> {
        self.app.test_handle_event(AppEvent::UiEvent(event));
        self.render()
    }

    fn mouse(&mut self, kind: MouseEventKind, column: u16, row: u16, shift: bool) -> Result<()> {
        let modifiers = if shift { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
        self.app.test_handle_event(AppEvent::Mouse(MouseEvent { kind, column, row, modifiers }));
        self.render()
    }
}
