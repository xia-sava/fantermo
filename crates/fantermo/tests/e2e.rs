//! 実際の fantermo を画面に出ないデスクトップで起動して、制御の口で確認する（feature `e2e` のときだけ実行する）。
//!
//! 実行は `./tools/dev.sh test --features e2e`。`smoke` で始まるテストは、その環境で別デスクトップと Direct3D が
//! 動くかだけを確認する。
//!
//! ブリッジのテストは環境変数があるときだけ実行し、なければスキップしたと標準エラー出力に出す。
//! - `FANTERMO_E2E_CYGWIN_ROOT`: Cygwin のフォルダ（`C:\cygwin64` など）。`./tools/bridge.sh build cygwin` で作っておく
//! - `FANTERMO_E2E_WSL`: WSL のディストリビューション（`Ubuntu` など）。`./tools/bridge.sh build linux` で作っておく
//!
//! クリップボードのテストは、一時フォルダのファイルを OS のクリップボードの代わりに使う。OS のクリップボードを
//! 実際に変えるテストは、環境変数 `FANTERMO_E2E_OS_CLIPBOARD` があるときだけ実行する（CI で付ける）。

mod common;

use std::{
    io::Cursor,
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};

use common::{
    OsClipboard, RestoreClipboard, Server,
    processes::{self, Process},
};
use fantermo_core::state::{self, State, WindowState};
use serde_json::Value;

/// `Server::start_cmd` のプロンプト。
const CMD_PROMPT: &str = "e2e>";

/// Cygwin の `sleep` が動き出す、または終わるのを待つ上限。
const CYGWIN_SLEEP_TIMEOUT: Duration = Duration::from_secs(60);

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

    // GitHub Actions のランナーの画面は 1024×768 で、それより大きいウィンドウにはならない。
    server.resize(96, 30);
    let state = server.state();
    assert_eq!(state["cols"], 96, "{state:#}");
    assert_eq!(state["rows"], 30, "{state:#}");
    server.type_line(print_size);
    server.wait_text("[96x30]");
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

/// Cygwin のブリッジのプロファイルの設定。ブリッジを置く。環境変数がなければスキップしたと知らせて `None`。
fn cygwin_config(test: &str) -> Option<String> {
    let Some(root) = std::env::var_os("FANTERMO_E2E_CYGWIN_ROOT") else {
        common::report_skip(test, "FANTERMO_E2E_CYGWIN_ROOT");
        return None;
    };
    common::install_bridge("x86_64-pc-cygwin", "fantermo-bridge.exe");
    Some(format!(
        r#"[[profiles]]
name = "cygwin"
backend = "cygwin"
cygwin_root = '{}'
command = "/bin/sh"
"#,
        root.display()
    ))
}

/// WSL のブリッジのプロファイルの設定。ブリッジを置く。環境変数がなければスキップしたと知らせて `None`。
fn wsl_config(test: &str) -> Option<String> {
    let Some(distribution) = std::env::var_os("FANTERMO_E2E_WSL") else {
        common::report_skip(test, "FANTERMO_E2E_WSL");
        return None;
    };
    common::install_bridge("x86_64-unknown-linux-musl", "fantermo-bridge");
    Some(format!(
        r#"[[profiles]]
name = "wsl"
backend = "wsl"
distribution = '{}'
command = "/bin/sh"
"#,
        distribution.display()
    ))
}

#[test]
fn cygwin_bridge_runs_a_shell() {
    let Some(config) = cygwin_config("cygwin_bridge_runs_a_shell") else {
        return;
    };
    exercise_bridge_profile(Server::start(&config, &[]));
}

#[test]
fn wsl_bridge_runs_a_shell() {
    let Some(config) = wsl_config("wsl_bridge_runs_a_shell") else {
        return;
    };
    exercise_bridge_profile(Server::start(&config, &[]));
}

/// 長く動くコマンドを動かしているタブ 2 つと、何も動かしていないタブ 1 つでウィンドウを閉じると、子孫のプロセスが
/// 残らない。`wait_ready` はタブのシェルが入力を受け付けるのを待つ。
fn exercise_closing_the_window(
    mut server: Server,
    long_commands: [&str; 2],
    wait_long_commands: &dyn Fn(&Server),
    wait_ready: fn(&Server),
) {
    wait_ready(&server);
    server.send_line(long_commands[0]);
    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    wait_ready(&server);
    server.send_line(long_commands[1]);
    server.post("/e2e/tab/new", b"");
    server.wait_until("three tabs", |state| tab_count(state) == 3);
    wait_ready(&server);

    wait_long_commands(&server);
    server.post("/e2e/quit", b"");
    assert_eq!(server.wait_exit(), 0);
    server.assert_no_children_remained();
}

/// `ping -t` は止めるまで動く。
const CMD_LONG_COMMAND: &str = "ping -t 127.0.0.1";
const CMD_LONG_PROCESS: &str = "ping.exe";

/// `/bin/sh` で長く動くコマンド。
const SH_LONG_COMMAND: &str = "sleep 300";

fn wait_cmd_ready(server: &Server) {
    server.wait_text(CMD_PROMPT);
}

/// `/bin/sh` が入力を読むのを待つ。プロンプトは環境によって違うので、コマンドの結果で見る。
fn wait_sh_ready(server: &Server) {
    server.type_line("echo ready-$((1+1))");
    server.wait_text("ready-2");
}

#[test]
fn closing_the_window_ends_the_child_processes_of_every_tab() {
    let server = Server::start_watching_children("", &["cmd.exe", "/k", "prompt", "e2e$G"]);
    let wait_pings = |server: &Server| {
        processes::wait_descendants_named(server.pid(), CMD_LONG_PROCESS, 2);
    };
    exercise_closing_the_window(
        server,
        [CMD_LONG_COMMAND, CMD_LONG_COMMAND],
        &wait_pings,
        wait_cmd_ready,
    );
}

/// 長く動くコマンドを動かしているタブを閉じると、そのタブの子孫のプロセスが終わり、ほかのタブのものは動き続ける。
#[test]
fn closing_a_tab_ends_its_child_processes_only() {
    let server = Server::start_cmd();
    wait_cmd_ready(&server);
    let pid = server.pid();
    let first = processes::descendants(pid);

    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    wait_cmd_ready(&server);
    server.type_line(CMD_LONG_COMMAND);
    let second: Vec<Process> = processes::wait_descendants_named(pid, CMD_LONG_PROCESS, 1)
        .into_iter()
        .filter(|process| !first.contains(process))
        .collect();

    server.post("/e2e/tab/close", b"");
    server.wait_until("one tab", |state| tab_count(state) == 1);
    processes::wait_ended(&second);
    processes::assert_running(&first);
}

