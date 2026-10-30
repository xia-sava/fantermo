//! ブリッジのバックエンド。Cygwin と WSL の側で pty とシェルを持つ補助プログラム（`fantermo-bridge`）を
//! 起動し、標準入出力のパイプでプロトコル（`fantermo-bridge-protocol`）の単位をやり取りする。
//!
//! 書き込みは書くためのスレッドに任せ、ウィンドウのスレッドは待たない。出力は読むためのスレッドで単位に
//! 分け、データと子プロセスの終了をチャネルで渡す。標準エラー出力はログへ流す。起動できないときや
//! ブリッジがエラーで終わったときは、その理由を赤い文字で端末に出し、タブは開いたままにする。

use std::{
    env,
    ffi::OsString,
    io::{self, BufRead, BufReader, Read, Write},
    os::windows::{
        io::{AsRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use fantermo_bridge_protocol::{
    self as protocol, ChildExit, Decoder, FromBridge, MAX_PAYLOAD_LEN, ToBridge, WindowSize,
};
use fantermo_core::backend::{Backend, ExitCode, GridSize, SpawnOptions, Wake};
use windows::Win32::{
    Foundation::{HANDLE, WAIT_OBJECT_0},
    System::Threading::{
        CREATE_NO_WINDOW, GetExitCodeProcess, INFINITE, TerminateProcess, WaitForSingleObject,
    },
};

/// Cygwin 向けのブリッジの実行ファイルの名前と、開発中にビルドしたものを探すときのターゲット。
const CYGWIN_BRIDGE: (&str, &str) = ("fantermo-bridge.exe", "x86_64-pc-cygwin");
/// Linux（WSL）向けのブリッジの実行ファイルの名前と、開発中にビルドしたものを探すときのターゲット。
const LINUX_BRIDGE: (&str, &str) = ("fantermo-bridge", "x86_64-unknown-linux-musl");

/// `cygwin_root` を書いていないときに、この順に探す Cygwin のフォルダ。
const CYGWIN_ROOTS: [&str; 2] = [r"C:\cygwin64", r"C:\cygwin"];
/// Cygwin のフォルダにあるはずの DLL（`bin` の下）。
const CYGWIN_DLL: &str = "cygwin1.dll";

/// Cygwin のブリッジに、相手の側のコマンドラインを渡す環境変数。Cygwin のプログラムはコマンドラインを
/// 独自の決まりで引数に分ける（`\` の扱いが引数の形で変わる）ので、利用者の書いた文字列は引数に入れない。
const COMMAND_VARIABLE: &str = "FANTERMO_BRIDGE_COMMAND";
/// Cygwin のブリッジに起動させるシェルのスクリプト。[`COMMAND_VARIABLE`] を消してから、その中身
/// （[`shell_command`] にしたもの）を `/bin/sh -c` で実行する。
const CYGWIN_COMMAND_SCRIPT: &str =
    "c=$FANTERMO_BRIDGE_COMMAND; unset FANTERMO_BRIDGE_COMMAND; exec /bin/sh -c \"$c\"";

/// Cygwin のログインシェルの設定に、作業フォルダのまま始めるよう伝える環境変数（chere と同じ）。
const CHERE_INVOKING: &str = "CHERE_INVOKING";

/// WSL のブリッジの起動に使うプログラム。
const WSL_EXE: &str = "wsl.exe";

/// 破棄したあと、ブリッジが自分で終わるのを待つ時間。過ぎたら止める。
const QUIT_TIMEOUT: Duration = Duration::from_secs(5);
/// 出力のパイプから一度に読む大きさ。
const READ_BUFFER_SIZE: usize = 64 * 1024;
/// ブリッジが名乗る前に届いたバイト列を、エラーの文に添えるために覚えておく上限。
const RAW_OUTPUT_LIMIT: usize = 4096;
/// シグナルで終わった子プロセスの終了コードの基準（シェルと同じく 128 + シグナルの番号）。
const SIGNAL_EXIT_BASE: u32 = 128;

/// ブリッジでつなぐ先。
#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    /// Cygwin。`root` は Cygwin のフォルダ（`None` ならよくある場所を探す）。
    Cygwin { root: Option<&'a str> },
    /// WSL。`distribution` はディストリビューションの名前（`None` なら既定）。
    Wsl { distribution: Option<&'a str> },
}

/// 読むためのスレッドから渡すもの。
enum Event {
    /// 端末に流すバイト列（ブリッジのデータと、fantermo が出すエラーの文）。
    Output(Vec<u8>),
    /// 子プロセスが終わった。
    Exit(ExitCode),
}

pub struct Bridge {
    /// 書くためのスレッドへ渡す単位。起動できなかったときは `None`。
    input: Option<mpsc::Sender<Vec<u8>>>,
    /// ブリッジのプロセス。起動できなかったときは `None`。
    process: Option<Arc<OwnedHandle>>,
    events: mpsc::Receiver<Event>,
    /// 破棄して、ブリッジに終わってもらっている。読むためのスレッドは、閉じたことを知らせない。
    closing: Arc<AtomicBool>,
    /// 最後に伝えた大きさ。
    size: WindowSize,
    exit: Option<ExitCode>,
}

impl Bridge {
    /// `target` のブリッジを起動し、名乗りと大きさを送る。`options` のコマンドラインは相手の側の
    /// コマンドラインで、空なら相手のログインシェル。ブリッジが見つからないなど起動できないときも
    /// エラーにはせず、その理由を端末に出すバックエンドを返す。
    pub fn spawn(target: Target, options: &SpawnOptions, wake: Wake) -> io::Result<Self> {
        let (sender, events) = mpsc::channel();
        let size = window_size(options.size);
        let command = match bridge_command(target, options) {
            Ok(command) => command,
            Err(message) => return Ok(Self::failed(&message, sender, events, size, &wake)),
        };
        let mut command = command;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW.0);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                let message = format!(
                    "ブリッジを起動できませんでした（{}）: {e}",
                    command.get_program().to_string_lossy()
                );
                return Ok(Self::failed(&message, sender, events, size, &wake));
            }
        };
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(io::Error::other("the bridge has no pipes"));
        };
        let process = Arc::new(OwnedHandle::from(child));

        let (input, queued) = mpsc::channel::<Vec<u8>>();
        for message in [
            ToBridge::Hello {
                version: protocol::VERSION,
            },
            ToBridge::Resize(size),
        ] {
            _ = input.send(encode(&message));
        }
        thread::Builder::new()
            .name("bridge-writer".into())
            .spawn(move || write_input(stdin, &queued))?;
        let closing = Arc::new(AtomicBool::new(false));
        thread::Builder::new().name("bridge-reader".into()).spawn({
            let process = Arc::clone(&process);
            let closing = Arc::clone(&closing);
            move || read_output(stdout, &process, &closing, &sender, &*wake)
        })?;
        thread::Builder::new()
            .name("bridge-stderr".into())
            .spawn(move || log_stderr(stderr))?;

        Ok(Self {
            input: Some(input),
            process: Some(process),
            events,
            closing,
            size,
            exit: None,
        })
    }

    /// 起動できなかったバックエンド。`message` を端末に出す。
    fn failed(
        message: &str,
        sender: mpsc::Sender<Event>,
        events: mpsc::Receiver<Event>,
        size: WindowSize,
        wake: &Wake,
    ) -> Self {
        tracing::warn!("{message}");
        _ = sender.send(Event::Output(error_text(message)));
        wake();
        Self {
            input: None,
            process: None,
            events,
            closing: Arc::new(AtomicBool::new(false)),
            size,
            exit: None,
        }
    }

    fn send(&self, message: &ToBridge) -> io::Result<()> {
        let Some(input) = &self.input else {
            // 起動できなかったので、送る先がない。
            return Ok(());
        };
        input
            .send(encode(message))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

impl Backend for Bridge {
    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        for chunk in data.chunks(MAX_PAYLOAD_LEN) {
            self.send(&ToBridge::Data(chunk.to_vec()))?;
        }
        Ok(())
    }

    fn resize(&mut self, size: GridSize) -> io::Result<()> {
        let size = window_size(size);
        if size == self.size {
            return Ok(());
        }
        self.send(&ToBridge::Resize(size))?;
        self.size = size;
        Ok(())
    }

    fn drain_output(&mut self, sink: &mut dyn FnMut(&[u8])) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Output(data) => sink(&data),
                Event::Exit(code) => self.exit = Some(code),
            }
        }
    }

    fn poll_exit(&mut self) -> Option<ExitCode> {
        self.exit
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.closing.store(true, Ordering::Relaxed);
        if let Err(e) = self.send(&ToBridge::Quit) {
            tracing::debug!("the bridge has already stopped reading: {e}");
        }
        // 書くためのスレッドは、残りを書いてから標準入力を閉じて抜ける。ブリッジはそれで子プロセスを
        // 終わらせて終わる。終わらなければ止める。
        self.input = None;
        let Some(process) = self.process.take() else {
            return;
        };
        let reaper = thread::Builder::new()
            .name("bridge-reaper".into())
            .spawn(move || {
                if !wait_for(&process, Some(QUIT_TIMEOUT)) {
                    tracing::warn!("the bridge did not stop; terminating it");
                    // SAFETY: process は Arc が持つ有効なプロセスのハンドル。
                    if let Err(e) = unsafe { TerminateProcess(handle(&process), 1) } {
                        tracing::warn!("failed to terminate the bridge: {e}");
                    }
                }
            });
        if let Err(e) = reaper {
            tracing::warn!("failed to start the thread that waits for the bridge: {e}");
        }
    }
}

