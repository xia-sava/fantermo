//! fantermo とシェルの間を中継するブリッジ。Cygwin と Linux（WSL）で動く。
//!
//! `fantermo-bridge [--cwd <パス>] [--] [<コマンド> [引数…]]`。コマンドがなければ利用者のシェル
//! （`$SHELL`、なければ passwd のシェル、それもなければ `/bin/sh`）をログインシェルとして起動する。標準入出力は fantermo との間の
//! プロトコル（`fantermo-bridge-protocol`）、標準エラー出力はログ。
//!
//! 開始の通知を交換し、最初のリサイズを受け取ってから、その大きさの pty で子プロセスを起動する。
//! そのあとは標準入力と pty を `poll` で見て中継し（Cygwin では時間切れ 0 で覗き、自分で待つ。[`PollWait`]）、
//! 子プロセスが終わったら終了の単位を送って終わる。
//! 環境変数（`TERM` など）は fantermo が渡したものをそのまま子プロセスに引き継ぐ。

use std::cell::OnceCell;
use std::collections::HashSet;
use std::ffi::{CStr, CString, OsString};
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use fantermo_bridge_protocol::{
    self as protocol, ChildExit, Decoder, FromBridge, ToBridge, VERSION, WindowSize,
};
use libc::{c_char, c_int, pid_t};

const STDIN: c_int = 0;
const STDOUT: c_int = 1;
const STDERR: c_int = 2;

/// 標準入力と pty から一度に読む上限。
const READ_BUF_LEN: usize = 64 * 1024;
/// pty の slave の名前を受け取る長さ。
#[cfg(target_os = "cygwin")]
const PTY_NAME_LEN: usize = 64;
/// pty へまだ書き終えていない入力がこれを超えたら、標準入力を読むのを止める。読むのを止めると
/// その後ろの終了の依頼やリサイズも届かないので、ふつうの貼り付けでは届かない大きさにする。
const MAX_PENDING_INPUT: usize = 64 << 20;
/// `poll` で待つ上限。子プロセスが終わったかを確認する間隔でもある。Cygwin では使わない（[`PollWait`]）。
#[cfg(not(target_os = "cygwin"))]
const POLL_INTERVAL_MS: c_int = 200;
/// SIGHUP を送ったあと、子プロセスと残ったプロセスが終わるのを待つ上限。標準入力が閉じたときなどは SIGHUP を
/// 送ってからの時間で、シェルを待つ時間と送り直す時間を合わせてこれまで（[`hang_up_wait`]）。終了の依頼で送った
/// あとにシェルが終わったのを中継の中で見つけたときは、シェルが終わってから送り直す時間がこれまで（[`Relay::finish`]）。
const HANGUP_WAIT_MS: u32 = 2000;
/// 上の待ちの一回の長さ。
const HANGUP_WAIT_STEP_MS: u32 = 50;
/// シェルが終わったあと、残ったプロセスへ SIGHUP を送り直す間隔。
const RESEND_HANGUP_INTERVAL_MS: u32 = 200;
/// 送り直す先がこの回数続けて見つからなければ、送り直すのをやめる。
const EMPTY_RESENDS_TO_STOP: u32 = 2;
/// 閉じ始めたときに、起動してからこれを過ぎたプロセスのいるプロセスグループを背景のジョブとみなす。
/// 起動して間もないプロセスだけのグループは、自分のプロセスグループに移ったが前面になる前のコマンドでありうる。
/// Cygwin は exec のたびに起動の時刻を付け直すので、Cygwin では最後の exec からの時間になる。
const BACKGROUND_JOB_MIN_AGE_MS: u64 = 500;
/// プロセスの起動の時刻が起動からの時間（`/proc/uptime`）をこれより超えていたら、時計が合わないとみなし、
/// 起動の時刻が分からないものとして扱う。`/proc/uptime` は 10 ms 単位で切り捨てるので、時計が合っていても
/// その分は超えうる。
const START_TIME_TOLERANCE_MS: u64 = 100;
const MILLIS_PER_SECOND: u64 = 1000;
/// `/proc/uptime` の小数部のうち読む桁数（ミリ秒まで）。
const UPTIME_FRACTION_DIGITS: usize = 3;
/// `$SHELL` がないときのシェル。
const DEFAULT_SHELL: &str = "/bin/sh";
/// 子プロセスで `exec` できなかったときの終了コード（シェルと同じ）。
const EXIT_CODE_EXEC_FAILED: c_int = 127;

/// ブリッジ自身の終了コード。
const EXIT_OK: i32 = 0;
const EXIT_USAGE: i32 = 2;
const EXIT_PROTOCOL_ERROR: i32 = 3;
const EXIT_SPAWN_FAILED: i32 = 4;
const EXIT_IO_ERROR: i32 = 5;

const USAGE: &str = "usage: fantermo-bridge [--cwd <path>] [--] [<command> [args...]]";

macro_rules! log {
    ($($arg:tt)*) => {
        write_log(format_args!($($arg)*))
    };
}

/// ログを標準エラー出力へ書く。fantermo が先に終わると閉じているので、書けなくても続ける（子プロセスを終わらせる
/// 処理の前にも書く）。
fn write_log(message: std::fmt::Arguments<'_>) {
    _ = writeln!(io::stderr(), "fantermo-bridge: {message}");
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let options = match Options::parse(std::env::args_os().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            log!("{message}");
            log!("{USAGE}");
            return EXIT_USAGE;
        }
    };
    if let Err(error) = send(&FromBridge::Hello { version: VERSION }) {
        log!("開始の通知を送れません: {error}");
        return EXIT_IO_ERROR;
    }

    match receive_blocking() {
        Ok(Some(ToBridge::Hello { version })) => {
            if let Err(mismatch) = protocol::check_version(version) {
                report_error(&mismatch.to_string());
                return EXIT_PROTOCOL_ERROR;
            }
        }
        Ok(Some(other)) => {
            report_error(&format!(
                "最初のデータが開始の通知ではありません: {other:?}"
            ));
            return EXIT_PROTOCOL_ERROR;
        }
        Ok(None) => return EXIT_OK,
        Err(message) => {
            report_error(&message);
            return EXIT_PROTOCOL_ERROR;
        }
    }
    let size = match receive_blocking() {
        Ok(Some(ToBridge::Resize(size))) => size,
        Ok(Some(ToBridge::Quit) | None) => return EXIT_OK,
        Ok(Some(other)) => {
            report_error(&format!(
                "開始の通知の次のデータがリサイズの通知ではありません: {other:?}"
            ));
            return EXIT_PROTOCOL_ERROR;
        }
        Err(message) => {
            report_error(&message);
            return EXIT_PROTOCOL_ERROR;
        }
    };

    if let Some(cwd) = &options.cwd {
        change_directory(cwd);
    }
    let command = match Command::new(options.command) {
        Ok(command) => command,
        Err(message) => {
            report_error(&message);
            return EXIT_USAGE;
        }
    };
    let child = match Child::spawn(&command, size) {
        Ok(child) => child,
        Err(error) => {
            report_error(&format!(
                "{} を起動できません: {error}",
                command.program.to_string_lossy()
            ));
            return EXIT_SPAWN_FAILED;
        }
    };
    log!(
        "{} を起動した（pid {}、{}x{}）",
        command.program.to_string_lossy(),
        child.pid,
        size.cols,
        size.rows
    );
    match Relay::new(child) {
        Ok(relay) => relay.run(),
        Err(error) => {
            report_error(&format!("pty へ書き込むスレッドを起動できません: {error}"));
            EXIT_SPAWN_FAILED
        }
    }
}

