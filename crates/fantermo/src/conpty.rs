//! ConPTY のバックエンド。
//!
//! 疑似コンソールとは 2 本の匿名パイプで接続する。入力は書くためのスレッドに任せ、ウィンドウのスレッドは
//! チャネルに積むだけにする（conhost は出力の書き込みで止まるとコンソールのロックを持ったままになることがあり、
//! 入力のパイプが読まれなくなりうるため）。出力は
//! 読むためのスレッドで待ってチャネルで渡す。チャネルの中のバイト数は [`OutputBudget`] で上限を置く。
//! 疑似コンソールは子プロセスが終了しても出力のパイプを閉じないので、終了は子プロセスのハンドルを待つ
//! スレッドで知る。
//!
//! 疑似コンソールを閉じると、接続しているプロセスに閉じる知らせが一度届く。そのあとに接続したプロセス（起動の途中
//! だったもの）には届かず、動き続ける。接続し終えないまま止まるプロセスもある。疑似コンソールのプロセス
//! （`conhost.exe`）はそれらを待って残るので、猶予のあとで、そのコンソールに接続しているプロセスと、タブの子孫の
//! うち接続し終えないまま止まっているコンソールのプログラムだけを終わらせる。別のコンソールを持つプロセス、
//! どのコンソールにも接続していないプロセス、ウィンドウのプログラムには触らない。
//!
//! タブの子孫は Job で見分ける。Job はプロセスの一覧を読むためだけに使い、閉じても中のプロセスは終わらない。

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString, c_void},
    fs::File,
    io::{self, Read, Write},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use fantermo_core::{
    backend::{Backend, ExitCode, GridSize, SpawnOptions, Wake},
    config,
};
use windows::{
    Wdk::System::Threading::{NtQueryInformationProcess, PROCESSINFOCLASS},
    Win32::{
        Foundation::{ERROR_MORE_DATA, HANDLE},
        System::{
            Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole},
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
                JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation,
                QueryInformationJobObject, SetInformationJobObject,
            },
            Pipes::CreatePipe,
            Threading::{
                CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
                DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcessId,
                InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
                PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
                PROCESS_TERMINATE, QueryFullProcessImageNameW, ResumeThread, STARTF_USESTDHANDLES,
                STARTUPINFOEXW, UpdateProcThreadAttribute,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

use crate::{
    output_budget::{MAX_PENDING_OUTPUT, OutputBudget},
    process,
};

/// 出力のパイプから一度に読む大きさ。
const READ_BUFFER_SIZE: usize = 64 * 1024;

/// 疑似コンソールを閉じたあと、接続しているプロセスが自分で終わるのを待つ猶予（ミリ秒）。
const CLOSE_TIMEOUT_MS: u32 = 5000;

/// 猶予のあと、遅れて接続するプロセスを終わらせ続ける上限。
const SWEEP_LIMIT: Duration = Duration::from_secs(30);

/// 猶予のあと、コンソールに接続しているプロセスを探す間隔。
const SWEEP_INTERVAL: Duration = Duration::from_millis(200);

/// 猶予のあと、接続し終えないまま止まっているとみなすまでの時間。起動の途中のプロセスはふつう数ミリ秒で接続する。
const STALLED_CONNECTION: Duration = Duration::from_secs(3);

/// [`wait_for_closing_consoles`] が待つ上限。猶予と、止まっているとみなすまでの時間に、終わらせる分を足す。
const CLOSING_AT_EXIT_TIMEOUT: Duration =
    Duration::from_millis(CLOSE_TIMEOUT_MS as u64 + STALLED_CONNECTION.as_millis() as u64 + 2000);

/// Job の中のプロセスの一覧を、まず何個の分の領域で読むか。入りきらなければ広げて読み直す。
const INITIAL_JOB_PROCESSES: usize = 1024;

/// Job の中のプロセスの一覧を読む回数の上限。読み直す間にもプロセスが増えうるので、一度で足りるとは限らない。
const JOB_PROCESS_LIST_ATTEMPTS: usize = 3;

/// 実行ファイルの先頭から、サブシステムを読むために読む長さ。
const IMAGE_HEADER_LEN: usize = 4096;
/// `IMAGE_DOS_HEADER` の `e_lfanew` の位置。
const PE_OFFSET_FIELD: usize = 0x3c;
/// `IMAGE_NT_HEADERS` の先頭から `OptionalHeader` までの長さ（署名 4 バイトと `IMAGE_FILE_HEADER` 20 バイト）。
const OPTIONAL_HEADER_OFFSET: usize = 24;
/// `OptionalHeader` の先頭から `Subsystem` までの長さ（32 ビットと 64 ビットで同じ）。
const SUBSYSTEM_OFFSET: usize = 68;
/// `IMAGE_SUBSYSTEM_WINDOWS_CUI`。コンソールのプログラム。
const SUBSYSTEM_CONSOLE: u16 = 3;
/// 実行ファイルのパスを読むバッファの長さ。
const IMAGE_PATH_CAPACITY: usize = 1024;

/// `NtQueryInformationProcess` の `ProcessConsoleHostProcess`。値はコンソールのプロセスの番号と、下位 2 ビットの印。
/// Windows SDK のヘッダーにはない。
const PROCESS_CONSOLE_HOST_PROCESS: PROCESSINFOCLASS = PROCESSINFOCLASS(49);

/// `ProcessConsoleHostProcess` の値の下位 2 ビット。接続したら conhost の番号と 1、接続する前は作ったプロセスの番号と 2
/// （ウィンドウのプログラムはそのまま）。
const CONSOLE_HOST_CONNECTED: usize = 1;
const CONSOLE_HOST_NOT_CONNECTED: usize = 2;
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
    /// 書くためのスレッドへ渡す入力。
    input: mpsc::Sender<Vec<u8>>,
    /// 破棄した。書くためのスレッドは、書いている途中のものを書き終えたら、残りを捨てて抜ける。
    closing: Arc<AtomicBool>,
    process: Arc<OwnedHandle>,
    /// 子プロセスとその子孫が入る Job。作れなければ `None` で、閉じたあとは接続したプロセスだけを探す。
    job: Option<OwnedHandle>,
    output: mpsc::Receiver<Vec<u8>>,
    /// `output` に入っていて、まだ受け取っていないバイト数。
    budget: Arc<OutputBudget>,
    reader: Option<thread::JoinHandle<()>>,
    exit: Option<ExitCode>,
}

impl ConPty {
    /// `options` のコマンドラインを疑似コンソールに接続して起動する。コマンドラインは
    /// `CreateProcessW` にそのまま渡す。最初の語が名前だけなら、実行ファイルは [`program_to_start`] で決める。
    pub fn spawn(options: &SpawnOptions, wake: Wake) -> io::Result<Self> {
        let application = program_to_start(options)?;
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

        // Job に入れる前に子孫を作らないよう、止めた状態で作る。
        let (process, job) =
            match spawn_process(console, application.as_deref(), options, CREATE_SUSPENDED)
                .and_then(start_in_job)
            {
                Ok((process, job)) => (Arc::new(process), job),
                Err(e) => {
                    // SAFETY: console は作ったばかりで、まだ誰も閉じていない。
                    unsafe { ClosePseudoConsole(console) };
                    return Err(e);
                }
            };

        // スレッドを作れなければ、組み立てた Self を drop して、疑似コンソールと子プロセスを Drop で片づける。
        let (sender, receiver) = mpsc::channel();
        let (input_sender, queued) = mpsc::channel();
        let closing = Arc::new(AtomicBool::new(false));
        let budget = Arc::new(OutputBudget::new(MAX_PENDING_OUTPUT));
        let mut pty = Self {
            console,
            console_size,
            input: input_sender,
            closing: Arc::clone(&closing),
            process,
            job,
            output: receiver,
            budget: Arc::clone(&budget),
            reader: None,
            exit: None,
        };
        thread::Builder::new().name("conpty-writer".into()).spawn({
            let input = File::from(input);
            move || write_input(input, &queued, &closing)
        })?;
        pty.reader = Some(thread::Builder::new().name("conpty-reader".into()).spawn({
            let wake = Arc::clone(&wake);
            move || read_output(File::from(output), &sender, &budget, &*wake)
        })?);
        thread::Builder::new()
            .name("conpty-exit-watcher".into())
            .spawn({
                let process = Arc::clone(&pty.process);
                move || {
                    process::wait(&process, None);
                    wake();
                }
            })?;
        Ok(pty)
    }
}

impl Backend for ConPty {
    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.input
            .send(data.to_vec())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }

    fn resize(&mut self, size: GridSize) -> io::Result<()> {
        let console_size = coord(size);
        if console_size == self.console_size {
            return Ok(());
        }
        // ResizePseudoConsole は conhost の signal のパイプへ小さな知らせを書くだけで、パイプが埋まるには数百回の
        // 変更が要るので、ウィンドウのスレッドから呼んで待たずに戻るものとして扱う。
        // SAFETY: self.console は Drop まで閉じない。
        unsafe { ResizePseudoConsole(self.console, console_size) }.map_err(io::Error::from)?;
        self.console_size = console_size;
        Ok(())
    }

    fn drain_output(&mut self, limit: usize, sink: &mut dyn FnMut(&[u8])) -> bool {
        self.budget.receiving();
        let mut passed = 0usize;
        while passed < limit {
            let Ok(chunk) = self.output.try_recv() else {
                return false;
            };
            sink(&chunk);
            self.budget.drained(chunk.len());
            passed = passed.saturating_add(chunk.len());
        }
        true
    }

    fn poll_exit(&mut self) -> Option<ExitCode> {
        if self.exit.is_some() {
            return self.exit;
        }
        if !process::wait(&self.process, Some(Duration::ZERO)) {
            return None;
        }
        let code = process::exit_code(&self.process).unwrap_or_else(|e| {
            tracing::warn!("failed to get the child exit code: {e}");
            0
        });
        self.exit = Some(ExitCode(code));
        self.exit
    }
}

