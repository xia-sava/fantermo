use crate::common;
use crate::common::CMD_COMMAND;
use crate::common::CMD_PROMPT;
use crate::common::Server;
use crate::common::processes;
use fantermo_core::state;
use fantermo_core::state::State;
use fantermo_core::state::WindowState;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use std::time::Instant;

/// Cygwin の `sleep` が動き出す、または終わるのを待つ上限。
pub const CYGWIN_SLEEP_TIMEOUT: Duration = Duration::from_secs(60);

pub fn tab_count(state: &Value) -> usize {
    state["tabs"].as_array().map_or(0, Vec::len)
}

pub fn number(value: &Value) -> u64 {
    value
        .as_u64()
        .unwrap_or_else(|| panic!("not a number: {value}"))
}

/// Cygwin のブリッジのプロファイルの設定。ブリッジを置く。環境変数がなければスキップしたと知らせて `None`。
pub fn cygwin_config(test: &str) -> Option<String> {
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
pub fn wsl_config(test: &str) -> Option<String> {
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

/// `ping -t` は止めるまで動く。
pub const CMD_LONG_COMMAND: &str = "ping -t 127.0.0.1";
pub const CMD_LONG_PROCESS: &str = "ping.exe";

/// Cygwin の `sleep` の印を `/proc` で確認する間隔。確認のたびに Cygwin の `sh.exe` と `grep` を起動するので、
/// 間を置かないと、並列に動くほかのテストの Cygwin のプロセスの生成を妨げる。
pub const CYGWIN_SLEEP_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// `/bin/sh` で長く動くコマンド。
pub const SH_LONG_COMMAND: &str = "sleep 300";

/// `/bin/sh` が入力を読むのを待つ。プロンプトは環境によって違うので、コマンドの結果で見る。
pub fn wait_sh_ready(server: &Server) {
    server.type_line("echo ready-$((1+1))");
    server.wait_text("ready-2");
}

/// 接続方式（ConPTY・Cygwin・WSL）ごとの、起動の準備と動作の確認の手順。
pub struct BackendSetup {
    /// 設定の TOML。
    pub config: String,
    /// fantermo の引数。
    pub arguments: &'static [&'static str],
    /// 表示しているタブのシェルが入力を受け付けるのを待つ。
    pub wait_ready: fn(&Server),
    /// 表示しているタブのシェルが入力に応えることを確認する。
    pub check_responds: fn(&Server),
    /// 長く動くコマンドの行。呼ぶたびに、動いているかを区別できる行を返す。
    pub long_command: fn() -> String,
    /// `long_command` で打った行のコマンドが動き出すのを待つ。
    pub wait_running: fn(&Server, &[String]),
}

impl BackendSetup {
    /// 子孫のプロセスの終わりを見る形で起動する。
    pub fn start_watching_children(&self) -> Server {
        Server::start_watching_children(&self.config, self.arguments)
    }
}

pub fn conpty_setup() -> BackendSetup {
    BackendSetup {
        config: String::new(),
        arguments: &CMD_COMMAND,
        wait_ready: |server| {
            server.wait_text(CMD_PROMPT);
        },
        check_responds: check_cmd_responds,
        long_command: || CMD_LONG_COMMAND.to_owned(),
        wait_running: |server, commands| {
            processes::wait_descendants_named(server.pid(), CMD_LONG_PROCESS, commands.len());
        },
    }
}

/// Cygwin のブリッジのタブの手順。環境変数がなければスキップしたと知らせて `None`。
pub fn cygwin_setup(test: &str) -> Option<BackendSetup> {
    Some(BackendSetup {
        config: cygwin_config(test)?,
        arguments: &[],
        wait_ready: wait_sh_ready,
        check_responds: check_sh_responds,
        long_command: || format!("sleep {}", unique_sleep_seconds()),
        wait_running: |_, commands| {
            for command in commands {
                let marker = command.strip_prefix("sleep ").expect("not a sleep command");
                wait_cygwin_sleep(marker, true);
            }
        },
    })
}

/// WSL のブリッジのタブの手順。WSL の中のプロセスは見えないので、動き出すのは待たない。環境変数がなければ
/// スキップしたと知らせて `None`。
pub fn wsl_setup(test: &str) -> Option<BackendSetup> {
    Some(BackendSetup {
        config: wsl_config(test)?,
        arguments: &[],
        wait_ready: wait_sh_ready,
        check_responds: check_sh_responds,
        long_command: || SH_LONG_COMMAND.to_owned(),
        wait_running: |_, _| {},
    })
}

/// 長く動くコマンドを動かしているタブ 2 つと、何も動かしていないタブ 1 つにして、コマンドが動き出すのを待つ。
#[track_caller]
pub fn open_tabs_with_long_commands(setup: &BackendSetup, server: &Server) {
    (setup.wait_ready)(server);
    let first = (setup.long_command)();
    server.send_line(&first);
    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    (setup.wait_ready)(server);
    let second = (setup.long_command)();
    server.send_line(&second);
    server.post("/e2e/tab/new", b"");
    server.wait_until("three tabs", |state| tab_count(state) == 3);
    (setup.wait_ready)(server);
    (setup.wait_running)(server, &[first, second]);
}

/// `cmd.exe` が入力に応えることを確認する。
pub fn check_cmd_responds(server: &Server) {
    server.type_line("echo alive-%ERRORLEVEL%-shell");
    server.wait_text("alive-0-shell");
}

/// `/bin/sh` が入力に応えることを確認する。
pub fn check_sh_responds(server: &Server) {
    server.type_line("echo alive-$((1+1))");
    server.wait_text("alive-2");
}

/// Cygwin のプログラムの親は Windows のプロセスの親子で辿れないので、`/proc` のコマンドラインを見る。
/// 自分の引数に一致しないよう、`.` を `[.]` と書いて探す。
pub fn cygwin_sleep_is_running(marker: &str) -> bool {
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

/// 実行ファイルの中で重ならない、`sleep` の引数にできる数。`/proc` のコマンドラインを部分一致で探すので、ある印が
/// 別の印の前方部分にならないよう、桁数を固定する（プロセスの番号の下 5 桁と、連番の 3 桁）。
pub fn unique_sleep_seconds() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        "300.{:05}{:03}",
        std::process::id() % 100_000,
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[track_caller]
pub fn wait_cygwin_sleep(marker: &str, running: bool) {
    let deadline = Instant::now() + CYGWIN_SLEEP_TIMEOUT;
    while cygwin_sleep_is_running(marker) != running {
        assert!(
            Instant::now() < deadline,
            "sleep {marker} did not become running={running}"
        );
        thread::sleep(CYGWIN_SLEEP_POLL_INTERVAL);
    }
}

/// `IDOK`。編集のダイアログの「貼り付け」。
pub const BUTTON_OK: i32 = 1;
/// `IDCANCEL`。編集のダイアログの「キャンセル」。
pub const BUTTON_CANCEL: i32 = 2;

/// 編集のダイアログのウィンドウのクラス。
pub const TEXT_DIALOG_CLASS: &str = "fantermo-text-dialog";

/// 編集のダイアログが一つ開くのを待ち、そのコントロールの文字を返す。
#[track_caller]
pub fn wait_text_dialog(server: &Server) -> Vec<String> {
    // メニューから開いたときは、閉じかけのメニューのウィンドウ（`#32768`）がしばらく並ぶので、編集のダイアログ
    // だけになるまで待つ。
    let state = server.wait_until("the text dialog", |state| {
        state["dialogs"]
            .as_array()
            .is_some_and(|dialogs| dialogs.len() == 1 && dialogs[0]["class"] == TEXT_DIALOG_CLASS)
    });
    let dialog = &state["dialogs"][0];
    dialog["controls"]
        .as_array()
        .expect("the dialog has no controls")
        .iter()
        .map(|control| control.as_str().unwrap_or_default().to_owned())
        .collect()
}

/// `(col, row)` のセル（0 から数える）の左上から `(dx, dy)` ピクセルの位置を、絵の中の位置にする。
pub fn cell_pixel(state: &Value, col: u32, row: u32, dx: u32, dy: u32) -> (u32, u32) {
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

/// 状態のファイルの `[window]`。ファイルがなかったり読めなければ失敗にする。
#[track_caller]
pub fn saved_window(server: &Server) -> WindowState {
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
pub fn current_window(state: &Value) -> WindowState {
    serde_json::from_value(state["window"].clone())
        .unwrap_or_else(|e| panic!("no window in the state: {e}\n{state:#}"))
}

/// `contents` を `state.toml` に置いて起動し、プロンプトが出るのを待つ。
pub fn start_with_state(contents: &[u8]) -> (Server, Value) {
    let server = Server::start_with_setup("", &CMD_COMMAND, |dir| {
        std::fs::write(dir.join(common::STATE_FILE), contents).expect("failed to write the state");
    });
    let state = server.wait_text(CMD_PROMPT);
    (server, state)
}

/// ウィンドウを閉じて、fantermo が終わるのを待つ。
#[track_caller]
pub fn quit(server: &mut Server) {
    server.post("/e2e/quit", b"");
    assert_eq!(server.wait_exit(), 0);
}

pub fn seeded_state(window: WindowState) -> State {
    State {
        window: Some(window),
    }
}

/// セルで割り切れない大きさのウィンドウ（枠を含むピクセル）。
pub const UNEVEN_WINDOW: WindowState = WindowState {
    left: 10,
    top: 20,
    width: 411,
    height: 291,
    maximized: false,
};

/// 設定 `config` と `state.toml` の [`UNEVEN_WINDOW`] で `command` を起動し、プロンプト `prompt` が出るのを待つ。端末を
/// 描く領域がセルで割り切れないことを確認する。
pub fn start_uneven(config: &str, command: &[&str], prompt: &str) -> (Server, Value) {
    start_uneven_with_setup(config, command, prompt, |_| {})
}

/// [`start_uneven`] と同じく起動する。起動の前に一時フォルダで `prepare` を呼ぶ。
pub fn start_uneven_with_setup(
    config: &str,
    command: &[&str],
    prompt: &str,
    prepare: impl FnOnce(&std::path::Path),
) -> (Server, Value) {
    let saved = state::to_toml(&seeded_state(UNEVEN_WINDOW));
    let server = Server::start_with_setup(config, command, |dir| {
        std::fs::write(dir.join(common::STATE_FILE), &saved).expect("failed to write the state");
        prepare(dir);
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
pub fn terminal_pixels(state: &Value) -> (u64, u64) {
    let padding = number(&state["terminal_x"]);
    (
        number(&state["client_width"]) - number(&state["scroll_bar"]["width"]) - 2 * padding,
        number(&state["client_height"]) - number(&state["background"]["y"]) - 2 * padding,
    )
}

/// 桁数と行数が、いまのクライアント領域の端末を描く領域にセルで入る数で、タブバー・スクロールバー・背景画像の範囲が
/// クライアント領域に合っていることを確認する。どのタブも同じ桁数と行数。
#[track_caller]
pub fn assert_layout_fits_the_client(state: &Value) {
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
    // 背景画像の範囲はタブバーの下の全体で、スクロールバーの列も含む。
    assert_eq!(number(&state["background"]["x"]), 0, "{state:#}");
    assert_eq!(
        number(&state["background"]["width"]),
        client_width,
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
pub fn assert_the_shell_sees_the_grid(server: &Server, state: &Value, tag: &str) {
    server.type_line(&format!(
        "\"{tag}-[$($Host.UI.RawUI.WindowSize.Width)x$($Host.UI.RawUI.WindowSize.Height)]\""
    ));
    server.wait_text(&format!(
        "{tag}-[{}x{}]",
        number(&state["cols"]),
        number(&state["rows"])
    ));
}

/// 背景画像のテストで固定する端末の背景色。
pub const TERMINAL_BACKGROUND: [u8; 3] = [16, 32, 48];
pub const TERMINAL_BACKGROUND_SETTING: &str = "[colors]\nbackground = \"#102030\"\n";
/// 色の許容の差。半透明の重ね合わせの丸めと、ソフトウェアの描画との差の分。
pub const BACKGROUND_COLOR_TOLERANCE: u8 = 6;
/// 画素が期待の色になるのを待つ上限。読み直しが描画に届くまでの分。
pub const PICTURE_TIMEOUT: Duration = Duration::from_secs(30);
/// 画素を読み直す間隔。
pub const PICTURE_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// 背景画像のファイルの名前。設定には設定ファイルのフォルダからの相対パスで書く。
pub const BANDS_IMAGE: &str = "bands.png";
/// [`bands_image`] の一辺と、上下の帯の高さ。
pub const BANDS_SIDE: u32 = 64;
pub const BAND_HEIGHT: u32 = 8;
pub const BAND_TOP: [u8; 3] = [255, 0, 255];
pub const BAND_BOTTOM: [u8; 3] = [0, 255, 255];
pub const MIDDLE_LEFT: [u8; 3] = [255, 0, 0];
pub const MIDDLE_RIGHT: [u8; 3] = [0, 255, 0];

/// 上と下に帯があり、間を左右に分けた正方形の画像。表示方法ごとに、見える部分と位置が変わる。
pub fn bands_image() -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..BANDS_SIDE {
        for x in 0..BANDS_SIDE {
            pixels.extend_from_slice(&bands_color(x, y));
        }
    }
    common::encode_png(BANDS_SIDE, BANDS_SIDE, &pixels)
}

/// [`bands_image`] の、画像の中の位置 `(x, y)` の色。
pub fn bands_color(x: u32, y: u32) -> [u8; 3] {
    if y < BAND_HEIGHT {
        BAND_TOP
    } else if y >= BANDS_SIDE - BAND_HEIGHT {
        BAND_BOTTOM
    } else if x < BANDS_SIDE / 2 {
        MIDDLE_LEFT
    } else {
        MIDDLE_RIGHT
    }
}

/// 単色の画像。
pub fn solid_image(color: [u8; 3]) -> Vec<u8> {
    const SIDE: u32 = 16;
    common::encode_png(SIDE, SIDE, &color.repeat((SIDE * SIDE) as usize))
}

/// 背景画像を敷く範囲（絵の中のピクセル）。
#[derive(Clone, Copy, Debug)]
pub struct BackdropArea {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl BackdropArea {
    pub fn of(state: &Value) -> Self {
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
    pub fn at(self, fx: f64, fy: f64) -> (u32, u32) {
        (
            self.x + (f64::from(self.width) * fx) as u32,
            self.y + (f64::from(self.height) * fy) as u32,
        )
    }
}

pub fn color_is_close(actual: [u8; 3], expected: [u8; 3]) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(&a, e)| a.abs_diff(e) <= BACKGROUND_COLOR_TOLERANCE)
}

/// 不透明度 `image_opacity` の画像の色 `image` が、[`TERMINAL_BACKGROUND`] の上に見える色。
pub fn over_terminal_background(image: [u8; 3], image_opacity: f64) -> [u8; 3] {
    blend(image, TERMINAL_BACKGROUND, image_opacity)
}

/// `top` が不透明度 `opacity` で `bottom` の上に重なって見える色。
pub fn blend(top: [u8; 3], bottom: [u8; 3], opacity: f64) -> [u8; 3] {
    let mut mixed = [0; 3];
    for ((out, &over), &under) in mixed.iter_mut().zip(&top).zip(&bottom) {
        *out = (f64::from(over) * opacity + f64::from(under) * (1.0 - opacity)).round() as u8;
    }
    mixed
}

/// `expected` の位置がすべて期待の色になるまで絵を読み直す。
#[track_caller]
pub fn wait_pixels(server: &Server, expected: &[((u32, u32), [u8; 3])]) {
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
pub fn start_with_background(background: &str, prepare: impl FnOnce(&std::path::Path)) -> Server {
    let config = format!("{TERMINAL_BACKGROUND_SETTING}{background}");
    let server = Server::start_with_setup(&config, &CMD_COMMAND, |dir| {
        std::fs::write(dir.join(BANDS_IMAGE), bands_image()).expect("failed to write the image");
        prepare(dir);
    });
    server.wait_text(CMD_PROMPT);
    server
}

/// スクロールバーの列の右端の、縦の真ん中の画素の位置。つまみは列の真ん中に描くので、右端には掛からない。
pub fn scroll_bar_edge(state: &Value) -> (u32, u32) {
    let bar = &state["scroll_bar"];
    let right = number(&bar["x"]) + number(&bar["width"]) - 1;
    assert_eq!(right + 1, number(&state["client_width"]), "{state:#}");
    (
        u32::try_from(right).expect("the position fits"),
        u32::try_from(scroll_bar_middle(state)).expect("the position fits"),
    )
}

/// クライアント領域の中の位置。ウィンドウの外を指す負の値も持てる。
pub type Point = (i64, i64);

/// メニューのウィンドウのクラス。
pub const MENU_CLASS: &str = "#32768";

/// 文字のない桁。テストの行はここまで届かない。
pub const EMPTY_COL: u32 = 40;
/// 文字のない行が続く位置を選ぶときの、行からの距離。
pub const EMPTY_ROWS_BELOW: u32 = 3;

/// スクロールバーをドラッグするピクセル数。
pub const DRAG_PIXELS: i64 = 100;

/// タブの左端からタブの本体の位置までのピクセル数。閉じるボタンより左。
pub const TAB_BODY_OFFSET: i64 = 8;

/// `(col, row)` のセルの真ん中。
pub fn cell_center(state: &Value, col: u32, row: u32) -> Point {
    let half = |size: &str| u32::try_from(number(&state[size]) / 2).expect("the size fits");
    let (x, y) = cell_pixel(state, col, row, half("cell_width"), half("cell_height"));
    (i64::from(x), i64::from(y))
}

/// `(col, row)` のセルの左端の、高さの真ん中。セルの半分を越えないので、ここから選び始めるとこのセルを含み、ここまで
/// 選ぶとこのセルを含まない。
pub fn cell_left(state: &Value, col: u32, row: u32) -> Point {
    let (x, y) = cell_center(state, col, row);
    (
        x - i64::try_from(number(&state["cell_width"]) / 2).expect("the size fits") + 1,
        y,
    )
}

/// `(col, row)` のセルの右端の、高さの真ん中。セルの半分を越えているので、ここまで選ぶとこのセルを含む。
pub fn cell_right(state: &Value, col: u32, row: u32) -> Point {
    let (x, y) = cell_left(state, col, row);
    (
        x + i64::try_from(number(&state["cell_width"])).expect("the size fits") - 2,
        y,
    )
}

/// 表示している範囲の中で `needle` が始まるセル。全体の文字の行と、表示している範囲の行は `scroll` の `offset` だけ
/// ずれる。`needle` が折り返さない長さの行にあるときに使う。
#[track_caller]
pub fn find_cell(state: &Value, needle: &str) -> (u32, u32) {
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

pub fn selection(state: &Value) -> Option<&str> {
    state["selection"].as_str()
}

pub fn step(action: &str, (x, y): Point, held: &str) -> String {
    format!("{action} {x} {y} {held}\n")
}

/// 左ボタンで `at` を一度クリックする。
pub fn left_click(at: Point) -> String {
    format!(
        "{}{}{}",
        step("move", at, ""),
        step("left-down", at, ""),
        step("left-up", at, "")
    )
}

/// `from` で左ボタンを押して `to` まで動かす。離さない。`held` は押している修飾キー。
pub fn left_press_and_move(from: Point, to: Point, held: &str) -> String {
    format!(
        "{}{}{}",
        step("move", from, held),
        step("left-down", from, held),
        step("move", to, &format!("left {held}"))
    )
}

/// `from` から `to` まで左ボタンでドラッグして離す。`held` は押している修飾キー。
pub fn left_drag(from: Point, to: Point, held: &str) -> String {
    format!(
        "{}{}",
        left_press_and_move(from, to, held),
        step("left-up", to, held)
    )
}

/// `at` で `button`（`left` など）を押して離す。`held` は押している修飾キー。
pub fn button_click(button: &str, at: Point, held: &str) -> String {
    format!(
        "{}{}{}",
        step("move", at, held),
        step(&format!("{button}-down"), at, held),
        step(&format!("{button}-up"), at, held)
    )
}

/// 打った行には出ない 2 行を出し、その全体の状態を返す。行は `first-0-end`、`second-0-end` の形にしたものを待つ。
pub fn echo_lines(server: &Server, first: &str, second: &str) -> Value {
    server.type_line(&format!(
        "echo {first}-%ERRORLEVEL%-end&echo {second}-%ERRORLEVEL%-end"
    ));
    server.wait_text(&format!("{second}-0-end"))
}

pub fn wait_menu(server: &Server) {
    server.wait_until("a menu", |state| {
        state["dialogs"]
            .as_array()
            .is_some_and(|dialogs| dialogs.iter().any(|dialog| dialog["class"] == MENU_CLASS))
    });
}

/// 開いているメニューを閉じて、閉じたのを確認する。
pub fn close_menu(server: &Server) {
    server.post("/e2e/menu/close", b"");
    server.wait_dialogs(0);
}

/// 画面に収まらない行を出す。全体の文字に行が出たあとの状態を返す。
pub fn fill_scrollback(server: &Server) -> Value {
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

pub fn scroll_offset(state: &Value) -> u64 {
    number(&state["scroll"]["offset"])
}

/// いちばん新しい行を表示しているときの `offset`。
pub fn newest_offset(state: &Value) -> u64 {
    number(&state["scroll"]["total"]) - number(&state["scroll"]["len"])
}

pub fn thumb_top(state: &Value) -> u64 {
    number(&state["scroll_bar"]["thumb"]["top"])
}

pub fn thumb_length(state: &Value) -> u64 {
    number(&state["scroll_bar"]["thumb"]["length"])
}

/// スクロールバーの横の真ん中の、高さ `y` の位置。
pub fn on_scroll_bar(state: &Value, y: u64) -> Point {
    let bar = &state["scroll_bar"];
    (
        i64::try_from(number(&bar["x"]) + number(&bar["width"]) / 2).expect("the position fits"),
        i64::try_from(y).expect("the position fits"),
    )
}

/// スクロールバーの縦の真ん中の高さ。
pub fn scroll_bar_middle(state: &Value) -> u64 {
    number(&state["scroll_bar"]["y"]) + number(&state["scroll_bar"]["height"]) / 2
}

/// つまみの真ん中。
pub fn on_thumb(state: &Value) -> Point {
    on_scroll_bar(state, thumb_top(state) + thumb_length(state) / 2)
}

pub fn area_center(area: &Value) -> Point {
    (
        i64::try_from(number(&area["x"]) + number(&area["width"]) / 2).expect("the position fits"),
        i64::try_from(number(&area["y"]) + number(&area["height"]) / 2).expect("the position fits"),
    )
}

/// `state` のタブバーの `index` 番目のタブの本体（閉じるボタンの外）の位置。
pub fn on_tab(state: &Value, index: usize) -> Point {
    let area = &state["tab_bar"]["tabs"][index]["area"];
    (
        i64::try_from(number(&area["x"])).expect("the position fits") + TAB_BODY_OFFSET,
        area_center(area).1,
    )
}

pub fn on_close_button(state: &Value, index: usize) -> Point {
    area_center(&state["tab_bar"]["tabs"][index]["close"])
}

/// タブが `count` 個になり、表示しているタブのシェルが動くまで待つ。
#[track_caller]
pub fn wait_tabs(server: &Server, count: usize) -> Value {
    server.wait_until(&format!("{count} tabs"), |state| tab_count(state) == count);
    server.wait_text(CMD_PROMPT)
}

/// 矢印のキーで項目へ移るのを待つ上限。
pub const MENU_TIMEOUT: Duration = Duration::from_secs(30);
/// 項目を読み直す間隔。
pub const MENU_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// 設定ダイアログのウィンドウのクラス。
pub const SETTINGS_DIALOG_CLASS: &str = "fantermo-settings-dialog";
/// `IDCANCEL`。設定ダイアログの「キャンセル」と、「fantermo について」の「閉じる」。
pub const DIALOG_CLOSE_BUTTON: i32 = 2;

/// 文字のあるセルで、背景と違う色の画素を探すときの差。
pub const INK_DIFFERENCE: u8 = 40;

/// 開いているメニューの項目を読む。
#[track_caller]
pub fn read_menu(server: &Server) -> Vec<Value> {
    let (status, body) = server.request("GET", "/e2e/menu", b"");
    let text = String::from_utf8_lossy(&body);
    assert_eq!(status, 200, "{text}\nstate: {:#}", server.state());
    let menu: Value = serde_json::from_slice(&body).expect("the menu is not JSON");
    menu["items"].as_array().expect("no menu items").clone()
}

/// 項目のアクセラレータの英数字（大文字）。なければ `None`。`&&` は `&` そのもの。
pub fn accelerator(item: &Value) -> Option<char> {
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

/// `steps` を送ってメニューが開くのを待ち、項目を返す。
#[track_caller]
pub fn open_menu(server: &Server, steps: &str) -> Vec<Value> {
    server.mouse(steps);
    wait_menu(server);
    read_menu(server)
}

/// 矢印のキーで `position` の項目へ移り、移ったのを待つ。移れなければ、そのときの項目と状態を出して失敗にする。
#[track_caller]
pub fn highlight_position(server: &Server, position: usize) {
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
pub fn select_position(server: &Server, position: usize) {
    highlight_position(server, position);
    server.post(&format!("/e2e/menu/select/{position}"), b"");
}

/// `accelerator_of` のアクセラレータの項目の位置。
#[track_caller]
pub fn menu_position(items: &[Value], accelerator_of: char) -> usize {
    items
        .iter()
        .position(|item| accelerator(item) == Some(accelerator_of))
        .unwrap_or_else(|| panic!("no item {accelerator_of}: {items:#?}"))
}

/// `accelerator_of` のアクセラレータの項目を選ぶ。
#[track_caller]
pub fn select_item(server: &Server, items: &[Value], accelerator_of: char) {
    select_position(server, menu_position(items, accelerator_of));
}

pub fn on_terminal(state: &Value) -> Point {
    cell_center(state, EMPTY_COL, EMPTY_ROWS_BELOW)
}

/// ≡ を押すマウスの操作。
pub fn click_main_menu(state: &Value) -> String {
    left_click(area_center(&state["tab_bar"]["menu"]))
}

/// 端末の上で右クリックするマウスの操作。
pub fn right_click_terminal(state: &Value) -> String {
    button_click("right", on_terminal(state), "")
}

/// 開いているダイアログが `class` の一つだけになるのを待ち、そのダイアログを返す。
#[track_caller]
pub fn wait_dialog_of_class(server: &Server, class: &str) -> Value {
    let state = server.wait_until(&format!("a dialog of {class}"), |state| {
        state["dialogs"]
            .as_array()
            .is_some_and(|dialogs| dialogs.len() == 1 && dialogs[0]["class"] == class)
    });
    state["dialogs"][0].clone()
}

/// シェルが入力を受け取って動き、画面に文字を描くことを確認する。`tag` は打つごとに変える。
#[track_caller]
pub fn check_terminal_still_works(server: &Server, tag: &str) {
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
pub fn assert_cell_is_drawn(server: &Server, state: &Value, col: u32, row: u32) {
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

pub const ID_OK: i32 = 1;
pub const ID_CANCEL: i32 = 2;

/// ページの位置。
pub const PAGE_APPEARANCE: usize = 0;

/// メッセージボックスと色を選ぶダイアログのボタン。
pub const ID_YES: i32 = 6;
pub const ID_NO: i32 = 7;

/// メッセージボックスと OS のダイアログのウィンドウのクラス。
pub const SYSTEM_DIALOG_CLASS: &str = "#32770";

/// ランナーの画面の大きさ（96 DPI）。
pub const RUNNER_SCREEN: (u64, u64) = (1024, 768);
pub const BASE_DPI: u64 = 96;

/// `cmd.exe` を起動するプロファイルが一つの設定。
pub const ONE_PROFILE: &str =
    "[[profiles]]\nname = \"one\"\ncommand = \"cmd.exe /k prompt e2e$G\"\n";

/// 引数なしで、設定 `config` のプロファイルの `cmd.exe` を起動する。
pub fn start_settings_test(config: &str) -> Server {
    let server = Server::start(config, &[]);
    server.wait_text(CMD_PROMPT);
    server
}

/// ≡ の「設定…」で設定ダイアログを開き、コントロールを返す。
#[track_caller]
pub fn open_settings(server: &Server) -> Value {
    let state = server.state();
    let items = open_menu(server, &click_main_menu(&state));
    select_item(server, &items, 'S');
    wait_dialog_of_class(server, SETTINGS_DIALOG_CLASS);
    read_controls(server)
}

/// いちばん上のダイアログのコントロール。
#[track_caller]
pub fn read_controls(server: &Server) -> Value {
    let (status, body) = server.request("GET", "/e2e/dialog/controls", b"");
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).expect("the controls are not JSON")
}

/// `dialog` の ID が `id` のコントロール。
#[track_caller]
pub fn control(dialog: &Value, id: i32) -> &Value {
    dialog["controls"]
        .as_array()
        .and_then(|controls| controls.iter().find(|control| control["id"] == id))
        .unwrap_or_else(|| panic!("no control {id}: {dialog:#}"))
}

/// いまの ID が `id` のコントロールを押せるか。
#[track_caller]
pub fn control_enabled(server: &Server, id: i32) -> bool {
    control(&read_controls(server), id)["enabled"] == true
}

pub fn click(server: &Server, id: i32) {
    server.post(&format!("/e2e/dialog/click/{id}"), b"");
}

/// いちばん上のダイアログのクラスが `class` になるのを待つ。
#[track_caller]
pub fn wait_top_dialog(server: &Server, class: &str) -> Value {
    let state = server.wait_until(&format!("the top dialog of {class}"), |state| {
        state["dialogs"][0]["class"] == class
    });
    state["dialogs"][0].clone()
}

/// 「キャンセル」を押して、閉じるのを待つ。
#[track_caller]
pub fn press_cancel(server: &Server) {
    click(server, ID_CANCEL);
    server.wait_dialogs(0);
}

pub fn config_path(server: &Server) -> PathBuf {
    server.dir().join("config.toml")
}

/// いま開いているメッセージボックスの文字（ボタンと本文）。メッセージボックスが開くのを待つ。
#[track_caller]
pub fn wait_message_box(server: &Server) -> String {
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
pub fn close_message_box(server: &Server, remaining: usize) {
    server.post("/e2e/dialog/close", b"");
    server.wait_dialogs(remaining);
}

pub fn number_of(dialog: &Value, name: &str) -> u64 {
    number(&dialog[name])
}

/// 見えているコントロールがダイアログのクライアント領域に収まり、ダイアログがランナーの画面（DPI の分だけ広げた大きさ）に
/// 収まることを確認する。
#[track_caller]
pub fn assert_controls_fit(dialog: &Value, page: usize) {
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

/// いちばん上のメッセージボックスの文に `needle` が出るのを待ち、その文を返す。
#[track_caller]
pub fn wait_message_box_with(server: &Server, needle: &str) -> String {
    let state = server.wait_until(&format!("a message box with {needle:?}"), |state| {
        state["dialogs"][0]["class"] == SYSTEM_DIALOG_CLASS
            && dialog_text(&state["dialogs"][0]).contains(needle)
    });
    dialog_text(&state["dialogs"][0])
}

pub fn dialog_text(dialog: &Value) -> String {
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

pub const DPI_CHANGED: &str = "dpichanged";

pub const DPI_100_PERCENT: u64 = BASE_DPI;
pub const DPI_150_PERCENT: u64 = 144;
pub const DPI_200_PERCENT: u64 = 192;

/// `/e2e/message/dpichanged/<DPI>` を送り、そのあとの状態を返す。処理はこの場で終わるので、待たずに読める。
#[track_caller]
pub fn change_dpi(server: &Server, dpi: u64) -> Value {
    let wparam = usize::try_from(dpi).expect("the DPI fits");
    assert_eq!(server.send_message(DPI_CHANGED, wparam), 0);
    let state = server.state();
    assert_eq!(state["dpi"], dpi, "{state:#}");
    assert_fits_the_runner_screen(&state);
    state
}

/// ランナーの画面に収まらないと OS がウィンドウを切り詰めて桁数が減るので、手元で先に気づけるようにする。
#[track_caller]
pub fn assert_fits_the_runner_screen(state: &Value) {
    let window = current_window(state);
    assert!(
        u64::try_from(window.width).expect("the width fits") <= RUNNER_SCREEN.0
            && u64::try_from(window.height).expect("the height fits") <= RUNNER_SCREEN.1,
        "the window does not fit the screen of the runner: {state:#}"
    );
}

/// プロファイルのテストの設定。プロンプトでどのプロファイルかが分かる。
pub const TWO_PROMPT_PROFILES: &str = r#"default_profile = "two"

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
