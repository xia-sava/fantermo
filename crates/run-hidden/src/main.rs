//! 利用者の画面に出ないデスクトップでプログラムを起動し、決めた時間が過ぎたら終わらせる。
//!
//! 画面を撮らずに GUI の動作を確認するための道具。描画の結果は、起動したプログラム自身に
//! ファイルへ書き出させて確認する（fantermo なら環境変数 `FANTERMO_DUMP`）。
//!
//! 使い方: `run-hidden [--seconds N] [--wait-children N] [--desktop NAME] [--env NAME=VALUE]... -- <プログラム> [引数]...`
//!
//! プログラムは `CreateProcessW` のコマンドラインとして渡すので、パスは Windows 形式（`\` 区切り）で書く。
//!
//! デスクトップの名前は `run-hidden-<この道具のプロセスの番号>`。`--desktop NAME` を付けると、すでにある別の
//! run-hidden のデスクトップ `NAME` で起動する（なければその名前で作る）。デスクトップの名前とヒープの大きさを
//! 標準エラー出力に出す。
//!
//! 起動したプロセスは Job に入れるので、この道具がどう終わっても一緒に終わる。
//! プログラムが先に終われば、その終了コードを返す。時間切れなら終わらせて 0 を返す。
//!
//! `--wait-children N` を付けると、プログラムが自分で終わったあと、子孫のプロセスが終わるのを N 秒まで待ち、
//! 結果を標準エラー出力に出してから Job を閉じる。Job が終わらせる前に残っていたものが分かる。

use std::{
    ffi::{OsStr, OsString},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    thread,
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation,
                QueryInformationJobObject, SetInformationJobObject,
            },
            StationsAndDesktops::{
                CloseDesktop, CreateDesktopExW, DESKTOP_CONTROL_FLAGS, GetUserObjectInformationW,
                HDESK, UOI_HEAPSIZE,
            },
            Threading::{
                CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess, OpenProcess,
                PROCESS_INFORMATION, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW, ResumeThread, STARTUPINFOW, TerminateProcess,
                WaitForSingleObject,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

/// 待つ時間の既定。
const DEFAULT_SECONDS: u64 = 10;

/// 時間切れで終わらせるときの終了コード。
const TERMINATED_EXIT_CODE: u32 = 0;

/// デスクトップに許す操作（`GENERIC_ALL`）。
const DESKTOP_ACCESS: u32 = 0x1000_0000;

/// 作るデスクトップのヒープの大きさ（KB）。
///
/// 利用者のログオンセッションの `WinSta0` に作るので、指定しなければ対話型のデスクトップの既定（64 ビットの
/// Windows で 20480 KB）になり、利用者のアプリと同じセッションの領域から取る。fantermo の e2e（タブを複数開く、
/// 設定ダイアログ・メニュー・文字のダイアログ・OS のファイルを選ぶダイアログと色を選ぶダイアログを開く、2 つ目の
/// fantermo が同じデスクトップに入る）は 256 KB で通り、128 KB では OS のファイルを選ぶダイアログが開かず、
/// 64 KB では設定ダイアログも作れなかった（`ERROR_NOT_ENOUGH_MEMORY`）。通った大きさの 4 倍にする。
///
/// 足りないと、ウィンドウを作るところで `ERROR_NOT_ENOUGH_MEMORY`（0x80070008）になるか、OS のダイアログが
/// 何も言わずに開かない。どちらもダイアログを待つテストの時間切れとして見える。
const DESKTOP_HEAP_KB: u32 = 1024;

/// Job の中のプロセスの一覧を読む数の上限。
const MAX_LISTED_PROCESSES: usize = 256;

/// Job の中のプロセスが終わったかを確認する間隔。
const CHILDREN_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// プロセスの実行ファイルのパスを読むバッファの長さ。
const IMAGE_PATH_CAPACITY: usize = 1024;

struct Args {
    seconds: u64,
    wait_children: Option<u64>,
    desktop: Option<String>,
    env: Vec<(String, String)>,
    command: Vec<OsString>,
}

fn main() {
    let args = match parse_args(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("run-hidden: {message}");
            eprintln!(
                "usage: run-hidden [--seconds N] [--wait-children N] [--desktop NAME] [--env NAME=VALUE]... -- <program> [args]..."
            );
            std::process::exit(2);
        }
    };
    match run(&args) {
        Ok(code) => std::process::exit(code as i32),
        Err(e) => {
            eprintln!("run-hidden: {e}");
            std::process::exit(1);
        }
    }
}