impl Drop for ConPty {
    /// 接続の途中のプロセスがあると、疑似コンソールを閉じても子プロセスと conhost が残り、出力のパイプが閉じない
    /// ことがある。呼び出し元（ウィンドウのスレッド）はすぐに戻り、閉じるのは別のスレッドでする。そのスレッドを
    /// 作れなければ、疑似コンソールを閉じることだけをその場でする。
    fn drop(&mut self) {
        // 書くためのスレッドは、書いている途中のものを書き終えたら残りを捨てて抜ける。conhost が読まずに
        // 書き込みが返らなくても、閉じるスレッドが疑似コンソールを閉じて conhost が終われば、入力のパイプが
        // 壊れて返る。conhost が終わらなければ書くためのスレッドも残る（読むためのスレッドを手放すのと同じ）。
        self.closing.store(true, Ordering::Relaxed);
        // 上限で待っている読むためのスレッドを抜けさせる。抜けると出力のパイプの読む側が閉じるので、
        // close_console が読むためのスレッドの終わりを待っても止まらない。
        self.budget.close();
        let console = self.console.0;
        let process = Arc::clone(&self.process);
        let job = self.job.take();
        let reader = self.reader.take();
        let count = ClosingCount::new();
        let closing = thread::Builder::new()
            .name("conpty-closer".into())
            .spawn(move || {
                let _count = count;
                close_console(HPCON(console), &process, job.as_ref(), reader);
            });
        if let Err(e) = closing {
            tracing::warn!("failed to start closing the pseudo console; closing it here: {e}");
            // SAFETY: スレッドを作れなかったので、console を閉じるものはほかになく、ここで一度だけ閉じる。
            unsafe { ClosePseudoConsole(HPCON(console)) };
        }
    }
}

