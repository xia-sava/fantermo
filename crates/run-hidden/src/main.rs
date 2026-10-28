//! 利用者の画面に出ないデスクトップでプログラムを起動し、決めた時間が過ぎたら終わらせる。
//!
//! 画面を撮らずに GUI の動作を確かめるための道具。描画の結果は、起動したプログラム自身に
//! ファイルへ書き出させて確かめる（fantermo なら環境変数 `FANTERMO_DUMP`）。
//!
//! 使い方: `run-hidden [--seconds N] [--env NAME=VALUE]... -- <プログラム> [引数]...`
//!
//! プログラムは `CreateProcessW` のコマンドラインとして渡すので、パスは Windows 形式（`\` 区切り）で書く。
//!
//! 起動したプロセスは Job に入れるので、この道具がどう終わっても道連れに終わる。
//! プログラムが先に終われば、その終了コードを返す。時間切れなら終わらせて 0 を返す。

use std::{
    ffi::{OsStr, OsString},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    time::Duration,
};

use windows::{
    Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            StationsAndDesktops::{CloseDesktop, CreateDesktopW, DESKTOP_CONTROL_FLAGS, HDESK},
            Threading::{
                CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess, PROCESS_INFORMATION,
                ResumeThread, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
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

struct Args {
    seconds: u64,
    env: Vec<(String, String)>,
    command: Vec<OsString>,
}

fn main() {
    let args = match parse_args(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("run-hidden: {message}");
            eprintln!(
                "usage: run-hidden [--seconds N] [--env NAME=VALUE]... -- <program> [args]..."
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
                    env,
                    command,
                });
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

    let desktop_name = format!("run-hidden-{}", std::process::id());
    let desktop = HiddenDesktop::create(&desktop_name)?;
    let job = kill_on_close_job()?;

    let mut desktop_wide = to_wide_nul(OsStr::new(&desktop_name));
    let startup_info = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop_wide.as_mut_ptr()),
        ..Default::default()
    };
    let mut command_line = to_wide_nul(&command_line(&args.command));
    let mut process_info = PROCESS_INFORMATION::default();
    // SAFETY: どのポインタもこの呼び出しの間は生きている。Job に入れる前に走り出さないよう、止めた状態で作る。
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
    // SAFETY: thread は止めた状態で作った主スレッド。
    unsafe { ResumeThread(HANDLE(thread.as_raw_handle())) };

    let timeout = Duration::from_secs(args.seconds);
    // SAFETY: process は有効なハンドル。
    let waited = unsafe { WaitForSingleObject(process_handle, timeout.as_millis() as u32) };
    let code = if waited == WAIT_OBJECT_0 {
        let mut code = 0u32;
        // SAFETY: process は終了済みのプロセスの有効なハンドル。
        unsafe { GetExitCodeProcess(process_handle, &mut code)? };
        eprintln!("run-hidden: exited with code {code}");
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
    fn create(name: &str) -> windows::core::Result<Self> {
        let name = to_wide_nul(OsStr::new(name));
        // SAFETY: name は終端付きの UTF-16 で、この呼び出しの間は生きている。
        let desktop = unsafe {
            CreateDesktopW(
                PCWSTR(name.as_ptr()),
                PCWSTR::null(),
                None,
                DESKTOP_CONTROL_FLAGS(0),
                DESKTOP_ACCESS,
                None,
            )?
        };
        Ok(Self(desktop))
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
