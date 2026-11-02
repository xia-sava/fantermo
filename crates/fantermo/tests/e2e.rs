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
//!
//! 設定ダイアログのテストのうち、OS のファイルを選ぶダイアログで実際に選ぶものは、環境変数 `FANTERMO_E2E_FILE_DIALOG` が
//! あるときだけ実行する（CI で付ける）。

mod common;

use std::{
    ffi::OsString,
    io::Cursor,
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use common::{
    LaunchAgain, OsClipboard, RestoreClipboard, Server,
    processes::{self, Process},
    release_host::{Gate, ReleaseHost, Reply, unused_port},
    signing::{SigningKey, der_signature, hex, sha256, sha256_hex},
};
use fantermo_core::{
    config::{
        self, BackendKind, BackgroundFit, BackgroundImage, ClipboardRead, ColorScheme, Config,
        PasteConfirm, WideGlyph,
    },
    frame::Rgb,
    keymap::{Action, KeyCombo, Keymap},
    state::{self, State, WindowState},
};
use serde_json::Value;
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    System::DataExchange::COPYDATASTRUCT,
    UI::WindowsAndMessaging::{SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_COPYDATA},
};

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
/// 一行で続けて起動する、同じコンソールの `ping` の数。
const CONSOLE_CLIENTS: u32 = 20;
/// 接続の途中で閉じるタブの数。
const TABS_CLOSED_WHILE_STARTING: usize = 6;
/// `start /b` で起動したプロセスができるのを待つ上限。
const CONSOLE_CLIENT_TIMEOUT: Duration = Duration::from_secs(60);
/// コンソールの外のプロセスができたかを確認する間隔。
const CONSOLE_CLIENT_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// どのコンソールにも接続せずに動き続ける、窓を出さないプログラム。
const GUI_PROCESS: &str = "wscript.exe";
/// 閉じたあと、起動の途中だった Cygwin の `sleep` が `/proc` に見えるようになるまで置く時間。
const CYGWIN_SLEEP_SETTLE: Duration = Duration::from_secs(2);

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
    // プロセスができたらすぐ閉じる。間を空けて見ると、接続を終えたあとに閉じることが多くなる。
    let second = wait_new_descendant(pid, CMD_LONG_PROCESS, &first);

    server.post("/e2e/tab/close", b"");
    server.wait_until("one tab", |state| tab_count(state) == 1);
    processes::wait_ended(&second);
    processes::assert_running(&first);
}

/// 疑似コンソールを閉じる前後に接続するプロセスができ続けるよう、同じコンソールの `ping` を続けて起動する行。
fn cmd_line_starting_console_clients() -> String {
    format!("for /l %i in (1,1,{CONSOLE_CLIENTS}) do @start /b {CMD_LONG_COMMAND} >NUL")
}

/// `name` のプロセスが `root` の子孫に新しくできるのを、できるだけ間を空けずに待つ。
#[track_caller]
fn wait_new_descendant(root: u32, name: &str, known: &[Process]) -> Vec<Process> {
    let deadline = Instant::now() + CONSOLE_CLIENT_TIMEOUT;
    loop {
        let found = processes::descendants(root);
        let new: Vec<Process> = found
            .into_iter()
            .filter(|process| !known.contains(process))
            .collect();
        if new.iter().any(|process| process.name == name) {
            return new;
        }
        assert!(Instant::now() < deadline, "no new {name} under {root}");
    }
}

/// 新しいタブを開いてシェルの準備ができるのを待ち、タブのプロセス（疑似コンソールの `conhost.exe` とシェル）を返す。
#[track_caller]
fn open_cmd_tab(server: &Server) -> Vec<Process> {
    let before = processes::descendants(server.pid());
    let tabs = tab_count(&server.state());
    server.post("/e2e/tab/new", b"");
    server.wait_until("a new tab", |state| tab_count(state) == tabs + 1);
    wait_cmd_ready(server);
    processes::descendants(server.pid())
        .into_iter()
        .filter(|process| !before.contains(process))
        .collect()
}

/// 接続の途中のプロセスがあるタブを閉じても、そのタブの疑似コンソールのプロセス（`conhost.exe`）まで終わる。
/// `conhost.exe` は接続しているプロセスがなくなるまで終わらないので、親を辿れなくなったものも含めて残っていないと分かる。
#[test]
fn closing_tabs_while_commands_start_ends_their_child_processes() {
    let server = Server::start_cmd();
    wait_cmd_ready(&server);
    let pid = server.pid();
    let first = processes::descendants(pid);

    let mut closed = Vec::new();
    for _ in 0..TABS_CLOSED_WHILE_STARTING {
        let tab = open_cmd_tab(&server);
        let known = processes::descendants(pid);
        server.send_line(&cmd_line_starting_console_clients());
        closed.extend(wait_new_descendant(pid, CMD_LONG_PROCESS, &known));
        closed.extend(tab);
        server.post("/e2e/tab/close", b"");
        server.wait_until("one tab", |state| tab_count(state) == 1);
    }

    processes::wait_ended(&closed);
    processes::assert_running(&first);
}

/// 接続の途中のプロセスがあるままウィンドウを閉じても、子孫のプロセスは残らない。
#[test]
fn closing_the_window_while_commands_start_leaves_no_child_processes() {
    let mut server = Server::start_watching_children("", &CMD_COMMAND);
    wait_cmd_ready(&server);
    open_cmd_tab(&server);
    let known = processes::descendants(server.pid());
    server.send_line(&cmd_line_starting_console_clients());
    server.post("/e2e/tab/select/0", b"");
    server.send_line(&cmd_line_starting_console_clients());
    wait_new_descendant(server.pid(), CMD_LONG_PROCESS, &known);
    server.post("/e2e/quit", b"");
    assert_eq!(server.wait_exit(), 0);
    server.assert_no_children_remained();
}

/// `root` の子孫に新しくできる `ping.exe` と窓のないプログラムを待って返す。
#[track_caller]
fn wait_descendants_outside_the_console(root: u32, known: &[Process]) -> Vec<Process> {
    let deadline = Instant::now() + CONSOLE_CLIENT_TIMEOUT;
    loop {
        let found: Vec<Process> = processes::descendants(root)
            .into_iter()
            .filter(|process| !known.contains(process))
            .filter(|process| process.name == CMD_LONG_PROCESS || process.name == GUI_PROCESS)
            .collect();
        if found.iter().any(|process| process.name == GUI_PROCESS)
            && found.iter().any(|process| process.name == CMD_LONG_PROCESS)
        {
            return found;
        }
        assert!(Instant::now() < deadline, "under {root}: {found:#?}");
        thread::sleep(CONSOLE_CLIENT_POLL_INTERVAL);
    }
}

/// 別のコンソールを持つプロセスと、どのコンソールにも接続しないプロセスは、接続の途中のプロセスがあったタブを
/// 閉じても動き続ける。
#[test]
fn closing_a_tab_while_commands_start_leaves_processes_outside_its_console() {
    let server = Server::start_cmd();
    wait_cmd_ready(&server);
    let pid = server.pid();
    let script = server.dir().join("sleep.vbs");
    std::fs::write(&script, "WScript.Sleep 300000\n").expect("failed to write the script");
    let tab = open_cmd_tab(&server);
    let known = processes::descendants(pid);
    // 親の PowerShell を動かしたままにして、親子で辿れるようにする。窓のあるコンソールは、別デスクトップで起動しても
    // 既定の端末へ渡って利用者の画面に出ることがあるので、窓のないコンソール（`CreateNoWindow`）で起動する。
    server.send_line(&format!(
        "start /b powershell.exe -NoProfile -Command \"$i = New-Object Diagnostics.ProcessStartInfo 'ping.exe', '-t 127.0.0.1'; $i.UseShellExecute = $false; $i.CreateNoWindow = $true; [void][Diagnostics.Process]::Start($i); Start-Sleep 300\" & start \"\" {GUI_PROCESS} //B \"{}\"",
        script.display()
    ));
    let detached = wait_descendants_outside_the_console(pid, &known);
    assert_eq!(detached.len(), 2, "{detached:#?}");

    let known = processes::descendants(pid);
    server.send_line(&cmd_line_starting_console_clients());
    wait_new_descendant(pid, CMD_LONG_PROCESS, &known);
    server.post("/e2e/tab/close", b"");
    server.wait_until("one tab", |state| tab_count(state) == 1);

    processes::wait_ended(&tab);
    processes::assert_running(&detached);
}

/// `sleep` を打った直後に閉じるときの、打ってから閉じるまでの時間（ミリ秒）。シェルが SIGHUP を受け取るのがコマンドの
/// 起動の途中になるよう、タブごとに変える。
const CYGWIN_CLOSE_DELAYS_MS: [u64; 6] = [10, 20, 30, 40, 50, 70];

/// 動き出すのを待たずにタブを閉じても、コマンドは残らない。
#[test]
fn cygwin_closing_tabs_right_after_typing_a_command_ends_it() {
    let Some(config) = cygwin_config("cygwin_closing_tabs_right_after_typing") else {
        return;
    };
    let server = Server::start(&config, &[]);
    wait_sh_ready(&server);
    let mut markers = Vec::new();
    for delay in CYGWIN_CLOSE_DELAYS_MS {
        server.post("/e2e/tab/new", b"");
        server.wait_until("two tabs", |state| tab_count(state) == 2);
        wait_sh_ready(&server);
        let marker = unique_sleep_seconds();
        server.send_line(&format!("sleep {marker}"));
        thread::sleep(Duration::from_millis(delay));
        server.post("/e2e/tab/close", b"");
        server.wait_until("one tab", |state| tab_count(state) == 1);
        markers.push(marker);
    }
    // 起動の途中だった `sleep` が `/proc` に見えるようになるまで待ってから見る。
    thread::sleep(CYGWIN_SLEEP_SETTLE);
    for marker in &markers {
        wait_cygwin_sleep(marker, false);
    }
}