/// 疑似コンソールを閉じ、接続しているプロセスが終わらなければ終わらせる。conhost が終わったと分かったときだけ、
/// 読むためのスレッドが抜けるのを待つ。conhost の番号を読めなかったときや、[`SWEEP_LIMIT`] までに終わらなかった
/// ときは、出力のパイプが閉じるとは限らないので、読むためのスレッドは待たずに手放す。
fn close_console(
    console: HPCON,
    process: &OwnedHandle,
    job: Option<&OwnedHandle>,
    reader: Option<thread::JoinHandle<()>>,
) {
    // console が閉じるまでは conhost が残っているので、番号が別のプロセスのものになっていない。
    let host_before_closing = ConsoleHost::of(process);
    // 疑似コンソールを閉じると、接続しているプロセスがなくなったところで出力のパイプも閉じ、読むためのスレッドが抜ける。
    // SAFETY: console は Drop がこのスレッドに渡したもので、ここで一度だけ閉じる。
    unsafe { ClosePseudoConsole(console) };

    let close_timeout = Duration::from_millis(CLOSE_TIMEOUT_MS.into());
    let deadline = Instant::now() + close_timeout;
    let child_ended = process::wait(process, Some(close_timeout));
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
        if let Err(e) = process::terminate(process) {
            tracing::warn!("failed to terminate the child: {e}");
        }
    }
    let host_ended = match &host {
        Some(host) => host.end_clients(job),
        None => false,
    };
    let Some(reader) = reader else {
        return;
    };
    if !host_ended {
        tracing::debug!("leaving the conpty reader thread without knowing that its console ended");
        return;
    }
    if reader.join().is_err() {
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
        // 終わるのを待つ権限で開く。
        let process = match process::open(pid, PROCESS_SYNCHRONIZE) {
            Ok(process) => process,
            Err(e) => {
                tracing::debug!("cannot open the console host {pid}: {e}");
                return None;
            }
        };
        Some(Self { pid, process })
    }

    /// 終わったか。`timeout` まで待つ。
    fn wait(&self, timeout: Duration) -> bool {
        process::wait(&self.process, Some(timeout))
    }

    fn wait_until(&self, deadline: Instant) {
        self.wait(deadline.saturating_duration_since(Instant::now()));
    }

    /// このコンソールに接続しているプロセスと、`job` の中で接続し終えないまま止まっているコンソールのプログラムを、
    /// conhost が終わるまで終わらせ続ける。conhost が [`SWEEP_LIMIT`] までに終われば真。
    fn end_clients(&self, job: Option<&OwnedHandle>) -> bool {
        let deadline = Instant::now() + SWEEP_LIMIT;
        let mut connecting = HashMap::new();
        while !self.wait(Duration::ZERO) {
            if Instant::now() >= deadline {
                tracing::warn!(
                    "the console host {} did not end; processes of the tab: {:?}",
                    self.pid,
                    job.map(describe_job_processes)
                );
                return false;
            }
            self.terminate_clients();
            if let Some(job) = job {
                terminate_stalled_connections(job, &mut connecting);
            }
            self.wait(SWEEP_INTERVAL);
        }
        true
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

/// `job` の中で、接続する前の値のまま [`STALLED_CONNECTION`] を過ぎたコンソールのプログラムを終わらせる。
/// `connecting` は、接続する前の値で見つけたプロセスのハンドル（番号が使い回されないよう持っておく）と、見つけた時刻。
fn terminate_stalled_connections(
    job: &OwnedHandle,
    connecting: &mut HashMap<u32, (OwnedHandle, Instant)>,
) {
    let pids = match job_process_ids(job) {
        Ok(pids) => pids,
        Err(e) => {
            tracing::warn!("failed to list the processes of the tab: {e}");
            return;
        }
    };
    connecting.retain(|pid, _| pids.contains(pid));
    for pid in pids {
        let Ok(process) = process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE)
        else {
            continue;
        };
        let handle = HANDLE(process.as_raw_handle());
        let not_connected = query_console_host(handle)
            .is_some_and(|value| value & CONSOLE_HOST_FLAG_MASK == CONSOLE_HOST_NOT_CONNECTED);
        if !not_connected || !is_console_program(handle) {
            connecting.remove(&pid);
            continue;
        }
        let Some((held, since)) = connecting.get(&pid) else {
            connecting.insert(pid, (process, Instant::now()));
            continue;
        };
        if since.elapsed() < STALLED_CONNECTION {
            continue;
        }
        tracing::warn!(
            "ending the process {pid}, which did not finish connecting to the closed console"
        );
        // held は終わらせる権限で開いてある。
        if let Err(e) = process::terminate(held) {
            tracing::warn!("failed to terminate the process {pid}: {e}");
        }
        connecting.remove(&pid);
    }
}