/// コマンドラインの引数。
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    cwd: Option<OsString>,
    /// 空ならシェル。
    command: Vec<OsString>,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_bytes() {
                b"--" => break,
                b"--cwd" => {
                    let cwd = args.next().ok_or("--cwd にパスがありません")?;
                    options.cwd = Some(cwd);
                }
                bytes if bytes.starts_with(b"-") => {
                    return Err(format!("知らないオプションです: {}", arg.to_string_lossy()));
                }
                _ => {
                    options.command.push(arg);
                    break;
                }
            }
        }
        options.command.extend(args);
        Ok(options)
    }
}

/// 起動するプログラムと、子プロセスに渡す `argv`。
struct Command {
    program: CString,
    argv: Vec<CString>,
}

impl Command {
    /// `command` が空なら利用者のシェル（[`user_shell`]）を、`argv[0]` の先頭に `-` を付けた
    /// ログインシェルとして起動する。
    fn new(command: Vec<OsString>) -> Result<Self, String> {
        let to_c = |arg: OsString| {
            CString::new(arg.into_vec()).map_err(|_| "引数に NUL が含まれています".to_string())
        };
        if command.is_empty() {
            let shell = user_shell();
            let name = Path::new(&shell)
                .file_name()
                .map_or(shell.as_bytes(), |name| name.as_bytes());
            let mut login_name = b"-".to_vec();
            login_name.extend_from_slice(name);
            let login_name = to_c(OsString::from_vec(login_name))?;
            return Ok(Self {
                program: to_c(shell)?,
                argv: vec![login_name],
            });
        }
        let argv = command
            .into_iter()
            .map(to_c)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            program: argv[0].clone(),
            argv,
        })
    }
}

/// 利用者のシェル。`$SHELL`、なければ passwd のシェル、それもなければ [`DEFAULT_SHELL`]。
fn user_shell() -> OsString {
    if let Some(shell) = std::env::var_os("SHELL").filter(|shell| !shell.is_empty()) {
        return shell;
    }
    // SAFETY: getpwuid が返すのは静的な領域か null。別のスレッドから passwd を引くことはなく、
    // 次に呼ぶまでに中身を写す。
    let shell = unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() || (*entry).pw_shell.is_null() {
            None
        } else {
            Some(CStr::from_ptr((*entry).pw_shell).to_bytes().to_vec())
        }
    };
    match shell {
        Some(shell) if !shell.is_empty() => OsString::from_vec(shell),
        _ => DEFAULT_SHELL.into(),
    }
}

/// 作業フォルダを変える。できなければホームで続ける。子プロセスはこれを引き継ぐ。
fn change_directory(cwd: &OsString) {
    let Err(error) = std::env::set_current_dir(cwd) else {
        return;
    };
    log!(
        "作業フォルダを {} にできません（{error}）。ホームで続けます",
        cwd.to_string_lossy()
    );
    let Some(home) = std::env::var_os("HOME") else {
        log!("HOME がありません");
        return;
    };
    if let Err(error) = std::env::set_current_dir(&home) {
        log!(
            "作業フォルダを {} にできません（{error}）",
            home.to_string_lossy()
        );
    }
}

/// pty に接続した子プロセス。
struct Child {
    pid: pid_t,
    /// pty の master。`O_NONBLOCK` を付けて開いておく。
    master: c_int,
    /// 閉じ始めたときの背景のジョブのプロセスグループ。最初の [`Child::hang_up`] で記録する。`/proc` を読めなければ
    /// `None`。
    background: OnceCell<Option<HashSet<pid_t>>>,
}