/// `target` のブリッジを起動するコマンド。ブリッジや Cygwin が見つからなければ、その理由の文。
fn bridge_command(target: Target, options: &SpawnOptions) -> Result<Command, String> {
    let cwd = match env::current_dir() {
        Ok(cwd) => Some(cwd),
        Err(e) => {
            tracing::warn!("failed to get the working folder: {e}");
            None
        }
    };
    let mut command = match target {
        Target::Cygwin { root } => cygwin_command(root, options.command_line, cwd.as_deref())?,
        Target::Wsl { distribution } => {
            wsl_command(distribution, options.command_line, cwd.as_deref())?
        }
    };
    for (name, value) in options.env {
        command.env(name, value);
    }
    Ok(command)
}

/// Cygwin のブリッジを起動するコマンド。Cygwin の `bin` を PATH の先頭に置き（`cygwin1.dll` と、子が
/// Cygwin のコマンドを拾うように）、`HOME` は渡さない（Cygwin の側で決める）。作業フォルダを渡すときは
/// `CHERE_INVOKING` も付ける（Cygwin の `/etc/profile` と `/etc/zprofile` は、これがなければログイン
/// シェルをホームへ移す）。
fn cygwin_command(
    configured_root: Option<&str>,
    command_line: &str,
    cwd: Option<&Path>,
) -> Result<Command, String> {
    let root = cygwin_root(configured_root)?;
    let exe = find_bridge(CYGWIN_BRIDGE)?;
    let mut command = Command::new(exe);
    let bin = root.join("bin");
    let mut path = OsString::from(bin.as_os_str());
    if let Some(rest) = env::var_os("PATH") {
        path.push(";");
        path.push(rest);
    }
    command.env("PATH", path).env_remove("HOME");
    if let Some(cwd) = cwd {
        command
            .env(CHERE_INVOKING, "1")
            .raw_arg("--cwd")
            .raw_arg(cygwin_quote(&cygwin_cwd(cwd)));
    }
    if !command_line.trim().is_empty() {
        command
            .env(COMMAND_VARIABLE, shell_command(command_line))
            .raw_arg("--")
            .raw_arg("/bin/sh")
            .raw_arg("-c")
            .raw_arg(cygwin_quote(CYGWIN_COMMAND_SCRIPT));
    }
    Ok(command)
}

