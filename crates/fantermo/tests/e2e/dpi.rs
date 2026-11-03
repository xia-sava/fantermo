use crate::common;
use crate::common::CMD_COMMAND;
use crate::common::CMD_PROMPT;
use crate::common::Server;
use crate::ui::{
    BAND_BOTTOM, BAND_HEIGHT, BAND_TOP, BANDS_IMAGE, BANDS_SIDE, BackdropArea, DPI_100_PERCENT,
    DPI_150_PERCENT, DPI_200_PERCENT, DRAG_PIXELS, INK_DIFFERENCE, MIDDLE_LEFT, MIDDLE_RIGHT,
    ONE_PROFILE, PAGE_APPEARANCE, PICTURE_POLL_INTERVAL, PICTURE_TIMEOUT, Point,
    TERMINAL_BACKGROUND, TERMINAL_BACKGROUND_SETTING, UNEVEN_WINDOW, area_center,
    assert_cell_is_drawn, assert_controls_fit, assert_layout_fits_the_client,
    assert_the_shell_sees_the_grid, blend, cell_center, cell_left, cell_pixel, cell_right,
    change_dpi, check_terminal_still_works, click_main_menu, close_menu, color_is_close,
    current_window, fill_scrollback, find_cell, left_click, left_drag, newest_offset, number,
    on_close_button, on_scroll_bar, on_tab, on_thumb, open_menu, open_settings,
    over_terminal_background, press_cancel, quit, read_controls, saved_window, scroll_bar_edge,
    scroll_bar_middle, scroll_offset, seeded_state, selection, solid_image, start_settings_test,
    start_uneven, start_with_background, start_with_state, step, tab_count, thumb_length,
    thumb_top, wait_pixels, wait_tabs,
};
use fantermo_core::state;
use fantermo_core::state::WindowState;
use serde_json::Value;
use std::ffi::OsString;
use std::time::Instant;

const GET_DPI_SCALED_SIZE: &str = "getdpiscaledsize";

/// DPI を上げてもランナーの画面に収まる桁数と行数。
const DPI_TEST_GRID: (u16, u16) = (50, 12);
/// ピクセルに丸める分の許容の差。
const SCALE_TOLERANCE: u64 = 1;
/// セルの大きさの許容の差。切り上げと、DPI の比をかける分。
const CELL_SCALE_TOLERANCE: u64 = 2;
/// 文字のインクの高さの許容の差。ヒンティングとアンチエイリアスの分。
const INK_SCALE_TOLERANCE: u64 = 3;
/// フォントの大きさ（ピクセル）の許容の差。UI のフォントは OS が整数に丸めた値から求める。
const FONT_SCALE_TOLERANCE: f64 = 1.5;
/// `FANTERMO_DPI` の環境変数の名前。
const DPI_ENV: &str = "FANTERMO_DPI";

/// DPI で決まる寸法。同じ DPI なら、いつ読んでも同じになる。
fn dpi_layout(state: &Value) -> Value {
    serde_json::json!({
        "dpi": state["dpi"],
        "font_size_px": state["font_size_px"],
        "ui_font_size_px": state["ui_font_size_px"],
        "cols": state["cols"],
        "rows": state["rows"],
        "cell_width": state["cell_width"],
        "cell_height": state["cell_height"],
        "terminal_x": state["terminal_x"],
        "terminal_y": state["terminal_y"],
        "client_width": state["client_width"],
        "client_height": state["client_height"],
        "scroll_bar_x": state["scroll_bar"]["x"],
        "scroll_bar_y": state["scroll_bar"]["y"],
        "scroll_bar_width": state["scroll_bar"]["width"],
        "scroll_bar_height": state["scroll_bar"]["height"],
        "background_x": state["background"]["x"],
        "background_y": state["background"]["y"],
        "background_width": state["background"]["width"],
        "background_height": state["background"]["height"],
        "tab_bar": state["tab_bar"],
        "window": state["window"],
    })
}

/// タブバーの高さ。背景画像の範囲はタブバーのすぐ下から始まる。
fn tab_bar_height(state: &Value) -> u64 {
    number(&state["background"]["y"])
}

/// `state` のクライアント領域が、`previous` のクライアント領域を DPI の比で拡大・縮小した大きさで（丸めの分の差は
/// 許す）、ウィンドウの左上と枠の大きさは変わらないことを確認する。
#[track_caller]
fn assert_window_scaled(state: &Value, previous: &Value) {
    let (from, to) = (number(&previous["dpi"]), number(&state["dpi"]));
    for name in ["client_width", "client_height"] {
        let (value, base) = (number(&state[name]), number(&previous[name]));
        assert!(
            (value * from).abs_diff(base * to) <= SCALE_TOLERANCE * from,
            "{name}: {value} at {to} DPI, {base} at {from} DPI"
        );
    }
    let (window, before) = (current_window(state), current_window(previous));
    assert_eq!((window.left, window.top), (before.left, before.top));
    let frame = |window: &WindowState, state: &Value| {
        (
            i64::from(window.width) - i64::try_from(number(&state["client_width"])).unwrap(),
            i64::from(window.height) - i64::try_from(number(&state["client_height"])).unwrap(),
        )
    };
    assert_eq!(frame(&window, state), frame(&before, previous), "{state:#}");
}

/// `value` が、96 DPI のときの `base` を `dpi` に比例させた値と `tolerance` ピクセルの差の中にあることを確認する。
#[track_caller]
fn assert_scaled(name: &str, value: u64, base: u64, dpi: u64, tolerance: u64) {
    assert!(
        (value * DPI_100_PERCENT).abs_diff(base * dpi) <= tolerance * DPI_100_PERCENT,
        "{name}: {value} at {dpi} DPI, {base} at {DPI_100_PERCENT} DPI"
    );
}

/// `state` の寸法が、96 DPI で読んだ `base` を `dpi` に比例させた大きさであることを確認する。
#[track_caller]
fn assert_sizes_follow_the_dpi(state: &Value, base: &Value, dpi: u64) {
    let font_size = |state: &Value, name: &str| state[name].as_f64().expect("no font size");
    let ratio = dpi as f64 / DPI_100_PERCENT as f64;
    for name in ["font_size_px", "ui_font_size_px"] {
        assert!(
            (font_size(state, name) - font_size(base, name) * ratio).abs() <= FONT_SCALE_TOLERANCE,
            "{name}: {state:#}"
        );
    }
    for name in ["cell_width", "cell_height"] {
        assert_scaled(
            name,
            number(&state[name]),
            number(&base[name]),
            dpi,
            CELL_SCALE_TOLERANCE,
        );
    }
    assert_scaled(
        "padding",
        number(&state["terminal_x"]),
        number(&base["terminal_x"]),
        dpi,
        SCALE_TOLERANCE,
    );
    assert_scaled(
        "tab bar",
        tab_bar_height(state),
        tab_bar_height(base),
        dpi,
        SCALE_TOLERANCE,
    );
    assert_scaled(
        "scroll bar",
        number(&state["scroll_bar"]["width"]),
        number(&base["scroll_bar"]["width"]),
        dpi,
        SCALE_TOLERANCE,
    );
}