impl Child {
    fn spawn(command: &Command, size: WindowSize) -> io::Result<Self> {
        let mut argv: Vec<*const c_char> = command.argv.iter().map(|arg| arg.as_ptr()).collect();
        argv.push(ptr::null());
        let mut exec_failed = b"fantermo-bridge: cannot execute ".to_vec();
        exec_failed.extend_from_slice(command.program.as_bytes());
        exec_failed.extend_from_slice(b"\r\n");

        let winsize = to_winsize(size);
        let mut master: c_int = -1;
        // SAFETY: `master` と `winsize` は有効な書き込み先と読み取り元。名前と termios は null で、
        // 既定のものを使う。
        let pid = unsafe { libc::forkpty(&mut master, ptr::null_mut(), ptr::null(), &winsize) };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }
        if pid == 0 {
            // SAFETY: fork したあとの子プロセス。割り当てをせず、fork の前に作った null で終わる
            // `argv` と、メッセージのバイト列だけを使う。標準エラー出力は pty の slave。
            unsafe {
                libc::execvp(command.program.as_ptr(), argv.as_ptr());
                libc::write(STDERR, exec_failed.as_ptr().cast(), exec_failed.len());
                libc::_exit(EXIT_CODE_EXEC_FAILED);
            }
        }
        let child = Self {
            pid,
            master,
            background: OnceCell::new(),
        };
        child.set_nonblocking()?;
        Ok(child)
    }

    fn set_nonblocking(&self) -> io::Result<()> {
        // SAFETY: `master` は forkpty が開いた fd。
        let flags = unsafe { libc::fcntl(self.master, libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: 同上。
        if unsafe { libc::fcntl(self.master, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn resize(&self, size: WindowSize) {
        let winsize = to_winsize(size);
        // SAFETY: `master` は pty の master、`winsize` は有効な読み取り元。
        if unsafe { libc::ioctl(self.master, libc::TIOCSWINSZ, &winsize) } < 0 {
            log!("大きさを変えられません: {}", io::Error::last_os_error());
        }
    }

    /// 子プロセスのプロセスグループに SIGHUP を送る。forkpty の子はセッションのリーダーなので、
    /// プロセスグループの番号もセッションの番号も pid と同じ。初めて送る前に、背景のジョブを記録する。
    fn hang_up(&self) {
        self.background.get_or_init(|| self.record_background());
        // SAFETY: 引数は数だけ。
        if unsafe { libc::kill(-self.pid, libc::SIGHUP) } < 0 {
            log!("SIGHUP を送れません: {}", io::Error::last_os_error());
        }
    }

    /// SIGHUP を送ったことがある。
    fn has_hung_up(&self) -> bool {
        self.background.get().is_some()
    }

    /// pty の前面のプロセスグループ。分からなければ負の数。
    fn foreground(&self) -> pid_t {
        // SAFETY: `master` は pty の master。
        unsafe { libc::tcgetpgrp(self.master) }
    }

    /// いま動いている背景のジョブのプロセスグループ（[`background_groups`]）を読む。`/proc` を読めなければ `None`。
    fn record_background(&self) -> Option<HashSet<pid_t>> {
        let processes = match read_processes() {
            Ok(processes) => processes,
            Err(error) => {
                log!(
                    "/proc を読めないので、シェルが終わったあとは前面とシェルのプロセスグループにだけ SIGHUP を送り直します: {error}"
                );
                return None;
            }
        };
        let clock = BootClock::read();
        if clock.is_none() {
            log!(
                "プロセスが起動してからの時間が分からないので、前面でもシェルのものでもないプロセスグループをすべて背景のジョブとみなします"
            );
        }
        let background = background_groups(&processes, self.pid, self.foreground(), clock.as_ref());
        let mut listed: Vec<pid_t> = background.iter().copied().collect();
        listed.sort_unstable();
        // 書き方はブリッジのテスト（`tests/protocol.rs` の `BACKGROUND_LOG`）が読む。
        log!("背景のジョブとして送らないプロセスグループ: {listed:?}");
        Some(background)
    }

    /// シェルが終わったあと、セッションに残ったプロセスのうち、閉じ始めたときの背景のジョブにいないもののプロセス
    /// グループに SIGHUP を送る。一つでも送れたら真。
    ///
    /// シェルが SIGHUP を受け取ったときに起動の途中だったコマンドには、Cygwin ではシェルからの SIGHUP が届かないことが
    /// ある。そのコマンドは、シェルのプロセスグループにいるか、自分のプロセスグループに移ったまま残る。Cygwin は
    /// セッションのリーダーが終わっても前面のプロセスグループに SIGHUP を送らず、リーダーが終わったあとは
    /// `tcsetpgrp` が失敗するので、移ったコマンドは前面にもなれない。
    fn hang_up_remaining(&self) -> bool {
        let groups = match self.background.get() {
            Some(Some(background)) => match read_processes() {
                Ok(processes) => resend_targets(&processes, self.pid, background),
                Err(error) => {
                    log!("/proc を読めません: {error}");
                    fallback_targets(self.pid, self.foreground())
                }
            },
            _ => fallback_targets(self.pid, self.foreground()),
        };
        let mut sent = false;
        for group in groups {
            // SAFETY: 引数は数だけ。
            if unsafe { libc::kill(-group, libc::SIGHUP) } == 0 {
                sent = true;
                continue;
            }
            // 読んだあとに終わったグループは ESRCH になる。
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                log!("プロセスグループ {group} に SIGHUP を送れません: {error}");
            }
        }
        sent
    }

    /// 終わっていれば終わり方を返す。`block` なら終わるまで待つ。
    fn wait(&self, block: bool) -> Option<ChildExit> {
        let options = if block { 0 } else { libc::WNOHANG };
        let mut status: c_int = 0;
        loop {
            // SAFETY: `pid` は自分の子、`status` は有効な書き込み先。
            let result = unsafe { libc::waitpid(self.pid, &mut status, options) };
            if result == self.pid {
                return exit_of(status);
            }
            if result == 0 {
                return None;
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            log!("waitpid: {error}");
            // 待てない子は終わったものとして扱う。
            return Some(ChildExit::Code(-1));
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: `master` は自分が開いた fd で、ほかに閉じるところはない。
        unsafe { libc::close(self.master) };
    }
}

fn exit_of(status: c_int) -> Option<ChildExit> {
    if libc::WIFEXITED(status) {
        Some(ChildExit::Code(libc::WEXITSTATUS(status)))
    } else if libc::WIFSIGNALED(status) {
        Some(ChildExit::Signal(libc::WTERMSIG(status)))
    } else {
        // 止まった・再開しただけ。
        None
    }
}

fn to_winsize(size: WindowSize) -> libc::winsize {
    libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: size.width_px,
        ws_ypixel: size.height_px,
    }
}

/// pty へ書くスレッド。
///
/// 子プロセスが読まないと、書き込みはいつまでも終わらない。書き込みは別のスレッドに任せ、中継の
/// スレッドは pty の出力と標準入力を読み続ける。止まっても、終了の依頼や標準入力が閉じたことで
/// 子プロセスを終わらせられる。
struct PtyWriter {
    sender: mpsc::Sender<Vec<u8>>,
    /// 渡したがまだ書き終えていないバイト数。
    queued: Arc<AtomicUsize>,
}

impl PtyWriter {
    fn spawn(master: c_int) -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        let queued = Arc::new(AtomicUsize::new(0));
        let written = Arc::clone(&queued);
        let pacer = InputPacer::new(master);
        std::thread::Builder::new()
            .name("pty-writer".into())
            .spawn(move || {
                let mut failed = false;
                for bytes in receiver {
                    if !failed && let Err(error) = pacer.write_all(master, &bytes) {
                        // 子プロセスが終わって pty が閉じたときなど。終わりは読む側で拾う。
                        log!("pty へ書けません: {error}");
                        failed = true;
                    }
                    written.fetch_sub(bytes.len(), Ordering::Relaxed);
                }
            })?;
        Ok(Self { sender, queued })
    }

    fn write(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        self.queued.fetch_add(bytes.len(), Ordering::Relaxed);
        if self.sender.send(bytes).is_err() {
            log!("pty へ書くスレッドが終わっています");
        }
    }

    fn queued(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }
}

/// pty の入力を、子プロセスが読むのに合わせて渡す。
///
/// Cygwin の pty は master から子プロセスへの入力を 128 KiB の pipe で渡す。`PIPE_NOWAIT` なので、
/// pipe が一杯だと受け取れない分を master の側に溜め、書き込みはそれも渡せたものとして返す。溜まった
/// 分は canonical モードでは次の改行が書かれるまで渡らず、raw では順序が崩れることがあり、echo が
/// 有効なら echo の pipe が詰まって書き込みが返らなくなる。また、書き込みの間は子プロセスの読み取りが
/// 待たされる。
///
/// そこで、[`PIECE_LEN`](Self::PIECE_LEN) ずつに分け、pipe が空になってから次を書く。pipe の中身は、
/// slave の `FIONREAD` で分かる（値は先頭のメッセージの残りで、溜まった総量ではない）。
#[cfg(target_os = "cygwin")]
struct InputPacer {
    /// pty の slave。開けなければ `None` で、[`NO_SLAVE_WAIT`](Self::NO_SLAVE_WAIT) だけ間を空ける。
    slave: Option<c_int>,
}

#[cfg(target_os = "cygwin")]
impl InputPacer {
    /// 一度に書く大きさ。大きいほど子プロセスの読み取りを長く待たせる。これより小さいと遅くなり
    /// （1 KiB で 33 KB/s、4 KiB で 50 KB/s）、16 KiB では echo が有効なときに止まることがあった。
    const PIECE_LEN: usize = 4 * 1024;
    /// pipe が空になるのを待つ間隔。
    const DRAIN_POLL: std::time::Duration = std::time::Duration::from_millis(1);
    /// slave を見られないときに書き込みの間に空ける時間。
    const NO_SLAVE_WAIT: std::time::Duration = std::time::Duration::from_millis(1);

    fn new(master: c_int) -> Self {
        let slave = open_slave(master);
        if let Err(error) = &slave {
            log!("pty の slave を開けないので、入力の間を空けるだけにします: {error}");
        }
        Self { slave: slave.ok() }
    }

    fn write_all(&self, master: c_int, bytes: &[u8]) -> io::Result<()> {
        for piece in bytes.chunks(Self::PIECE_LEN) {
            self.wait_for_drain();
            write_all_pty(master, piece)?;
        }
        Ok(())
    }

    /// 子プロセスが pipe を読み終えるまで待つ。読まない子プロセスにはいつまでも待つ。
    fn wait_for_drain(&self) {
        let Some(slave) = self.slave else {
            std::thread::sleep(Self::NO_SLAVE_WAIT);
            return;
        };
        loop {
            let mut pending: c_int = 0;
            // SAFETY: `slave` は自分が開いた fd、`pending` は有効な書き込み先。
            if unsafe { libc::ioctl(slave, libc::FIONREAD, &mut pending) } < 0 {
                log!(
                    "pty の入力の残りを調べられません: {}",
                    io::Error::last_os_error()
                );
                return;
            }
            if pending == 0 {
                return;
            }
            std::thread::sleep(Self::DRAIN_POLL);
        }
    }
}

/// pty の slave を、入力の残りを調べるために開く。端末としては使わないので、制御端末にはしない。
#[cfg(target_os = "cygwin")]
fn open_slave(master: c_int) -> io::Result<c_int> {
    let mut name = [0 as c_char; PTY_NAME_LEN];
    // SAFETY: `name` は渡した長さの有効な書き込み先。
    let error = unsafe { libc::ptsname_r(master, name.as_mut_ptr(), name.len()) };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error));
    }
    // SAFETY: `name` は ptsname_r が書いた NUL で終わる文字列。
    let slave = unsafe { libc::open(name.as_ptr(), libc::O_RDONLY | libc::O_NOCTTY) };
    if slave < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(slave)
}