/// WSL のブリッジを起動するコマンド。`wsl.exe` に作業フォルダを Windows のパスのまま渡して直させ、
/// ブリッジを `/mnt/<ドライブ>/…` のパスで起動させる。`TERM` を届けるため `WSLENV` に足す。
fn wsl_command(
    distribution: Option<&str>,
    command_line: &str,
    cwd: Option<&Path>,
) -> Result<Command, String> {
    let exe = find_bridge(LINUX_BRIDGE)?;
    let linux_exe = wsl_path(&exe).ok_or_else(|| {
        format!(
            "ブリッジ（{}）を WSL のパスに直せません。ドライブ文字のあるフォルダに置いてください。",
            exe.display()
        )
    })?;
    let mut command = Command::new(WSL_EXE);
    if let Some(distribution) = distribution {
        command.arg("-d").arg(distribution);
    }
    if let Some(cwd) = cwd {
        command.arg("--cd").arg(cwd);
    }
    command.arg("--exec").arg(linux_exe);
    if !command_line.trim().is_empty() {
        command.args(["--", "/bin/sh", "-c", &shell_command(command_line)]);
    }
    command.env(
        "WSLENV",
        wslenv_with_term(env::var("WSLENV").ok().as_deref()),
    );
    Ok(command)
}

/// 引用の外にあると、コマンドを区切る・つなぐ・入出力を付け替える・別のシェルで実行するシェルの記号。
const SHELL_OPERATORS: [char; 10] = ['|', '&', ';', '<', '>', '(', ')', '`', '\n', '\r'];
/// 先頭にあると、続くものが一つのコマンドでなくなる語（`exec` は付けても変わらないので含める）。
const SHELL_KEYWORDS: [&str; 22] = [
    "!", "{", "}", "[[", "]]", "case", "do", "done", "elif", "else", "esac", "exec", "fi", "for",
    "function", "if", "in", "select", "then", "time", "until", "while",
];

