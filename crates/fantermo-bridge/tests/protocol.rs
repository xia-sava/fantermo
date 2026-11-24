//! ブリッジを標準入出力のパイプ付きで起動し、プロトコルの単位で話して確認する。
//!
//! Linux と Cygwin のターゲットで実行する（`./tools/bridge.sh test <cygwin|linux|all>`）。子プロセスのシェルは
//! `/bin/sh` を使う。手元で大きなものを流すときは、環境変数 `FANTERMO_BRIDGE_BULK_BYTES`（大きな入力のバイト数）と
//! `FANTERMO_BRIDGE_LINE_EDITOR_BYTES`（zsh の行編集に流す改行のない行のバイト数）を付ける。WSL の中で実行するときは
//! `WSLENV` にその名前を足す。

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use fantermo_bridge_protocol::{self as protocol, ChildExit, FromBridge, ToBridge, WindowSize};

/// 単位が届くのを待つ上限。
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(30);
/// リサイズのあと、子プロセスに大きさが届いたかを確認し直す間隔。
const RESIZE_RETRY_INTERVAL: Duration = Duration::from_millis(500);
/// リサイズのあと、子プロセスに大きさが届いたかを確認する回数。
const RESIZE_RETRIES: u32 = 20;
/// ブリッジ自身が終わるのを待つ上限。
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
/// ブリッジ自身が終わったかを確認する間隔。
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// プロトコルの誤りでブリッジが終わるときの終了コード。
const EXIT_PROTOCOL_ERROR: i32 = 3;

const SIZE: WindowSize = WindowSize {
    cols: 132,
    rows: 50,
    width_px: 1188,
    height_px: 1000,
};

/// テストが起動したブリッジ。破棄すると止める。
struct Bridge {
    child: Child,
    stdin: Option<ChildStdin>,
    received: mpsc::Receiver<FromBridge>,
    /// ここまでに受け取ったデータの単位の中身。
    output: Vec<u8>,
}

impl Bridge {
    fn start(args: &[&str]) -> Self {
        Self::start_with_stderr(args, Stdio::inherit())
    }

    fn start_with_stderr(args: &[&str], stderr: Stdio) -> Self {
        let mut child = Command::new(bridge_path())
            .args(args)
            .env("TERM", "xterm-256color")
            .env_remove("HOME")
            .env_remove("SHELL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .expect("failed to start the bridge");
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().expect("the bridge has no stdout");
        let (sender, received) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(Some(message)) = protocol::read_message::<FromBridge>(&mut stdout) {
                if sender.send(message).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            received,
            output: Vec::new(),
        }
    }

    /// 起動して開始の通知を交わし、大きさを伝えたブリッジ。
    fn start_with_size(args: &[&str]) -> Self {
        let mut bridge = Self::start(args);
        bridge.exchange_hello_and_size();
        bridge
    }

    fn exchange_hello_and_size(&mut self) {
        self.expect_hello();
        self.send(&ToBridge::Hello {
            version: protocol::VERSION,
        });
        self.send(&ToBridge::Resize(SIZE));
    }

    fn send(&mut self, message: &ToBridge) {
        let stdin = self.stdin.as_mut().expect("stdin is already closed");
        protocol::write_message(stdin, message).expect("failed to write to the bridge");
        stdin.flush().expect("failed to flush the bridge's stdin");
    }

    fn type_line(&mut self, line: &str) {
        self.send(&ToBridge::Data(format!("{line}\r").into_bytes()));
    }

    fn receive(&self, timeout: Duration) -> Result<FromBridge, mpsc::RecvTimeoutError> {
        self.received.recv_timeout(timeout)
    }

    fn expect_hello(&self) {
        match self.receive(RECEIVE_TIMEOUT) {
            Ok(FromBridge::Hello { version }) => assert_eq!(version, protocol::VERSION),
            other => panic!("expected the bridge's hello, got {other:?}"),
        }
    }

    fn output_text(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }

    /// `needle` が出力に出るまで、最長で `timeout` 待つ。出たら `true`。
    fn try_wait_output(&mut self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.output_text().contains(needle) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.receive(left) {
                Ok(FromBridge::Data(data)) => self.output.extend_from_slice(&data),
                Ok(other) => panic!(
                    "waiting for {needle:?}, got {other:?}; output: {}",
                    self.output_text()
                ),
                Err(mpsc::RecvTimeoutError::Timeout) => return false,
                Err(mpsc::RecvTimeoutError::Disconnected) => panic!(
                    "the bridge closed while waiting for {needle:?}; output: {}",
                    self.output_text()
                ),
            }
        }
    }

