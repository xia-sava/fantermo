use crate::common::Server;
use crate::ui::{
    CMD_COMMAND, CMD_PROMPT, current_window, number, quit, saved_window, seeded_state,
    start_with_state,
};
use fantermo_core::state;
use fantermo_core::state::WindowState;
use serde_json::Value;

/// 状態がないときのウィンドウの桁数と行数。
const DEFAULT_GRID: (u64, u64) = (100, 30);
/// 読めなかった状態のファイルを写す先の名前。
const BROKEN_STATE_FILE: &str = "state.toml.broken";

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
