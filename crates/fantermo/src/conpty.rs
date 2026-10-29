//! ConPTY のバックエンド。
//!
//! 疑似コンソールとは 2 本の匿名パイプでつながる。入力はウィンドウのスレッドから書き込み、出力は読むための
//! スレッドで待ってチャネルで渡す。疑似コンソールは子プロセスが終了しても出力のパイプを閉じないので、
//! 終了は子プロセスのハンドルを待つスレッドで知る。

use std::{
    ffi::{OsStr, OsString, c_void},
    fs::File,
    io::{self, Read, Write},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    sync::{Arc, mpsc},
    thread,
};

use fantermo_core::backend::{Backend, ExitCode, GridSize, SpawnOptions, Wake};
use windows::{
    Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0},
        System::{
            Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole},
            Pipes::CreatePipe,
            Threading::{
                CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
                EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE,
                InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
                STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

/// 出力のパイプから一度に読む大きさ。
const READ_BUFFER_SIZE: usize = 64 * 1024;

pub struct ConPty {
    console: HPCON,
    /// 疑似コンソールに伝えてある桁数と行数。セルのピクセル寸法は疑似コンソールに渡らないので、
    /// これが変わったときだけ伝える。
    console_size: COORD,
    input: File,
    process: Arc<OwnedHandle>,
    output: mpsc::Receiver<Vec<u8>>,
    reader: Option<thread::JoinHandle<()>>,
    exit: Option<ExitCode>,
}

impl ConPty {
    /// `options` のコマンドラインを疑似コンソールにつないで起動する。コマンドラインは
    /// `CreateProcessW` にそのまま渡す。
    pub fn spawn(options: &SpawnOptions, wake: Wake) -> io::Result<Self> {
        let (console_input, input) = create_pipe()?;
        let (output, console_output) = create_pipe()?;

        let console_size = coord(options.size);
        // SAFETY: 二つのハンドルは直前に作った有効なパイプの端で、この呼び出しの間は生きている。
        let console = unsafe {
            CreatePseudoConsole(
                console_size,
                HANDLE(console_input.as_raw_handle()),
                HANDLE(console_output.as_raw_handle()),
                0,
            )?
        };
        // 疑似コンソールは自分で複製を持つので、こちらの分は閉じる。
        drop(console_input);
        drop(console_output);

        let process = match spawn_process(console, options) {
            Ok(process) => Arc::new(process),
            Err(e) => {
                // SAFETY: console は作ったばかりで、まだ誰も閉じていない。
                unsafe { ClosePseudoConsole(console) };
                return Err(e);
            }
        };

        let (sender, receiver) = mpsc::channel();
        let reader = thread::Builder::new().name("conpty-reader".into()).spawn({
            let wake = Arc::clone(&wake);
            move || read_output(File::from(output), &sender, &*wake)
        })?;
        thread::Builder::new()
            .name("conpty-exit-watcher".into())
            .spawn({
                let process = Arc::clone(&process);
                move || {
                    wait_for_exit(&process);
                    wake();
                }
            })?;

        Ok(Self {
            console,
            console_size,
            input: File::from(input),
            process,
            output: receiver,
            reader: Some(reader),
            exit: None,
        })
    }
}

impl Backend for ConPty {
    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.input.write_all(data)
    }

    fn resize(&mut self, size: GridSize) -> io::Result<()> {
        let console_size = coord(size);
        if console_size == self.console_size {
            return Ok(());
        }
        // SAFETY: self.console は Drop まで閉じない。
        unsafe { ResizePseudoConsole(self.console, console_size) }.map_err(io::Error::from)?;
        self.console_size = console_size;
        Ok(())
    }

    fn drain_output(&mut self, sink: &mut dyn FnMut(&[u8])) {
        while let Ok(chunk) = self.output.try_recv() {
            sink(&chunk);
        }
    }

    fn poll_exit(&mut self) -> Option<ExitCode> {
        if self.exit.is_some() {
            return self.exit;
        }
        let process = HANDLE(self.process.as_raw_handle());
        // SAFETY: process は self.process が持つ有効なハンドル。
        if unsafe { WaitForSingleObject(process, 0) } != WAIT_OBJECT_0 {
            return None;
        }
        let mut code = 0u32;
        // SAFETY: process は終了済みのプロセスの有効なハンドルで、code は書き込める。
        if let Err(e) = unsafe { GetExitCodeProcess(process, &mut code) } {
            tracing::warn!("failed to get the child exit code: {e}");
        }
        self.exit = Some(ExitCode(code));
        self.exit
    }
}