/// `FANTERMO_DPI` を `dpi` にして、`state.toml` を `saved` にして起動し、プロンプトが出るのを待つ。
fn start_at_dpi(dpi: u64, saved: &[u8]) -> (Server, Value) {
    let server = Server::start_with_environment("", &CMD_COMMAND, |dir| {
        std::fs::write(dir.join(common::STATE_FILE), saved).expect("failed to write the state");
        vec![(DPI_ENV, dpi.to_string().into())]
    });
    let state = server.wait_text(CMD_PROMPT);
    (server, state)
}

/// DPI を変えると、セル・フォント・余白・タブバー・スクロールバーが比例して大きくなり、ウィンドウの大きさも DPI の比で
/// 変わる。ウィンドウの左上は動かない。桁数と行数は新しいクライアント領域に入る数になり、どのタブも同じで、シェルにも届く。
#[test]
fn a_dpi_change_scales_the_window_and_its_parts() {
    let server = Server::start_powershell();
    server.wait_text("PS ");
    server.open_tab("PS ");
    server.resize(DPI_TEST_GRID.0, DPI_TEST_GRID.1);
    let base = server.state();
    assert_eq!(base["dpi"], DPI_100_PERCENT, "{base:#}");
    let monitor_dpi = base["monitor_dpi"].clone();
    let mut previous = base.clone();

    for dpi in [DPI_150_PERCENT, DPI_200_PERCENT, DPI_100_PERCENT] {
        let state = change_dpi(&server, dpi);
        assert_sizes_follow_the_dpi(&state, &base, dpi);
        assert_window_scaled(&state, &previous);
        assert_layout_fits_the_client(&state);
        // メッセージはモニターの DPI を変えない。
        assert_eq!(state["monitor_dpi"], monitor_dpi, "{state:#}");
        assert!(
            server.log().contains(&format!("at {dpi} DPI: cell")),
            "the font was not measured for {dpi} DPI\n{}",
            server.log()
        );
        assert_the_shell_sees_the_grid(&server, &state, &format!("at{dpi}"));
        previous = state;
    }
}

/// DPI が変わる前に問われる大きさ（`WM_GETDPISCALEDSIZE`）には答えず、Windows に任せる。問われただけではウィンドウは
/// 変わらない。
#[track_caller]
fn assert_the_size_is_left_to_windows(server: &Server, dpi: u64) {
    let before = server.state();
    let answer = server.post(&format!("/e2e/message/{GET_DPI_SCALED_SIZE}/{dpi}"), b"");
    let answer: Value = serde_json::from_slice(&answer).expect("the answer is not JSON");
    assert_eq!(answer["result"], 0, "{dpi}: {answer:#}");
    assert_eq!(answer["width"], 0, "{dpi}: {answer:#}");
    assert_eq!(answer["height"], 0, "{dpi}: {answer:#}");
    assert_eq!(
        dpi_layout(&server.state()),
        dpi_layout(&before),
        "asked {dpi}"
    );
}

/// DPI で決まる寸法。ウィンドウの大きさによらない。
fn dpi_parts(state: &Value) -> Value {
    serde_json::json!({
        "dpi": state["dpi"],
        "font_size_px": state["font_size_px"],
        "ui_font_size_px": state["ui_font_size_px"],
        "cell_width": state["cell_width"],
        "cell_height": state["cell_height"],
        "terminal_x": state["terminal_x"],
        "terminal_y": state["terminal_y"],
        "scroll_bar_width": state["scroll_bar"]["width"],
        "tab_bar_height": tab_bar_height(state),
    })
}

/// 96、144、192 の間を行き来しても、同じ DPI に戻るとセル・フォント・余白・タブバー・スクロールバーの寸法が一つも違わない
/// （誤差が積もらない）。ウィンドウはそのたびに DPI の比で変わる。変える前に問われる大きさには、そのたびに答えない。
#[test]
fn going_between_dpis_returns_to_the_same_parts() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.resize(DPI_TEST_GRID.0, DPI_TEST_GRID.1);
    let mut previous = server.state();
    let at_100 = dpi_parts(&previous);
    assert_the_size_is_left_to_windows(&server, DPI_150_PERCENT);
    let state = change_dpi(&server, DPI_150_PERCENT);
    assert_window_scaled(&state, &previous);
    let at_150 = dpi_parts(&state);
    previous = state;
    assert_the_size_is_left_to_windows(&server, DPI_200_PERCENT);
    let state = change_dpi(&server, DPI_200_PERCENT);
    assert_window_scaled(&state, &previous);
    let at_200 = dpi_parts(&state);
    previous = state;

    for (dpi, expected) in [
        (DPI_100_PERCENT, &at_100),
        (DPI_200_PERCENT, &at_200),
        (DPI_150_PERCENT, &at_150),
        (DPI_100_PERCENT, &at_100),
        (DPI_150_PERCENT, &at_150),
        (DPI_200_PERCENT, &at_200),
        (DPI_100_PERCENT, &at_100),
    ] {
        assert_the_size_is_left_to_windows(&server, dpi);
        let state = change_dpi(&server, dpi);
        assert_eq!(&dpi_parts(&state), expected, "at {dpi} DPI");
        assert_window_scaled(&state, &previous);
        assert_layout_fits_the_client(&state);
        previous = state;
    }
}

/// セルに割り切れない大きさのウィンドウも、DPI が変わると DPI の比で変わり、桁数と行数はその中に入る数になる。元の DPI に
/// 戻すと元の大きさに戻る。
#[test]
fn an_uneven_window_scales_by_the_dpi_ratio() {
    let (server, state) = start_uneven("", &CMD_COMMAND, CMD_PROMPT);

    let changed = change_dpi(&server, DPI_200_PERCENT);
    assert_window_scaled(&changed, &state);
    assert_layout_fits_the_client(&changed);
    let back = change_dpi(&server, DPI_100_PERCENT);
    assert_eq!(current_window(&back), UNEVEN_WINDOW, "{back:#}");
    assert_eq!(dpi_layout(&back), dpi_layout(&state));
}

