use super::*;

#[test]
fn feed_shows_text_on_screen() {
    let mut vt = new_vt();
    vt.feed("hello\r\n日本語 ○".as_bytes());
    assert_eq!(vt.plain_text().unwrap(), "hello\n日本語 ○");
}

#[test]
fn long_line_wraps_at_cols() {
    let mut vt = new_vt();
    vt.feed(b"abcdefghijklmnopqrstuvwxyz");
    assert_eq!(vt.plain_text().unwrap(), "abcdefghijklmnopqrst\nuvwxyz");
}

#[test]
fn cursor_position_report_is_queued_as_reply() {
    let mut vt = new_vt();
    vt.feed(b"ab\x1b[6n");
    assert_eq!(vt.take_replies(), b"\x1b[1;3R");
    assert!(vt.take_replies().is_empty());
}

#[test]
fn the_cursor_follows_text() {
    let mut vt = new_vt();
    vt.feed("ab\r\nあ".as_bytes());
    assert_eq!(vt.cursor_cell().unwrap(), (2, 1));
    let cursor = vt.frame().unwrap().cursor.unwrap();
    assert_eq!((cursor.col, cursor.row, cursor.width), (2, 1, 1));
}

/// ウィンドウの大きさを変えている間は、Windows の処理の上にウィンドウプロシージャが積まれて呼ばれ、
/// スタックの残りが少ない。そこで桁数を増やしても、スタックがあふれないこと。
#[test]
fn growing_columns_fits_in_a_small_stack() {
    const SMALL_STACK: usize = 256 * 1024;
    let worker = std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(|| {
            let mut vt = new_vt();
            for line in 0..30 {
                vt.feed(format!("line {line}: 日本語と ascii\r\n").as_bytes());
            }
            vt.resize(GridSize {
                cols: 200,
                rows: 30,
                ..SIZE
            })
            .unwrap();
        })
        .unwrap();
    worker.join().unwrap();
}

#[test]
fn resizing_through_many_sizes_keeps_working() {
    let mut vt = new_vt();
    for line in 0..30 {
        vt.feed(format!("line {line}: 日本語と ascii が混ざった長めの行です\r\n").as_bytes());
    }
    for (cols, rows) in [(1, 1), (3, 2), (80, 24), (2, 50), (200, 3), (20, 5)] {
        vt.resize(GridSize { cols, rows, ..SIZE }).unwrap();
        vt.frame().unwrap();
    }
}

#[test]
fn size_report_follows_resize() {
    let mut vt = new_vt();
    let resized = GridSize {
        cols: 40,
        rows: 10,
        ..SIZE
    };
    vt.resize(resized).unwrap();
    assert_eq!(vt.size(), resized);
    // CSI 18 t: テキスト領域の大きさを文字数で問い合わせる。
    vt.feed(b"\x1b[18t");
    assert_eq!(vt.take_replies(), b"\x1b[8;10;40t");
}