impl Drop for ConPty {
    fn drop(&mut self) {
        // 疑似コンソールを閉じると出力のパイプも閉じ、読むためのスレッドが抜ける。
        // SAFETY: self.console はここで一度だけ閉じる。
        unsafe { ClosePseudoConsole(self.console) };
        if let Some(reader) = self.reader.take()
            && reader.join().is_err()
        {
            tracing::warn!("the conpty reader thread panicked");
        }
    }
}

fn create_pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    // SAFETY: read と write は書き込める HANDLE を指す。
    unsafe { CreatePipe(&mut read, &mut write, None, 0)? };
    // SAFETY: CreatePipe が成功したので、二つのハンドルは有効で、ほかに持ち主はいない。
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(read.0),
            OwnedHandle::from_raw_handle(write.0),
        )
    })
}

/// `console` につないだ子プロセスを起動し、そのプロセスのハンドルを返す。
fn spawn_process(console: HPCON, options: &SpawnOptions) -> io::Result<OwnedHandle> {
    // 一度目の呼び出しは属性リストに要る大きさを返すだけで、失敗として戻る。
    let mut size = 0usize;
    // SAFETY: リストを渡さずに大きさだけを問い合わせる呼び方。
    _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut size) };
    let mut attribute_buffer = vec![0u8; size];
    let attributes = LPPROC_THREAD_ATTRIBUTE_LIST(attribute_buffer.as_mut_ptr().cast());

    // SAFETY: attributes は size バイトの確保済みの領域を指し、この関数を抜けるまで生きている。
    unsafe { InitializeProcThreadAttributeList(Some(attributes), 1, None, &mut size)? };
    // 属性の値は HPCON を指すポインタではなく、HPCON そのもの。
    // SAFETY: attributes は初期化済みで、console は有効な疑似コンソール。
    let updated = unsafe {
        UpdateProcThreadAttribute(
            attributes,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            Some(console.0 as *const c_void),
            size_of::<HPCON>(),
            None,
            None,
        )
    };
    if let Err(e) = updated {
        // SAFETY: attributes は初期化済みで、ここで一度だけ破棄する。
        unsafe { DeleteProcThreadAttributeList(attributes) };
        return Err(e.into());
    }

    let mut startup_info = STARTUPINFOEXW::default();
    startup_info.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    // 標準ハンドルを空にすると、子プロセスは疑似コンソールを使う。このフラグがないと、このプロセスの
    // 標準入出力がリダイレクトされているときに子プロセスがそれを引き継ぎ、出力が疑似コンソールを通らない。
    startup_info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup_info.lpAttributeList = attributes;

    // CreateProcessW はコマンドラインの領域を書き換えることがあるので、書き込める領域を渡す。
    let mut command_line = to_wide_nul(OsStr::new(options.command_line));
    let environment = environment_block(options.env);
    let mut process_info = PROCESS_INFORMATION::default();
    // SAFETY: どのポインタもこの呼び出しの間は生きている。環境変数のブロックは UTF-16 で、
    // 終端の空文字列まで含む。
    let created = unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            Some(environment.as_ptr().cast()),
            PCWSTR::null(),
            &startup_info.StartupInfo,
            &mut process_info,
        )
    };
    // SAFETY: attributes は初期化済みで、ここで一度だけ破棄する。
    unsafe { DeleteProcThreadAttributeList(attributes) };
    created?;

    // SAFETY: CreateProcessW が成功したので、二つのハンドルは有効で、ほかに持ち主はいない。
    let (process, _thread) = unsafe {
        (
            OwnedHandle::from_raw_handle(process_info.hProcess.0),
            OwnedHandle::from_raw_handle(process_info.hThread.0),
        )
    };
    Ok(process)
}