/// 残ったときのログ用に、`job` の中のプロセスの番号と `ProcessConsoleHostProcess` の値を並べる。
fn describe_job_processes(job: &OwnedHandle) -> Vec<(u32, Option<usize>)> {
    job_process_ids(job)
        .unwrap_or_default()
        .into_iter()
        .map(|pid| {
            let value = process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION)
                .ok()
                .and_then(|process| query_console_host(HANDLE(process.as_raw_handle())));
            (pid, value)
        })
        .collect()
}

/// 子プロセスとその子孫を入れる Job を作り、止めた状態で作った子プロセスを入れてから動かす。Job を作れないか
/// 入れられなければ、Job なしで動かす。
fn start_in_job(spawned: Spawned) -> io::Result<(OwnedHandle, Option<OwnedHandle>)> {
    let job = match tab_job() {
        Ok(job) => {
            // SAFETY: job と process は有効なハンドル。
            let assigned = unsafe {
                AssignProcessToJobObject(
                    HANDLE(job.as_raw_handle()),
                    HANDLE(spawned.process.as_raw_handle()),
                )
            };
            match assigned {
                Ok(()) => Some(job),
                Err(e) => {
                    tracing::warn!("failed to put the shell in a job: {e}");
                    None
                }
            }
        }
        Err(e) => {
            tracing::warn!("failed to create a job for the tab: {e}");
            None
        }
    };
    // SAFETY: thread は止めた状態で作った子プロセスのメインスレッドの有効なハンドル。
    if unsafe { ResumeThread(HANDLE(spawned.thread.as_raw_handle())) } == u32::MAX {
        let error = io::Error::last_os_error();
        if let Err(e) = process::terminate(&spawned.process) {
            tracing::warn!("failed to terminate the child that did not start: {e}");
        }
        return Err(error);
    }
    Ok((spawned.process, job))
}