/// 利用者のコマンドライン `command_line` を `/bin/sh -c` に渡す形。一つのコマンドなら `exec` を前に
/// 付け、シェルを親として残さずそのコマンドにする（Ubuntu の dash などは一つのコマンドでも exec せず、
/// Ctrl+C でシェルも終わってそのシェルの終了コードが返る）。パイプや `;` などを含むときに `exec` を
/// 付けると最初のコマンドだけが exec されて意味が変わるので、そのまま渡す。
fn shell_command(command_line: &str) -> String {
    let command_line = command_line.trim();
    if is_single_command(command_line) {
        format!("exec {command_line}")
    } else {
        command_line.to_owned()
    }
}

/// `command` が、シェルの文法の記号を引用の外に含まない一つのコマンドか。先頭が変数の代入
/// （`NAME=値`）やシェルの予約語のものは含めない。
fn is_single_command(command: &str) -> bool {
    let first = command.split_whitespace().next().unwrap_or_default();
    if first.is_empty() || first.contains('=') || SHELL_KEYWORDS.contains(&first) {
        return false;
    }
    let mut quote = None;
    let mut escaped = false;
    for c in command.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (Some('\''), '\'') => quote = None,
            (Some('\''), _) => {}
            (_, '\\') => escaped = true,
            (Some(_), '"') => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, c) if SHELL_OPERATORS.contains(&c) => return false,
            (None, _) => {}
        }
    }
    quote.is_none() && !escaped
}

/// `WSLENV` の値 `current` に `TERM` を足したもの。すでにあればそのまま。
fn wslenv_with_term(current: Option<&str>) -> String {
    const TERM: &str = "TERM";
    let current = current.unwrap_or("");
    let has_term = current
        .split(':')
        .any(|entry| entry.split('/').next() == Some(TERM));
    match (current.is_empty(), has_term) {
        (true, _) => TERM.to_owned(),
        (false, true) => current.to_owned(),
        (false, false) => format!("{current}:{TERM}"),
    }
}

/// Cygwin のフォルダ。`configured` を書いていればそれ、なければ [`CYGWIN_ROOTS`] の順に、`bin` に
/// `cygwin1.dll` があるもの。見つからなければその理由の文。
fn cygwin_root(configured: Option<&str>) -> Result<PathBuf, String> {
    let has_dll = |root: &Path| root.join("bin").join(CYGWIN_DLL).is_file();
    if let Some(root) = configured {
        let root = PathBuf::from(root);
        if has_dll(&root) {
            return Ok(root);
        }
        return Err(format!(
            "Cygwin が見つかりません（{} がありません）。cygwin_root に Cygwin のフォルダを書いてください。",
            root.join("bin").join(CYGWIN_DLL).display()
        ));
    }
    CYGWIN_ROOTS
        .iter()
        .map(PathBuf::from)
        .find(|root| has_dll(root))
        .ok_or_else(|| {
            format!(
                "Cygwin が見つかりません（{} を探しました）。cygwin_root に Cygwin のフォルダを書いてください。",
                CYGWIN_ROOTS.join("、")
            )
        })
}

/// ブリッジの実行ファイル。fantermo の実行ファイルと同じフォルダ、なければ開発中にビルドしたもの
/// （`target/bridge/<ターゲット>/<debug|release>/`）。見つからなければその理由の文。
fn find_bridge((file_name, triple): (&str, &str)) -> Result<PathBuf, String> {
    let exe = env::current_exe()
        .map_err(|e| format!("fantermo の実行ファイルの場所が分かりません: {e}"))?;
    let candidates = bridge_candidates(&exe, file_name, triple);
    candidates
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .ok_or_else(|| {
            format!(
                "ブリッジ（{file_name}）が見つかりません（{} を探しました）。",
                candidates[0].display()
            )
        })
}

/// fantermo の実行ファイル `exe` に対して、ブリッジ `file_name` を探す場所。先頭がいつもの場所。
fn bridge_candidates(exe: &Path, file_name: &str, triple: &str) -> Vec<PathBuf> {
    let Some(dir) = exe.parent() else {
        return vec![PathBuf::from(file_name)];
    };
    let mut candidates = vec![dir.join(file_name)];
    if let Some(target_dir) = dir.parent() {
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        candidates.push(
            target_dir
                .join("bridge")
                .join(triple)
                .join(profile)
                .join(file_name),
        );
    }
    candidates
}

