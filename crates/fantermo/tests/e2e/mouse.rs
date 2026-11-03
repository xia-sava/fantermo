use crate::common::CMD_PROMPT;
use crate::common::Server;
use crate::ui::{
    DRAG_PIXELS, EMPTY_COL, EMPTY_ROWS_BELOW, Point, area_center, button_click, cell_center,
    cell_left, cell_right, close_menu, cygwin_config, echo_lines, fill_scrollback, find_cell,
    left_click, left_drag, left_press_and_move, newest_offset, number, on_close_button,
    on_scroll_bar, on_tab, on_thumb, scroll_bar_middle, scroll_offset, selection, step, tab_count,
    thumb_length, thumb_top, wait_menu, wait_sh_ready, wait_tabs, wsl_config,
};
use serde_json::Value;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// ホイールの 1 ノッチの回転量。
const NOTCH: i32 = 120;
/// 表示している範囲のどこからでもいちばん古い行に届く、ノッチの数。
const NOTCHES_TO_THE_OLDEST: i32 = 200;

/// ホイールを回すセルの桁と行。
const WHEEL_COL: u32 = 5;
const WHEEL_ROW: u32 = 5;

/// バーの外へ出すときの、横のピクセル数。
const BESIDE_THE_BAR: i64 = 200;
/// ウィンドウの上の外の高さ。
const ABOVE_THE_WINDOW: i64 = -100;
/// ウィンドウの下の外の高さ。
const BELOW_THE_WINDOW: i64 = 10_000;

/// つまみがマウスの位置で止まったあと、繰り返しの間隔の数回分として見る長さ。
const REPEAT_STOPPED_CHECK: Duration = Duration::from_millis(300);

/// `at` でホイールを `delta` 回す。
fn wheel(delta: i32, at: Point) -> String {
    step(&format!("wheel:{delta}"), at, "")
}

#[test]
fn dragging_selects_the_text_and_the_copy_key_copies_it() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.post("/e2e/clipboard", b"before");
    let state = echo_lines(&server, "alpha beta", "other");
    let (col, row) = find_cell(&state, "alpha beta-0-end");

    server.mouse(&left_drag(
        cell_left(&state, col + 6, row),
        cell_right(&state, col + 9, row),
        "",
    ));
    let state = server.state();
    assert_eq!(selection(&state), Some("beta"), "{state:#}");
    // 選んだだけではコピーしない。
    server.wait_clipboard("before");

    server.press("ctrl+c");
    server.wait_clipboard("beta");
    assert_eq!(selection(&server.state()), None);

    // 右から左へのドラッグも同じ範囲を選び、Ctrl+Insert でもコピーできる。
    server.mouse(&left_drag(
        cell_right(&state, col + 3, row),
        cell_left(&state, col, row),
        "",
    ));
    assert_eq!(selection(&server.state()), Some("alph"));
    server.press("ctrl+insert");
    server.wait_clipboard("alph");

    // 選んでいなければ Ctrl+C はシェルへ送り、クリップボードは変えない。
    server.press("ctrl+c");
    server.type_line("echo alive-%ERRORLEVEL%-end");
    server.wait_text("alive-0-end");
    server.wait_clipboard("alph");
}

#[test]
fn a_drag_over_lines_selects_up_to_the_end_of_the_first_line() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = echo_lines(&server, "upper", "lower");
    let (col, row) = find_cell(&state, "upper-0-end");
    server.mouse(&left_drag(
        cell_left(&state, col + 2, row),
        cell_right(&state, col + 1, row + 1),
        "",
    ));
    assert_eq!(selection(&server.state()), Some("per-0-end\nlo"));
}

#[test]
fn a_drag_from_the_padding_and_out_of_the_window_uses_the_nearest_cell() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = echo_lines(&server, "alpha", "other");
    let (col, row) = find_cell(&state, "alpha-0-end");
    assert_eq!(col, 0, "{state:#}");
    let (_, y) = cell_center(&state, 0, row);

    let padding = i64::try_from(number(&state["terminal_x"])).expect("the position fits") - 1;
    server.mouse(&left_drag((padding, y), cell_right(&state, 4, row), ""));
    assert_eq!(selection(&server.state()), Some("alpha"));

    server.mouse(&left_drag(cell_right(&state, 4, row), (-20, y), ""));
    assert_eq!(selection(&server.state()), Some("alpha"));
}

#[test]
fn double_click_selects_a_word_and_triple_click_a_line() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = echo_lines(&server, "alpha beta", "gamma delta");

    let (col, row) = find_cell(&state, "alpha beta-0-end");
    let at = cell_center(&state, col + 1, row);
    server.mouse(&format!("{}{}", left_click(at), left_click(at)));
    assert_eq!(selection(&server.state()), Some("alpha"));

    // 別の行なので、前のクリックの続きにならない。
    let (col, row) = find_cell(&state, "gamma delta-0-end");
    let at = cell_center(&state, col + 1, row);
    server.mouse(&format!("{0}{0}{0}", left_click(at)));
    assert_eq!(selection(&server.state()), Some("gamma delta-0-end"));
    server.press("ctrl+c");
    server.wait_clipboard("gamma delta-0-end");
}