/// Cygwin のプログラムの親は Windows のプロセスの親子で辿れないので、`/proc` のコマンドラインを見る。
/// 自分の引数に一致しないよう、`.` を `[.]` と書いて探す。
fn cygwin_sleep_is_running(marker: &str) -> bool {
    let root = std::env::var_os("FANTERMO_E2E_CYGWIN_ROOT").expect("no Cygwin root");
    let bin = std::path::Path::new(&root).join("bin");
    let pattern = marker.replace('.', "[.]");
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("cannot make PATH");
    let output = std::process::Command::new(bin.join("sh.exe"))
        .args([
            "-c",
            &format!("grep -aqs {pattern} /proc/[0-9]*/cmdline && echo found; true"),
        ])
        .env("PATH", path)
        .env_remove("HOME")
        .output()
        .expect("cannot start the Cygwin shell");
    String::from_utf8_lossy(&output.stdout).contains("found")
}

/// 実行ファイルの中で重ならない、`sleep` の引数にできる数。
fn unique_sleep_seconds() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        "300.{}{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[track_caller]
fn wait_cygwin_sleep(marker: &str, running: bool) {
    let deadline = Instant::now() + CYGWIN_SLEEP_TIMEOUT;
    while cygwin_sleep_is_running(marker) != running {
        assert!(
            Instant::now() < deadline,
            "sleep {marker} did not become running={running}"
        );
    }
}

#[test]
fn cygwin_closing_the_window_ends_the_child_processes_of_every_tab() {
    let Some(config) = cygwin_config("cygwin_closing_the_window") else {
        return;
    };
    let server = Server::start_watching_children(&config, &[]);
    let (first, second) = (unique_sleep_seconds(), unique_sleep_seconds());
    let wait_sleeps = |_: &Server| {
        wait_cygwin_sleep(&first, true);
        wait_cygwin_sleep(&second, true);
    };
    exercise_closing_the_window(
        server,
        [&format!("sleep {first}"), &format!("sleep {second}")],
        &wait_sleeps,
        wait_sh_ready,
    );
    wait_cygwin_sleep(&first, false);
    wait_cygwin_sleep(&second, false);
}

#[test]
fn cygwin_closing_a_tab_ends_its_child_processes_only() {
    let Some(config) = cygwin_config("cygwin_closing_a_tab") else {
        return;
    };
    let server = Server::start(&config, &[]);
    wait_sh_ready(&server);
    let first = unique_sleep_seconds();
    server.send_line(&format!("sleep {first}"));
    wait_cygwin_sleep(&first, true);

    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    wait_sh_ready(&server);
    let second = unique_sleep_seconds();
    server.send_line(&format!("sleep {second}"));
    wait_cygwin_sleep(&second, true);

    server.post("/e2e/tab/close", b"");
    server.wait_until("one tab", |state| tab_count(state) == 1);
    wait_cygwin_sleep(&second, false);
    assert!(cygwin_sleep_is_running(&first));
}

/// WSL の中のプロセスは見えないので、Windows の側の `wsl.exe` までを見る。
#[test]
fn wsl_closing_the_window_ends_the_child_processes_of_every_tab() {
    let Some(config) = wsl_config("wsl_closing_the_window") else {
        return;
    };
    let server = Server::start_watching_children(&config, &[]);
    exercise_closing_the_window(
        server,
        [SH_LONG_COMMAND, SH_LONG_COMMAND],
        &|_| {},
        wait_sh_ready,
    );
}

/// `IDOK`。編集のダイアログの「貼り付け」。
const BUTTON_OK: i32 = 1;
/// `IDCANCEL`。編集のダイアログの「キャンセル」。
const BUTTON_CANCEL: i32 = 2;
/// `paste.confirm_size` の既定の、バイトの単位の値。
const DEFAULT_PASTE_CONFIRM_BYTES: usize = 8 * 1024;
/// 編集のダイアログのウィンドウのクラス。
const TEXT_DIALOG_CLASS: &str = "fantermo-text-dialog";
/// 改行を含む貼り付けをそのまま送る設定。
const PASTE_NEVER_CONFIRMS: &str = "[paste]\nconfirm = \"never\"\n";

/// 全体が `bytes` バイトになる、`cmd.exe` で実行できる行の並び。最後の行は `marker-0-<tail>` を出す。
fn paste_commands(bytes: usize, tail: &str) -> String {
    const LINE: &str = "rem 0123456789012345678901234567890123456789\n";
    let last = format!("echo marker-%ERRORLEVEL%-{tail}\n");
    let mut text = LINE.repeat((bytes - last.len()) / LINE.len());
    let padding = bytes - last.len() - text.len();
    if padding > 0 {
        assert!(padding >= "rem \n".len(), "{padding}");
        text.push_str("rem ");
        text.push_str(&"x".repeat(padding - "rem \n".len()));
        text.push('\n');
    }
    text.push_str(&last);
    assert_eq!(text.len(), bytes);
    text
}

