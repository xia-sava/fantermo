//! ConPTY のバックエンド。
//!
//! 疑似コンソールとは 2 本の匿名パイプで接続する。入力はウィンドウのスレッドから書き込み、出力は読むための
//! スレッドで待ってチャネルで渡す。疑似コンソールは子プロセスが終了しても出力のパイプを閉じないので、
//! 終了は子プロセスのハンドルを待つスレッドで知る。
//!
//! 疑似コンソールを閉じると、接続しているプロセスに閉じる知らせが一度届く。そのあとに接続したプロセス（起動の途中
//! だったもの）には届かず、動き続ける。疑似コンソールのプロセス（`conhost.exe`）もそれを待って残るので、猶予の
//! あとで、そのコンソールに接続しているプロセスだけを終わらせる。別のコンソールを持つプロセスと、どのコンソールにも
//! 接続していないプロセスには触らない。

use std::{
    ffi::{OsStr, OsString, c_void},
    fs::File,
    io::{self, Read, Write},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use fantermo_core::backend::{Backend, ExitCode, GridSize, SpawnOptions, Wake};
use windows::{
    Wdk::System::Threading::{NtQueryInformationProcess, PROCESSINFOCLASS},
    Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0},
        System::{
            Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole},
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            Pipes::CreatePipe,
            Threading::{
                CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
                EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcessId, GetExitCodeProcess, INFINITE,
                InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcess,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
                STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
                WaitForSingleObject,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

/// 出力のパイプから一度に読む大きさ。
const READ_BUFFER_SIZE: usize = 64 * 1024;

/// 疑似コンソールを閉じたあと、接続しているプロセスが自分で終わるのを待つ猶予（ミリ秒）。
const CLOSE_TIMEOUT_MS: u32 = 5000;

/// 猶予のあと、遅れて接続するプロセスを終わらせ続ける上限。
const SWEEP_LIMIT: Duration = Duration::from_secs(30);

/// 猶予のあと、コンソールに接続しているプロセスを探す間隔（ミリ秒）。
const SWEEP_INTERVAL_MS: u32 = 200;

/// プロセスを終わらせるときの終了コード。
const TERMINATED_EXIT_CODE: u32 = 1;

/// [`wait_for_closing_consoles`] が待つ上限。猶予のあと、残りを終わらせるまでの分を足す。
const CLOSING_AT_EXIT_TIMEOUT: Duration = Duration::from_millis(CLOSE_TIMEOUT_MS as u64 + 2000);

/// `NtQueryInformationProcess` の `ProcessConsoleHostProcess`。値はコンソールのプロセスの番号と、下位 2 ビットの印。
/// Windows SDK のヘッダーにはない。
const PROCESS_CONSOLE_HOST_PROCESS: PROCESSINFOCLASS = PROCESSINFOCLASS(49);

/// `ProcessConsoleHostProcess` の値の下位 2 ビットのうち、コンソールに接続していることを表す値。
const CONSOLE_HOST_CONNECTED: usize = 1;
const CONSOLE_HOST_FLAG_MASK: usize = 3;

/// 閉じている途中の疑似コンソールの数。プロセスが終わる前に [`wait_for_closing_consoles`] で待つ。
static CLOSING: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// 閉じている途中の疑似コンソールの片づけが終わるのを、上限まで待つ。プロセスが終わると閉じるスレッドも
/// 終わり、接続の途中だったプロセスが残るので、ウィンドウを閉じたあと、プロセスを終える前に呼ぶ。
pub fn wait_for_closing_consoles() {
    let (count, changed) = &CLOSING;
    let Ok(count) = count.lock() else {
        tracing::warn!("the count of closing pseudo consoles is poisoned");
        return;
    };
    let waited = changed.wait_timeout_while(count, CLOSING_AT_EXIT_TIMEOUT, |count| *count > 0);
    match waited {
        Ok((count, result)) if result.timed_out() => {
            tracing::warn!("{} pseudo consoles are still closing", *count);
        }
        Ok(_) => {}
        Err(_) => tracing::warn!("the count of closing pseudo consoles is poisoned"),
    }
}

/// 閉じている途中の疑似コンソールの数を、生きている間だけ一つ増やす。
struct ClosingCount;

impl ClosingCount {
    fn new() -> Self {
        Self::add(1);
        Self
    }

    fn add(delta: isize) {
        let (count, changed) = &CLOSING;
        match count.lock() {
            Ok(mut count) => {
                *count = count.saturating_add_signed(delta);
                changed.notify_all();
            }
            Err(_) => tracing::warn!("the count of closing pseudo consoles is poisoned"),
        }
    }
}

impl Drop for ClosingCount {
    fn drop(&mut self) {
        Self::add(-1);
    }
}

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
    /// `options` のコマンドラインを疑似コンソールに接続して起動する。コマンドラインは
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

        let process = match spawn_process(console, options, PROCESS_CREATION_FLAGS(0)) {
            Ok(Spawned { process, .. }) => Arc::new(process),
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
    /// 接続の途中のプロセスがあると、疑似コンソールを閉じても子プロセスと conhost が残り、出力のパイプが閉じない
    /// ことがある。呼び出し元（ウィンドウのスレッド）を止めないよう、閉じるのは別のスレッドでする。
    fn drop(&mut self) {
        let console = self.console.0;
        let process = Arc::clone(&self.process);
        let reader = self.reader.take();
        let count = ClosingCount::new();
        let closing = thread::Builder::new()
            .name("conpty-closer".into())
            .spawn(move || {
                let _count = count;
                close_console(HPCON(console), &process, reader);
            });
        if let Err(e) = closing {
            tracing::warn!("failed to start closing the pseudo console: {e}");
        }
    }
}

/// 疑似コンソールを閉じ、接続しているプロセスが終わらなければ終わらせて、読むためのスレッドが抜けるのを待つ。
fn close_console(console: HPCON, process: &OwnedHandle, reader: Option<thread::JoinHandle<()>>) {
    // console が閉じるまでは conhost が残っているので、番号が別のプロセスのものになっていない。
    let host_before_closing = ConsoleHost::of(process);
    // 疑似コンソールを閉じると、接続しているプロセスがなくなったところで出力のパイプも閉じ、読むためのスレッドが抜ける。
    // SAFETY: console は Drop がこのスレッドに渡したもので、ここで一度だけ閉じる。
    unsafe { ClosePseudoConsole(console) };

    let child = HANDLE(process.as_raw_handle());
    let deadline = Instant::now() + Duration::from_millis(CLOSE_TIMEOUT_MS.into());
    // SAFETY: child は process が持つ有効なハンドル。
    let child_ended = unsafe { WaitForSingleObject(child, CLOSE_TIMEOUT_MS) } == WAIT_OBJECT_0;
    // 閉じる前に接続していなかった子プロセスは、動いている間は接続した先の conhost が残っている。
    let host = host_before_closing.or_else(|| {
        if child_ended {
            None
        } else {
            ConsoleHost::of(process)
        }
    });
    if let Some(host) = &host {
        host.wait_until(deadline);
    }
    if !child_ended {
        tracing::warn!("the child did not end after closing the pseudo console; terminating it");
        // SAFETY: child は process が持つ有効なハンドル。
        if let Err(e) = unsafe { TerminateProcess(child, TERMINATED_EXIT_CODE) } {
            tracing::warn!("failed to terminate the child: {e}");
        }
    }
    if let Some(host) = &host {
        host.end_clients();
    }
    if let Some(reader) = reader
        && reader.join().is_err()
    {
        tracing::warn!("the conpty reader thread panicked");
    }
}

/// 疑似コンソールのプロセス。
struct ConsoleHost {
    pid: u32,
    process: OwnedHandle,
}

impl ConsoleHost {
    /// `client` が接続しているコンソールのプロセス。接続する前か、開けなければ `None`。
    fn of(client: &OwnedHandle) -> Option<Self> {
        let pid = connected_console_host(query_console_host(HANDLE(client.as_raw_handle()))?)?;
        // SAFETY: 終わるのを待つ権限で開くだけ。
        let process = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
            Ok(process) => process,
            Err(e) => {
                tracing::debug!("cannot open the console host {pid}: {e}");
                return None;
            }
        };
        // SAFETY: OpenProcess が成功したので、ハンドルは有効で、ほかに持ち主はいない。
        let process = unsafe { OwnedHandle::from_raw_handle(process.0) };
        Some(Self { pid, process })
    }

    /// 終わったか。`milliseconds` まで待つ。
    fn wait(&self, milliseconds: u32) -> bool {
        // SAFETY: self.process は有効なハンドル。
        let waited =
            unsafe { WaitForSingleObject(HANDLE(self.process.as_raw_handle()), milliseconds) };
        waited == WAIT_OBJECT_0
    }

    fn wait_until(&self, deadline: Instant) {
        let left = deadline.saturating_duration_since(Instant::now());
        self.wait(u32::try_from(left.as_millis()).unwrap_or(CLOSE_TIMEOUT_MS));
    }

    /// このコンソールに接続しているプロセスを、接続するものがなくなって conhost が終わるまで終わらせ続ける。
    fn end_clients(&self) {
        let deadline = Instant::now() + SWEEP_LIMIT;
        while !self.wait(0) {
            if Instant::now() >= deadline {
                tracing::warn!("the console host {} did not end", self.pid);
                return;
            }
            self.terminate_clients();
            self.wait(SWEEP_INTERVAL_MS);
        }
    }

    fn terminate_clients(&self) {
        let processes = match process_ids() {
            Ok(processes) => processes,
            Err(e) => {
                tracing::warn!("failed to list the processes: {e}");
                return;
            }
        };
        for pid in processes {
            if console_host_of_process(pid) == Some(self.pid) {
                tracing::warn!(
                    "ending the process {pid}, which connected to the console {} after it closed",
                    self.pid
                );
                terminate_process(pid);
            }
        }
    }
}

/// `ProcessConsoleHostProcess` の値から、接続しているコンソールのプロセスの番号を取り出す。
///
/// 接続する前は作ったプロセスの番号に別の印が付いた値で、ウィンドウのプログラムはそのまま変わらない。コンソールを
/// 手放すと番号は 0 になる。
fn connected_console_host(value: usize) -> Option<u32> {
    if value & CONSOLE_HOST_FLAG_MASK != CONSOLE_HOST_CONNECTED {
        return None;
    }
    u32::try_from(value & !CONSOLE_HOST_FLAG_MASK)
        .ok()
        .filter(|pid| *pid != 0)
}

fn query_console_host(process: HANDLE) -> Option<usize> {
    let mut value = 0usize;
    // SAFETY: process は有効なハンドルで、value は渡した長さの分だけ書き込める。
    let status = unsafe {
        NtQueryInformationProcess(
            process,
            PROCESS_CONSOLE_HOST_PROCESS,
            (&raw mut value).cast(),
            size_of::<usize>() as u32,
            std::ptr::null_mut(),
        )
    };
    if status.is_err() {
        tracing::warn!("failed to query the console host: {status:?}");
        return None;
    }
    Some(value)
}

/// プロセス `pid` が接続しているコンソールのプロセスの番号。開けない（ほかの利用者のものなど）か、接続して
/// いなければ `None`。
fn console_host_of_process(pid: u32) -> Option<u32> {
    // SAFETY: 読み取りの権限で開くだけ。
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    // SAFETY: OpenProcess が成功したので、ハンドルは有効で、ほかに持ち主はいない。
    let process = unsafe { OwnedHandle::from_raw_handle(process.0) };
    connected_console_host(query_console_host(HANDLE(process.as_raw_handle()))?)
}

fn terminate_process(pid: u32) {
    // SAFETY: 終わらせる権限で開くだけ。
    let process = match unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) } {
        Ok(process) => process,
        Err(e) => {
            tracing::warn!("cannot open the process {pid}: {e}");
            return;
        }
    };
    // SAFETY: OpenProcess が成功したので、ハンドルは有効で、ほかに持ち主はいない。
    let process = unsafe { OwnedHandle::from_raw_handle(process.0) };
    // SAFETY: ハンドルは有効で、終わらせる権限がある。
    let terminated =
        unsafe { TerminateProcess(HANDLE(process.as_raw_handle()), TERMINATED_EXIT_CODE) };
    if let Err(e) = terminated {
        tracing::warn!("failed to terminate the process {pid}: {e}");
    }
}