/// 動き出すのを待たずにウィンドウを閉じても、コマンドは残らない。ブリッジの標準入力と標準エラー出力が同時に閉じる。
#[test]
fn cygwin_closing_the_window_right_after_typing_a_command_ends_it() {
    let Some(config) = cygwin_config("cygwin_closing_the_window_right_after_typing") else {
        return;
    };
    for delay in CYGWIN_CLOSE_DELAYS_MS {
        let mut server = Server::start_watching_children(&config, &[]);
        wait_sh_ready(&server);
        server.send_line(&format!("sleep {}", unique_sleep_seconds()));
        thread::sleep(Duration::from_millis(delay));
        server.post("/e2e/quit", b"");
        assert_eq!(server.wait_exit(), 0);
        server.assert_no_children_remained();
    }
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

/// セルで割り切れない大きさのウィンドウ（枠を含むピクセル）。
const UNEVEN_WINDOW: WindowState = WindowState {
    left: 10,
    top: 20,
    width: 411,
    height: 291,
    maximized: false,
};

/// 設定 `config` と `state.toml` の [`UNEVEN_WINDOW`] で `command` を起動し、プロンプト `prompt` が出るのを待つ。端末を
/// 描く領域がセルで割り切れないことを確認する。
fn start_uneven(config: &str, command: &[&str], prompt: &str) -> (Server, Value) {
    let saved = state::to_toml(&seeded_state(UNEVEN_WINDOW));
    let server = Server::start_with_setup(config, command, |dir| {
        std::fs::write(dir.join(common::STATE_FILE), &saved).expect("failed to write the state");
    });
    let state = server.wait_text(prompt);
    assert_eq!(current_window(&state), UNEVEN_WINDOW, "{state:#}");
    let (width, height) = terminal_pixels(&state);
    assert!(
        width % number(&state["cell_width"]) != 0 || height % number(&state["cell_height"]) != 0,
        "the window fits the cells: {state:#}"
    );
    (server, state)
}

/// 端末を描く領域の大きさ（ピクセル）。クライアント領域からタブバー・スクロールバー・余白を除いたもの。
fn terminal_pixels(state: &Value) -> (u64, u64) {
    let padding = number(&state["terminal_x"]);
    (
        number(&state["client_width"]) - number(&state["scroll_bar"]["width"]) - 2 * padding,
        number(&state["client_height"]) - number(&state["background"]["y"]) - 2 * padding,
    )
}

/// 桁数と行数が、いまのクライアント領域の端末を描く領域にセルで入る数で、タブバー・スクロールバー・背景画像の範囲が
/// クライアント領域に合っていることを確認する。どのタブも同じ桁数と行数。
#[track_caller]
fn assert_layout_fits_the_client(state: &Value) {
    let padding = number(&state["terminal_x"]);
    let tab_bar = number(&state["background"]["y"]);
    let bar_width = number(&state["scroll_bar"]["width"]);
    let client_width = number(&state["client_width"]);
    let client_height = number(&state["client_height"]);
    let (width, height) = terminal_pixels(state);
    assert_eq!(
        number(&state["cols"]),
        width / number(&state["cell_width"]),
        "{state:#}"
    );
    assert_eq!(
        number(&state["rows"]),
        height / number(&state["cell_height"]),
        "{state:#}"
    );
    for tab in state["tabs"].as_array().expect("no tabs") {
        assert_eq!(tab["cols"], state["cols"], "{state:#}");
        assert_eq!(tab["rows"], state["rows"], "{state:#}");
    }
    assert_eq!(number(&state["terminal_y"]), tab_bar + padding, "{state:#}");
    assert_eq!(
        number(&state["scroll_bar"]["x"]),
        client_width - bar_width,
        "{state:#}"
    );
    assert_eq!(
        number(&state["scroll_bar"]["y"]),
        tab_bar + padding,
        "{state:#}"
    );
    assert_eq!(number(&state["scroll_bar"]["height"]), height, "{state:#}");
    assert_eq!(number(&state["background"]["x"]), 0, "{state:#}");
    assert_eq!(
        number(&state["background"]["width"]),
        client_width - bar_width,
        "{state:#}"
    );
    assert_eq!(
        number(&state["background"]["height"]),
        client_height - tab_bar,
        "{state:#}"
    );
}

/// PowerShell に端末の大きさを `tag` を添えて出させ、`state` の桁数と行数が届いていることを確認する。
#[track_caller]
fn assert_the_shell_sees_the_grid(server: &Server, state: &Value, tag: &str) {
    server.type_line(&format!(
        "\"{tag}-[$($Host.UI.RawUI.WindowSize.Width)x$($Host.UI.RawUI.WindowSize.Height)]\""
    ));
    server.wait_text(&format!(
        "{tag}-[{}x{}]",
        number(&state["cols"]),
        number(&state["rows"])
    ));
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

/// ログオフの問い合わせ（`WM_QUERYENDSESSION`）と、その結果の知らせ（`WM_ENDSESSION`）の名前。
const QUERY_END_SESSION: &str = "queryendsession";
const END_SESSION: &str = "endsession";
/// `WM_ENDSESSION` の `wParam`。
const SESSION_CONTINUES: usize = 0;
const SESSION_ENDS: usize = 1;
/// `WM_QUERYENDSESSION` で、終わってよいと答えるときの戻り値。
const SESSION_MAY_END: i64 = 1;
/// OS が終わらせたことにするときの終了コード。
const SESSION_END_EXIT_CODE: u32 = 41;

#[track_caller]
fn remove_state_file(server: &Server) {
    std::fs::remove_file(server.state_file()).expect("failed to remove the state");
}

/// OS がセッションを終えるときと同じく、`WM_ENDSESSION` のあとでプロセスごと終わらせて、終わるのを待つ。
#[track_caller]
fn terminate_like_the_session_end(server: &mut Server) {
    processes::terminate(server.pid(), SESSION_END_EXIT_CODE);
    assert_eq!(server.wait_exit(), SESSION_END_EXIT_CODE as i32);
}

fn check_cmd_responds(server: &Server) {
    server.type_line("echo alive-%ERRORLEVEL%-shell");
    server.wait_text("alive-0-shell");
}

fn check_sh_responds(server: &Server) {
    server.type_line("echo alive-$((1+1))");
    server.wait_text("alive-2");
}

/// 長く動くコマンドを動かしているタブ 2 つと、何も動かしていないタブ 1 つで、ログオフの問い合わせ・取りやめ・
/// 実行の順にメッセージを受ける。問われた時点と実行のときに状態を書き、取りやめでは書かない。その間ウィンドウと
/// シェルは動き続け、プロセスごと終わらされたあとに子孫のプロセスが残らない。
fn exercise_session_end(
    mut server: Server,
    long_commands: [&str; 2],
    wait_long_commands: &dyn Fn(&Server),
    wait_ready: fn(&Server),
    check_responds: fn(&Server),
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
    assert!(!server.state_file().exists());

    server.resize(87, 23);
    let asked = current_window(&server.state());
    assert_eq!(
        server.send_message(QUERY_END_SESSION, 0),
        SESSION_MAY_END,
        "{}",
        server.log()
    );
    assert_eq!(saved_window(&server), asked);

    remove_state_file(&server);
    assert_eq!(server.send_message(END_SESSION, SESSION_CONTINUES), 0);
    assert!(
        !server.state_file().exists(),
        "the state was written though the session continues"
    );
    check_responds(&server);

    server.resize(90, 25);
    let ending = current_window(&server.state());
    assert_ne!(ending, asked);
    assert_eq!(server.send_message(END_SESSION, SESSION_ENDS), 0);
    assert_eq!(saved_window(&server), ending);

    // ウィンドウは閉じず、端末も閉じない。
    let state = server.state();
    assert_eq!(tab_count(&state), 3, "{state:#}");
    assert_eq!(state["dialogs"].as_array().map(Vec::len), Some(0));
    check_responds(&server);

    terminate_like_the_session_end(&mut server);
    assert_eq!(saved_window(&server), ending);
    server.assert_no_children_remained();
}

#[test]
fn the_session_end_saves_the_state_and_leaves_no_child_processes() {
    let server = Server::start_watching_children("", &CMD_COMMAND);
    let wait_pings = |server: &Server| {
        processes::wait_descendants_named(server.pid(), CMD_LONG_PROCESS, 2);
    };
    exercise_session_end(
        server,
        [CMD_LONG_COMMAND, CMD_LONG_COMMAND],
        &wait_pings,
        wait_cmd_ready,
        check_cmd_responds,
    );
}

#[test]
fn cygwin_the_session_end_saves_the_state_and_leaves_no_child_processes() {
    let Some(config) = cygwin_config("cygwin_the_session_end") else {
        return;
    };
    let server = Server::start_watching_children(&config, &[]);
    let (first, second) = (unique_sleep_seconds(), unique_sleep_seconds());
    let wait_sleeps = |_: &Server| {
        wait_cygwin_sleep(&first, true);
        wait_cygwin_sleep(&second, true);
    };
    exercise_session_end(
        server,
        [&format!("sleep {first}"), &format!("sleep {second}")],
        &wait_sleeps,
        wait_sh_ready,
        check_sh_responds,
    );
    wait_cygwin_sleep(&first, false);
    wait_cygwin_sleep(&second, false);
}

/// WSL の中のプロセスは見えないので、Windows の側の `wsl.exe` までを見る。
#[test]
fn wsl_the_session_end_saves_the_state_and_leaves_no_child_processes() {
    let Some(config) = wsl_config("wsl_the_session_end") else {
        return;
    };
    let server = Server::start_watching_children(&config, &[]);
    exercise_session_end(
        server,
        [SH_LONG_COMMAND, SH_LONG_COMMAND],
        &|_| {},
        wait_sh_ready,
        check_sh_responds,
    );
}

/// ほかのアプリが止めて取りやめになったら、普段どおりに動き、閉じれば子孫のプロセスは残らない。
#[test]
fn after_a_cancelled_session_end_the_window_still_closes_normally() {
    let mut server = Server::start_watching_children("", &CMD_COMMAND);
    wait_cmd_ready(&server);
    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    wait_cmd_ready(&server);
    server.send_line(CMD_LONG_COMMAND);
    processes::wait_descendants_named(server.pid(), CMD_LONG_PROCESS, 1);
    server.post("/e2e/tab/select/0", b"");

    assert_eq!(server.send_message(QUERY_END_SESSION, 0), SESSION_MAY_END);
    assert_eq!(server.send_message(END_SESSION, SESSION_CONTINUES), 0);
    check_cmd_responds(&server);

    server.resize(87, 23);
    let window = current_window(&server.state());
    quit(&mut server);
    assert_eq!(saved_window(&server), window);
    server.assert_no_children_remained();
}

/// `WM_ENDSESSION` で書いた状態で、次の起動の大きさが戻る。
#[test]
fn the_state_saved_at_the_session_end_comes_back_after_restarting() {
    let mut first = Server::start("", &CMD_COMMAND);
    first.wait_text(CMD_PROMPT);
    first.resize(87, 23);
    let window = current_window(&first.state());
    assert_eq!(first.send_message(END_SESSION, SESSION_ENDS), 0);
    terminate_like_the_session_end(&mut first);
    let saved = std::fs::read(first.state_file()).expect("failed to read the state");
    drop(first);

    let (_second, state) = start_with_state(&saved);
    assert_eq!(state["cols"], 87, "{state:#}");
    assert_eq!(state["rows"], 23, "{state:#}");
    assert_eq!(current_window(&state), window);
}

/// ダイアログを開いたままでも終わるのを止めず、状態を書く。ダイアログは閉じない。
#[test]
fn the_session_end_saves_the_state_while_a_dialog_is_open() {
    let server = Server::start("[paste]\nconfirm = \"multiline\"\n", &CMD_COMMAND);
    server.wait_text(CMD_PROMPT);
    server.paste("echo multi-%ERRORLEVEL%-line\n");
    wait_text_dialog(&server);
    let window = current_window(&server.state());

    assert_eq!(server.send_message(QUERY_END_SESSION, 0), SESSION_MAY_END);
    assert_eq!(saved_window(&server), window);
    remove_state_file(&server);
    assert_eq!(server.send_message(END_SESSION, SESSION_ENDS), 0);
    assert_eq!(saved_window(&server), window);
    wait_text_dialog(&server);
}

/// クライアント領域の中の位置。ウィンドウの外を指す負の値も持てる。
type Point = (i64, i64);

/// メニューのウィンドウのクラス。
const MENU_CLASS: &str = "#32768";

/// ホイールの 1 ノッチの回転量。
const NOTCH: i32 = 120;
/// 表示している範囲のどこからでもいちばん古い行に届く、ノッチの数。
const NOTCHES_TO_THE_OLDEST: i32 = 200;
/// 文字のない桁。テストの行はここまで届かない。
const EMPTY_COL: u32 = 40;
/// 文字のない行が続く位置を選ぶときの、行からの距離。
const EMPTY_ROWS_BELOW: u32 = 3;
/// ホイールを回すセルの桁と行。
const WHEEL_COL: u32 = 5;
const WHEEL_ROW: u32 = 5;
/// スクロールバーをドラッグするピクセル数。
const DRAG_PIXELS: i64 = 100;
/// バーの外へ出すときの、横のピクセル数。
const BESIDE_THE_BAR: i64 = 200;
/// ウィンドウの上の外の高さ。
const ABOVE_THE_WINDOW: i64 = -100;
/// ウィンドウの下の外の高さ。
const BELOW_THE_WINDOW: i64 = 10_000;
/// タブの左端からタブの本体の位置までのピクセル数。閉じるボタンより左。
const TAB_BODY_OFFSET: i64 = 8;
/// つまみがマウスの位置で止まったあと、繰り返しの間隔の数回分として見る長さ。
const REPEAT_STOPPED_CHECK: Duration = Duration::from_millis(300);

/// `(col, row)` のセルの真ん中。
fn cell_center(state: &Value, col: u32, row: u32) -> Point {
    let half = |size: &str| u32::try_from(number(&state[size]) / 2).expect("the size fits");
    let (x, y) = cell_pixel(state, col, row, half("cell_width"), half("cell_height"));
    (i64::from(x), i64::from(y))
}

/// `(col, row)` のセルの左端の、高さの真ん中。セルの半分を越えないので、ここから選び始めるとこのセルを含み、ここまで
/// 選ぶとこのセルを含まない。
fn cell_left(state: &Value, col: u32, row: u32) -> Point {
    let (x, y) = cell_center(state, col, row);
    (
        x - i64::try_from(number(&state["cell_width"]) / 2).expect("the size fits") + 1,
        y,
    )
}

/// `(col, row)` のセルの右端の、高さの真ん中。セルの半分を越えているので、ここまで選ぶとこのセルを含む。
fn cell_right(state: &Value, col: u32, row: u32) -> Point {
    let (x, y) = cell_left(state, col, row);
    (
        x + i64::try_from(number(&state["cell_width"])).expect("the size fits") - 2,
        y,
    )
}

/// 表示している範囲の中で `needle` が始まるセル。全体の文字の行と、表示している範囲の行は `scroll` の `offset` だけ
/// ずれる。`needle` が折り返さない長さの行にあるときに使う。
#[track_caller]
fn find_cell(state: &Value, needle: &str) -> (u32, u32) {
    let offset = number(&state["scroll"]["offset"]);
    let text = state["text"].as_str().unwrap_or_default();
    for (row, line) in text.lines().enumerate() {
        if let Some(col) = line.find(needle) {
            let row = u64::try_from(row).expect("the row fits") - offset;
            return (
                u32::try_from(col).expect("the column fits"),
                u32::try_from(row).expect("the row fits"),
            );
        }
    }
    panic!("{needle:?} is not in the text: {state:#}");
}

fn selection(state: &Value) -> Option<&str> {
    state["selection"].as_str()
}

fn step(action: &str, (x, y): Point, held: &str) -> String {
    format!("{action} {x} {y} {held}\n")
}

/// 左ボタンで `at` を一度クリックする。
fn left_click(at: Point) -> String {
    format!(
        "{}{}{}",
        step("move", at, ""),
        step("left-down", at, ""),
        step("left-up", at, "")
    )
}

/// `from` で左ボタンを押して `to` まで動かす。離さない。`held` は押している修飾キー。
fn left_press_and_move(from: Point, to: Point, held: &str) -> String {
    format!(
        "{}{}{}",
        step("move", from, held),
        step("left-down", from, held),
        step("move", to, &format!("left {held}"))
    )
}

/// `from` から `to` まで左ボタンでドラッグして離す。`held` は押している修飾キー。
fn left_drag(from: Point, to: Point, held: &str) -> String {
    format!(
        "{}{}",
        left_press_and_move(from, to, held),
        step("left-up", to, held)
    )
}

/// `at` で `button`（`left` など）を押して離す。`held` は押している修飾キー。
fn button_click(button: &str, at: Point, held: &str) -> String {
    format!(
        "{}{}{}",
        step("move", at, held),
        step(&format!("{button}-down"), at, held),
        step(&format!("{button}-up"), at, held)
    )
}

/// `at` でホイールを `delta` 回す。
fn wheel(delta: i32, at: Point) -> String {
    step(&format!("wheel:{delta}"), at, "")
}

/// 打った行には出ない 2 行を出し、その全体の状態を返す。行は `first-0-end`、`second-0-end` の形にしたものを待つ。
fn echo_lines(server: &Server, first: &str, second: &str) -> Value {
    server.type_line(&format!(
        "echo {first}-%ERRORLEVEL%-end&echo {second}-%ERRORLEVEL%-end"
    ));
    server.wait_text(&format!("{second}-0-end"))
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

fn wait_menu(server: &Server) {
    server.wait_until("a menu", |state| {
        state["dialogs"]
            .as_array()
            .is_some_and(|dialogs| dialogs.iter().any(|dialog| dialog["class"] == MENU_CLASS))
    });
}

/// 開いているメニューを閉じて、閉じたのを確認する。
fn close_menu(server: &Server) {
    server.post("/e2e/menu/close", b"");
    server.wait_dialogs(0);
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

/// 画面に収まらない行を出す。全体の文字に行が出たあとの状態を返す。
fn fill_scrollback(server: &Server) -> Value {
    server.type_line("for /L %i in (1,1,200) do @echo scroll-line-%i");
    server.wait_text("scroll-line-200");
    server.type_line("echo filled-%ERRORLEVEL%-end");
    // 次のプロンプトまで待つ。全体の行数はそこまで増える。
    let state = server.wait_until("the prompt after the output", |state| {
        let text = state["text"].as_str().unwrap_or_default();
        text.contains("filled-0-end") && text.trim_end().ends_with(CMD_PROMPT)
    });
    assert!(scroll_offset(&state) > 0, "{state:#}");
    assert_eq!(scroll_offset(&state), newest_offset(&state), "{state:#}");
    state
}

fn scroll_offset(state: &Value) -> u64 {
    number(&state["scroll"]["offset"])
}

/// いちばん新しい行を表示しているときの `offset`。
fn newest_offset(state: &Value) -> u64 {
    number(&state["scroll"]["total"]) - number(&state["scroll"]["len"])
}

fn thumb_top(state: &Value) -> u64 {
    number(&state["scroll_bar"]["thumb"]["top"])
}

fn thumb_length(state: &Value) -> u64 {
    number(&state["scroll_bar"]["thumb"]["length"])
}

/// スクロールバーの横の真ん中の、高さ `y` の位置。
fn on_scroll_bar(state: &Value, y: u64) -> Point {
    let bar = &state["scroll_bar"];
    (
        i64::try_from(number(&bar["x"]) + number(&bar["width"]) / 2).expect("the position fits"),
        i64::try_from(y).expect("the position fits"),
    )
}

/// スクロールバーの縦の真ん中の高さ。
fn scroll_bar_middle(state: &Value) -> u64 {
    number(&state["scroll_bar"]["y"]) + number(&state["scroll_bar"]["height"]) / 2
}

/// つまみの真ん中。
fn on_thumb(state: &Value) -> Point {
    on_scroll_bar(state, thumb_top(state) + thumb_length(state) / 2)
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

fn area_center(area: &Value) -> Point {
    (
        i64::try_from(number(&area["x"]) + number(&area["width"]) / 2).expect("the position fits"),
        i64::try_from(number(&area["y"]) + number(&area["height"]) / 2).expect("the position fits"),
    )
}

/// `state` のタブバーの `index` 番目のタブの本体（閉じるボタンの外）の位置。
fn on_tab(state: &Value, index: usize) -> Point {
    let area = &state["tab_bar"]["tabs"][index]["area"];
    (
        i64::try_from(number(&area["x"])).expect("the position fits") + TAB_BODY_OFFSET,
        area_center(area).1,
    )
}

fn on_close_button(state: &Value, index: usize) -> Point {
    area_center(&state["tab_bar"]["tabs"][index]["close"])
}

/// タブが `count` 個になり、表示しているタブのシェルが動くまで待つ。
#[track_caller]
fn wait_tabs(server: &Server, count: usize) -> Value {
    server.wait_until(&format!("{count} tabs"), |state| tab_count(state) == count);
    server.wait_text(CMD_PROMPT)
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

/// コンテキストメニューの並び。形は [`menu_shape`]。
const CONTEXT_MENU: &str = "C- P | A | T W";
const CONTEXT_MENU_WITH_SELECTION: &str = "C P | A | T W";
/// ≡ のメニューの並び。
const MAIN_MENU: &str = "T W | C- P A | S F B | X";
const MAIN_MENU_WITH_SELECTION: &str = "T W | C P A | S F B | X";
/// 矢印のキーで項目へ移るのを待つ上限。
const MENU_TIMEOUT: Duration = Duration::from_secs(30);
/// 項目を読み直す間隔。
const MENU_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// 設定ダイアログのウィンドウのクラス。
const SETTINGS_DIALOG_CLASS: &str = "fantermo-settings-dialog";
/// `IDCANCEL`。設定ダイアログの「キャンセル」と、「fantermo について」の「閉じる」。
const DIALOG_CLOSE_BUTTON: i32 = 2;
/// タブバーの右の、何もないところを選ぶときの、プロファイルの一覧のボタンからの距離。
const BESIDE_THE_TAB_BAR_BUTTONS: i64 = 40;
/// 文字のあるセルで、背景と違う色の画素を探すときの差。
const INK_DIFFERENCE: u8 = 40;

/// 開いているメニューの項目を読む。
#[track_caller]
fn read_menu(server: &Server) -> Vec<Value> {
    let (status, body) = server.request("GET", "/e2e/menu", b"");
    let text = String::from_utf8_lossy(&body);
    assert_eq!(status, 200, "{text}\nstate: {:#}", server.state());
    let menu: Value = serde_json::from_slice(&body).expect("the menu is not JSON");
    menu["items"].as_array().expect("no menu items").clone()
}

/// 項目のアクセラレータの英数字（大文字）。なければ `None`。`&&` は `&` そのもの。
fn accelerator(item: &Value) -> Option<char> {
    let text = item["text"].as_str().unwrap_or_default();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '&' {
            match chars.next() {
                Some('&') => {}
                other => return other.map(|c| c.to_ascii_uppercase()),
            }
        }
    }
    None
}

/// メニューの並びの短い形。項目はアクセラレータ（なければ `.`）で、押せなければ `-`、印があれば `*` を続け、区切りは
/// `|`。項目の文字の言い回しによらず、順番と押せるかを比べる。
fn menu_shape(items: &[Value]) -> String {
    let shapes: Vec<String> = items
        .iter()
        .map(|item| {
            if item["separator"] == true {
                return "|".to_owned();
            }
            let mut shape = accelerator(item).unwrap_or('.').to_string();
            if item["enabled"] != true {
                shape.push('-');
            }
            if item["checked"] == true {
                shape.push('*');
            }
            shape
        })
        .collect();
    shapes.join(" ")
}

/// 項目の表示の、キーの組み合わせの前の部分から、アクセラレータの印を除いたもの。
fn item_name(item: &Value) -> String {
    let text = item["text"].as_str().unwrap_or_default();
    let name = text.split('\t').next().unwrap_or_default();
    let mut plain = String::new();
    let mut chars = name.chars();
    while let Some(c) = chars.next() {
        if c == '&' {
            plain.extend(chars.next());
        } else {
            plain.push(c);
        }
    }
    plain
}

/// 項目に添えたキーの組み合わせ。
fn item_shortcut(item: &Value) -> Option<&str> {
    item["text"]
        .as_str()?
        .split_once('\t')
        .map(|(_, keys)| keys)
}

/// `steps` を送ってメニューが開くのを待ち、項目を返す。
#[track_caller]
fn open_menu(server: &Server, steps: &str) -> Vec<Value> {
    server.mouse(steps);
    wait_menu(server);
    read_menu(server)
}

/// 矢印のキーで `position` の項目へ移り、移ったのを待つ。移れなければ、そのときの項目と状態を出して失敗にする。
#[track_caller]
fn highlight_position(server: &Server, position: usize) {
    server.post(&format!("/e2e/menu/highlight/{position}"), b"");
    let deadline = Instant::now() + MENU_TIMEOUT;
    loop {
        let items = read_menu(server);
        if items[position]["highlighted"] == true {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the arrow keys did not reach the item at {position}: {items:#?}\nstate: {:#}\n{}",
            server.state(),
            server.log()
        );
        std::thread::sleep(MENU_POLL_INTERVAL);
    }
}

/// 矢印のキーで `position` の項目へ移り、Enter で選ぶ。
#[track_caller]
fn select_position(server: &Server, position: usize) {
    highlight_position(server, position);
    server.post(&format!("/e2e/menu/select/{position}"), b"");
}

/// `accelerator_of` のアクセラレータの項目の位置。
#[track_caller]
fn menu_position(items: &[Value], accelerator_of: char) -> usize {
    items
        .iter()
        .position(|item| accelerator(item) == Some(accelerator_of))
        .unwrap_or_else(|| panic!("no item {accelerator_of}: {items:#?}"))
}

/// `accelerator_of` のアクセラレータの項目。
#[track_caller]
fn menu_item(items: &[Value], accelerator_of: char) -> &Value {
    &items[menu_position(items, accelerator_of)]
}

/// `accelerator_of` のアクセラレータの項目を選ぶ。
#[track_caller]
fn select_item(server: &Server, items: &[Value], accelerator_of: char) {
    select_position(server, menu_position(items, accelerator_of));
}

/// メニューのループへキーを送る。
fn press_menu_key(server: &Server, key: &str) {
    server.post(&format!("/e2e/menu/key/{key}"), b"");
}

fn on_terminal(state: &Value) -> Point {
    cell_center(state, EMPTY_COL, EMPTY_ROWS_BELOW)
}

/// ≡ を押すマウスの操作。
fn click_main_menu(state: &Value) -> String {
    left_click(area_center(&state["tab_bar"]["menu"]))
}

/// 端末の上で右クリックするマウスの操作。
fn right_click_terminal(state: &Value) -> String {
    button_click("right", on_terminal(state), "")
}

/// 開いているダイアログが `class` の一つだけになるのを待ち、そのダイアログを返す。
#[track_caller]
fn wait_dialog_of_class(server: &Server, class: &str) -> Value {
    let state = server.wait_until(&format!("a dialog of {class}"), |state| {
        state["dialogs"]
            .as_array()
            .is_some_and(|dialogs| dialogs.len() == 1 && dialogs[0]["class"] == class)
    });
    state["dialogs"][0].clone()
}

/// シェルが入力を受け取って動き、画面に文字を描くことを確認する。`tag` は打つごとに変える。
#[track_caller]
fn check_terminal_still_works(server: &Server, tag: &str) {
    server.type_line(&format!("echo {tag}-%ERRORLEVEL%-end"));
    let needle = format!("{tag}-0-end");
    let state = server.wait_until(&format!("the text {needle:?}"), |state| {
        let text = state["text"].as_str().unwrap_or_default();
        text.contains(&needle) && text.trim_end().ends_with(CMD_PROMPT)
    });
    let (col, row) = find_cell(&state, &needle);
    assert_cell_is_drawn(server, &state, col, row);
}

/// `(col, row)` のセルに、背景と違う色の画素があることを確認する。
#[track_caller]
fn assert_cell_is_drawn(server: &Server, state: &Value, col: u32, row: u32) {
    let picture = server.picture();
    let size = |name: &str| u32::try_from(number(&state[name])).expect("the size fits");
    let (left, top) = cell_pixel(state, col, row, 0, 0);
    let background = picture.rgb(left, top);
    let drawn = (0..size("cell_height")).any(|dy| {
        (0..size("cell_width")).any(|dx| {
            let color = picture.rgb(left + dx, top + dy);
            color
                .iter()
                .zip(background)
                .any(|(&a, b)| a.abs_diff(b) > INK_DIFFERENCE)
        })
    });
    assert!(drawn, "the cell ({col}, {row}) is blank: {state:#}");
}

/// どの開き方でもコンテキストメニューの並びが同じで、コピーだけが選択範囲に従う。スクロールバーの上の右クリックでは
/// 開かない。
#[test]
fn the_context_menu_has_the_same_items_wherever_it_opens_and_copy_follows_the_selection() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = echo_lines(&server, "alpha", "other");
    let blank_of_tab_bar = {
        let (x, y) = area_center(&state["tab_bar"]["profiles"]);
        (x + BESIDE_THE_TAB_BAR_BUTTONS, y)
    };
    let check_every_way_to_open = |expected: &str| {
        for steps in [
            right_click_terminal(&state),
            button_click("right", on_tab(&state, 0), ""),
            button_click("right", blank_of_tab_bar, ""),
        ] {
            let items = open_menu(&server, &steps);
            assert_eq!(menu_shape(&items), expected, "{steps}: {items:#?}");
            close_menu(&server);
        }
        for key in ["context_menu", "shift+f10"] {
            server.press(key);
            wait_menu(&server);
            let items = read_menu(&server);
            assert_eq!(menu_shape(&items), expected, "{key}: {items:#?}");
            close_menu(&server);
        }
    };
    check_every_way_to_open(CONTEXT_MENU);

    let (col, row) = find_cell(&state, "alpha-0-end");
    server.mouse(&left_drag(
        cell_left(&state, col, row),
        cell_right(&state, col + 4, row),
        "",
    ));
    check_every_way_to_open(CONTEXT_MENU_WITH_SELECTION);
    assert_eq!(selection(&server.state()), Some("alpha"));

    server.mouse(&button_click(
        "right",
        on_scroll_bar(&state, scroll_bar_middle(&state)),
        "",
    ));
    let after = server.state();
    assert_eq!(
        after["dialogs"].as_array().map(Vec::len),
        Some(0),
        "{after:#}"
    );
}

/// メニューの要求は、閉じているときと項目でない位置を断る。断っても開いたまま。
#[test]
fn menu_requests_are_refused_without_a_menu_or_an_item() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);
    for (method, path) in [
        ("GET", "/e2e/menu"),
        ("POST", "/e2e/menu/highlight/0"),
        ("POST", "/e2e/menu/select/0"),
        ("POST", "/e2e/menu/key/escape"),
        ("POST", "/e2e/menu/close"),
    ] {
        let (status, _) = server.request(method, path, b"");
        assert_eq!(status, 400, "{path}");
    }

    let items = open_menu(&server, &click_main_menu(&state));
    let separator = items
        .iter()
        .position(|item| item["separator"] == true)
        .expect("no separator");
    for position in [separator, items.len()] {
        for action in ["highlight", "select"] {
            let path = format!("/e2e/menu/{action}/{position}");
            let (status, _) = server.request("POST", &path, b"");
            assert_eq!(status, 400, "{path}");
        }
    }
    wait_menu(&server);
    close_menu(&server);
}

/// ≡ のメニューの並び。項目に添えるキーは割り当てから決まる。
#[test]
fn the_main_menu_has_its_items_in_order_and_copy_follows_the_selection() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = echo_lines(&server, "alpha", "other");

    let items = open_menu(&server, &click_main_menu(&state));
    assert_eq!(menu_shape(&items), MAIN_MENU, "{items:#?}");
    assert!(
        items
            .iter()
            .all(|item| item["submenu"] == false && item["highlighted"] == false),
        "{items:#?}"
    );
    for (key, shortcut) in [
        ('T', Some("Ctrl+Shift+T")),
        ('W', Some("Ctrl+Shift+W")),
        ('C', Some("Ctrl+C")),
        ('P', Some("Shift+Insert")),
        ('A', None),
        ('S', None),
        ('X', None),
    ] {
        assert_eq!(
            item_shortcut(menu_item(&items, key)),
            shortcut,
            "{items:#?}"
        );
    }
    close_menu(&server);

    let (col, row) = find_cell(&state, "alpha-0-end");
    server.mouse(&left_drag(
        cell_left(&state, col, row),
        cell_right(&state, col + 4, row),
        "",
    ));
    let items = open_menu(&server, &click_main_menu(&state));
    assert_eq!(menu_shape(&items), MAIN_MENU_WITH_SELECTION, "{items:#?}");
    close_menu(&server);
}

#[test]
fn the_keys_beside_the_menu_items_follow_the_key_settings() {
    let config = r#"[keys]
"alt+t" = "new_tab"
"ctrl+shift+t" = "none"
"ctrl+shift+w" = "none"
"#;
    let server = Server::start(config, &CMD_COMMAND);
    let state = server.wait_text(CMD_PROMPT);
    for steps in [click_main_menu(&state), right_click_terminal(&state)] {
        let items = open_menu(&server, &steps);
        assert_eq!(
            item_shortcut(menu_item(&items, 'T')),
            Some("Alt+T"),
            "{items:#?}"
        );
        assert_eq!(item_shortcut(menu_item(&items, 'W')), None, "{items:#?}");
        close_menu(&server);
    }
}

/// 押せないコピーで Enter を押すと、何も選ばずにメニューが閉じ、クリップボードは変わらない。
#[test]
fn a_grayed_copy_in_the_menu_does_not_touch_the_clipboard() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);
    server.post("/e2e/clipboard", b"before-the-menu");
    for steps in [click_main_menu(&state), right_click_terminal(&state)] {
        let items = open_menu(&server, &steps);
        select_item(&server, &items, 'C');
        server.wait_dialogs(0);
        server.wait_clipboard("before-the-menu");
    }
    check_terminal_still_works(&server, "after-grayed");
}

/// メニューのコピーは選択範囲をクリップボードへ、貼り付けはクリップボードを端末へ送る。すべて選択は全体を選ぶ。
#[test]
fn copy_paste_and_select_all_in_the_menus_work() {
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
    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'C');
    server.wait_clipboard("alpha");
    server.wait_dialogs(0);

    server.post("/e2e/clipboard", b"echo pasted-%ERRORLEVEL%-menu");
    let items = open_menu(&server, &right_click_terminal(&state));
    select_item(&server, &items, 'P');
    server.wait_dialogs(0);
    server.wait_text("echo pasted-");
    server.press("enter");
    server.wait_text("pasted-0-menu");

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'A');
    let selected = server.wait_until("everything selected", |state| {
        selection(state).is_some_and(|text| text.contains("other-0-end"))
    });
    assert!(
        selection(&selected).is_some_and(|text| text.contains("pasted-0-menu")),
        "{selected:#}"
    );

    // 選択したあとのコピーは、コンテキストメニューからも押せる。
    let items = open_menu(&server, &right_click_terminal(&state));
    assert_eq!(
        menu_shape(&items),
        CONTEXT_MENU_WITH_SELECTION,
        "{items:#?}"
    );
    select_item(&server, &items, 'C');
    server.wait_dialogs(0);
    let (status, body) = server.request("GET", "/e2e/clipboard", b"");
    assert_eq!(status, 200);
    let clipboard: Value = serde_json::from_slice(&body).expect("the answer is not JSON");
    assert!(
        clipboard["text"]
            .as_str()
            .is_some_and(|text| text.contains("other-0-end")),
        "{clipboard:#}"
    );

    // コンテキストメニューの「すべて選択」と、≡ の「貼り付け」。
    server.post("/e2e/clipboard", b"echo main-%ERRORLEVEL%-paste");
    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'P');
    server.wait_text("echo main-");
    assert_eq!(selection(&server.state()), None);
    server.press("enter");
    server.wait_text("main-0-paste");
    let items = open_menu(&server, &right_click_terminal(&state));
    assert_eq!(menu_shape(&items), CONTEXT_MENU, "{items:#?}");
    select_item(&server, &items, 'A');
    server.wait_until("everything selected", |state| {
        selection(state).is_some_and(|text| text.contains("main-0-paste"))
    });
    check_terminal_still_works(&server, "after-copy");
}

/// クリップボードが空なら、メニューの貼り付けは何も送らない。
#[test]
fn a_paste_from_the_menu_with_an_empty_clipboard_sends_nothing() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);
    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'P');
    server.wait_dialogs(0);
    check_terminal_still_works(&server, "after-empty-paste");
}

/// メニューを開いている間にシェルが終わってもタブは閉じ、メニューの操作は残ったタブに効く。最後のシェルが終わると、
/// メニューが開いていてもウィンドウは閉じ、シェルの終了コードで終わる。
#[test]
fn a_shell_ending_while_a_menu_is_open_closes_its_tab_and_then_the_window() {
    let mut server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.post("/e2e/tab/new", b"");
    let state = wait_tabs(&server, 2);

    let items = open_menu(&server, &click_main_menu(&state));
    server.post("/e2e/input", b"exit\r");
    let state = wait_tabs(&server, 1);
    wait_menu(&server);
    select_item(&server, &items, 'T');
    let reopened = wait_tabs(&server, 2);
    assert_eq!(reopened["active"], 1, "{reopened:#}");
    server.post("/e2e/tab/close", b"");
    wait_tabs(&server, 1);

    open_menu(&server, &click_main_menu(&state));
    server.post("/e2e/input", b"exit 7\r");
    assert_eq!(server.wait_exit(), 7);
}

/// メニューの貼り付けも、設定に従って編集のダイアログを通す。
#[test]
fn a_paste_from_the_menu_is_confirmed_like_any_other() {
    let server = Server::start("[paste]\nconfirm = \"multiline\"\n", &CMD_COMMAND);
    let state = server.wait_text(CMD_PROMPT);
    server.post("/e2e/clipboard", b"echo from-%ERRORLEVEL%-menu\n");

    let items = open_menu(&server, &right_click_terminal(&state));
    select_item(&server, &items, 'P');
    wait_text_dialog(&server);
    server.press_dialog_button(BUTTON_CANCEL);
    server.wait_dialogs(0);

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'P');
    wait_text_dialog(&server);
    server.press_dialog_button(BUTTON_OK);
    server.wait_text("from-0-menu");
}

/// メニューの「新しいタブ」「タブを閉じる」。どちらのメニューでも動き、最後のタブを閉じるとウィンドウを閉じる。
#[test]
fn tabs_open_and_close_from_the_menus() {
    let mut server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'T');
    let state = wait_tabs(&server, 2);
    assert_eq!(state["active"], 1, "{state:#}");

    let items = open_menu(&server, &right_click_terminal(&state));
    select_item(&server, &items, 'T');
    let state = wait_tabs(&server, 3);
    assert_eq!(state["active"], 2, "{state:#}");

    let items = open_menu(&server, &right_click_terminal(&state));
    select_item(&server, &items, 'W');
    let state = wait_tabs(&server, 2);
    assert_eq!(state["active"], 1, "{state:#}");

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'W');
    let state = wait_tabs(&server, 1);
    assert_eq!(state["active"], 0, "{state:#}");

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'W');
    assert_eq!(server.wait_exit(), 0);
}

#[test]
fn exit_in_the_main_menu_closes_the_window_with_every_tab() {
    let mut server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);
    server.post("/e2e/tab/new", b"");
    wait_tabs(&server, 2);
    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'X');
    assert_eq!(server.wait_exit(), 0);
}

/// 「設定…」と「fantermo について…」はダイアログを開く。閉じたあとも端末は動く。
#[test]
fn the_main_menu_opens_the_settings_and_about_dialogs() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'S');
    wait_dialog_of_class(&server, SETTINGS_DIALOG_CLASS);
    server.press_dialog_button(DIALOG_CLOSE_BUTTON);
    server.wait_dialogs(0);
    check_terminal_still_works(&server, "after-settings");

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'B');
    let dialog = wait_dialog_of_class(&server, TEXT_DIALOG_CLASS);
    let version = state["version"].as_str().expect("no version");
    assert!(
        dialog["controls"]
            .as_array()
            .is_some_and(|controls| controls
                .iter()
                .any(|control| control.as_str().is_some_and(|text| text.contains(version)))),
        "{dialog:#}"
    );
    server.press_dialog_button(DIALOG_CLOSE_BUTTON);
    server.wait_dialogs(0);
    check_terminal_still_works(&server, "after-about");
}

/// メニューのループへ送ったキーで動く。アクセラレータで選び、Esc で閉じる。
#[test]
fn the_keys_of_a_menu_choose_items_and_escape_closes_it() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);

    open_menu(&server, &click_main_menu(&state));
    press_menu_key(&server, "escape");
    server.wait_dialogs(0);
    assert_eq!(tab_count(&server.state()), 1);

    open_menu(&server, &click_main_menu(&state));
    press_menu_key(&server, "t");
    let state = wait_tabs(&server, 2);

    // 矢印のキーは下へも上へも動き、区切りを飛ばし、押せない項目でも止まる。
    let items = open_menu(&server, &click_main_menu(&state));
    highlight_position(&server, menu_position(&items, 'X'));
    highlight_position(&server, menu_position(&items, 'C'));
    close_menu(&server);
}

/// メニューを開いている間もシェルの出力を読み、入力も届く。閉じたあとは入力と描画が続く。
#[test]
fn the_terminal_keeps_working_during_and_after_a_menu() {
    let server = Server::start(TERMINAL_BACKGROUND_SETTING, &CMD_COMMAND);
    let state = server.wait_text(CMD_PROMPT);
    for (index, steps) in [
        click_main_menu(&state),
        left_click(area_center(&state["tab_bar"]["profiles"])),
        right_click_terminal(&state),
    ]
    .into_iter()
    .enumerate()
    {
        open_menu(&server, &steps);
        let during = format!("during{index}");
        server.post(
            "/e2e/input",
            format!("echo {during}-%ERRORLEVEL%-end\r").as_bytes(),
        );
        let needle = format!("{during}-0-end");
        let state = server.wait_text(&needle);
        let (col, row) = find_cell(&state, &needle);
        assert_cell_is_drawn(&server, &state, col, row);
        wait_menu(&server);
        close_menu(&server);
        check_terminal_still_works(&server, &format!("after{index}"));
    }
}