/// Windows のパス `path`（`C:\a\b`）を WSL の既定のマウントのパス（`/mnt/c/a/b`）に直す。ドライブ文字で
/// 始まらなければ `None`。
fn wsl_path(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);
    let mut chars = text.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    let rest = chars.as_str().strip_prefix(':')?;
    if !(rest.is_empty() || rest.starts_with(['\\', '/'])) {
        return None;
    }
    let rest = rest.replace('\\', "/");
    Some(format!(
        "/mnt/{}{}",
        drive.to_ascii_lowercase(),
        rest.trim_end_matches('/')
    ))
}

/// Cygwin のブリッジの `--cwd` に渡す作業フォルダ。Cygwin はドライブ文字のパスも受け付ける。`\` は
/// Cygwin のコマンドラインの分け方で扱いが変わるので、`/` にする。
fn cygwin_cwd(cwd: &Path) -> String {
    let text = cwd.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.replace('\\', "/")
}

/// Cygwin のプログラムのコマンドラインの引数一つとして、`text` を `'` で囲む。中の `'` は `'"'"'` にする。
/// `\` を含む文字列は正しく渡らない（Cygwin が引数の形によって `\` を取り除く）。
fn cygwin_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r#"'"'"'"#))
}

fn window_size(size: GridSize) -> WindowSize {
    let pixels = |cells: u16, cell: u32| {
        u16::try_from(u32::from(cells).saturating_mul(cell)).unwrap_or(u16::MAX)
    };
    WindowSize {
        cols: size.cols,
        rows: size.rows,
        width_px: pixels(size.cols, size.cell_width_px),
        height_px: pixels(size.rows, size.cell_height_px),
    }
}

/// 単位をバイト列にする。データは送る側で [`MAX_PAYLOAD_LEN`] までに分けるので、長すぎることはない。
fn encode(message: &ToBridge) -> Vec<u8> {
    protocol::encode(message).expect("データは MAX_PAYLOAD_LEN までに分けて送る")
}

/// `queued` の単位を標準入力に書く。チャネルが閉じたら（破棄したら）標準入力を閉じて抜ける。
fn write_input(mut stdin: ChildStdin, queued: &mpsc::Receiver<Vec<u8>>) {
    for bytes in queued {
        match stdin.write_all(&bytes).and_then(|()| stdin.flush()) {
            Ok(()) => {}
            // ブリッジが終わったあと（終了の依頼を送るときなど）。
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                tracing::debug!("the bridge has closed its input: {e}");
                return;
            }
            Err(e) => {
                tracing::warn!("failed to write to the bridge: {e}");
                return;
            }
        }
    }
}

/// ブリッジの標準出力を閉じるまで読み、単位ごとに `sender` へ渡して `wake` を呼ぶ。`closing` が立って
/// いれば（破棄したあとなら）、閉じたことは知らせない。
fn read_output(
    mut stdout: ChildStdout,
    process: &OwnedHandle,
    closing: &AtomicBool,
    sender: &mpsc::Sender<Event>,
    wake: &dyn Fn(),
) {
    let mut reader = OutputReader::default();
    let mut buf = vec![0u8; READ_BUFFER_SIZE];
    loop {
        let len = match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(len) => len,
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => break,
            Err(e) => {
                tracing::warn!("failed to read from the bridge: {e}");
                break;
            }
        };
        let events = reader.push(&buf[..len]);
        let done = reader.done;
        if send_all(sender, events) {
            wake();
        }
        if done {
            return;
        }
    }
    if closing.load(Ordering::Relaxed) {
        tracing::debug!("the bridge has closed its output after the tab was closed");
        return;
    }
    // 終わりの単位もエラーもなく閉じた。ブリッジの終了コードを添えて知らせる。
    let code = wait_for(process, Some(QUIT_TIMEOUT)).then(|| exit_code(process));
    if send_all(sender, reader.closed(code.flatten())) {
        wake();
    }
}

/// `events` を `sender` へ渡す。一つでも渡したら `true`。
fn send_all(sender: &mpsc::Sender<Event>, events: Vec<Event>) -> bool {
    let sent = !events.is_empty();
    for event in events {
        if sender.send(event).is_err() {
            return false;
        }
    }
    sent
}