/// pty の入力をそのまま書く。
#[cfg(not(target_os = "cygwin"))]
struct InputPacer;

#[cfg(not(target_os = "cygwin"))]
impl InputPacer {
    fn new(_master: c_int) -> Self {
        Self
    }

    fn write_all(&self, master: c_int, bytes: &[u8]) -> io::Result<()> {
        write_all_pty(master, bytes)
    }
}

/// pty の master へすべて書く。`O_NONBLOCK` で書けなければ、書けるようになるまで待つ。
fn write_all_pty(master: c_int, mut bytes: &[u8]) -> io::Result<()> {
    let mut wait = PollWait::new();
    while !bytes.is_empty() {
        // SAFETY: `bytes` は有効な読み取り元、`master` は開いている fd。
        let n = unsafe { libc::write(master, bytes.as_ptr().cast(), bytes.len()) };
        if n > 0 {
            bytes = &bytes[n as usize..];
            continue;
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::Interrupted => {}
            io::ErrorKind::WouldBlock => {
                let mut fds = [libc::pollfd {
                    fd: master,
                    events: libc::POLLOUT,
                    revents: 0,
                }];
                // 待てなくても、次の write で書けるかが分かる。
                _ = poll(&mut fds, &mut wait);
            }
            _ => return Err(error),
        }
    }
    Ok(())
}

/// `fds` を `poll` で見る。待ち方は `wait` が決める。準備のできた fd の数を返す。
///
/// Cygwin では時間切れ 0 で覗き、何もなければ自分で待つ（[`PollWait`]）。Cygwin の時間待ちのある `poll`・`select` は、
/// fd を覗く内部のスレッド（cygthread）を立てて待つ。そのスレッドは、他のプロセスにこのプロセスの `cmdline` などを
/// 読まれたときに Cygwin が立てる答えのためのスレッドと同じ枠を使い、その競合で `poll` が戻らなくなることがある。
/// 時間切れ 0 の `poll` はスレッドを立てずに覗くだけなので、この競合が起きない。止まる `read` を読むスレッドで使う形は
/// 取らない。Cygwin の止まる `read` は、Cygwin のものでないパイプ（fantermo から継いだ標準入力）や pty の master では
/// 中で覗き続けて CPU を使い、応答も遅くなる。
#[expect(
    clippy::disallowed_methods,
    reason = "poll を呼ぶのはここだけにし、Cygwin では時間切れ 0 にする"
)]
fn poll(fds: &mut [libc::pollfd], wait: &mut PollWait) -> io::Result<c_int> {
    // SAFETY: `fds` は要素の数だけの有効な pollfd。
    let ready = unsafe {
        libc::poll(
            fds.as_mut_ptr(),
            fds.len() as libc::nfds_t,
            wait.timeout_ms(),
        )
    };
    if ready < 0 {
        // 待つ前に読む（待つと errno が変わりうる）。
        return Err(io::Error::last_os_error());
    }
    wait.after_poll(ready);
    Ok(ready)
}