/// プロファイルの一覧は設定の名前の順で、既定に印が付き、絵が付く。どのボタンからでも同じ。選ぶと、fantermo の引数が
/// あってもそのプロファイルで開く。
#[test]
fn the_profile_menu_lists_the_profiles_and_opens_the_chosen_one() {
    let config = r#"default_profile = "two"

[[profiles]]
name = "one"
command = "cmd.exe /k prompt one$G"

[[profiles]]
name = "two"
command = "cmd.exe /k prompt two$G"

[[profiles]]
name = "R&D"
command = "cmd.exe /k prompt three$G"
"#;
    let server = Server::start(config, &CMD_COMMAND);
    let state = server.wait_text(CMD_PROMPT);
    for steps in [
        left_click(area_center(&state["tab_bar"]["profiles"])),
        button_click("right", area_center(&state["tab_bar"]["profiles"]), ""),
        button_click("right", area_center(&state["tab_bar"]["new_tab"]), ""),
    ] {
        let items = open_menu(&server, &steps);
        assert_eq!(menu_shape(&items), "1 2* 3", "{steps}: {items:#?}");
        let names: Vec<String> = items.iter().map(item_name).collect();
        assert_eq!(names, ["1 one", "2 two", "3 R&D"], "{items:#?}");
        assert!(
            items
                .iter()
                .all(|item| item["icon"] == true && item_shortcut(item).is_none()),
            "{items:#?}"
        );
        close_menu(&server);
    }

    let items = open_menu(
        &server,
        &left_click(area_center(&state["tab_bar"]["profiles"])),
    );
    select_item(&server, &items, '3');
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    let state = server.wait_text("three>");
    assert_eq!(state["tabs"][1]["profile"], "R&D", "{state:#}");
    assert_eq!(state["active"], 1, "{state:#}");
}

/// プロファイルを書いていなければ、既定のプロファイルが一つだけ並ぶ。
#[test]
fn the_profile_menu_without_profiles_has_the_default_one() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);
    let items = open_menu(
        &server,
        &left_click(area_center(&state["tab_bar"]["profiles"])),
    );
    assert_eq!(menu_shape(&items), "1*", "{items:#?}");
    assert_eq!(
        item_name(&items[0]),
        format!("1 {}", state["profiles"][0].as_str().expect("no profile")),
        "{items:#?}"
    );
    close_menu(&server);
}

/// 10 個目からは数字のアクセラレータがなく、選ぶのは矢印のキーと Enter で届く。
#[test]
fn the_profile_menu_items_past_the_ninth_have_no_accelerator_and_can_still_be_chosen() {
    const COUNT: usize = 11;
    let mut config = String::new();
    for index in 1..=COUNT {
        config.push_str(&format!(
            "[[profiles]]\nname = \"p{index}\"\ncommand = \"cmd.exe /k prompt p{index}$G\"\n"
        ));
    }
    let server = Server::start(&config, &[]);
    let state = server.wait_text("p1>");
    let items = open_menu(
        &server,
        &left_click(area_center(&state["tab_bar"]["profiles"])),
    );
    assert_eq!(menu_shape(&items), "1* 2 3 4 5 6 7 8 9 . .", "{items:#?}");
    assert_eq!(item_name(&items[COUNT - 1]), format!("p{COUNT}"));
    select_position(&server, COUNT - 1);
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    let state = server.wait_text("p11>");
    assert_eq!(state["tabs"][1]["profile"], "p11", "{state:#}");
}

/// 設定ファイルを変えると、次に開く一覧が変わる。一覧を開いている間に変わって位置が並びの外になったら、先頭のプロファイルで
/// 開く。「新しいタブ」は既定のプロファイルで開く。
#[test]
fn the_profile_menu_follows_the_settings_file() {
    let two_profiles = r#"default_profile = "first"

[[profiles]]
name = "first"
command = "cmd.exe /k prompt first$G"

[[profiles]]
name = "second"
command = "cmd.exe /k prompt second$G"
"#;
    let renamed = r#"default_profile = "added"

[[profiles]]
name = "first"
command = "cmd.exe /k prompt first$G"

[[profiles]]
name = "added"
command = "cmd.exe /k prompt added$G"

[[profiles]]
name = "second"
command = "cmd.exe /k prompt second$G"
"#;
    let one_profile = r#"[[profiles]]
name = "first"
command = "cmd.exe /k prompt first$G"
"#;
    let server = Server::start(two_profiles, &[]);
    let state = server.wait_text("first>");
    let list = left_click(area_center(&state["tab_bar"]["profiles"]));
    let path = server.dir().join("config.toml");

    let items = open_menu(&server, &list);
    assert_eq!(menu_shape(&items), "1* 2", "{items:#?}");
    close_menu(&server);

    std::fs::write(&path, renamed).expect("failed to write the settings");
    server.wait_until("three profiles", |state| {
        state["profiles"].as_array().map_or(0, Vec::len) == 3
    });
    let items = open_menu(&server, &list);
    assert_eq!(menu_shape(&items), "1 2* 3", "{items:#?}");
    let names: Vec<String> = items.iter().map(item_name).collect();
    assert_eq!(names, ["1 first", "2 added", "3 second"], "{items:#?}");
    close_menu(&server);

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'T');
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    let after = server.wait_text("added>");
    assert_eq!(after["tabs"][1]["profile"], "added", "{after:#}");

    // タブが増えて、ボタンの位置が変わっている。
    let list = left_click(area_center(&after["tab_bar"]["profiles"]));
    let items = open_menu(&server, &list);
    assert_eq!(items.len(), 3, "{items:#?}");
    std::fs::write(&path, one_profile).expect("failed to write the settings");
    server.wait_until("one profile", |state| {
        state["profiles"].as_array().map_or(0, Vec::len) == 1
    });
    wait_menu(&server);
    select_position(&server, 2);
    server.wait_until("three tabs", |state| tab_count(state) == 3);
    let after = server.wait_text("first>");
    assert_eq!(after["tabs"][2]["profile"], "first", "{after:#}");
}

// 設定ダイアログ。コントロールの ID は `settings_dialog.rs` と同じ。

const ID_OK: i32 = 1;
const ID_CANCEL: i32 = 2;
const ID_PAGES: i32 = 100;
const ID_APPLY: i32 = 101;
const ID_ERROR: i32 = 102;
const ID_FONT: i32 = 200;
const ID_SIZE: i32 = 201;
const ID_SIZE_SPIN: i32 = 202;
const ID_WIDE_GLYPH: i32 = 210;
const ID_SCROLL_BAR_WIDTH: i32 = 220;
const ID_SCROLL_BAR_WIDTH_SPIN: i32 = 221;
const ID_PADDING: i32 = 230;
const ID_PASTE_CONFIRM: i32 = 300;
const ID_CLIPBOARD_READ: i32 = 310;
const ID_PASTE_CONFIRM_SIZE: i32 = 320;
const ID_PASTE_CONFIRM_SIZE_SPIN: i32 = 321;
const ID_PROFILES: i32 = 400;
const ID_PROFILE_ADD: i32 = 401;
const ID_PROFILE_REMOVE: i32 = 402;
const ID_PROFILE_DEFAULT: i32 = 403;
const ID_PROFILE_NAME: i32 = 404;
const ID_PROFILE_COMMAND: i32 = 405;
const ID_PROFILE_BACKEND: i32 = 406;
const ID_PROFILE_LOCATION: i32 = 407;
const ID_PROFILE_COMMAND_WARNING: i32 = 408;
const ID_PROFILE_ICON: i32 = 409;
const ID_PROFILE_ICON_CHOOSE: i32 = 410;
const ID_BACKGROUND_IMAGES: i32 = 450;
const ID_BACKGROUND_ADD_FILES: i32 = 451;
const ID_BACKGROUND_ADD_FOLDER: i32 = 452;
const ID_BACKGROUND_REMOVE: i32 = 453;
const ID_BACKGROUND_OPACITY: i32 = 454;
const ID_BACKGROUND_FIT: i32 = 456;
const ID_BACKGROUND_ITEM_FIT: i32 = 457;
const ID_BACKGROUND_ITEM_OPACITY: i32 = 458;
const ID_BACKGROUND_ITEM_OPACITY_DEFAULT: i32 = 460;
const ID_BACKGROUND_ITEM_OPACITY_CUSTOM: i32 = 461;
const ID_KEYS: i32 = 500;
const ID_KEY_ADD: i32 = 501;
const ID_KEY_CHANGE: i32 = 502;
const ID_KEY_REMOVE: i32 = 503;
const ID_KEY_RESET: i32 = 504;
const ID_COLOR_SCHEME: i32 = 590;
const ID_COLORS: i32 = 600;
/// 色のページの一行のコントロールの ID の間隔と、値の文字・選ぶ・既定に戻すの位置。
const COLOR_ID_STRIDE: i32 = 4;
const COLOR_TEXT: i32 = 1;
const COLOR_CHOOSE: i32 = 2;
const COLOR_RESET: i32 = 3;
/// 色のページの行の位置。端末の背景色と文字色、タブバーの背景、スクロールバーのつまみ。
const COLOR_ROW_BACKGROUND: i32 = 0;
const COLOR_ROW_FOREGROUND: i32 = 1;
const COLOR_ROW_TAB_BAR: i32 = 2;
const COLOR_ROW_THUMB: i32 = 12;

/// ページの位置。
const PAGE_APPEARANCE: usize = 0;
const PAGE_BACKGROUND: usize = 1;
const PAGE_BEHAVIOR: usize = 2;
const PAGE_PROFILES: usize = 3;
const PAGE_KEYS: usize = 4;
const PAGE_COLORS: usize = 5;

/// 接続方式のコンボボックスの位置。
const BACKEND_CONPTY: usize = 0;
const BACKEND_CYGWIN: usize = 1;
const BACKEND_WSL: usize = 2;
/// 表示方法のコンボボックスの位置（項目のものは先頭に「既定」があり、一つずれる）。
const FIT_TILE: usize = 3;
const FIT_CENTER: usize = 4;
const ITEM_FIT_DEFAULT: usize = 0;
const ITEM_FIT_CENTER: usize = FIT_CENTER + 1;

/// メッセージボックスと色を選ぶダイアログのボタン。
const ID_YES: i32 = 6;
const ID_NO: i32 = 7;
/// 色を選ぶダイアログの赤・緑・青の入力欄。
const COLOR_DIALOG_RED: i32 = 706;
const COLOR_DIALOG_GREEN: i32 = 707;
const COLOR_DIALOG_BLUE: i32 = 708;

const KEY_CAPTURE_CLASS: &str = "fantermo-key-capture";
/// メッセージボックスと OS のダイアログのウィンドウのクラス。
const SYSTEM_DIALOG_CLASS: &str = "#32770";

/// ランナーの画面の大きさ（96 DPI）。
const RUNNER_SCREEN: (u64, u64) = (1024, 768);
const BASE_DPI: u64 = 96;

/// `cmd.exe` を起動するプロファイルが一つの設定。
const ONE_PROFILE: &str = "[[profiles]]\nname = \"one\"\ncommand = \"cmd.exe /k prompt e2e$G\"\n";

/// 引数なしで、設定 `config` のプロファイルの `cmd.exe` を起動する。
fn start_settings_test(config: &str) -> Server {
    let server = Server::start(config, &[]);
    server.wait_text(CMD_PROMPT);
    server
}

/// ≡ の「設定…」で設定ダイアログを開き、コントロールを返す。
#[track_caller]
fn open_settings(server: &Server) -> Value {
    let state = server.state();
    let items = open_menu(server, &click_main_menu(&state));
    select_item(server, &items, 'S');
    wait_dialog_of_class(server, SETTINGS_DIALOG_CLASS);
    read_controls(server)
}

/// いちばん上のダイアログのコントロール。
#[track_caller]
fn read_controls(server: &Server) -> Value {
    let (status, body) = server.request("GET", "/e2e/dialog/controls", b"");
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).expect("the controls are not JSON")
}

/// `dialog` の ID が `id` のコントロール。
#[track_caller]
fn control(dialog: &Value, id: i32) -> &Value {
    dialog["controls"]
        .as_array()
        .and_then(|controls| controls.iter().find(|control| control["id"] == id))
        .unwrap_or_else(|| panic!("no control {id}: {dialog:#}"))
}

/// いまの ID が `id` のコントロールの文字。
#[track_caller]
fn control_text(server: &Server, id: i32) -> String {
    let dialog = read_controls(server);
    control(&dialog, id)["text"]
        .as_str()
        .expect("no text")
        .to_owned()
}

/// いまの ID が `id` のコントロールを押せるか。
#[track_caller]
fn control_enabled(server: &Server, id: i32) -> bool {
    control(&read_controls(server), id)["enabled"] == true
}

/// いまの一覧やコンボボックス `id` の項目の数と、選んでいる位置。
#[track_caller]
fn list_state(server: &Server, id: i32) -> (usize, Value) {
    let dialog = read_controls(server);
    let list = control(&dialog, id);
    (
        list["items"].as_array().map_or(0, Vec::len),
        list["selected"].clone(),
    )
}

/// ラジオボタン `id` を選んでいるか。
#[track_caller]
fn radio_checked(dialog: &Value, id: i32) -> bool {
    control(dialog, id)["checked"] == true
}

fn set_text(server: &Server, id: i32, text: &str) {
    server.post(&format!("/e2e/dialog/text/{id}"), text.as_bytes());
}

fn select(server: &Server, id: i32, position: usize) {
    server.post(&format!("/e2e/dialog/select/{id}/{position}"), b"");
}

fn choose_radio(server: &Server, id: i32) {
    server.post(&format!("/e2e/dialog/radio/{id}"), b"");
}

fn click(server: &Server, id: i32) {
    server.post(&format!("/e2e/dialog/click/{id}"), b"");
}

fn spin(server: &Server, id: i32, delta: i32) {
    server.post(&format!("/e2e/dialog/spin/{id}/{delta}"), b"");
}

fn press_key_in_dialog(server: &Server, combo: &str) {
    server.post(&format!("/e2e/dialog/key/{combo}"), b"");
}

fn show_page(server: &Server, page: usize) {
    select(server, ID_PAGES, page);
}

/// いちばん上のダイアログのクラスが `class` になるのを待つ。
#[track_caller]
fn wait_top_dialog(server: &Server, class: &str) -> Value {
    let state = server.wait_until(&format!("the top dialog of {class}"), |state| {
        state["dialogs"][0]["class"] == class
    });
    state["dialogs"][0].clone()
}

/// 「OK」を押して、閉じるのを待つ。
#[track_caller]
fn press_ok(server: &Server) {
    click(server, ID_OK);
    server.wait_dialogs(0);
}

/// 「キャンセル」を押して、閉じるのを待つ。
#[track_caller]
fn press_cancel(server: &Server) {
    click(server, ID_CANCEL);
    server.wait_dialogs(0);
}

/// 「適用」を押す。続く要求はこの処理のあとなので、状態を読んだあとにファイルを読めば書いたものが読める。
#[track_caller]
fn press_apply(server: &Server) {
    click(server, ID_APPLY);
    server.state();
}

fn config_path(server: &Server) -> PathBuf {
    server.dir().join("config.toml")
}

fn config_text(server: &Server) -> String {
    std::fs::read_to_string(config_path(server)).expect("failed to read the settings")
}

/// 設定ファイルを読んで解釈する。
#[track_caller]
fn saved_config(server: &Server) -> Config {
    let text = config_text(server);
    let parsed = config::parse(&text, &Config::default());
    assert!(!parsed.syntax_error, "{text}");
    parsed.config
}