#[test]
fn alt_drag_selects_a_rectangle() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = echo_lines(&server, "abcdef", "ghijkl");
    let (col, row) = find_cell(&state, "abcdef-0-end");
    let from = cell_left(&state, col + 1, row);
    let to = cell_right(&state, col + 2, row + 1);

    server.mouse(&left_drag(from, to, "alt"));
    assert_eq!(selection(&server.state()), Some("bc\nhi"));

    // 同じ位置をすぐ押すとダブルクリックになるので、間に別の位置を押す。
    server.mouse(&left_click(cell_center(
        &state,
        EMPTY_COL,
        row + EMPTY_ROWS_BELOW,
    )));
    server.mouse(&left_drag(from, to, ""));
    assert_eq!(selection(&server.state()), Some("bcdef-0-end\nghi"));
}

#[test]
fn a_click_and_input_clear_the_selection() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = echo_lines(&server, "alpha", "other");
    let (col, row) = find_cell(&state, "alpha-0-end");
    let select = left_drag(
        cell_left(&state, col, row),
        cell_right(&state, col + 4, row),
        "",
    );

    server.mouse(&select);
    assert_eq!(selection(&server.state()), Some("alpha"));
    server.mouse(&left_click(cell_center(
        &state,
        EMPTY_COL,
        row + EMPTY_ROWS_BELOW,
    )));
    assert_eq!(selection(&server.state()), None);

    server.mouse(&select);
    assert_eq!(selection(&server.state()), Some("alpha"));
    server.type_text("x");
    assert_eq!(selection(&server.state()), None);
}

/// ほかのウィンドウがマウスをキャプチャしたあとは、離したことが届かない。そのあとの動きで選択を広げない。
#[test]
fn a_selection_stops_following_the_pointer_when_the_capture_is_lost() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = echo_lines(&server, "alpha", "other");
    let (col, row) = find_cell(&state, "alpha-0-end");
    server.mouse(&left_press_and_move(
        cell_left(&state, col, row),
        cell_right(&state, col + 1, row),
        "",
    ));
    assert_eq!(selection(&server.state()), Some("al"));

    server.send_message("capturechanged", 0);
    server.mouse(&step("move", cell_right(&state, col + 4, row), ""));
    assert_eq!(selection(&server.state()), Some("al"));
}

#[test]
fn the_right_button_opens_the_menu_and_the_middle_button_does_not_paste() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.post("/e2e/clipboard", b"must-not-be-pasted");
    let state = echo_lines(&server, "alpha", "other");
    let (col, row) = find_cell(&state, "alpha-0-end");
    let at = cell_center(&state, col, row);

    server.mouse(&button_click("middle", at, ""));
    server.type_line("echo after-%ERRORLEVEL%-middle");
    let after = server.wait_text("after-0-middle");
    let text = after["text"].as_str().unwrap_or_default();
    assert!(!text.contains("must-not-be-pasted"), "{text}");
    assert_eq!(selection(&after), None);

    // 押しただけではメニューを出さず、離したときに出す。選択範囲は残す。
    server.mouse(&left_drag(
        cell_left(&state, col, row),
        cell_right(&state, col + 4, row),
        "",
    ));
    server.mouse(&step("right-down", at, ""));
    let pressed = server.state();
    assert_eq!(
        pressed["dialogs"].as_array().map(Vec::len),
        Some(0),
        "{pressed:#}"
    );
    server.mouse(&step("right-up", at, ""));
    wait_menu(&server);
    assert_eq!(selection(&server.state()), Some("alpha"));
    close_menu(&server);

    server.type_line("echo after-%ERRORLEVEL%-menu");
    let after = server.wait_text("after-0-menu");
    let text = after["text"].as_str().unwrap_or_default();
    assert!(!text.contains("must-not-be-pasted"), "{text}");
}

#[test]
fn the_wheel_scrolls_the_scrollback() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = fill_scrollback(&server);
    let newest = newest_offset(&state);
    let lines = number(&state["scroll"]["lines_per_notch"]);
    let at = cell_center(&state, WHEEL_COL, WHEEL_ROW);
    let top_of_thumb = thumb_top(&state);

    server.mouse(&wheel(NOTCH, at));
    let scrolled = server.state();
    assert_eq!(scroll_offset(&scrolled), newest - lines);
    assert!(thumb_top(&scrolled) < top_of_thumb, "{scrolled:#}");

    server.mouse(&wheel(-NOTCH, at));
    assert_eq!(scroll_offset(&server.state()), newest);

    // 半分のノッチは溜めて、合わせて 1 ノッチになる。
    server.mouse(&format!("{0}{0}", wheel(NOTCH / 2, at)));
    assert_eq!(scroll_offset(&server.state()), newest - lines);

    // タブバーの上では動かない。
    server.mouse(&wheel(NOTCH, on_tab(&state, 0)));
    assert_eq!(scroll_offset(&server.state()), newest - lines);

    // いちばん古い行で止まる。
    server.mouse(&wheel(NOTCH * NOTCHES_TO_THE_OLDEST, at));
    assert_eq!(scroll_offset(&server.state()), 0);

    // 打つと、いちばん新しい行に戻る。
    server.type_text("x");
    server.wait_until("the newest lines", |state| {
        scroll_offset(state) == newest_offset(state)
    });
}

