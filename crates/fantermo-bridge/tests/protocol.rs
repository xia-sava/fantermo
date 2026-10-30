//! ブリッジを標準入出力のパイプ付きで起動し、プロトコルの単位で話して確かめる。
//!
//! Linux と Cygwin のターゲットで実行する（`./tools/bridge.sh test <cygwin|linux|all>`）。子プロセスのシェルは
//! `/bin/sh` を使う。

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use fantermo_bridge_protocol::{self as protocol, ChildExit, FromBridge, ToBridge, WindowSize};

/// 単位が届くのを待つ上限。
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(30);
/// リサイズのあと、子プロセスに大きさが届いたかを確かめ直す間隔。
const RESIZE_RETRY_INTERVAL: Duration = Duration::from_millis(500);
/// リサイズのあと、子プロセスに大きさが届いたかを確かめる回数。
const RESIZE_RETRIES: u32 = 20;
/// ブリッジ自身が終わるのを待つ上限。
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
/// ブリッジ自身が終わったかを確かめる間隔。
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
        let mut child = Command::new(bridge_path())
            .args(args)
            .env("TERM", "xterm-256color")
            .env_remove("HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
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

    /// 起動して名乗りを交わし、大きさを伝えたブリッジ。
    fn start_with_size(args: &[&str]) -> Self {
        let mut bridge = Self::start(args);
        bridge.expect_hello();
        bridge.send(&ToBridge::Hello {
            version: protocol::VERSION,
        });
        bridge.send(&ToBridge::Resize(SIZE));
        bridge
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