/// いま開いているメッセージボックスの文字（ボタンと本文）。メッセージボックスが開くのを待つ。
#[track_caller]
fn wait_message_box(server: &Server) -> String {
    let dialog = wait_top_dialog(server, SYSTEM_DIALOG_CLASS);
    dialog["controls"]
        .as_array()
        .map(|controls| {
            controls
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// メッセージボックスを × で閉じて、開いているダイアログが `remaining` 個になるのを待つ。
#[track_caller]
fn close_message_box(server: &Server, remaining: usize) {
    server.post("/e2e/dialog/close", b"");
    server.wait_dialogs(remaining);
}

fn number_of(dialog: &Value, name: &str) -> u64 {
    number(&dialog[name])
}

/// 起動時の値と、ページごとに見えるコントロールと、大きさの確認の設定。
const EVERY_PAGE_CONFIG: &str = r##"default_profile = "two"

[[profiles]]
name = "one"
command = "cmd.exe /k prompt e2e$G"

[[profiles]]
name = "two"
command = "cmd.exe /k prompt e2e$G"
icon = 'two.png'

[[profiles]]
name = "ubuntu"
backend = "wsl"
distribution = "Ubuntu"
command = "bash -l"

[[profiles]]
name = "zsh"
backend = "cygwin"
cygwin_root = 'C:\cygwin-of-the-test'

[font]
family = ["Consolas"]
size = 14
wide_glyph = "clip"

[paste]
confirm = "multiline"
confirm_size = 16

[clipboard]
read = "deny"

[scroll_bar]
width = 20

[window]
padding = 8

[colors]
scheme = "light"
background = "#102030"

[colors.tab_bar]
bar = "#112233"

[colors.scroll_bar]
thumb = "#445566"

[background]
image_opacity = 0.5
fit = "tile"

[[background.images]]
path = 'a.png'

[[background.images]]
path = 'b.png'
fit = "center"
image_opacity = 0.25

[keys]
"ctrl+shift+n" = "new_tab"
"ctrl+tab" = "none"
"##;

/// `id` の色の行の、`offset` のコントロールの ID。
const fn color_control(row: i32, offset: i32) -> i32 {
    ID_COLORS + row * COLOR_ID_STRIDE + offset
}

/// キーの割り当ての一覧の行の数（組み合わせのない動作も一行）と、`action` の `combo` の行の位置（`combo` が `None` なら
/// 組み合わせのない行）。
fn key_row_of(keys: &Keymap, action: Action, combo: Option<&str>) -> usize {
    let combo = combo.map(|combo| KeyCombo::parse(combo).expect("a valid combination"));
    let mut position = 0;
    for candidate in Action::ALL {
        let combos = keys.combos_of(candidate);
        if candidate == action {
            let offset = combo.map_or(0, |combo| {
                combos
                    .iter()
                    .position(|&other| other == combo)
                    .unwrap_or_else(|| panic!("{combo} is not bound to {action:?}"))
            });
            return position + offset;
        }
        position += combos.len().max(1);
    }
    unreachable!("every action is listed")
}

/// 全ページが開いて、見えるコントロールがページごとに替わり、起動時の値が設定ファイルのとおり。
#[test]
fn every_settings_page_opens_and_shows_the_values_of_the_file() {
    let server = start_settings_test_with_images(EVERY_PAGE_CONFIG, &["a.png", "b.png"]);
    let config = saved_config(&server);
    let dialog = open_settings(&server);
    assert_eq!(control(&dialog, ID_PAGES)["selected"], PAGE_APPEARANCE);
    let pages = control(&dialog, ID_PAGES)["items"]
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(pages, 6, "{dialog:#}");

    // 外観
    assert_eq!(control(&dialog, ID_FONT)["text"], "Consolas", "{dialog:#}");
    assert_eq!(control(&dialog, ID_SIZE)["text"], "14");
    assert!(radio_checked(&dialog, ID_WIDE_GLYPH + 1), "{dialog:#}");
    assert_eq!(control(&dialog, ID_SCROLL_BAR_WIDTH)["text"], "20");
    assert_eq!(control(&dialog, ID_PADDING)["text"], "8");

    // 背景
    assert_eq!(control(&dialog, ID_BACKGROUND_OPACITY)["text"], "0.5");
    assert_eq!(control(&dialog, ID_BACKGROUND_FIT)["selected"], FIT_TILE);
    assert_eq!(list_state(&server, ID_BACKGROUND_IMAGES), (2, Value::Null));
    assert!(!control_enabled_on_page(
        &server,
        PAGE_BACKGROUND,
        ID_BACKGROUND_REMOVE
    ));
    select(&server, ID_BACKGROUND_IMAGES, 0);
    let dialog = read_controls(&server);
    assert_eq!(
        control(&dialog, ID_BACKGROUND_ITEM_FIT)["selected"],
        ITEM_FIT_DEFAULT
    );
    assert!(radio_checked(&dialog, ID_BACKGROUND_ITEM_OPACITY_DEFAULT));
    select(&server, ID_BACKGROUND_IMAGES, 1);
    let dialog = read_controls(&server);
    assert_eq!(
        control(&dialog, ID_BACKGROUND_ITEM_FIT)["selected"],
        ITEM_FIT_CENTER
    );
    assert!(radio_checked(&dialog, ID_BACKGROUND_ITEM_OPACITY_CUSTOM));
    assert_eq!(control(&dialog, ID_BACKGROUND_ITEM_OPACITY)["text"], "0.25");

    // 動作
    assert!(radio_checked(&dialog, ID_PASTE_CONFIRM + 1), "{dialog:#}");
    assert!(radio_checked(&dialog, ID_CLIPBOARD_READ + 2), "{dialog:#}");
    assert_eq!(control(&dialog, ID_PASTE_CONFIRM_SIZE)["text"], "16");

    // プロファイル。開くと既定のプロファイルを選んでいる。
    assert_eq!(list_state(&server, ID_PROFILES), (4, Value::from(1)));
    assert_eq!(control(&dialog, ID_PROFILE_NAME)["text"], "two");
    assert_eq!(
        control(&dialog, ID_PROFILE_COMMAND)["text"],
        "cmd.exe /k prompt e2e$G"
    );
    assert_eq!(control(&dialog, ID_PROFILE_ICON)["text"], "two.png");
    assert_eq!(
        control(&dialog, ID_PROFILE_BACKEND)["selected"],
        BACKEND_CONPTY
    );
    show_page(&server, PAGE_PROFILES);
    assert!(!control_enabled(&server, ID_PROFILE_LOCATION));
    select(&server, ID_PROFILES, 2);
    let dialog = read_controls(&server);
    assert_eq!(
        control(&dialog, ID_PROFILE_BACKEND)["selected"],
        BACKEND_WSL
    );
    assert_eq!(control(&dialog, ID_PROFILE_LOCATION)["text"], "Ubuntu");
    select(&server, ID_PROFILES, 3);
    let dialog = read_controls(&server);
    assert_eq!(
        control(&dialog, ID_PROFILE_BACKEND)["selected"],
        BACKEND_CYGWIN
    );
    assert_eq!(
        control(&dialog, ID_PROFILE_LOCATION)["text"],
        r"C:\cygwin-of-the-test"
    );
    assert_eq!(control(&dialog, ID_PROFILE_COMMAND)["text"], "");

    // キー。組み合わせごとに一行で、組み合わせのない動作も一行。
    let rows: usize = Action::ALL
        .iter()
        .map(|&action| config.keys.combos_of(action).len().max(1))
        .sum();
    assert_eq!(list_state(&server, ID_KEYS).0, rows);

    // 色
    assert!(radio_checked(&dialog, ID_COLOR_SCHEME + 1), "{dialog:#}");
    for (row, color) in [
        (COLOR_ROW_BACKGROUND, "#102030"),
        (COLOR_ROW_TAB_BAR, "#112233"),
        (COLOR_ROW_THUMB, "#445566"),
    ] {
        let value = &control(&dialog, color_control(row, COLOR_TEXT))["text"];
        assert!(
            value.as_str().is_some_and(|text| text.contains(color)),
            "{row}: {value}"
        );
    }

    // ページごとに見えるコントロールが替わる。
    let on_page: [(usize, &[i32]); 6] = [
        (
            PAGE_APPEARANCE,
            &[ID_FONT, ID_SIZE, ID_WIDE_GLYPH, ID_PADDING],
        ),
        (
            PAGE_BACKGROUND,
            &[
                ID_BACKGROUND_IMAGES,
                ID_BACKGROUND_OPACITY,
                ID_BACKGROUND_FIT,
            ],
        ),
        (
            PAGE_BEHAVIOR,
            &[ID_PASTE_CONFIRM, ID_CLIPBOARD_READ, ID_PASTE_CONFIRM_SIZE],
        ),
        (
            PAGE_PROFILES,
            &[ID_PROFILES, ID_PROFILE_NAME, ID_PROFILE_BACKEND],
        ),
        (PAGE_KEYS, &[ID_KEYS, ID_KEY_ADD, ID_KEY_RESET]),
        (
            PAGE_COLORS,
            &[
                ID_COLOR_SCHEME,
                color_control(COLOR_ROW_FOREGROUND, COLOR_RESET),
            ],
        ),
    ];
    for &(page, shown) in &on_page {
        show_page(&server, page);
        let dialog = read_controls(&server);
        for &(other, ids) in &on_page {
            for &id in ids {
                assert_eq!(
                    control(&dialog, id)["visible"],
                    other == page,
                    "the control {id} on the page {page}"
                );
            }
        }
        for id in [ID_PAGES, ID_OK, ID_CANCEL, ID_APPLY] {
            assert_eq!(control(&dialog, id)["visible"], true, "{id} on {page}");
        }
        assert!(
            shown
                .iter()
                .all(|&id| control(&dialog, id)["visible"] == true)
        );
        assert_controls_fit(&dialog, page);
    }
    press_cancel(&server);
}

/// 見えているコントロールがダイアログのクライアント領域に収まり、ダイアログがランナーの画面（DPI の分だけ広げた大きさ）に
/// 収まることを確認する。
#[track_caller]
fn assert_controls_fit(dialog: &Value, page: usize) {
    let dpi = number_of(dialog, "dpi");
    assert!(
        number_of(dialog, "width") * BASE_DPI <= RUNNER_SCREEN.0 * dpi
            && number_of(dialog, "height") * BASE_DPI <= RUNNER_SCREEN.1 * dpi,
        "the dialog does not fit the screen of the runner: {dialog:#}"
    );
    let (client_width, client_height) = (
        dialog["client_width"].as_i64().expect("no width"),
        dialog["client_height"].as_i64().expect("no height"),
    );
    for control in dialog["controls"].as_array().expect("no controls") {
        if control["visible"] != true {
            continue;
        }
        let field = |name: &str| control[name].as_i64().expect("no position");
        assert!(
            field("x") >= 0
                && field("y") >= 0
                && field("x") + field("width") <= client_width
                && field("y") + field("height") <= client_height,
            "the control is out of the dialog on the page {page}: {control:#}"
        );
    }
}

/// 小さいウィンドウの真ん中に置くと画面の外へはみ出す設定ダイアログは、作業領域の中に置く。
#[test]
fn the_settings_dialog_stays_on_the_screen_over_a_small_window() {
    const SMALL_GRID: (u16, u16) = (20, 5);
    let server = start_settings_test(ONE_PROFILE);
    server.resize(SMALL_GRID.0, SMALL_GRID.1);
    let dialog = open_settings(&server);
    let work = &dialog["work_area"];
    let field = |value: &Value| value.as_i64().expect("not a number");
    let (left, top) = (field(&dialog["left"]), field(&dialog["top"]));
    let (width, height) = (field(&dialog["width"]), field(&dialog["height"]));
    assert!(
        left >= field(&work["left"])
            && top >= field(&work["top"])
            && left + width <= field(&work["right"])
            && top + height <= field(&work["bottom"]),
        "{dialog:#}"
    );
    press_cancel(&server);
}

/// `page` を見せて、コントロール `id` を押せるかを読む。
#[track_caller]
fn control_enabled_on_page(server: &Server, page: usize, id: i32) -> bool {
    show_page(server, page);
    control_enabled(server, id)
}

/// 設定 `config` で起動する前に、`images` の名前で色の画像を置く。
fn start_settings_test_with_images(config: &str, images: &[&str]) -> Server {
    let server = Server::start_with_setup(config, &[], |dir| {
        for image in images {
            std::fs::write(dir.join(image), solid_image([255, 0, 0]))
                .expect("failed to write the image");
        }
    });
    server.wait_text(CMD_PROMPT);
    server
}

/// 外観のページを変えると端末にすぐ反映するが、設定ファイルは「OK」まで変わらない。「OK」で変えた項目が入る。
/// ウィンドウの位置と大きさは変わらず、桁数と行数がいまのクライアント領域に合わせて変わる。
#[test]
fn the_appearance_page_previews_at_once_and_ok_writes_it() {
    let original = format!(
        "{ONE_PROFILE}[font]\nfamily = [\"Consolas\"]\nsize = 11\n\n[scroll_bar]\nwidth = 12\n\n[window]\npadding = 4\n"
    );
    let server = start_settings_test(&original);
    server.resize(50, 15);
    let before = server.state();
    let window = current_window(&before);
    let dialog = open_settings(&server);
    assert_eq!(server.state()["window_enabled"], false);

    set_text(&server, ID_SIZE, "20");
    spin(&server, ID_SIZE_SPIN, 1);
    assert_eq!(control_text(&server, ID_SIZE), "21");
    let taller = server.wait_until("taller cells", |state| {
        number(&state["cell_height"]) > number(&before["cell_height"])
    });
    assert_eq!(current_window(&taller), window, "{taller:#}");
    assert!(
        number(&taller["cols"]) < number(&before["cols"])
            && number(&taller["rows"]) < number(&before["rows"]),
        "{taller:#}"
    );
    assert_layout_fits_the_client(&taller);

    set_text(&server, ID_SCROLL_BAR_WIDTH, "30");
    let wider = server.wait_until("the wider scroll bar", |state| {
        state["scroll_bar"]["width"] == 30
    });
    assert_eq!(current_window(&wider), window, "{wider:#}");
    assert_layout_fits_the_client(&wider);
    set_text(&server, ID_PADDING, "16");
    let padding_grew = number(&before["terminal_x"]) + 12;
    let padded = server.wait_until("the wider padding", |state| {
        number(&state["terminal_x"]) == padding_grew
    });
    assert_eq!(current_window(&padded), window, "{padded:#}");
    assert_layout_fits_the_client(&padded);
    choose_radio(&server, ID_WIDE_GLYPH + 2);

    // 一覧の別のフォントを選ぶ。ランナーに一つしかなければ選ばない。
    let fonts = control(&dialog, ID_FONT)["items"]
        .as_array()
        .expect("no fonts");
    let other_font = fonts
        .iter()
        .position(|font| font != "Consolas")
        .map(|position| {
            (
                position,
                fonts[position].as_str().expect("a name").to_owned(),
            )
        });
    if let Some((position, _)) = &other_font {
        select(&server, ID_FONT, *position);
    }
    assert_eq!(config_text(&server), original, "written before OK");

    press_ok(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.font.size_pt, 21.0);
    assert_eq!(saved.scroll_bar.width, 30);
    assert_eq!(saved.window.padding, 16);
    assert_eq!(saved.font.wide_glyph, WideGlyph::Overflow);
    if let Some((_, name)) = other_font {
        assert_eq!(saved.font.families[0], name);
    }
    let state = server.wait_until("the window enabled again", |state| {
        state["window_enabled"] == true
    });
    assert_eq!(state["scroll_bar"]["width"], 30, "{state:#}");
    assert_eq!(current_window(&state), window, "{state:#}");
    check_terminal_still_works(&server, "after-ok");
}

/// 試し表示のあいだも「キャンセル」で戻したあとも、ウィンドウの位置と大きさは 1 ピクセルも変わらない。桁数と行数は
/// 元に戻る。セルで割り切れない大きさのウィンドウで確認する。
#[test]
fn the_preview_and_cancel_keep_the_window_rect() {
    let (server, before) = start_uneven(
        &format!("{ONE_PROFILE}[font]\nfamily = [\"Consolas\"]\nsize = 11\n"),
        &[],
        CMD_PROMPT,
    );
    let window = current_window(&before);
    open_settings(&server);

    set_text(&server, ID_SIZE, "20");
    let previewed = server.wait_until("taller cells", |state| {
        number(&state["cell_height"]) > number(&before["cell_height"])
    });
    assert_eq!(current_window(&previewed), window, "{previewed:#}");
    assert_ne!(previewed["rows"], before["rows"], "{previewed:#}");
    assert_layout_fits_the_client(&previewed);

    press_cancel(&server);
    let state = server.wait_until("the cells back", |state| {
        state["cell_height"] == before["cell_height"] && state["window_enabled"] == true
    });
    assert_eq!(current_window(&state), window, "{state:#}");
    assert_eq!(state["cols"], before["cols"], "{state:#}");
    assert_eq!(state["rows"], before["rows"], "{state:#}");
    assert_layout_fits_the_client(&state);
}

/// 「適用」は書いて開いたまま。そのあとの「キャンセル」は適用したときへ戻す。
#[test]
fn apply_keeps_the_dialog_open_and_cancel_returns_to_what_was_applied() {
    let server = start_settings_test(&format!("{ONE_PROFILE}[window]\npadding = 4\n"));
    let first_x = number(&server.state()["terminal_x"]);
    open_settings(&server);

    set_text(&server, ID_PADDING, "10");
    press_apply(&server);
    assert_eq!(saved_config(&server).window.padding, 10);
    assert_eq!(server.state()["dialogs"].as_array().map_or(0, Vec::len), 1);
    let applied_x = first_x + 6;
    server.wait_until("the applied padding", |state| {
        number(&state["terminal_x"]) == applied_x
    });
    let applied = config_text(&server);

    set_text(&server, ID_PADDING, "20");
    let previewed_x = first_x + 16;
    server.wait_until("the previewed padding", |state| {
        number(&state["terminal_x"]) == previewed_x
    });
    press_cancel(&server);
    let state = server.state();
    assert_eq!(number(&state["terminal_x"]), applied_x, "{state:#}");
    assert_eq!(config_text(&server), applied);
}

/// 「キャンセル」、×、Esc は、どのページの変更も端末から取り下げ、設定ファイルを変えない。Enter は「OK」。
#[test]
fn cancelling_leaves_the_file_and_the_window_as_they_were() {
    let original = format!(
        "{ONE_PROFILE}\n[font]\nsize = 11\n\n[window]\npadding = 4\n\n[paste]\nconfirm = \"never\"\n"
    );
    let server = start_settings_test(&original);
    let before = server.state();
    type CloseBy = (&'static str, fn(&Server));
    let close_by: [CloseBy; 3] = [
        ("the cancel button", |server| {
            click(server, ID_CANCEL);
        }),
        ("the close button of the window", |server| {
            server.post("/e2e/dialog/close", b"");
        }),
        ("the escape key", |server| {
            press_key_in_dialog(server, "escape");
        }),
    ];
    for (name, close) in close_by {
        open_settings(&server);
        set_text(&server, ID_SIZE, "20");
        set_text(&server, ID_PADDING, "16");
        show_page(&server, PAGE_BEHAVIOR);
        choose_radio(&server, ID_PASTE_CONFIRM + 1);
        show_page(&server, PAGE_PROFILES);
        click(&server, ID_PROFILE_ADD);
        server.wait_until("the previewed settings", |state| {
            number(&state["cell_height"]) > number(&before["cell_height"])
                && state["profiles"].as_array().map_or(0, Vec::len) == 2
        });
        close(&server);
        server.wait_dialogs(0);
        let state = server.state();
        assert_eq!(state["cell_height"], before["cell_height"], "{name}");
        assert_eq!(state["terminal_x"], before["terminal_x"], "{name}");
        assert_eq!(state["profiles"], before["profiles"], "{name}");
        assert_eq!(config_text(&server), original, "{name}");
        // 取り下げた貼り付けの設定に戻っている。改行があっても確認しない。
        server.paste("rem back\r\n");
        server.wait_text("rem back");
        assert_eq!(server.state()["dialogs"].as_array().map_or(0, Vec::len), 0);
    }

    open_settings(&server);
    set_text(&server, ID_PADDING, "9");
    press_key_in_dialog(&server, "enter");
    server.wait_dialogs(0);
    assert_eq!(saved_config(&server).window.padding, 9);
}

/// 設定ファイルの、ダイアログで扱わないコメントと未知のキーとテーブルは、保存で消えない。
#[test]
fn saving_keeps_the_comments_and_the_unknown_keys_of_the_file() {
    let original = r##"# 先頭のコメント
unknown_top = "kept"

# プロファイルのコメント
[[profiles]]
name = "one" # 名前のコメント
command = "cmd.exe /k prompt e2e$G"
unknown_in_profile = 1

[font]
# サイズのコメント
size = 11 # 行末のコメント
unknown_in_font = true

[window]
padding = 4

[unknown_table]
key = [1, 2, 3] # 未知のテーブルのコメント

[background]
image_opacity = 0.5

# 画像のコメント
[[background.images]]
path = 'a.png' # パスのコメント
unknown_in_image = "kept"

[[background.images]]
path = 'b.png'

[keys]
# キーのコメント
"ctrl+tab" = "none"
"##;
    let server = start_settings_test_with_images(original, &["a.png", "b.png"]);
    // 未知のキーは起動の警告で知らせる。
    wait_message_box(&server);
    close_message_box(&server, 0);

    // 何も変えなければ書かない。
    open_settings(&server);
    press_ok(&server);
    assert_eq!(config_text(&server), original);

    // 一つ変えれば、その値だけが変わる。
    open_settings(&server);
    set_text(&server, ID_SIZE, "14");
    press_ok(&server);
    assert_eq!(
        config_text(&server),
        original.replace("size = 11 #", "size = 14 #")
    );

    // いろいろなページを変えても、ほかの行は残る。
    open_settings(&server);
    set_text(&server, ID_PADDING, "6");
    show_page(&server, PAGE_BACKGROUND);
    select(&server, ID_BACKGROUND_FIT, FIT_CENTER);
    select(&server, ID_BACKGROUND_IMAGES, 0);
    select(&server, ID_BACKGROUND_ITEM_FIT, ITEM_FIT_CENTER);
    select(&server, ID_BACKGROUND_IMAGES, 1);
    click(&server, ID_BACKGROUND_REMOVE);
    show_page(&server, PAGE_PROFILES);
    set_text(&server, ID_PROFILE_COMMAND, "cmd.exe /k prompt renamed$G");
    click(&server, ID_PROFILE_ADD);
    show_page(&server, PAGE_COLORS);
    choose_radio(&server, ID_COLOR_SCHEME + 1);
    press_ok(&server);

    let text = config_text(&server);
    for line in original.lines() {
        // 変えた値の行と、一覧から外した項目の行のほかは残る。
        let changed = [
            "size = 11",
            "padding = 4",
            "command = ",
            "path = 'b.png'",
            "[[background.images]]",
            "[[profiles]]",
        ]
        .iter()
        .any(|written| line.starts_with(written));
        assert!(changed || text.contains(line), "lost {line:?} in:\n{text}");
    }
    let saved = saved_config(&server);
    assert_eq!(saved.window.padding, 6);
    assert_eq!(saved.background.images.len(), 1);
    assert_eq!(saved.background.images[0].fit, Some(BackgroundFit::Center));
    assert_eq!(saved.profiles.len(), 2);
    assert_eq!(saved.profiles[0].command, "cmd.exe /k prompt renamed$G");
    assert_eq!(saved.colors.scheme, ColorScheme::Light);
}

/// 数値の欄の不正な入力は、エラーの文を出し、書かず、その入力欄へ移る。直せば消える。
#[test]
fn invalid_numbers_show_an_error_and_are_not_saved() {
    let original = format!(
        "{ONE_PROFILE}[font]\nsize = 11\n\n[scroll_bar]\nwidth = 12\n\n[window]\npadding = 4\n\n[paste]\nconfirm_size = 8\n\n[background]\nimage_opacity = 0.5\n\n[[background.images]]\npath = 'a.png'\nimage_opacity = 0.25\n"
    );
    let server = start_settings_test_with_images(&original, &["a.png"]);
    let before = server.state();
    open_settings(&server);
    show_page(&server, PAGE_BACKGROUND);
    select(&server, ID_BACKGROUND_IMAGES, 0);

    let cases: [(usize, i32, &[&str], &str); 6] = [
        (
            PAGE_APPEARANCE,
            ID_SIZE,
            &["abc", "", "0", "129", "-1"],
            "11",
        ),
        (
            PAGE_APPEARANCE,
            ID_SCROLL_BAR_WIDTH,
            &["abc", "3", "65", "1.5"],
            "12",
        ),
        (PAGE_APPEARANCE, ID_PADDING, &["abc", "-1", "65"], "4"),
        (
            PAGE_BEHAVIOR,
            ID_PASTE_CONFIRM_SIZE,
            &["abc", "-1", "1048577"],
            "8",
        ),
        (
            PAGE_BACKGROUND,
            ID_BACKGROUND_OPACITY,
            &["abc", "-0.1", "1.1"],
            "0.5",
        ),
        (
            PAGE_BACKGROUND,
            ID_BACKGROUND_ITEM_OPACITY,
            &["abc", "2"],
            "0.25",
        ),
    ];
    for (page, id, bad_values, good) in cases {
        show_page(&server, page);
        for bad in bad_values {
            set_text(&server, id, bad);
            press_apply(&server);
            let dialog = read_controls(&server);
            assert_ne!(
                control(&dialog, ID_ERROR)["text"],
                "",
                "{id} accepted {bad:?}"
            );
            assert_eq!(dialog["focus"], id, "{id} with {bad:?}");
            assert_eq!(config_text(&server), original, "{id} with {bad:?}");
            // 端末は読めた値のまま。
            let state = server.state();
            assert_eq!(state["cell_height"], before["cell_height"], "{id}");
            assert_eq!(state["terminal_x"], before["terminal_x"], "{id}");
            // 「OK」でも閉じない。
            click(&server, ID_OK);
            assert_eq!(open_dialog_count(&server), 1, "{id} with {bad:?}");
        }
        set_text(&server, id, good);
        press_apply(&server);
        assert_eq!(control_text(&server, ID_ERROR), "", "{id}");
        assert_eq!(config_text(&server), original, "{id}");
    }

    // 打っているだけでも、少し待ってエラーの文を出す。
    show_page(&server, PAGE_APPEARANCE);
    set_text(&server, ID_SIZE, "abc");
    let deadline = Instant::now() + MENU_TIMEOUT;
    while control_text(&server, ID_ERROR).is_empty() {
        assert!(Instant::now() < deadline, "no error for what was typed");
        std::thread::sleep(MENU_POLL_INTERVAL);
    }
    // 矢印は読めない入力なら開いたときの値から動かす。
    spin(&server, ID_SIZE_SPIN, 1);
    assert_eq!(control_text(&server, ID_SIZE), "12");
    assert_eq!(control_text(&server, ID_ERROR), "");

    // 矢印は範囲の外へ出ない。
    set_text(&server, ID_SCROLL_BAR_WIDTH, "64");
    spin(&server, ID_SCROLL_BAR_WIDTH_SPIN, 1);
    assert_eq!(control_text(&server, ID_SCROLL_BAR_WIDTH), "64");
    set_text(&server, ID_SCROLL_BAR_WIDTH, "4");
    spin(&server, ID_SCROLL_BAR_WIDTH_SPIN, -1);
    assert_eq!(control_text(&server, ID_SCROLL_BAR_WIDTH), "4");
    show_page(&server, PAGE_BEHAVIOR);
    set_text(&server, ID_PASTE_CONFIRM_SIZE, "0");
    spin(&server, ID_PASTE_CONFIRM_SIZE_SPIN, -1);
    assert_eq!(control_text(&server, ID_PASTE_CONFIRM_SIZE), "0");

    // 不正な入力があってもキャンセルでは閉じる。
    show_page(&server, PAGE_APPEARANCE);
    set_text(&server, ID_PADDING, "abc");
    press_cancel(&server);
    assert_eq!(config_text(&server), original);
    let state = server.state();
    assert_eq!(state["cell_height"], before["cell_height"], "{state:#}");
}

/// 開いているダイアログの数を読む。前の要求の処理のあとに読むので、閉じたならここで分かる。
fn open_dialog_count(server: &Server) -> usize {
    server.state()["dialogs"].as_array().map_or(0, Vec::len)
}

/// 動作のページの値が入り、貼り付けとクリップボードの動作が変わる。
#[test]
fn the_behavior_page_changes_how_pastes_are_confirmed() {
    let server = start_settings_test(&format!("{ONE_PROFILE}[paste]\nconfirm = \"never\"\n"));
    server.paste("rem first\r\nrem second\r\n");
    server.wait_text("rem second");
    assert_eq!(open_dialog_count(&server), 0);

    open_settings(&server);
    show_page(&server, PAGE_BEHAVIOR);
    choose_radio(&server, ID_PASTE_CONFIRM + 1);
    let dialog = read_controls(&server);
    assert!(radio_checked(&dialog, ID_PASTE_CONFIRM + 1));
    assert!(!radio_checked(&dialog, ID_PASTE_CONFIRM + 2));
    choose_radio(&server, ID_CLIPBOARD_READ + 1);
    spin(&server, ID_PASTE_CONFIRM_SIZE_SPIN, 1);
    press_ok(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.paste.confirm, PasteConfirm::Multiline);
    assert_eq!(saved.paste.confirm_size, 9);
    assert_eq!(saved.clipboard.read, ClipboardRead::Allow);

    server.paste("rem third\r\nrem fourth\r\n");
    wait_dialog_of_class(&server, TEXT_DIALOG_CLASS);
    server.press_dialog_button(BUTTON_CANCEL);
    server.wait_dialogs(0);
}

/// 開いているダイアログにないコントロールと、押せないコントロールの操作は断る。
#[test]
fn dialog_requests_are_refused_for_what_a_user_cannot_touch() {
    let server = start_settings_test(ONE_PROFILE);
    let refused = |path: &str, body: &[u8]| {
        let (status, answer) = server.request("POST", path, body);
        assert_eq!(status, 400, "{path}: {}", String::from_utf8_lossy(&answer));
    };
    let (status, _) = server.request("GET", "/e2e/dialog/controls", b"");
    assert_eq!(status, 400, "no dialog is open");
    refused(&format!("/e2e/dialog/click/{ID_OK}"), b"");
    refused("/e2e/dialog/close", b"");

    open_settings(&server);
    // 隠れているページのコントロール。
    refused(&format!("/e2e/dialog/click/{ID_KEY_ADD}"), b"");
    // 無いコントロールと、クラスの違うコントロール。
    refused("/e2e/dialog/text/9999", b"x");
    refused(&format!("/e2e/dialog/text/{ID_OK}"), b"x");
    refused(&format!("/e2e/dialog/radio/{ID_OK}"), b"");
    refused(&format!("/e2e/dialog/select/{ID_SIZE}/0"), b"");
    // 並びの外の項目。
    refused(&format!("/e2e/dialog/select/{ID_PAGES}/6"), b"");
    show_page(&server, PAGE_PROFILES);
    // 一つしかないプロファイルは削除できない。ConPTY には場所がない。
    assert!(!control_enabled(&server, ID_PROFILE_REMOVE));
    refused(&format!("/e2e/dialog/click/{ID_PROFILE_REMOVE}"), b"");
    refused(&format!("/e2e/dialog/text/{ID_PROFILE_LOCATION}"), b"x");
    press_cancel(&server);
}

/// プロファイルを足し、名前とコマンドラインを変え、既定にして、次に開くタブから使う。削除すると先頭が既定に戻る。
#[test]
fn profiles_are_added_edited_made_default_and_removed() {
    let server = start_settings_test(&format!("# 残すコメント\n{ONE_PROFILE}"));
    open_settings(&server);
    show_page(&server, PAGE_PROFILES);
    assert_eq!(list_state(&server, ID_PROFILES), (1, Value::from(0)));

    click(&server, ID_PROFILE_ADD);
    assert_eq!(list_state(&server, ID_PROFILES), (2, Value::from(1)));
    let added = control_text(&server, ID_PROFILE_NAME);
    assert!(!added.is_empty());
    assert_eq!(read_controls(&server)["focus"], ID_PROFILE_NAME);
    assert!(control_enabled(&server, ID_PROFILE_REMOVE));
    // もう一つ足すと、名前は重ならない。
    click(&server, ID_PROFILE_ADD);
    assert_ne!(control_text(&server, ID_PROFILE_NAME), added);
    click(&server, ID_PROFILE_REMOVE);
    assert_eq!(list_state(&server, ID_PROFILES), (2, Value::from(1)));

    set_text(&server, ID_PROFILE_NAME, "  two  ");
    set_text(&server, ID_PROFILE_COMMAND, "cmd.exe /k prompt two$G");
    set_text(&server, ID_PROFILE_ICON, "two.png");
    // 既定にすると、選んでいる間は押せない。
    assert!(control_enabled(&server, ID_PROFILE_DEFAULT));
    click(&server, ID_PROFILE_DEFAULT);
    assert!(!control_enabled(&server, ID_PROFILE_DEFAULT));
    press_ok(&server);

    let saved = saved_config(&server);
    assert_eq!(saved.profiles.len(), 2);
    assert_eq!(saved.profiles[1].name, "two", "前後の空白を除く");
    assert_eq!(saved.profiles[1].command, "cmd.exe /k prompt two$G");
    assert_eq!(saved.profiles[1].icon.as_deref(), Some("two.png"));
    assert_eq!(saved.default_profile.as_deref(), Some("two"));
    assert!(config_text(&server).contains("# 残すコメント"));
    let state = server.state();
    assert_eq!(state["profiles"], serde_json::json!(["one", "two"]));

    // 既定のプロファイルで新しいタブが開く。
    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    let state = server.wait_text("two>");
    assert_eq!(state["tabs"][1]["profile"], "two", "{state:#}");
    server.post("/e2e/tab/close", b"");
    server.wait_until("one tab", |state| tab_count(state) == 1);

    // 既定のものを削除すると先頭が既定。
    open_settings(&server);
    show_page(&server, PAGE_PROFILES);
    assert_eq!(list_state(&server, ID_PROFILES), (2, Value::from(1)));
    click(&server, ID_PROFILE_REMOVE);
    press_ok(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.profiles.len(), 1);
    assert_eq!(saved.profiles[0].name, "one");
    assert_eq!(saved.default_profile_index(), 0);
    server.post("/e2e/tab/new", b"");
    let state = server.wait_until("two tabs", |state| tab_count(state) == 2);
    assert_eq!(state["tabs"][1]["profile"], "one", "{state:#}");
}

/// 空の名前、重なった名前、ConPTY の空のコマンドラインは、エラーの文を出してそのプロファイルを選び、書かない。
#[test]
fn profiles_with_empty_or_repeated_values_are_not_saved() {
    let original = format!(
        "{ONE_PROFILE}[[profiles]]\nname = \"two\"\ncommand = \"cmd.exe /k prompt e2e$G\"\n"
    );
    let server = start_settings_test(&original);
    open_settings(&server);
    show_page(&server, PAGE_PROFILES);

    let cases = [
        (ID_PROFILE_NAME, "", "two"),
        (ID_PROFILE_NAME, "   ", "two"),
        (ID_PROFILE_NAME, "one", "two"),
        (ID_PROFILE_COMMAND, "", "cmd.exe /k prompt e2e$G"),
    ];
    for (id, bad, good) in cases {
        select(&server, ID_PROFILES, 1);
        set_text(&server, id, bad);
        // ほかのプロファイルを選んでいても、エラーのあるプロファイルへ戻る。
        select(&server, ID_PROFILES, 0);
        press_apply(&server);
        let dialog = read_controls(&server);
        assert_ne!(control(&dialog, ID_ERROR)["text"], "", "{id} {bad:?}");
        assert_eq!(control(&dialog, ID_PROFILES)["selected"], 1, "{id} {bad:?}");
        assert_eq!(dialog["focus"], id, "{id} {bad:?}");
        assert_eq!(config_text(&server), original, "{id} {bad:?}");
        set_text(&server, id, good);
        press_apply(&server);
        assert_eq!(control_text(&server, ID_ERROR), "");
    }
    press_cancel(&server);
    assert_eq!(config_text(&server), original);
}

/// 不正な入力が見えていないページにあれば、「適用」でも「OK」でも、そのページへ移ってフォーカスする。
#[test]
fn an_invalid_input_on_a_hidden_page_is_shown_and_focused() {
    let original = format!(
        "{ONE_PROFILE}[paste]\nconfirm_size = 8\n\n[background]\nimage_opacity = 0.5\n\n[[background.images]]\npath = 'a.png'\n"
    );
    let server = start_settings_test_with_images(&original, &["a.png"]);
    open_settings(&server);

    let cases = [
        (PAGE_APPEARANCE, ID_SIZE, "12"),
        (PAGE_APPEARANCE, ID_PADDING, "4"),
        (PAGE_BACKGROUND, ID_BACKGROUND_OPACITY, "0.5"),
        (PAGE_BEHAVIOR, ID_PASTE_CONFIRM_SIZE, "8"),
        (PAGE_PROFILES, ID_PROFILE_COMMAND, "cmd.exe /k prompt e2e$G"),
    ];
    for (page, id, good) in cases {
        show_page(&server, page);
        let shown = |server: &Server, how: &str| {
            let dialog = read_controls(server);
            assert_ne!(control(&dialog, ID_ERROR)["text"], "", "{id} {how}");
            assert_eq!(control(&dialog, ID_PAGES)["selected"], page, "{id} {how}");
            assert_eq!(control(&dialog, id)["visible"], true, "{id} {how}");
            assert_eq!(dialog["focus"], id, "{id} {how}");
        };
        set_text(&server, id, "");
        show_page(&server, PAGE_COLORS);
        press_apply(&server);
        shown(&server, "applying");

        show_page(&server, PAGE_COLORS);
        click(&server, ID_OK);
        shown(&server, "pressing OK");
        assert_eq!(open_dialog_count(&server), 1, "{id}");

        set_text(&server, id, good);
        press_apply(&server);
        assert_eq!(control_text(&server, ID_ERROR), "", "{id}");
    }
    press_cancel(&server);
}

/// 接続方式を切り替えると、場所の入力欄が替わり、ブリッジの `command` に Windows のプログラムを書くと警告を出す。警告が
/// あっても保存できる。
#[test]
fn the_connection_method_changes_the_location_and_warns_about_windows_programs() {
    let server = start_settings_test(&format!(
        "{ONE_PROFILE}[[profiles]]\nname = \"bridge\"\ncommand = \"bash -l\"\n"
    ));
    open_settings(&server);
    show_page(&server, PAGE_PROFILES);
    select(&server, ID_PROFILES, 1);
    assert!(!control_enabled(&server, ID_PROFILE_LOCATION));
    let without_warning = read_controls(&server);
    assert_eq!(
        control(&without_warning, ID_PROFILE_COMMAND_WARNING)["text"],
        ""
    );

    // Cygwin: 場所は Cygwin のフォルダ。`cmd` のような Windows のプログラムで警告を出す。
    select(&server, ID_PROFILE_BACKEND, BACKEND_CYGWIN);
    assert!(control_enabled(&server, ID_PROFILE_LOCATION));
    set_text(&server, ID_PROFILE_LOCATION, r"C:\cygwin-of-the-test");
    set_text(&server, ID_PROFILE_COMMAND, "cmd /k");
    let warned = read_controls(&server);
    assert_ne!(control(&warned, ID_PROFILE_COMMAND_WARNING)["text"], "");
    // 警告の高さの分だけ、下のコントロールが下がる。
    assert!(
        number_of(control(&warned, ID_PROFILE_ICON), "y")
            > number_of(control(&without_warning, ID_PROFILE_ICON), "y"),
        "{warned:#}"
    );
    assert_controls_fit(&warned, PAGE_PROFILES);
    press_apply(&server);
    assert_eq!(control_text(&server, ID_ERROR), "");
    let saved = saved_config(&server);
    assert_eq!(saved.profiles[1].backend, BackendKind::Cygwin);
    assert_eq!(saved.profiles[1].command, "cmd /k");
    assert_eq!(
        saved.profiles[1].cygwin_root.as_deref(),
        Some(r"C:\cygwin-of-the-test")
    );

    // ブリッジではコマンドラインが空でよい。警告は消える。
    set_text(&server, ID_PROFILE_COMMAND, "");
    assert_eq!(control_text(&server, ID_PROFILE_COMMAND_WARNING), "");
    assert_eq!(
        number_of(control(&read_controls(&server), ID_PROFILE_ICON), "y"),
        number_of(control(&without_warning, ID_PROFILE_ICON), "y")
    );
    press_apply(&server);
    assert_eq!(control_text(&server, ID_ERROR), "");
    assert_eq!(saved_config(&server).profiles[1].command, "");

    // WSL: 切り替えると場所は空に戻り、ディストリビューションになる。`.exe` で警告を出す。
    select(&server, ID_PROFILE_BACKEND, BACKEND_WSL);
    assert_eq!(control_text(&server, ID_PROFILE_LOCATION), "");
    set_text(&server, ID_PROFILE_LOCATION, "Ubuntu");
    set_text(&server, ID_PROFILE_COMMAND, "docker.exe ps");
    assert_ne!(control_text(&server, ID_PROFILE_COMMAND_WARNING), "");
    press_apply(&server);
    assert_eq!(control_text(&server, ID_ERROR), "");
    let saved = saved_config(&server);
    assert_eq!(saved.profiles[1].backend, BackendKind::Wsl);
    assert_eq!(saved.profiles[1].distribution.as_deref(), Some("Ubuntu"));
    assert_eq!(saved.profiles[1].cygwin_root, None);

    // ConPTY: 場所は押せなくなり、空のコマンドラインはエラー。
    select(&server, ID_PROFILE_BACKEND, BACKEND_CONPTY);
    assert!(!control_enabled(&server, ID_PROFILE_LOCATION));
    assert_eq!(control_text(&server, ID_PROFILE_COMMAND_WARNING), "");
    set_text(&server, ID_PROFILE_COMMAND, "");
    press_apply(&server);
    assert_ne!(control_text(&server, ID_ERROR), "");
    set_text(&server, ID_PROFILE_COMMAND, "cmd.exe /k prompt bridge$G");
    press_ok(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.profiles[1].backend, BackendKind::Conpty);
    assert_eq!(saved.profiles[1].distribution, None);
    assert!(!config_text(&server).contains("distribution"));
}

/// 背景のページの項目の表示方法と不透明度、既定の値が、端末の絵と `[[background.images]]` に入る。
#[test]
fn the_background_page_changes_the_items_and_the_picture() {
    let server = start_with_background(
        &format!(
            "[background]\nimage_opacity = 1.0\nfit = \"stretch\"\n\n# 項目のコメント\n[[background.images]]\npath = '{BANDS_IMAGE}'\n"
        ),
        |_| {},
    );
    let state = server.state();
    let area = BackdropArea::of(&state);
    let top = area.at(0.5, 0.01);
    let bottom = area.at(0.5, 0.99);
    wait_pixels(&server, &[(top, BAND_TOP), (bottom, BAND_BOTTOM)]);

    open_settings(&server);
    show_page(&server, PAGE_BACKGROUND);
    // 選ぶまでは項目の値を触れない。
    assert!(!control_enabled(&server, ID_BACKGROUND_ITEM_FIT));
    assert!(!control_enabled(&server, ID_BACKGROUND_REMOVE));
    select(&server, ID_BACKGROUND_IMAGES, 0);
    assert!(control_enabled(&server, ID_BACKGROUND_ITEM_FIT));
    assert!(!control_enabled(&server, ID_BACKGROUND_ITEM_OPACITY));

    // 既定の不透明度は、打ってから少し待って反映する。
    set_text(&server, ID_BACKGROUND_OPACITY, "0.5");
    wait_pixels(&server, &[(top, over_terminal_background(BAND_TOP, 0.5))]);
    // 項目の不透明度は既定より優先する。
    choose_radio(&server, ID_BACKGROUND_ITEM_OPACITY_CUSTOM);
    assert!(control_enabled(&server, ID_BACKGROUND_ITEM_OPACITY));
    set_text(&server, ID_BACKGROUND_ITEM_OPACITY, "0.25");
    wait_pixels(&server, &[(top, over_terminal_background(BAND_TOP, 0.25))]);
    // 項目の表示方法で、元の大きさで真ん中に置く。上の端に画像がなくなる。
    select(&server, ID_BACKGROUND_ITEM_FIT, ITEM_FIT_CENTER);
    wait_pixels(&server, &[(top, TERMINAL_BACKGROUND)]);
    let rows = read_controls(&server);
    let row = &control(&rows, ID_BACKGROUND_IMAGES)["items"][0];
    assert!(
        row.as_str().is_some_and(|row| row.contains("0.25")),
        "{row}"
    );
    press_apply(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.background.image_opacity, 0.5);
    assert_eq!(saved.background.images[0].fit, Some(BackgroundFit::Center));
    assert_eq!(saved.background.images[0].image_opacity, Some(0.25));
    assert!(config_text(&server).contains("# 項目のコメント"));

    // 項目の値を既定に戻すと、テーブルからキーが消え、既定の値で描く。
    select(&server, ID_BACKGROUND_ITEM_FIT, ITEM_FIT_DEFAULT);
    choose_radio(&server, ID_BACKGROUND_ITEM_OPACITY_DEFAULT);
    assert!(!control_enabled(&server, ID_BACKGROUND_ITEM_OPACITY));
    assert_eq!(control_text(&server, ID_BACKGROUND_ITEM_OPACITY), "");
    wait_pixels(&server, &[(top, over_terminal_background(BAND_TOP, 0.5))]);
    press_apply(&server);
    let saved = saved_config(&server);
    assert_eq!(
        saved.background.images[0],
        BackgroundImage::new(BANDS_IMAGE)
    );

    // 一覧から外すと画像がなくなる。
    click(&server, ID_BACKGROUND_REMOVE);
    assert_eq!(list_state(&server, ID_BACKGROUND_IMAGES).0, 0);
    assert!(!control_enabled(&server, ID_BACKGROUND_REMOVE));
    server.wait_until("no background image", |state| {
        state["background"]["image"].is_null()
    });
    wait_pixels(&server, &[(top, TERMINAL_BACKGROUND)]);
    press_ok(&server);
    let saved = saved_config(&server);
    assert!(saved.background.images.is_empty());
    assert!(!config_text(&server).contains("[[background.images]]"));
}

/// 背景画像の一覧は、項目ごとの値を項目に入れ、ほかの項目は変えない。
#[test]
fn the_background_list_removes_only_the_chosen_item() {
    let server = start_settings_test_with_images(
        &format!(
            "{ONE_PROFILE}[[background.images]]\npath = 'a.png'\nfit = \"tile\"\n\n[[background.images]]\npath = 'b.png'\n\n[[background.images]]\npath = 'c.png'\nimage_opacity = 0.75\n"
        ),
        &["a.png", "b.png", "c.png"],
    );
    open_settings(&server);
    show_page(&server, PAGE_BACKGROUND);
    assert_eq!(list_state(&server, ID_BACKGROUND_IMAGES).0, 3);
    select(&server, ID_BACKGROUND_IMAGES, 1);
    click(&server, ID_BACKGROUND_REMOVE);
    // 同じ位置の項目を選び続ける。
    let (count, selected) = list_state(&server, ID_BACKGROUND_IMAGES);
    assert_eq!((count, selected), (2, Value::from(1)));
    assert_eq!(
        control(&read_controls(&server), ID_BACKGROUND_ITEM_OPACITY)["text"],
        "0.75"
    );
    press_ok(&server);
    let saved = saved_config(&server);
    let paths: Vec<&str> = saved
        .background
        .images
        .iter()
        .map(|image| image.path.as_str())
        .collect();
    assert_eq!(paths, ["a.png", "c.png"]);
    assert_eq!(saved.background.images[0].fit, Some(BackgroundFit::Tile));
    assert_eq!(saved.background.images[1].image_opacity, Some(0.75));
}

/// OS のファイルを選ぶダイアログは、やめれば何も変えない。
#[test]
fn cancelling_the_file_dialogs_changes_nothing() {
    let original = format!("{ONE_PROFILE}[[background.images]]\npath = 'a.png'\n");
    let server = start_settings_test_with_images(&original, &["a.png"]);
    open_settings(&server);
    for (page, id, list) in [
        (
            PAGE_BACKGROUND,
            ID_BACKGROUND_ADD_FILES,
            ID_BACKGROUND_IMAGES,
        ),
        (
            PAGE_BACKGROUND,
            ID_BACKGROUND_ADD_FOLDER,
            ID_BACKGROUND_IMAGES,
        ),
        (PAGE_PROFILES, ID_PROFILE_ICON_CHOOSE, ID_PROFILES),
    ] {
        show_page(&server, page);
        let before = list_state(&server, list);
        click(&server, id);
        wait_top_dialog(&server, SYSTEM_DIALOG_CLASS);
        click(&server, ID_CANCEL);
        wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
        assert_eq!(list_state(&server, list), before, "{id}");
    }
    assert_eq!(control_text(&server, ID_PROFILE_ICON), "");
    press_ok(&server);
    assert_eq!(config_text(&server), original);
}

/// 環境変数があるときだけ、ファイルを選ぶダイアログで実際に選ぶ。OS がダイアログの履歴（レジストリの
/// `LastVisitedPidlMRU`）に一時フォルダを残すので、手元では付けない（CI で付ける）。
const FILE_DIALOG_ENV: &str = "FANTERMO_E2E_FILE_DIALOG";

/// いちばん上のダイアログの、最初の入力欄（ファイルを選ぶダイアログのファイル名の欄）の ID。
#[track_caller]
fn first_edit_id(server: &Server) -> i32 {
    let dialog = read_controls(server);
    let edit = dialog["controls"]
        .as_array()
        .and_then(|controls| {
            controls
                .iter()
                .find(|control| control["class"] == "Edit" && control["visible"] == true)
        })
        .unwrap_or_else(|| panic!("no edit control: {dialog:#}"));
    i32::try_from(edit["id"].as_i64().expect("no ID")).expect("the ID fits")
}

/// `button` で開くファイルを選ぶダイアログで `path` を選ぶ。
#[track_caller]
fn choose_file(server: &Server, button: i32, path: &std::path::Path) {
    click(server, button);
    wait_top_dialog(server, SYSTEM_DIALOG_CLASS);
    set_text(
        server,
        first_edit_id(server),
        path.to_str().expect("a UTF-8 path"),
    );
    click(server, ID_OK);
    wait_top_dialog(server, SETTINGS_DIALOG_CLASS);
}

/// ファイルを選ぶダイアログで選んだ画像とアイコンが、絶対パスで入る。同じ画像は足さない。
#[test]
fn the_file_dialogs_add_a_background_image_and_a_profile_icon() {
    let test = "the_file_dialogs_add_a_background_image_and_a_profile_icon";
    if std::env::var_os(FILE_DIALOG_ENV).is_none() {
        common::report_skip(test, FILE_DIALOG_ENV);
        return;
    }
    let server = start_settings_test_with_images(ONE_PROFILE, &["added.png"]);
    let image = server.dir().join("added.png");
    open_settings(&server);
    show_page(&server, PAGE_BACKGROUND);
    choose_file(&server, ID_BACKGROUND_ADD_FILES, &image);
    assert_eq!(
        list_state(&server, ID_BACKGROUND_IMAGES),
        (1, Value::from(0))
    );
    choose_file(&server, ID_BACKGROUND_ADD_FILES, &image);
    assert_eq!(list_state(&server, ID_BACKGROUND_IMAGES).0, 1);

    show_page(&server, PAGE_PROFILES);
    choose_file(&server, ID_PROFILE_ICON_CHOOSE, &image);
    press_ok(&server);
    let saved = saved_config(&server);
    // ダイアログは長い名前のパスを返し、一時フォルダは短い名前（`RUNNER~1` など）のことがあるので、
    // 同じファイルを指すかで比べる。
    let same_file = |path: &str| {
        std::fs::canonicalize(path).expect("the written path exists")
            == std::fs::canonicalize(&image).expect("the image exists")
    };
    assert!(
        same_file(&saved.background.images[0].path),
        "{}",
        saved.background.images[0].path
    );
    let icon = saved.profiles[0]
        .icon
        .as_deref()
        .expect("an icon is written");
    assert!(same_file(icon), "{icon}");
    assert!(std::path::Path::new(icon).is_absolute(), "{icon}");
}

/// 色のページの配色と、色を選ぶダイアログ、既定に戻すが、設定ファイルと端末の絵に入る。
#[test]
fn the_colors_page_changes_the_scheme_and_the_colors() {
    let server = start_settings_test(ONE_PROFILE);
    let state = server.state();
    let at = cell_pixel(&state, EMPTY_COL, EMPTY_ROWS_BELOW, 0, 0);
    wait_pixels(&server, &[(at, [0, 0, 0])]);
    let choose = color_control(COLOR_ROW_BACKGROUND, COLOR_CHOOSE);
    let reset = color_control(COLOR_ROW_BACKGROUND, COLOR_RESET);
    let value = color_control(COLOR_ROW_BACKGROUND, COLOR_TEXT);

    open_settings(&server);
    show_page(&server, PAGE_COLORS);
    assert!(!control_enabled(&server, reset));

    // 配色はすぐ反映する。
    choose_radio(&server, ID_COLOR_SCHEME + 1);
    wait_pixels(&server, &[(at, [255, 255, 255])]);

    // 色を選ぶダイアログをやめても変わらない。
    click(&server, choose);
    wait_top_dialog(&server, SYSTEM_DIALOG_CLASS);
    click(&server, ID_CANCEL);
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    assert!(!control_enabled(&server, reset));

    // 選ぶと入る。
    click(&server, choose);
    wait_top_dialog(&server, SYSTEM_DIALOG_CLASS);
    set_text(&server, COLOR_DIALOG_RED, "16");
    set_text(&server, COLOR_DIALOG_GREEN, "32");
    set_text(&server, COLOR_DIALOG_BLUE, "48");
    click(&server, ID_OK);
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    wait_pixels(&server, &[(at, TERMINAL_BACKGROUND)]);
    assert!(control_text(&server, value).contains("#102030"));
    assert!(control_enabled(&server, reset));
    press_apply(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.colors.scheme, ColorScheme::Light);
    assert_eq!(
        saved.colors.terminal[0],
        Some(Rgb {
            r: 16,
            g: 32,
            b: 48
        })
    );

    // 既定に戻すと配色の色になる。
    click(&server, reset);
    wait_pixels(&server, &[(at, [255, 255, 255])]);
    assert!(!control_enabled(&server, reset));
    press_ok(&server);
    assert_eq!(saved_config(&server).colors.terminal[0], None);
    assert!(config_text(&server).contains("# background = \"#102030\""));
}

/// `action` の行を選ぶ。
#[track_caller]
fn select_key_row(server: &Server, keys: &Keymap, action: Action, combo: Option<&str>) {
    select(server, ID_KEYS, key_row_of(keys, action, combo));
}

/// 「追加」か「変更」のボタン `button` を押して、キーを読み取るダイアログで `combo` を押す。
#[track_caller]
fn capture_key(server: &Server, button: i32, combo: &str) {
    click(server, button);
    wait_top_dialog(server, KEY_CAPTURE_CLASS);
    press_key_in_dialog(server, combo);
}

/// キーの割り当てを足し、変え、外し、既定に戻す。Esc なら読み取りをやめる。
#[test]
fn the_keys_page_adds_changes_removes_and_resets_bindings() {
    let server = start_settings_test(ONE_PROFILE);
    open_settings(&server);
    show_page(&server, PAGE_KEYS);
    let default = Keymap::default();
    let rows = list_state(&server, ID_KEYS).0;

    // やめても変わらない。
    select_key_row(&server, &default, Action::NewTab, None);
    capture_key(&server, ID_KEY_ADD, "escape");
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    assert_eq!(list_state(&server, ID_KEYS).0, rows);

    // 修飾キーのある組み合わせを足す。
    capture_key(&server, ID_KEY_ADD, "ctrl+shift+n");
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    assert_eq!(list_state(&server, ID_KEYS).0, rows + 1);
    assert!(control_enabled(&server, ID_KEY_RESET));
    press_ok(&server);
    let keys = saved_config(&server).keys;
    let new_combo = KeyCombo::parse("ctrl+shift+n").expect("a valid combination");
    assert_eq!(keys.lookup_combo(new_combo), Some(Action::NewTab));
    assert!(config_text(&server).contains("\"ctrl+shift+n\" = \"new_tab\""));

    // 足した組み合わせは端末で動く。
    server.press("ctrl+shift+n");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    server.press("ctrl+shift+w");
    server.wait_until("one tab", |state| tab_count(state) == 1);

    // 修飾キーのない組み合わせに変える。
    open_settings(&server);
    show_page(&server, PAGE_KEYS);
    select_key_row(&server, &keys, Action::NewTab, Some("ctrl+shift+n"));
    capture_key(&server, ID_KEY_CHANGE, "f5");
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    press_apply(&server);
    let keys = saved_config(&server).keys;
    assert_eq!(keys.lookup_combo(new_combo), None);
    let f5 = KeyCombo::parse("f5").expect("a valid combination");
    assert_eq!(keys.lookup_combo(f5), Some(Action::NewTab));
    assert!(!config_text(&server).contains("ctrl+shift+n"));

    // 外す。組み合わせがなくなると、変更と外すは押せない。
    select_key_row(&server, &keys, Action::CloseTab, Some("ctrl+shift+w"));
    click(&server, ID_KEY_REMOVE);
    assert!(!control_enabled(&server, ID_KEY_CHANGE));
    assert!(!control_enabled(&server, ID_KEY_REMOVE));
    press_apply(&server);
    let close_combo = KeyCombo::parse("ctrl+shift+w").expect("a valid combination");
    assert_eq!(saved_config(&server).keys.lookup_combo(close_combo), None);
    assert!(config_text(&server).contains("\"ctrl+shift+w\" = \"none\""));

    // 既定に戻す。動作ごとに戻し、戻したら押せない。
    let keys = saved_config(&server).keys;
    select_key_row(&server, &keys, Action::CloseTab, None);
    assert!(control_enabled(&server, ID_KEY_RESET));
    click(&server, ID_KEY_RESET);
    assert!(!control_enabled(&server, ID_KEY_RESET));
    select_key_row(&server, &saved_config(&server).keys, Action::NewTab, None);
    click(&server, ID_KEY_RESET);
    press_ok(&server);
    let keys = saved_config(&server).keys;
    assert_eq!(keys, Keymap::default());
    assert!(!config_text(&server).contains("f5"));
    assert!(config_text(&server).contains("\"ctrl+shift+w\""));
    server.press("ctrl+shift+t");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
}

/// ほかの動作に割り当てた組み合わせは、割り当て直すかを尋ね、「いいえ」なら変えず、「はい」なら移す。
#[test]
fn a_key_bound_to_another_action_asks_before_moving_it() {
    let server = start_settings_test(ONE_PROFILE);
    open_settings(&server);
    show_page(&server, PAGE_KEYS);
    let default = Keymap::default();
    let close_combo = KeyCombo::parse("ctrl+shift+w").expect("a valid combination");
    let rows = list_state(&server, ID_KEYS).0;
    select_key_row(&server, &default, Action::NewTab, None);

    capture_key(&server, ID_KEY_ADD, "ctrl+shift+w");
    wait_message_box(&server);
    // メッセージボックスが開いている間は、設定ダイアログは無効。
    assert_eq!(read_controls(&server)["class"], SYSTEM_DIALOG_CLASS);
    click(&server, ID_NO);
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    assert_eq!(list_state(&server, ID_KEYS).0, rows);
    press_apply(&server);
    assert_eq!(saved_config(&server).keys, Keymap::default());

    // 同じ動作の同じ組み合わせは尋ねない。
    capture_key(&server, ID_KEY_ADD, "ctrl+shift+t");
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    assert_eq!(list_state(&server, ID_KEYS).0, rows);

    capture_key(&server, ID_KEY_ADD, "ctrl+shift+w");
    wait_message_box(&server);
    click(&server, ID_YES);
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    press_ok(&server);
    let keys = saved_config(&server).keys;
    assert_eq!(keys.lookup_combo(close_combo), Some(Action::NewTab));
    assert!(keys.combos_of(Action::CloseTab).is_empty());

    // 移した組み合わせで、タブが開く。
    server.press("ctrl+shift+w");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
}

/// ダイアログの間も端末の出力を処理して描き、閉じたあとも入力できる。ダイアログの間、ウィンドウは無効になる。
#[test]
fn the_terminal_keeps_working_while_the_settings_dialog_is_open() {
    let server = start_settings_test(ONE_PROFILE);
    open_settings(&server);
    assert_eq!(server.state()["window_enabled"], false);

    server.send_line("echo during-%ERRORLEVEL%-dialog");
    let state = server.wait_text("during-0-dialog");
    let (col, row) = find_cell(&state, "during-0-dialog");
    assert_cell_is_drawn(&server, &state, col, row);

    // 裏のタブの出力も届く。
    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    server.wait_text(CMD_PROMPT);
    server.post("/e2e/tab/select/0", b"");
    server.wait_text("during-0-dialog");

    press_cancel(&server);
    let state = server.state();
    assert_eq!(state["window_enabled"], true, "{state:#}");
    check_terminal_still_works(&server, "after-settings");
}

/// ダイアログの間に最後のシェルが終わると、ダイアログごとウィンドウが閉じる。
#[test]
fn a_shell_ending_while_the_settings_dialog_is_open_closes_the_window() {
    let mut server = start_settings_test(ONE_PROFILE);
    open_settings(&server);
    server.send_line("exit");
    server.wait_exit();
}

/// 開いている間に別のプログラムが書き換えた設定は、キャンセルで閉じたあとに読み直す。
#[test]
fn a_file_changed_while_the_dialog_is_open_is_read_after_closing() {
    let server = start_settings_test(&format!("{ONE_PROFILE}[window]\npadding = 4\n"));
    let first_x = number(&server.state()["terminal_x"]);
    open_settings(&server);
    std::fs::write(
        config_path(&server),
        format!("{ONE_PROFILE}[window]\npadding = 10\n"),
    )
    .expect("failed to write the settings");
    let unchanged = Duration::from_millis(600);
    server.assert_holds_for(unchanged, "the padding before closing", |state| {
        number(&state["terminal_x"]) == first_x
    });
    press_cancel(&server);
    let expected = first_x + 6;
    server.wait_until("the padding of the file", |state| {
        number(&state["terminal_x"]) == expected
    });
}

/// 開いている間に別のプログラムが書き換えた設定は、保存したときも閉じたあとに反映する。ダイアログで変えた項目も残る。
#[test]
fn a_file_changed_while_the_dialog_is_open_is_read_after_saving() {
    let server = start_settings_test(&format!("{ONE_PROFILE}[window]\npadding = 4\n"));
    let first_x = number(&server.state()["terminal_x"]);

    // ダイアログで何も変えずに「OK」を押す。
    open_settings(&server);
    std::fs::write(
        config_path(&server),
        format!("{ONE_PROFILE}[window]\npadding = 10\n"),
    )
    .expect("failed to write the settings");
    press_ok(&server);
    server.wait_until("the padding of the file", |state| {
        number(&state["terminal_x"]) == first_x + 6
    });

    // ダイアログでほかの項目を変えて「OK」を押す。
    let bar = number(&server.state()["scroll_bar"]["width"]);
    open_settings(&server);
    std::fs::write(
        config_path(&server),
        format!("{ONE_PROFILE}[window]\npadding = 12\n"),
    )
    .expect("failed to write the settings");
    set_text(&server, ID_SCROLL_BAR_WIDTH, &(bar + 5).to_string());
    press_ok(&server);
    server.wait_until("the padding of the file and the width", |state| {
        number(&state["terminal_x"]) == first_x + 8
            && number(&state["scroll_bar"]["width"]) == bar + 5
    });
    let saved = saved_config(&server);
    assert_eq!(saved.window.padding, 12);
    assert_eq!(u64::from(saved.scroll_bar.width), bar + 5);

    // 「適用」のあとは、開いたままで反映せず、「キャンセル」で閉じたあとに反映する。
    open_settings(&server);
    press_apply(&server);
    std::fs::write(
        config_path(&server),
        format!("{ONE_PROFILE}[window]\npadding = 14\n"),
    )
    .expect("failed to write the settings");
    server.assert_holds_for(Duration::from_millis(600), "the padding", |state| {
        number(&state["terminal_x"]) == first_x + 8
    });
    press_cancel(&server);
    server.wait_until("the padding of the file", |state| {
        number(&state["terminal_x"]) == first_x + 10
    });
}

/// 設定ファイルの警告を、続けて出さない時間の窓。
const NO_REPEATED_WARNING: Duration = Duration::from_millis(600);

/// メッセージボックスが開かないことを確認する。
#[track_caller]
fn assert_no_warning_opens(server: &Server) {
    server.assert_holds_for(NO_REPEATED_WARNING, "no warning", |state| {
        state["dialogs"].as_array().map_or(0, Vec::len) == 0
    });
}

/// 同じ警告は、保存しても別のプログラムが書き換えても続けて出さない。中身が変わったら出し、直ったあとにまた
/// 同じ警告になれば出す。
#[test]
fn the_same_settings_warning_is_not_shown_again() {
    let first = format!("unknown_one = 1\n{ONE_PROFILE}");
    let server = Server::start(&first, &[]);
    assert!(wait_message_box(&server).contains("unknown_one"));
    close_message_box(&server, 0);
    server.wait_text(CMD_PROMPT);

    // 「OK」のたびに出さない。項目を書いて行がずれても、中身は同じ。
    open_settings(&server);
    press_ok(&server);
    assert_no_warning_opens(&server);
    open_settings(&server);
    set_text(&server, ID_PADDING, "9");
    press_ok(&server);
    assert_no_warning_opens(&server);
    assert_eq!(saved_config(&server).window.padding, 9);
    open_settings(&server);
    press_apply(&server);
    press_cancel(&server);
    assert_no_warning_opens(&server);

    // 別のプログラムが書き換えても、警告が同じなら出さない。
    let padded = number(&server.state()["terminal_x"]) + 1;
    std::fs::write(
        config_path(&server),
        format!("# a comment\n\nunknown_one = 1\n{ONE_PROFILE}[window]\npadding = 10\n"),
    )
    .expect("failed to write the settings");
    server.wait_until("the changed padding", |state| {
        number(&state["terminal_x"]) == padded
    });
    assert_no_warning_opens(&server);

    // 中身が変わったら出す。
    std::fs::write(
        config_path(&server),
        format!("unknown_two = 1\n{ONE_PROFILE}"),
    )
    .expect("failed to write the settings");
    assert!(wait_message_box(&server).contains("unknown_two"));
    close_message_box(&server, 0);

    // ダイアログの間に中身が変わったら、閉じたあとに出す。
    open_settings(&server);
    std::fs::write(
        config_path(&server),
        format!("unknown_three = 1\n{ONE_PROFILE}"),
    )
    .expect("failed to write the settings");
    click(&server, ID_OK);
    assert!(wait_message_box(&server).contains("unknown_three"));
    close_message_box(&server, 0);

    // 直ったあとに同じ警告になれば出す。
    std::fs::write(config_path(&server), ONE_PROFILE).expect("failed to write the settings");
    let width = number(&server.state()["scroll_bar"]["width"]);
    open_settings(&server);
    set_text(&server, ID_SCROLL_BAR_WIDTH, &(width + 1).to_string());
    press_ok(&server);
    server.wait_until("the changed width", |state| {
        number(&state["scroll_bar"]["width"]) == width + 1
    });
    std::fs::write(
        config_path(&server),
        format!("unknown_three = 1\n{ONE_PROFILE}"),
    )
    .expect("failed to write the settings");
    assert!(wait_message_box(&server).contains("unknown_three"));
    close_message_box(&server, 0);
}

/// 読めない設定ファイルは、起動時にメッセージで知らせて既定の設定で動く。
#[test]
fn a_settings_file_that_cannot_be_read_is_reported_at_startup() {
    let server = Server::start_with_setup("", &CMD_COMMAND, |dir| {
        let path = dir.join("config.toml");
        std::fs::remove_file(&path).expect("failed to remove the settings");
        std::fs::create_dir(&path).expect("failed to create the folder");
    });
    let text = wait_message_box(&server);
    assert!(text.contains("config.toml"), "{text}");
    close_message_box(&server, 0);
    server.wait_text(CMD_PROMPT);
    check_terminal_still_works(&server, "after-the-message");
}

/// `config.toml` を、`share` で共有して開いたまま `body` を実行する。
fn with_settings_file_open<T>(server: &Server, share: u32, body: impl FnOnce() -> T) -> T {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(share)
        .open(config_path(server))
        .expect("failed to open the settings");
    let result = body();
    drop(file);
    result
}

/// メッセージが開き、設定ダイアログは開いたまま、ファイルと、一時ファイルが残らないことを確認して閉じる。
#[track_caller]
fn assert_save_failed(server: &Server) {
    let text = wait_message_box(server);
    assert!(text.contains("config.toml"), "{text}");
    close_message_box(server, 1);
    wait_top_dialog(server, SETTINGS_DIALOG_CLASS);
    assert!(!server.dir().join("config.toml.tmp").exists());
}

/// 読めない・書けないファイルへの保存はメッセージを出し、ダイアログは開いたままで、やり直せる。
#[test]
fn saving_to_a_file_that_cannot_be_read_or_written_shows_a_message_and_can_be_retried() {
    /// 共有モード: 共有しない（読めない）。
    const SHARE_NONE: u32 = 0;
    /// 共有モード: 読むことだけ共有する（読めるが、置き換えられない）。
    const SHARE_READ: u32 = 1;
    let original = format!("{ONE_PROFILE}[window]\npadding = 4\n");
    let server = start_settings_test(&original);
    open_settings(&server);
    set_text(&server, ID_PADDING, "10");

    for share in [SHARE_NONE, SHARE_READ] {
        with_settings_file_open(&server, share, || {
            click(&server, ID_APPLY);
            assert_save_failed(&server);
        });
        assert_eq!(config_text(&server), original, "{share}");
        // 端末には編集中の値を見せたまま。
        assert_eq!(control_text(&server, ID_PADDING), "10");
    }

    // 読み取り専用のファイルは置き換えられない。
    let path = config_path(&server);
    let set_read_only = |read_only: bool| {
        let mut permissions = std::fs::metadata(&path)
            .expect("failed to read the attributes")
            .permissions();
        permissions.set_readonly(read_only);
        std::fs::set_permissions(&path, permissions).expect("failed to set the attributes");
    };
    set_read_only(true);
    click(&server, ID_APPLY);
    assert_save_failed(&server);
    set_read_only(false);
    assert_eq!(config_text(&server), original);

    // 開けるようになれば入る。
    press_apply(&server);
    assert_eq!(saved_config(&server).window.padding, 10);

    // TOML として読めないときも同じ。閉じたあとの読み直しでも知らせる。
    std::fs::write(config_path(&server), "[font\n").expect("failed to write the settings");
    set_text(&server, ID_PADDING, "12");
    click(&server, ID_APPLY);
    assert_save_failed(&server);
    assert_eq!(config_text(&server), "[font\n");
    click(&server, ID_CANCEL);
    wait_message_box(&server);
    close_message_box(&server, 0);
    std::fs::write(config_path(&server), &original).expect("failed to write the settings");
    let expected = number(&server.state()["terminal_x"]) - 6;
    server.wait_until("the padding of the repaired file", |state| {
        number(&state["terminal_x"]) == expected
    });
}

/// 開いている間に消された設定ファイルは、ひな形に変えた項目を書いて作る。
#[test]
fn saving_after_the_file_was_removed_creates_it_from_the_template() {
    let server = start_settings_test(&format!("{ONE_PROFILE}[window]\npadding = 4\n"));
    open_settings(&server);
    std::fs::remove_file(config_path(&server)).expect("failed to remove the settings");
    set_text(&server, ID_PADDING, "10");
    press_ok(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.window.padding, 10);
    assert!(config_text(&server).contains("[colors.tab_bar]"));
}

/// 「fantermo について」の「更新を確認」。
const ID_CHECK_UPDATE: i32 = 101;
/// fantermo が取得するパス。
const MANIFEST_PATH: &str = "/xia-sava/fantermo/releases/download/latest/latest.json";
const SIGNATURE_PATH: &str = "/xia-sava/fantermo/releases/download/latest/latest.json.sig";
const MSI_PATH: &str = "/xia-sava/fantermo/releases/download/latest/fantermo.msi";
/// 受け取る大きさの上限（`update.rs`）。
const MANIFEST_MAX_BYTES: usize = 64 * 1024;
const SIGNATURE_MAX_BYTES: usize = 1024;
const MSI_MAX_BYTES: usize = 200 * 1024 * 1024;
/// 配布物のダミー。
const DUMMY_MSI: &[u8] = b"not a real installer";
/// いまのバージョンとして扱うもの。比べる相手は桁の増える `0.0.9` と `0.0.11` など。
const CURRENT_VERSION: &str = "0.0.10";
const NEWER_VERSION: &str = "0.0.11";
/// 一時フォルダの中の、fantermo の `TMP` にするフォルダ。ダウンロードした配布物はここに置かれる。
const UPDATE_TEMP_FOLDER: &str = "tmp";
/// ダウンロードした配布物を置くフォルダの名前の前半。
const DOWNLOAD_FOLDER_PREFIX: &str = "fantermo-update-";
/// 一時フォルダの中の、スクリプトを起動する代わりにそのパスが書かれるファイル。
const HANDOFF_RECORD: &str = "handoff.txt";
/// メッセージボックスの文。
const OFFER_TEXT: &str = "ダウンロードして更新しますか";
const UP_TO_DATE_TEXT: &str = "最新バージョンです";
const BAD_SIGNATURE_TEXT: &str = "更新情報の署名を確認できませんでした";
const UNREADABLE_MANIFEST_TEXT: &str = "更新情報を読み取れませんでした";
const CANNOT_FETCH_TEXT: &str = "更新情報を取得できませんでした";
const NETWORK_TEXT: &str = "ネットワークの接続を確認してください";
const CANNOT_DOWNLOAD_TEXT: &str = "新しいバージョンをダウンロードできませんでした";
const DIGEST_MISMATCH_TEXT: &str = "ダウンロードしたファイルが、更新情報と一致しませんでした";
const CANNOT_START_TEXT: &str = "更新を始められませんでした";
const OTHER_INSTANCES_TEXT: &str = "ほかに fantermo が";
const CONFIRM_APPLY_TEXT: &str = "すべてのタブを閉じて";
/// 何も起きないことを見る時間の窓。
const NO_REQUEST_WINDOW: Duration = Duration::from_secs(3);

/// テストの配布元。HTTP と、署名する鍵。
struct ReleaseSource {
    host: ReleaseHost,
    key: SigningKey,
}

impl ReleaseSource {
    fn new() -> Self {
        Self {
            host: ReleaseHost::start(),
            key: SigningKey::generate(),
        }
    }

    /// いまのバージョンを `current` として fantermo を起動する。署名はこの配布元の鍵で検証する。
    fn start_fantermo(&self, current: &str) -> Server {
        start_update_test(self.host.port(), current, Some(&self.key.public_key()))
    }

    /// 更新の情報 `manifest` と、この配布元の鍵の署名を置く。
    fn publish_manifest(&self, manifest: &[u8]) {
        self.host.set(MANIFEST_PATH, Reply::body(manifest));
        self.host
            .set(SIGNATURE_PATH, Reply::body(self.key.sign(manifest)));
    }

    /// バージョン `version` の配布物 `msi` を、更新の情報と署名とあわせて置く。
    fn publish(&self, version: &str, msi: &[u8]) {
        self.publish_manifest(manifest_for(version, &sha256_hex(msi)).as_bytes());
        self.host.set(MSI_PATH, Reply::body(msi));
    }
}

/// `tools/manifest.sh write` が書くのと同じ形の更新の情報。
fn manifest_for(version: &str, msi_sha256: &str) -> String {
    let base = "https://github.com/xia-sava/fantermo/releases/download/latest";
    let zip_sha256 = sha256_hex(b"zip");
    format!(
        "{{\n  \"versionName\": \"{version}\",\n  \"zip\": {{\n    \"url\": \"{base}/fantermo.zip\",\n    \
         \"sha256\": \"{zip_sha256}\"\n  }},\n  \"msi\": {{\n    \"url\": \"{base}/fantermo.msi\",\n    \
         \"sha256\": \"{msi_sha256}\"\n  }}\n}}\n"
    )
}

/// `127.0.0.1` の `port` から更新を取得する fantermo を起動する。`public_key` がなければ埋め込みの公開鍵で検証する。
#[track_caller]
fn start_update_test(port: u16, current: &str, public_key: Option<&[u8]>) -> Server {
    let server = Server::start_with_environment("", &CMD_COMMAND, |dir| {
        let temp = dir.join(UPDATE_TEMP_FOLDER);
        std::fs::create_dir(&temp).expect("failed to create the temporary folder");
        let mut environment = vec![
            ("TMP", temp.clone().into_os_string()),
            ("TEMP", temp.into_os_string()),
            ("FANTERMO_E2E_UPDATE_PORT", port.to_string().into()),
            ("FANTERMO_E2E_UPDATE_CURRENT_VERSION", current.into()),
            (
                "FANTERMO_E2E_UPDATE_HANDOFF_RECORD",
                dir.join(HANDOFF_RECORD).into_os_string(),
            ),
        ];
        if let Some(key) = public_key {
            environment.push(("FANTERMO_E2E_UPDATE_PUBLIC_KEY", hex(key).into()));
        }
        environment
    });
    server.wait_text(CMD_PROMPT);
    server
}

/// ≡ の「fantermo について…」を開く。
#[track_caller]
fn open_about(server: &Server) -> Value {
    let items = open_menu(server, &click_main_menu(&server.state()));
    select_item(server, &items, 'B');
    wait_dialog_of_class(server, TEXT_DIALOG_CLASS)
}

/// 「fantermo について…」を閉じる。
#[track_caller]
fn close_about(server: &Server) {
    click(server, DIALOG_CLOSE_BUTTON);
    server.wait_dialogs(0);
}

/// 「更新を確認」を押し、結果のメッセージボックスの文を返す。
#[track_caller]
fn check_for_update(server: &Server) -> String {
    click(server, ID_CHECK_UPDATE);
    let text = wait_message_box(server);
    let state = server.state();
    assert_eq!(state["dialogs"][0]["title"], "fantermo の更新", "{state:#}");
    text
}

/// 結果のメッセージボックスを閉じて、「更新を確認」を押せるようになるのを待つ。
#[track_caller]
fn dismiss_result(server: &Server) {
    close_message_box(server, 1);
    wait_check_enabled(server);
}

/// 新しいバージョンの知らせに「いいえ」と答えて、「更新を確認」を押せるようになるのを待つ。
#[track_caller]
fn decline_offer(server: &Server) {
    click(server, ID_NO);
    server.wait_dialogs(1);
    wait_check_enabled(server);
}

#[track_caller]
fn wait_check_enabled(server: &Server) {
    server.wait_until("the update button to be enabled", |_| {
        control_enabled(server, ID_CHECK_UPDATE)
    });
}

/// いちばん上のメッセージボックスの文に `needle` が出るのを待ち、その文を返す。
#[track_caller]
fn wait_message_box_with(server: &Server, needle: &str) -> String {
    let state = server.wait_until(&format!("a message box with {needle:?}"), |state| {
        state["dialogs"][0]["class"] == SYSTEM_DIALOG_CLASS
            && dialog_text(&state["dialogs"][0]).contains(needle)
    });
    dialog_text(&state["dialogs"][0])
}

fn dialog_text(dialog: &Value) -> String {
    dialog["controls"]
        .as_array()
        .map(|controls| {
            controls
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// 新しいバージョンの知らせに「はい」と答える。「fantermo について…」が閉じてダウンロードに進む。
#[track_caller]
fn accept_offer(server: &Server, source: &ReleaseSource) {
    source.host.clear_requests();
    assert_contains(&check_for_update(server), OFFER_TEXT);
    click(server, ID_YES);
}

/// ダウンロードのあとの確認まで進む。テストは並列に動くので、同じ実行ファイルのほかの fantermo の知らせが
/// 間に出ることがあり、そのときは続ける。
#[track_caller]
fn wait_confirm_apply(server: &Server) -> String {
    let state = server.wait_until("the prompt after the download", |state| {
        let text = dialog_text(&state["dialogs"][0]);
        text.contains(OTHER_INSTANCES_TEXT) || text.contains(CONFIRM_APPLY_TEXT)
    });
    if dialog_text(&state["dialogs"][0]).contains(OTHER_INSTANCES_TEXT) {
        click(server, ID_YES);
    }
    wait_message_box_with(server, CONFIRM_APPLY_TEXT)
}

#[track_caller]
fn assert_contains(text: &str, needle: &str) {
    assert!(text.contains(needle), "{needle:?} is not in {text:?}");
}

/// fantermo の `TMP` にある、ダウンロードした配布物のフォルダ。
fn download_folders(server: &Server) -> Vec<PathBuf> {
    std::fs::read_dir(server.dir().join(UPDATE_TEMP_FOLDER))
        .expect("failed to read the temporary folder")
        .map(|entry| entry.expect("failed to read a folder entry").path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(DOWNLOAD_FOLDER_PREFIX))
        })
        .collect()
}

/// ダウンロードした配布物が消えるのを待ち、スクリプトの起動を試みていないことを確認する。
#[track_caller]
fn assert_nothing_is_handed_off(server: &Server) {
    server.wait_until("the download to be removed", |_| {
        download_folders(server).is_empty()
    });
    assert!(
        !server.dir().join(HANDOFF_RECORD).exists(),
        "the update was handed off"
    );
}

/// 起動を試みたスクリプトのパスと、BOM を除いた中身。
#[track_caller]
fn handed_off_script(server: &Server) -> (PathBuf, String) {
    let path = std::fs::read_to_string(server.dir().join(HANDOFF_RECORD))
        .expect("the update was not handed off");
    let path = PathBuf::from(path);
    let script = std::fs::read_to_string(&path).expect("failed to read the script");
    (path, script.trim_start_matches('\u{feff}').to_owned())
}

/// インストーラで入れていない開発中のビルドでは「更新を確認」を押せず、そのわけを添える。
#[test]
fn the_update_button_is_grayed_unless_installed_by_the_installer() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let dialog = open_about(&server);
    assert_contains(
        &dialog_text(&dialog),
        "インストーラでインストールした場合にのみ",
    );
    assert!(!control_enabled(&server, ID_CHECK_UPDATE));
    let (status, _) = server.request("POST", &format!("/e2e/dialog/click/{ID_CHECK_UPDATE}"), b"");
    assert_eq!(status, 400);
    close_about(&server);
    check_terminal_still_works(&server, "after-grayed");
}

/// 確認は押したときだけする。起動しても「fantermo について…」を開いても取得しない。起動時の確認はない。
#[test]
fn the_update_is_checked_only_when_the_button_is_pressed() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let server = source.start_fantermo(CURRENT_VERSION);
    let no_requests = |_: &Value| source.host.requests().is_empty();
    server.assert_holds_for(NO_REQUEST_WINDOW, "no request after the start", no_requests);
    open_about(&server);
    assert!(control_enabled(&server, ID_CHECK_UPDATE));
    server.assert_holds_for(NO_REQUEST_WINDOW, "no request with the dialog", no_requests);
    close_about(&server);
    assert!(source.host.requests().is_empty());
}

/// 新しいバージョンは知らせ、いまのバージョンを添えて尋ねる。取得するのは更新の情報と署名だけで、断ればダウンロードしない。
#[test]
fn a_newer_version_is_offered_and_declining_changes_nothing() {
    let source = ReleaseSource::new();
    source.publish("0.0.10", DUMMY_MSI);
    let server = source.start_fantermo("0.0.9");
    open_about(&server);
    assert!(control_enabled(&server, ID_CHECK_UPDATE));

    let text = check_for_update(&server);
    assert_contains(&text, "新しいバージョン 0.0.10 があります");
    assert_contains(&text, "現在のバージョンは 0.0.9");
    assert_contains(&text, OFFER_TEXT);
    assert_eq!(source.host.requests(), [MANIFEST_PATH, SIGNATURE_PATH]);

    decline_offer(&server);
    assert_eq!(source.host.requests(), [MANIFEST_PATH, SIGNATURE_PATH]);
    assert!(download_folders(&server).is_empty());
    assert_eq!(server.state()["dialogs"][0]["class"], TEXT_DIALOG_CLASS);

    close_about(&server);
    check_terminal_still_works(&server, "after-decline");
}

/// バージョンは数として比べる。同じか古ければ更新を出さず、配布物を取得しない。新しくないときは MSI の項目がなくてよい。
#[test]
fn versions_are_compared_as_numbers() {
    let source = ReleaseSource::new();
    let server = source.start_fantermo(CURRENT_VERSION);
    open_about(&server);
    let digest = sha256_hex(DUMMY_MSI);

    for latest in ["0.0.10", "0.0.9", "0.0.2", "0.0.1", "0.0.010"] {
        source.publish_manifest(manifest_for(latest, &digest).as_bytes());
        let text = check_for_update(&server);
        assert_contains(&text, UP_TO_DATE_TEXT);
        assert_contains(&text, &format!("現在のバージョン: {CURRENT_VERSION}"));
        dismiss_result(&server);
    }
    for latest in ["0.0.11", "0.0.100", "0.1.0", "0.10.0", "1.0.0"] {
        source.publish_manifest(manifest_for(latest, &digest).as_bytes());
        assert_contains(
            &check_for_update(&server),
            &format!("新しいバージョン {latest} があります"),
        );
        decline_offer(&server);
    }
    source.publish_manifest(br#"{ "versionName": "0.0.10" }"#);
    assert_contains(&check_for_update(&server), UP_TO_DATE_TEXT);
    dismiss_result(&server);
    assert!(!source.host.requests().iter().any(|path| path == MSI_PATH));

    close_about(&server);
    check_terminal_still_works(&server, "after-versions");
}

/// 署名を検証できなければ、中身が新しいバージョンでも更新を出さず、配布物を取得せず、署名のせいだと分かる文を出す。
#[test]
fn a_manifest_that_does_not_verify_offers_nothing() {
    let source = ReleaseSource::new();
    let other_key = SigningKey::generate();
    let server = source.start_fantermo(CURRENT_VERSION);
    open_about(&server);
    let manifest = manifest_for(NEWER_VERSION, &sha256_hex(DUMMY_MSI));
    let signature = source.key.sign(manifest.as_bytes());
    let zero = [0u8; 32];
    let one = {
        let mut value = [0u8; 32];
        value[31] = 1;
        value
    };
    let tampered_digest = manifest_for(NEWER_VERSION, &sha256_hex(b"another installer"));
    let tampered_version = manifest.replace(NEWER_VERSION, "9.9.9");
    let extra_newline = format!("{manifest}\n");

    // 署名が合う形で同じ流れが更新を出すことを、先に確認する。
    source.publish_manifest(manifest.as_bytes());
    source.host.set(MSI_PATH, Reply::body(DUMMY_MSI));
    assert_contains(&check_for_update(&server), OFFER_TEXT);
    decline_offer(&server);

    // (説明, 更新の情報, 署名)
    let cases: Vec<(&str, &str, Reply)> = vec![
        (
            "signed with another key",
            &manifest,
            Reply::body(other_key.sign(manifest.as_bytes())),
        ),
        (
            "the version was changed",
            &tampered_version,
            Reply::body(&signature),
        ),
        (
            "the digest was changed",
            &tampered_digest,
            Reply::body(&signature),
        ),
        ("a line was added", &extra_newline, Reply::body(&signature)),
        (
            "the signature of something else",
            &manifest,
            Reply::body(source.key.sign(b"something else")),
        ),
        ("no signature", &manifest, Reply::Status(404)),
        ("the signature fails", &manifest, Reply::Status(500)),
        ("an empty signature", &manifest, Reply::body("")),
        ("not base64", &manifest, Reply::body("not base64!")),
        ("not text", &manifest, Reply::body(b"\xff\xfe")),
        ("base64 of nothing like DER", &manifest, Reply::body("AAAA")),
        (
            "zero as r and s",
            &manifest,
            Reply::body(common::base64(&der_signature(&zero, &zero))),
        ),
        (
            "one as r and s",
            &manifest,
            Reply::body(common::base64(&der_signature(&one, &one))),
        ),
        (
            "the raw hash of the manifest",
            &manifest,
            Reply::body(common::base64(&sha256(manifest.as_bytes()))),
        ),
        (
            "a signature that is too large",
            &manifest,
            Reply::Zeros(SIGNATURE_MAX_BYTES + 1),
        ),
        ("the connection closes", &manifest, Reply::Close),
    ];
    for (description, body, signature) in cases {
        source.host.clear_requests();
        source.host.set(MANIFEST_PATH, Reply::body(body));
        source.host.set(SIGNATURE_PATH, signature);
        let text = check_for_update(&server);
        assert_contains(&text, BAD_SIGNATURE_TEXT);
        assert!(!text.contains(OFFER_TEXT), "{description}: {text}");
        assert!(
            !source.host.requests().iter().any(|path| path == MSI_PATH),
            "{description}"
        );
        dismiss_result(&server);
    }

    close_about(&server);
    assert!(download_folders(&server).is_empty());
    check_terminal_still_works(&server, "after-signatures");
}

/// 公開鍵を差し替えていなければ、テストの鍵の署名は埋め込みの公開鍵で通らない。
#[test]
fn the_embedded_key_does_not_accept_another_key() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let server = start_update_test(source.host.port(), CURRENT_VERSION, None);
    open_about(&server);
    assert_contains(&check_for_update(&server), BAD_SIGNATURE_TEXT);
    dismiss_result(&server);
    close_about(&server);
    check_terminal_still_works(&server, "after-embedded-key");
}

/// 更新の情報を取得できない・読めないときは、わけの分かる文を出す。落ちず、もう一度確認できる。
#[test]
fn failures_to_fetch_or_read_the_manifest_are_told() {
    let source = ReleaseSource::new();
    let server = source.start_fantermo(CURRENT_VERSION);
    open_about(&server);
    let manifest = manifest_for(NEWER_VERSION, &sha256_hex(DUMMY_MSI));
    source.host.set(
        SIGNATURE_PATH,
        Reply::body(source.key.sign(manifest.as_bytes())),
    );

    // 取得できない。署名は取得しない。
    let cases: Vec<(&str, Reply, &[&str])> = vec![
        ("404", Reply::Status(404), &[CANNOT_FETCH_TEXT, "HTTP 404"]),
        ("500", Reply::Status(500), &[CANNOT_FETCH_TEXT, "HTTP 500"]),
        ("closed", Reply::Close, &[CANNOT_FETCH_TEXT, NETWORK_TEXT]),
        (
            "too large",
            Reply::Zeros(MANIFEST_MAX_BYTES + 1),
            &[CANNOT_FETCH_TEXT],
        ),
    ];
    for (description, reply, expected) in cases {
        source.host.clear_requests();
        source.host.set(MANIFEST_PATH, reply);
        let text = check_for_update(&server);
        for needle in expected {
            assert!(text.contains(needle), "{description}: {text}");
        }
        assert!(
            !source
                .host
                .requests()
                .iter()
                .any(|path| path == SIGNATURE_PATH),
            "{description}"
        );
        dismiss_result(&server);
    }

    // 途中で切れた更新の情報は、署名が合わない。
    source.host.set(
        MANIFEST_PATH,
        Reply::Truncated {
            claimed: manifest.len(),
            body: Arc::from(&manifest.as_bytes()[..manifest.len() / 2]),
        },
    );
    assert_contains(&check_for_update(&server), BAD_SIGNATURE_TEXT);
    dismiss_result(&server);

    // 署名は合うが読めない。
    let malformed: [(&str, &[u8]); 6] = [
        ("empty", b""),
        ("broken JSON", b"{"),
        ("no version", b"{}"),
        ("not a version", br#"{ "versionName": "dev-abc" }"#),
        ("newer without an MSI", br#"{ "versionName": "0.0.11" }"#),
        (
            "a short digest",
            br#"{ "versionName": "0.0.11", "msi": { "sha256": "8b25" } }"#,
        ),
    ];
    for (description, manifest) in malformed {
        source.publish_manifest(manifest);
        let text = check_for_update(&server);
        assert!(
            text.contains(UNREADABLE_MANIFEST_TEXT),
            "{description}: {text}"
        );
        dismiss_result(&server);
    }

    // 上限ちょうどまでは受け取る。
    let mut largest = b"{ \"versionName\": \"0.0.10\" }".to_vec();
    largest.resize(MANIFEST_MAX_BYTES, b' ');
    source.publish_manifest(&largest);
    assert_contains(&check_for_update(&server), UP_TO_DATE_TEXT);
    dismiss_result(&server);

    close_about(&server);
    check_terminal_still_works(&server, "after-fetch-failures");
}

/// 接続できないときは、ネットワークを確認するよう知らせる。
#[test]
fn a_host_that_refuses_connections_is_told() {
    let server = start_update_test(unused_port(), CURRENT_VERSION, None);
    open_about(&server);
    let text = check_for_update(&server);
    assert_contains(&text, CANNOT_FETCH_TEXT);
    assert_contains(&text, NETWORK_TEXT);
    dismiss_result(&server);
    close_about(&server);
    check_terminal_still_works(&server, "after-refused");
}

/// 答えない相手は時間切れになる（受信の上限は 30 秒）。待っている間も端末は動く。
#[test]
fn a_silent_host_times_out_while_the_terminal_keeps_working() {
    let source = ReleaseSource::new();
    source.host.set(MANIFEST_PATH, Reply::Silent);
    let server = source.start_fantermo(CURRENT_VERSION);
    open_about(&server);
    click(&server, ID_CHECK_UPDATE);
    server.wait_until("the check to start", |_| !source.host.requests().is_empty());
    close_about(&server);
    check_terminal_still_works(&server, "while-waiting");
    let text = wait_message_box_with(&server, CANNOT_FETCH_TEXT);
    assert_contains(&text, NETWORK_TEXT);
    close_message_box(&server, 0);
    check_terminal_still_works(&server, "after-timeout");
}

/// 確認している間は押せない。「fantermo について…」を閉じても確認は続き、結果は端末のウィンドウの上に出る。
/// 開き直せば、結果はその上に出て、押せるように戻る。
#[test]
fn closing_or_reopening_the_about_dialog_during_a_check() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let manifest = manifest_for(NEWER_VERSION, &sha256_hex(DUMMY_MSI));
    let hold = |gate: &Arc<Gate>| Reply::After {
        gate: Arc::clone(gate),
        reply: Box::new(Reply::body(&manifest)),
    };
    let server = source.start_fantermo(CURRENT_VERSION);

    // 閉じてから結果が届く。
    let gate = Gate::new();
    source.host.set(MANIFEST_PATH, hold(&gate));
    open_about(&server);
    click(&server, ID_CHECK_UPDATE);
    server.wait_until("the check to start", |_| !source.host.requests().is_empty());
    assert!(!control_enabled(&server, ID_CHECK_UPDATE));
    close_about(&server);
    check_terminal_still_works(&server, "during-check");
    gate.open();
    wait_message_box_with(&server, OFFER_TEXT);
    assert_eq!(server.state()["dialogs"].as_array().map(Vec::len), Some(1));
    assert_eq!(server.state()["window_enabled"], false);
    click(&server, ID_NO);
    server.wait_dialogs(0);
    server.wait_until("the window to be enabled", |state| {
        state["window_enabled"] == true
    });
    check_terminal_still_works(&server, "after-result-on-window");

    // 開き直している間に結果が届く。
    let gate = Gate::new();
    source.host.clear_requests();
    source.host.set(MANIFEST_PATH, hold(&gate));
    open_about(&server);
    click(&server, ID_CHECK_UPDATE);
    server.wait_until("the check to start", |_| !source.host.requests().is_empty());
    close_about(&server);
    open_about(&server);
    assert!(!control_enabled(&server, ID_CHECK_UPDATE));
    gate.open();
    wait_message_box_with(&server, OFFER_TEXT);
    assert_eq!(server.state()["dialogs"].as_array().map(Vec::len), Some(2));
    decline_offer(&server);
    close_about(&server);
    check_terminal_still_works(&server, "after-reopen");
}

/// ほかのダイアログの間に結果が届いても、閉じたあとにそのダイアログの間だけウィンドウを無効のままにする。
#[test]
fn a_result_arriving_during_the_settings_dialog_keeps_the_window_disabled() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let manifest = manifest_for(NEWER_VERSION, &sha256_hex(DUMMY_MSI));
    let gate = Gate::new();
    source.host.set(
        MANIFEST_PATH,
        Reply::After {
            gate: Arc::clone(&gate),
            reply: Box::new(Reply::body(&manifest)),
        },
    );
    let server = source.start_fantermo(CURRENT_VERSION);
    open_about(&server);
    click(&server, ID_CHECK_UPDATE);
    server.wait_until("the check to start", |_| !source.host.requests().is_empty());
    close_about(&server);
    open_settings(&server);
    gate.open();
    wait_message_box_with(&server, OFFER_TEXT);
    click(&server, ID_NO);
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    server.wait_dialogs(1);
    let state = server.state();
    assert_eq!(state["window_enabled"], false, "{state:#}");
    click(&server, ID_CANCEL);
    server.wait_dialogs(0);
    server.wait_until("the window to be enabled", |state| {
        state["window_enabled"] == true
    });
    check_terminal_still_works(&server, "after-settings-and-result");
}

/// 確認やダウンロードの途中でウィンドウを閉じても、普通に終わり、何も起動しない。
#[test]
fn quitting_during_a_check_or_a_download_ends_normally() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let manifest = manifest_for(NEWER_VERSION, &sha256_hex(DUMMY_MSI));
    let check_gate = Gate::new();
    source.host.set(
        MANIFEST_PATH,
        Reply::After {
            gate: Arc::clone(&check_gate),
            reply: Box::new(Reply::body(&manifest)),
        },
    );
    let mut during_check = source.start_fantermo(CURRENT_VERSION);
    open_about(&during_check);
    click(&during_check, ID_CHECK_UPDATE);
    during_check.wait_until("the check to start", |_| !source.host.requests().is_empty());
    close_about(&during_check);
    during_check.post("/e2e/quit", b"");
    assert_eq!(during_check.wait_exit(), 0);
    check_gate.open();
    assert!(!during_check.dir().join(HANDOFF_RECORD).exists());
    drop(during_check);

    let download_gate = Gate::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    source.host.set(
        MSI_PATH,
        Reply::After {
            gate: Arc::clone(&download_gate),
            reply: Box::new(Reply::body(DUMMY_MSI)),
        },
    );
    let mut during_download = source.start_fantermo(CURRENT_VERSION);
    open_about(&during_download);
    accept_offer(&during_download, &source);
    during_download.wait_until("the download to start", |_| {
        source.host.requests().iter().any(|path| path == MSI_PATH)
    });
    during_download.wait_dialogs(0);
    during_download.post("/e2e/quit", b"");
    assert_eq!(during_download.wait_exit(), 0);
    download_gate.open();
    assert!(!during_download.dir().join(HANDOFF_RECORD).exists());
    assert!(download_folders(&during_download).is_empty());
}

/// 配布物が更新の情報のハッシュと合わない、取得できないときは、スクリプトに引き渡さず、知らせる。
#[test]
fn a_download_that_does_not_match_or_fails_is_not_handed_off() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let server = source.start_fantermo(CURRENT_VERSION);
    let cases: Vec<(&str, Reply, &str)> = vec![
        (
            "another file",
            Reply::body(b"a replaced installer"),
            DIGEST_MISMATCH_TEXT,
        ),
        (
            "cut in the middle",
            Reply::Truncated {
                claimed: DUMMY_MSI.len(),
                body: Arc::from(&DUMMY_MSI[..DUMMY_MSI.len() / 2]),
            },
            DIGEST_MISMATCH_TEXT,
        ),
        ("404", Reply::Status(404), "HTTP 404"),
        ("500", Reply::Status(500), "HTTP 500"),
        ("closed", Reply::Close, CANNOT_DOWNLOAD_TEXT),
        (
            "too large",
            Reply::Zeros(MSI_MAX_BYTES + 1),
            CANNOT_DOWNLOAD_TEXT,
        ),
    ];
    for (description, reply, expected) in cases {
        source.host.set(MSI_PATH, reply);
        open_about(&server);
        accept_offer(&server, &source);
        let text = wait_message_box_with(&server, "更新できませんでした");
        assert!(text.contains(expected), "{description}: {text}");
        assert!(!text.contains(CONFIRM_APPLY_TEXT), "{description}: {text}");
        assert_eq!(
            source.host.requests().last().map(String::as_str),
            Some(MSI_PATH),
            "{description}"
        );
        close_message_box(&server, 0);
        assert_nothing_is_handed_off(&server);
    }
    check_terminal_still_works(&server, "after-bad-downloads");
}

/// ハッシュが合えば確認へ進み、「OK」でスクリプトに引き渡して閉じる。スクリプトは起動せず、中身を確認する。
#[test]
fn a_matching_download_is_handed_off_to_the_script_without_running_it() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let mut server = source.start_fantermo(CURRENT_VERSION);
    let pid = server.pid();
    open_about(&server);
    accept_offer(&server, &source);
    assert_contains(
        &wait_confirm_apply(&server),
        &format!("バージョン {NEWER_VERSION} に更新します"),
    );
    assert_eq!(
        source.host.requests(),
        [MANIFEST_PATH, SIGNATURE_PATH, MSI_PATH]
    );
    assert!(!server.dir().join(HANDOFF_RECORD).exists());

    click(&server, ID_OK);
    assert_eq!(server.wait_exit(), 0);
    assert!(server.state_file().exists(), "the state was not saved");

    let (script_path, script) = handed_off_script(&server);
    let folder = script_path.parent().expect("no folder");
    assert_eq!(
        folder.parent().expect("no temporary folder"),
        server.dir().join(UPDATE_TEMP_FOLDER)
    );
    let msi = folder.join("fantermo.msi");
    assert_eq!(std::fs::read(&msi).expect("no MSI"), DUMMY_MSI);
    assert_contains(&script, &format!("foreach ($id in @({pid}"));
    assert_contains(&script, r"System32\msiexec.exe");
    assert_contains(
        &script,
        &format!("@('/i', '\"{}\"', '/qn', '/norestart'", msi.display()),
    );
    assert_contains(
        &script,
        &format!(
            "Start-Process -FilePath '{}'",
            env!("CARGO_BIN_EXE_fantermo").replace('/', r"\")
        ),
    );
    assert!(!script.contains(r"AppData\Local\Programs"), "{script}");
}

/// 確認で「キャンセル」か × を選ぶと、引き渡さず、ダウンロードしたものを消す。もう一度確認できる。
#[test]
fn cancelling_the_final_confirmation_removes_the_download() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let server = source.start_fantermo(CURRENT_VERSION);

    open_about(&server);
    accept_offer(&server, &source);
    wait_confirm_apply(&server);
    assert_eq!(download_folders(&server).len(), 1);
    click(&server, ID_CANCEL);
    server.wait_dialogs(0);
    assert_nothing_is_handed_off(&server);
    check_terminal_still_works(&server, "after-cancel");

    open_about(&server);
    accept_offer(&server, &source);
    wait_confirm_apply(&server);
    close_message_box(&server, 0);
    assert_nothing_is_handed_off(&server);
    check_terminal_still_works(&server, "after-close-button");
}

