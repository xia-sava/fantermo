use crate::common::Server;
use crate::ui::{TWO_PROMPT_PROFILES, close_message_box, tab_count, wait_message_box_with};
use serde_json::Value;
use std::ffi::OsString;
use std::path::PathBuf;

#[test]
fn the_profile_option_chooses_the_profile_of_the_tabs() {
    let server = Server::start(TWO_PROMPT_PROFILES, &["--profile", "one"]);
    let state = server.wait_text("one>");
    assert_eq!(state["tabs"][0]["profile"], "one", "{state:#}");
    assert_eq!(state["dialogs"].as_array().map(Vec::len), Some(0));

    // 新しいタブも引数と同じ決め方で開く。一覧から選べばそのプロファイル。
    let state = server.open_tab("one>");
    assert_eq!(state["tabs"][1]["profile"], "one", "{state:#}");
    server.post("/e2e/tab/new/1", b"");
    server.wait_until("three tabs", |state| tab_count(state) == 3);
    let state = server.wait_text("two>");
    assert_eq!(state["tabs"][2]["profile"], "two", "{state:#}");
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
fn a_profile_with_a_command_runs_the_command_in_that_profile() {
    let server = Server::start(
        TWO_PROMPT_PROFILES,
        &["--profile", "one", "cmd.exe", "/k", "prompt", "command$G"],
    );
    let state = server.wait_text("command>");
    assert_eq!(state["tabs"][0]["profile"], "one", "{state:#}");

    let state = server.open_tab("command>");
    assert_eq!(state["tabs"][1]["profile"], "one", "{state:#}");

    // 一覧から選ぶと、そのプロファイルの `command`。
    server.post("/e2e/tab/new/0", b"");
    server.wait_until("three tabs", |state| tab_count(state) == 3);
    server.wait_text("one>");
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
