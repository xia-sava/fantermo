use super::*;

/// 先頭のセルの文字の色。
fn first_fg(vt: &mut Vt) -> Rgb {
    vt.frame().unwrap().cells[0].fg
}

#[test]
fn dark_scheme_is_the_default_of_the_terminal() {
    use crate::config::{ANSI_COLOR_COUNT, DARK_COLORS};
    let mut vt = new_vt();
    let frame = vt.frame().unwrap();
    assert_eq!(frame.background, DARK_COLORS.background);
    assert_eq!(frame.foreground, DARK_COLORS.foreground);
    let palette = Palette::default();
    for index in 0..ANSI_COLOR_COUNT {
        assert_eq!(rgb(palette.0[index]), DARK_COLORS.ansi[index], "{index}");
    }
}

#[test]
fn default_colors_replace_the_background_the_text_and_16_colors() {
    use crate::config::LIGHT_COLORS;
    let mut vt = new_vt();
    vt.set_default_colors(&LIGHT_COLORS).unwrap();
    vt.feed(
        b"a\x1b[31mb\x1b[93mc\x1b[38;5;16md\x1b[38;5;196me\x1b[38;5;255mf\x1b[0m\x1b[44m \x1b[7mg",
    );
    let frame = vt.frame().unwrap();
    assert_eq!(frame.background, LIGHT_COLORS.background);
    assert_eq!(frame.foreground, LIGHT_COLORS.foreground);
    let fg = |col: u16| frame.cells.iter().find(|cell| cell.col == col).unwrap().fg;
    let bg = |col: u16| frame.cells.iter().find(|cell| cell.col == col).unwrap().bg;
    assert_eq!(fg(0), LIGHT_COLORS.foreground);
    assert_eq!(fg(1), LIGHT_COLORS.ansi[1]);
    assert_eq!(fg(2), LIGHT_COLORS.ansi[11]);
    // 256 色の残りは標準の色のまま。
    let standard = Palette::default();
    assert_eq!(fg(3), rgb(standard.0[16]));
    assert_eq!(fg(4), rgb(standard.0[196]));
    assert_eq!(fg(5), rgb(standard.0[255]));
    assert_eq!(bg(6), Some(LIGHT_COLORS.ansi[4]));
    // 反転（と選択範囲）は、配色の文字の色と背景色を入れ替える。
    assert_eq!(fg(7), LIGHT_COLORS.ansi[4]);
    let cursor = frame.cursor.unwrap();
    // カーソルは文字の色。
    assert_eq!(cursor.color, LIGHT_COLORS.foreground);
}

#[test]
fn colors_set_by_the_application_outlive_the_default_colors() {
    use crate::config::{DARK_COLORS, LIGHT_COLORS};
    let mut vt = new_vt();
    vt.set_default_colors(&DARK_COLORS).unwrap();
    // OSC 11 で背景色、OSC 4 で赤を変える。
    vt.feed(b"\x1b]11;rgb:12/34/56\x1b\\\x1b]4;1;rgb:01/02/03\x1b\\\x1b[31mr\x1b[32mg");
    vt.set_default_colors(&LIGHT_COLORS).unwrap();
    let frame = vt.frame().unwrap();
    let changed = Rgb {
        r: 0x12,
        g: 0x34,
        b: 0x56,
    };
    assert_eq!(frame.background, changed);
    assert_eq!(frame.foreground, LIGHT_COLORS.foreground);
    assert_eq!(first_fg(&mut vt), Rgb { r: 1, g: 2, b: 3 });
    assert_eq!(vt.frame().unwrap().cells[1].fg, LIGHT_COLORS.ansi[2]);
    // OSC 111 と OSC 104 で戻すと、既定の色になる。
    vt.feed(b"\x1b]111\x1b\\\x1b]104\x1b\\");
    let frame = vt.frame().unwrap();
    assert_eq!(frame.background, LIGHT_COLORS.background);
    assert_eq!(frame.cells[0].fg, LIGHT_COLORS.ansi[1]);
}

#[test]
fn color_queries_answer_the_default_colors() {
    use crate::config::LIGHT_COLORS;
    let mut vt = new_vt();
    vt.set_default_colors(&LIGHT_COLORS).unwrap();
    vt.take_replies();
    vt.feed(b"\x1b]11;?\x1b\\\x1b]10;?\x1b\\\x1b]4;3;?\x1b\\");
    let replies = String::from_utf8(vt.take_replies()).unwrap();
    assert_eq!(
        replies,
        "\x1b]11;rgb:ffff/ffff/ffff\x1b\\\x1b]10;rgb:3838/3a3a/4242\x1b\\\x1b]4;3;rgb:8a8a/6464/0000\x1b\\"
    );
}
