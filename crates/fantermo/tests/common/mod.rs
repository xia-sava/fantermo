//! e2e テストの土台。fantermo を画面に出ないデスクトップで起動し、制御の口で操作する。
//!
//! [`Server`] は起動ごとにユニークな一時フォルダを作り、設定・ウィンドウの状態・ログ・ポートの番号をそこに置く。
//! ポートは OS の割り当てなので、テストを並列に実行しても衝突しない。破棄すると子プロセスを止めて一時フォルダを
//! 消す。テストが失敗したときは、破棄のときに fantermo のログと画面の文字を出す。
//!
//! テストの実行は `./tools/dev.sh test --features e2e`（出力先は `target/e2e`）。run-hidden は別の crate なので、
//! `-p fantermo` で絞るときは先に `./tools/dev.sh build --features e2e` で作っておく。

// 使う関数はテストの実行ファイルごとに違う。
#![allow(dead_code)]

pub mod processes;

use std::{
    collections::HashSet,
    fs::File,
    io::{Cursor, Read, Write},
    net::{Ipv4Addr, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Condvar, Mutex, MutexGuard,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

/// テストの実行ファイルごとの、起動の連番。
static SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// 同時に起動する fantermo の数を数える。デスクトップのヒープと描画の負荷を抑える。
static RUNNING: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// 同時に起動する fantermo の上限。
const MAX_RUNNING: usize = 4;

/// OS のクリップボードはテスト全体で一つなので、使うテストを直列にする。
static OS_CLIPBOARD: Mutex<()> = Mutex::new(());

/// fantermo が OS のクリップボードの代わりに使うファイルを指定する環境変数。
const CLIPBOARD_FILE_ENV: &str = "FANTERMO_E2E_CLIPBOARD_FILE";
/// OS のクリップボードを実際に変えるテストを実行する環境変数。
const OS_CLIPBOARD_ENV: &str = "FANTERMO_E2E_OS_CLIPBOARD";

/// 隣へ写したブリッジ。テストの実行ファイルの中で一度だけ写す。
static COPIED_BRIDGES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

/// run-hidden が止めるまでの秒数。テストの実行ファイルが落ちて破棄が走らなくても、これで終わる。
const LIFETIME_SECONDS: &str = "300";
/// fantermo が終わったあと、run-hidden が子孫のプロセスの終わりを待つ秒数。タブを閉じるときに子プロセスを
/// 終わらせるまで待つ 5 秒より長くする。
const WAIT_CHILDREN_SECONDS: &str = "20";
/// 子孫のプロセスが残らなかったときに run-hidden が出す行。
const NO_CHILDREN_REMAINED: &str = "run-hidden: no processes remained";
/// 起動してポートの番号が書かれるまで待つ上限。
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// 条件が成り立つまで待つ上限。並列で起動するとシェルの起動も遅くなるので、長めに取る。
const WAIT_TIMEOUT: Duration = Duration::from_secs(60);
/// 条件を確認する間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// 制御の口の接続を読み書きする上限。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
/// 一時フォルダを消すのを試す回数。止めたプロセスがつかんでいたファイルは、少し遅れて消せるようになる。
const REMOVE_ATTEMPTS: u32 = 50;
/// 一時フォルダを消すのを試す間隔。
const REMOVE_INTERVAL: Duration = Duration::from_millis(100);

const STATUS_OK: u16 = 200;

/// 隔離して起動した fantermo。
pub struct Server {
    child: Child,
    port: u16,
    dir: PathBuf,
}

impl Server {
    /// 設定 `config_toml` で fantermo を起動する。`command` は fantermo の引数（端末で起動するコマンドライン）。
    pub fn start(config_toml: &str, command: &[&str]) -> Self {
        Self::start_with(config_toml, command, &[])
    }

    /// [`Server::start`] に加えて、fantermo が終わったあとに子孫のプロセスが残ったかを
    /// [`Server::assert_no_children_remained`] で確認できるようにする。
    pub fn start_watching_children(config_toml: &str, command: &[&str]) -> Self {
        Self::start_with(
            config_toml,
            command,
            &["--wait-children", WAIT_CHILDREN_SECONDS],
        )
    }

    /// [`Server::start`] と同じだが、一時フォルダのファイルではなく OS のクリップボードを使う。
    /// [`OsClipboard`] を持っているあいだに使う。
    pub fn start_with_os_clipboard(
        _clipboard: &OsClipboard,
        config: &str,
        command: &[&str],
    ) -> Self {
        Self::launch(config, command, &[], true)
    }

    fn start_with(config_toml: &str, command: &[&str], run_hidden_args: &[&str]) -> Self {
        Self::launch(config_toml, command, run_hidden_args, false)
    }

    fn launch(
        config_toml: &str,
        command: &[&str],
        run_hidden_args: &[&str],
        os_clipboard: bool,
    ) -> Self {
        let exe = fantermo_exe();
        let run_hidden = sibling_of_fantermo("run-hidden.exe");
        assert!(
            run_hidden.is_file(),
            "{} がない。`./tools/dev.sh test --features e2e` か `./tools/dev.sh build --features e2e` で作る",
            run_hidden.display()
        );
        acquire_slot();
        let dir = std::env::temp_dir().join(format!(
            "fantermo-e2e-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("failed to create the temporary folder");
        let config = dir.join("config.toml");
        std::fs::write(&config, config_toml).expect("failed to write the settings");
        let log = File::create(log_path(&dir)).expect("failed to create the log");
        let log_for_stderr = log.try_clone().expect("failed to duplicate the log");
        let mut launcher = Command::new(&run_hidden);
        if !os_clipboard {
            launcher.env(CLIPBOARD_FILE_ENV, dir.join("clipboard.txt"));
        }
        let child = launcher
            .args(["--seconds", LIFETIME_SECONDS])
            .args(run_hidden_args)
            .arg("--")
            .arg(&exe)
            .args(command)
            .current_dir(&dir)
            .env("FANTERMO_E2E_DIR", &dir)
            .env("FANTERMO_CONFIG", &config)
            .env("FANTERMO_STATE", dir.join("state.toml"))
            .env("FANTERMO_DPI", "96")
            .env("RUST_LOG", "info,fantermo=debug")
            .env("NO_COLOR", "1")
            // git bash から実行したときの値を、端末の中のシェルへ渡さない。
            .env_remove("HOME")
            .env_remove("SHELL")
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log_for_stderr)
            .spawn()
            .unwrap_or_else(|e| panic!("failed to start {}: {e}", run_hidden.display()));
        let mut server = Self {
            child,
            port: 0,
            dir,
        };
        server.port = server.wait_for_port();
        server
    }

    /// 既定の設定で、`cmd.exe` をプロンプト `e2e>` で起動する。
    pub fn start_cmd() -> Self {
        Self::start("", &["cmd.exe", "/k", "prompt", "e2e$G"])
    }

    /// 既定の設定で、PowerShell を起動する。
    pub fn start_powershell() -> Self {
        Self::start("", &["powershell.exe", "-NoLogo", "-NoProfile"])
    }

    fn wait_for_port(&mut self) -> u16 {
        let path = self.dir.join("port");
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if let Ok(text) = std::fs::read_to_string(&path) {
                return text.trim().parse().expect("the port file is not a number");
            }
            if let Some(status) = self
                .child
                .try_wait()
                .expect("failed to wait for run-hidden")
            {
                panic!(
                    "fantermo ended before opening the control: {status}\n{}",
                    self.log()
                );
            }
            assert!(
                Instant::now() < deadline,
                "fantermo did not open the control\n{}",
                self.log()
            );
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// 要求を送り、状態コードと本文を返す。
    pub fn request(&self, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port))
            .unwrap_or_else(|e| panic!("cannot connect for {path}: {e}\n{}", self.log()));
        stream
            .set_read_timeout(Some(REQUEST_TIMEOUT))
            .expect("set_read_timeout");
        stream
            .set_write_timeout(Some(REQUEST_TIMEOUT))
            .expect("set_write_timeout");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream
            .write_all(head.as_bytes())
            .expect("failed to send the request");
        stream.write_all(body).expect("failed to send the body");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .unwrap_or_else(|e| panic!("cannot read the answer to {path}: {e}\n{}", self.log()));
        split_response(&response)
            .unwrap_or_else(|| panic!("malformed answer to {path}: {response:?}\n{}", self.log()))
    }

    /// `POST` を送る。200 でなければ失敗にする。
    pub fn post(&self, path: &str, body: &[u8]) -> Vec<u8> {
        let (status, answer) = self.request("POST", path, body);
        assert_eq!(
            status,
            STATUS_OK,
            "{path}: {}",
            String::from_utf8_lossy(&answer)
        );
        answer
    }

    /// 文字を打つ。
    pub fn type_text(&self, text: &str) {
        self.post("/e2e/text", text.as_bytes());
    }

    /// キーを押す（`enter`、`ctrl+shift+t` など）。
    pub fn press(&self, combo: &str) {
        self.post(&format!("/e2e/key/{combo}"), b"");
    }

    /// 一行打って Enter を押す。
    pub fn type_line(&self, line: &str) {
        self.type_text(line);
        self.press("enter");
    }

    /// 一行を Enter までまとめてバックエンドへ送る。[`Server::type_line`] は一文字ずつ送るので、Cygwin の pty では
    /// 一文字に 30 ms ほどかかる。長い行はこちらで送る。
    pub fn send_line(&self, line: &str) {
        self.post("/e2e/input", format!("{line}\r").as_bytes());
    }

    /// 端末が `cols` × `rows` になるようにウィンドウの大きさを変える。
    pub fn resize(&self, cols: u16, rows: u16) {
        self.post(&format!("/e2e/window/size/{cols}/{rows}"), b"");
    }

    pub fn state(&self) -> Value {
        let (status, body) = self.request("GET", "/e2e/state", b"");
        assert_eq!(status, STATUS_OK, "{}", String::from_utf8_lossy(&body));
        serde_json::from_slice(&body).expect("the state is not JSON")
    }

    /// 一時フォルダ。設定ファイル（`config.toml`）もここにある。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// fantermo のプロセスの番号。
    pub fn pid(&self) -> u32 {
        let pid = self.state()["pid"].as_u64().expect("the state has no pid");
        u32::try_from(pid).expect("the pid does not fit")
    }

    /// `body` を、クリップボードから読んだものとして貼り付ける。
    pub fn paste(&self, body: &str) {
        self.post("/e2e/paste", body.as_bytes());
    }

    /// 開いているダイアログが `count` 個になるまで待つ。
    #[track_caller]
    pub fn wait_dialogs(&self, count: usize) -> Value {
        self.wait_until(&format!("{count} dialogs"), |state| {
            state["dialogs"].as_array().map_or(0, Vec::len) == count
        })
    }

    /// 開いているダイアログのボタンを押す。
    pub fn press_dialog_button(&self, id: i32) {
        self.post(&format!("/e2e/dialog/button/{id}"), b"");
    }

    /// OS のクリップボードを `text` で置き換える。ほかのプログラムが開いている間は失敗するので、やり直す。
    #[track_caller]
    pub fn set_clipboard(&self, text: &str) {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let (status, answer) = self.request("POST", "/e2e/clipboard", text.as_bytes());
            if status == STATUS_OK {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "cannot write the clipboard: {}",
                String::from_utf8_lossy(&answer)
            );
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// OS のクリップボードの文字列が `expected` になるまで待つ。
    #[track_caller]
    pub fn wait_clipboard(&self, expected: &str) {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let (status, answer) = self.request("GET", "/e2e/clipboard", b"");
            let text = String::from_utf8_lossy(&answer).into_owned();
            if status == STATUS_OK {
                let value: Value = serde_json::from_str(&text).expect("the answer is not JSON");
                if value["text"] == expected {
                    return;
                }
            }
            assert!(
                Instant::now() < deadline,
                "the clipboard did not become {expected:?}: {text}"
            );
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// `wait_exit` のあとで、fantermo の子孫のプロセスが自分で終わっていたことを確認する。
    /// [`Server::start_watching_children`] で起動したときに使う。
    #[track_caller]
    pub fn assert_no_children_remained(&self) {
        let log = self.log();
        assert!(
            log.contains(NO_CHILDREN_REMAINED),
            "child processes remained after fantermo ended\n{log}"
        );
    }

    /// いまの画面の絵を読む。
    pub fn picture(&self) -> Picture {
        Picture::decode(&self.frame())
    }

    /// いまの画面の PNG。
    pub fn frame(&self) -> Vec<u8> {
        let (status, body) = self.request("GET", "/e2e/frame", b"");
        assert_eq!(
            status,
            STATUS_OK,
            "{}\n{}",
            String::from_utf8_lossy(&body),
            self.log()
        );
        body
    }

    /// `condition` が成り立つまで状態を読み続け、成り立った状態を返す。時間切れは失敗にする。
    #[track_caller]
    pub fn wait_until(&self, description: &str, condition: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let state = self.state();
            if condition(&state) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {description}; state: {state:#}"
            );
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// 表示している端末の文字に `needle` が出るまで待つ。
    #[track_caller]
    pub fn wait_text(&self, needle: &str) -> Value {
        self.wait_until(&format!("the text {needle:?}"), |state| {
            state["text"]
                .as_str()
                .is_some_and(|text| text.contains(needle))
        })
    }

    /// fantermo が自分で終わるのを待ち、その終了コードを返す。
    #[track_caller]
    pub fn wait_exit(&mut self) -> i32 {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .expect("failed to wait for run-hidden")
            {
                return status.code().expect("run-hidden has no exit code");
            }
            assert!(Instant::now() < deadline, "fantermo did not exit");
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// fantermo と run-hidden のログ。
    pub fn log(&self) -> String {
        match std::fs::read(log_path(&self.dir)) {
            Ok(bytes) => format!("--- log ---\n{}", String::from_utf8_lossy(&bytes)),
            Err(e) => format!("--- no log: {e} ---"),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if thread::panicking() {
            eprintln!("{}", self.log());
        }
        // run-hidden を止めると、Job が閉じて fantermo と子孫のプロセスも終わる。
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(e) = self.child.kill() {
                    eprintln!("failed to stop run-hidden: {e}");
                }
                if let Err(e) = self.child.wait() {
                    eprintln!("failed to wait for run-hidden: {e}");
                }
            }
            Err(e) => eprintln!("failed to check run-hidden: {e}"),
        }
        remove_folder(&self.dir);
        release_slot();
    }
}

/// OS のクリップボードを使うテストのあいだ持つもの。テストを直列にし、終わると文字列を元に戻す。文字列のほかの
/// 形式は戻らないので、環境変数 [`OS_CLIPBOARD_ENV`] があるときだけ使う。
pub struct OsClipboard {
    _serial: MutexGuard<'static, ()>,
}

impl OsClipboard {
    /// 環境変数がなければ、スキップしたと知らせて `None`。
    pub fn take(test: &str) -> Option<Self> {
        if std::env::var_os(OS_CLIPBOARD_ENV).is_none() {
            report_skip(test, OS_CLIPBOARD_ENV);
            return None;
        }
        Some(Self {
            _serial: OS_CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner()),
        })
    }
}

/// 破棄すると OS のクリップボードの文字列を、作ったときのものに戻す。
pub struct RestoreClipboard<'a> {
    server: &'a Server,
    saved: String,
}

impl<'a> RestoreClipboard<'a> {
    pub fn new(server: &'a Server) -> Self {
        let (status, answer) = server.request("GET", "/e2e/clipboard", b"");
        assert_eq!(status, STATUS_OK, "{}", String::from_utf8_lossy(&answer));
        let value: Value = serde_json::from_slice(&answer).expect("the answer is not JSON");
        Self {
            server,
            saved: value["text"].as_str().unwrap_or_default().to_owned(),
        }
    }
}

impl Drop for RestoreClipboard<'_> {
    fn drop(&mut self) {
        let (status, answer) = self
            .server
            .request("POST", "/e2e/clipboard", self.saved.as_bytes());
        if status != STATUS_OK {
            eprintln!(
                "failed to restore the clipboard: {}",
                String::from_utf8_lossy(&answer)
            );
        }
    }
}

/// `/e2e/frame` の絵。
pub struct Picture {
    width: u32,
    height: u32,
    channels: usize,
    bytes: Vec<u8>,
}

impl Picture {
    fn decode(png_bytes: &[u8]) -> Self {
        let mut reader = png::Decoder::new(Cursor::new(png_bytes))
            .read_info()
            .expect("the frame is not a PNG");
        let mut bytes = vec![0; reader.output_buffer_size().expect("the frame is too large")];
        let info = reader
            .next_frame(&mut bytes)
            .expect("cannot decode the frame");
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        bytes.truncate(info.buffer_size());
        Self {
            width: info.width,
            height: info.height,
            channels: info.color_type.samples(),
            bytes,
        }
    }

    /// `(x, y)` の画素の赤・緑・青。
    #[track_caller]
    pub fn rgb(&self, x: u32, y: u32) -> [u8; 3] {
        assert!(
            x < self.width && y < self.height,
            "({x}, {y}) is outside of {}x{}",
            self.width,
            self.height
        );
        let at = (y as usize * self.width as usize + x as usize) * self.channels;
        [self.bytes[at], self.bytes[at + 1], self.bytes[at + 2]]
    }
}

/// RFC 4648 の base64。
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let [a, b, c] = [
            group[0],
            group.get(1).copied().unwrap_or(0),
            group.get(2).copied().unwrap_or(0),
        ];
        let sextets = [
            a >> 2,
            (a << 4 | b >> 4) & 63,
            (b << 2 | c >> 6) & 63,
            c & 63,
        ];
        for (index, &sextet) in sextets.iter().enumerate() {
            out.push(if index <= group.len() {
                char::from(ALPHABET[usize::from(sextet)])
            } else {
                '='
            });
        }
    }
    out
}