/// 起動のときの DPI（`FANTERMO_DPI`）でも、起動のあとに変えても、同じ寸法になる。
#[test]
fn starting_at_a_dpi_gives_the_same_sizes_as_changing_to_it() {
    let names = [
        "dpi",
        "font_size_px",
        "ui_font_size_px",
        "cell_width",
        "cell_height",
        "terminal_x",
        "terminal_y",
    ];
    let reference = Server::start_cmd();
    let state = reference.wait_text(CMD_PROMPT);
    reference.resize(DPI_TEST_GRID.0, DPI_TEST_GRID.1);
    let monitor_dpi = number(&state["monitor_dpi"]);
    let window = current_window(&reference.state());
    let saved = state::to_toml(&seeded_state(WindowState {
        maximized: false,
        ..window
    }));

    // モニターの DPI が 144 か 192 なら、その DPI の起動は繰り返さない。
    let mut dpis = vec![DPI_150_PERCENT, DPI_200_PERCENT];
    if !dpis.contains(&monitor_dpi) {
        dpis.push(monitor_dpi);
    }
    for dpi in dpis {
        let changed = change_dpi(&reference, dpi);
        let (started, state) = start_at_dpi(dpi, saved.as_bytes());
        for name in names {
            assert_eq!(state[name], changed[name], "{name} at {dpi} DPI: {state:#}");
        }
        assert_eq!(
            state["scroll_bar"]["width"], changed["scroll_bar"]["width"],
            "at {dpi} DPI: {state:#}"
        );
        assert_eq!(
            tab_bar_height(&state),
            tab_bar_height(&changed),
            "at {dpi} DPI"
        );
        drop(started);
    }
}

/// `paint.ps1` が描く背景色のセルの色。
const PAINTED_CELL: [u8; 3] = [255, 0, 255];
/// `paint.ps1` が描く背景色のセルと、その右の文字のセルの数。
const PAINTED_CELLS: u32 = 4;
/// `paint.ps1` が描く文字。全体の文字のこの列は `paint.ps1` の行にしかない。
const PAINTED_TEXT: &str = "HHHH";
/// 描画を確認するときの行数。確認のたびに増える行が収まり、[`RUNNER_SCREEN`] にも収まる数。
const PAINT_ROWS: u16 = 16;
/// プロンプトを短くして、タブのタイトルを長くし、背景色のセルと文字を出すスクリプト。
const PAINT_SCRIPT: &str = r#"function global:prompt { 'e2e> ' }
$Host.UI.RawUI.WindowTitle = 'title-' * 20
$e = [char]27
[Console]::WriteLine("$($e)[48;2;255;0;255m    $($e)[0mHHHH")
"#;

/// 文字のインクの範囲（セルの左上からのピクセル）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ink {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

/// `(left, top)` から `width` × `height` の範囲で、`background` と違う画素の範囲。
fn ink_in(
    picture: &common::Picture,
    (left, top): (u32, u32),
    (width, height): (u32, u32),
    background: [u8; 3],
) -> Option<Ink> {
    let mut ink: Option<Ink> = None;
    for dy in 0..height {
        for dx in 0..width {
            let is_ink = picture
                .rgb(left + dx, top + dy)
                .iter()
                .zip(background)
                .any(|(&a, b)| a.abs_diff(b) > INK_DIFFERENCE);
            if is_ink {
                let found = ink.get_or_insert(Ink {
                    left: dx,
                    top: dy,
                    right: dx,
                    bottom: dy,
                });
                found.left = found.left.min(dx);
                found.top = found.top.min(dy);
                found.right = found.right.max(dx);
                found.bottom = found.bottom.max(dy);
            }
        }
    }
    ink
}

/// 背景色のセルの四隅が状態の値から求めた位置にあり、その外には出ていないことと、右の文字が各セルの真ん中にあることを
/// 確認する。文字のインクの範囲を返す。
#[track_caller]
fn assert_painted_cells_are_where_the_state_says(server: &Server, state: &Value) -> Ink {
    let picture = server.picture();
    let dimension = |name: &str| u32::try_from(number(&state[name])).expect("the size fits");
    let (client_width, client_height) = (dimension("client_width"), dimension("client_height"));
    assert_eq!(picture.size(), (client_width, client_height), "{state:#}");
    let (cell_width, cell_height) = (dimension("cell_width"), dimension("cell_height"));

    let (text_col, row) = find_cell(state, PAINTED_TEXT);
    let col = text_col - PAINTED_CELLS;
    let (left, top) = cell_pixel(state, col, row, 0, 0);
    let right = left + PAINTED_CELLS * cell_width - 1;
    let bottom = top + cell_height - 1;
    for (x, y) in [(left, top), (right, top), (left, bottom), (right, bottom)] {
        let actual = picture.rgb(x, y);
        assert!(
            color_is_close(actual, PAINTED_CELL),
            "({x}, {y}) is {actual:?}: {state:#}"
        );
    }
    let outside = [
        (left - 1, top),
        (left - 1, bottom),
        (left, top - 1),
        (right, top - 1),
        (left, bottom + 1),
        (right, bottom + 1),
    ];
    for (x, y) in outside {
        let actual = picture.rgb(x, y);
        assert!(
            !color_is_close(actual, PAINTED_CELL),
            "({x}, {y}) is {actual:?}: {state:#}"
        );
    }

    let mut first = None;
    for text in 0..PAINTED_CELLS {
        let origin = cell_pixel(state, text_col + text, row, 0, 0);
        let ink = ink_in(
            &picture,
            origin,
            (cell_width, cell_height),
            TERMINAL_BACKGROUND,
        )
        .unwrap_or_else(|| panic!("the cell of the letter {text} is blank: {state:#}"));
        assert!(
            (ink.left + ink.right).abs_diff(cell_width - 1) <= cell_width / 2,
            "the letter {text} is off the middle of its cell: {ink:?}: {state:#}"
        );
        assert_eq!(
            *first.get_or_insert(ink),
            ink,
            "the letter {text}: {state:#}"
        );
    }
    let empty = cell_pixel(state, text_col + PAINTED_CELLS, row, 0, 0);
    assert_eq!(
        ink_in(
            &picture,
            empty,
            (cell_width, cell_height),
            TERMINAL_BACKGROUND
        ),
        None,
        "the cell after the letters is not blank: {state:#}"
    );

    // タブのタイトルも描かれている。後ろ半分はアイコンの外で、長いタイトルが届く。
    let tab = &state["tab_bar"]["tabs"][0];
    let area = &tab["area"];
    let (x, y) = (number(&area["x"]), number(&area["y"]));
    let (width, height) = (number(&area["width"]), number(&area["height"]));
    let colors: Vec<[u8; 3]> = (x + width / 2..number(&tab["close"]["x"]))
        .map(|at| {
            picture.rgb(
                u32::try_from(at).expect("the position fits"),
                u32::try_from(y + height / 2).expect("the position fits"),
            )
        })
        .collect();
    assert!(
        colors.iter().any(|color| {
            color
                .iter()
                .zip(colors[0])
                .any(|(&a, b)| a.abs_diff(b) > INK_DIFFERENCE)
        }),
        "the tab title is blank: {state:#}"
    );
    first.expect("no letters were read")
}