#[test]
fn dragging_the_thumb_moves_the_view_and_keeps_the_grabbed_position() {
    /// つまみの動きがマウスの動きからずれてよいピクセル数。行の単位に丸める分。
    const THUMB_TOLERANCE: u64 = 3;

    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = fill_scrollback(&server);
    let newest = newest_offset(&state);
    let (x, y) = on_thumb(&state);
    let top_of_thumb = thumb_top(&state);
    let idle_color = server.picture().rgb(
        u32::try_from(x).expect("fits"),
        u32::try_from(y).expect("fits"),
    );

    server.mouse(&left_press_and_move((x, y), (x, y - DRAG_PIXELS), ""));
    let dragged = server.state();
    assert_eq!(dragged["scroll_bar"]["pressed"], true, "{dragged:#}");
    assert!(scroll_offset(&dragged) < newest, "{dragged:#}");
    assert!(selection(&dragged).is_none(), "{dragged:#}");
    assert!(
        thumb_top(&dragged).abs_diff(top_of_thumb - DRAG_PIXELS.unsigned_abs()) <= THUMB_TOLERANCE,
        "{dragged:#}"
    );
    // ドラッグしている間は色を変える。
    let dragged_color = server.picture().rgb(
        u32::try_from(x).expect("fits"),
        u32::try_from(thumb_top(&dragged) + thumb_length(&dragged) / 2).expect("fits"),
    );
    assert_ne!(idle_color, dragged_color);

    // バーの外へ出ても高さだけを見て、トラックの端で止まる。
    server.mouse(&step(
        "move",
        (x - BESIDE_THE_BAR, ABOVE_THE_WINDOW),
        "left",
    ));
    let at_top = server.state();
    assert_eq!(scroll_offset(&at_top), 0, "{at_top:#}");
    server.mouse(&step("move", (x, BELOW_THE_WINDOW), "left"));
    assert_eq!(scroll_offset(&server.state()), newest);

    // 離すと、動かしてもついてこない。
    server.mouse(&format!(
        "{}{}{}",
        step("move", (x, y - DRAG_PIXELS), "left"),
        step("left-up", (x, y - DRAG_PIXELS), ""),
        step("move", (x, y), "")
    ));
    let released = server.state();
    assert_eq!(released["scroll_bar"]["pressed"], false, "{released:#}");
    assert!(scroll_offset(&released) < newest, "{released:#}");
}

#[test]
fn clicking_the_scroll_bar_track_moves_a_page() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = fill_scrollback(&server);
    let newest = newest_offset(&state);
    let page = number(&state["scroll"]["len"]);

    let above = on_scroll_bar(&state, thumb_top(&state) - 1);
    server.mouse(&button_click("left", above, ""));
    let moved = server.state();
    assert_eq!(scroll_offset(&moved), newest - page, "{moved:#}");
    assert_eq!(moved["scroll_bar"]["pressed"], false, "{moved:#}");

    let below = on_scroll_bar(&moved, thumb_top(&moved) + thumb_length(&moved) + 1);
    server.mouse(&button_click("left", below, ""));
    assert_eq!(scroll_offset(&server.state()), newest);
}

/// つまみの外を押し続けると、つまみがマウスの位置に来るまで送り、そこで止まる。
#[test]
fn holding_the_scroll_bar_track_repeats_until_the_thumb_reaches_the_pointer() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = fill_scrollback(&server);
    let target = scroll_bar_middle(&state);
    assert!(target < thumb_top(&state), "{state:#}");

    let at = on_scroll_bar(&state, target);
    server.mouse(&format!(
        "{}{}",
        step("move", at, ""),
        step("left-down", at, "")
    ));
    let reached = server.wait_until("the thumb under the pointer", |state| {
        thumb_top(state) <= target
    });
    assert!(
        target < thumb_top(&reached) + thumb_length(&reached),
        "{reached:#}"
    );
    // 押したままでも、つまみはマウスの位置を通り越さない。
    let stopped_at = scroll_offset(&reached);
    server.assert_holds_for(REPEAT_STOPPED_CHECK, "the stopped view", |state| {
        scroll_offset(state) == stopped_at
    });
    server.mouse(&step("left-up", at, ""));
    assert_eq!(server.state()["scroll_bar"]["pressed"], false);
}