/// 起動している数が上限を超えないよう、空くまで待つ。
fn acquire_slot() {
    let (count, freed) = &RUNNING;
    let mut count = count.lock().unwrap_or_else(|e| e.into_inner());
    while *count >= MAX_RUNNING {
        count = freed.wait(count).unwrap_or_else(|e| e.into_inner());
    }
    *count += 1;
}

fn release_slot() {
    let (count, freed) = &RUNNING;
    *count.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
    freed.notify_one();
}

fn remove_folder(dir: &Path) {
    let mut last_error = None;
    for _ in 0..REMOVE_ATTEMPTS {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) => last_error = Some(e),
        }
        thread::sleep(REMOVE_INTERVAL);
    }
    if let Some(e) = last_error {
        eprintln!("failed to remove {}: {e}", dir.display());
    }
}

fn log_path(dir: &Path) -> PathBuf {
    dir.join("run.log")
}

fn fantermo_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fantermo"))
}

fn sibling_of_fantermo(file_name: &str) -> PathBuf {
    fantermo_exe().with_file_name(file_name)
}

/// HTTP の答えを、状態コードと本文に分ける。
fn split_response(response: &[u8]) -> Option<(u16, Vec<u8>)> {
    const HEADER_END: &[u8] = b"\r\n\r\n";
    let end = response
        .windows(HEADER_END.len())
        .position(|window| window == HEADER_END)?;
    let head = std::str::from_utf8(&response[..end]).ok()?;
    let status = head.split_ascii_whitespace().nth(1)?.parse().ok()?;
    Some((status, response[end + HEADER_END.len()..].to_vec()))
}