fn parse_args(mut args: impl Iterator<Item = OsString>) -> Result<Args, String> {
    let mut seconds = DEFAULT_SECONDS;
    let mut wait_children = None;
    let mut desktop = None;
    let mut env = Vec::new();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--") => {
                let command: Vec<OsString> = args.collect();
                if command.is_empty() {
                    return Err("no program given after --".into());
                }
                return Ok(Args {
                    seconds,
                    wait_children,
                    desktop,
                    env,
                    command,
                });
            }
            Some("--desktop") => {
                let value = args.next().ok_or("--desktop needs a name")?;
                let value = value.to_str().ok_or("--desktop must be valid Unicode")?;
                desktop = Some(value.to_owned());
            }
            Some("--wait-children") => {
                let value = args.next().ok_or("--wait-children needs a value")?;
                wait_children = Some(
                    value
                        .to_str()
                        .and_then(|v| v.parse().ok())
                        .ok_or("--wait-children needs a number")?,
                );
            }
            Some("--seconds") => {
                let value = args.next().ok_or("--seconds needs a value")?;
                seconds = value
                    .to_str()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--seconds needs a number")?;
            }
            Some("--env") => {
                let value = args.next().ok_or("--env needs NAME=VALUE")?;
                let value = value.to_str().ok_or("--env must be valid Unicode")?;
                let (name, value) = value.split_once('=').ok_or("--env needs NAME=VALUE")?;
                env.push((name.to_owned(), value.to_owned()));
            }
            _ => return Err(format!("unknown argument {arg:?}")),
        }
    }
    Err("missing -- before the program".into())
}

fn run(args: &Args) -> windows::core::Result<u32> {
    // 環境変数はこのプロセスに設定し、子プロセスに引き継がせる。
    for (name, value) in &args.env {
        // SAFETY: スレッドを立てる前で、環境を読み書きするのはこのスレッドだけ。
        unsafe { std::env::set_var(name, value) };
    }

    let desktop_name = args
        .desktop
        .clone()
        .unwrap_or_else(|| format!("run-hidden-{}", std::process::id()));
    let desktop = HiddenDesktop::create(&desktop_name)?;
    match desktop.heap_kb() {
        Ok(kb) => eprintln!("run-hidden: desktop {desktop_name}, heap {kb} KB"),
        Err(e) => eprintln!("run-hidden: failed to read the desktop heap size: {e}"),
    }
    let job = kill_on_close_job()?;

    let mut desktop_wide = to_wide_nul(OsStr::new(&desktop_name));
    let startup_info = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop_wide.as_mut_ptr()),
        ..Default::default()
    };
    let mut command_line = to_wide_nul(&command_line(&args.command));
    let mut process_info = PROCESS_INFORMATION::default();
    // SAFETY: どのポインタもこの呼び出しの間は生きている。Job に入れる前に動き出さないよう、止めた状態で作る。
    unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_SUSPENDED,
            None,
            PCWSTR::null(),
            &startup_info,
            &mut process_info,
        )?
    };
    // SAFETY: CreateProcessW が成功したので、二つのハンドルは有効で、ほかに持ち主はいない。
    let (process, thread) = unsafe {
        (
            OwnedHandle::from_raw_handle(process_info.hProcess.0),
            OwnedHandle::from_raw_handle(process_info.hThread.0),
        )
    };
    let process_handle = HANDLE(process.as_raw_handle());
    // SAFETY: job と process は有効なハンドル。
    unsafe { AssignProcessToJobObject(HANDLE(job.as_raw_handle()), process_handle)? };
    // SAFETY: thread は止めた状態で作ったメインスレッド。
    unsafe { ResumeThread(HANDLE(thread.as_raw_handle())) };

    let timeout = Duration::from_secs(args.seconds);
    // SAFETY: process は有効なハンドル。
    let waited = unsafe { WaitForSingleObject(process_handle, timeout.as_millis() as u32) };
    let code = if waited == WAIT_OBJECT_0 {
        let mut code = 0u32;
        // SAFETY: process は終了済みのプロセスの有効なハンドル。
        unsafe { GetExitCodeProcess(process_handle, &mut code)? };
        eprintln!("run-hidden: exited with code {code}");
        if let Some(seconds) = args.wait_children {
            report_children(HANDLE(job.as_raw_handle()), Duration::from_secs(seconds));
        }
        code
    } else {
        if waited != WAIT_TIMEOUT {
            eprintln!("run-hidden: wait failed ({waited:?}), terminating");
        } else {
            eprintln!("run-hidden: {} s elapsed, terminating", args.seconds);
        }
        // SAFETY: process は有効なハンドル。Job を閉じれば子孫も終わる。
        unsafe { TerminateProcess(process_handle, TERMINATED_EXIT_CODE)? };
        TERMINATED_EXIT_CODE
    };
    drop(job);
    drop(desktop);
    Ok(code)
}

