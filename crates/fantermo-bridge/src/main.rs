//! fantermo とシェルの間を中継するブリッジ。Cygwin と Linux（WSL）で動く。
//!
//! `fantermo-bridge [--cwd <パス>] [--] [<コマンド> [引数…]]`。コマンドがなければ利用者のシェル
//! （`$SHELL`、なければ passwd のシェル、それもなければ `/bin/sh`）をログインシェルとして起動する。標準入出力は fantermo との間の
//! プロトコル（`fantermo-bridge-protocol`）、標準エラー出力はログ。
//!
//! 開始の通知を交換し、最初のリサイズを受け取ってから、その大きさの pty で子プロセスを起動する。
//! そのあとは標準入力と pty を `poll` で見て中継し、子プロセスが終わったら終了の単位を送って終わる。
//! 環境変数（`TERM` など）は fantermo が渡したものをそのまま子プロセスに引き継ぐ。

use std::ffi::{CStr, CString, OsString};
use std::io;
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
/// 子プロセスが終わったかを確かめる間隔。
const POLL_INTERVAL_MS: c_int = 200;
/// 標準入力が閉じて子プロセスに SIGHUP を送ったあと、終わるのを待つ上限。
const HANGUP_WAIT_MS: u32 = 2000;
/// 上の待ちの一回の長さ。
const HANGUP_WAIT_STEP_MS: u32 = 50;
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
        eprintln!("fantermo-bridge: {}", format_args!($($arg)*))
    };
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

    let mut decoder = Decoder::new();
    match receive_blocking(&mut decoder) {
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
    let size = match receive_blocking(&mut decoder) {
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
    match Relay::new(child, decoder) {
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
        let child = Self { pid, master };
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
    /// プロセスグループの番号は pid と同じ。
    fn hang_up(&self) {
        // SAFETY: 引数は数だけ。
        if unsafe { libc::kill(-self.pid, libc::SIGHUP) } < 0 {
            log!("SIGHUP を送れません: {}", io::Error::last_os_error());
        }
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
                // SAFETY: `fds` は要素の数だけの有効な pollfd。
                unsafe { libc::poll(fds.as_mut_ptr(), 1, POLL_INTERVAL_MS) };
            }
            _ => return Err(error),
        }
    }
    Ok(())
}

/// 標準入力と pty の間の中継。
struct Relay {
    child: Child,
    decoder: Decoder,
    input: PtyWriter,
    buf: Vec<u8>,
    exit: Option<ChildExit>,
    master_closed: bool,
}

/// 中継を続けるか、終わるか。
enum Flow {
    Continue,
    /// 終わる。値はブリッジの終了コード。
    Finish(i32),
}

impl Relay {
    fn new(child: Child, decoder: Decoder) -> io::Result<Self> {
        let input = PtyWriter::spawn(child.master)?;
        Ok(Self {
            child,
            decoder,
            input,
            buf: vec![0; READ_BUF_LEN],
            exit: None,
            master_closed: false,
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
        // SAFETY: `fds` は要素の数だけの有効な pollfd。
        let ready = unsafe {
            libc::poll(
                fds.as_mut_ptr(),
                fds.len() as libc::nfds_t,
                POLL_INTERVAL_MS,
            )
        };
        if ready < 0 {
            let error = io::Error::last_os_error();
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
        if let Err(error) = send(&FromBridge::Exit(exit)) {
            log!("終了の単位を送れません: {error}");
            return EXIT_IO_ERROR;
        }
        EXIT_OK
    }
}

/// SIGHUP を送った子プロセスが終わるのを、[`HANGUP_WAIT_MS`] まで待つ。
fn hang_up_wait(child: &Child) {
    let mut waited = 0;
    while waited < HANGUP_WAIT_MS {
        if child.wait(false).is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(HANGUP_WAIT_STEP_MS.into()));
        waited += HANGUP_WAIT_STEP_MS;
    }
    log!("子プロセスが終わらないまま終わります（pid {}）", child.pid);
}

/// 標準入力から単位を一つ受け取るまで待つ。標準入力が単位の切れ目で閉じたら `None`。
fn receive_blocking(decoder: &mut Decoder) -> Result<Option<ToBridge>, String> {
    let mut buf = vec![0; READ_BUF_LEN];
    loop {
        if let Some(message) = decoder.next_message().map_err(|error| error.to_string())? {
            return Ok(Some(message));
        }
        let n = read_fd(STDIN, &mut buf)
            .map_err(|error| format!("標準入力から読み取れません: {error}"))?;
        if n == 0 {
            if decoder.pending_len() > 0 {
                return Err("標準入力がデータの途中で閉じました".to_string());
            }
            return Ok(None);
        }
        decoder.push(&buf[..n]);
    }
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
    let bytes = protocol::encode(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    write_all_fd(STDOUT, &bytes)
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

fn write_all_fd(fd: c_int, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        // SAFETY: `bytes` は有効な読み取り元。
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n > 0 {
            bytes = &bytes[n as usize..];
            continue;
        }
        let error = io::Error::last_os_error();
        if n < 0 && error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(error);
    }
    Ok(())
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
}