#[test]
fn the_scroll_bar_press_ends_when_the_capture_is_lost_or_the_tab_changes() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = fill_scrollback(&server);
    let (x, y) = on_thumb(&state);

    server.mouse(&left_press_and_move((x, y), (x, y - DRAG_PIXELS), ""));
    let dragged = server.state();
    assert_eq!(dragged["scroll_bar"]["pressed"], true, "{dragged:#}");
    server.send_message("capturechanged", 0);
    assert_eq!(server.state()["scroll_bar"]["pressed"], false);
    // 動かしても、もうついてこない。
    server.mouse(&step("move", (x, 0), ""));
    assert_eq!(scroll_offset(&server.state()), scroll_offset(&dragged));
    server.mouse(&step("left-up", (x, 0), ""));

    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    server.wait_text(CMD_PROMPT);
    server.post("/e2e/tab/select/0", b"");
    let state = server.wait_text("filled-0-end");
    let (x, y) = on_thumb(&state);
    server.mouse(&left_press_and_move((x, y), (x, y - DRAG_PIXELS), ""));
    assert_eq!(server.state()["scroll_bar"]["pressed"], true);
    server.post("/e2e/tab/select/1", b"");
    let switched = server.state();
    assert_eq!(switched["active"], 1, "{switched:#}");
    assert_eq!(switched["scroll_bar"]["pressed"], false, "{switched:#}");
    server.mouse(&step("left-up", (x, y), ""));
}

/// スクロールバックがなければつまみはなく、押しても何も起きない。右ボタンでメニューも出さず、選択も始めない。
#[test]
fn the_scroll_bar_without_scrollback_does_nothing() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);
    assert!(state["scroll_bar"]["thumb"].is_null(), "{state:#}");
    let at = on_scroll_bar(&state, scroll_bar_middle(&state));

    server.mouse(&format!(
        "{}{}{}",
        step("left-down", at, ""),
        step("move", (at.0, at.1 + DRAG_PIXELS), "left"),
        step("left-up", (at.0, at.1 + DRAG_PIXELS), "")
    ));
    server.mouse(&button_click("right", at, ""));
    server.mouse(&button_click("middle", at, ""));
    let after = server.state();
    assert_eq!(after["scroll_bar"]["pressed"], false, "{after:#}");
    assert_eq!(selection(&after), None, "{after:#}");
    assert_eq!(
        after["dialogs"].as_array().map(Vec::len),
        Some(0),
        "{after:#}"
    );
    server.type_line("echo alive-%ERRORLEVEL%-end");
    server.wait_text("alive-0-end");
}

#[test]
fn the_tab_bar_switches_and_opens_tabs() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);

    server.mouse(&left_click(area_center(&state["tab_bar"]["new_tab"])));
    let state = wait_tabs(&server, 2);
    assert_eq!(state["active"], 1, "{state:#}");

    // タブは押したときに表示する。
    server.mouse(&step("left-down", on_tab(&state, 0), ""));
    assert_eq!(server.state()["active"], 0);
    server.mouse(&step("left-up", on_tab(&state, 0), ""));

    // タブを押して離す間の動きで、タブを並べ替えない。選択も始めない。
    server.type_line("echo first-%ERRORLEVEL%-tab");
    let state = server.wait_text("first-0-tab");
    server.mouse(&left_drag(on_tab(&state, 0), on_tab(&state, 1), ""));
    let state = server.state();
    assert_eq!(state["active"], 0, "{state:#}");
    assert_eq!(tab_count(&state), 2, "{state:#}");
    assert_eq!(selection(&state), None, "{state:#}");
    server.wait_text("first-0-tab");
    server.mouse(&step("left-down", on_tab(&state, 1), ""));
    server.mouse(&step("left-up", on_tab(&state, 1), ""));
    assert_eq!(server.state()["active"], 1);
    let text = server.state();
    assert!(
        !text["text"]
            .as_str()
            .unwrap_or_default()
            .contains("first-0-tab"),
        "{text:#}"
    );

    // タブバーの何もないところは何もしない。
    let right = i64::try_from(number(&state["client_width"])).expect("the width fits") - 2;
    server.mouse(&left_click((right, on_tab(&state, 0).1)));
    let state = server.state();
    assert_eq!(tab_count(&state), 2, "{state:#}");
    assert_eq!(state["active"], 1, "{state:#}");
}