/// 文字とセルの色は、DPI を変えたあとも状態の値が示す位置に、その DPI の大きさで描かれ、アトラスを作り直しても消えない。
/// DPI を変えたあとに初めて出す文字も描ける。元の DPI に戻すと同じ形に戻る。
#[test]
fn the_cells_are_drawn_where_the_state_says_after_a_dpi_change() {
    let command = [
        "powershell.exe",
        "-NoLogo",
        "-NoProfile",
        "-NoExit",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        "paint.ps1",
    ];
    let server = Server::start_with_setup(TERMINAL_BACKGROUND_SETTING, &command, |dir| {
        std::fs::write(dir.join("paint.ps1"), PAINT_SCRIPT).expect("failed to write the script");
    });
    server.wait_text(CMD_PROMPT);
    server.resize(DPI_TEST_GRID.0, PAINT_ROWS);
    let base_state = server.wait_until("the title in the tab", |state| {
        state["window_title"]
            .as_str()
            .is_some_and(|title| title.starts_with("title-"))
            && state["text"]
                .as_str()
                .is_some_and(|text| text.contains(PAINTED_TEXT))
    });
    let base = assert_painted_cells_are_where_the_state_says(&server, &base_state);

    for dpi in [DPI_150_PERCENT, DPI_200_PERCENT, DPI_100_PERCENT] {
        let state = change_dpi(&server, dpi);
        let ink = assert_painted_cells_are_where_the_state_says(&server, &state);
        assert_scaled(
            "ink height",
            u64::from(ink.bottom - ink.top),
            u64::from(base.bottom - base.top),
            dpi,
            INK_SCALE_TOLERANCE,
        );
        if dpi == DPI_100_PERCENT {
            assert_eq!(ink, base, "the letters did not come back: {state:#}");
        }

        // 新しいアトラスにまだない文字を出す。打った行には続きの形で出ない。
        server.type_line(&format!("'ne'+'wk{dpi}'"));
        let needle = format!("newk{dpi}");
        let state = server.wait_text(&needle);
        let (col, row) = find_cell(&state, &needle);
        assert_cell_is_drawn(&server, &state, col, row);
    }
}

/// DPI を変えたあとのマウスの当たり判定を、状態の値から求めた新しい座標で確認する。行の中の桁の境目、余白、スクロールバーの
/// 左端、タブの下端を押して、ひとつ外が別のものに当たることも見る。
#[test]
fn the_mouse_hits_the_new_positions_after_a_dpi_change() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.resize(DPI_TEST_GRID.0, DPI_TEST_GRID.1);
    for dpi in [DPI_200_PERCENT, DPI_150_PERCENT, DPI_100_PERCENT] {
        change_dpi(&server, dpi);
        check_selection_at_the_current_dpi(&server, dpi);
        check_scroll_bar_at_the_current_dpi(&server);
        check_tab_bar_at_the_current_dpi(&server);
    }
}

#[track_caller]
fn check_selection_at_the_current_dpi(server: &Server, dpi: u64) {
    server.type_line(&format!("echo alpha{dpi} beta-%ERRORLEVEL%-end"));
    let needle = format!("alpha{dpi} beta-0-end");
    let state = server.wait_text(&needle);
    let (col, row) = find_cell(&state, &needle);
    let word = u32::try_from(format!("alpha{dpi} ").len()).expect("the length fits");

    server.mouse(&left_drag(
        cell_left(&state, col + word, row),
        cell_right(&state, col + word + 3, row),
        "",
    ));
    assert_eq!(selection(&server.state()), Some("beta"), "at {dpi} DPI");

    // 左の余白から押すと、いちばん近い、行の先頭のセルから選ぶ。
    let (_, y) = cell_center(&state, col, row);
    server.mouse(&left_drag((0, y), cell_right(&state, col + 4, row), ""));
    assert_eq!(selection(&server.state()), Some("alpha"), "at {dpi} DPI");
}

#[track_caller]
fn check_scroll_bar_at_the_current_dpi(server: &Server) {
    let state = fill_scrollback(server);
    let newest = newest_offset(&state);
    let lines = number(&state["scroll"]["len"]);
    let y = i64::try_from(scroll_bar_middle(&state)).expect("the position fits");
    let bar_left = i64::try_from(number(&state["scroll_bar"]["x"])).expect("the position fits");
    assert!(thumb_top(&state) > scroll_bar_middle(&state), "{state:#}");

    // スクロールバーのすぐ左は余白で、スクロールしない。
    server.mouse(&left_click((bar_left - 1, y)));
    assert_eq!(scroll_offset(&server.state()), newest);
    // スクロールバーの左端はスクロールバーで、1 ページ戻る。
    server.mouse(&left_click((bar_left, y)));
    let paged = server.state();
    assert_eq!(scroll_offset(&paged), newest - lines, "{paged:#}");

    // つまみをドラッグすると上へ動く。
    let thumb = on_thumb(&paged);
    server.mouse(&left_drag(thumb, (thumb.0, thumb.1 - DRAG_PIXELS), ""));
    let dragged = server.state();
    assert!(
        scroll_offset(&dragged) < scroll_offset(&paged),
        "{dragged:#}"
    );
    assert_eq!(selection(&dragged), None, "{dragged:#}");
}