/// タブの子孫を見分けるための Job。中のプロセスは、ほかの Job へ抜けられる（`CREATE_BREAKAWAY_FROM_JOB`）。
/// 閉じても中のプロセスは終わらない。
fn tab_job() -> windows::core::Result<OwnedHandle> {
    // SAFETY: 名前のない Job を作るだけ。
    let job = unsafe { CreateJobObjectW(None, PCWSTR::null())? };
    // SAFETY: CreateJobObjectW が成功したので、ハンドルは有効で、ほかに持ち主はいない。
    let job = unsafe { OwnedHandle::from_raw_handle(job.0) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    // SAFETY: limits は指定した情報の種類に合う大きさの構造体。
    unsafe {
        SetInformationJobObject(
            HANDLE(job.as_raw_handle()),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )?;
    }
    Ok(job)
}

/// `job` の中で動いているプロセスの番号。
fn job_process_ids(job: &OwnedHandle) -> windows::core::Result<Vec<u32>> {
    job_process_ids_with_capacity(job, INITIAL_JOB_PROCESSES)
}

/// [`job_process_ids`] の中身。まず `capacity` 個の分の領域で読み、入りきらなければ（`ERROR_MORE_DATA`）、
/// 領域を倍と Job の中のプロセスの数の大きい方まで広げて、合わせて [`JOB_PROCESS_LIST_ATTEMPTS`] 回まで読む。
fn job_process_ids_with_capacity(
    job: &OwnedHandle,
    capacity: usize,
) -> windows::core::Result<Vec<u32>> {
    let list_offset = std::mem::offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList);
    let mut capacity = capacity.max(1);
    let mut attempts = 1;
    loop {
        // 構造体の末尾が可変長の配列なので、並びの合う `usize` の配列で確保する。
        let words = (list_offset + capacity * size_of::<usize>()).div_ceil(size_of::<usize>());
        let mut buffer = vec![0usize; words];
        let list = buffer
            .as_mut_ptr()
            .cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
        // SAFETY: buffer は渡す長さの分だけ書き込め、構造体の大きさ以上で、構造体の並びを満たす。
        let queried = unsafe {
            QueryInformationJobObject(
                Some(HANDLE(job.as_raw_handle())),
                JobObjectBasicProcessIdList,
                list.cast(),
                (words * size_of::<usize>()) as u32,
                None,
            )
        };
        match queried {
            Ok(()) => {}
            Err(e)
                if e.code() == ERROR_MORE_DATA.to_hresult()
                    && attempts < JOB_PROCESS_LIST_ATTEMPTS =>
            {
                // SAFETY: 先頭の構造体は buffer の中にあり、0 で埋めてあるか QueryInformationJobObject が書いた。
                let assigned = unsafe { (*list).NumberOfAssignedProcesses } as usize;
                capacity = capacity.saturating_mul(2).max(assigned);
                attempts += 1;
                continue;
            }
            Err(e) => return Err(e),
        }
        let slots = (words * size_of::<usize>() - list_offset) / size_of::<usize>();
        // SAFETY: 先頭の構造体と、ProcessIdList から slots 個は buffer の中にあり、0 で埋めてあるか
        // QueryInformationJobObject が書いた。読む数は slots を超えない。
        let pids = unsafe {
            std::slice::from_raw_parts(
                (&raw const (*list).ProcessIdList).cast::<usize>(),
                ((*list).NumberOfProcessIdsInList as usize).min(slots),
            )
        };
        return Ok(pids.iter().map(|&pid| pid as u32).collect());
    }
}

/// プロセスがコンソールのプログラムか。実行ファイルの PE ヘッダーのサブシステムで見る。読めなければ偽。
fn is_console_program(process: HANDLE) -> bool {
    let mut path = vec![0u16; IMAGE_PATH_CAPACITY];
    let mut len = path.len() as u32;
    // SAFETY: path は len 文字の分だけ書き込める。
    if let Err(e) = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut len,
        )
    } {
        tracing::debug!("cannot read the image path: {e}");
        return false;
    }
    let path = OsString::from_wide(&path[..len as usize]);
    let mut header = vec![0u8; IMAGE_HEADER_LEN];
    let read = File::open(&path).and_then(|mut file| file.read(&mut header));
    match read {
        Ok(read) => image_subsystem(&header[..read]) == Some(SUBSYSTEM_CONSOLE),
        Err(e) => {
            tracing::debug!("cannot read {}: {e}", path.to_string_lossy());
            false
        }
    }
}

