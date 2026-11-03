use crate::common::CMD_COMMAND;
use crate::common::CMD_PROMPT;
use crate::common::Server;
use crate::ui::{assert_the_shell_sees_the_grid, number, run_with_cygwin, run_with_wsl, tab_count};
use std::io::Cursor;
use std::path::PathBuf;

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

/// 壊れたときに原因を切り分ける最小のテスト。`smoke` の段で流す。
#[test]
fn smoke_powershell_shows_a_prompt() {
    let server = Server::start_powershell();
    server.wait_text("PS ");
}

/// 名前だけのコマンドは PATH だけから探し、作業フォルダに置いた同じ名前のファイルは起動しない。作業フォルダに
/// `cmd.exe` の写しを `powershell.exe` の名前で置いて、PowerShell のプロンプトになることを見る。
#[test]
fn powershell_in_the_working_folder_is_not_started() {
    let server =
        Server::start_with_setup("", &["powershell.exe", "-NoLogo", "-NoProfile"], |dir| {
            let system = std::env::var_os("SystemRoot").expect("no SystemRoot");
            let cmd = PathBuf::from(system).join("System32").join("cmd.exe");
            std::fs::copy(&cmd, dir.join("powershell.exe")).expect("failed to copy cmd.exe");
        });
    server.wait_text("PS ");
}

/// 打った入力がシェルに届いて出力を読める。`/e2e/input` の入力はそのままバックエンドへ届く。
#[test]
fn input_reaches_the_shell_and_its_output_is_read() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    // 打った行そのものには出ない文字列を待つ。
    server.type_line("echo marker-%ERRORLEVEL%-end");
    server.wait_text("marker-0-end");
    server.post("/e2e/input", b"echo raw-%ERRORLEVEL%-end\r");
    server.wait_text("raw-0-end");
}

#[test]
fn tabs_open_switch_and_close() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.type_line("echo first-%ERRORLEVEL%-tab");
    server.wait_text("first-0-tab");

    let state = server.open_tab(CMD_PROMPT);
    assert_eq!(state["active"], 1);
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
    server.wait_text(CMD_PROMPT);
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

    let state = server.resize(87, 23);
    assert_eq!(state["cols"], 87, "{state:#}");
    assert_eq!(state["rows"], 23, "{state:#}");
    assert_the_shell_sees_the_grid(&server, &state, "first");

    // GitHub Actions のランナーの画面は 1024×768 で、それより大きいウィンドウにはならない。
    let state = server.resize(96, 30);
    assert_eq!(state["cols"], 96, "{state:#}");
    assert_eq!(state["rows"], 30, "{state:#}");
    assert_the_shell_sees_the_grid(&server, &state, "second");
}

/// 最小化中に書き換える余白。
const MINIMIZED_PADDING: u64 = 40;

/// 最小化しても桁数と行数は変わらず、戻すと元の大きさになる。最小化中に設定を読み直しても同じで、戻したときに
/// 新しい余白とフォントで決め直す。
#[test]
fn minimizing_the_window_keeps_the_grid() {
    let server = Server::start("[font]\nsize = 10\n", &CMD_COMMAND);
    server.wait_text(CMD_PROMPT);
    server.resize(87, 23);
    let before = server.state();
    assert_eq!(before["cols"], 87, "{before:#}");
    assert_eq!(before["rows"], 23, "{before:#}");

    let minimized = server.minimize();
    assert_eq!(number(&minimized["client_width"]), 0, "{minimized:#}");
    assert_eq!(minimized["cols"], before["cols"], "{minimized:#}");
    assert_eq!(minimized["rows"], before["rows"], "{minimized:#}");

    let restored = server.restore();
    assert_eq!(restored["cols"], before["cols"], "{restored:#}");
    assert_eq!(restored["rows"], before["rows"], "{restored:#}");

    // 最小化中の設定の読み直しは、桁数と行数を変えない。
    server.minimize();
    let cell_height = number(&before["cell_height"]);
    std::fs::write(
        server.dir().join("config.toml"),
        format!("[font]\nsize = 20\n[window]\npadding = {MINIMIZED_PADDING}\n"),
    )
    .expect("failed to write the settings");
    let reloaded = server.wait_until("taller cells", |state| {
        number(&state["cell_height"]) > cell_height
    });
    assert_eq!(reloaded["cols"], before["cols"], "{reloaded:#}");
    assert_eq!(reloaded["rows"], before["rows"], "{reloaded:#}");

    let restored = server.restore();
    assert_eq!(
        number(&restored["terminal_x"]),
        MINIMIZED_PADDING,
        "{restored:#}"
    );
    assert!(
        number(&restored["cols"]) < number(&before["cols"]),
        "{restored:#}"
    );
}

#[test]
fn shell_exit_closes_its_tab_and_then_the_window() {
    let mut server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.open_tab(CMD_PROMPT);

    server.type_line("exit 3");
    server.wait_until("one tab", |state| tab_count(state) == 1);

    server.type_line("exit 7");
    assert_eq!(server.wait_exit(), 7);
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
    run_with_cygwin("cygwin_bridge_runs_a_shell", exercise_bridge_profile);
}

#[test]
fn wsl_bridge_runs_a_shell() {
    run_with_wsl("wsl_bridge_runs_a_shell", exercise_bridge_profile);
}

#[test]
fn each_tab_keeps_its_own_title() {
    let server = Server::start_powershell();
    server.wait_text("PS ");
    server.type_line("[Console]::Write([char]27 + ']2;title-one' + [char]7)");
    wait_window_title(&server, "title-one");

    server.open_tab("PS ");
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
fn the_settings_are_read_again_when_the_file_changes() {
    let server = Server::start("[font]\nsize = 10\n", &CMD_COMMAND);
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