    fn wait_output(&mut self, needle: &str) {
        assert!(
            self.try_wait_output(needle, RECEIVE_TIMEOUT),
            "{needle:?} did not appear; output: {}",
            self.output_text()
        );
    }

    /// 子プロセスの終了の単位を待つ。途中のデータは溜める。
    fn wait_child_exit(&mut self) -> ChildExit {
        loop {
            match self.receive(RECEIVE_TIMEOUT) {
                Ok(FromBridge::Data(data)) => self.output.extend_from_slice(&data),
                Ok(FromBridge::Exit(exit)) => return exit,
                other => panic!(
                    "waiting for the child's exit, got {other:?}; output: {}",
                    self.output_text()
                ),
            }
        }
    }

    /// エラーの単位を待ち、その文を返す。
    fn wait_error(&mut self) -> String {
        match self.receive(RECEIVE_TIMEOUT) {
            Ok(FromBridge::Error(message)) => message,
            other => panic!("expected an error from the bridge, got {other:?}"),
        }
    }

    /// ブリッジ自身が終わるのを待つ。
    fn wait_bridge_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .expect("failed to wait for the bridge")
            {
                return status;
            }
            assert!(Instant::now() < deadline, "the bridge did not exit");
            thread::sleep(EXIT_POLL_INTERVAL);
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        if let Err(e) = self.child.kill() {
            eprintln!("failed to stop the bridge: {e}");
        }
        if let Err(e) = self.child.wait() {
            eprintln!("failed to wait for the bridge: {e}");
        }
    }
}