/// 実行ファイルの先頭のバイト列から、PE のサブシステムを読む。
fn image_subsystem(header: &[u8]) -> Option<u16> {
    let read_u16 = |at: usize| Some(u16::from_le_bytes(header.get(at..at + 2)?.try_into().ok()?));
    let read_u32 = |at: usize| Some(u32::from_le_bytes(header.get(at..at + 4)?.try_into().ok()?));
    if header.get(..2)? != b"MZ" {
        return None;
    }
    let nt_headers = usize::try_from(read_u32(PE_OFFSET_FIELD)?).ok()?;
    if header.get(nt_headers..nt_headers + 4)? != b"PE\0\0" {
        return None;
    }
    read_u16(nt_headers + OPTIONAL_HEADER_OFFSET + SUBSYSTEM_OFFSET)
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
    let process = process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
    connected_console_host(query_console_host(HANDLE(process.as_raw_handle()))?)
}

fn terminate_process(pid: u32) {
    let process = match process::open(pid, PROCESS_TERMINATE) {
        Ok(process) => process,
        Err(e) => {
            tracing::warn!("cannot open the process {pid}: {e}");
            return;
        }
    };
    if let Err(e) = process::terminate(&process) {
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
    thread: OwnedHandle,
}

/// `console` に接続した子プロセスを起動する。`application` は `CreateProcessW` の `lpApplicationName`
/// （[`program_to_start`] で決めたもの）、`flags` は `CreateProcessW` に足すフラグ。
fn spawn_process(
    console: HPCON,
    application: Option<&Path>,
    options: &SpawnOptions,
    flags: PROCESS_CREATION_FLAGS,
) -> io::Result<Spawned> {
    let application = application.map(|path| to_wide_nul(path.as_os_str()));

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
            application
                .as_ref()
                .map_or(PCWSTR::null(), |path| PCWSTR(path.as_ptr())),
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

/// `CreateProcessW` の `lpApplicationName` に渡す実行ファイル。
///
/// コマンドラインの最初の語が名前だけなら、子プロセスに渡す環境の `PATH` から探した絶対パス。見つからなければ
/// 起動の失敗にする。`lpApplicationName` を渡さないと、`CreateProcessW` は fantermo の実行ファイルのフォルダと、
/// fantermo 自身の作業フォルダ（タブに渡す作業フォルダではなく、fantermo を起動したときのフォルダ）も探すので、
/// そこに置かれた同じ名前のファイルを起動しうる。最初の語にフォルダかドライブ名があれば
/// `None` で、`CreateProcessW` に任せる。
fn program_to_start(options: &SpawnOptions) -> io::Result<Option<PathBuf>> {
    let (program, _) = config::split_program(options.command_line);
    if !config::is_bare_program_name(program) {
        return Ok(None);
    }
    let path_variable = child_path_variable(options.env);
    config::find_program_on_path(program, path_variable.as_deref(), Path::is_file)
        .map(Some)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("「{program}」が環境変数 PATH のフォルダに見つかりません。"),
            )
        })
}

/// 子プロセスに渡す環境の `PATH`。`overrides` になければこのプロセスのもの（[`environment_block`] と同じ）。
fn child_path_variable(overrides: &[(&str, &str)]) -> Option<OsString> {
    const PATH: &str = "PATH";
    match overrides
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(PATH))
    {
        Some((_, value)) => Some(OsString::from(value)),
        None => std::env::var_os(PATH),
    }
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

/// `queued` の入力を `input` に書く。チャネルが閉じたら抜ける。`closing` が立ったら（破棄したら）、受け取った
/// ものと残りは捨てて抜ける。conhost が読まずに書き込みが返らない間も、ウィンドウのスレッドは待たない。
fn write_input(mut input: impl Write, queued: &mpsc::Receiver<Vec<u8>>, closing: &AtomicBool) {
    for bytes in queued {
        if closing.load(Ordering::Relaxed) {
            return;
        }
        if let Err(e) = input.write_all(&bytes) {
            // 疑似コンソールを閉じたあと。
            if e.kind() == io::ErrorKind::BrokenPipe {
                tracing::debug!("the pseudo console has closed its input: {e}");
            } else {
                tracing::warn!("failed to write to the pseudo console: {e}");
            }
            return;
        }
    }
}