/// ダウンロードの間に開いた「fantermo について…」の上にも確認を出し、取りやめれば「更新を確認」を押せるように戻る。
#[test]
fn a_download_finishing_over_the_about_dialog_asks_there() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let gate = Gate::new();
    source.host.set(
        MSI_PATH,
        Reply::After {
            gate: Arc::clone(&gate),
            reply: Box::new(Reply::body(DUMMY_MSI)),
        },
    );
    let server = source.start_fantermo(CURRENT_VERSION);
    open_about(&server);
    accept_offer(&server, &source);
    server.wait_until("the download to start", |_| {
        source.host.requests().iter().any(|path| path == MSI_PATH)
    });
    server.wait_dialogs(0);
    open_about(&server);
    assert!(!control_enabled(&server, ID_CHECK_UPDATE));
    gate.open();
    wait_confirm_apply(&server);
    click(&server, ID_CANCEL);
    server.wait_until("the about dialog to be on top", |state| {
        state["dialogs"][0]["class"] == TEXT_DIALOG_CLASS
    });
    wait_check_enabled(&server);
    assert_nothing_is_handed_off(&server);
    close_about(&server);
    check_terminal_still_works(&server, "after-download-over-about");
}

/// ほかの fantermo が動いていれば知らせる。断れば引き渡さず、続ければスクリプトはそのプロセスも待つ。
#[test]
fn other_instances_are_told_and_waited_for() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let other = Server::start_cmd();
    other.wait_text(CMD_PROMPT);
    let other_pid = other.pid();
    let mut server = source.start_fantermo(CURRENT_VERSION);

    open_about(&server);
    accept_offer(&server, &source);
    wait_message_box_with(&server, OTHER_INSTANCES_TEXT);
    click(&server, ID_NO);
    server.wait_dialogs(0);
    assert_nothing_is_handed_off(&server);
    check_terminal_still_works(&server, "after-declining-others");

    open_about(&server);
    accept_offer(&server, &source);
    wait_message_box_with(&server, OTHER_INSTANCES_TEXT);
    click(&server, ID_YES);
    wait_message_box_with(&server, CONFIRM_APPLY_TEXT);
    click(&server, ID_OK);
    assert_eq!(server.wait_exit(), 0);
    let (_, script) = handed_off_script(&server);
    let waited = script
        .lines()
        .find(|line| line.starts_with("foreach ($id in @("))
        .expect("the script waits for nothing");
    assert!(
        waited.contains(&other_pid.to_string()),
        "{other_pid} is not in {waited}"
    );
}

