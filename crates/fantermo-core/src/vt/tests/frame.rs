use super::*;

#[test]
fn graphemes_are_measured_like_the_terminal() {
    let widths = |text: &str| -> Vec<(String, u8)> {
        split_graphemes(text)
            .into_iter()
            .map(|grapheme| (grapheme.text, grapheme.width))
            .collect()
    };
    assert_eq!(
        widths("a端"),
        vec![("a".to_owned(), 1), ("端".to_owned(), 2)]
    );
    // 曖昧幅は 1。
    assert_eq!(widths("○"), vec![("○".to_owned(), 1)]);
    // 結合文字は前の文字と一つにまとまる。
    assert_eq!(widths("e\u{301}"), vec![("e\u{301}".to_owned(), 1)]);
    // 絵文字の ZWJ の並びは一つで 2 セル。
    assert_eq!(widths("👨\u{200d}👩"), vec![("👨\u{200d}👩".to_owned(), 2)]);
    assert_eq!(widths(""), vec![]);
}

#[test]
fn frame_lists_text_cells_with_widths() {
    let mut vt = new_vt();
    vt.feed("aあ○".as_bytes());
    let frame = vt.frame().unwrap();
    let cells: Vec<(u16, &str, u8)> = frame
        .cells
        .iter()
        .map(|cell| (cell.col, cell.text.as_str(), cell.width))
        .collect();
    // 曖昧幅の ○ は 1 セル。あ の右半分は並べない。
    assert_eq!(cells, [(0, "a", 1), (1, "あ", 2), (3, "○", 1)]);
    assert_eq!((frame.cols, frame.rows), (SIZE.cols, SIZE.rows));
}

#[test]
fn frame_resolves_colors_and_inverse() {
    let mut vt = new_vt();
    // 前景を 24 ビットの赤、次の文字は反転。
    vt.feed(b"\x1b[38;2;255;0;0mr\x1b[0m\x1b[7mi\x1b[0m");
    let frame = vt.frame().unwrap();
    let red = Rgb { r: 255, g: 0, b: 0 };
    assert_eq!(frame.cells[0].fg, red);
    assert_eq!(frame.cells[0].bg, None);
    assert_eq!(frame.cells[1].fg, frame.background);
    assert_eq!(frame.cells[1].bg, Some(frame.foreground));
}

/// `sgr` の SGR を付けた `text` を書き、最初のセルを返す。
fn styled_cell(sgr: &str, text: &str) -> CellView {
    let mut vt = new_vt();
    vt.feed(format!("\x1b[{sgr}m{text}\x1b[0m").as_bytes());
    vt.frame().unwrap().cells.remove(0)
}

#[test]
fn frame_passes_underline_styles() {
    for (sgr, expected) in [
        ("4", Underline::Single),
        ("4:1", Underline::Single),
        ("21", Underline::Double),
        ("4:2", Underline::Double),
        ("4:3", Underline::Curly),
        ("4:4", Underline::Dotted),
        ("4:5", Underline::Dashed),
    ] {
        assert_eq!(styled_cell(sgr, "a").underline, expected, "SGR {sgr}");
    }
    assert_eq!(styled_cell("4;24", "a").underline, Underline::None);
    assert_eq!(styled_cell("4:3;4:0", "a").underline, Underline::None);
}

#[test]
fn frame_keeps_decorated_blanks() {
    let mut vt = new_vt();
    // 下線を付けた空白は、文字がなくても下線を描くために並べる。
    vt.feed(b"a\x1b[4m  \x1b[0m b");
    let frame = vt.frame().unwrap();
    let underlined: Vec<(u16, Underline)> = frame
        .cells
        .iter()
        .map(|cell| (cell.col, cell.underline))
        .collect();
    assert_eq!(
        underlined,
        [
            (0, Underline::None),
            (1, Underline::Single),
            (2, Underline::Single),
            (3, Underline::None),
            (4, Underline::None),
        ]
    );
}

#[test]
fn frame_resolves_the_underline_color() {
    let mut vt = new_vt();
    // 指定がなければ None（文字と同じ色）。
    assert_eq!(styled_cell("4", "a").underline_color, None);
    assert_eq!(
        styled_cell("4;58:2::10:20:30", "a").underline_color,
        Some(Rgb {
            r: 10,
            g: 20,
            b: 30
        })
    );
    assert_eq!(
        styled_cell("4;58;2;10;20;30", "a").underline_color,
        Some(Rgb {
            r: 10,
            g: 20,
            b: 30
        })
    );
    // パレットの番号はパレットで引く。
    vt.feed(b"\x1b[4;58;5;196ma");
    let palette_red = vt.frame().unwrap().cells[0].underline_color;
    vt.feed(b"\x1b[0m\r\n\x1b[38;5;196mb");
    let frame = vt.frame().unwrap();
    assert_eq!(palette_red, Some(frame.cells[1].fg));
    assert_eq!(styled_cell("4;58;2;10;20;30;59", "a").underline_color, None);
}

#[test]
fn frame_passes_strikethrough_and_overline() {
    let cell = styled_cell("9", "a");
    assert!(cell.strikethrough && !cell.overline);
    let cell = styled_cell("53", "a");
    assert!(cell.overline && !cell.strikethrough);
    assert!(!styled_cell("9;29", "a").strikethrough);
    assert!(!styled_cell("53;55", "a").overline);
}

#[test]
fn frame_passes_bold_and_italic() {
    let cell = styled_cell("1;3", "a");
    assert!(cell.bold && cell.italic);
    let cell = styled_cell("1;3;22;23", "a");
    assert!(!cell.bold && !cell.italic);
}

#[test]
fn faint_text_is_blended_into_the_background() {
    let mut vt = new_vt();
    vt.feed(b"\x1b[2;38;2;200;100;0;48;2;0;0;100ma\x1b[0m\x1b[2mb");
    let frame = vt.frame().unwrap();
    assert_eq!(
        frame.cells[0].fg,
        Rgb {
            r: 100,
            g: 50,
            b: 50
        }
    );
    // 背景色がなければ既定の背景色に寄せる。
    let expected =
        |front: u8, back: u8| ((u16::from(front) + u16::from(back)) as f32 / 2.0).round() as u8;
    let (fg, bg) = (frame.foreground, frame.background);
    assert_eq!(
        frame.cells[1].fg,
        Rgb {
            r: expected(fg.r, bg.r),
            g: expected(fg.g, bg.g),
            b: expected(fg.b, bg.b)
        }
    );
}

#[test]
fn frame_passes_blink() {
    // 速い点滅（SGR 6）も同じ点滅として渡る。
    assert!(styled_cell("5", "a").blink);
    assert!(styled_cell("6", "a").blink);
    assert!(!styled_cell("5;25", "a").blink);
    assert!(!styled_cell("1", "a").blink);
    // 不可視の文字は点滅させない。
    assert!(!styled_cell("5;8;48;2;1;2;3", "a").blink);
}

#[test]
fn invisible_text_keeps_only_the_background() {
    let cell = styled_cell("8;4;9;53;48;2;1;2;3", "a");
    assert_eq!(cell.text, "");
    assert_eq!(cell.bg, Some(Rgb { r: 1, g: 2, b: 3 }));
    assert!(!cell.has_decoration());
    // 背景色も装飾もない不可視の文字は並べない。
    let mut vt = new_vt();
    vt.feed(b"\x1b[8ma");
    assert!(vt.frame().unwrap().cells.is_empty());
}
