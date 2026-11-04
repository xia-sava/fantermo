//! 管理者として開く。feature `e2e` のビルドは `ShellExecuteExW` の `runas` を呼ばず、起動の指定をファイルに書くか、
//! 環境変数で指定した結果を返す（`crates/fantermo/src/elevate/stand_in.rs`）。UAC の確認は出ない。
//!
//! 管理者かどうか（ウィンドウの印、メニュー、`--elevate`）は `FANTERMO_E2E_ELEVATED` だけで決まり、テストを動かす
//! プロセスが管理者でも、付けなければ管理者ではない。

use crate::common;
use crate::common::CMD_COMMAND;
use crate::common::CMD_PROMPT;
use crate::common::LaunchAgain;
use crate::common::Server;
use crate::ui::{
    DPI_200_PERCENT, ONE_PROFILE, PICTURE_POLL_INTERVAL, PICTURE_TIMEOUT, area_center, change_dpi,
    check_terminal_still_works, close_menu, color_is_close, left_click, menu_position, menu_shape,
    number, open_menu, open_submenu, select_item, tab_count, wait_message_box_with,
};
use serde_json::Value;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

/// このプロセスを管理者として扱わせる環境変数。
const ELEVATED_ENV: &str = "FANTERMO_E2E_ELEVATED";
/// 起動する代わりに起動の指定を書くファイルの環境変数。
const RECORD_ENV: &str = "FANTERMO_E2E_ELEVATE_RECORD";
/// 起動の結果を決める環境変数。
const RESULT_ENV: &str = "FANTERMO_E2E_ELEVATE_RESULT";

/// 起動する代わりに書かれた、実行ファイル・引数・作業フォルダ。
#[derive(Debug)]
struct Recorded {
    exe: PathBuf,
    parameters: String,
    directory: PathBuf,
}

/// `path` に書かれた起動の指定を読む。
#[track_caller]
fn read_record(path: &Path) -> Recorded {
    let text = std::fs::read_to_string(path).expect("the elevated launch was not recorded");
    let lines: Vec<&str> = text.lines().collect();
    let [exe, parameters, directory] = lines[..] else {
        panic!("unexpected record: {text:?}");
    };
    Recorded {
        exe: PathBuf::from(exe),
        parameters: parameters.to_owned(),
        directory: PathBuf::from(directory),
    }
}