/// 編集のダイアログが一つ開くのを待ち、そのコントロールの文字を返す。
#[track_caller]
fn wait_text_dialog(server: &Server) -> Vec<String> {
    let state = server.wait_dialogs(1);
    let dialog = &state["dialogs"][0];
    assert_eq!(dialog["class"], TEXT_DIALOG_CLASS, "{dialog:#}");
    dialog["controls"]
        .as_array()
        .expect("the dialog has no controls")
        .iter()
        .map(|control| control.as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn a_paste_just_under_the_size_is_not_confirmed() {
    let server = Server::start(PASTE_NEVER_CONFIRMS, &["cmd.exe", "/k", "prompt", "e2e$G"]);
    server.wait_text(CMD_PROMPT);
    server.paste(&paste_commands(DEFAULT_PASTE_CONFIRM_BYTES - 1, "small"));
    let state = server.wait_text("marker-0-small");
    assert_eq!(state["dialogs"].as_array().map(Vec::len), Some(0));
}

#[test]
fn a_paste_of_the_size_is_confirmed_and_can_be_cancelled() {
    let server = Server::start(PASTE_NEVER_CONFIRMS, &["cmd.exe", "/k", "prompt", "e2e$G"]);
    server.wait_text(CMD_PROMPT);

    server.paste(&paste_commands(DEFAULT_PASTE_CONFIRM_BYTES, "cancelled"));
    let controls = wait_text_dialog(&server);
    assert!(
        controls
            .iter()
            .any(|text| text.contains("大きなテキストを貼り付けようとしています（8 KB）")),
        "{controls:?}"
    );
    assert!(
        controls
            .iter()
            .any(|text| text.contains("marker-%ERRORLEVEL%-cancelled")),
        "the text box does not show the text: {controls:?}"
    );
    server.press_dialog_button(BUTTON_CANCEL);
    server.wait_dialogs(0);

    server.paste(&paste_commands(DEFAULT_PASTE_CONFIRM_BYTES, "accepted"));
    wait_text_dialog(&server);
    server.press_dialog_button(BUTTON_OK);
    let state = server.wait_text("marker-0-accepted");
    let text = state["text"].as_str().unwrap_or_default();
    assert!(!text.contains("marker-0-cancelled"), "{text}");
}

#[test]
fn a_large_paste_is_not_confirmed_when_the_size_is_zero() {
    let config = format!("{PASTE_NEVER_CONFIRMS}confirm_size = 0\n");
    let server = Server::start(&config, &["cmd.exe", "/k", "prompt", "e2e$G"]);
    server.wait_text(CMD_PROMPT);
    server.paste(&paste_commands(4 * DEFAULT_PASTE_CONFIRM_BYTES, "large"));
    let state = server.wait_text("marker-0-large");
    assert_eq!(state["dialogs"].as_array().map(Vec::len), Some(0));
}

#[test]
fn a_paste_with_newlines_is_confirmed_when_the_setting_asks_for_it() {
    let server = Server::start(
        "[paste]\nconfirm = \"multiline\"\n",
        &["cmd.exe", "/k", "prompt", "e2e$G"],
    );
    server.wait_text(CMD_PROMPT);
    server.paste("echo multi-%ERRORLEVEL%-line\n");
    let controls = wait_text_dialog(&server);
    assert!(
        !controls.iter().any(|text| text.contains("大きなテキスト")),
        "{controls:?}"
    );
    server.press_dialog_button(BUTTON_OK);
    server.wait_text("multi-0-line");
}

/// 画像の左上のセルの位置（1 から数える）。
const IMAGE_COL: u32 = 5;
const IMAGE_ROW: u32 = 3;
/// 色の許容の差。描画の丸めの分。
const COLOR_TOLERANCE: u8 = 2;

/// `(col, row)` のセル（0 から数える）の左上から `(dx, dy)` ピクセルの位置を、絵の中の位置にする。
fn cell_pixel(state: &Value, col: u32, row: u32, dx: u32, dy: u32) -> (u32, u32) {
    let at = |origin: &str, size: &str, cell: u32, delta: u32| {
        u32::try_from(number(&state[origin]) + number(&state[size]) * u64::from(cell))
            .expect("the position does not fit")
            + delta
    };
    (
        at("terminal_x", "cell_width", col, dx),
        at("terminal_y", "cell_height", row, dy),
    )
}

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
    let Some(config) = cygwin_config("cygwin_kitty_image_is_drawn") else {
        return;
    };
    exercise_kitty_image(&Server::start(&config, &[]));
}

#[test]
fn cygwin_sixel_image_is_drawn() {
    let Some(config) = cygwin_config("cygwin_sixel_image_is_drawn") else {
        return;
    };
    exercise_sixel_image(&Server::start(&config, &[]));
}

#[test]
fn wsl_kitty_image_is_drawn() {
    let Some(config) = wsl_config("wsl_kitty_image_is_drawn") else {
        return;
    };
    exercise_kitty_image(&Server::start(&config, &[]));
}

#[test]
fn wsl_sixel_image_is_drawn() {
    let Some(config) = wsl_config("wsl_sixel_image_is_drawn") else {
        return;
    };
    exercise_sixel_image(&Server::start(&config, &[]));
}

/// 確認のダイアログの「許可」「拒否」「このタブでは今後も許可」。
const BUTTON_ALLOW: i32 = 1001;
const BUTTON_DENY: i32 = 1002;
const BUTTON_ALLOW_FOR_TAB: i32 = 1003;
/// クリップボードに置く試しの文字列。
const CLIPBOARD_TEXT: &str = "fantermo-e2e-clipboard";

/// シェルでクリップボードの読み取りを依頼する。依頼と同じ出力の後ろの `<marker>-1` が画面に出るのを待つので、
/// 戻ったときには fantermo は依頼を受け取っている。答えは [`finish_clipboard_read`] で読む。
fn start_clipboard_read(server: &Server, marker: &str) {
    server.send_line(&format!(
        r"stty raw -echo; printf '\033]52;c;?\007{marker}-%d\r\n' 1; head -n 1 | od -An -c | tr -d ' \n'; stty sane; echo; echo {marker}-end-$((1+1))"
    ));
    server.wait_text(&format!("{marker}-1"));
}

/// 改行を送って `head` を終わらせ、依頼のあとに届いたものを読む。`od -c` の形で、空白を除いた文字列を返す。
fn finish_clipboard_read(server: &Server, marker: &str) -> String {
    server.post("/e2e/input", b"\n");
    let end = format!("{marker}-end-2");
    let state = server.wait_text(&end);
    let text = state["text"].as_str().unwrap_or_default();
    let start = format!("{marker}-1");
    let from = text.rfind(&start).expect("no start marker") + start.len();
    let to = text.rfind(&end).expect("no end marker");
    text[from..to].lines().map(str::trim).collect()
}

/// `od -c` の形で書いた、`text` の読み取りの答えの中身。
fn clipboard_answer(text: &str) -> String {
    format!("]52;c;{}", common::base64(text.as_bytes()))
}

#[test]
fn cygwin_clipboard_read_denied_by_the_setting_gets_no_answer() {
    let Some(config) = cygwin_config("cygwin_clipboard_read_denied") else {
        return;
    };
    let config = format!("[clipboard]\nread = \"deny\"\n{config}");
    let server = Server::start(&config, &[]);
    wait_sh_ready(&server);
    server.set_clipboard(CLIPBOARD_TEXT);
    start_clipboard_read(&server, "denied");
    let state = server.state();
    assert_eq!(state["dialogs"].as_array().map(Vec::len), Some(0));
    let answer = finish_clipboard_read(&server, "denied");
    assert!(!answer.contains("52;c"), "{answer}");
}

#[test]
fn cygwin_clipboard_read_allowed_by_the_setting_is_answered() {
    let Some(config) = cygwin_config("cygwin_clipboard_read_allowed") else {
        return;
    };
    let config = format!("[clipboard]\nread = \"allow\"\n{config}");
    let server = Server::start(&config, &[]);
    wait_sh_ready(&server);
    server.set_clipboard(CLIPBOARD_TEXT);
    start_clipboard_read(&server, "allowed");
    let answer = finish_clipboard_read(&server, "allowed");
    assert!(
        answer.contains(&clipboard_answer(CLIPBOARD_TEXT)),
        "{answer}"
    );
}

#[test]
fn cygwin_clipboard_read_follows_the_answer_to_the_dialog() {
    let Some(config) = cygwin_config("cygwin_clipboard_read_asks") else {
        return;
    };
    let server = Server::start(&config, &[]);
    wait_sh_ready(&server);
    server.set_clipboard(CLIPBOARD_TEXT);

    start_clipboard_read(&server, "refused");
    server.wait_dialogs(1);
    server.press_dialog_button(BUTTON_DENY);
    server.wait_dialogs(0);
    let answer = finish_clipboard_read(&server, "refused");
    assert!(!answer.contains("52;c"), "{answer}");

    start_clipboard_read(&server, "once");
    server.wait_dialogs(1);
    server.press_dialog_button(BUTTON_ALLOW);
    server.wait_dialogs(0);
    let answer = finish_clipboard_read(&server, "once");
    assert!(
        answer.contains(&clipboard_answer(CLIPBOARD_TEXT)),
        "{answer}"
    );

    start_clipboard_read(&server, "tab");
    server.wait_dialogs(1);
    server.press_dialog_button(BUTTON_ALLOW_FOR_TAB);
    server.wait_dialogs(0);
    let answer = finish_clipboard_read(&server, "tab");
    assert!(
        answer.contains(&clipboard_answer(CLIPBOARD_TEXT)),
        "{answer}"
    );

    // このタブでは、もう聞かない。
    start_clipboard_read(&server, "again");
    let state = server.state();
    assert_eq!(state["dialogs"].as_array().map(Vec::len), Some(0));
    let answer = finish_clipboard_read(&server, "again");
    assert!(
        answer.contains(&clipboard_answer(CLIPBOARD_TEXT)),
        "{answer}"
    );
}

#[test]
fn cygwin_clipboard_write_from_an_application_reaches_the_clipboard() {
    let Some(config) = cygwin_config("cygwin_clipboard_write") else {
        return;
    };
    let server = Server::start(&config, &[]);
    wait_sh_ready(&server);
    server.send_line(&format!(
        r"printf '\033]52;c;{}\007'",
        common::base64(CLIPBOARD_TEXT.as_bytes())
    ));
    server.wait_clipboard(CLIPBOARD_TEXT);
}

/// PowerShell から OSC 52 で書く。
fn write_clipboard_through_powershell(server: &Server, text: &str) {
    server.wait_text("PS ");
    server.type_line(&format!(
        "[Console]::Write([char]27 + ']52;c;{}' + [char]7)",
        common::base64(text.as_bytes())
    ));
}

#[test]
fn clipboard_write_through_conpty_reaches_the_clipboard() {
    let server = Server::start_powershell();
    write_clipboard_through_powershell(&server, CLIPBOARD_TEXT);
    server.wait_clipboard(CLIPBOARD_TEXT);
}

/// OS のクリップボードを実際に変える（環境変数があるときだけ）。
#[test]
fn os_clipboard_is_written_by_an_application_and_read_back() {
    let Some(clipboard) = OsClipboard::take("os_clipboard_is_written") else {
        return;
    };
    let server = Server::start_with_os_clipboard(
        &clipboard,
        "",
        &["powershell.exe", "-NoLogo", "-NoProfile"],
    );
    let _restore = RestoreClipboard::new(&server);
    write_clipboard_through_powershell(&server, CLIPBOARD_TEXT);
    server.wait_clipboard(CLIPBOARD_TEXT);

    let other = format!("{CLIPBOARD_TEXT}-other");
    server.set_clipboard(&other);
    server.wait_clipboard(&other);
}

/// アプリの読み取りの依頼に OS のクリップボードの中身で答える（環境変数があるときだけ）。
#[test]
fn cygwin_os_clipboard_answers_a_read() {
    let Some(clipboard) = OsClipboard::take("cygwin_os_clipboard_answers_a_read") else {
        return;
    };
    let Some(config) = cygwin_config("cygwin_os_clipboard_answers_a_read") else {
        return;
    };
    let config = format!("[clipboard]\nread = \"allow\"\n{config}");
    let server = Server::start_with_os_clipboard(&clipboard, &config, &[]);
    let _restore = RestoreClipboard::new(&server);
    wait_sh_ready(&server);
    server.set_clipboard(CLIPBOARD_TEXT);
    start_clipboard_read(&server, "os-read");
    let answer = finish_clipboard_read(&server, "os-read");
    assert!(
        answer.contains(&clipboard_answer(CLIPBOARD_TEXT)),
        "{answer}"
    );
}

#[test]
fn each_tab_keeps_its_own_title() {
    let server = Server::start_powershell();
    server.wait_text("PS ");
    server.type_line("[Console]::Write([char]27 + ']2;title-one' + [char]7)");
    wait_window_title(&server, "title-one");

    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    server.wait_text("PS ");
    server.type_line("[Console]::Write([char]27 + ']2;title-two' + [char]7)");
    wait_window_title(&server, "title-two");

    let state = server.state();
    assert_eq!(state["tabs"][0]["title"], "title-one", "{state:#}");
    assert_eq!(state["tabs"][1]["title"], "title-two", "{state:#}");

    server.post("/e2e/tab/select/0", b"");
    wait_window_title(&server, "title-one");
}

#[test]
fn a_bell_in_a_tab_not_shown_marks_it_until_it_is_shown() {
    let server = Server::start_powershell();
    server.wait_text("PS ");
    // 裏に回したのを確認してから鳴らすよう、ファイルができるのを待つ。
    let trigger = server.dir().join("ring-the-bell");
    server.type_line(&format!(
        "while (-not (Test-Path '{}')) {{ Start-Sleep -Milliseconds 50 }}; [Console]::Write([char]7)",
        trigger.display()
    ));
    server.post("/e2e/tab/new", b"");
    let state = server.wait_until("two tabs", |state| tab_count(state) == 2);
    assert_eq!(state["tabs"][0]["bell"], false, "{state:#}");

    std::fs::write(&trigger, b"").expect("failed to create the trigger");
    server.wait_until("the bell mark", |state| state["tabs"][0]["bell"] == true);

    server.post("/e2e/tab/select/0", b"");
    let state = server.state();
    assert_eq!(state["tabs"][0]["bell"], false, "{state:#}");
}

#[test]
fn the_scrollback_keeps_lines_that_left_the_screen() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.type_line("for /L %i in (1,1,300) do @echo scroll-line-%i");
    let state = server.wait_text("scroll-line-300");
    assert!(number(&state["rows"]) < 300, "{state:#}");
    let text = state["text"].as_str().unwrap_or_default();
    assert!(text.contains("scroll-line-1\n"), "{text}");
    assert!(text.contains("scroll-line-150\n"), "{text}");
}