/// 確認している間に配布物が変わったら引き渡さず、知らせる。ダウンロードしたものは消す。
#[test]
fn a_download_changed_after_the_check_is_not_handed_off() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let server = source.start_fantermo(CURRENT_VERSION);
    open_about(&server);
    accept_offer(&server, &source);
    wait_confirm_apply(&server);
    let folders = download_folders(&server);
    let msi = folders[0].join("fantermo.msi");
    std::fs::write(&msi, b"changed after the download").expect("failed to change the MSI");
    click(&server, ID_OK);
    wait_message_box_with(&server, CANNOT_START_TEXT);
    close_message_box(&server, 0);
    assert_nothing_is_handed_off(&server);
    check_terminal_still_works(&server, "after-changed-download");
}

/// スクリプトを起動できなければ、知らせて端末のまま続ける。ダウンロードしたものは消す。
#[test]
fn a_script_that_cannot_start_is_told_and_the_terminal_continues() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let server = source.start_fantermo(CURRENT_VERSION);
    // 書けないパスを作って、起動の失敗にする。
    std::fs::create_dir(server.dir().join(HANDOFF_RECORD)).expect("failed to create the folder");
    open_about(&server);
    accept_offer(&server, &source);
    wait_confirm_apply(&server);
    click(&server, ID_OK);
    wait_message_box_with(&server, CANNOT_START_TEXT);
    close_message_box(&server, 0);
    server.wait_until("the download to be removed", |_| {
        download_folders(&server).is_empty()
    });
    check_terminal_still_works(&server, "after-script-failure");
}