/// [`poll`] の待ち方。Cygwin では時間切れ 0 で覗き、何も起きなければ自分で待つ。待ちは、何も起きない回が
/// [`POLLS_PER_STEP`](Self::POLLS_PER_STEP) 回続くたびに 1 ms ずつ延ばし、[`MAX_WAIT_MS`](Self::MAX_WAIT_MS) で
/// 止める。何か起きたら 0 に戻す。刻みは、Cygwin の時間待ちのある `select` が中の覗くスレッド（`thread_pipe`）で
/// 使うものと同じにし、覗く間隔を時間待ちのある `poll` と揃える。
#[cfg(target_os = "cygwin")]
struct PollWait {
    /// 続けて何も起きなかった回数。待ちが上限に届いたら増やさない。
    idle_polls: u32,
}

#[cfg(target_os = "cygwin")]
impl PollWait {
    /// 待ちを 1 ms 延ばすまでの、何も起きない回数。
    const POLLS_PER_STEP: u32 = 8;
    /// 待ちの上限。何もしていないときは、この間隔で覗く。
    const MAX_WAIT_MS: u32 = 10;

    fn new() -> Self {
        Self { idle_polls: 0 }
    }

    fn timeout_ms(&self) -> c_int {
        0
    }

    /// `poll` が `ready` を返したあとに待つ。
    fn after_poll(&mut self, ready: c_int) {
        if ready > 0 {
            self.idle_polls = 0;
            return;
        }
        let wait_ms = (self.idle_polls / Self::POLLS_PER_STEP).min(Self::MAX_WAIT_MS);
        if wait_ms == 0 {
            std::thread::yield_now();
        } else {
            std::thread::sleep(std::time::Duration::from_millis(wait_ms.into()));
            if wait_ms == Self::MAX_WAIT_MS {
                return;
            }
        }
        self.idle_polls += 1;
    }
}

/// [`poll`] の待ち方。Cygwin のほかでは `poll` で [`POLL_INTERVAL_MS`] まで待つ。
#[cfg(not(target_os = "cygwin"))]
struct PollWait;

#[cfg(not(target_os = "cygwin"))]
impl PollWait {
    fn new() -> Self {
        Self
    }

    fn timeout_ms(&self) -> c_int {
        POLL_INTERVAL_MS
    }

    fn after_poll(&mut self, _ready: c_int) {}
}

/// 標準入力と pty の間の中継。
struct Relay {
    child: Child,
    decoder: Decoder,
    input: PtyWriter,
    buf: Vec<u8>,
    exit: Option<ChildExit>,
    master_closed: bool,
    poll_wait: PollWait,
}

/// 中継を続けるか、終わるか。
enum Flow {
    Continue,
    /// 終わる。値はブリッジの終了コード。
    Finish(i32),
}

impl Relay {
    fn new(child: Child) -> io::Result<Self> {
        let input = PtyWriter::spawn(child.master)?;
        Ok(Self {
            child,
            decoder: Decoder::new(),
            input,
            buf: vec![0; READ_BUF_LEN],
            exit: None,
            master_closed: false,
            poll_wait: PollWait::new(),
        })
    }

    fn run(mut self) -> i32 {
        loop {
            if let Flow::Finish(code) = self.step() {
                return code;
            }
        }
    }

    fn step(&mut self) -> Flow {
        if let flow @ Flow::Finish(_) = self.handle_messages() {
            return flow;
        }
        if self.master_closed || self.exit.is_some() {
            return Flow::Finish(self.finish());
        }

        let read_stdin = self.input.queued() < MAX_PENDING_INPUT;
        let mut fds = [
            libc::pollfd {
                fd: self.child.master,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: STDIN,
                events: if read_stdin { libc::POLLIN } else { 0 },
                revents: 0,
            },
        ];
        if let Err(error) = poll(&mut fds, &mut self.poll_wait) {
            if error.kind() == io::ErrorKind::Interrupted {
                return Flow::Continue;
            }
            log!("poll: {error}");
            self.child.hang_up();
            return Flow::Finish(EXIT_IO_ERROR);
        }

        if fds[0].revents != 0
            && let flow @ Flow::Finish(_) = self.read_master()
        {
            return flow;
        }
        if fds[1].revents != 0
            && let flow @ Flow::Finish(_) = self.read_stdin()
        {
            return flow;
        }
        if self.exit.is_none() {
            self.exit = self.child.wait(false);
        }
        Flow::Continue
    }

    /// 溜まった単位を、入力を溜めすぎない範囲で処理する。
    fn handle_messages(&mut self) -> Flow {
        while self.input.queued() < MAX_PENDING_INPUT {
            let message = match self.decoder.next_message::<ToBridge>() {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(error) => return self.protocol_error(&error.to_string()),
            };
            match message {
                ToBridge::Data(bytes) => self.input.write(bytes),
                ToBridge::Resize(size) => self.child.resize(size),
                ToBridge::Quit => self.child.hang_up(),
                ToBridge::Hello { .. } => {
                    return self.protocol_error("開始の通知を二度受信しました");
                }
            }
        }
        Flow::Continue
    }

    fn protocol_error(&mut self, message: &str) -> Flow {
        report_error(message);
        self.child.hang_up();
        hang_up_wait(&self.child);
        Flow::Finish(EXIT_PROTOCOL_ERROR)
    }

    /// pty の出力を読めるだけ読んで送る。
    fn read_master(&mut self) -> Flow {
        loop {
            // SAFETY: `buf` は有効な書き込み先、`master` は開いている fd。
            let n = unsafe {
                libc::read(
                    self.child.master,
                    self.buf.as_mut_ptr().cast(),
                    self.buf.len(),
                )
            };
            if n > 0 {
                let data = FromBridge::Data(self.buf[..n as usize].to_vec());
                if let Err(error) = send(&data) {
                    return self.peer_gone(&error);
                }
                continue;
            }
            if n == 0 {
                self.master_closed = true;
                return Flow::Continue;
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => return Flow::Continue,
                // Linux は slave がすべて閉じると EIO を返す。
                _ => {
                    if error.raw_os_error() != Some(libc::EIO) {
                        log!("pty から読めません: {error}");
                    }
                    self.master_closed = true;
                    return Flow::Continue;
                }
            }
        }
    }