#[test]
fn the_close_buttons_and_the_middle_button_close_tabs() {
    let mut server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);
    let new_tab = area_center(&state["tab_bar"]["new_tab"]);
    server.mouse(&left_click(new_tab));
    let state = wait_tabs(&server, 2);
    server.mouse(&left_click(area_center(&state["tab_bar"]["new_tab"])));
    let state = wait_tabs(&server, 3);
    server.mouse(&left_click(area_center(&state["tab_bar"]["new_tab"])));
    let state = wait_tabs(&server, 4);
    assert_eq!(state["active"], 3, "{state:#}");

    // 閉じるボタンの上で押して、外で離したら閉じない。
    let close = on_close_button(&state, 3);
    server.mouse(&format!(
        "{}{}",
        step("left-down", close, ""),
        step("left-up", on_tab(&state, 0), "")
    ));
    assert_eq!(tab_count(&server.state()), 4);

    // 表示していないタブの閉じるボタンは、そのタブを閉じる。表示は替えない。
    server.mouse(&left_click(on_close_button(&state, 0)));
    let state = wait_tabs(&server, 3);
    assert_eq!(state["active"], 2, "{state:#}");

    // 中ボタンは、押したのと同じタブの上で離したら閉じる。
    server.mouse(&format!(
        "{}{}",
        step("middle-down", on_tab(&state, 0), ""),
        step("middle-up", on_tab(&state, 1), "")
    ));
    assert_eq!(tab_count(&server.state()), 3);
    server.mouse(&button_click("middle", on_tab(&state, 0), ""));
    let state = wait_tabs(&server, 2);
    assert_eq!(state["active"], 1, "{state:#}");

    server.mouse(&left_click(on_close_button(&state, 1)));
    let state = wait_tabs(&server, 1);
    assert_eq!(state["active"], 0, "{state:#}");

    // 最後のタブを閉じるとウィンドウを閉じる。
    server.mouse(&left_click(on_close_button(&state, 0)));
    assert_eq!(server.wait_exit(), 0);
}

#[test]
fn the_tab_bar_buttons_and_the_right_button_open_menus() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);
    for steps in [
        left_click(area_center(&state["tab_bar"]["menu"])),
        left_click(area_center(&state["tab_bar"]["profiles"])),
        button_click("right", area_center(&state["tab_bar"]["new_tab"]), ""),
        button_click("right", on_tab(&state, 0), ""),
    ] {
        server.mouse(&steps);
        wait_menu(&server);
        close_menu(&server);
    }
    assert_eq!(tab_count(&server.state()), 1);
}

/// マウスの報告の読み手を動かす `/bin/sh` のコマンド。`cat -v` が受け取ったバイト列を出す。`modes` は先に出す制御
/// シーケンス。
fn start_report_reader(server: &Server, modes: &str) {
    server.send_line(&format!(
        r"stty -icanon -echo -isig -icrnl; printf '{modes}'; echo reader-$((6*7)); cat -v"
    ));
    server.wait_text("reader-42");
}

/// 新しいタブのシェルで読み手を動かす。
fn start_report_reader_in_new_tab(server: &Server, modes: &str) -> Value {
    let tabs = tab_count(&server.state());
    server.post("/e2e/tab/new", b"");
    server.wait_until("the new tab", |state| tab_count(state) == tabs + 1);
    wait_sh_ready(server);
    start_report_reader(server, modes);
    server.state()
}

/// 読み手に区切りを出させる印の連番。
static REPORT_MARKER: AtomicU32 = AtomicU32::new(0);

/// `steps` を送り、読み手が受け取ったものを返す。区切りの印を入力へ送り、それが出るのを待つので、`steps` の分はすべて
/// 届いている。印を送ると選択は外れる。
#[track_caller]
fn reports_of(server: &Server, steps: &str) -> String {
    server.mouse(steps);
    let marker = format!("#{}#", REPORT_MARKER.fetch_add(1, Ordering::Relaxed));
    server.post("/e2e/input", format!("{marker}\n").as_bytes());
    let state = server.wait_text(&marker);
    let text = state["text"].as_str().unwrap_or_default();
    let line = text
        .lines()
        .find(|line| line.contains(&marker))
        .expect("the marker is in the text");
    line[..line.find(&marker).expect("the marker is in the line")].to_owned()
}

/// SGR の形の報告。`col` と `row` は 0 から数える。
fn sgr(button: u32, col: u32, row: u32, end: char) -> String {
    format!("^[[<{button};{};{}{end}", col + 1, row + 1)
}

/// SGR の形の報告の、アプリがマウスを求める制御シーケンス。
const SGR_CLICKS: &str = r"\033[?1000h\033[?1006h";
const SGR_DRAGS: &str = r"\033[?1002h\033[?1006h";
const SGR_MOTION: &str = r"\033[?1003h\033[?1006h";
/// SGR でない古い形の報告の、アプリがマウスを求める制御シーケンス。
const CLICKS_IN_THE_LEGACY_FORM: &str = r"\033[?1000h";
/// 代替画面に入って、カーソルを左上へ置く制御シーケンス。
const ALTERNATE_SCREEN: &str = r"\033[?1049h\033[H";
/// `cat -v` が出す、カーソルキーのモードが既定のときの上下のカーソルキー。
const CURSOR_UP: &str = "^[[A";
const CURSOR_DOWN: &str = "^[[B";

/// SGR のボタンの番号。修飾キーは足す。動きは 32 を足す。ホイールの奥は 64、手前は 65。
const SGR_LEFT: u32 = 0;
const SGR_MIDDLE: u32 = 1;
const SGR_RIGHT: u32 = 2;
const SGR_ALT: u32 = 8;
const SGR_CTRL: u32 = 16;
const SGR_MOTION_BIT: u32 = 32;
const SGR_NO_BUTTON: u32 = 3;
const SGR_WHEEL_UP: u32 = 64;
const SGR_WHEEL_DOWN: u32 = 65;