/// `job` の中のプロセスがなくなるのを `limit` まで待ち、結果を標準エラー出力に出す。
fn report_children(job: HANDLE, limit: Duration) {
    let deadline = Instant::now() + limit;
    loop {
        let remaining = match processes_in_job(job) {
            Ok(remaining) => remaining,
            Err(e) => {
                eprintln!("run-hidden: failed to list the remaining processes: {e}");
                return;
            }
        };
        if remaining.is_empty() {
            eprintln!("run-hidden: no processes remained");
            return;
        }
        if Instant::now() >= deadline {
            let names: Vec<String> = remaining
                .iter()
                .map(|&pid| format!("{pid} {}", process_image(pid)))
                .collect();
            eprintln!("run-hidden: processes remained: {}", names.join(", "));
            return;
        }
        thread::sleep(CHILDREN_POLL_INTERVAL);
    }
}

/// `job` の中で動いているプロセスの番号。
fn processes_in_job(job: HANDLE) -> windows::core::Result<Vec<u32>> {
    // 構造体の末尾が可変長の配列なので、並びの合う `usize` の配列で確保する。
    let words = (size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>()
        + MAX_LISTED_PROCESSES * size_of::<usize>())
    .div_ceil(size_of::<usize>());
    let mut buffer = vec![0usize; words];
    let list = buffer
        .as_mut_ptr()
        .cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
    // SAFETY: buffer は指定した長さの分だけ書き込め、構造体の並びを満たす。
    unsafe {
        QueryInformationJobObject(
            Some(job),
            JobObjectBasicProcessIdList,
            list.cast(),
            (words * size_of::<usize>()) as u32,
            None,
        )?;
    }
    // SAFETY: QueryInformationJobObject が先頭の構造体と、ProcessIdList の NumberOfProcessIdsInList 個を書いた。
    let pids = unsafe {
        std::slice::from_raw_parts(
            (&raw const (*list).ProcessIdList).cast::<usize>(),
            (*list).NumberOfProcessIdsInList as usize,
        )
    };
    Ok(pids.iter().map(|&pid| pid as u32).collect())
}