#[track_caller]
fn assert_same_file(left: &Path, right: &Path) {
    let canonical = |path: &Path| {
        std::fs::canonicalize(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    };
    assert_eq!(canonical(left), canonical(right));
}

/// 記録のファイルを `path` にする環境変数。
fn record_at(path: &Path) -> (&'static str, OsString) {
    (RECORD_ENV, path.as_os_str().to_owned())
}

/// `--elevate` は、動いているウィンドウへ渡さずに管理者の権限の fantermo を起動して終わる。渡す引数は `--elevate` を
/// 除いて組み立て直したもので、作業フォルダは起動したときの作業フォルダ。UAC の確認で断られたら、何も開かずに終わる。
/// すでに管理者なら、管理者としての起動はせず、そのまま開く。管理者のウィンドウのタイトルだけ先頭に印が付く。
#[test]
fn the_elevate_option_starts_an_elevated_fantermo_and_ends() {
    let server = Server::start(ONE_PROFILE, &[]);
    server.wait_text(CMD_PROMPT);
    let record = server.dir().join("elevated.txt");

    let mut launched = server.launch_again_with(
        &["--elevate", "--profile", "one", "cmd.exe", "/k", "x"],
        &LaunchAgain {
            environment: vec![record_at(&record)],
            ..Default::default()
        },
    );
    assert_eq!(launched.wait_exit(), 0, "{}", launched.log());
    assert!(!launched.opened_a_window(), "{}", launched.log());
    let recorded = read_record(&record);
    assert_same_file(&recorded.exe, Path::new(env!("CARGO_BIN_EXE_fantermo")));
    assert_eq!(recorded.parameters, r#"--profile "one" -- cmd.exe /k x"#);
    assert_same_file(&recorded.directory, server.dir());
    assert_eq!(tab_count(&server.state()), 1);
    std::fs::remove_file(&record).expect("failed to remove the record");

    // UAC の確認で断ると、何も見せずに終わる。
    let mut cancelled = server.launch_again_with(
        &["--elevate"],
        &LaunchAgain {
            environment: vec![record_at(&record), (RESULT_ENV, "cancelled".into())],
            ..Default::default()
        },
    );
    assert_eq!(cancelled.wait_exit(), 1, "{}", cancelled.log());
    assert!(!cancelled.opened_a_window(), "{}", cancelled.log());
    assert!(!record.exists());
    assert_eq!(tab_count(&server.state()), 1);

    // すでに管理者なら、そのまま開く。
    let mut elevated = server.launch_again_with(
        &["--elevate", "--new-window", "--profile", "one"],
        &LaunchAgain {
            environment: vec![record_at(&record), (ELEVATED_ENV, "1".into())],
            ..Default::default()
        },
    );
    elevated.wait_for_window();
    let state = elevated.wait_text(CMD_PROMPT);
    assert_eq!(state["tabs"][0]["profile"], "one", "{state:#}");
    assert_ne!(state["pid"], server.state()["pid"], "{state:#}");
    // 管理者のウィンドウのタイトルだけ、先頭に印が付く。
    let title = state["window_title"].as_str().expect("no window title");
    assert!(title.starts_with("管理者: "), "{title}");
    let normal = server.state();
    let normal_title = normal["window_title"].as_str().expect("no window title");
    assert!(!normal_title.starts_with("管理者: "), "{normal_title}");
    assert!(!record.exists());
    assert_eq!(tab_count(&server.state()), 1);
}

/// プロファイルが三つの設定。既定は 2 つ目で、3 つ目の名前は引用が要る。
const THREE_PROFILES: &str = r#"default_profile = "two"

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

/// 開いているウィンドウのプロファイルの一覧を開き、項目を返す。
#[track_caller]
fn open_profile_menu(server: &Server) -> Vec<Value> {
    let state = server.state();
    open_menu(
        server,
        &left_click(area_center(&state["tab_bar"]["profiles"])),
    )
}

/// プロファイルの一覧の末尾に「管理者として開く」のサブメニューがあり、盾が付く。中はプロファイルの一覧と同じ並び・印・
/// アイコン。管理者のウィンドウの一覧には出さない。
#[test]
fn the_profile_menu_offers_the_elevate_submenu_unless_elevated() {
    {
        let server = Server::start(THREE_PROFILES, &CMD_COMMAND);
        server.wait_text(CMD_PROMPT);
        let items = open_profile_menu(&server);
        assert_eq!(menu_shape(&items), "1 2* 3 | A", "{items:#?}");
        let elevate = &items[menu_position(&items, 'A')];
        assert_eq!(elevate["text"], "管理者として開く(&A)", "{items:#?}");
        assert!(
            elevate["submenu"] == true && elevate["icon"] == true,
            "{items:#?}"
        );
        let inner = open_submenu(&server, &items, 'A');
        assert_eq!(menu_shape(&inner), "1 2* 3", "{inner:#?}");
        let texts: Vec<&Value> = inner.iter().map(|item| &item["text"]).collect();
        assert_eq!(texts, ["&1 one", "&2 two", "&3 R&&D"], "{inner:#?}");
        assert!(
            inner
                .iter()
                .all(|item| item["icon"] == true && item["submenu"] == false),
            "{inner:#?}"
        );
        close_menu(&server);
    }

    let elevated = Server::start_with_environment(THREE_PROFILES, &CMD_COMMAND, |_| {
        vec![(ELEVATED_ENV, "1".into())]
    });
    elevated.wait_text(CMD_PROMPT);
    let items = open_profile_menu(&elevated);
    assert_eq!(menu_shape(&items), "1 2* 3", "{items:#?}");
    close_menu(&elevated);
}

/// 「管理者として開く」の中の `accelerator_of` のプロファイルを選ぶ。
#[track_caller]
fn choose_elevated_profile(server: &Server, accelerator_of: char) {
    let items = open_profile_menu(server);
    let inner = open_submenu(server, &items, 'A');
    select_item(server, &inner, accelerator_of);
}

/// 「管理者として開く」で選んだプロファイルは、その名前だけを渡して管理者の権限の fantermo を起動する（このウィンドウの
/// 引数は渡さない）。作業フォルダはこの fantermo の作業フォルダ。このウィンドウにはタブを足さない。
#[test]
fn choosing_from_the_elevate_submenu_starts_an_elevated_fantermo() {
    const RECORD: &str = "elevated.txt";
    let server = Server::start_with_environment(THREE_PROFILES, &CMD_COMMAND, |dir| {
        vec![record_at(&dir.join(RECORD))]
    });
    server.wait_text(CMD_PROMPT);
    let record = server.dir().join(RECORD);

    choose_elevated_profile(&server, '3');
    server.wait_dialogs(0);
    let deadline = Instant::now() + PICTURE_TIMEOUT;
    while !record.exists() {
        assert!(Instant::now() < deadline, "no record\n{}", server.log());
        std::thread::sleep(PICTURE_POLL_INTERVAL);
    }
    let recorded = read_record(&record);
    assert_same_file(&recorded.exe, Path::new(env!("CARGO_BIN_EXE_fantermo")));
    assert_eq!(recorded.parameters, r#"--profile "R&D""#);
    assert_same_file(&recorded.directory, server.dir());
    let state = server.state();
    assert_eq!(tab_count(&state), 1, "{state:#}");
    assert_eq!(state["dialogs"].as_array().map(Vec::len), Some(0));
    check_terminal_still_works(&server, "after-elevate");
}

/// UAC の確認で断られたら何も見せない。ほかの理由で起動できなければ、警告のメッセージボックスで知らせる。どちらも
/// タブは足さず、端末は動き続ける。
#[test]
fn a_refused_or_failed_elevation_from_the_menu_keeps_the_window() {
    let cancelled = Server::start_with_environment(THREE_PROFILES, &CMD_COMMAND, |_| {
        vec![(RESULT_ENV, "cancelled".into())]
    });
    cancelled.wait_text(CMD_PROMPT);
    choose_elevated_profile(&cancelled, '1');
    cancelled.wait_dialogs(0);
    check_terminal_still_works(&cancelled, "after-cancel");
    assert_eq!(tab_count(&cancelled.state()), 1);
    let log = cancelled.log();
    assert!(log.contains("as administrator was cancelled"), "{log}");
    drop(cancelled);

    let failed = Server::start_with_environment(THREE_PROFILES, &CMD_COMMAND, |_| {
        vec![(RESULT_ENV, "failed".into())]
    });
    failed.wait_text(CMD_PROMPT);
    choose_elevated_profile(&failed, '1');
    wait_message_box_with(&failed, "管理者として起動できませんでした。");
    failed.post("/e2e/dialog/close", b"");
    failed.wait_dialogs(0);
    check_terminal_still_works(&failed, "after-failure");
    assert_eq!(tab_count(&failed.state()), 1);
}

/// タブのアイコンにする、一色の赤の画像。
const RED_ICON: &str = "red-icon.png";
const RED_ICON_SIDE: u32 = 64;
const RED: [u8; 3] = [0xff, 0x00, 0x00];
/// 96 DPI と 192 DPI のタブのアイコンの一辺と、そこに重ねる盾の一辺。
const TAB_ICON_AT_100_PERCENT: u32 = 18;
const SHIELD_AT_100_PERCENT: u32 = 10;
const TAB_ICON_AT_200_PERCENT: u32 = 36;
const SHIELD_AT_200_PERCENT: u32 = 18;

/// 画像のアイコンのプロファイルが一つの設定。
fn red_icon_profile() -> String {
    format!(
        r#"[[profiles]]
name = "red"
command = "cmd.exe /k prompt red$G"
icon = '{RED_ICON}'
"#
    )
}

/// 一時フォルダ `dir` に [`RED_ICON`] を書く。
fn write_red_icon(dir: &Path) {
    let pixels = RED.repeat((RED_ICON_SIDE * RED_ICON_SIDE) as usize);
    std::fs::write(
        dir.join(RED_ICON),
        common::encode_png(RED_ICON_SIDE, RED_ICON_SIDE, &pixels),
    )
    .expect("failed to write the icon");
}

/// 最初のタブのアイコンの、赤くない画素のアイコンの中の位置と色。アイコンは [`RED`] の画素を囲む矩形で、一辺が
/// `side` になるまで絵を読み直す（盾はアイコンの右下にだけ重なるので、上の辺と左の辺は赤のまま残る）。
#[track_caller]
fn wait_tab_icon_marks(server: &Server, side: u32) -> Vec<(u32, u32, [u8; 3])> {
    let deadline = Instant::now() + PICTURE_TIMEOUT;
    loop {
        let state = server.state();
        let picture = server.picture();
        let area = &state["tab_bar"]["tabs"][0]["area"];
        let coordinate = |value: &Value| u32::try_from(number(value)).expect("the position fits");
        let (x, y) = (coordinate(&area["x"]), coordinate(&area["y"]));
        let (width, height) = (coordinate(&area["width"]), coordinate(&area["height"]));
        let red: Vec<(u32, u32)> = (y..y + height)
            .flat_map(|py| (x..x + width).map(move |px| (px, py)))
            .filter(|&(px, py)| color_is_close(picture.rgb(px, py), RED))
            .collect();
        let left = red.iter().map(|&(px, _)| px).min();
        let top = red.iter().map(|&(_, py)| py).min();
        let right = red.iter().map(|&(px, _)| px).max();
        let bottom = red.iter().map(|&(_, py)| py).max();
        if let (Some(left), Some(top), Some(right), Some(bottom)) = (left, top, right, bottom)
            && right + 1 - left == side
            && bottom + 1 - top == side
        {
            return (top..top + side)
                .flat_map(|py| (left..left + side).map(move |px| (px, py)))
                .map(|(px, py)| (px - left, py - top, picture.rgb(px, py)))
                .filter(|&(_, _, color)| !color_is_close(color, RED))
                .collect();
        }
        assert!(
            Instant::now() < deadline,
            "no {side} px red icon in the first tab\n{state:#}\n{}",
            server.log()
        );
        std::thread::sleep(PICTURE_POLL_INTERVAL);
    }
}

/// 一辺 `side` のアイコンの赤くない画素 `marks` が、右下の一辺 `shield` の中だけにあり、盾の青を含む。
#[track_caller]
fn assert_shield_in_the_corner(marks: &[(u32, u32, [u8; 3])], side: u32, shield: u32) {
    let corner = side - shield;
    assert!(
        marks.iter().all(|&(x, y, _)| x >= corner && y >= corner),
        "marks outside the bottom right {shield} px: {marks:?}"
    );
    assert!(
        marks.iter().any(|&(_, _, [r, _, b])| b > 0x80 && r < 0x80),
        "no blue of the shield: {marks:?}"
    );
}

/// 管理者のウィンドウのタブのアイコンは、右下に UAC の盾が重なる。DPI が変わってアイコンを作り直しても重なる。
/// 管理者でないウィンドウのアイコンには重ならない。
#[test]
fn elevated_tab_icons_carry_the_shield() {
    let config = red_icon_profile();
    let normal = Server::start_with_setup(&config, &[], write_red_icon);
    normal.wait_text("red>");
    assert_eq!(
        wait_tab_icon_marks(&normal, TAB_ICON_AT_100_PERCENT),
        [],
        "the icon of a window that is not elevated"
    );

    let elevated = Server::start_with_environment(&config, &[], |dir| {
        write_red_icon(dir);
        vec![(ELEVATED_ENV, "1".into())]
    });
    elevated.wait_text("red>");
    let marks = wait_tab_icon_marks(&elevated, TAB_ICON_AT_100_PERCENT);
    assert_shield_in_the_corner(&marks, TAB_ICON_AT_100_PERCENT, SHIELD_AT_100_PERCENT);

    elevated.resize(50, 12);
    change_dpi(&elevated, DPI_200_PERCENT);
    let marks = wait_tab_icon_marks(&elevated, TAB_ICON_AT_200_PERCENT);
    assert_shield_in_the_corner(&marks, TAB_ICON_AT_200_PERCENT, SHIELD_AT_200_PERCENT);
}