#[track_caller]
fn check_tab_bar_at_the_current_dpi(server: &Server) {
    let state = server.state();
    server.mouse(&left_click(area_center(&state["tab_bar"]["new_tab"])));
    let state = wait_tabs(server, 2);
    assert_eq!(state["active"], 1, "{state:#}");

    // タブの下端はタブで、そのひとつ下は端末。
    let area = &state["tab_bar"]["tabs"][0]["area"];
    let x = on_tab(&state, 0).0;
    let bottom =
        i64::try_from(number(&area["y"]) + number(&area["height"])).expect("the position fits");
    server.mouse(&left_click((x, bottom)));
    assert_eq!(server.state()["active"], 1);
    server.mouse(&left_click((x, bottom - 1)));
    assert_eq!(server.state()["active"], 0);

    let items = open_menu(server, &click_main_menu(&state));
    assert!(!items.is_empty());
    close_menu(server);

    server.mouse(&left_click(on_close_button(&state, 0)));
    let state = wait_tabs(server, 1);
    assert_eq!(state["active"], 0, "{state:#}");
}

/// 背景画像は、DPI を変えたあとの新しい範囲（スクロールバーの列も含む）いっぱいに広がり、タブバーには出ない。
#[test]
fn a_stretched_background_image_fills_its_new_area_after_a_dpi_change() {
    let server = start_with_background(
        &format!(
            "[background]\nimage_opacity = 1.0\nfit = \"stretch\"\n\n[[background.images]]\npath = '{BANDS_IMAGE}'\n"
        ),
        |_| {},
    );
    server.resize(DPI_TEST_GRID.0, DPI_TEST_GRID.1);

    for dpi in [DPI_150_PERCENT, DPI_200_PERCENT, DPI_100_PERCENT] {
        let state = change_dpi(&server, dpi);
        assert_eq!(state["background"]["image"], BANDS_IMAGE, "{state:#}");
        assert_layout_fits_the_client(&state);
        let area = BackdropArea::of(&state);
        let (x, _) = area.at(0.75, 0.0);
        wait_pixels(
            &server,
            &[
                ((x, area.y), BAND_TOP),
                (area.at(0.25, 0.5), MIDDLE_LEFT),
                (area.at(0.75, 0.5), MIDDLE_RIGHT),
                ((x, area.y + area.height - 1), BAND_BOTTOM),
            ],
        );
        wait_pixels(&server, &[(scroll_bar_edge(&state), MIDDLE_RIGHT)]);
        let picture = server.picture();
        assert!(
            !color_is_close(picture.rgb(x, area.y - 1), BAND_TOP),
            "at {dpi} DPI"
        );
    }
}

/// スクロールバーの色を固定する設定（`[colors]` の後ろに置く）と、その色。
const SCROLL_BAR_COLORS_SETTING: &str = "[colors.scroll_bar]\ntrack = \"#f0f0f0\"\nthumb = \"#c0c000\"\nhovered_thumb = \"#00c0c0\"\ndragged_thumb = \"#c000c0\"\n\n";
const SCROLL_BAR_TRACK: [u8; 3] = [240, 240, 240];
const SCROLL_BAR_THUMB: [u8; 3] = [192, 192, 0];
const SCROLL_BAR_HOVERED_THUMB: [u8; 3] = [0, 192, 192];
const SCROLL_BAR_DRAGGED_THUMB: [u8; 3] = [192, 0, 192];
/// 背景画像があるときのトラックの不透明度（`scroll_bar.rs` の `TRACK_OPACITY_OVER_IMAGE`）。
const TRACK_OPACITY_OVER_IMAGE: f64 = 0.5;
/// スクロールバーの下に敷く単色の画像の名前と色。
const SOLID_IMAGE: &str = "solid.png";
const SOLID_IMAGE_COLOR: [u8; 3] = [200, 120, 0];

/// 画素の位置にする。
fn pixel_at((x, y): Point) -> (u32, u32) {
    (
        u32::try_from(x).expect("the position fits"),
        u32::try_from(y).expect("the position fits"),
    )
}

/// スクロールバーの中で、つまみから離れた高さ。つまみの上と下のうち、広いほうの真ん中。
fn away_from_the_thumb(state: &Value) -> u64 {
    let bar_top = number(&state["scroll_bar"]["y"]);
    let bar_bottom = bar_top + number(&state["scroll_bar"]["height"]);
    let (top, bottom) = (thumb_top(state), thumb_top(state) + thumb_length(state));
    if top - bar_top >= bar_bottom - bottom {
        bar_top + (top - bar_top) / 2
    } else {
        bottom + (bar_bottom - bottom) / 2
    }
}

/// スクロールバックのあるスクロールバーの見え方を確認する。`under` は何も塗らないときにスクロールバーの列に見える色、
/// `track` はつまみを押している間のトラックに見える色。マウスを載せていないつまみ、ドラッグしているつまみ、
/// つまみの外を押し続けているときのつまみ（マウスを載せているときの色）を見て、離す。
#[track_caller]
fn assert_scroll_bar_colors(server: &Server, under: [u8; 3], track: [u8; 3]) {
    let state = server.state();
    let away = on_scroll_bar(&state, away_from_the_thumb(&state));
    // スクロールバーの列の右端。太いつまみも列の真ん中に描くので、右端には掛からない。
    let right = u32::try_from(number(&state["client_width"]) - 1).expect("the position fits");
    let edge = |y: u64| (right, u32::try_from(y).expect("the position fits"));
    let thumb_middle = |state: &Value| thumb_top(state) + thumb_length(state) / 2;
    wait_pixels(
        server,
        &[
            (pixel_at(on_thumb(&state)), SCROLL_BAR_THUMB),
            (pixel_at(away), under),
            (edge(thumb_middle(&state)), under),
        ],
    );

    // つまみを押してドラッグを始める（動かさない）。トラックを塗り、つまみは太く不透明に描く。
    let grabbed = on_thumb(&state);
    server.mouse(&format!(
        "{}{}",
        step("move", grabbed, ""),
        step("left-down", grabbed, "")
    ));
    let dragged = server.state();
    assert_eq!(dragged["scroll_bar"]["pressed"], true, "{dragged:#}");
    wait_pixels(
        server,
        &[
            (pixel_at(grabbed), SCROLL_BAR_DRAGGED_THUMB),
            (pixel_at(away), track),
            (edge(thumb_middle(&dragged)), track),
        ],
    );
    server.mouse(&step("left-up", grabbed, ""));
    assert_eq!(server.state()["scroll_bar"]["pressed"], false);

    // つまみの外を押し続けると、つまみがマウスの位置まで来て止まる。押している間はマウスを載せているときの形。
    server.mouse(&format!(
        "{}{}",
        step("move", away, ""),
        step("left-down", away, "")
    ));
    let reached = server.wait_until("the thumb to reach the pointer", |state| {
        let y = u64::try_from(away.1).expect("fits");
        !state["scroll_bar"]["thumb"].is_null()
            && thumb_top(state) <= y
            && y < thumb_top(state) + thumb_length(state)
    });
    assert_eq!(reached["scroll_bar"]["pressed"], true, "{reached:#}");
    wait_pixels(
        server,
        &[
            (
                on_scroll_bar_pixel(&reached, thumb_middle(&reached)),
                SCROLL_BAR_HOVERED_THUMB,
            ),
            (edge(thumb_middle(&reached)), track),
        ],
    );
    server.mouse(&step("left-up", away, ""));
    assert_eq!(server.state()["scroll_bar"]["pressed"], false);
}