#[test]
fn profiles_are_listed_and_open_tabs() {
    let config = r#"default_profile = "two"

[[profiles]]
name = "one"
command = "cmd.exe /k prompt one$G"

[[profiles]]
name = "two"
command = "cmd.exe /k prompt two$G"
"#;
    let server = Server::start(config, &[]);
    let state = server.wait_text("two>");
    let names = state["profiles"].as_array().expect("no profiles");
    assert_eq!(names.len(), 2, "{state:#}");
    assert_eq!(names[0], "one");
    assert_eq!(names[1], "two");
    assert_eq!(state["tabs"][0]["profile"], "two", "{state:#}");

    server.post("/e2e/tab/new/0", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    let state = server.wait_text("one>");
    assert_eq!(state["tabs"][1]["profile"], "one", "{state:#}");

    let (status, _) = server.request("POST", "/e2e/tab/new/9", b"");
    assert_eq!(status, 400);
}

#[test]
fn the_settings_are_read_again_when_the_file_changes() {
    let server = Server::start("[font]\nsize = 10\n", &["cmd.exe", "/k", "prompt", "e2e$G"]);
    let state = server.wait_text(CMD_PROMPT);
    let before = number(&state["cell_height"]);
    let path = server.dir().join("config.toml");

    std::fs::write(&path, "[font]\nsize = 20\n").expect("failed to write the settings");
    server.wait_until("taller cells", |state| {
        number(&state["cell_height"]) > before
    });

    // 読めない中身は警告を出し、直すと警告を取り下げる。
    std::fs::write(&path, "[font\n").expect("failed to write the settings");
    server.wait_dialogs(1);
    std::fs::write(&path, "[font]\nsize = 10\n").expect("failed to write the settings");
    server.wait_dialogs(0);
    server.wait_until("the first height", |state| {
        number(&state["cell_height"]) == before
    });
}

/// `cmd.exe` を [`CMD_PROMPT`] のプロンプトで起動する引数。
const CMD_COMMAND: [&str; 4] = ["cmd.exe", "/k", "prompt", "e2e$G"];
/// 状態がないときのウィンドウの桁数と行数。
const DEFAULT_GRID: (u64, u64) = (100, 30);
/// 読めなかった状態のファイルを写す先の名前。
const BROKEN_STATE_FILE: &str = "state.toml.broken";

/// 状態のファイルの `[window]`。ファイルがなかったり読めなければ失敗にする。
#[track_caller]
fn saved_window(server: &Server) -> WindowState {
    let path = server.state_file();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    state::parse(&text)
        .unwrap_or_else(|e| panic!("cannot parse {text:?}: {e}"))
        .window
        .unwrap_or_else(|| panic!("no window in {text:?}"))
}

/// 制御の口の状態の `window`。
#[track_caller]
fn current_window(state: &Value) -> WindowState {
    serde_json::from_value(state["window"].clone())
        .unwrap_or_else(|e| panic!("no window in the state: {e}\n{state:#}"))
}

/// `contents` を `state.toml` に置いて起動し、プロンプトが出るのを待つ。
fn start_with_state(contents: &[u8]) -> (Server, Value) {
    let server = Server::start_with_setup("", &CMD_COMMAND, |dir| {
        std::fs::write(dir.join(common::STATE_FILE), contents).expect("failed to write the state");
    });
    let state = server.wait_text(CMD_PROMPT);
    (server, state)
}

/// ウィンドウを閉じて、fantermo が終わるのを待つ。
#[track_caller]
fn quit(server: &mut Server) {
    server.post("/e2e/quit", b"");
    assert_eq!(server.wait_exit(), 0);
}

#[track_caller]
fn assert_default_grid(state: &Value) {
    assert_eq!(
        (number(&state["cols"]), number(&state["rows"])),
        DEFAULT_GRID,
        "{state:#}"
    );
}

#[test]
fn the_window_size_comes_back_after_restarting() {
    let mut first = Server::start("", &CMD_COMMAND);
    first.wait_text(CMD_PROMPT);
    first.resize(87, 23);
    let resized = first.state();
    quit(&mut first);
    assert_eq!(saved_window(&first), current_window(&resized));
    let saved = std::fs::read(first.state_file()).expect("failed to read the state");
    drop(first);

    let (mut second, state) = start_with_state(&saved);
    assert_eq!(state["cols"], 87, "{state:#}");
    assert_eq!(state["rows"], 23, "{state:#}");
    assert_eq!(state["client_width"], resized["client_width"]);
    assert_eq!(state["client_height"], resized["client_height"]);
    assert_eq!(current_window(&state), current_window(&resized));
    quit(&mut second);
    assert_eq!(saved_window(&second), current_window(&resized));
}

#[test]
fn the_window_position_comes_back_and_is_written_again() {
    const WINDOW: WindowState = WindowState {
        left: 60,
        top: 40,
        width: 700,
        height: 480,
        maximized: false,
    };
    let (mut server, state) = start_with_state(state::to_toml(&seeded_state(WINDOW)).as_bytes());
    assert_eq!(current_window(&state), WINDOW);
    quit(&mut server);
    assert_eq!(saved_window(&server), WINDOW);
}

/// 最大化して閉じても、戻す先の位置と大きさは最大化する前のものが残る。
#[test]
fn a_maximized_window_comes_back_maximized_and_keeps_its_normal_size() {
    const WINDOW: WindowState = WindowState {
        left: 60,
        top: 40,
        width: 700,
        height: 480,
        maximized: true,
    };
    let (mut server, state) = start_with_state(state::to_toml(&seeded_state(WINDOW)).as_bytes());
    assert_eq!(current_window(&state), WINDOW);
    let client_width = number(&state["client_width"]);
    assert!(
        client_width > u64::from(WINDOW.width.unsigned_abs()),
        "the window is not maximized: {state:#}"
    );
    quit(&mut server);
    assert_eq!(saved_window(&server), WINDOW);
}

fn seeded_state(window: WindowState) -> State {
    State {
        window: Some(window),
    }
}

#[test]
fn without_a_state_file_the_window_opens_at_the_default_size_and_writes_one() {
    let mut server = Server::start("", &CMD_COMMAND);
    let state = server.wait_text(CMD_PROMPT);
    assert_default_grid(&state);
    assert!(
        !server.state_file().exists(),
        "the state was written before closing"
    );
    quit(&mut server);
    assert_eq!(saved_window(&server), current_window(&state));
}

/// 読めないファイルは写しておき、既定の大きさで開き、閉じるときに正しい中身で置き換える。
#[test]
fn an_unreadable_state_file_is_copied_aside_and_replaced_when_closing() {
    let contents: [&[u8]; 3] = [
        // TOML として読めない。
        b"[window\n",
        // 状態の型として読めない。
        b"[window]\nleft = \"x\"\ntop = 0\nwidth = 700\nheight = 480\nmaximized = false\n",
        // UTF-8 でない。
        b"[window]\n# \xff\xfe\n",
    ];
    for content in contents {
        let (mut server, state) = start_with_state(content);
        assert_default_grid(&state);
        let broken = server.dir().join(BROKEN_STATE_FILE);
        assert_eq!(
            std::fs::read(&broken).expect("the unreadable state was not copied"),
            content
        );
        assert_eq!(
            std::fs::read(server.state_file()).expect("the unreadable state was removed"),
            content
        );
        quit(&mut server);
        assert_eq!(saved_window(&server), current_window(&state));
        assert_eq!(
            std::fs::read(&broken).expect("the copy was removed"),
            content
        );
    }
}

/// 読めるが使えない値は、写さずに既定の大きさで開く。
#[test]
fn a_state_file_with_an_unusable_size_is_ignored() {
    let (server, state) = start_with_state(
        b"[window]\nleft = 0\ntop = 0\nwidth = -1\nheight = 480\nmaximized = false\n",
    );
    assert_default_grid(&state);
    assert!(!server.dir().join(BROKEN_STATE_FILE).exists());
}

/// 背景画像のテストで固定する端末の背景色。
const TERMINAL_BACKGROUND: [u8; 3] = [16, 32, 48];
const TERMINAL_BACKGROUND_SETTING: &str = "[colors]\nbackground = \"#102030\"\n";
/// 色の許容の差。半透明の重ね合わせの丸めと、ソフトウェアの描画との差の分。
const BACKGROUND_COLOR_TOLERANCE: u8 = 6;
/// 画素が期待の色になるのを待つ上限。読み直しが描画に届くまでの分。
const PICTURE_TIMEOUT: Duration = Duration::from_secs(30);
/// 画素を読み直す間隔。
const PICTURE_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// 背景画像のファイルの名前。設定には設定ファイルのフォルダからの相対パスで書く。
const BANDS_IMAGE: &str = "bands.png";
/// [`bands_image`] の一辺と、上下の帯の高さ。
const BANDS_SIDE: u32 = 64;
const BAND_HEIGHT: u32 = 8;
const BAND_TOP: [u8; 3] = [255, 0, 255];
const BAND_BOTTOM: [u8; 3] = [0, 255, 255];
const MIDDLE_LEFT: [u8; 3] = [255, 0, 0];
const MIDDLE_RIGHT: [u8; 3] = [0, 255, 0];

/// 上と下に帯があり、間を左右に分けた正方形の画像。表示方法ごとに、見える部分と位置が変わる。
fn bands_image() -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..BANDS_SIDE {
        for x in 0..BANDS_SIDE {
            let color = if y < BAND_HEIGHT {
                BAND_TOP
            } else if y >= BANDS_SIDE - BAND_HEIGHT {
                BAND_BOTTOM
            } else if x < BANDS_SIDE / 2 {
                MIDDLE_LEFT
            } else {
                MIDDLE_RIGHT
            };
            pixels.extend_from_slice(&color);
        }
    }
    common::encode_png(BANDS_SIDE, BANDS_SIDE, &pixels)
}