/// プロセス `pid` の実行ファイルのパス。読めなければその旨の文。
fn process_image(pid: u32) -> String {
    let read = || -> windows::core::Result<String> {
        // SAFETY: 読み取りの権限で開くだけ。
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)? };
        // SAFETY: OpenProcess が成功したので、ハンドルは有効で、ほかに持ち主はいない。
        let process = unsafe { OwnedHandle::from_raw_handle(process.0) };
        let mut path = vec![0u16; IMAGE_PATH_CAPACITY];
        let mut len = path.len() as u32;
        // SAFETY: path は len 文字の分だけ書き込める。
        unsafe {
            QueryFullProcessImageNameW(
                HANDLE(process.as_raw_handle()),
                PROCESS_NAME_WIN32,
                PWSTR(path.as_mut_ptr()),
                &mut len,
            )?;
        }
        Ok(String::from_utf16_lossy(&path[..len as usize]))
    };
    read().unwrap_or_else(|e| format!("(unknown: {e})"))
}

/// 閉じると中のプロセスをすべて終わらせる Job。
fn kill_on_close_job() -> windows::core::Result<OwnedHandle> {
    // SAFETY: 名前のない Job を作るだけ。
    let job = unsafe { CreateJobObjectW(None, PCWSTR::null())? };
    // SAFETY: CreateJobObjectW が成功したので、ハンドルは有効で、ほかに持ち主はいない。
    let job = unsafe { OwnedHandle::from_raw_handle(job.0) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: limits は指定した情報の種類に合う大きさの構造体。
    unsafe {
        SetInformationJobObject(
            HANDLE(job.as_raw_handle()),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )?
    };
    Ok(job)
}

/// 利用者の画面に出ないデスクトップ。破棄すると閉じる。
struct HiddenDesktop(HDESK);

impl HiddenDesktop {
    /// デスクトップ `name` を、ヒープを [`DESKTOP_HEAP_KB`] にして作る。すでにあれば、それを開く
    /// （`CreateDesktopExW` の決まり）。開いたときのヒープは、作ったときの大きさのまま。
    fn create(name: &str) -> windows::core::Result<Self> {
        let name = to_wide_nul(OsStr::new(name));
        // SAFETY: name は終端付きの UTF-16 で、この呼び出しの間は生きている。
        let desktop = unsafe {
            CreateDesktopExW(
                PCWSTR(name.as_ptr()),
                PCWSTR::null(),
                None,
                DESKTOP_CONTROL_FLAGS(0),
                DESKTOP_ACCESS,
                None,
                DESKTOP_HEAP_KB,
                None,
            )?
        };
        Ok(Self(desktop))
    }

    /// デスクトップのヒープの大きさ（KB）。
    fn heap_kb(&self) -> windows::core::Result<u32> {
        let mut size = 0u32;
        // SAFETY: self.0 は開いているデスクトップで、size は指定した長さの分だけ書き込める。
        unsafe {
            GetUserObjectInformationW(
                HANDLE(self.0.0),
                UOI_HEAPSIZE,
                Some((&raw mut size).cast()),
                size_of::<u32>() as u32,
                None,
            )?
        };
        Ok(size)
    }
}

impl Drop for HiddenDesktop {
    fn drop(&mut self) {
        // SAFETY: self.0 は create が作ったデスクトップで、ここで一度だけ閉じる。
        if let Err(e) = unsafe { CloseDesktop(self.0) } {
            eprintln!("run-hidden: failed to close the desktop: {e}");
        }
    }
}

/// 引数を `CreateProcessW` のコマンドラインに組み立てる。空白を含む引数は引用符で囲む。
fn command_line(command: &[OsString]) -> OsString {
    let mut line = OsString::new();
    for (index, arg) in command.iter().enumerate() {
        if index > 0 {
            line.push(" ");
        }
        let text = arg.to_string_lossy();
        if text.is_empty() || text.contains([' ', '\t']) {
            line.push(format!("\"{text}\""));
        } else {
            line.push(arg);
        }
    }
    line
}

fn to_wide_nul(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}
