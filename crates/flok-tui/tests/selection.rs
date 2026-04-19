use anyhow::Result;
use flok_tui::{test_support::TestAppHarness, UiEvent};

fn chat_body_point(harness: &TestAppHarness) -> (u16, u16) {
    let rect = harness.chat_rect();
    (rect.x + 2, rect.y + 1)
}

#[test]
fn click_drag_in_chat_extracts_correct_text() -> Result<()> {
    let mut harness = TestAppHarness::new(80, 24)?;
    harness.add_user_message("hello world")?;

    let (start_col, row) = chat_body_point(&harness);
    harness.mouse_down_left(start_col, row, false)?;
    harness.mouse_drag_left(start_col + 4, row, false)?;

    assert!(harness.cell_is_reversed(start_col + 2, row));

    harness.mouse_up_left(start_col + 4, row, false)?;

    assert_eq!(harness.copied_text(), Some("hello"));
    assert!(harness.has_selection());
    Ok(())
}

#[test]
fn double_click_selects_word() -> Result<()> {
    let mut harness = TestAppHarness::new(80, 24)?;
    harness.add_user_message("hello world")?;

    let (col, row) = chat_body_point(&harness);
    harness.mouse_down_left(col, row, false)?;
    harness.mouse_up_left(col, row, false)?;
    harness.mouse_down_left(col, row, false)?;
    harness.mouse_up_left(col, row, false)?;

    assert_eq!(harness.copied_text(), Some("hello"));
    Ok(())
}

#[test]
fn triple_click_selects_line() -> Result<()> {
    let mut harness = TestAppHarness::new(80, 24)?;
    harness.add_user_message("hello world")?;

    let (col, row) = chat_body_point(&harness);
    for _ in 0..3 {
        harness.mouse_down_left(col, row, false)?;
        harness.mouse_up_left(col, row, false)?;
    }

    assert_eq!(harness.copied_text().map(str::trim), Some("hello world"));
    Ok(())
}

#[test]
fn click_drag_in_sidebar_extracts_correct_text() -> Result<()> {
    let mut harness = TestAppHarness::new(100, 24)?;
    harness.set_sidebar_visible(true)?;

    let sidebar = harness.sidebar_rect().expect("sidebar should be visible");
    let row = sidebar.y + 1;
    let col = sidebar.x + 1;
    harness.mouse_down_left(col, row, false)?;
    harness.mouse_drag_left(col + 4, row, false)?;
    harness.mouse_up_left(col + 4, row, false)?;

    assert_eq!(harness.copied_text(), Some("model"));
    Ok(())
}

#[test]
fn click_drag_in_composer_extracts_correct_text() -> Result<()> {
    let mut harness = TestAppHarness::new(80, 24)?;
    harness.set_composer_text("draft copy")?;

    let composer = harness.composer_rect();
    let row = composer.y + 1;
    let col = composer.x + 1;
    harness.mouse_down_left(col, row, false)?;
    harness.mouse_drag_left(col + 4, row, false)?;
    harness.mouse_up_left(col + 4, row, false)?;

    assert_eq!(harness.copied_text(), Some("draft"));
    Ok(())
}

#[test]
fn ctrl_c_with_selection_copies_and_does_not_quit() -> Result<()> {
    let mut harness = TestAppHarness::new(80, 24)?;
    harness.add_user_message("hello world")?;

    let (start_col, row) = chat_body_point(&harness);
    harness.mouse_down_left(start_col, row, false)?;
    harness.mouse_drag_left(start_col + 4, row, false)?;
    harness.mouse_up_left(start_col + 4, row, false)?;
    harness.ctrl_key('c')?;

    assert!(harness.is_running());
    assert!(!harness.has_selection());
    assert_eq!(harness.copied_text(), Some("hello"));
    Ok(())
}

#[test]
fn ctrl_c_without_selection_quits() -> Result<()> {
    let mut harness = TestAppHarness::new(80, 24)?;
    harness.ctrl_key('c')?;

    assert!(!harness.is_running());
    Ok(())
}

#[test]
fn shift_mouse_does_not_start_selection() -> Result<()> {
    let mut harness = TestAppHarness::new(80, 24)?;
    harness.add_user_message("hello world")?;

    let (col, row) = chat_body_point(&harness);
    harness.mouse_down_left(col, row, true)?;
    harness.mouse_drag_left(col + 4, row, true)?;
    harness.mouse_up_left(col + 4, row, true)?;

    assert!(!harness.has_selection());
    assert_eq!(harness.copied_text(), None);
    Ok(())
}

#[test]
fn scroll_locked_during_drag() -> Result<()> {
    let mut harness = TestAppHarness::new(80, 24)?;
    for index in 0..20 {
        harness.add_user_message(&format!("message {index}"))?;
    }

    let (col, row) = chat_body_point(&harness);
    harness.mouse_down_left(col, row, false)?;
    harness.mouse_drag_left(col + 3, row, false)?;

    harness.ui_event(UiEvent::TextDelta("stream lock".to_string()))?;
    let locked_offset = harness.chat_scroll_offset();
    harness.scroll_up(col, row)?;

    assert!(locked_offset > 0);
    assert_eq!(harness.chat_scroll_offset(), locked_offset);
    Ok(())
}

#[test]
fn overlay_blocks_selection_start() -> Result<()> {
    let mut harness = TestAppHarness::new(80, 24)?;
    harness.add_user_message("hello world")?;
    harness.open_permission_overlay()?;

    let (col, row) = chat_body_point(&harness);
    harness.mouse_down_left(col, row, false)?;
    harness.mouse_drag_left(col + 4, row, false)?;
    harness.mouse_up_left(col + 4, row, false)?;

    assert!(!harness.has_selection());
    assert_eq!(harness.copied_text(), None);
    Ok(())
}