/// 単色の画像。
fn solid_image(color: [u8; 3]) -> Vec<u8> {
    const SIDE: u32 = 16;
    common::encode_png(SIDE, SIDE, &color.repeat((SIDE * SIDE) as usize))
}

/// 背景画像を敷く範囲（絵の中のピクセル）。
#[derive(Clone, Copy, Debug)]
struct BackdropArea {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl BackdropArea {
    fn of(state: &Value) -> Self {
        let field = |name: &str| {
            u32::try_from(number(&state["background"][name])).expect("the area does not fit")
        };
        Self {
            x: field("x"),
            y: field("y"),
            width: field("width"),
            height: field("height"),
        }
    }

    /// 範囲の幅と高さを 1 とした位置 `(fx, fy)`。
    fn at(self, fx: f64, fy: f64) -> (u32, u32) {
        (
            self.x + (f64::from(self.width) * fx) as u32,
            self.y + (f64::from(self.height) * fy) as u32,
        )
    }
}

fn color_is_close(actual: [u8; 3], expected: [u8; 3]) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(&a, e)| a.abs_diff(e) <= BACKGROUND_COLOR_TOLERANCE)
}

/// 不透明度 `image_opacity` の画像の色 `image` が、[`TERMINAL_BACKGROUND`] の上に見える色。
fn over_terminal_background(image: [u8; 3], image_opacity: f64) -> [u8; 3] {
    let mut mixed = [0; 3];
    for ((out, &foreground), &background) in mixed.iter_mut().zip(&image).zip(&TERMINAL_BACKGROUND)
    {
        let value =
            f64::from(foreground) * image_opacity + f64::from(background) * (1.0 - image_opacity);
        *out = value.round() as u8;
    }
    mixed
}

