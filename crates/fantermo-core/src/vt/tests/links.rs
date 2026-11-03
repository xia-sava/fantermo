use super::*;

fn span(row: u16, cols: std::ops::Range<u16>) -> LinkSpan {
    LinkSpan { row, cols }
}

#[test]
fn osc_8_hyperlinks_are_found_with_their_cells() {
    let mut vt = new_vt();
    vt.feed(b"go \x1b]8;;https://example.com/a\x1b\\click here\x1b]8;;\x1b\\ end");
    let link = vt.link_at(9, 0).unwrap().unwrap();
    assert_eq!(link.uri, "https://example.com/a");
    // 空白のセルも含めて、同じリンクのセルをまとめる。
    assert_eq!(link.spans, [span(0, 3..13)]);
    assert_eq!(vt.link_at(2, 0).unwrap(), None);
    assert_eq!(vt.link_at(14, 0).unwrap(), None);
    // 長い URI も読める。
    let long = format!("https://example.com/{}", "a".repeat(URI_BUFFER_LEN * 2));
    vt.feed(format!("\r\n\x1b]8;id=x;{long}\x07link\x1b]8;;\x07").as_bytes());
    assert_eq!(vt.link_at(0, 1).unwrap().unwrap().uri, long);
}

#[test]
fn urls_in_text_are_found() {
    let mut vt = new_vt();
    vt.feed("見 https://a.jp/ だ".as_bytes());
    // 全角の文字は 2 セル。URL は 3 桁目から。
    let link = vt.link_at(8, 0).unwrap().unwrap();
    assert_eq!(link.uri, "https://a.jp/");
    assert_eq!(link.spans, [span(0, 3..16)]);
    assert_eq!(vt.link_at(2, 0).unwrap(), None);
    assert_eq!(vt.link_at(17, 0).unwrap(), None);
    // 画面の外は何もない。
    assert_eq!(vt.link_at(SIZE.cols, 0).unwrap(), None);
}

#[test]
fn wrapped_urls_are_joined() {
    let mut vt = new_vt();
    // 20 桁で折り返す。
    vt.feed(b"see https://example.com/abcdefghijklmnop.\r\nnext https://x.example");
    let expected = "https://example.com/abcdefghijklmnop";
    for (col, row) in [(4, 0), (19, 0), (0, 1), (19, 1)] {
        let link = vt.link_at(col, row).unwrap().unwrap();
        assert_eq!(link.uri, expected, "({col}, {row})");
        assert_eq!(link.spans, [span(0, 4..20), span(1, 0..20)]);
    }
    // 終わりの「.」は 3 行目に折り返している。
    assert_eq!(vt.link_at(0, 2).unwrap(), None);
    // 改行で終わった行の次の行は別の行。
    let link = vt.link_at(6, 3).unwrap().unwrap();
    assert_eq!(link.uri, "https://x.example");
}

#[test]
fn wrapped_urls_reach_into_the_scrollback() {
    let mut vt = new_vt();
    vt.feed(b"https://example.com/abcdefghijklmnopqrstuvwxyz");
    // URL は 3 行にわたる。5 行の改行で、表示している範囲の先頭は 4 行目になる。
    for _ in 0..SIZE.rows {
        vt.feed(b"\r\n");
    }
    vt.scroll(-2);
    // URL の 2 行目が表示している範囲の先頭にあり、1 行目は範囲の外。
    let link = vt.link_at(0, 0).unwrap().unwrap();
    assert_eq!(link.uri, "https://example.com/abcdefghijklmnopqrstuvwxyz");
    assert_eq!(link.spans, [span(0, 0..20), span(1, 0..6)]);
}

#[test]
fn the_link_underline_is_drawn_on_its_cells() {
    let mut vt = new_vt();
    vt.feed(b"\x1b]8;;https://example.com\x1b\\a b\x1b]8;;\x1b\\ \x1b[4:3mc\x1b[0m");
    vt.set_link_underline(vec![span(0, 0..3), span(0, 4..5)]);
    let underlines: Vec<(u16, Underline)> = vt
        .frame()
        .unwrap()
        .cells
        .iter()
        .map(|cell| (cell.col, cell.underline))
        .collect();
    // リンクの中の空白にも引く。下線を持つ文字はその下線のまま。
    assert_eq!(
        underlines,
        [
            (0, Underline::Single),
            (1, Underline::Single),
            (2, Underline::Single),
            (3, Underline::None),
            (4, Underline::Curly),
        ]
    );
    vt.set_link_underline(Vec::new());
    assert!(
        vt.frame()
            .unwrap()
            .cells
            .iter()
            .all(|cell| cell.col == 4 || cell.underline == Underline::None)
    );
}