/// ブリッジの標準出力を単位に分け、端末に流すものと子プロセスの終了にする。
#[derive(Default)]
struct OutputReader {
    decoder: Decoder,
    /// ブリッジが名乗った。
    greeted: bool,
    /// 名乗る前に届いたバイト列（[`RAW_OUTPUT_LIMIT`] まで）。ブリッジでないものが書いた文かもしれない。
    raw: Vec<u8>,
    /// 終わりの単位か、エラーを受け取った。このあとは読まない。
    done: bool,
}

impl OutputReader {
    /// 届いた `bytes` から取り出せた分。
    fn push(&mut self, bytes: &[u8]) -> Vec<Event> {
        if !self.greeted {
            let room = RAW_OUTPUT_LIMIT.saturating_sub(self.raw.len());
            self.raw.extend_from_slice(&bytes[..bytes.len().min(room)]);
        }
        self.decoder.push(bytes);
        let mut events = Vec::new();
        while !self.done {
            match self.decoder.next_message::<FromBridge>() {
                Ok(Some(message)) => events.extend(self.on_message(message)),
                Ok(None) => break,
                Err(e) => {
                    if self.greeted {
                        events.push(self.fail(&format!("ブリッジの出力を読めませんでした: {e}")));
                    } else {
                        // 単位として読めないものは、ブリッジの前に動いたもの（wsl.exe など）の文かもしれない。
                        // 閉じるまで待って、届いたものをまとめて見せる。
                        break;
                    }
                }
            }
        }
        events
    }

    fn on_message(&mut self, message: FromBridge) -> Option<Event> {
        match message {
            FromBridge::Hello { version } if !self.greeted => {
                self.greeted = true;
                self.raw = Vec::new();
                protocol::check_version(version)
                    .err()
                    .map(|e| self.fail(&e.to_string()))
            }
            FromBridge::Hello { .. } => None,
            _ if !self.greeted => Some(self.fail("ブリッジが名乗る前に、ほかの単位が届きました。")),
            FromBridge::Data(data) => Some(Event::Output(data)),
            FromBridge::Exit(exit) => {
                self.done = true;
                let code = match exit {
                    ChildExit::Code(code) => ExitCode(code as u32),
                    ChildExit::Signal(signal) => {
                        ExitCode(SIGNAL_EXIT_BASE.saturating_add(signal as u32))
                    }
                };
                tracing::info!("the bridge reports that the child ended: {exit:?}");
                Some(Event::Exit(code))
            }
            FromBridge::Error(text) => Some(self.fail(&format!("ブリッジのエラー: {text}"))),
        }
    }

    /// ブリッジとのやり取りを続けられない。`message` を端末に出し、このあとは読まない。
    fn fail(&mut self, message: &str) -> Event {
        tracing::warn!("{message}");
        self.done = true;
        Event::Output(error_text(message))
    }

    /// 終わりの単位もエラーもなく標準出力が閉じた。`code` はブリッジのプロセスの終了コード。
    fn closed(&mut self, code: Option<u32>) -> Vec<Event> {
        if self.done {
            return Vec::new();
        }
        let code = code.map_or_else(|| "不明".to_owned(), exit_code_text);
        let mut message = if self.greeted {
            format!("ブリッジが終わりました（終了コード {code}）。")
        } else {
            format!("ブリッジが名乗らずに終わりました（終了コード {code}）。")
        };
        let raw = readable_text(&self.raw);
        if !raw.trim().is_empty() {
            message.push('\n');
            message.push_str(raw.trim_end());
        }
        vec![self.fail(&message)]
    }
}

/// プロセスの終了コードの見せ方。NTSTATUS（`0xc0000135` など）のような大きな値は 16 進で見せる。
fn exit_code_text(code: u32) -> String {
    const HEX_FROM: u32 = 0x1_0000;
    if code < HEX_FROM {
        code.to_string()
    } else {
        format!("{code:#x}")
    }
}

/// 名乗る前に届いたバイト列を、読める文にする。`wsl.exe` は自分のエラーを UTF-16LE で標準出力に書く
/// ので、NUL を含めば UTF-16LE として読む。制御文字は改行とタブのほかは除く。
fn readable_text(bytes: &[u8]) -> String {
    // UTF-8 の文には NUL がないが、UTF-16LE の ASCII の文字は NUL を含む。
    let text = if bytes.contains(&0) {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_le_bytes(pair))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    text.chars()
        .filter(|&c| !c.is_control() || c == '\n' || c == '\t')
        .collect()
}