/// `expected` の位置がすべて期待の色になるまで絵を読み直す。
#[track_caller]
fn wait_pixels(server: &Server, expected: &[((u32, u32), [u8; 3])]) {
    let deadline = Instant::now() + PICTURE_TIMEOUT;
    loop {
        let picture = server.picture();
        let wrong: Vec<String> = expected
            .iter()
            .filter_map(|&((x, y), color)| {
                let actual = picture.rgb(x, y);
                (!color_is_close(actual, color))
                    .then(|| format!("({x}, {y}) is {actual:?}, not {color:?}"))
            })
            .collect();
        if wrong.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{}\n{}",
            wrong.join("\n"),
            server.log()
        );
        std::thread::sleep(PICTURE_POLL_INTERVAL);
    }
}

/// 端末の背景色を固定し、設定に `background` を足して起動し、シェルのプロンプトまで待つ。[`BANDS_IMAGE`] を置く。
fn start_with_background(background: &str, prepare: impl FnOnce(&std::path::Path)) -> Server {
    let config = format!("{TERMINAL_BACKGROUND_SETTING}{background}");
    let server = Server::start_with_setup(&config, &CMD_COMMAND, |dir| {
        std::fs::write(dir.join(BANDS_IMAGE), bands_image()).expect("failed to write the image");
        prepare(dir);
    });
    server.wait_text(CMD_PROMPT);
    server
}