/// いまのバージョンが読めなければ、取得せずに知らせる。
#[test]
fn an_unreadable_current_version_is_told() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let server = source.start_fantermo("dev-abc");
    open_about(&server);
    assert_contains(
        &check_for_update(&server),
        "現在のバージョンを読み取れませんでした",
    );
    assert!(source.host.requests().is_empty());
    dismiss_result(&server);
    close_about(&server);
    check_terminal_still_works(&server, "after-unreadable-version");
}

// DPI の変更。モニターの DPI は変えられないので、`WM_DPICHANGED` への反応を見る。手元のモニター（192 DPI）でも
// ランナー（96 DPI）でも同じに通るよう、寸法は状態の値から求め、基準は 96 DPI で読んだ値にする。

const DPI_CHANGED: &str = "dpichanged";
const GET_DPI_SCALED_SIZE: &str = "getdpiscaledsize";
const DPI_100_PERCENT: u64 = BASE_DPI;
const DPI_150_PERCENT: u64 = 144;
const DPI_200_PERCENT: u64 = 192;
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

/// `/e2e/message/dpichanged/<DPI>` を送り、そのあとの状態を返す。処理はこの場で終わるので、待たずに読める。
#[track_caller]
fn change_dpi(server: &Server, dpi: u64) -> Value {
    let wparam = usize::try_from(dpi).expect("the DPI fits");
    assert_eq!(server.send_message(DPI_CHANGED, wparam), 0);
    let state = server.state();
    assert_eq!(state["dpi"], dpi, "{state:#}");
    assert_fits_the_runner_screen(&state);
    state
}