/// スクロールバーの横の真ん中の、高さ `y` の画素の位置。
fn on_scroll_bar_pixel(state: &Value, y: u64) -> (u32, u32) {
    pixel_at(on_scroll_bar(state, y))
}

/// スクロールバックのない（つまみのない）スクロールバーの列に `under` が見えることを確認する。
fn check_the_bar_column_without_a_thumb(server: &Server, under: [u8; 3]) {
    let state = server.state();
    assert!(state["scroll_bar"]["thumb"].is_null(), "{state:#}");
    wait_pixels(
        server,
        &[
            (
                on_scroll_bar_pixel(&state, scroll_bar_middle(&state)),
                under,
            ),
            (scroll_bar_edge(&state), under),
        ],
    );
}

/// 背景画像がなければ、スクロールバーの列は端末の背景色で、トラックは設定の色のまま不透明に塗る。設定ファイルに背景画像を
/// 足すと、背景画像はスクロールバーの列にも敷き、トラックは半透明に塗って画像を透かす（つまみは不透明）。画像があるときは
/// DPI を変えたあとも同じ。
#[test]
fn the_scroll_bar_track_follows_the_background_image() {
    const IMAGE_OPACITY: f64 = 0.5;
    let server = start_with_background(SCROLL_BAR_COLORS_SETTING, |dir| {
        std::fs::write(dir.join(SOLID_IMAGE), solid_image(SOLID_IMAGE_COLOR))
            .expect("failed to write the image");
    });
    server.resize(DPI_TEST_GRID.0, DPI_TEST_GRID.1);
    check_the_bar_column_without_a_thumb(&server, TERMINAL_BACKGROUND);
    fill_scrollback(&server);
    assert_scroll_bar_colors(&server, TERMINAL_BACKGROUND, SCROLL_BAR_TRACK);

    std::fs::write(
        server.dir().join("config.toml"),
        format!(
            "{TERMINAL_BACKGROUND_SETTING}{SCROLL_BAR_COLORS_SETTING}[background]\nimage_opacity = {IMAGE_OPACITY:?}\nfit = \"stretch\"\n\n[[background.images]]\npath = '{SOLID_IMAGE}'\n"
        ),
    )
    .expect("failed to write the settings");
    let under = over_terminal_background(SOLID_IMAGE_COLOR, IMAGE_OPACITY);
    let track = blend(SCROLL_BAR_TRACK, under, TRACK_OPACITY_OVER_IMAGE);
    assert_scroll_bar_colors(&server, under, track);
    for dpi in [DPI_150_PERCENT, DPI_200_PERCENT, DPI_100_PERCENT] {
        change_dpi(&server, dpi);
        assert_scroll_bar_colors(&server, under, track);
    }
}

/// 元の大きさの背景画像は、DPI を変えても画素の数が変わらず、新しい範囲の真ん中に出る。
#[test]
fn a_centered_background_image_keeps_its_pixels_after_a_dpi_change() {
    let server = start_with_background(
        &format!(
            "[background]\nimage_opacity = 1.0\nfit = \"center\"\n\n[[background.images]]\npath = '{BANDS_IMAGE}'\n"
        ),
        |_| {},
    );
    server.resize(DPI_TEST_GRID.0, DPI_TEST_GRID.1);

    let state = change_dpi(&server, DPI_200_PERCENT);
    let (x, y) = BackdropArea::of(&state).at(0.5, 0.5);
    let side = BANDS_SIDE;
    wait_pixels(
        &server,
        &[
            ((x - side / 4, y), MIDDLE_LEFT),
            ((x + side / 4, y), MIDDLE_RIGHT),
            ((x - side / 4, y - side / 2 + BAND_HEIGHT / 2), BAND_TOP),
            ((x - side / 4, y + side / 2 - BAND_HEIGHT / 2), BAND_BOTTOM),
            ((x - side * 3 / 4, y), TERMINAL_BACKGROUND),
            ((x + side * 3 / 4, y), TERMINAL_BACKGROUND),
        ],
    );
}

/// タブのアイコンの画像のファイル。真っ赤な正方形。
const RED_ICON: &str = "red-icon.png";
const RED_ICON_SIDE: u32 = 64;
const RED: [u8; 3] = [0xff, 0x00, 0x00];
/// 96 DPI と 192 DPI のタブのアイコンの一辺。
const TAB_ICON_AT_100_PERCENT: u64 = 18;
const TAB_ICON_AT_200_PERCENT: u64 = 36;

/// タブごとの、縦の真ん中の行の [`RED`] の画素の数が `expected` になるまで絵を読み直す。
#[track_caller]
fn wait_red_in_tabs(server: &Server, expected: &[u64]) {
    let deadline = Instant::now() + PICTURE_TIMEOUT;
    loop {
        let state = server.state();
        let picture = server.picture();
        let counts: Vec<u64> = (0..expected.len())
            .map(|index| {
                let area = &state["tab_bar"]["tabs"][index]["area"];
                let x = u32::try_from(number(&area["x"])).expect("the position fits");
                let width = u32::try_from(number(&area["width"])).expect("the width fits");
                let y = u32::try_from(number(&area["y"]) + number(&area["height"]) / 2)
                    .expect("the position fits");
                (x..x + width)
                    .filter(|&x| color_is_close(picture.rgb(x, y), RED))
                    .count() as u64
            })
            .collect();
        if counts == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "red pixels in the tabs: {counts:?}, not {expected:?}\n{state:#}\n{}",
            server.log()
        );
        std::thread::sleep(PICTURE_POLL_INTERVAL);
    }
}

/// fantermo のログの、[`RED_ICON`] を一辺 `side` で読んだ行の数。
fn red_icon_loads(server: &Server, side: u64) -> usize {
    let read = format!("{RED_ICON} at {side} px");
    server
        .log()
        .lines()
        .filter(|line| line.contains("loaded the tab icon") && line.contains(&read))
        .count()
}