const REPORT_COL: u32 = 4;
const REPORT_ROW: u32 = 2;

fn exercise_clicks_reach_the_application(server: &Server) {
    wait_sh_ready(server);
    start_report_reader(server, SGR_CLICKS);
    let state = server.wait_until("the mouse mode", |state| state["mouse_tracking"] == true);
    let at = cell_center(&state, REPORT_COL, REPORT_ROW);
    let click_report = |button| {
        format!(
            "{}{}",
            sgr(button, REPORT_COL, REPORT_ROW, 'M'),
            sgr(button, REPORT_COL, REPORT_ROW, 'm')
        )
    };

    assert_eq!(
        reports_of(server, &button_click("left", at, "")),
        click_report(SGR_LEFT)
    );
    assert_eq!(
        reports_of(server, &button_click("middle", at, "")),
        click_report(SGR_MIDDLE)
    );
    assert_eq!(
        reports_of(server, &button_click("right", at, "")),
        click_report(SGR_RIGHT)
    );
    let after = server.state();
    assert_eq!(
        after["dialogs"].as_array().map(Vec::len),
        Some(0),
        "{after:#}"
    );
    assert_eq!(
        reports_of(server, &button_click("left", at, "ctrl")),
        click_report(SGR_LEFT + SGR_CTRL)
    );
    assert_eq!(
        reports_of(server, &button_click("left", at, "alt")),
        click_report(SGR_LEFT + SGR_ALT)
    );

    // このモードでは動きを送らない。
    assert_eq!(reports_of(server, &step("move", at, "")), "");

    // 余白の上は、いちばん近いセルとして送る。
    let padding = i64::try_from(number(&state["terminal_x"])).expect("the position fits") - 1;
    assert_eq!(
        reports_of(server, &button_click("left", (padding, at.1), "")),
        format!(
            "{}{}",
            sgr(SGR_LEFT, 0, REPORT_ROW, 'M'),
            sgr(SGR_LEFT, 0, REPORT_ROW, 'm')
        )
    );

    // アプリが求めている間は、左ボタンで選択を始めない。Shift を押していれば選択で、送らない。右ボタンはメニュー。
    let (col, row) = find_cell(&state, "reader-42");
    let from = cell_left(&state, col, row);
    let to = cell_right(&state, col + 6, row);
    assert_eq!(selection(&server.state()), None);
    server.mouse(&left_drag(from, to, "shift"));
    let selected = server.state();
    assert_eq!(selection(&selected), Some("reader-"), "{selected:#}");
    assert_eq!(reports_of(server, ""), "");
    server.mouse(&button_click("right", at, "shift"));
    wait_menu(server);
    close_menu(server);
    assert_eq!(reports_of(server, ""), "");

    // スクロールバーとタブバーの上の操作は送らない。
    let on_bar = on_scroll_bar(&state, scroll_bar_middle(&state));
    assert_eq!(reports_of(server, &button_click("left", on_bar, "")), "");
    assert_eq!(
        reports_of(server, &button_click("left", on_tab(&state, 0), "")),
        ""
    );
    assert_eq!(reports_of(server, &step("move", on_tab(&state, 0), "")), "");
}

/// ConPTY のタブでも、コンソールの入力を VT にしたアプリへマウスの報告が届く。PowerShell で読み手を動かす。読んだ
/// バイト列の ESC を `^[` に置き換えて出す。
#[test]
fn mouse_reports_reach_an_application_through_conpty() {
    /// コンソールの入力を VT の形で受け取るモードにして、読んだものを出し続けるスクリプト。
    const READER: &str = concat!(
        "$k=Add-Type -PassThru -Name Con -MemberDefinition '",
        "[DllImport(\"kernel32\")]public static extern IntPtr GetStdHandle(int h);",
        "[DllImport(\"kernel32\")]public static extern bool SetConsoleMode(IntPtr h,uint m);';",
        "$null=$k::SetConsoleMode($k::GetStdHandle(-10),0x200);",
        "$e=[string][char]27;",
        "[Console]::Write($e+'[?1000h'+$e+'[?1006h');",
        "'reader-'+(6*7);",
        "$i=[Console]::OpenStandardInput();$b=New-Object byte[] 256;",
        "while($true){$n=$i.Read($b,0,256);",
        "[Console]::WriteLine([Text.Encoding]::ASCII.GetString($b,0,$n).Replace($e,'^['))}",
    );

    let server = Server::start_powershell();
    server.wait_text("PS ");
    server.send_line(READER);
    server.wait_text("reader-42");
    let state = server.wait_until("the mouse mode", |state| state["mouse_tracking"] == true);
    let at = cell_center(&state, REPORT_COL, REPORT_ROW);

    server.mouse(&button_click("left", at, ""));
    server.wait_text(&sgr(SGR_LEFT, REPORT_COL, REPORT_ROW, 'M'));
    server.wait_text(&sgr(SGR_LEFT, REPORT_COL, REPORT_ROW, 'm'));
    server.mouse(&wheel(NOTCH, at));
    server.wait_text(&sgr(SGR_WHEEL_UP, REPORT_COL, REPORT_ROW, 'M'));

    // マウスをやめて代替画面に入ると、ホイールはカーソルキーになる。読み手は動いたままなので、別のタブで動かす。
    server.post("/e2e/tab/new", b"");
    server.wait_until("the new tab", |state| tab_count(state) == 2);
    server.wait_text("PS ");
    server.send_line(&READER.replace("[?1000h'+$e+'[?1006h", "[?1049h"));
    let state = server.wait_text("reader-42");
    assert_eq!(state["mouse_tracking"], false, "{state:#}");
    server.mouse(&wheel(NOTCH, at));
    server.wait_text(CURSOR_UP);
}