/// ランナーの画面に収まらないと OS がウィンドウを切り詰めて桁数が減るので、手元で先に気づけるようにする。
#[track_caller]
fn assert_fits_the_runner_screen(state: &Value) {
    let window = current_window(state);
    assert!(
        u64::try_from(window.width).expect("the width fits") <= RUNNER_SCREEN.0
            && u64::try_from(window.height).expect("the height fits") <= RUNNER_SCREEN.1,
        "the window does not fit the screen of the runner: {state:#}"
    );
}

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
    server.post("/e2e/tab/new", b"");
    server.wait_until("the second tab", |state| tab_count(state) == 2);
    server.wait_text("PS ");
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
#[test]
fn the_size_asked_before_a_dpi_change_is_left_to_windows() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.resize(DPI_TEST_GRID.0, DPI_TEST_GRID.1);

    for dpi in [DPI_150_PERCENT, DPI_200_PERCENT, DPI_100_PERCENT] {
        let before = server.state();
        let answer = server.post(&format!("/e2e/message/{GET_DPI_SCALED_SIZE}/{dpi}"), b"");
        let answer: Value = serde_json::from_slice(&answer).expect("the answer is not JSON");
        assert_eq!(answer["result"], 0, "{answer:#}");
        assert_eq!(answer["width"], 0, "{answer:#}");
        assert_eq!(answer["height"], 0, "{answer:#}");
        assert_eq!(dpi_layout(&server.state()), dpi_layout(&before));
        change_dpi(&server, dpi);
    }
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
/// （誤差が積もらない）。ウィンドウはそのたびに DPI の比で変わる。
#[test]
fn going_between_dpis_returns_to_the_same_parts() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.resize(DPI_TEST_GRID.0, DPI_TEST_GRID.1);
    let mut previous = server.state();
    let at_100 = dpi_parts(&previous);
    let state = change_dpi(&server, DPI_150_PERCENT);
    assert_window_scaled(&state, &previous);
    let at_150 = dpi_parts(&state);
    previous = state;
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

    for dpi in [DPI_150_PERCENT, DPI_200_PERCENT, monitor_dpi] {
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

/// 背景画像は、DPI を変えたあとの新しい範囲いっぱいに広がり、タブバーとスクロールバーには出ない。
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
        let picture = server.picture();
        assert!(
            !color_is_close(picture.rgb(x, area.y - 1), BAND_TOP),
            "at {dpi} DPI"
        );
        let (bar_x, bar_y) = area.at(1.0, 0.5);
        assert!(
            !color_is_close(picture.rgb(bar_x, bar_y), MIDDLE_RIGHT),
            "at {dpi} DPI"
        );
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

/// `[window]` の `dpi` を書いた `state.toml`（`dpi` を書いていたバージョンのもの）も読め、`dpi` は無視してピクセルのまま
/// 開く。閉じるときに書き直すと `dpi` はなくなる。
#[test]
fn a_saved_size_with_a_dpi_is_read_in_pixels() {
    let saved = format!(
        "{}dpi = {DPI_200_PERCENT}\n",
        state::to_toml(&seeded_state(UNEVEN_WINDOW))
    );
    let (mut server, state) = start_at_dpi(DPI_100_PERCENT, saved.as_bytes());
    assert_eq!(state["dpi"], DPI_100_PERCENT, "{state:#}");
    assert_eq!(current_window(&state), UNEVEN_WINDOW, "{state:#}");
    assert_layout_fits_the_client(&state);
    quit(&mut server);
    assert_eq!(saved_window(&server), UNEVEN_WINDOW);
    let written = std::fs::read_to_string(server.state_file()).expect("failed to read the state");
    assert!(!written.contains("dpi"), "{written}");
}

/// `FANTERMO_DPI` を使わず、モニターの DPI で起動する。別の DPI を書いた `state.toml` もピクセルのまま開く。
#[test]
fn a_size_saved_at_another_monitor_dpi_opens_in_pixels() {
    let probe = Server::start_cmd();
    let monitor_dpi = number(&probe.wait_text(CMD_PROMPT)["monitor_dpi"]);
    drop(probe);
    let other_dpi = if monitor_dpi == DPI_200_PERCENT {
        DPI_100_PERCENT
    } else {
        DPI_200_PERCENT
    };
    let saved = format!(
        "{}dpi = {other_dpi}\n",
        state::to_toml(&seeded_state(UNEVEN_WINDOW))
    );

    // `FANTERMO_DPI` を読めない値にすると、モニターの DPI を使う。
    let server = Server::start_with_environment("", &CMD_COMMAND, |dir| {
        std::fs::write(dir.join(common::STATE_FILE), &saved).expect("failed to write the state");
        vec![(DPI_ENV, OsString::new())]
    });
    let state = server.wait_text(CMD_PROMPT);
    assert_eq!(state["dpi"], monitor_dpi, "{state:#}");
    assert_eq!(current_window(&state), UNEVEN_WINDOW, "{state:#}");
    assert_layout_fits_the_client(&state);
}

/// 最大化して、別の DPI を書いた `state.toml` は、最大化を戻す前の大きさをピクセルのまま使い、最大化して開く。
#[test]
fn a_maximized_window_saved_at_another_dpi_keeps_its_normal_size() {
    let expected = WindowState {
        maximized: true,
        ..UNEVEN_WINDOW
    };
    let saved = format!(
        "{}dpi = {DPI_200_PERCENT}\n",
        state::to_toml(&seeded_state(expected))
    );

    let (mut server, state) = start_at_dpi(DPI_100_PERCENT, saved.as_bytes());
    assert_eq!(current_window(&state), expected);
    quit(&mut server);
    assert_eq!(saved_window(&server), expected);
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

/// プロファイルのテストの設定。プロンプトでどのプロファイルかが分かる。
const TWO_PROMPT_PROFILES: &str = r#"default_profile = "two"

[[profiles]]
name = "one"
command = "cmd.exe /k prompt one$G"

[[profiles]]
name = "two"
command = "cmd.exe /k prompt two$G"

[[profiles]]
name = "with space"
command = "cmd.exe /k prompt space$G"
"#;

#[test]
fn the_profile_option_chooses_the_profile_of_the_tabs() {
    let server = Server::start(TWO_PROMPT_PROFILES, &["--profile", "one"]);
    let state = server.wait_text("one>");
    assert_eq!(state["tabs"][0]["profile"], "one", "{state:#}");
    assert_eq!(state["dialogs"].as_array().map(Vec::len), Some(0));

    // 新しいタブも引数と同じ決め方で開く。一覧から選べばそのプロファイル。
    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    let state = server.wait_text("one>");
    assert_eq!(state["tabs"][1]["profile"], "one", "{state:#}");
    server.post("/e2e/tab/new/1", b"");
    server.wait_until("three tabs", |state| tab_count(state) == 3);
    let state = server.wait_text("two>");
    assert_eq!(state["tabs"][2]["profile"], "two", "{state:#}");
}

#[test]
fn the_profile_option_takes_a_quoted_name() {
    let server = Server::start(TWO_PROMPT_PROFILES, &["--profile", "with space"]);
    let state = server.wait_text("space>");
    assert_eq!(state["tabs"][0]["profile"], "with space", "{state:#}");
}

#[test]
fn an_unknown_profile_is_reported_and_the_default_one_opens() {
    // 名前は大文字小文字も含めて完全に一致するものだけ。
    let server = Server::start(TWO_PROMPT_PROFILES, &["--profile", "ONE"]);
    let text = wait_message_box_with(&server, "「ONE」");
    assert!(text.contains("one、two、with space"), "{text}");
    let state = server.wait_text("two>");
    assert_eq!(state["tabs"][0]["profile"], "two", "{state:#}");
    close_message_box(&server, 0);
    server.type_line("echo after-unknown-%ERRORLEVEL%-end");
    server.wait_text("after-unknown-0-end");
}

#[test]
fn a_profile_option_without_a_name_is_reported_and_the_default_one_opens() {
    let server = Server::start(TWO_PROMPT_PROFILES, &["--profile"]);
    wait_message_box_with(&server, "--profile");
    server.wait_text("two>");
    close_message_box(&server, 0);
    server.type_line("echo after-no-name-%ERRORLEVEL%-end");
    server.wait_text("after-no-name-0-end");
}

#[test]
fn an_unknown_profile_with_a_command_runs_the_command() {
    let server = Server::start(
        TWO_PROMPT_PROFILES,
        &["--profile", "none", "cmd.exe", "/k", "prompt", "plain$G"],
    );
    wait_message_box_with(&server, "「none」");
    let state = server.wait_text("plain>");
    assert_eq!(state["tabs"][0]["profile"], Value::Null, "{state:#}");
}

#[test]
fn a_profile_with_a_command_runs_the_command_in_that_profile() {
    let server = Server::start(
        TWO_PROMPT_PROFILES,
        &["--profile", "one", "cmd.exe", "/k", "prompt", "command$G"],
    );
    let state = server.wait_text("command>");
    assert_eq!(state["tabs"][0]["profile"], "one", "{state:#}");

    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    let state = server.wait_text("command>");
    assert_eq!(state["tabs"][1]["profile"], "one", "{state:#}");

    // 一覧から選ぶと、そのプロファイルの `command`。
    server.post("/e2e/tab/new/0", b"");
    server.wait_until("three tabs", |state| tab_count(state) == 3);
    server.wait_text("one>");
}

#[test]
fn two_dashes_end_the_options() {
    let server = Server::start(
        TWO_PROMPT_PROFILES,
        &["--", "cmd.exe", "/k", "prompt", "after-dashes$G"],
    );
    let state = server.wait_text("after-dashes>");
    assert_eq!(state["tabs"][0]["profile"], Value::Null, "{state:#}");

    let server = Server::start(
        TWO_PROMPT_PROFILES,
        &[
            "--profile",
            "one",
            "--",
            "cmd.exe",
            "/k",
            "prompt",
            "both$G",
        ],
    );
    let state = server.wait_text("both>");
    assert_eq!(state["tabs"][0]["profile"], "one", "{state:#}");
}

#[test]
fn an_unknown_option_at_the_start_is_the_command() {
    // 一時フォルダに `--` で始まる名前で置いた `cmd.exe` を、`PATH` から起動する。
    let system = std::env::var_os("SystemRoot").expect("no SystemRoot");
    let server = Server::start_with_environment(
        TWO_PROMPT_PROFILES,
        &["--e2e-cmd.exe", "/k", "prompt", "unknown-option$G"],
        |dir| {
            std::fs::copy(
                PathBuf::from(system).join(r"System32\cmd.exe"),
                dir.join("--e2e-cmd.exe"),
            )
            .expect("failed to copy cmd.exe");
            let mut path = OsString::from(dir);
            path.push(";");
            path.push(std::env::var_os("PATH").unwrap_or_default());
            vec![("PATH", path)]
        },
    );
    let state = server.wait_text("unknown-option>");
    assert_eq!(state["tabs"][0]["profile"], Value::Null, "{state:#}");
}

#[test]
fn options_after_the_command_go_to_the_command_with_their_quotes() {
    let server = Server::start(
        TWO_PROMPT_PROFILES,
        &[
            "cmd.exe",
            "/k",
            "echo",
            "argument-%ERRORLEVEL%-end",
            "two  spaces",
            "--profile",
            "one",
        ],
    );
    let state = server.wait_text(r#"argument-0-end "two  spaces" --profile one"#);
    assert_eq!(state["tabs"][0]["profile"], Value::Null, "{state:#}");
    assert_eq!(state["dialogs"].as_array().map(Vec::len), Some(0));
}

/// 別のプロセスからの起動の指定が受け取られて、起動したプロセスが終わったことを確認する。
#[track_caller]
fn assert_handed_over(mut launched: Server) {
    assert_eq!(launched.wait_exit(), 0, "{}", launched.log());
    assert!(!launched.opened_a_window(), "{}", launched.log());
}

/// `dir` の下に作業フォルダを作る。
fn working_folder(dir: &std::path::Path, name: &str) -> PathBuf {
    let folder = dir.join(name);
    std::fs::create_dir_all(&folder).expect("failed to create the working folder");
    folder
}

/// 作業フォルダの名前を出す `cmd.exe` の行。
const PRINT_FOLDER_NAME: &str = "for %I in (.) do echo folder-%~nxI-end";

#[test]
fn a_second_launch_adds_a_tab_and_ends() {
    let server = Server::start(TWO_PROMPT_PROFILES, &[]);
    let state = server.wait_text("two>");
    let pid = state["pid"].clone();

    // 引数がなければ既定のプロファイル。
    assert_handed_over(server.launch_again(&[]));
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    let state = server.wait_text("two>");
    assert_eq!(state["pid"], pid);
    assert_eq!(state["active"], 1);
    assert_eq!(state["tabs"][1]["profile"], "two", "{state:#}");

    // プロファイルの指定。
    assert_handed_over(server.launch_again(&["--profile", "one"]));
    server.wait_until("three tabs", |state| tab_count(state) == 3);
    let state = server.wait_text("one>");
    assert_eq!(state["tabs"][2]["profile"], "one", "{state:#}");

    // コマンドとプロファイルの指定。
    assert_handed_over(server.launch_again(&[
        "--profile",
        "one",
        "cmd.exe",
        "/k",
        "prompt",
        "both$G",
    ]));
    server.wait_until("four tabs", |state| tab_count(state) == 4);
    let state = server.wait_text("both>");
    assert_eq!(state["tabs"][3]["profile"], "one", "{state:#}");

    // コマンドの指定。
    assert_handed_over(server.launch_again(&["cmd.exe", "/k", "prompt", "plain$G"]));
    server.wait_until("five tabs", |state| tab_count(state) == 5);
    let state = server.wait_text("plain>");
    assert_eq!(state["tabs"][4]["profile"], Value::Null, "{state:#}");

    // ウィンドウの引数は変わらないので、新しいタブは既定のプロファイルのまま。
    server.post("/e2e/tab/new", b"");
    server.wait_until("six tabs", |state| tab_count(state) == 6);
    let state = server.wait_text("two>");
    assert_eq!(state["tabs"][5]["profile"], "two", "{state:#}");
}

#[test]
fn a_second_launch_does_not_use_the_arguments_of_the_window() {
    let server = Server::start(TWO_PROMPT_PROFILES, &["cmd.exe", "/k", "prompt", "first$G"]);
    server.wait_text("first>");
    assert_handed_over(server.launch_again(&[]));
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    server.wait_text("two>");
    // ウィンドウの引数で開く新しいタブは変わらない。
    server.post("/e2e/tab/new", b"");
    server.wait_until("three tabs", |state| tab_count(state) == 3);
    server.wait_text("first>");
}

#[test]
fn a_second_launch_passes_its_working_folder() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let folder = working_folder(server.dir(), "launched-from-here");
    assert_handed_over(server.launch_again_with(
        &CMD_COMMAND,
        &LaunchAgain {
            working_folder: Some(&folder),
            ..Default::default()
        },
    ));
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    server.wait_text(CMD_PROMPT);
    server.type_line(PRINT_FOLDER_NAME);
    server.wait_text("folder-launched-from-here-end");

    // 最初のタブの作業フォルダは変わらない。
    server.post("/e2e/tab/select/0", b"");
    server.wait_until("the first tab", |state| state["active"] == 0);
    server.type_line(PRINT_FOLDER_NAME);
    let name = server
        .dir()
        .file_name()
        .expect("no folder name")
        .to_string_lossy()
        .into_owned();
    server.wait_text(&format!("folder-{name}-end"));
}

#[test]
fn a_launch_from_a_tab_of_the_window_adds_a_tab() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let exe = env!("CARGO_BIN_EXE_fantermo");
    server.type_line(&format!(
        "\"{exe}\" cmd.exe /k prompt inner$G & echo launched-%ERRORLEVEL%-end"
    ));
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    server.wait_text("inner>");
    server.post("/e2e/tab/select/0", b"");
    server.wait_text("launched-0-end");
}

#[test]
fn the_new_window_option_opens_another_window() {
    let server = Server::start_cmd();
    let first = server.wait_text(CMD_PROMPT);

    let mut second = server.launch_again(&["--new-window", "cmd.exe", "/k", "prompt", "second$G"]);
    second.wait_for_window();
    let state = second.wait_text("second>");
    assert_ne!(state["pid"], first["pid"], "{state:#}");
    assert_eq!(tab_count(&state), 1);
    assert_eq!(tab_count(&server.state()), 1);

    // プロファイルと組み合わせられる。
    std::fs::write(server.dir().join("config.toml"), TWO_PROMPT_PROFILES)
        .expect("failed to write the settings");
    let mut third = server.launch_again(&["--new-window", "--profile", "one"]);
    third.wait_for_window();
    let state = third.wait_text("one>");
    assert_eq!(state["tabs"][0]["profile"], "one", "{state:#}");
    assert_eq!(tab_count(&server.state()), 1);
    assert_eq!(tab_count(&second.state()), 1);
}

#[test]
fn a_second_launch_goes_to_the_window_used_last() {
    let first = Server::start_cmd();
    first.wait_text(CMD_PROMPT);
    let mut second = first.launch_again(&["--new-window", "cmd.exe", "/k", "prompt", "e2e$G"]);
    second.wait_for_window();
    second.wait_text(CMD_PROMPT);

    // 新しいウィンドウがいちばん上。
    assert_handed_over(first.launch_again(&CMD_COMMAND));
    second.wait_until("two tabs in the second window", |state| {
        tab_count(state) == 2
    });
    assert_eq!(tab_count(&first.state()), 1);

    // もう一方のウィンドウを下げると、こちらがいちばん上になる。利用者がこちらを使ったときと同じ順。
    second.post("/e2e/window/lower", b"");
    assert_handed_over(first.launch_again(&CMD_COMMAND));
    first.wait_until("two tabs in the first window", |state| {
        tab_count(state) == 2
    });
    assert_eq!(tab_count(&second.state()), 2);

    first.post("/e2e/window/lower", b"");
    assert_handed_over(first.launch_again(&CMD_COMMAND));
    second.wait_until("three tabs in the second window", |state| {
        tab_count(state) == 3
    });
    assert_eq!(tab_count(&first.state()), 2);
}

#[test]
fn windows_on_other_desktops_are_not_touched() {
    let here = Server::start_cmd();
    here.wait_text(CMD_PROMPT);
    let elsewhere = Server::start_cmd();
    elsewhere.wait_text(CMD_PROMPT);

    assert_handed_over(here.launch_again(&CMD_COMMAND));
    here.wait_until("two tabs", |state| tab_count(state) == 2);
    assert_handed_over(here.launch_again(&CMD_COMMAND));
    here.wait_until("three tabs", |state| tab_count(state) == 3);
    assert_eq!(tab_count(&elsewhere.state()), 1);
}

#[test]
fn a_launch_with_other_settings_opens_its_own_window() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let mut other = server.launch_again_with(
        &[],
        &LaunchAgain {
            config_toml: Some(ONE_PROFILE),
            ..Default::default()
        },
    );
    other.wait_for_window();
    other.wait_text(CMD_PROMPT);
    assert_eq!(tab_count(&server.state()), 1);
    // 同じ設定の起動は、Z オーダーで上にある別の設定のウィンドウを飛ばして届く。
    assert_handed_over(server.launch_again(&CMD_COMMAND));
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    assert_eq!(tab_count(&other.state()), 1);
}

#[test]
fn a_launch_of_another_executable_opens_its_own_window() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let mut other = server.launch_again_with(
        &["cmd.exe", "/k", "prompt", "copy$G"],
        &LaunchAgain {
            copy_of_executable: true,
            ..Default::default()
        },
    );
    other.wait_for_window();
    other.wait_text("copy>");
    assert_eq!(tab_count(&server.state()), 1);
}

#[test]
fn an_unknown_profile_in_a_second_launch_is_reported_by_the_window() {
    let server = Server::start(TWO_PROMPT_PROFILES, &[]);
    server.wait_text("two>");
    assert_handed_over(server.launch_again(&["--profile", "none"]));
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    wait_message_box_with(&server, "「none」");
    close_message_box(&server, 0);
    let state = server.wait_text("two>");
    assert_eq!(state["tabs"][1]["profile"], "two", "{state:#}");

    assert_handed_over(server.launch_again(&["--profile"]));
    server.wait_until("three tabs", |state| tab_count(state) == 3);
    wait_message_box_with(&server, "--profile");
    close_message_box(&server, 0);
}

#[test]
fn a_command_that_cannot_start_in_a_second_launch_is_reported_by_the_window() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    assert_handed_over(server.launch_again(&["fantermo-e2e-no-such-program.exe"]));
    wait_message_box_with(&server, "fantermo-e2e-no-such-program.exe");
    close_message_box(&server, 0);
    assert_eq!(tab_count(&server.state()), 1);
    server.type_line("echo after-%ERRORLEVEL%-end");
    server.wait_text("after-0-end");
}

#[test]
fn a_launch_waits_for_a_dialog_to_close() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    open_settings(&server);

    assert_handed_over(server.launch_again(&CMD_COMMAND));
    server.assert_holds_for(Duration::from_millis(500), "one tab", |state| {
        tab_count(state) == 1
    });
    press_cancel(&server);
    let state = server.wait_until("two tabs", |state| tab_count(state) == 2);
    assert_eq!(state["active"], 1);
}

#[test]
fn a_launch_waits_for_a_menu_to_close() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);
    open_menu(&server, &click_main_menu(&state));

    assert_handed_over(server.launch_again(&CMD_COMMAND));
    server.assert_holds_for(Duration::from_millis(500), "one tab", |state| {
        tab_count(state) == 1
    });
    close_menu(&server);
    server.wait_until("two tabs", |state| tab_count(state) == 2);
}

#[test]
fn a_launch_opens_its_own_window_when_the_window_does_not_respond() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    // 送る側が待つ時間より長く止める。
    server.post("/e2e/window/block/8000", b"");

    let mut other = server.launch_again(&["cmd.exe", "/k", "prompt", "other$G"]);
    other.wait_for_window();
    other.wait_text("other>");

    // 止まっていたウィンドウは動き続け、渡しそこねた指定を開かない。
    server.wait_text(CMD_PROMPT);
    server.assert_holds_for(Duration::from_millis(500), "one tab", |state| {
        tab_count(state) == 1
    });
    server.type_line("echo after-%ERRORLEVEL%-end");
    server.wait_text("after-0-end");
}

#[test]
fn a_launch_waiting_for_a_dialog_is_not_lost_when_the_window_closes() {
    let mut server = Server::start_watching_children(
        TWO_PROMPT_PROFILES,
        &["--profile", "none", "cmd.exe", "/k", "prompt", "e2e$G"],
    );
    // 警告のメッセージボックスの間に届いた起動は、開かれるのを待つ。
    wait_message_box_with(&server, "「none」");
    server.wait_text(CMD_PROMPT);
    let folder = working_folder(server.dir(), "reopened-here");
    assert_handed_over(server.launch_again_with(
        &["cmd.exe", "/k", "prompt", "reopened$G"],
        &LaunchAgain {
            working_folder: Some(&folder),
            ..Default::default()
        },
    ));
    assert_eq!(tab_count(&server.state()), 1);

    // その間にウィンドウが閉じると、新しいウィンドウで開く。
    std::fs::remove_file(server.dir().join("port")).expect("failed to remove the port file");
    server.post("/e2e/input", b"exit\r");
    server.wait_for_window();
    let state = server.wait_text("reopened>");
    assert_eq!(tab_count(&state), 1);
    server.type_line(PRINT_FOLDER_NAME);
    server.wait_text("folder-reopened-here-end");
}

/// [`send_copy_data_on_this_desktop`] が答えを待つ上限。
const COPY_DATA_TIMEOUT_MS: u32 = 30_000;
/// 起動の指定の `dwData`。
const LAUNCH_DATA_KIND: usize = 0x4654_4D01;

/// `hwnd` のウィンドウへ `WM_COPYDATA` を送り、戻り値を返す。`hwnd` は制御の口から読んだ、テストの fantermo のウィンドウ。
fn send_copy_data_on_this_desktop(hwnd: usize, kind: usize, bytes: &[u8]) -> isize {
    let data = COPYDATASTRUCT {
        dwData: kind,
        cbData: bytes.len() as u32,
        lpData: if bytes.is_empty() {
            std::ptr::null_mut()
        } else {
            bytes.as_ptr().cast_mut().cast()
        },
    };
    let mut result = 0;
    // SAFETY: data と bytes はこの呼び出しの間は生きている。応答しなくても時間で戻る。
    let sent = unsafe {
        SendMessageTimeoutW(
            HWND(hwnd as *mut _),
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(std::ptr::from_ref(&data) as isize),
            SMTO_ABORTIFHUNG,
            COPY_DATA_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    assert_ne!(
        sent.0,
        0,
        "the window did not answer WM_COPYDATA: {}",
        windows::core::Error::from_thread()
    );
    result as isize
}

/// 起動の指定の本文を、欄 `fields` から作る。
fn launch_bytes(fields: &[&str]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for field in fields {
        for unit in field.encode_utf16().chain([0]) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
    }
    bytes
}

#[test]
fn malformed_copy_data_is_ignored_and_the_terminal_keeps_working() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);
    let hwnd = usize::try_from(number(&state["hwnd"])).expect("the window handle does not fit");
    let send_copy_data = |kind: usize, bytes: &[u8]| {
        server.on_its_desktop(|| send_copy_data_on_this_desktop(hwnd, kind, bytes))
    };
    let exe = env!("CARGO_BIN_EXE_fantermo");
    let config = server.dir().join("config.toml");
    let config = config.to_str().expect("the path is not text");
    let valid = launch_bytes(&["0", exe, config, "", "", "cmd.exe /k prompt valid$G"]);

    let oversized = vec![b'a'; 4 << 20];
    let mut broken_text = launch_bytes(&["0", exe, config, "", ""]);
    broken_text.extend_from_slice(&[0x00, 0xD8, 0, 0]);
    let malformed: Vec<(usize, Vec<u8>)> = vec![
        (0, valid.clone()),
        (LAUNCH_DATA_KIND + 1, valid.clone()),
        (LAUNCH_DATA_KIND, Vec::new()),
        (LAUNCH_DATA_KIND, valid[..valid.len() - 1].to_vec()),
        (LAUNCH_DATA_KIND, valid[..valid.len() - 2].to_vec()),
        (LAUNCH_DATA_KIND, oversized),
        (LAUNCH_DATA_KIND, b"not the launch format".to_vec()),
        (LAUNCH_DATA_KIND, launch_bytes(&["0", exe, config, "", ""])),
        (
            LAUNCH_DATA_KIND,
            launch_bytes(&["0", exe, config, "", "", "cmd.exe", "more"]),
        ),
        (
            LAUNCH_DATA_KIND,
            launch_bytes(&["x", exe, config, "", "", "cmd.exe"]),
        ),
        (
            LAUNCH_DATA_KIND,
            launch_bytes(&["0", exe, config, "relative", "", "cmd.exe"]),
        ),
        (LAUNCH_DATA_KIND, broken_text),
    ];
    for (kind, bytes) in &malformed {
        assert_eq!(
            send_copy_data(*kind, bytes),
            0,
            "kind {kind:#x}, {} bytes",
            bytes.len()
        );
    }
    // 実行ファイルや設定ファイルが違う指定は、形が正しくても受け取らない。
    assert_eq!(
        send_copy_data(
            LAUNCH_DATA_KIND,
            &launch_bytes(&["0", r"C:\other\fantermo.exe", config, "", "", "cmd.exe"])
        ),
        0
    );
    assert_eq!(
        send_copy_data(
            LAUNCH_DATA_KIND,
            &launch_bytes(&["0", exe, r"C:\other\config.toml", "", "", "cmd.exe"])
        ),
        0
    );

    server.assert_holds_for(Duration::from_millis(300), "one tab", |state| {
        tab_count(state) == 1 && state["dialogs"].as_array().map(Vec::len) == Some(0)
    });
    assert!(
        server.log().contains("ignoring a malformed WM_COPYDATA"),
        "{}",
        server.log()
    );
    server.type_line("echo after-%ERRORLEVEL%-end");
    server.wait_text("after-0-end");

    // 同じ手段で正しい形のものなら開く。
    assert_eq!(send_copy_data(LAUNCH_DATA_KIND, &valid), 1);
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    server.wait_text("valid>");
}

#[test]
fn the_tabs_of_a_second_launch_end_like_the_others() {
    let mut server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    assert_handed_over(server.launch_again(&["cmd.exe", "/k", "prompt", "second$G"]));
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    server.wait_text("second>");

    server.type_line("exit 3");
    server.wait_until("one tab", |state| tab_count(state) == 1);
    server.type_line("exit 7");
    assert_eq!(server.wait_exit(), 7);
}

/// ブリッジのプロファイルのタブを渡して、作業フォルダが届き、コマンドの指定も効くことを確認する。
fn exercise_second_launch_with_bridge(config: &str, profile: &str) {
    let server = Server::start(config, &CMD_COMMAND);
    server.wait_text(CMD_PROMPT);
    let folder = working_folder(server.dir(), "bridge-from-here");
    assert_handed_over(server.launch_again_with(
        &["--profile", profile],
        &LaunchAgain {
            working_folder: Some(&folder),
            ..Default::default()
        },
    ));
    let state = server.wait_until("two tabs", |state| tab_count(state) == 2);
    assert_eq!(state["tabs"][1]["profile"], profile, "{state:#}");
    server.type_line(r#"echo folder-$(basename "$PWD")-end"#);
    server.wait_text("folder-bridge-from-here-end");

    assert_handed_over(server.launch_again(&["--profile", profile, "PS1=zz-prompt:", "/bin/sh"]));
    let state = server.wait_until("three tabs", |state| tab_count(state) == 3);
    assert_eq!(state["tabs"][2]["profile"], profile, "{state:#}");
    server.wait_text("zz-prompt:");
}

#[test]
fn cygwin_second_launch_opens_a_bridge_tab() {
    let Some(config) = cygwin_config("cygwin_second_launch_opens_a_bridge_tab") else {
        return;
    };
    exercise_second_launch_with_bridge(&config, "cygwin");
}

#[test]
fn wsl_second_launch_opens_a_bridge_tab() {
    let Some(config) = wsl_config("wsl_second_launch_opens_a_bridge_tab") else {
        return;
    };
    exercise_second_launch_with_bridge(&config, "wsl");
}
