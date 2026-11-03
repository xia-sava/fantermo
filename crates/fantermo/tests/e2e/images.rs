use crate::common;
use crate::common::Server;
use crate::ui::{
    assert_cell_is_drawn, cell_pixel, find_cell, number, run_with_cygwin, run_with_wsl,
    wait_sh_ready,
};

/// 画像の左上のセルの位置（1 から数える）。
const IMAGE_COL: u32 = 5;
const IMAGE_ROW: u32 = 3;
/// 色の許容の差。描画の丸めの分。
const COLOR_TOLERANCE: u8 = 2;

#[track_caller]
fn assert_color(actual: [u8; 3], expected: [u8; 3]) {
    let close = actual
        .iter()
        .zip(expected)
        .all(|(&a, e)| a.abs_diff(e) <= COLOR_TOLERANCE);
    assert!(close, "{actual:?} is not {expected:?}");
}

/// kitty graphics protocol で単色の画像を置き、絵のその位置の色を比べる。
fn exercise_kitty_image(server: &Server) {
    const COLUMNS: u32 = 6;
    const ROWS: u32 = 3;
    const WIDTH: usize = 16;
    const COLOR: [u8; 3] = [230, 20, 40];

    wait_sh_ready(server);
    let pixels = COLOR.repeat(WIDTH * WIDTH);
    server.send_line(&format!(
        r"printf '\033[2J\033[{IMAGE_ROW};{IMAGE_COL}H\033_Ga=T,f=24,s={WIDTH},v={WIDTH},c={COLUMNS},r={ROWS},i=1,q=2;{}\033\\'",
        common::base64(&pixels)
    ));
    let state = server.wait_until("an image", |state| number(&state["images"]) == 1);
    let picture = server.picture();
    let cell_width = u32::try_from(number(&state["cell_width"])).expect("cell width");
    let cell_height = u32::try_from(number(&state["cell_height"])).expect("cell height");
    let (x, y) = cell_pixel(
        &state,
        IMAGE_COL - 1,
        IMAGE_ROW - 1,
        COLUMNS * cell_width / 2,
        ROWS * cell_height / 2,
    );
    assert_color(picture.rgb(x, y), COLOR);
    let (x, y) = cell_pixel(&state, IMAGE_COL - 1 + COLUMNS, IMAGE_ROW - 1, 1, 1);
    let outside = picture.rgb(x, y);
    assert_ne!(outside, COLOR, "the image is wider than it should be");
}

/// sixel で単色の画像を置き、絵のその位置の色を比べる。
fn exercise_sixel_image(server: &Server) {
    // 色の登録の `#0;2;0;100;0` は緑。`~` は 6 ピクセルの縦の並び。2 行で 12 ピクセルの高さ。
    const COLOR: [u8; 3] = [0, 255, 0];
    const WIDTH: u32 = 48;
    const HEIGHT: u32 = 12;

    wait_sh_ready(server);
    server.send_line(&format!(
        r"printf '\033[2J\033[{IMAGE_ROW};{IMAGE_COL}H\033Pq#0;2;0;100;0#0!{WIDTH}~-!{WIDTH}~\033\\'"
    ));
    let state = server.wait_until("an image", |state| number(&state["images"]) == 1);
    let picture = server.picture();
    let (x, y) = cell_pixel(&state, IMAGE_COL - 1, IMAGE_ROW - 1, WIDTH / 2, HEIGHT / 2);
    assert_color(picture.rgb(x, y), COLOR);
}

#[test]
fn cygwin_kitty_image_is_drawn() {
    run_with_cygwin("cygwin_kitty_image_is_drawn", |server| {
        exercise_kitty_image(&server);
    });
}

#[test]
fn cygwin_sixel_image_is_drawn() {
    run_with_cygwin("cygwin_sixel_image_is_drawn", |server| {
        exercise_sixel_image(&server);
    });
}

#[test]
fn wsl_kitty_image_is_drawn() {
    run_with_wsl("wsl_kitty_image_is_drawn", |server| {
        exercise_kitty_image(&server);
    });
}

#[test]
fn wsl_sixel_image_is_drawn() {
    run_with_wsl("wsl_sixel_image_is_drawn", |server| {
        exercise_sixel_image(&server);
    });
}

/// グリフアトラスの一辺を小さくする環境変数。
const ATLAS_SIDE_ENV: &str = "FANTERMO_E2E_ATLAS_SIDE";
/// アトラスに入りきらない小ささにする一辺。
const TINY_ATLAS_SIDE: &str = "256";
/// 画面に出す漢字の最初と最後（U+4E00〜U+4FFF の 512 字。小さなアトラスには入りきらない）。
const FIRST_KANJI: char = '\u{4E00}';
const LAST_KANJI: char = '\u{4FFF}';

/// アトラスに入りきらない文字を出しても、入った文字（先頭の行）は描かれる。
#[test]
fn the_glyphs_that_fit_the_atlas_are_drawn_when_a_frame_has_more() {
    let server = Server::start_with_environment(
        "[font]\nsize = 10\n",
        &["powershell.exe", "-NoLogo", "-NoProfile"],
        |_| vec![(ATLAS_SIDE_ENV, TINY_ATLAS_SIDE.into())],
    );
    server.wait_text("PS ");
    server.resize(100, 30);
    // 先頭の行の印は、コマンドの文字列に現れない形で組み立てる。
    server.send_line(&format!(
        "Clear-Host; 'HEAD'+'LINE'; -join ((0x{:X}..0x{:X}) | ForEach-Object {{ [char]$_ }})",
        u32::from(FIRST_KANJI),
        u32::from(LAST_KANJI)
    ));
    let state = server.wait_text(&LAST_KANJI.to_string());
    let (col, row) = find_cell(&state, "HEADLINE");
    // 見出しは漢字の行より上にあり、先に場所を取る。
    let (_, kanji_row) = find_cell(&state, &FIRST_KANJI.to_string());
    assert!(row < kanji_row, "{state:#}");
    assert_cell_is_drawn(&server, &state, col, row);
    let log = server.log();
    assert!(
        log.contains("the glyph atlas cannot hold one frame, skipped"),
        "the atlas did not overflow: {log}"
    );
}