#[test]
fn cygwin_clicks_reach_the_application() {
    let Some(config) = cygwin_config("cygwin_clicks_reach_the_application") else {
        return;
    };
    exercise_clicks_reach_the_application(&Server::start(&config, &[]));
}

#[test]
fn wsl_clicks_reach_the_application() {
    let Some(config) = wsl_config("wsl_clicks_reach_the_application") else {
        return;
    };
    exercise_clicks_reach_the_application(&Server::start(&config, &[]));
}

/// ドラッグと動きの報告、古い形の報告。モードごとにタブを開く。
fn exercise_motion_reaches_the_application(server: &Server) {
    wait_sh_ready(server);
    start_report_reader(server, SGR_DRAGS);
    let state = server.wait_until("the mouse mode", |state| state["mouse_tracking"] == true);
    let from = cell_center(&state, REPORT_COL, REPORT_ROW);
    let to = cell_center(&state, REPORT_COL + 3, REPORT_ROW + 1);

    // ボタンを押している間だけ動きを送る。
    assert_eq!(reports_of(server, &step("move", to, "")), "");
    assert_eq!(
        reports_of(server, &left_drag(from, to, "")),
        format!(
            "{}{}{}",
            sgr(SGR_LEFT, REPORT_COL, REPORT_ROW, 'M'),
            sgr(
                SGR_LEFT + SGR_MOTION_BIT,
                REPORT_COL + 3,
                REPORT_ROW + 1,
                'M'
            ),
            sgr(SGR_LEFT, REPORT_COL + 3, REPORT_ROW + 1, 'm')
        )
    );

    let state = start_report_reader_in_new_tab(server, SGR_MOTION);
    assert_eq!(state["mouse_tracking"], true, "{state:#}");
    let at = cell_center(&state, REPORT_COL, REPORT_ROW);
    assert_eq!(
        reports_of(server, &step("move", at, "")),
        sgr(SGR_NO_BUTTON + SGR_MOTION_BIT, REPORT_COL, REPORT_ROW, 'M')
    );

    // SGR を求めなければ、古い形（ボタンと位置に 32 を足したバイト）で送る。
    let state = start_report_reader_in_new_tab(server, r"\033[?1000h");
    let at = cell_center(&state, REPORT_COL, REPORT_ROW);
    assert_eq!(
        reports_of(server, &button_click("left", at, "")),
        "^[[M %#^[[M#%#"
    );
}

#[test]
fn cygwin_motion_reaches_the_application() {
    let Some(config) = cygwin_config("cygwin_motion_reaches_the_application") else {
        return;
    };
    exercise_motion_reaches_the_application(&Server::start(&config, &[]));
}

#[test]
fn wsl_motion_reaches_the_application() {
    let Some(config) = wsl_config("wsl_motion_reaches_the_application") else {
        return;
    };
    exercise_motion_reaches_the_application(&Server::start(&config, &[]));
}

