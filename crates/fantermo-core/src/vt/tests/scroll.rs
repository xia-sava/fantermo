use super::*;

fn first_row_text(vt: &mut Vt) -> String {
    let frame = vt.frame().unwrap();
    frame
        .cells
        .iter()
        .filter(|cell| cell.row == 0)
        .map(|cell| cell.text.as_str())
        .collect()
}

#[test]
fn scrolling_shows_older_lines_and_returns_to_the_bottom() {
    let mut vt = new_vt();
    for line in 0..20 {
        vt.feed(format!("line{line:02}\r\n").as_bytes());
    }
    // 5 行の画面の先頭は line16（最後の行は空のプロンプトの行）。
    assert_eq!(first_row_text(&mut vt), "line16");
    vt.scroll(-10);
    assert_eq!(first_row_text(&mut vt), "line06");
    // 過去を表示している間は、カーソルは画面の外。
    assert!(vt.frame().unwrap().cursor.is_none());
    vt.scroll_to_bottom();
    assert_eq!(first_row_text(&mut vt), "line16");
}

fn position(total: usize, offset: usize, len: usize) -> ScrollPosition {
    ScrollPosition { total, offset, len }
}

#[test]
fn the_scroll_position_counts_rows_from_the_oldest_line() {
    let mut vt = new_vt();
    // スクロールバックがなければ、全体は画面だけ。
    assert_eq!(vt.scroll_position().unwrap(), position(5, 0, 5));
    for line in 0..20 {
        vt.feed(format!("line{line:02}\r\n").as_bytes());
    }
    // 21 行（最後は空のプロンプトの行）のうち、いちばん新しい 5 行を表示している。
    assert_eq!(vt.scroll_position().unwrap(), position(21, 16, 5));
    vt.scroll(-10);
    assert_eq!(vt.scroll_position().unwrap(), position(21, 6, 5));
    vt.scroll_to_row(0);
    assert_eq!(vt.scroll_position().unwrap(), position(21, 0, 5));
    assert_eq!(first_row_text(&mut vt), "line00");
    vt.scroll_to_bottom();
    assert_eq!(vt.scroll_position().unwrap(), position(21, 16, 5));
}

#[test]
fn scrolling_to_a_row_uses_the_same_count_as_the_offset() {
    let mut vt = new_vt();
    for line in 0..20 {
        vt.feed(format!("line{line:02}\r\n").as_bytes());
    }
    vt.scroll_to_row(3);
    assert_eq!(vt.scroll_position().unwrap().offset, 3);
    assert_eq!(first_row_text(&mut vt), "line03");
    // いちばん新しい行より先は、いちばん新しい行までに収まる。
    vt.scroll_to_row(1000);
    assert_eq!(vt.scroll_position().unwrap(), position(21, 16, 5));
    assert_eq!(first_row_text(&mut vt), "line16");
}

#[test]
fn the_alternate_screen_has_no_scrollback() {
    let mut vt = new_vt();
    for line in 0..20 {
        vt.feed(format!("line{line:02}\r\n").as_bytes());
    }
    assert!(!vt.is_alternate_screen().unwrap());
    vt.feed(b"\x1b[?1049h");
    assert!(vt.is_alternate_screen().unwrap());
    assert_eq!(vt.scroll_position().unwrap(), position(5, 0, 5));
    vt.scroll(-3);
    assert_eq!(vt.scroll_position().unwrap(), position(5, 0, 5));
    // 代替画面を抜ければ、スクロールバックが戻る。
    vt.feed(b"\x1b[?1049l");
    assert!(!vt.is_alternate_screen().unwrap());
    assert_eq!(vt.scroll_position().unwrap(), position(21, 16, 5));
}