    fn read_stdin(&mut self) -> Flow {
        match read_fd(STDIN, &mut self.buf) {
            Ok(0) => {
                log!("標準入力が閉じたので子プロセスを終わらせます");
                self.child.hang_up();
                hang_up_wait(&self.child);
                Flow::Finish(EXIT_OK)
            }
            Ok(n) => {
                self.decoder.push(&self.buf[..n]);
                Flow::Continue
            }
            Err(error) => {
                log!("標準入力から読めません: {error}");
                self.child.hang_up();
                hang_up_wait(&self.child);
                Flow::Finish(EXIT_IO_ERROR)
            }
        }
    }

    fn peer_gone(&mut self, error: &io::Error) -> Flow {
        log!("標準出力へ書けないので子プロセスを終わらせます: {error}");
        self.child.hang_up();
        hang_up_wait(&self.child);
        Flow::Finish(EXIT_IO_ERROR)
    }

    /// 子プロセスが終わったか pty が閉じたあと、残りの出力と終了の単位を送る。
    fn finish(&mut self) -> i32 {
        if !self.master_closed
            && let Flow::Finish(code) = self.read_master()
        {
            return code;
        }
        let exit = match self.exit {
            Some(exit) => exit,
            None => loop {
                if let Some(exit) = self.child.wait(true) {
                    break exit;
                }
            },
        };
        log!("子プロセスが終わった: {exit:?}");
        let sent = send(&FromBridge::Exit(exit));
        // 終了の依頼で SIGHUP を送ったあと、標準入力が閉じる前にシェルが終わったときも、残ったプロセスへ送り直す。
        if self.child.has_hung_up() {
            hang_up_remaining_wait(&self.child, HANGUP_WAIT_MS);
        }
        if let Err(error) = sent {
            log!("終了の単位を送れません: {error}");
            return EXIT_IO_ERROR;
        }
        EXIT_OK
    }
}

/// SIGHUP を送った子プロセスと、そのあとに残ったプロセスが終わるのを、合わせて [`HANGUP_WAIT_MS`] まで待つ。
fn hang_up_wait(child: &Child) {
    let mut waited = 0;
    while waited < HANGUP_WAIT_MS {
        if child.wait(false).is_some() {
            hang_up_remaining_wait(child, HANGUP_WAIT_MS - waited);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(HANGUP_WAIT_STEP_MS.into()));
        waited += HANGUP_WAIT_STEP_MS;
    }
    log!("子プロセスが終わらないまま終わります（pid {}）", child.pid);
}

/// シェルが終わったあとに残ったプロセスへ（[`Child::hang_up_remaining`]）、`limit_ms` を上限に SIGHUP を送り直す。
/// 起動の途中のプロセスには届かないことがあるので間を空けて繰り返し、[`EMPTY_RESENDS_TO_STOP`] 回続けて送る先が
/// なければやめる。
fn hang_up_remaining_wait(child: &Child, limit_ms: u32) {
    let mut waited = 0;
    let mut empty = 0;
    while waited < limit_ms {
        if child.hang_up_remaining() {
            empty = 0;
        } else {
            empty += 1;
            if empty >= EMPTY_RESENDS_TO_STOP {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(
            RESEND_HANGUP_INTERVAL_MS.into(),
        ));
        waited += RESEND_HANGUP_INTERVAL_MS;
    }
}

/// `/proc/<pid>/stat` から読んだ、SIGHUP を送る先を決めるのに使う値。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcessStat {
    pgid: pid_t,
    sid: pid_t,
    /// 起動の時刻。OS が起動してからの時間で、単位は `sysconf(_SC_CLK_TCK)`。
    start_ticks: u64,
}

/// `/proc/<pid>/stat` の中身を読む。2 つめの項目（コマンドの名前）は括弧で囲まれ、空白や括弧を含みうるので、
/// 残りは最後の `)` の後ろから数える。
fn parse_stat(text: &[u8]) -> Option<ProcessStat> {
    /// `)` の後ろの、空白で区切った項目の位置（3 つめの項目 `state` が 0）。
    const PGID_FIELD: usize = 2;
    const SID_FIELD: usize = 3;
    const START_TIME_FIELD: usize = 19;

    let open = text.iter().position(|&byte| byte == b'(')?;
    let close = text.iter().rposition(|&byte| byte == b')')?;
    if close < open {
        return None;
    }
    let rest = std::str::from_utf8(&text[close + 1..]).ok()?;
    let fields: Vec<&str> = rest.split_ascii_whitespace().collect();
    Some(ProcessStat {
        pgid: fields.get(PGID_FIELD)?.parse().ok()?,
        sid: fields.get(SID_FIELD)?.parse().ok()?,
        start_ticks: fields.get(START_TIME_FIELD)?.parse().ok()?,
    })
}

/// `/proc` のプロセスをすべて読む。読む間に終わったものと、読めない中身のものは飛ばす。
fn read_processes() -> io::Result<Vec<ProcessStat>> {
    let mut processes = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name();
        let name = name.as_bytes();
        if name.is_empty() || !name.iter().all(u8::is_ascii_digit) {
            continue;
        }
        let Ok(text) = std::fs::read(entry.path().join("stat")) else {
            continue;
        };
        if let Some(stat) = parse_stat(&text) {
            processes.push(stat);
        }
    }
    Ok(processes)
}

/// プロセスが起動してからの時間を求めるための時計。`/proc/<pid>/stat` の起動の時刻と `/proc/uptime` は、Cygwin でも
/// Linux でも OS が起動してからの時間で、前者の単位は `sysconf(_SC_CLK_TCK)`（Cygwin は 1000、Linux は多くが 100）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BootClock {
    /// OS が起動してからの時間（ミリ秒）。
    uptime_ms: u64,
    ticks_per_second: u64,
}

impl BootClock {
    /// いまの時計。プロセスを読んだあとに読む（読む間に起動したプロセスが、起動の時刻で時計を超えないように）。
    /// 読めなければ `None`。
    fn read() -> Option<Self> {
        // SAFETY: 引数は数だけ。
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let ticks_per_second = u64::try_from(ticks).ok().filter(|&ticks| ticks > 0)?;
        let uptime = std::fs::read("/proc/uptime").ok()?;
        Some(Self {
            uptime_ms: parse_uptime(&uptime)?,
            ticks_per_second,
        })
    }