/// ブリッジの実行ファイル。テストの実行ファイルのあるフォルダから上へ探す。コンパイルしたときのパス
/// （`CARGO_BIN_EXE_*`）は Windows のパスなので、WSL の中で実行するときに使えない。
fn bridge_path() -> PathBuf {
    let test_exe = std::env::current_exe().expect("failed to get the test's path");
    let file_name = format!("fantermo-bridge{}", std::env::consts::EXE_SUFFIX);
    test_exe
        .ancestors()
        .map(|folder| folder.join(&file_name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("{file_name} is not above {}", test_exe.display()))
}

#[test]
fn shell_exchanges_data_size_and_exit_code() {
    let mut bridge = Bridge::start_with_size(&["--cwd", "/tmp", "--", "/bin/sh"]);
    bridge.type_line("echo hello-$((6*7)); pwd; echo term=$TERM; stty size");
    bridge.wait_output("hello-42");
    bridge.wait_output("\r\n/tmp\r\n");
    bridge.wait_output("term=xterm-256color");
    bridge.wait_output("50 132");

    bridge.send(&ToBridge::Resize(WindowSize {
        cols: 100,
        rows: 30,
        width_px: 0,
        height_px: 0,
    }));
    let mut resized = false;
    for _ in 0..RESIZE_RETRIES {
        bridge.type_line("stty size");
        if bridge.try_wait_output("30 100", RESIZE_RETRY_INTERVAL) {
            resized = true;
            break;
        }
    }
    assert!(
        resized,
        "the new size did not reach the child; output: {}",
        bridge.output_text()
    );

    bridge.type_line("exit 7");
    assert_eq!(bridge.wait_child_exit(), ChildExit::Code(7));
    assert!(bridge.wait_bridge_exit().success());
}

#[test]
fn quit_request_ends_the_child_and_the_bridge() {
    let mut bridge = Bridge::start_with_size(&["--", "/bin/sh"]);
    bridge.type_line("echo ready-$((1+1))");
    bridge.wait_output("ready-2\r\n");
    bridge.send(&ToBridge::Quit);
    bridge.wait_child_exit();
    assert!(bridge.wait_bridge_exit().success());
}

#[test]
fn closing_stdin_ends_the_bridge() {
    let mut bridge = Bridge::start_with_size(&["--", "/bin/sh"]);
    bridge.type_line("echo ready-$((1+1))");
    bridge.wait_output("ready-2\r\n");
    bridge.stdin = None;
    assert!(bridge.wait_bridge_exit().success());
}

#[test]
fn another_protocol_version_is_refused() {
    let mut bridge = Bridge::start(&["--", "/bin/sh"]);
    bridge.expect_hello();
    bridge.send(&ToBridge::Hello {
        version: protocol::VERSION + 1,
    });
    bridge.wait_error();
    assert_eq!(bridge.wait_bridge_exit().code(), Some(EXIT_PROTOCOL_ERROR));
}

#[test]
fn data_before_the_first_resize_is_refused() {
    let mut bridge = Bridge::start(&["--", "/bin/sh"]);
    bridge.expect_hello();
    bridge.send(&ToBridge::Hello {
        version: protocol::VERSION,
    });
    bridge.send(&ToBridge::Data(b"x".to_vec()));
    bridge.wait_error();
    assert_eq!(bridge.wait_bridge_exit().code(), Some(EXIT_PROTOCOL_ERROR));
}

#[test]
fn child_runs_even_if_the_working_folder_is_missing() {
    let mut bridge =
        Bridge::start_with_size(&["--cwd", "/no/such/dir", "--", "/bin/sh", "-c", "exit 0"]);
    assert_eq!(bridge.wait_child_exit(), ChildExit::Code(0));
    assert!(bridge.wait_bridge_exit().success());
}

/// ログインシェルが起動する。ログインシェルは環境で違うので、名前が `-` で始まることだけを見る。
/// 先頭の空白は、シェルの履歴に残さないため。
#[test]
fn without_a_command_the_login_shell_runs() {
    let mut bridge = Bridge::start_with_size(&[]);
    bridge.type_line(" echo shell=$0; exit 3");
    bridge.wait_output("shell=-");
    assert_eq!(bridge.wait_child_exit(), ChildExit::Code(3));
    assert!(bridge.wait_bridge_exit().success());
}

/// 大きな入力の大きさ（バイト）。Cygwin の pty の内部の pipe（128 KiB）に一度に書けば中身が崩れる大きさ。
const DEFAULT_BULK_BYTES: usize = 200_000;
/// 大きな入力の大きさを変える環境変数。手元で大きなものを流すときに使う。
const BULK_BYTES_ENV: &str = "FANTERMO_BRIDGE_BULK_BYTES";
/// 大きな入力の 1 行の長さ（改行を含む）。
const BULK_LINE_LEN: usize = 100;
/// 大きな入力を待つときに見込む、最も遅い速さ（バイト毎秒）。Cygwin の pty は 1 本で約 50 KB/s で、並列に流すと
/// 全体で頭打ちになる。
const SLOWEST_INPUT_BYTES_PER_SECOND: usize = 5_000;
/// 読まない子プロセスへ送る大きさ。Cygwin の pty が一度に受ける量を超える。
const STALLED_INPUT_BYTES: usize = 500_000;
/// 読まない子プロセスへ、閉じる間際に送る大きさ。
const STALLED_BIG_INPUT_BYTES: usize = protocol::MAX_PAYLOAD_LEN;
/// 入力を送ったあと、ブリッジが子プロセスへの書き込みで止まるまでの間。止まらなくても結果は同じで、止まる前に
/// 閉じると確認の意味が薄れるだけ。
const STALL_SETTLE: Duration = Duration::from_millis(300);
/// 子プロセスが動き出す、または終わるのを待つ上限。
const CHILD_GONE_TIMEOUT: Duration = Duration::from_secs(30);
/// 子プロセスが動き出したか、終わったかを確認する間隔。
const CHILD_GONE_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// シェルの行編集に流す、改行のない長い行の大きさを指定する環境変数。重いので指定したときだけ実行する。
const LINE_EDITOR_BYTES_ENV: &str = "FANTERMO_BRIDGE_LINE_EDITOR_BYTES";
/// シェルの行編集を待つ上限。Cygwin の zsh は 100 KB で 74 秒かかる。
const LINE_EDITOR_TIMEOUT: Duration = Duration::from_secs(600);

/// テストの実行ファイルの中で重ならない番号。
fn next_unique() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// 子プロセスが書き出す一時ファイル。破棄すると消す。テストと子プロセスは同じ環境で動くので、同じパスで読める。
struct OutputFile(String);

impl OutputFile {
    fn new() -> Self {
        Self(format!(
            "/tmp/fantermo-bridge-test-{}-{}",
            std::process::id(),
            next_unique()
        ))
    }
}

impl Drop for OutputFile {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.0)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("failed to remove {}: {e}", self.0);
        }
    }
}