/// `message` を端末に赤い文字で出すバイト列。前後で改行し、`fantermo: ` を前に付ける。
fn error_text(message: &str) -> Vec<u8> {
    let message = message.replace('\n', "\r\n");
    format!("\r\n\x1b[31mfantermo: {message}\x1b[m\r\n").into_bytes()
}

/// ブリッジの標準エラー出力を、行ごとにログへ流す。
fn log_stderr(stderr: impl Read) {
    for line in BufReader::new(stderr).lines() {
        match line {
            Ok(line) => tracing::info!("bridge: {line}"),
            Err(e) => {
                tracing::debug!("stopped reading the bridge's stderr: {e}");
                return;
            }
        }
    }
}

fn handle(process: &OwnedHandle) -> HANDLE {
    HANDLE(process.as_raw_handle())
}

/// プロセスが終わるのを `timeout` まで（`None` なら終わるまで）待つ。終わったら `true`。
fn wait_for(process: &OwnedHandle, timeout: Option<Duration>) -> bool {
    let millis = timeout.map_or(INFINITE, |timeout| {
        u32::try_from(timeout.as_millis()).unwrap_or(INFINITE)
    });
    // SAFETY: process は呼び出し元が持つ有効なプロセスのハンドル。
    unsafe { WaitForSingleObject(handle(process), millis) == WAIT_OBJECT_0 }
}