/// このプロセスの環境に `overrides` を重ねた、`CreateProcessW` 用の環境変数のブロックを作る。
///
/// 名前の大文字小文字を区別せずに並べる（Windows の環境変数のブロックの決まり）。
fn environment_block(overrides: &[(&str, &str)]) -> Vec<u16> {
    let mut vars: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| {
            !overrides
                .iter()
                .any(|(overridden, _)| name.eq_ignore_ascii_case(overridden))
        })
        .chain(
            overrides
                .iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value))),
        )
        .collect();
    vars.sort_by_key(|(name, _)| name.to_ascii_uppercase());

    let mut block = Vec::new();
    for (name, value) in &vars {
        block.extend(name.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

fn to_wide_nul(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

fn coord(size: GridSize) -> COORD {
    COORD {
        X: i16::try_from(size.cols).unwrap_or(i16::MAX),
        Y: i16::try_from(size.rows).unwrap_or(i16::MAX),
    }
}

/// 出力のパイプが閉じるまで読み、読めた分をチャネルで渡す。
fn read_output(mut output: File, sender: &mpsc::Sender<Vec<u8>>, wake: &dyn Fn()) {
    let mut buf = vec![0u8; READ_BUFFER_SIZE];
    loop {
        match output.read(&mut buf) {
            Ok(0) => return,
            Ok(len) => {
                if sender.send(buf[..len].to_vec()).is_err() {
                    return;
                }
                wake();
            }
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return,
            Err(e) => {
                tracing::warn!("failed to read from the pseudo console: {e}");
                return;
            }
        }
    }
}

fn wait_for_exit(process: &OwnedHandle) {
    // SAFETY: process は呼び出し元の Arc が持つ有効なハンドル。
    unsafe { WaitForSingleObject(HANDLE(process.as_raw_handle()), INFINITE) };
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use fantermo_core::vt::Vt;

    use super::*;

    const SIZE: GridSize = GridSize {
        cols: 80,
        rows: 24,
        cell_width_px: 8,
        cell_height_px: 16,
    };

    /// 子プロセスが終わるまでの上限。
    const TIMEOUT: Duration = Duration::from_secs(10);

    /// 出力を待つ間隔。
    const POLL_INTERVAL: Duration = Duration::from_millis(20);

    /// `command_line` を起動し、終了して画面に `expected` が出るまで待つ。画面の文字と終了コードを返す。
    fn run_until(command_line: &str, env: &[(&str, &str)], expected: &str) -> (String, ExitCode) {
        let options = SpawnOptions {
            command_line,
            env,
            size: SIZE,
        };
        let mut pty = ConPty::spawn(&options, Arc::new(|| {})).unwrap();
        let mut vt = Vt::new(SIZE, "xterm-256color").unwrap();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            pty.drain_output(&mut |data| vt.feed(data));
            let screen = vt.plain_text().unwrap();
            if let Some(code) = pty.poll_exit()
                && screen.contains(expected)
            {
                return (screen, code);
            }
            assert!(Instant::now() < deadline, "timed out; screen:\n{screen}");
            thread::sleep(POLL_INTERVAL);
        }
    }

    #[test]
    fn output_reaches_the_terminal() {
        let (screen, code) = run_until("cmd.exe /c echo fantermo-marker", &[], "fantermo-marker");
        assert!(screen.contains("fantermo-marker"));
        assert_eq!(code, ExitCode(0));
    }

    #[test]
    fn exit_code_is_reported() {
        let (_, code) = run_until("cmd.exe /c exit 3", &[], "");
        assert_eq!(code, ExitCode(3));
    }

    #[test]
    fn environment_overrides_reach_the_child() {
        let (screen, _) = run_until(
            "cmd.exe /c echo [%FANTERMO_TEST%]",
            &[("FANTERMO_TEST", "passed")],
            "[passed]",
        );
        assert!(screen.contains("[passed]"));
    }
}