    /// 起動の時刻が `start_ticks` のプロセスが起動してからの時間（ミリ秒）。時計が合わなければ `None`。
    fn age_ms(&self, start_ticks: u64) -> Option<u64> {
        let start_ms = start_ticks.checked_mul(MILLIS_PER_SECOND)? / self.ticks_per_second;
        if start_ms > self.uptime_ms.saturating_add(START_TIME_TOLERANCE_MS) {
            return None;
        }
        Some(self.uptime_ms.saturating_sub(start_ms))
    }
}

/// `/proc/uptime` の最初の数（秒、小数あり）をミリ秒にする。
fn parse_uptime(text: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(text).ok()?;
    let seconds = text.split_ascii_whitespace().next()?;
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let whole: u64 = whole.parse().ok()?;
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut millis = 0;
    let mut scale = MILLIS_PER_SECOND / 10;
    for digit in fraction.bytes().take(UPTIME_FRACTION_DIGITS) {
        millis += u64::from(digit - b'0') * scale;
        scale /= 10;
    }
    whole.checked_mul(MILLIS_PER_SECOND)?.checked_add(millis)
}

/// 閉じ始めたときの背景のジョブのプロセスグループ。セッション `shell`（シェルの pid）のプロセスグループのうち、
/// シェルのものでも前面 `foreground` でもなく、起動して [`BACKGROUND_JOB_MIN_AGE_MS`] を過ぎたプロセスのいるもの。
/// `clock` がなければ（起動からの時間が分からなければ）、起動の時刻によらず背景のジョブとみなす。
fn background_groups(
    processes: &[ProcessStat],
    shell: pid_t,
    foreground: pid_t,
    clock: Option<&BootClock>,
) -> HashSet<pid_t> {
    processes
        .iter()
        .filter(|process| {
            process.sid == shell && process.pgid != shell && process.pgid != foreground
        })
        .filter(|process| {
            clock
                .and_then(|clock| clock.age_ms(process.start_ticks))
                .is_none_or(|age| age >= BACKGROUND_JOB_MIN_AGE_MS)
        })
        .map(|process| process.pgid)
        .collect()
}

/// シェルが終わったあとに SIGHUP を送り直すプロセスグループ。セッション `shell` のプロセスのうち、閉じ始めたときの
/// 背景のジョブ `background` にいないもののグループ。並びは小さい順で、重ならない。
fn resend_targets(
    processes: &[ProcessStat],
    shell: pid_t,
    background: &HashSet<pid_t>,
) -> Vec<pid_t> {
    let mut groups: Vec<pid_t> = processes
        .iter()
        .filter(|process| {
            // 0 と 1 は kill(2) に渡すと自分のグループや全プロセスを指すので、送らない。
            process.sid == shell && process.pgid > 1 && !background.contains(&process.pgid)
        })
        .map(|process| process.pgid)
        .collect();
    groups.sort_unstable();
    groups.dedup();
    groups
}

/// `/proc` を読めないときに SIGHUP を送り直すプロセスグループ。シェルのものと、前面 `foreground` のもの。
fn fallback_targets(shell: pid_t, foreground: pid_t) -> Vec<pid_t> {
    let mut groups = vec![shell];
    if foreground > 1 && foreground != shell {
        groups.push(foreground);
    }
    groups
}

/// 標準入力から単位を一つ受け取るまで待つ。標準入力が単位の切れ目で閉じたら `None`。
fn receive_blocking() -> Result<Option<ToBridge>, String> {
    protocol::read_message(&mut Fd(STDIN)).map_err(|error| match error.kind() {
        io::ErrorKind::UnexpectedEof => "標準入力がデータの途中で閉じました".to_string(),
        io::ErrorKind::InvalidData => error.to_string(),
        _ => format!("標準入力から読み取れません: {error}"),
    })
}

/// エラーをログに残し、fantermo にも送る。
fn report_error(message: &str) {
    log!("{message}");
    if let Err(error) = send(&FromBridge::Error(message.to_string())) {
        log!("エラーの単位を送れません: {error}");
    }
}

/// 単位を標準出力へ書く。
fn send(message: &FromBridge) -> io::Result<()> {
    protocol::write_message(&mut Fd(STDOUT), message)
}

/// ファイル記述子を `Read` と `Write` として扱う。閉じない。
struct Fd(c_int);

impl io::Read for Fd {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        read_fd(self.0, buf)
    }
}