/// `image_opacity = 1` で表示方法 `fit` の [`BANDS_IMAGE`] を背景にして起動する。
fn start_with_fit(fit: &str) -> (Server, BackdropArea) {
    let server = start_with_background(
        &format!(
            "[background]\nimage_opacity = 1.0\nfit = \"{fit}\"\n\n[[background.images]]\npath = '{BANDS_IMAGE}'\n"
        ),
        |_| {},
    );
    let state = server.state();
    assert_eq!(state["background"]["image"], BANDS_IMAGE, "{state:#}");
    let area = BackdropArea::of(&state);
    // 表示方法を見分ける位置は、横に長い範囲を前提にしている。
    assert!(
        f64::from(area.width) > 1.4 * f64::from(area.height),
        "{area:?}"
    );
    (server, area)
}

#[test]
fn a_stretched_background_image_shows_all_of_it_and_only_in_its_area() {
    let (server, area) = start_with_fit("stretch");
    wait_pixels(
        &server,
        &[
            (area.at(0.25, 0.03), BAND_TOP),
            (area.at(0.25, 0.5), MIDDLE_LEFT),
            (area.at(0.75, 0.5), MIDDLE_RIGHT),
            (area.at(0.75, 0.97), BAND_BOTTOM),
        ],
    );
    // タブバーとスクロールバーには敷かない。
    let picture = server.picture();
    let (tab_x, tab_y) = area.at(0.75, 0.0);
    assert_ne!(picture.rgb(tab_x, tab_y - 1), BAND_TOP);
    let (bar_x, bar_y) = area.at(1.0, 0.5);
    assert!(!color_is_close(picture.rgb(bar_x, bar_y), MIDDLE_RIGHT));
}

/// 範囲を覆うように広げ、はみ出した上下を切るので、上下の帯は見えない。
#[test]
fn a_filling_background_image_cuts_what_overflows() {
    let (server, area) = start_with_fit("fill");
    wait_pixels(
        &server,
        &[
            (area.at(0.25, 0.03), MIDDLE_LEFT),
            (area.at(0.25, 0.97), MIDDLE_LEFT),
            (area.at(0.75, 0.03), MIDDLE_RIGHT),
            (area.at(0.75, 0.97), MIDDLE_RIGHT),
        ],
    );
}

/// 全体が入る大きさで真ん中に置き、左右の余りは背景色になる。
#[test]
fn a_fitted_background_image_is_whole_in_the_middle() {
    let (server, area) = start_with_fit("fit");
    let side = area.height;
    let left = area.x + (area.width - side) / 2;
    let quarter = left + side / 4;
    let three_quarters = left + side * 3 / 4;
    let middle = area.y + side / 2;
    wait_pixels(
        &server,
        &[
            ((quarter, middle), MIDDLE_LEFT),
            ((three_quarters, middle), MIDDLE_RIGHT),
            ((quarter, area.y + side * 3 / 100), BAND_TOP),
            ((quarter, area.y + side * 97 / 100), BAND_BOTTOM),
            ((area.x + (left - area.x) / 2, middle), TERMINAL_BACKGROUND),
            (
                (area.x + area.width - (left - area.x) / 2, middle),
                TERMINAL_BACKGROUND,
            ),
        ],
    );
}

/// 元の大きさで真ん中に置き、周りは背景色になる。
#[test]
fn a_centered_background_image_keeps_its_size() {
    let (server, area) = start_with_fit("center");
    let (x, y) = area.at(0.5, 0.5);
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
            ((x - side / 4, y - side * 3 / 4), TERMINAL_BACKGROUND),
            ((x - side / 4, y + side * 3 / 4), TERMINAL_BACKGROUND),
        ],
    );
}

/// 元の大きさで範囲の左上から敷き詰める。プロンプトの文字を避けて、右下へ離れたところを見る。
#[test]
fn a_tiled_background_image_repeats_from_the_top_left() {
    let (server, area) = start_with_fit("tile");
    let side = BANDS_SIDE;
    let (left, top) = (area.x + 2 * side, area.y + side);
    wait_pixels(
        &server,
        &[
            ((left + side / 4, top + side / 2), MIDDLE_LEFT),
            ((left + side * 3 / 4, top + side / 2), MIDDLE_RIGHT),
            ((left + side / 4, top + BAND_HEIGHT / 2), BAND_TOP),
            ((left + side / 4, top + side - BAND_HEIGHT / 2), BAND_BOTTOM),
            ((left + side + side / 4, top + side / 2), MIDDLE_LEFT),
            ((left + side / 4, top + side + side / 2), MIDDLE_LEFT),
        ],
    );
}

#[test]
fn the_opacity_of_the_background_image_sets_how_much_of_it_shows() {
    let settings = [
        ("image_opacity = 0.0\n", 0.0),
        ("image_opacity = 0.5\n", 0.5),
        ("image_opacity = 1.0\n", 1.0),
        // 既定の値。
        ("", 0.2),
    ];
    for (opacity, expected) in settings {
        let server = start_with_background(
            &format!(
                "[background]\nfit = \"stretch\"\n{opacity}\n[[background.images]]\npath = '{BANDS_IMAGE}'\n"
            ),
            |_| {},
        );
        let area = BackdropArea::of(&server.state());
        wait_pixels(
            &server,
            &[
                (
                    area.at(0.25, 0.5),
                    over_terminal_background(MIDDLE_LEFT, expected),
                ),
                (
                    area.at(0.75, 0.5),
                    over_terminal_background(MIDDLE_RIGHT, expected),
                ),
            ],
        );
    }
}