/// 行ごとに中身の違う、`len` バイトの入力。順序が崩れたり欠けたりすると分かる。
fn numbered_lines(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len + BULK_LINE_LEN);
    let mut number = 0usize;
    while data.len() < len {
        data.extend_from_slice(format!("{number:0width$}\n", width = BULK_LINE_LEN - 1).as_bytes());
        number += 1;
    }
    data.truncate(len);
    data
}

fn bulk_bytes() -> usize {
    std::env::var(BULK_BYTES_ENV)
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(DEFAULT_BULK_BYTES)
}

/// `stty` の設定 `stty` のもとで大きな入力を流し、子プロセスが受け取ったものと比べる。
fn exercise_bulk_input(stty: &str) {
    let data = numbered_lines(bulk_bytes());
    let file = OutputFile::new();
    let script = format!(
        "stty {stty}; echo bulk-ready-$((1+1)); head -c {} > {}; echo bulk-done-$((1+1)); exit 5",
        data.len(),
        file.0
    );
    let mut bridge = Bridge::start_with_size(&["--", "/bin/sh", "-c", &script]);
    bridge.wait_output("bulk-ready-2");
    for chunk in data.chunks(protocol::MAX_PAYLOAD_LEN) {
        bridge.send(&ToBridge::Data(chunk.to_vec()));
    }
    let timeout =
        RECEIVE_TIMEOUT + Duration::from_secs((data.len() / SLOWEST_INPUT_BYTES_PER_SECOND) as u64);
    assert!(
        bridge.try_wait_output("bulk-done-2", timeout),
        "the child did not read all of the input; {} bytes arrived",
        std::fs::metadata(&file.0).map_or(0, |metadata| metadata.len())
    );
    let received = std::fs::read(&file.0).expect("the child did not write the file");
    assert_eq!(received.len(), data.len());
    if let Some(at) = received.iter().zip(&data).position(|(a, b)| a != b) {
        panic!("the input differs at byte {at}");
    }
    assert_eq!(bridge.wait_child_exit(), ChildExit::Code(5));
    assert!(bridge.wait_bridge_exit().success());
}

#[test]
fn bulk_input_reaches_the_child_unchanged_without_echo() {
    exercise_bulk_input("-echo");
}

#[test]
fn bulk_input_reaches_the_child_unchanged_with_echo() {
    exercise_bulk_input("echo");
}

#[test]
fn bulk_output_reaches_the_terminal_in_order() {
    const LAST: usize = 20_000;
    let script = format!("seq 1 {LAST}; exit 6");
    let mut bridge = Bridge::start_with_size(&["--", "/bin/sh", "-c", &script]);
    assert_eq!(bridge.wait_child_exit(), ChildExit::Code(6));
    let text = bridge.output_text();
    let numbers: Vec<&str> = text.split_terminator("\r\n").collect();
    assert_eq!(numbers.len(), LAST);
    for (index, number) in numbers.iter().enumerate() {
        assert_eq!(number.parse(), Ok(index + 1));
    }
    assert!(bridge.wait_bridge_exit().success());
}

/// `sleep` の引数にできる、実行中のプロセスの中で重ならない数。`/proc` のコマンドラインで見分ける。
fn unique_sleep_seconds() -> String {
    format!("300.{}{}", std::process::id(), next_unique())
}

/// コマンドラインに `sleep` と `seconds` を持つプロセスの番号。`/proc` の番号の項目（プロセス）だけを見る。
fn sleep_pids(seconds: &str) -> Vec<libc::pid_t> {
    let entries = std::fs::read_dir("/proc").expect("cannot read /proc");
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<libc::pid_t>().ok()?;
            let line = std::fs::read(entry.path().join("cmdline")).ok()?;
            let mut args = line.split(|&byte| byte == 0);
            let is_sleep = args
                .next()
                .is_some_and(|program| program.ends_with(b"sleep"));
            (is_sleep && args.next() == Some(seconds.as_bytes())).then_some(pid)
        })
        .collect()
}

/// コマンドラインに `sleep` と `seconds` を持つプロセスが動いている。
fn sleep_is_running(seconds: &str) -> bool {
    !sleep_pids(seconds).is_empty()
}