impl io::Write for Fd {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            // SAFETY: `bytes` は有効な読み取り元。
            let n = unsafe { libc::write(self.0, bytes.as_ptr().cast(), bytes.len()) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn read_fd(fd: c_int, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        // SAFETY: `buf` は有効な書き込み先。
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n >= 0 {
            return Ok(n as usize);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(OsString::from))
    }

    #[test]
    fn options() {
        assert_eq!(parse(&[]), Ok(Options::default()));
        assert_eq!(
            parse(&["--cwd", "/tmp", "--", "zsh", "-l"]),
            Ok(Options {
                cwd: Some("/tmp".into()),
                command: vec!["zsh".into(), "-l".into()],
            })
        );
        assert_eq!(
            parse(&["zsh", "--cwd", "x"]),
            Ok(Options {
                cwd: None,
                command: vec!["zsh".into(), "--cwd".into(), "x".into()],
            })
        );
        assert_eq!(
            parse(&["--", "--cwd"]),
            Ok(Options {
                cwd: None,
                command: vec!["--cwd".into()],
            })
        );
        assert!(parse(&["--cwd"]).is_err());
        assert!(parse(&["-x"]).is_err());
    }

    #[test]
    fn stat_is_read_after_the_last_parenthesis() {
        // Cygwin の `sleep` の実物。
        let cygwin = b"1216 (sleep) S 1215 1216 1215 34816 -1 0 2668 2668 0 0 0 0 0 0 20 0 7 0 1074395539 6389760 2562 1413120\n";
        assert_eq!(
            parse_stat(cygwin),
            Some(ProcessStat {
                pgid: 1216,
                sid: 1215,
                start_ticks: 1_074_395_539,
            })
        );
        // Linux（WSL）の `cat` の実物。項目は 52 ある。
        let linux = b"60911 (cat) R 60910 60910 60910 34820 60910 4194304 69 0 1 0 0 0 0 0 20 0 1 0 103930385 4104192 478 18446744073709551615 101504974422016 101504974439601 140734421490192 0 0 0 0 0 0 0 0 0 17 6 0 0 0 0 0 101504974453392 101504974454888 101505217716224 140734421493028 140734421493048 140734421493048 140734421495787 0\n";
        assert_eq!(
            parse_stat(linux),
            Some(ProcessStat {
                pgid: 60910,
                sid: 60910,
                start_ticks: 103_930_385,
            })
        );
        // コマンドの名前に空白と括弧があっても、最後の `)` の後ろから数える。
        let odd =
            b"42 (a b) (c) d) S 1 40 30 34816 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 98765 1000 10 0";
        assert_eq!(
            parse_stat(odd),
            Some(ProcessStat {
                pgid: 40,
                sid: 30,
                start_ticks: 98_765,
            })
        );
        assert_eq!(parse_stat(b"42 (sleep) S 1 40 30"), None);
        assert_eq!(parse_stat(b"42 sleep S 1 40 30"), None);
        assert_eq!(
            parse_stat(b"42 (sleep) S 1 x 30 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 5"),
            None
        );
        assert_eq!(parse_stat(b") 42 ("), None);
    }

    #[test]
    fn uptime_is_read_in_milliseconds() {
        assert_eq!(
            parse_uptime(b"1075391.28 1014023.69\n"),
            Some(1_075_391_280)
        );
        assert_eq!(parse_uptime(b"12 3"), Some(12_000));
        assert_eq!(parse_uptime(b"1.2345 0"), Some(1_234));
        assert_eq!(parse_uptime(b"1.5"), Some(1_500));
        assert_eq!(parse_uptime(b"x.5"), None);
        assert_eq!(parse_uptime(b"1.x"), None);
        assert_eq!(parse_uptime(b""), None);
    }

    #[test]
    fn age_follows_the_ticks_per_second() {
        let cygwin = BootClock {
            uptime_ms: 10_000,
            ticks_per_second: 1000,
        };
        let linux = BootClock {
            uptime_ms: 10_000,
            ticks_per_second: 100,
        };
        assert_eq!(cygwin.age_ms(9_600), Some(400));
        assert_eq!(linux.age_ms(960), Some(400));
        // 時計の刻みより後に起動したものは 0。
        assert_eq!(cygwin.age_ms(10_005), Some(0));
        // 時計より大きく後なら、時計が合わないので分からない。
        assert_eq!(cygwin.age_ms(10_000 + START_TIME_TOLERANCE_MS + 1), None);
        assert_eq!(linux.age_ms(u64::MAX), None);
    }

    const SHELL: pid_t = 100;
    const FOREGROUND: pid_t = 200;

    /// 起動して `age_ms` のプロセス。時計は [`clock`]。
    fn process(pgid: pid_t, sid: pid_t, age_ms: u64) -> ProcessStat {
        ProcessStat {
            pgid,
            sid,
            start_ticks: clock().uptime_ms - age_ms,
        }
    }

    fn clock() -> BootClock {
        BootClock {
            uptime_ms: 1_000_000,
            ticks_per_second: 1000,
        }
    }

    fn groups(groups: &[pid_t]) -> HashSet<pid_t> {
        groups.iter().copied().collect()
    }

    #[test]
    fn background_jobs_are_groups_with_a_settled_process() {
        let old = BACKGROUND_JOB_MIN_AGE_MS;
        let processes = [
            process(SHELL, SHELL, 10_000),
            process(SHELL, SHELL, 10_000),
            process(FOREGROUND, SHELL, 10_000),
            // しばらく動いている背景のジョブ。境目ちょうども含める。
            process(300, SHELL, old),
            process(310, SHELL, 60_000),
            // 自分のグループに移ったが前面になる前のコマンドでありうる。
            process(400, SHELL, old - 1),
            // 起動して間もないプロセスがいても、しばらく動いているプロセスのいるグループは背景のジョブ。
            process(500, SHELL, old - 1),
            process(500, SHELL, old + 1),
            // 別のセッション。
            process(600, 600, 60_000),
        ];
        assert_eq!(
            background_groups(&processes, SHELL, FOREGROUND, Some(&clock())),
            groups(&[300, 310, 500])
        );
        // 起動からの時間が分からなければ、起動の時刻によらない。
        assert_eq!(
            background_groups(&processes, SHELL, FOREGROUND, None),
            groups(&[300, 310, 400, 500])
        );
        // 前面が分からなくても、シェルのグループは含めない。
        assert_eq!(
            background_groups(&processes, SHELL, -1, Some(&clock())),
            groups(&[FOREGROUND, 300, 310, 500])
        );
        // 時計が合わないプロセスは、起動の時刻が分からないものとして背景のジョブにする。
        let future = ProcessStat {
            start_ticks: clock().uptime_ms + START_TIME_TOLERANCE_MS + 1,
            ..process(700, SHELL, 0)
        };
        assert_eq!(
            background_groups(&[future], SHELL, FOREGROUND, Some(&clock())),
            groups(&[700])
        );
    }

    #[test]
    fn resent_groups_are_the_session_outside_the_background_jobs() {
        let processes = [
            process(SHELL, SHELL, 0),
            process(SHELL, SHELL, 0),
            process(FOREGROUND, SHELL, 0),
            process(300, SHELL, 10_000),
            process(400, SHELL, 0),
            process(600, 600, 0),
        ];
        assert_eq!(
            resend_targets(&processes, SHELL, &groups(&[300])),
            [SHELL, FOREGROUND, 400]
        );
        assert_eq!(
            resend_targets(&processes, SHELL, &groups(&[SHELL, FOREGROUND, 300, 400])),
            []
        );
        assert_eq!(resend_targets(&[], SHELL, &groups(&[])), []);
        // kill(2) で自分のグループや全プロセスを指す番号と、負の番号には送らない。
        let odd = [
            process(0, SHELL, 0),
            process(1, SHELL, 0),
            process(-5, SHELL, 0),
            process(400, SHELL, 0),
        ];
        assert_eq!(resend_targets(&odd, SHELL, &groups(&[])), [400]);
    }

    #[test]
    fn without_proc_the_shell_and_the_foreground_are_resent() {
        assert_eq!(fallback_targets(SHELL, FOREGROUND), [SHELL, FOREGROUND]);
        assert_eq!(fallback_targets(SHELL, SHELL), [SHELL]);
        assert_eq!(fallback_targets(SHELL, -1), [SHELL]);
        assert_eq!(fallback_targets(SHELL, 0), [SHELL]);
        assert_eq!(fallback_targets(SHELL, 1), [SHELL]);
    }
}