/// 出力のパイプが閉じるまで読み、読めた分をチャネルで渡す。まだ受け取られていない分が `budget` の上限に
/// 達していれば、受け取られるまで待つ。`budget` を閉じたら（破棄したら）抜ける。`wake` は、まだ知らせて
/// いないか、前の知らせのあとにウィンドウのスレッドが受け取り始めていたときだけ呼ぶ（[`OutputBudget::sent`]）。
fn read_output(
    mut output: File,
    sender: &mpsc::Sender<Vec<u8>>,
    budget: &OutputBudget,
    wake: &dyn Fn() -> bool,
) {
    let mut buf = vec![0u8; READ_BUFFER_SIZE];
    loop {
        match output.read(&mut buf) {
            Ok(0) => return,
            Ok(len) => {
                if !budget.push(len) || sender.send(buf[..len].to_vec()).is_err() {
                    return;
                }
                budget.sent(wake);
            }
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return,
            Err(e) => {
                tracing::warn!("failed to read from the pseudo console: {e}");
                return;
            }
        }
    }
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
        run_options_until(
            &SpawnOptions {
                command_line,
                env,
                size: SIZE,
                working_folder: None,
            },
            expected,
        )
    }

    /// [`run_until`] と同じだが、起動の指定を `options` で渡す。
    fn run_options_until(options: &SpawnOptions, expected: &str) -> (String, ExitCode) {
        let mut pty = ConPty::spawn(options, Arc::new(|| true)).unwrap();
        let mut vt = Vt::new(SIZE, "xterm-256color").unwrap();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            pty.drain_output(usize::MAX, &mut |data| vt.feed(data));
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
    fn write_input_writes_in_order_and_nothing_once_closing() {
        let closing = AtomicBool::new(false);
        let mut written = Vec::new();
        let (sender, queued) = mpsc::channel();
        sender.send(b"ab".to_vec()).unwrap();
        sender.send(b"cd".to_vec()).unwrap();
        drop(sender);
        write_input(&mut written, &queued, &closing);
        assert_eq!(written, b"abcd");

        written.clear();
        closing.store(true, Ordering::Relaxed);
        let (sender, queued) = mpsc::channel();
        sender.send(b"after".to_vec()).unwrap();
        drop(sender);
        write_input(&mut written, &queued, &closing);
        assert!(written.is_empty());
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
    fn a_bare_program_missing_from_path_fails_to_start() {
        let options = SpawnOptions {
            command_line: "no-such-program-for-fantermo -x",
            env: &[],
            size: SIZE,
            working_folder: None,
        };
        let Err(e) = ConPty::spawn(&options, Arc::new(|| true)) else {
            panic!("a missing program started");
        };
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert!(
            e.to_string().contains("「no-such-program-for-fantermo」"),
            "{e}"
        );
    }

    /// 名前だけのバッチファイルも、PATH で見つけたものが動き、作業フォルダの同じ名前のものは動かない。
    /// `CreateProcessW` がバッチファイルのために起動する `cmd.exe` が、渡した絶対パスを使うことを見る。
    #[test]
    fn a_bare_batch_file_runs_the_one_found_on_path() {
        let root =
            std::env::temp_dir().join(format!("fantermo-conpty-batch-{}", std::process::id()));
        let on_path = root.join("a");
        let working_folder = root.join("b");
        for (folder, marker) in [(&on_path, "marker-a"), (&working_folder, "marker-b")] {
            std::fs::create_dir_all(folder).unwrap();
            std::fs::write(folder.join("tool.bat"), format!("@echo {marker}\r\n")).unwrap();
        }
        let system = std::env::var_os("SystemRoot").expect("no SystemRoot");
        let path_variable = format!(
            "{};{}",
            on_path.display(),
            Path::new(&system).join("System32").display()
        );
        let env = [("PATH", path_variable.as_str())];
        let (screen, _) = run_options_until(
            &SpawnOptions {
                command_line: "tool.bat x",
                env: &env,
                size: SIZE,
                working_folder: Some(working_folder.as_path()),
            },
            "marker-",
        );
        _ = std::fs::remove_dir_all(&root);
        assert!(screen.contains("marker-a"), "{screen}");
        assert!(!screen.contains("marker-b"), "{screen}");
    }

    #[test]
    fn the_child_path_comes_from_the_overrides_first() {
        assert_eq!(
            child_path_variable(&[("Path", r"C:\only")]),
            Some(OsString::from(r"C:\only"))
        );
        assert_eq!(child_path_variable(&[]), std::env::var_os("PATH"));
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

    #[test]
    fn the_subsystem_of_an_image_is_read() {
        let system = std::env::var_os("SystemRoot").expect("no SystemRoot");
        let read = |name: &str| {
            let path = std::path::Path::new(&system).join("System32").join(name);
            let bytes = std::fs::read(&path).unwrap();
            image_subsystem(&bytes[..IMAGE_HEADER_LEN.min(bytes.len())])
        };
        assert_eq!(read("cmd.exe"), Some(SUBSYSTEM_CONSOLE));
        // ウィンドウのプログラム（IMAGE_SUBSYSTEM_WINDOWS_GUI）。
        assert_eq!(read("wscript.exe"), Some(2));
        assert_eq!(image_subsystem(b"MZ"), None);
        assert_eq!(image_subsystem(b"not an image"), None);
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
        let mut pty = ConPty::spawn(&options, Arc::new(|| true)).unwrap();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let mut output = Vec::new();
            pty.drain_output(usize::MAX, &mut |data| output.extend_from_slice(data));
            if ConsoleHost::of(&pty.process).is_some() {
                return;
            }
            assert!(Instant::now() < deadline, "the child has no console host");
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// Job の中のプロセスが用意した領域に入りきらなくても、領域を広げて全部を読む。入りきらないと
    /// `QueryInformationJobObject` が `ERROR_MORE_DATA` で失敗することも見る。
    #[test]
    fn the_processes_of_a_job_are_read_beyond_the_initial_capacity() {
        /// ping が送る回数（1 秒に 1 回）。テストの間は動いていて、片づけに失敗しても自分で終わる。
        const PING_COUNT: u32 = 30;

        let command_line = format!("cmd.exe /c ping.exe -n {PING_COUNT} 127.0.0.1");
        let options = SpawnOptions {
            command_line: &command_line,
            env: &[],
            size: SIZE,
            working_folder: None,
        };
        let mut pty = ConPty::spawn(&options, Arc::new(|| true)).unwrap();
        let job = pty.job.take().expect("the child is not in a job");
        let deadline = Instant::now() + TIMEOUT;
        let all = loop {
            pty.drain_output(usize::MAX, &mut |_| {});
            let pids = job_process_ids(&job).unwrap();
            // cmd と ping。
            if pids.len() >= 2 {
                break pids;
            }
            assert!(Instant::now() < deadline, "the job has {pids:?}");
            thread::sleep(POLL_INTERVAL);
        };

        let mut list = JOBOBJECT_BASIC_PROCESS_ID_LIST::default();
        // SAFETY: list は渡す長さの分だけ書き込める。
        let queried = unsafe {
            QueryInformationJobObject(
                Some(HANDLE(job.as_raw_handle())),
                JobObjectBasicProcessIdList,
                (&raw mut list).cast(),
                size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() as u32,
                None,
            )
        };
        assert_eq!(
            queried.map_err(|e| e.code()),
            Err(ERROR_MORE_DATA.to_hresult())
        );

        let read = job_process_ids_with_capacity(&job, 1).unwrap();
        for pid in &all {
            assert!(read.contains(pid), "{pid} is missing from {read:?}");
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
        const END_TIMEOUT: Duration = Duration::from_millis(CLOSE_TIMEOUT_MS as u64 + 10_000);

        let shell_options = SpawnOptions {
            command_line: "cmd.exe /k prompt e2e$G",
            env: &[],
            size: SIZE,
            working_folder: None,
        };
        let mut pty = ConPty::spawn(&shell_options, Arc::new(|| true)).unwrap();
        let mut vt = Vt::new(SIZE, "xterm-256color").unwrap();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            pty.drain_output(usize::MAX, &mut |data| vt.feed(data));
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
        let application = program_to_start(&late_options).unwrap();
        let late = spawn_process(
            pty.console,
            application.as_deref(),
            &late_options,
            CREATE_SUSPENDED,
        )
        .unwrap();

        drop(pty);
        thread::sleep(RESUME_AFTER_CLOSING);
        // SAFETY: thread は止めた状態で作ったスレッドの有効なハンドル。
        unsafe { ResumeThread(HANDLE(late.thread.as_raw_handle())) };

        let ended = process::wait(&late.process, Some(END_TIMEOUT));
        if !ended {
            process::terminate(&late.process).unwrap();
        }
        assert!(ended, "the process connected late did not end");
    }
}