/// ブリッジを fantermo が探す場所に置く。fantermo は実行ファイルの隣と `<出力先>/bridge/` を探す。
/// `target/e2e` の fantermo からは `target/bridge/` は見えないので、ビルドしたものを隣へ写す。
pub fn install_bridge(triple: &str, file_name: &'static str) {
    let exe = fantermo_exe();
    let output_dir = exe
        .parent()
        .and_then(Path::parent)
        .expect("no output folder");
    if output_dir
        .join("bridge")
        .join(triple)
        .join("debug")
        .join(file_name)
        .is_file()
    {
        return;
    }
    let mut copied = COPIED_BRIDGES.lock().unwrap_or_else(|e| e.into_inner());
    if !copied.get_or_insert_default().insert(file_name) {
        return;
    }
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = repository
        .join("target/bridge")
        .join(triple)
        .join("debug")
        .join(file_name);
    assert!(
        source.is_file(),
        "{} がない。`./tools/bridge.sh build <cygwin|linux>` で作る",
        source.display()
    );
    let destination = sibling_of_fantermo(file_name);
    std::fs::copy(&source, &destination)
        .unwrap_or_else(|e| panic!("cannot copy to {}: {e}", destination.display()));
}

/// 環境変数の指定がないのでスキップしたと、テストの出力を横取りされない形で知らせる。
pub fn report_skip(test: &str, variable: &str) {
    let mut stderr = std::io::stderr();
    if let Err(e) = writeln!(stderr, "SKIPPED: {test}（環境変数 {variable} がない）") {
        panic!("cannot report the skip: {e}");
    }
}
