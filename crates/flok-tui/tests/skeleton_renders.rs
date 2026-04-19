use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn empty_frame_renders_blank() {
    let backend = TestBackend::new(10, 4);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|_f| {}).unwrap();
    let buf = terminal.backend().buffer();
    assert_eq!(buf.area.width, 10);
    assert_eq!(buf.area.height, 4);
}