fn wait_sleep_gone(seconds: &str) {
    let deadline = Instant::now() + CHILD_GONE_TIMEOUT;
    while sleep_is_running(seconds) {
        assert!(
            Instant::now() < deadline,
            "the child `sleep {seconds}` is still running"
        );
        thread::sleep(CHILD_GONE_POLL_INTERVAL);
    }
}

/// 入力を読まない子プロセスを起動する。戻ったときには、子プロセスは `sleep` を動かしている。
fn start_child_that_does_not_read(stty: &str, seconds: &str) -> Bridge {
    let script = format!("stty {stty}; echo stall-ready-$((1+1)); exec sleep {seconds}");
    let mut bridge = Bridge::start_with_size(&["--", "/bin/sh", "-c", &script]);
    bridge.wait_output("stall-ready-2");
    let deadline = Instant::now() + CHILD_GONE_TIMEOUT;
    while !sleep_is_running(seconds) {
        assert!(
            Instant::now() < deadline,
            "the child `sleep {seconds}` did not start"
        );
        thread::sleep(CHILD_GONE_POLL_INTERVAL);
    }
    bridge
}

/// 子プロセスが入力を読まなくても、終了の依頼で終わる。
fn exercise_quit_with_a_stalled_child(stty: &str) {
    let seconds = unique_sleep_seconds();
    let mut bridge = start_child_that_does_not_read(stty, &seconds);
    bridge.send(&ToBridge::Data(numbered_lines(STALLED_INPUT_BYTES)));
    thread::sleep(STALL_SETTLE);
    bridge.send(&ToBridge::Quit);
    bridge.wait_child_exit();
    assert!(bridge.wait_bridge_exit().success());
    wait_sleep_gone(&seconds);
}

#[test]
fn quit_ends_a_child_that_does_not_read_without_echo() {
    exercise_quit_with_a_stalled_child("raw -echo");
}

#[test]
fn quit_ends_a_child_that_does_not_read_with_echo() {
    exercise_quit_with_a_stalled_child("echo");
}

/// 子プロセスが読まない入力を流している途中で標準入力を閉じても（タブを閉じるときの形）、ブリッジも子プロセスも
/// 終わる。
#[test]
fn closing_stdin_ends_a_child_that_does_not_read() {
    let seconds = unique_sleep_seconds();
    let mut bridge = start_child_that_does_not_read("-echo", &seconds);
    bridge.send(&ToBridge::Data(numbered_lines(STALLED_BIG_INPUT_BYTES)));
    bridge.stdin = None;
    assert!(bridge.wait_bridge_exit().success());
    wait_sleep_gone(&seconds);
}

/// fantermo が終わると標準入力と標準エラー出力が同時に閉じる。ログを書けなくても、子プロセスを終わらせて普通に終わる。
#[test]
fn closing_stdin_and_stderr_together_ends_the_child() {
    let seconds = unique_sleep_seconds();
    let script = format!("echo stderr-ready-$((1+1)); exec sleep {seconds}");
    let mut bridge = Bridge::start_with_stderr(&["--", "/bin/sh", "-c", &script], Stdio::piped());
    bridge.exchange_hello_and_size();
    bridge.wait_output("stderr-ready-2");
    let deadline = Instant::now() + CHILD_GONE_TIMEOUT;
    while !sleep_is_running(&seconds) {
        assert!(
            Instant::now() < deadline,
            "the child `sleep {seconds}` did not start"
        );
        thread::sleep(CHILD_GONE_POLL_INTERVAL);
    }
    drop(bridge.child.stderr.take());
    bridge.stdin = None;
    assert!(bridge.wait_bridge_exit().success());
    wait_sleep_gone(&seconds);
}

/// 送られたとしたら SIGHUP が効くのを待つ時間。プロセスが残ったことを見る前に置く。
const SIGNAL_SETTLE: Duration = Duration::from_secs(1);
/// 閉じ始めたときの背景のジョブとして扱われるよう、`sleep` を exec してから閉じるまで待つ時間。ブリッジが背景の
/// ジョブとみなす 500 ms より長くする。
const BACKGROUND_SETTLED: Duration = Duration::from_millis(1500);
/// 起動して間もない背景のジョブを、ブリッジがそうみなす回が出るまで試す回数。
const YOUNG_BACKGROUND_ATTEMPTS: u32 = 5;
/// ブリッジが、記録した背景のジョブのプロセスグループを書くログの行の頭（`main.rs` の `record_background`）。
const BACKGROUND_LOG: &str = "fantermo-bridge: 背景のジョブとして送らないプロセスグループ: ";

