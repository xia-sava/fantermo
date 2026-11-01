//! 実際の fantermo を画面に出ないデスクトップで起動して、制御の口で確認する（feature `e2e` のときだけ実行する）。
//!
//! 実行は `./tools/dev.sh test --features e2e`。`smoke` で始まるテストは、その環境で別デスクトップと Direct3D が
//! 動くかだけを確認する。
//!
//! ブリッジのテストは環境変数があるときだけ実行し、なければスキップしたと標準エラー出力に出す。
//! - `FANTERMO_E2E_CYGWIN_ROOT`: Cygwin のフォルダ（`C:\cygwin64` など）。`./tools/bridge.sh build cygwin` で作っておく
//! - `FANTERMO_E2E_WSL`: WSL のディストリビューション（`Ubuntu` など）。`./tools/bridge.sh build linux` で作っておく

mod common;

use std::io::Cursor;

use common::Server;
use serde_json::Value;

/// `Server::start_cmd` のプロンプト。
const CMD_PROMPT: &str = "e2e>";

fn tab_count(state: &Value) -> usize {
    state["tabs"].as_array().map_or(0, Vec::len)
}

fn number(value: &Value) -> u64 {
    value
        .as_u64()
        .unwrap_or_else(|| panic!("not a number: {value}"))
}

/// ウィンドウのタイトルが `<title> - fantermo <バージョン>` の形になるまで待つ。
#[track_caller]
fn wait_window_title(server: &Server, title: &str) {
    let expected = format!("{title} - fantermo {}", env!("CARGO_PKG_VERSION"));
    server.wait_until(&format!("the window title {expected:?}"), |state| {
        state["window_title"]
            .as_str()
            .is_some_and(|text| text.starts_with(&expected))
    });
}

#[test]
fn smoke_frame_is_written_on_a_hidden_desktop() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);
    let frame = server.frame();
    let info = png::Decoder::new(Cursor::new(frame))
        .read_info()
        .expect("the frame is not a PNG");
    assert_eq!(u64::from(info.info().width), number(&state["client_width"]));
    assert_eq!(
        u64::from(info.info().height),
        number(&state["client_height"])
    );
    // `--nocapture` で実行すれば、通ったときも使っている描画のデバイスなどが分かる。
    eprintln!("{}", server.log());
}

#[test]
fn powershell_shows_a_prompt() {
    let server = Server::start_powershell();
    server.wait_text("PS ");
}

#[test]
fn input_reaches_the_shell_and_its_output_is_read() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    // 打った行そのものには出ない文字列を待つ。
    server.type_line("echo marker-%ERRORLEVEL%-end");
    server.wait_text("marker-0-end");
}

#[test]
fn raw_input_goes_to_the_backend_as_is() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.post("/e2e/input", b"echo raw-%ERRORLEVEL%-end\r");
    server.wait_text("raw-0-end");
}

#[test]
fn tabs_open_switch_and_close() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.type_line("echo first-%ERRORLEVEL%-tab");
    server.wait_text("first-0-tab");

    server.post("/e2e/tab/new", b"");
    let state = server.wait_until("two tabs", |state| tab_count(state) == 2);
    assert_eq!(state["active"], 1);
    server.wait_text(CMD_PROMPT);
    let second = server.state();
    assert!(
        !second["text"]
            .as_str()
            .unwrap_or_default()
            .contains("first-0-tab"),
        "the new tab shows the first tab's text: {second:#}"
    );
    server.type_line("echo second-%ERRORLEVEL%-tab");
    server.wait_text("second-0-tab");

    // キーの割り当てを通しても開ける。
    server.press("ctrl+shift+t");
    let state = server.wait_until("three tabs", |state| tab_count(state) == 3);
    assert_eq!(state["active"], 2);
    server.press("ctrl+shift+w");
    let state = server.wait_until("two tabs again", |state| tab_count(state) == 2);
    assert_eq!(state["active"], 1);

    server.post("/e2e/tab/select/0", b"");
    let state = server.wait_text("first-0-tab");
    assert_eq!(state["active"], 0);
    server.post("/e2e/tab/select/1", b"");
    server.wait_text("second-0-tab");
    let (status, _) = server.request("POST", "/e2e/tab/select/9", b"");
    assert_eq!(status, 400);

    server.post("/e2e/tab/close", b"");
    let state = server.wait_until("one tab", |state| tab_count(state) == 1);
    assert_eq!(state["active"], 0);
    server.wait_text("first-0-tab");
}

