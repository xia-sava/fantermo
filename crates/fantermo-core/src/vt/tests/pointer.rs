use super::*;

/// `col` 桁目・`row` 行目のセルの、左から `fraction` の割合の位置を指す。
fn pointer_at(col: u16, row: u16, fraction: f64) -> PointerPosition {
    PointerPosition {
        col,
        row,
        x: (f64::from(col) + fraction) * f64::from(SIZE.cell_width_px),
        y: (f64::from(row) + 0.5) * f64::from(SIZE.cell_height_px),
    }
}

fn pointer(col: u16, row: u16) -> PointerPosition {
    pointer_at(col, row, 0.5)
}

const DOUBLE_CLICK: Duration = Duration::from_millis(500);

#[test]
fn dragging_selects_text_and_marks_cells() {
    let mut vt = new_vt();
    vt.feed(b"hello world");
    vt.selection_press(pointer_at(0, 0, 0.1), Duration::ZERO, DOUBLE_CLICK)
        .unwrap();
    // セルの半分を越えたところまで動かすと、そのセルまで選ばれる。
    let end = pointer_at(4, 0, 0.9);
    vt.selection_drag(end, false).unwrap();
    vt.selection_release(end).unwrap();
    assert_eq!(vt.selected_text().unwrap().as_deref(), Some("hello"));

    let frame = vt.frame().unwrap();
    // 選んだセルは色が入れ替わり、背景色を持つ。
    assert_eq!(frame.cells[0].bg, Some(frame.foreground));
    assert_eq!(frame.cells[6].bg, None);

    vt.clear_selection().unwrap();
    assert_eq!(vt.selected_text().unwrap(), None);
}

#[test]
fn select_all_includes_the_scrollback() {
    let mut vt = new_vt();
    for line in 0..10 {
        vt.feed(format!("line{line}\r\n").as_bytes());
    }
    vt.select_all().unwrap();
    let text = vt.selected_text().unwrap().unwrap();
    assert!(text.starts_with("line0\n"));
    assert!(text.contains("line9"));
}

#[test]
fn double_click_selects_a_word() {
    let mut vt = new_vt();
    vt.feed(b"hello world");
    let at = pointer(7, 0);
    vt.selection_press(at, Duration::ZERO, DOUBLE_CLICK)
        .unwrap();
    vt.selection_release(at).unwrap();
    vt.selection_press(at, Duration::from_millis(100), DOUBLE_CLICK)
        .unwrap();
    vt.selection_release(at).unwrap();
    assert_eq!(vt.selected_text().unwrap().as_deref(), Some("world"));
}

fn mouse(action: MouseAction, col: u16, row: u16) -> MouseInput {
    let at = pointer(col, row);
    MouseInput {
        action,
        button: Some(MouseButton::Left),
        mods: Mods::empty(),
        x: at.x as f32,
        y: at.y as f32,
        any_button_pressed: action == MouseAction::Press,
    }
}

#[test]
fn mouse_is_not_reported_unless_requested() {
    let mut vt = new_vt();
    assert!(!vt.is_mouse_tracking().unwrap());
    let encoded = vt.encode_mouse(&mouse(MouseAction::Press, 2, 3)).unwrap();
    assert!(encoded.is_empty());
}

#[test]
fn mouse_clicks_are_reported_in_sgr_format() {
    let mut vt = new_vt();
    // ボタンの押し離しを報告させ、形式は SGR にする。
    vt.feed(b"\x1b[?1000h\x1b[?1006h");
    assert!(vt.is_mouse_tracking().unwrap());
    let press = vt.encode_mouse(&mouse(MouseAction::Press, 2, 3)).unwrap();
    assert_eq!(press, b"\x1b[<0;3;4M");
    let release = vt.encode_mouse(&mouse(MouseAction::Release, 2, 3)).unwrap();
    assert_eq!(release, b"\x1b[<0;3;4m");
}