/// fantermo のウィンドウを閉じたときと同じく、標準エラー出力と標準入力を同時に閉じる。
fn close_like_the_window(bridge: &mut Bridge) {
    drop(bridge.child.stderr.take());
    bridge.stdin = None;
}

/// 標準エラー出力をパイプにして起動し、開始の通知と大きさを交わしたブリッジ。
fn start_with_piped_stderr(args: &[&str]) -> Bridge {
    let mut bridge = Bridge::start_with_stderr(args, Stdio::piped());
    bridge.exchange_hello_and_size();
    bridge
}

/// 標準エラー出力を閉じるまで読むスレッド。ブリッジが終わったあとに join すると、ログの全体を返す。
fn collect_stderr(bridge: &mut Bridge) -> thread::JoinHandle<String> {
    let mut stderr = bridge.child.stderr.take().expect("stderr is not piped");
    thread::spawn(move || {
        let mut log = String::new();
        if let Err(e) = std::io::Read::read_to_string(&mut stderr, &mut log) {
            eprintln!("failed to read the bridge's log: {e}");
        }
        log
    })
}

/// ブリッジのログから、記録した背景のジョブのプロセスグループを読む。
fn recorded_background(log: &str) -> Vec<libc::pid_t> {
    let line = log
        .lines()
        .find_map(|line| line.strip_prefix(BACKGROUND_LOG))
        .unwrap_or_else(|| panic!("the bridge did not log the background jobs: {log}"));
    line.trim_matches(|c| c == '[' || c == ']')
        .split(", ")
        .filter(|group| !group.is_empty())
        .map(|group| group.parse().expect("not a process group"))
        .collect()
}

