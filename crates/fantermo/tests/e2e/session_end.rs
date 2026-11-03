use crate::common::CMD_COMMAND;
use crate::common::CMD_PROMPT;
use crate::common::Server;
use crate::common::processes;
use crate::ui::{
    CMD_LONG_COMMAND, CMD_LONG_PROCESS, SH_LONG_COMMAND, current_window, cygwin_config, quit,
    saved_window, start_with_state, tab_count, unique_sleep_seconds, wait_cmd_ready,
    wait_cygwin_sleep, wait_sh_ready, wait_text_dialog, wsl_config,
};

/// ログオフの問い合わせ（`WM_QUERYENDSESSION`）と、その結果の知らせ（`WM_ENDSESSION`）の名前。
const QUERY_END_SESSION: &str = "queryendsession";
const END_SESSION: &str = "endsession";
/// ウィンドウを閉じる知らせ（`WM_CLOSE`）の名前。
const CLOSE: &str = "close";
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

/// App を借りている間の処理の中で問い合わせと知らせが届いても（ほかのスレッドから送られて、借用の中の同期の
/// 呼び出しの間に処理されたとき）、状態を書く。借用を手放したあとも端末は動き続ける。
#[test]
fn the_session_end_saves_the_state_while_the_app_is_borrowed() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.resize(87, 23);
    let window = current_window(&server.state());
    assert!(!server.state_file().exists());

    assert_eq!(
        server.send_message_in_borrow(QUERY_END_SESSION, 0),
        SESSION_MAY_END,
        "{}",
        server.log()
    );
    assert_eq!(saved_window(&server), window);
    remove_state_file(&server);
    assert_eq!(server.send_message_in_borrow(END_SESSION, SESSION_ENDS), 0);
    assert_eq!(saved_window(&server), window);
    check_cmd_responds(&server);
}

/// App を借りている間の処理の中で `WM_CLOSE` が届いたら、その場では閉じず、借用を手放したあとで状態を書いて閉じる。
#[test]
fn a_close_while_the_app_is_borrowed_closes_the_window_afterwards() {
    let mut server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.resize(87, 23);
    let window = current_window(&server.state());
    assert!(!server.state_file().exists());

    assert_eq!(server.send_message_in_borrow(CLOSE, 0), 0);
    assert_eq!(server.wait_exit(), 0, "{}", server.log());
    assert_eq!(saved_window(&server), window);
}