/// 動いているプロセスの番号。このプロセスを除く。
fn process_ids() -> windows::core::Result<Vec<u32>> {
    // SAFETY: 引数はフラグだけ。
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)? };
    // SAFETY: 成功したので、ハンドルは有効で、ほかに持ち主はいない。
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot.0) };
    let handle = HANDLE(snapshot.as_raw_handle());
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: 引数のない問い合わせ。
    let this_process = unsafe { GetCurrentProcessId() };
    let mut pids = Vec::new();
    // SAFETY: handle は有効で、entry は dwSize を入れてある。
    let mut more = unsafe { Process32FirstW(handle, &mut entry) }.is_ok();
    while more {
        if entry.th32ProcessID != this_process {
            pids.push(entry.th32ProcessID);
        }
        // SAFETY: 同上。
        more = unsafe { Process32NextW(handle, &mut entry) }.is_ok();
    }
    Ok(pids)
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

/// 起動した子プロセス。
struct Spawned {
    process: OwnedHandle,
    #[cfg_attr(not(all(test, feature = "e2e")), expect(dead_code))]
    thread: OwnedHandle,
}

/// `console` に接続した子プロセスを起動する。`flags` は `CreateProcessW` に足すフラグ。
fn spawn_process(
    console: HPCON,
    options: &SpawnOptions,
    flags: PROCESS_CREATION_FLAGS,
) -> io::Result<Spawned> {
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
    let working_folder = options
        .working_folder
        .map(|folder| to_wide_nul(folder.as_os_str()));
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
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | flags,
            Some(environment.as_ptr().cast()),
            working_folder
                .as_ref()
                .map_or(PCWSTR::null(), |folder| PCWSTR(folder.as_ptr())),
            &startup_info.StartupInfo,
            &mut process_info,
        )
    };
    // SAFETY: attributes は初期化済みで、ここで一度だけ破棄する。
    unsafe { DeleteProcThreadAttributeList(attributes) };
    created?;

    // SAFETY: CreateProcessW が成功したので、二つのハンドルは有効で、ほかに持ち主はいない。
    let (process, thread) = unsafe {
        (
            OwnedHandle::from_raw_handle(process_info.hProcess.0),
            OwnedHandle::from_raw_handle(process_info.hThread.0),
        )
    };
    Ok(Spawned { process, thread })
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
            working_folder: None,
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

    #[test]
    fn the_connected_console_host_is_read_from_the_flags() {
        const HOST: usize = 89432;
        assert_eq!(
            connected_console_host(HOST | CONSOLE_HOST_CONNECTED),
            Some(89432)
        );
        // 接続する前とウィンドウのプログラムは、作ったプロセスの番号に別の印が付く。
        assert_eq!(connected_console_host(HOST | 2), None);
        // コンソールを手放した。
        assert_eq!(connected_console_host(CONSOLE_HOST_CONNECTED), None);
        assert_eq!(connected_console_host(0), None);
    }

    /// 子プロセスが接続している conhost を読める。読めないと、閉じたあとに残ったプロセスを終わらせられない。
    #[test]
    fn the_console_host_of_a_child_is_found() {
        let options = SpawnOptions {
            command_line: "cmd.exe /c pause",
            env: &[],
            size: SIZE,
            working_folder: None,
        };
        let mut pty = ConPty::spawn(&options, Arc::new(|| {})).unwrap();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let mut output = Vec::new();
            pty.drain_output(&mut |data| output.extend_from_slice(data));
            if ConsoleHost::of(&pty.process).is_some() {
                return;
            }
            assert!(Instant::now() < deadline, "the child has no console host");
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// 疑似コンソールを閉じたあとに接続したプロセスは、猶予のあとで終わる。プロセスを止めた状態で作り、閉じてから
    /// 動かす。接続できないと新しいコンソールを作るプログラムがありうるので、feature `e2e` のときだけ実行する。
    #[cfg(feature = "e2e")]
    #[test]
    fn a_process_connecting_after_the_console_closed_is_ended() {
        use windows::Win32::System::Threading::{CREATE_SUSPENDED, ResumeThread};

        /// 閉じてから動かすまでの時間。
        const RESUME_AFTER_CLOSING: Duration = Duration::from_millis(200);
        /// 猶予と、残りを探す間隔の分より長くする。
        const END_TIMEOUT_MS: u32 = CLOSE_TIMEOUT_MS + 10_000;

        let shell_options = SpawnOptions {
            command_line: "cmd.exe /k prompt e2e$G",
            env: &[],
            size: SIZE,
            working_folder: None,
        };
        let mut pty = ConPty::spawn(&shell_options, Arc::new(|| {})).unwrap();
        let mut vt = Vt::new(SIZE, "xterm-256color").unwrap();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            pty.drain_output(&mut |data| vt.feed(data));
            if vt.plain_text().unwrap().contains("e2e>") {
                break;
            }
            assert!(Instant::now() < deadline, "the shell did not start");
            thread::sleep(POLL_INTERVAL);
        }
        let late_options = SpawnOptions {
            command_line: "ping.exe -t 127.0.0.1",
            env: &[],
            size: SIZE,
            working_folder: None,
        };
        let late = spawn_process(pty.console, &late_options, CREATE_SUSPENDED).unwrap();

        drop(pty);
        thread::sleep(RESUME_AFTER_CLOSING);
        // SAFETY: thread は止めた状態で作ったスレッドの有効なハンドル。
        unsafe { ResumeThread(HANDLE(late.thread.as_raw_handle())) };

        let process = HANDLE(late.process.as_raw_handle());
        // SAFETY: process は有効なハンドル。
        let ended = unsafe { WaitForSingleObject(process, END_TIMEOUT_MS) } == WAIT_OBJECT_0;
        if !ended {
            // SAFETY: process は有効なハンドルで、自分が作ったプロセス。
            unsafe { TerminateProcess(process, TERMINATED_EXIT_CODE) }.unwrap();
        }
        assert!(ended, "the process connected late did not end");
    }
}
