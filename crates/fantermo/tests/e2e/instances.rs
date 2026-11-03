use crate::common::CMD_COMMAND;
use crate::common::CMD_PROMPT;
use crate::common::LONG_WAIT_CHILDREN_SECONDS;
use crate::common::LaunchAgain;
use crate::common::Server;
use crate::ui::{
    ONE_PROFILE, TWO_PROMPT_PROFILES, click_main_menu, close_menu, close_message_box,
    cygwin_config, number, open_menu, open_settings, press_cancel, tab_count,
    wait_message_box_with, wsl_config,
};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::SMTO_ABORTIFHUNG;
use windows::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW;
use windows::Win32::UI::WindowsAndMessaging::WM_COPYDATA;

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
    let state = server.open_tab("two>");
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
    server.open_tab("first>");
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
    // 閉じたあとに開き直したウィンドウは run-hidden の Job の中にいるので、run-hidden が子孫を待っているあいだだけ
    // 生きる。ウィンドウが開いてからの操作を終えるまで生かすため、待つ秒数を長くする。
    let mut server = Server::start_watching_children_for(
        TWO_PROMPT_PROFILES,
        &["--profile", "none", "cmd.exe", "/k", "prompt", "e2e$G"],
        LONG_WAIT_CHILDREN_SECONDS,
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