/// 終わったプロセスの終了コード。読めなければ `None`。
fn exit_code(process: &OwnedHandle) -> Option<u32> {
    let mut code = 0u32;
    // SAFETY: process は終わったプロセスの有効なハンドルで、code は書き込める。
    match unsafe { GetExitCodeProcess(handle(process), &mut code) } {
        Ok(()) => Some(code),
        Err(e) => {
            tracing::warn!("failed to get the bridge's exit code: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(events: &[Event]) -> Vec<u8> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Output(data) => Some(data.clone()),
                Event::Exit(_) => None,
            })
            .flatten()
            .collect()
    }

    fn exits(events: &[Event]) -> Vec<ExitCode> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Exit(code) => Some(*code),
                Event::Output(_) => None,
            })
            .collect()
    }

    fn bytes(messages: &[FromBridge]) -> Vec<u8> {
        let mut out = Vec::new();
        for message in messages {
            protocol::encode_into(message, &mut out).unwrap();
        }
        out
    }

    #[test]
    fn data_and_exit_come_through_after_the_greeting() {
        let mut reader = OutputReader::default();
        let all = bytes(&[
            FromBridge::Hello {
                version: protocol::VERSION,
            },
            FromBridge::Data(b"hello".to_vec()),
            FromBridge::Exit(ChildExit::Code(7)),
        ]);
        // 途中で切れて届いても、揃った単位から取り出す。
        let (first, rest) = all.split_at(7);
        let mut events = reader.push(first);
        events.extend(reader.push(rest));
        assert_eq!(output(&events), b"hello");
        assert_eq!(exits(&events), [ExitCode(7)]);
        assert!(reader.done);
        assert!(reader.closed(Some(0)).is_empty());
    }

    #[test]
    fn signals_become_128_plus_the_number() {
        let mut reader = OutputReader::default();
        let events = reader.push(&bytes(&[
            FromBridge::Hello {
                version: protocol::VERSION,
            },
            FromBridge::Exit(ChildExit::Signal(1)),
        ]));
        assert_eq!(exits(&events), [ExitCode(129)]);
    }

    #[test]
    fn errors_are_shown_in_red_and_stop_reading() {
        let mut reader = OutputReader::default();
        let events = reader.push(&bytes(&[
            FromBridge::Hello {
                version: protocol::VERSION + 1,
            },
            FromBridge::Data(b"after".to_vec()),
        ]));
        let text = String::from_utf8(output(&events)).unwrap();
        assert!(text.starts_with("\r\n\x1b[31mfantermo: "), "{text:?}");
        assert!(text.contains("バージョンが合いません"), "{text:?}");
        assert!(!text.contains("after"));
        assert!(reader.closed(Some(3)).is_empty());

        let mut reader = OutputReader::default();
        let events = reader.push(&bytes(&[
            FromBridge::Hello {
                version: protocol::VERSION,
            },
            FromBridge::Error("cannot fork".to_owned()),
        ]));
        let text = String::from_utf8(output(&events)).unwrap();
        assert!(text.contains("ブリッジのエラー: cannot fork"), "{text:?}");
        assert!(exits(&events).is_empty());
    }

    #[test]
    fn text_before_the_greeting_is_shown_when_closed() {
        let mut reader = OutputReader::default();
        let message: Vec<u8> = "エラー: Wsl/Service/WSL_E_DISTRO_NOT_FOUND\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert!(reader.push(&message).is_empty());
        let events = reader.closed(Some(u32::MAX));
        let text = String::from_utf8(output(&events)).unwrap();
        assert!(
            text.contains("名乗らずに終わりました（終了コード 0xffffffff）"),
            "{text:?}"
        );
        assert!(
            text.contains("\r\nエラー: Wsl/Service/WSL_E_DISTRO_NOT_FOUND"),
            "{text:?}"
        );

        assert_eq!(
            readable_text(b"plain\x1b[31m\ttext\r\n"),
            "plain[31m\ttext\n"
        );
    }

    #[test]
    fn wsl_paths_use_the_default_mount() {
        assert_eq!(
            wsl_path(Path::new(r"C:\Users\xia\fantermo-bridge")).as_deref(),
            Some("/mnt/c/Users/xia/fantermo-bridge")
        );
        assert_eq!(
            wsl_path(Path::new(r"\\?\D:\a b\x")).as_deref(),
            Some("/mnt/d/a b/x")
        );
        assert_eq!(wsl_path(Path::new(r"\\server\share\x")), None);
        assert_eq!(wsl_path(Path::new(r"C:x")), None);
    }

    #[test]
    fn cygwin_arguments_avoid_backslashes() {
        assert_eq!(cygwin_cwd(Path::new(r"C:\Users\xia")), "C:/Users/xia");
        assert_eq!(cygwin_quote("it's $HOME"), r#"'it'"'"'s $HOME'"#);
        assert!(!CYGWIN_COMMAND_SCRIPT.contains('\\'));
        assert!(!CYGWIN_COMMAND_SCRIPT.contains('\''));
        assert!(CYGWIN_COMMAND_SCRIPT.contains(COMMAND_VARIABLE));
    }

    #[test]
    fn single_commands_are_executed_in_place_of_the_shell() {
        assert_eq!(shell_command(" zsh -l "), "exec zsh -l");
        assert_eq!(
            shell_command(r#"sh -c 'trap "exit 7" INT; sleep 100'"#),
            r#"exec sh -c 'trap "exit 7" INT; sleep 100'"#
        );
        assert_eq!(
            shell_command(r#"printf "%s|%s\n" a\;b "$(date)""#),
            r#"exec printf "%s|%s\n" a\;b "$(date)""#
        );
        for command in [
            "ls | less",
            "cd /tmp; zsh",
            "make && zsh",
            "sleep 1 &",
            "zsh > log",
            "(zsh)",
            "echo `date`",
            "echo $(date; date)",
            "a\nb",
            "LANG=C zsh",
            "exec zsh",
            "if true; then zsh; fi",
            "{ zsh; }",
            "! false",
            "echo 'open",
            "echo end\\",
        ] {
            assert_eq!(shell_command(command), command, "{command:?}");
        }
    }

    #[test]
    fn wslenv_gets_term_once() {
        assert_eq!(wslenv_with_term(None), "TERM");
        assert_eq!(wslenv_with_term(Some("")), "TERM");
        assert_eq!(
            wslenv_with_term(Some("USERPROFILE/p")),
            "USERPROFILE/p:TERM"
        );
        assert_eq!(wslenv_with_term(Some("TERM/u:X")), "TERM/u:X");
    }

    #[test]
    fn bridge_is_looked_up_next_to_fantermo_then_in_the_build_output() {
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        assert_eq!(
            bridge_candidates(
                Path::new(r"C:\repo\target\debug\fantermo.exe"),
                "fantermo-bridge",
                "x86_64-unknown-linux-musl"
            ),
            [
                PathBuf::from(r"C:\repo\target\debug\fantermo-bridge"),
                PathBuf::from(format!(
                    r"C:\repo\target\bridge\x86_64-unknown-linux-musl\{profile}\fantermo-bridge"
                )),
            ]
        );
    }

    #[test]
    fn sizes_carry_pixels() {
        let size = window_size(GridSize {
            cols: 100,
            rows: 30,
            cell_width_px: 16,
            cell_height_px: 32,
        });
        assert_eq!(
            size,
            WindowSize {
                cols: 100,
                rows: 30,
                width_px: 1600,
                height_px: 960
            }
        );
        let huge = window_size(GridSize {
            cols: 5000,
            rows: 1,
            cell_width_px: 100,
            cell_height_px: 1,
        });
        assert_eq!(huge.width_px, u16::MAX);
    }
}