/// プロファイルのアイコンの画像はタブに出て、DPI を変えるとその DPI の一辺で出る。読めない画像のプロファイルの
/// タブには、自動で作ったアイコンが出る。
///
/// 画像のファイルを読むのは、そのアイコンのタブを初めて開いたときと DPI が変わったときだけで、同じアイコンのタブを
/// 開き足したときと、フォントの設定を変えて描き直したときは読まない。
#[test]
fn tab_icons_show_the_profile_image_and_follow_the_dpi() {
    let profiles = format!(
        r#"[[profiles]]
name = "image"
command = "cmd.exe /k prompt image$G"
icon = '{RED_ICON}'

[[profiles]]
name = "missing"
command = "cmd.exe /k prompt missing$G"
icon = 'missing.png'
"#
    );
    let server = Server::start_with_setup(&profiles, &[], |dir| {
        let pixels = RED.repeat((RED_ICON_SIDE * RED_ICON_SIDE) as usize);
        std::fs::write(
            dir.join(RED_ICON),
            common::encode_png(RED_ICON_SIDE, RED_ICON_SIDE, &pixels),
        )
        .expect("failed to write the icon");
    });
    server.wait_text("image>");
    server.post("/e2e/tab/new/1", b"");
    server.wait_text("missing>");
    server.resize(DPI_TEST_GRID.0, DPI_TEST_GRID.1);
    wait_red_in_tabs(&server, &[TAB_ICON_AT_100_PERCENT, 0]);
    assert_eq!(red_icon_loads(&server, TAB_ICON_AT_100_PERCENT), 1);

    // 同じアイコンのタブを開き足しても読まない。
    server.post("/e2e/tab/new/0", b"");
    server.wait_until("three tabs", |state| tab_count(state) == 3);
    wait_red_in_tabs(
        &server,
        &[TAB_ICON_AT_100_PERCENT, 0, TAB_ICON_AT_100_PERCENT],
    );
    assert_eq!(red_icon_loads(&server, TAB_ICON_AT_100_PERCENT), 1);

    // フォントを変えて描き直しても読まない。描き直したことは、新しいセルの高さの状態のあとに絵を読んで見る。
    let cell_height = server.state()["cell_height"].clone();
    write_settings(&server, &format!("{profiles}\n[font]\nsize = 16\n"));
    server.wait_until("the new font", |state| state["cell_height"] != cell_height);
    wait_red_in_tabs(
        &server,
        &[TAB_ICON_AT_100_PERCENT, 0, TAB_ICON_AT_100_PERCENT],
    );
    assert_eq!(red_icon_loads(&server, TAB_ICON_AT_100_PERCENT), 1);

    // DPI が変わったら、その一辺で読み直す。
    change_dpi(&server, DPI_200_PERCENT);
    wait_red_in_tabs(
        &server,
        &[TAB_ICON_AT_200_PERCENT, 0, TAB_ICON_AT_200_PERCENT],
    );
    assert_eq!(red_icon_loads(&server, TAB_ICON_AT_200_PERCENT), 1);
    change_dpi(&server, DPI_100_PERCENT);
    wait_red_in_tabs(
        &server,
        &[TAB_ICON_AT_100_PERCENT, 0, TAB_ICON_AT_100_PERCENT],
    );
    assert_eq!(red_icon_loads(&server, TAB_ICON_AT_100_PERCENT), 2);
}

/// DPI を変えたあとに書く `state.toml` は、そのときのウィンドウの大きさ（ピクセル）で、DPI は書かない。同じ DPI で起動
/// すると同じ寸法に戻り、違う DPI で起動してもピクセルのまま開いて、桁数と行数はその DPI のセルで入る数になる。
#[test]
fn the_size_saved_after_a_dpi_change_opens_in_pixels() {
    let mut first = Server::start("", &CMD_COMMAND);
    first.wait_text(CMD_PROMPT);
    first.resize(DPI_TEST_GRID.0, DPI_TEST_GRID.1);
    let changed = change_dpi(&first, DPI_200_PERCENT);
    quit(&mut first);
    assert_eq!(saved_window(&first), current_window(&changed));
    let saved = std::fs::read(first.state_file()).expect("failed to read the state");
    assert!(
        !String::from_utf8_lossy(&saved).contains("dpi"),
        "{}",
        String::from_utf8_lossy(&saved)
    );
    drop(first);

    let (mut same, state) = start_at_dpi(DPI_200_PERCENT, &saved);
    assert_eq!(dpi_layout(&state), dpi_layout(&changed));
    quit(&mut same);
    assert_eq!(saved_window(&same), current_window(&changed));
    drop(same);

    let (mut other, state) = start_at_dpi(DPI_100_PERCENT, &saved);
    assert_eq!(current_window(&state), current_window(&changed));
    assert_eq!(state["client_width"], changed["client_width"]);
    assert_eq!(state["client_height"], changed["client_height"]);
    assert!(
        number(&state["cols"]) > number(&changed["cols"])
            && number(&state["rows"]) > number(&changed["rows"]),
        "{state:#}"
    );
    assert_layout_fits_the_client(&state);
    quit(&mut other);
    assert_eq!(saved_window(&other), current_window(&changed));
}

/// `FANTERMO_DPI` を使わず、モニターの DPI で起動する。`[window]` の `dpi` を書いた `state.toml`（`dpi` を書いていた
/// バージョンのもの）も読め、`dpi` は無視してピクセルのまま開く。`dpi` の値は読まれないので、書く値は固定でよい
/// （モニターの DPI と同じ手元でも成り立つ）。
#[test]
fn a_size_saved_at_another_monitor_dpi_opens_in_pixels() {
    let saved = format!(
        "{}dpi = {DPI_200_PERCENT}\n",
        state::to_toml(&seeded_state(UNEVEN_WINDOW))
    );

    // `FANTERMO_DPI` を読めない値にすると、モニターの DPI を使う。
    let server = Server::start_with_environment("", &CMD_COMMAND, |dir| {
        std::fs::write(dir.join(common::STATE_FILE), &saved).expect("failed to write the state");
        vec![(DPI_ENV, OsString::new())]
    });
    let state = server.wait_text(CMD_PROMPT);
    assert_eq!(state["dpi"], state["monitor_dpi"], "{state:#}");
    assert_eq!(current_window(&state), UNEVEN_WINDOW, "{state:#}");
    assert_layout_fits_the_client(&state);
}