/// アプリがマウスを求めているときと代替画面の、ホイールの振る舞い。
fn exercise_wheel_with_applications(server: &Server) {
    wait_sh_ready(server);
    start_report_reader(server, SGR_CLICKS);
    let state = server.wait_until("the mouse mode", |state| state["mouse_tracking"] == true);
    let at = cell_center(&state, REPORT_COL, REPORT_ROW);
    // 1 ノッチごとに、ボタンの押下だけを送る。
    let up = sgr(SGR_WHEEL_UP, REPORT_COL, REPORT_ROW, 'M');
    let down = sgr(SGR_WHEEL_DOWN, REPORT_COL, REPORT_ROW, 'M');

    assert_eq!(reports_of(server, &wheel(NOTCH, at)), up);
    assert_eq!(reports_of(server, &wheel(-NOTCH, at)), down);
    assert_eq!(
        reports_of(server, &wheel(2 * NOTCH, at)),
        format!("{up}{up}")
    );
    // 半分のノッチは溜めて、合わせて 1 ノッチになる。
    assert_eq!(reports_of(server, &wheel(NOTCH / 2, at)), "");
    assert_eq!(reports_of(server, &wheel(NOTCH / 2, at)), up);
    // タブバーの上では送らない。
    assert_eq!(reports_of(server, &wheel(NOTCH, on_tab(&state, 0))), "");

    // 代替画面にはスクロールバックがなく、つまみを描かない。アプリが求めていれば送る。
    let state = start_report_reader_in_new_tab(server, &format!(r"{ALTERNATE_SCREEN}{SGR_CLICKS}"));
    assert_eq!(state["mouse_tracking"], true, "{state:#}");
    assert_eq!(scroll_offset(&state), 0, "{state:#}");
    assert_eq!(newest_offset(&state), 0, "{state:#}");
    assert!(state["scroll_bar"]["thumb"].is_null(), "{state:#}");
    assert_eq!(reports_of(server, &wheel(NOTCH, at)), up);

    // SGR でない古い形でも、押下だけを送る。押下は `CSI M` に続けて、ボタン + 32、桁 + 33、行 + 33 の文字。
    let state = start_report_reader_in_new_tab(server, CLICKS_IN_THE_LEGACY_FORM);
    assert_eq!(state["mouse_tracking"], true, "{state:#}");
    let legacy = |button: u32| {
        let char_of = |value: u32| char::from_u32(value).expect("a printable character");
        format!(
            "^[[M{}{}{}",
            char_of(button + 32),
            char_of(REPORT_COL + 33),
            char_of(REPORT_ROW + 33)
        )
    };
    assert_eq!(reports_of(server, &wheel(NOTCH, at)), legacy(SGR_WHEEL_UP));
    assert_eq!(
        reports_of(server, &wheel(-NOTCH, at)),
        legacy(SGR_WHEEL_DOWN)
    );

    // 代替画面でアプリが求めていなければ、ノッチの行数だけカーソルキーを送る。表示は動かない。
    let state = start_report_reader_in_new_tab(server, ALTERNATE_SCREEN);
    assert_eq!(state["mouse_tracking"], false, "{state:#}");
    let lines = number(&state["scroll"]["lines_per_notch"]) as usize;
    assert_eq!(
        reports_of(server, &wheel(NOTCH, at)),
        CURSOR_UP.repeat(lines)
    );
    assert_eq!(
        reports_of(server, &wheel(-NOTCH, at)),
        CURSOR_DOWN.repeat(lines)
    );
    // 半分のノッチは溜めて、合わせて 1 ノッチの行数になる。
    let halves = format!(
        "{}{}",
        reports_of(server, &wheel(NOTCH / 2, at)),
        reports_of(server, &wheel(NOTCH / 2, at))
    );
    assert_eq!(halves, CURSOR_UP.repeat(lines));
    // タブバーの上では送らない。
    assert_eq!(reports_of(server, &wheel(NOTCH, on_tab(&state, 0))), "");
    let after = server.state();
    assert_eq!(scroll_offset(&after), 0, "{after:#}");
    assert_eq!(newest_offset(&after), 0, "{after:#}");
    // アプリが求めていない代替画面では、選択できる。
    server.mouse(&left_drag(
        cell_left(&after, 0, 0),
        cell_right(&after, 7, 0),
        "",
    ));
    assert_eq!(selection(&server.state()), Some("reader-4"));

    // カーソルキーのモード（DECCKM）に従う。
    let state = start_report_reader_in_new_tab(server, &format!(r"{ALTERNATE_SCREEN}\033[?1h"));
    let lines = number(&state["scroll"]["lines_per_notch"]) as usize;
    assert_eq!(reports_of(server, &wheel(NOTCH, at)), "^[OA".repeat(lines));
    assert_eq!(reports_of(server, &wheel(-NOTCH, at)), "^[OB".repeat(lines));

    // kitty keyboard protocol では、本物のカーソルキーと同じ形にする。
    start_report_reader_in_new_tab(server, &format!(r"{ALTERNATE_SCREEN}\033[>3u"));
    assert_eq!(
        reports_of(server, &wheel(NOTCH, at)),
        "^[[1;1:1A^[[1;1:3A".repeat(lines)
    );

    // Alternate Scroll（DECSET 1007）を切ったアプリには送らない。
    start_report_reader_in_new_tab(server, &format!(r"{ALTERNATE_SCREEN}\033[?1007l"));
    assert_eq!(reports_of(server, &wheel(NOTCH, at)), "");

    // Shift で報告をやめても、アプリがマウスを求めていればカーソルキーにしない。
    start_report_reader_in_new_tab(server, &format!(r"{ALTERNATE_SCREEN}{SGR_CLICKS}"));
    assert_eq!(
        reports_of(server, &step(&format!("wheel:{NOTCH}"), at, "shift")),
        ""
    );
}

#[test]
fn cygwin_the_wheel_reaches_the_application() {
    let Some(config) = cygwin_config("cygwin_the_wheel_reaches_the_application") else {
        return;
    };
    exercise_wheel_with_applications(&Server::start(&config, &[]));
}

#[test]
fn wsl_the_wheel_reaches_the_application() {
    let Some(config) = wsl_config("wsl_the_wheel_reaches_the_application") else {
        return;
    };
    exercise_wheel_with_applications(&Server::start(&config, &[]));
}