#[test]
fn closing_a_tab_right_after_opening_it_does_not_block_the_window() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    for _ in 0..3 {
        server.post("/e2e/tab/new", b"");
        server.post("/e2e/tab/close", b"");
    }
    let state = server.state();
    assert_eq!(tab_count(&state), 1, "{state:#}");
}

#[test]
fn resizing_the_window_changes_the_grid_and_reaches_the_shell() {
    let server = Server::start_powershell();
    server.wait_text("PS ");
    let print_size = "\"[$($Host.UI.RawUI.WindowSize.Width)x$($Host.UI.RawUI.WindowSize.Height)]\"";

    server.resize(87, 23);
    let state = server.state();
    assert_eq!(state["cols"], 87, "{state:#}");
    assert_eq!(state["rows"], 23, "{state:#}");
    server.type_line(print_size);
    server.wait_text("[87x23]");

    server.resize(120, 40);
    let state = server.state();
    assert_eq!(state["cols"], 120, "{state:#}");
    assert_eq!(state["rows"], 40, "{state:#}");
    server.type_line(print_size);
    server.wait_text("[120x40]");
}

#[test]
fn osc_2_sets_the_window_title() {
    let server = Server::start_powershell();
    server.wait_text("PS ");
    server.type_line("[Console]::Write([char]27 + ']2;e2e-title' + [char]7)");
    wait_window_title(&server, "e2e-title");
}

#[test]
fn shell_exit_closes_its_tab_and_then_the_window() {
    let mut server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    server.wait_text(CMD_PROMPT);

    server.type_line("exit 3");
    server.wait_until("one tab", |state| tab_count(state) == 1);

    server.type_line("exit 7");
    assert_eq!(server.wait_exit(), 7);
}

#[test]
fn quit_closes_the_window() {
    let mut server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.post("/e2e/quit", b"");
    assert_eq!(server.wait_exit(), 0);
}

/// ブリッジのタブの基本: 入出力、大きさ、タイトル、シェルの終了コード。シェルは `/bin/sh`。
fn exercise_bridge_profile(mut server: Server) {
    let state = server.state();
    let (cols, rows) = (number(&state["cols"]), number(&state["rows"]));
    server.type_line("echo hello-$((6*7)); stty size");
    server.wait_text("hello-42");
    server.wait_text(&format!("{rows} {cols}"));

    server.resize(90, 25);
    server.type_line("stty size");
    server.wait_text("25 90");

    server.type_line(r"printf '\033]2;e2e-bridge\007'");
    wait_window_title(&server, "e2e-bridge");

    server.type_line("exit 7");
    assert_eq!(server.wait_exit(), 7);
}

#[test]
fn cygwin_bridge_runs_a_shell() {
    let Some(root) = std::env::var_os("FANTERMO_E2E_CYGWIN_ROOT") else {
        common::report_skip("cygwin_bridge_runs_a_shell", "FANTERMO_E2E_CYGWIN_ROOT");
        return;
    };
    common::install_bridge("x86_64-pc-cygwin", "fantermo-bridge.exe");
    let config = format!(
        r#"[[profiles]]
name = "cygwin"
backend = "cygwin"
cygwin_root = '{}'
command = "/bin/sh"
"#,
        root.display()
    );
    exercise_bridge_profile(Server::start(&config, &[]));
}

#[test]
fn wsl_bridge_runs_a_shell() {
    let Some(distribution) = std::env::var_os("FANTERMO_E2E_WSL") else {
        common::report_skip("wsl_bridge_runs_a_shell", "FANTERMO_E2E_WSL");
        return;
    };
    common::install_bridge("x86_64-unknown-linux-musl", "fantermo-bridge");
    let config = format!(
        r#"[[profiles]]
name = "wsl"
backend = "wsl"
distribution = '{}'
command = "/bin/sh"
"#,
        distribution.display()
    );
    exercise_bridge_profile(Server::start(&config, &[]));
}