/// 設定ダイアログは、メッセージの DPI ではなくモニターの DPI で開く。開いている間に端末の DPI が変わっても、ダイアログの
/// 大きさは変わらず、端末は動き続ける。
#[test]
fn the_settings_dialog_keeps_the_monitor_dpi_after_a_dpi_change() {
    let server = start_settings_test(ONE_PROFILE);
    server.resize(DPI_TEST_GRID.0, DPI_TEST_GRID.1);
    let state = change_dpi(&server, DPI_150_PERCENT);

    let dialog = open_settings(&server);
    assert_eq!(dialog["dpi"], state["monitor_dpi"], "{dialog:#}");
    assert_controls_fit(&dialog, PAGE_APPEARANCE);

    let changed = change_dpi(&server, DPI_200_PERCENT);
    assert_layout_fits_the_client(&changed);
    let after = read_controls(&server);
    assert_eq!(after["dpi"], dialog["dpi"], "{after:#}");
    assert_eq!(after["width"], dialog["width"], "{after:#}");
    assert_eq!(after["height"], dialog["height"], "{after:#}");

    press_cancel(&server);
    let state = server.state();
    assert_eq!(state["dpi"], DPI_200_PERCENT, "{state:#}");
    assert_layout_fits_the_client(&state);
    check_terminal_still_works(&server, "after-dialog-and-dpi");
}

/// DPI を変えたあとに設定を読み直しても、変えたあとの DPI で測り直す。ウィンドウの位置と大きさは変わらない。
#[test]
fn a_settings_change_after_a_dpi_change_uses_the_changed_dpi() {
    const PADDING: u64 = 8;
    let server = Server::start("", &CMD_COMMAND);
    server.wait_text(CMD_PROMPT);
    server.resize(DPI_TEST_GRID.0, DPI_TEST_GRID.1);
    let state = change_dpi(&server, DPI_200_PERCENT);

    write_settings(&server, &format!("[window]\npadding = {PADDING}\n"));
    let padding = PADDING * DPI_200_PERCENT / DPI_100_PERCENT;
    let read = server.wait_until("the new padding", |state| {
        number(&state["terminal_x"]) == padding
    });
    assert_eq!(read["dpi"], DPI_200_PERCENT, "{read:#}");
    assert_eq!(read["cell_width"], state["cell_width"], "{read:#}");
    assert_eq!(current_window(&read), current_window(&state), "{read:#}");
    assert_eq!(read["client_width"], state["client_width"], "{read:#}");
    assert_layout_fits_the_client(&read);
}

/// 設定ファイルを `text` に書き換える。fantermo は書き換えを見張って読み直す。
fn write_settings(server: &Server, text: &str) {
    std::fs::write(server.dir().join("config.toml"), text).expect("failed to write the settings");
}

/// 設定ファイルでフォントの大きさ、スクロールバーの幅、余白を変えても、ウィンドウの位置と大きさは 1 ピクセルも変わらない。
/// 桁数と行数がいまのクライアント領域に合わせて変わり、シェルにも届く。
#[test]
fn a_settings_change_keeps_the_window_rect_and_changes_the_grid() {
    let server = Server::start_powershell();
    let before = server.wait_text("PS ");
    let window = current_window(&before);
    type Applied = fn(&Value) -> bool;
    let changes: [(&str, &str, Applied); 3] = [
        ("font", "[font]\nsize = 16\n", |state| {
            state["font_size_px"]
                .as_f64()
                .is_some_and(|size| size > 20.0)
        }),
        (
            "scroll-bar",
            "[font]\nsize = 16\n[scroll_bar]\nwidth = 40\n",
            |state| state["scroll_bar"]["width"] == 40,
        ),
        (
            "padding",
            "[font]\nsize = 16\n[scroll_bar]\nwidth = 40\n[window]\npadding = 40\n",
            |state| state["terminal_x"] == 40,
        ),
    ];
    let mut previous = before;
    for (name, settings, changed) in changes {
        write_settings(&server, settings);
        let state = server.wait_until(name, changed);
        assert_eq!(current_window(&state), window, "{name}: {state:#}");
        assert_eq!(state["client_width"], previous["client_width"], "{name}");
        assert_eq!(state["client_height"], previous["client_height"], "{name}");
        assert_layout_fits_the_client(&state);
        assert!(
            number(&state["cols"]) < number(&previous["cols"]),
            "{name}: {state:#}"
        );
        if name != "scroll-bar" {
            assert!(
                number(&state["rows"]) < number(&previous["rows"]),
                "{name}: {state:#}"
            );
        }
        assert_the_shell_sees_the_grid(&server, &state, name);
        previous = state;
    }
}

/// セルで割り切れない大きさのウィンドウでも、フォントを変えて戻したあとに位置と大きさが変わらず、桁数と行数は元に戻る。
#[test]
fn a_settings_change_keeps_an_uneven_window_rect() {
    let (server, before) = start_uneven("", &CMD_COMMAND, CMD_PROMPT);
    write_settings(&server, "[font]\nsize = 16\n");
    let larger = server.wait_until("larger cells", |state| {
        number(&state["cell_height"]) > number(&before["cell_height"])
    });
    assert_eq!(current_window(&larger), UNEVEN_WINDOW, "{larger:#}");
    assert_layout_fits_the_client(&larger);

    write_settings(&server, "");
    let back = server.wait_until("the cells back", |state| {
        state["cell_height"] == before["cell_height"]
    });
    assert_eq!(current_window(&back), UNEVEN_WINDOW, "{back:#}");
    assert_eq!(dpi_layout(&back), dpi_layout(&before));
}

/// 最大化しているときに余白を変えても、最大化したままで、クライアント領域の大きさも変わらない。
#[test]
fn a_settings_change_keeps_a_maximized_window_maximized() {
    let window = WindowState {
        maximized: true,
        ..UNEVEN_WINDOW
    };
    let (server, before) = start_with_state(state::to_toml(&seeded_state(window)).as_bytes());
    assert_eq!(current_window(&before), window, "{before:#}");
    write_settings(&server, "[window]\npadding = 20\n");
    let state = server.wait_until("the new padding", |state| state["terminal_x"] == 20);
    assert_eq!(current_window(&state), window, "{state:#}");
    assert_eq!(state["client_width"], before["client_width"], "{state:#}");
    assert_eq!(state["client_height"], before["client_height"], "{state:#}");
    assert_layout_fits_the_client(&state);
}
