use crate::common;
use crate::common::OsClipboard;
use crate::common::RestoreClipboard;
use crate::common::Server;
use crate::ui::{
    MENU_CLASS, SYSTEM_DIALOG_CLASS, config_path, cygwin_config, dialog_text, open_menu,
    right_click_terminal, tab_count, wait_dialog_of_class, wait_sh_ready,
};

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

/// 確認のダイアログの上に設定ファイルの警告が重なり、エラーが直って警告を取り下げても、確認のダイアログは
/// 残って答えを受ける。
#[test]
fn cygwin_clipboard_read_dialog_stays_when_the_settings_warning_is_withdrawn() {
    let Some(config) = cygwin_config("cygwin_clipboard_read_dialog_stays") else {
        return;
    };
    let server = Server::start(&config, &[]);
    wait_sh_ready(&server);
    server.set_clipboard(CLIPBOARD_TEXT);

    start_clipboard_read(&server, "withdrawn");
    server.wait_dialogs(1);
    let path = config_path(&server);
    std::fs::write(&path, format!("[font\n{config}")).expect("failed to write the settings");
    server.wait_dialogs(2);
    std::fs::write(&path, &config).expect("failed to write the settings");
    let state = server.wait_dialogs(1);
    // 残ったのは確認のダイアログで、主ウィンドウは無効のまま。
    assert!(
        dialog_text(&state["dialogs"][0]).contains("許可(&A)"),
        "{state:#}"
    );
    assert_eq!(state["window_enabled"], false, "{state:#}");

    server.press_dialog_button(BUTTON_ALLOW);
    server.wait_dialogs(0);
    let answer = finish_clipboard_read(&server, "withdrawn");
    assert!(
        answer.contains(&clipboard_answer(CLIPBOARD_TEXT)),
        "{answer}"
    );
}

/// メニューを開いている間に裏のタブから読み取りの依頼が届いても、確認のダイアログはメニューを閉じてから開く。
#[test]
fn cygwin_clipboard_read_dialog_waits_for_the_menu_to_close() {
    let Some(config) = cygwin_config("cygwin_clipboard_read_dialog_waits_for_the_menu") else {
        return;
    };
    let server = Server::start(&config, &[]);
    wait_sh_ready(&server);
    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    wait_sh_ready(&server);

    // 裏に回したあとで依頼を出させるため、ファイルができるのを待ってから依頼し、タイトルで知らせる。
    // ファイルは設定ファイルと同じフォルダに置くが、設定の監視はファイルの名前で照合して捨てる。
    let trigger = server.dir().join("clipboard-read-trigger");
    server.send_line(&format!(
        r#"t=$(cygpath -u '{}'); echo waiting-$((1+1)); while [ ! -e "$t" ]; do sleep 0.1; done; printf '\033]52;c;?\007\033]2;read-sent\007'"#,
        trigger.display()
    ));
    server.wait_text("waiting-2");
    server.post("/e2e/tab/select/0", b"");
    let state = server.wait_until("the first tab", |state| state["active"] == 0);
    open_menu(&server, &right_click_terminal(&state));

    std::fs::write(&trigger, b"").expect("failed to write the trigger");
    server.wait_until("the read from the other tab", |state| {
        state["tabs"][1]["title"] == "read-sent"
    });
    server.wait_until("the read waiting for the menu", |_| {
        server
            .log()
            .contains("a clipboard read waits for the menu to close")
    });
    let state = server.state();
    let only_the_menu = state["dialogs"].as_array().is_some_and(|dialogs| {
        !dialogs.is_empty() && dialogs.iter().all(|dialog| dialog["class"] == MENU_CLASS)
    });
    assert!(only_the_menu, "{state:#}");

    server.post("/e2e/menu/close", b"");
    wait_dialog_of_class(&server, SYSTEM_DIALOG_CLASS);
    server.press_dialog_button(BUTTON_DENY);
    server.wait_dialogs(0);
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

#[test]
fn cygwin_clipboard_write_denied_by_the_setting_leaves_the_clipboard() {
    let Some(config) = cygwin_config("cygwin_clipboard_write_denied") else {
        return;
    };
    let config = format!("[clipboard]\nread = \"allow\"\nwrite = \"deny\"\n{config}");
    let server = Server::start(&config, &[]);
    wait_sh_ready(&server);
    server.set_clipboard(CLIPBOARD_TEXT);
    let written = format!("{CLIPBOARD_TEXT}-written");
    server.send_line(&format!(
        r"printf '\033]52;c;{}\007'",
        common::base64(written.as_bytes())
    ));
    // 書き込みのあとの読み取りの答えが元の内容なら、書き込みは処理されたうえで捨てられている。
    start_clipboard_read(&server, "after-write");
    let answer = finish_clipboard_read(&server, "after-write");
    assert!(
        answer.contains(&clipboard_answer(CLIPBOARD_TEXT)),
        "{answer}"
    );
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