/// 番号 `pid` のプロセスが動いている（終わっていて、親に回収されるのを待っているだけのものは除く）。
fn process_is_running(pid: libc::pid_t) -> bool {
    let Ok(stat) = std::fs::read(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(close) = stat.iter().rposition(|&byte| byte == b')') else {
        return false;
    };
    let state = stat[close + 1..]
        .iter()
        .find(|byte| !byte.is_ascii_whitespace());
    !matches!(state, Some(b'Z' | b'X'))
}

/// 番号 `pid` のプロセスを止める。
fn kill_process(pid: libc::pid_t) {
    // SAFETY: 引数は数だけ。
    if unsafe { libc::kill(pid, libc::SIGKILL) } < 0 {
        eprintln!(
            "failed to stop the process {pid}: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// 番号 `pid` のプロセスが終わるのを待つ。終わらなければ、止めてから失敗にする。
fn wait_process_gone(pid: libc::pid_t, what: &str) {
    let deadline = Instant::now() + CHILD_GONE_TIMEOUT;
    while process_is_running(pid) {
        if Instant::now() >= deadline {
            kill_process(pid);
            panic!("{what} ({pid}) is still running");
        }
        thread::sleep(CHILD_GONE_POLL_INTERVAL);
    }
}

/// 番号 `pid` のプロセスが、`sleep` を exec し終えるのを待つ。
fn wait_exec_sleep(pid: libc::pid_t) {
    let deadline = Instant::now() + CHILD_GONE_TIMEOUT;
    loop {
        let line = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        if line
            .split(|&byte| byte == 0)
            .next()
            .is_some_and(|program| program.ends_with(b"sleep"))
        {
            return;
        }
        if Instant::now() >= deadline {
            kill_process(pid);
            panic!("the process {pid} did not execute `sleep`");
        }
        thread::sleep(CHILD_GONE_POLL_INTERVAL);
    }
}

/// 出力の `{tag}-<番号>-{tag}-end` から番号を読む。シェルに `echo {tag}-$!-{tag}-end` と書かせて、起動した背景の
/// ジョブの番号を受け取る。
fn reported_pid(bridge: &mut Bridge, tag: &str) -> libc::pid_t {
    let end = format!("-{tag}-end");
    bridge.wait_output(&end);
    let text = bridge.output_text();
    let before = &text[..text.find(&end).expect("the end of the number is missing")];
    let start = before
        .rfind(&format!("{tag}-"))
        .expect("the number is missing")
        + tag.len()
        + 1;
    before[start..].parse().expect("not a process number")
}

/// 子プロセスが書いた番号のファイルを読む。
fn read_pid_file(file: &OutputFile) -> libc::pid_t {
    let text = std::fs::read_to_string(&file.0).expect("the shell did not write the number");
    text.trim().parse().expect("not a process number")
}

/// シェルが SIGHUP を受けたときに `started` で背景のジョブを起動し、その番号を `pid_file` に書いてから終わる
/// スクリプト。起動するものは、シェルが SIGHUP を受けたときに起動の途中だったコマンドの代わり。
fn script_starting_on_hang_up(started: &str, pid_file: &str) -> String {
    format!(
        "trap '{started} echo $! > {pid_file}\nexit' HUP; echo hup-ready-$((1+1)); while :; do sleep 0.05; done"
    )
}

/// 閉じ方。
#[derive(Clone, Copy)]
enum Close {
    /// fantermo のウィンドウを閉じたとき（[`close_like_the_window`]）。
    Window,
    /// 終了の依頼だけを送り、標準入力は閉じない。シェルが終わったのを中継の中で見つける。
    Quit,
}

/// シェルが SIGHUP を受けたときに `started`（`sleep <秒> &` の形）で起動したコマンドが、`close` で閉じたあとに
/// 残らない。
fn exercise_command_started_on_hang_up(started: &str, close: Close) {
    let file = OutputFile::new();
    let script = script_starting_on_hang_up(started, &file.0);
    let mut bridge = start_with_piped_stderr(&["--", "/bin/sh", "-c", &script]);
    bridge.wait_output("hup-ready-2");
    match close {
        Close::Window => close_like_the_window(&mut bridge),
        Close::Quit => {
            bridge.send(&ToBridge::Quit);
            bridge.wait_child_exit();
        }
    }
    assert!(bridge.wait_bridge_exit().success());
    // シェルは番号を書いてから終わり、ブリッジはシェルが終わってから終わる。
    let pid = read_pid_file(&file);
    wait_process_gone(pid, "the command started on the hang-up");
}

/// シェルが SIGHUP で終わるときに起動の途中だったコマンドが、シェルのプロセスグループに残っても終わる。Cygwin は
/// セッションのリーダーが終わっても前面のプロセスグループに SIGHUP を送らない。
#[test]
fn a_command_left_in_the_shells_group_after_the_hang_up_is_ended() {
    let started = format!("sleep {} &", unique_sleep_seconds());
    exercise_command_started_on_hang_up(&started, Close::Window);
}

/// シェルが SIGHUP で終わるときに起動の途中だったコマンドが、自分のプロセスグループに移ったまま前面になれずに
/// 残っても終わる。セッションのリーダーが終わったあとは前面のプロセスグループを変えられない。
#[test]
fn a_command_left_in_its_own_group_after_the_hang_up_is_ended() {
    let started = format!("set -m; sleep {} &", unique_sleep_seconds());
    exercise_command_started_on_hang_up(&started, Close::Window);
}

/// 終了の依頼で SIGHUP を送ったあと、標準入力が閉じる前にシェルが終わっても、シェルのプロセスグループに残った
/// コマンドは終わる。
#[test]
fn a_command_left_in_the_shells_group_after_the_quit_request_is_ended() {
    let started = format!("sleep {} &", unique_sleep_seconds());
    exercise_command_started_on_hang_up(&started, Close::Quit);
}

/// 終了の依頼で SIGHUP を送ったあと、標準入力が閉じる前にシェルが終わっても、自分のプロセスグループに残った
/// コマンドは終わる。
#[test]
fn a_command_left_in_its_own_group_after_the_quit_request_is_ended() {
    let started = format!("set -m; sleep {} &", unique_sleep_seconds());
    exercise_command_started_on_hang_up(&started, Close::Quit);
}

/// シェルが自分で終わったときは、起動して間もない背景のジョブにも SIGHUP を送らない。
#[test]
fn a_young_background_job_is_left_when_the_shell_exits_by_itself() {
    let seconds = unique_sleep_seconds();
    let script = format!("set -m; sleep {seconds} & echo self-$!-self-end; set +m; exit 3");
    let mut bridge = start_with_piped_stderr(&["--", "/bin/sh", "-c", &script]);
    assert_eq!(bridge.wait_child_exit(), ChildExit::Code(3));
    let pid = reported_pid(&mut bridge, "self");
    assert!(bridge.wait_bridge_exit().success());
    thread::sleep(SIGNAL_SETTLE);
    let left = process_is_running(pid);
    kill_process(pid);
    assert!(left, "the background job ({pid}) was ended");
    wait_process_gone(pid, "the stopped background job");
}

/// 閉じ始めたときにしばらく動いていた背景のジョブには、SIGHUP を送らない。
#[test]
fn a_background_job_running_before_the_close_is_left() {
    let seconds = unique_sleep_seconds();
    let script = format!(
        "set -m; sleep {seconds} & echo bg-$!-bg-end; set +m; while :; do sleep 0.05; done"
    );
    let mut bridge = start_with_piped_stderr(&["--", "/bin/sh", "-c", &script]);
    let pid = reported_pid(&mut bridge, "bg");
    // Cygwin は exec のたびに起動の時刻を付け直すので、exec してから数える。
    wait_exec_sleep(pid);
    thread::sleep(BACKGROUND_SETTLED);
    close_like_the_window(&mut bridge);
    assert!(bridge.wait_bridge_exit().success());
    thread::sleep(SIGNAL_SETTLE);
    let left = process_is_running(pid);
    kill_process(pid);
    assert!(left, "the background job ({pid}) was ended");
    wait_process_gone(pid, "the stopped background job");
}

/// 閉じる直前に起動した背景のジョブは、前面になる前のコマンドと見分けられないので、SIGHUP を送る。
///
/// 起動して間もないかはブリッジが閉じ始めたときに決めるので、ブリッジがログに書いた背景のジョブで見る。遅い環境で
/// ブリッジが背景のジョブとみなした回は、止めてやり直す。ログを読むため、標準入力だけを閉じる（タブを閉じたときの形）。
#[test]
fn a_background_job_started_right_before_the_close_is_ended() {
    for _ in 0..YOUNG_BACKGROUND_ATTEMPTS {
        let seconds = unique_sleep_seconds();
        let script = format!(
            "echo young-ready-$((1+1)); read line; set -m; sleep {seconds} & echo young-$!-young-end; set +m; while :; do sleep 0.05; done"
        );
        let mut bridge = start_with_piped_stderr(&["--", "/bin/sh", "-c", &script]);
        let log = collect_stderr(&mut bridge);
        bridge.wait_output("young-ready-2");
        bridge.type_line("");
        let pid = reported_pid(&mut bridge, "young");
        bridge.stdin = None;
        assert!(bridge.wait_bridge_exit().success());
        let log = log.join().expect("the thread reading the log panicked");
        if recorded_background(&log).contains(&pid) {
            kill_process(pid);
            wait_process_gone(pid, "the stopped background job");
            continue;
        }
        wait_process_gone(pid, "the background job started right before the close");
        return;
    }
    panic!(
        "the bridge took the job started right before the close for a background job in {YOUNG_BACKGROUND_ATTEMPTS} attempts"
    );
}

/// シェルの行編集に、改行のない長い行を流す。環境変数があるときだけ実行する（`zsh` が要り、重い）。
#[test]
fn line_editor_receives_a_long_line() {
    let Some(size) = std::env::var(LINE_EDITOR_BYTES_ENV)
        .ok()
        .and_then(|text| text.parse::<usize>().ok())
    else {
        // テストの出力を横取りされないよう、直接書く。
        writeln!(
            std::io::stderr(),
            "SKIPPED: line_editor_receives_a_long_line（環境変数 {LINE_EDITOR_BYTES_ENV} がない）"
        )
        .expect("cannot report the skip");
        return;
    };
    let mut bridge = Bridge::start_with_size(&["--", "zsh", "-f"]);
    bridge.type_line(&format!("x='{}'", "a".repeat(size)));
    bridge.type_line("echo len=${#x}");
    bridge.type_line("exit 4");
    assert!(
        bridge.try_wait_output(&format!("len={size}\r\n"), LINE_EDITOR_TIMEOUT),
        "the line did not reach the shell"
    );
    assert_eq!(bridge.wait_child_exit(), ChildExit::Code(4));
    assert!(bridge.wait_bridge_exit().success());
}