/// 項目に書いた表示方法と不透明度は `[background]` の値より優先し、書かなかったものは `[background]` の値に従う。
#[test]
fn an_image_entry_overrides_the_fit_and_the_opacity_it_writes() {
    const WHOLE: &str = "[background]\nfit = \"stretch\"\nimage_opacity = 1.0\n\n";
    let image = format!("[[background.images]]\npath = '{BANDS_IMAGE}'\n");

    // どちらも項目の値: 敷き詰めて半透明。
    let server = start_with_background(
        &format!("{WHOLE}{image}fit = \"tile\"\nimage_opacity = 0.5\n"),
        |_| {},
    );
    let area = BackdropArea::of(&server.state());
    let (left, top) = (area.x + 2 * BANDS_SIDE, area.y + BANDS_SIDE);
    let half_left = over_terminal_background(MIDDLE_LEFT, 0.5);
    wait_pixels(
        &server,
        &[
            ((left + BANDS_SIDE / 4, top + BANDS_SIDE / 2), half_left),
            (
                (left + BANDS_SIDE + BANDS_SIDE / 4, top + BANDS_SIDE / 2),
                half_left,
            ),
        ],
    );
    drop(server);

    // 不透明度だけ項目の値: 全体の表示方法で半透明。
    let server = start_with_background(&format!("{WHOLE}{image}image_opacity = 0.5\n"), |_| {});
    let area = BackdropArea::of(&server.state());
    wait_pixels(
        &server,
        &[
            (area.at(0.25, 0.03), over_terminal_background(BAND_TOP, 0.5)),
            (area.at(0.25, 0.5), half_left),
        ],
    );
    drop(server);

    // 表示方法だけ項目の値: 全体の不透明度で敷き詰める。
    let server = start_with_background(&format!("{WHOLE}{image}fit = \"tile\"\n"), |_| {});
    let area = BackdropArea::of(&server.state());
    let (left, top) = (area.x + 2 * BANDS_SIDE, area.y + BANDS_SIDE);
    wait_pixels(
        &server,
        &[
            ((left + BANDS_SIDE / 4, top + BANDS_SIDE / 2), MIDDLE_LEFT),
            (
                (left + BANDS_SIDE + BANDS_SIDE / 4, top + BANDS_SIDE / 2),
                MIDDLE_LEFT,
            ),
        ],
    );
}

#[test]
fn the_background_image_follows_the_settings_file() {
    let server = start_with_background("", |_| {});
    let state = server.state();
    assert_eq!(state["background"]["image"], Value::Null, "{state:#}");
    let area = BackdropArea::of(&state);
    wait_pixels(&server, &[(area.at(0.25, 0.5), TERMINAL_BACKGROUND)]);

    let write = |background: String| {
        std::fs::write(
            server.dir().join("config.toml"),
            format!("{TERMINAL_BACKGROUND_SETTING}{background}"),
        )
        .expect("failed to write the settings");
    };
    write(format!(
        "[background]\nfit = \"stretch\"\nimage_opacity = 1.0\n\n[[background.images]]\npath = '{BANDS_IMAGE}'\n"
    ));
    wait_pixels(&server, &[(area.at(0.25, 0.5), MIDDLE_LEFT)]);

    write(format!(
        "[background]\nfit = \"fill\"\nimage_opacity = 0.5\n\n[[background.images]]\npath = '{BANDS_IMAGE}'\n"
    ));
    wait_pixels(
        &server,
        &[(
            area.at(0.25, 0.03),
            over_terminal_background(MIDDLE_LEFT, 0.5),
        )],
    );

    write(String::new());
    wait_pixels(&server, &[(area.at(0.25, 0.5), TERMINAL_BACKGROUND)]);
}

/// 読めない画像は外して別の候補を選び、全部読めなければ画像なしで動く。
#[test]
fn unreadable_background_images_are_skipped() {
    const BROKEN_IMAGE: &str = "broken.png";
    let write_broken = |dir: &std::path::Path| {
        std::fs::write(dir.join(BROKEN_IMAGE), b"not an image").expect("failed to write");
    };
    let entries = |paths: &[&str]| {
        let tables: Vec<String> = paths
            .iter()
            .map(|path| format!("[[background.images]]\npath = '{path}'\n"))
            .collect();
        format!(
            "[background]\nfit = \"stretch\"\nimage_opacity = 1.0\n\n{}",
            tables.join("")
        )
    };

    let server = start_with_background(
        &entries(&[BROKEN_IMAGE, "no-such-file.png", BANDS_IMAGE]),
        write_broken,
    );
    let state = server.state();
    assert_eq!(state["background"]["image"], BANDS_IMAGE, "{state:#}");
    let area = BackdropArea::of(&state);
    wait_pixels(&server, &[(area.at(0.25, 0.5), MIDDLE_LEFT)]);
    drop(server);

    let server = start_with_background(&entries(&[BROKEN_IMAGE, "no-such-file.png"]), write_broken);
    let state = server.state();
    assert_eq!(state["background"]["image"], Value::Null, "{state:#}");
    let area = BackdropArea::of(&state);
    wait_pixels(&server, &[(area.at(0.25, 0.5), TERMINAL_BACKGROUND)]);
}

/// タブごとにフォルダの中から選び、切り替えても選んだ画像のまま。選ぶのはランダムなので、タブの選んだ名前と絵を
/// 比べる。
#[test]
fn each_tab_keeps_the_background_image_it_chose() {
    const IMAGES: [(&str, [u8; 3]); 2] = [("red.png", [255, 0, 0]), ("blue.png", [0, 0, 255])];
    const TABS: usize = 4;
    let color_of = |state: &Value| {
        IMAGES
            .iter()
            .find(|(name, _)| state["background"]["image"] == *name)
            .unwrap_or_else(|| panic!("an unknown image: {state:#}"))
            .1
    };
    let server = start_with_background(
        "[background]\nfit = \"stretch\"\nimage_opacity = 1.0\n\n[[background.images]]\npath = 'images'\n",
        |dir| {
            let images = dir.join("images");
            std::fs::create_dir(&images).expect("failed to create the folder");
            for (name, color) in IMAGES {
                std::fs::write(images.join(name), solid_image(color))
                    .expect("failed to write the image");
            }
            // 拡張子が画像でないファイルは候補にしない。
            std::fs::write(images.join("notes.txt"), b"x").expect("failed to write");
        },
    );
    for count in 2..=TABS {
        server.post("/e2e/tab/new", b"");
        server.wait_until("a new tab", |state| tab_count(state) == count);
        server.wait_text(CMD_PROMPT);
    }
    for tab in 0..TABS {
        server.post(&format!("/e2e/tab/select/{tab}"), b"");
        let first = server.wait_until("the tab to be shown", |state| state["active"] == tab);
        let area = BackdropArea::of(&first);
        wait_pixels(&server, &[(area.at(0.25, 0.5), color_of(&first))]);
        server.post(&format!("/e2e/tab/select/{}", (tab + 1) % TABS), b"");
        server.post(&format!("/e2e/tab/select/{tab}"), b"");
        let again = server.wait_until("the tab to be shown", |state| state["active"] == tab);
        assert_eq!(again["background"]["image"], first["background"]["image"]);
        wait_pixels(&server, &[(area.at(0.25, 0.5), color_of(&again))]);
    }
}
