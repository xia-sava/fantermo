//! プロセスのハンドルを開く・待つ・終了コードを読む・終わらせる処理と、子プロセスの出力のパイプを閉じるまで
//! 読む処理。ConPTY とブリッジのバックエンドと、起動の指定を渡す先のウィンドウを選ぶ処理が使う。
//!
//! 失敗したときのログは、何のプロセスかが分かる呼び出し元で残すので、ここではエラーを返すだけにする。

use std::{
    io::{self, Read},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    time::Duration,
};

use windows::Win32::{
    Foundation::{HANDLE, WAIT_OBJECT_0},
    System::Threading::{
        GetExitCodeProcess, INFINITE, OpenProcess, PROCESS_ACCESS_RIGHTS, TerminateProcess,
        WaitForSingleObject,
    },
};

/// fantermo がプロセスを終わらせるときの終了コード。
pub const TERMINATED_EXIT_CODE: u32 = 1;

/// プロセス `pid` を `access` の権限で開く。
pub fn open(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> windows::core::Result<OwnedHandle> {
    // SAFETY: 指定した権限で開くだけ。
    let process = unsafe { OpenProcess(access, false, pid)? };
    // SAFETY: OpenProcess が成功したので、ハンドルは有効で、ほかに持ち主はいない。
    Ok(unsafe { OwnedHandle::from_raw_handle(process.0) })
}

/// プロセスが終わるのを `timeout` まで（`None` なら終わるまで）待つ。終わったら `true`。ミリ秒で表せない
/// 長さは、終わるまで待つ。
pub fn wait(process: &OwnedHandle, timeout: Option<Duration>) -> bool {
    let millis = timeout.map_or(INFINITE, |timeout| {
        u32::try_from(timeout.as_millis()).unwrap_or(INFINITE)
    });
    // SAFETY: process は呼び出し元が持つ有効なプロセスのハンドル。
    unsafe { WaitForSingleObject(handle(process), millis) == WAIT_OBJECT_0 }
}

/// 終わったプロセスの終了コード。
pub fn exit_code(process: &OwnedHandle) -> windows::core::Result<u32> {
    let mut code = 0u32;
    // SAFETY: process は有効なプロセスのハンドルで、code は書き込める。
    unsafe { GetExitCodeProcess(handle(process), &mut code)? };
    Ok(code)
}

/// プロセスを [`TERMINATED_EXIT_CODE`] で終わらせる。`process` は終わらせる権限を持つハンドル。
pub fn terminate(process: &OwnedHandle) -> windows::core::Result<()> {
    // SAFETY: process は呼び出し元が持つ有効なプロセスのハンドル。
    unsafe { TerminateProcess(handle(process), TERMINATED_EXIT_CODE) }
}

/// 子プロセスの出力のパイプ `reader` を、閉じるまで `buffer_size` バイトずつ読んで `on_chunk` に渡す。
/// `on_chunk` が `false` を返したら、そこで読むのをやめる。パイプが閉じたか読めなくなって抜けたら `true`、
/// `on_chunk` が止めたら `false`。読めなかったときは `source`（読む相手の名前）を添えてログに残す。
pub fn read_until_closed(
    mut reader: impl Read,
    buffer_size: usize,
    source: &str,
    mut on_chunk: impl FnMut(&[u8]) -> bool,
) -> bool {
    let mut buf = vec![0u8; buffer_size];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return true,
            Ok(len) => {
                if !on_chunk(&buf[..len]) {
                    return false;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return true,
            Err(e) => {
                tracing::warn!("failed to read from {source}: {e}");
                return true;
            }
        }
    }
}

fn handle(process: &OwnedHandle) -> HANDLE {
    HANDLE(process.as_raw_handle())
}

#[cfg(test)]
mod tests {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };

    use windows::Win32::System::Threading::{CREATE_NO_WINDOW, PROCESS_SYNCHRONIZE};

    use super::*;

    /// 子プロセスが終わるまでの上限。
    const TIMEOUT: Duration = Duration::from_secs(10);

    fn spawn(command_line: &str) -> OwnedHandle {
        let mut words = command_line.split(' ');
        let child = Command::new(words.next().unwrap())
            .args(words)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .unwrap();
        OwnedHandle::from(child)
    }

    #[test]
    fn the_exit_code_is_read_after_waiting() {
        let process = spawn("cmd.exe /c exit 3");
        assert!(wait(&process, Some(TIMEOUT)));
        assert_eq!(exit_code(&process).unwrap(), 3);
    }

    #[test]
    fn a_terminated_process_ends_with_the_terminated_exit_code() {
        /// ping が送る回数（1 秒に 1 回）。テストの間は動いていて、終わらせられなくても自分で終わる。
        const PING_COUNT: u32 = 30;

        let process = spawn(&format!("ping.exe -n {PING_COUNT} 127.0.0.1"));
        assert!(!wait(&process, Some(Duration::ZERO)));
        terminate(&process).unwrap();
        assert!(wait(&process, Some(TIMEOUT)));
        assert_eq!(exit_code(&process).unwrap(), TERMINATED_EXIT_CODE);
    }

    #[test]
    fn reading_tells_whether_the_pipe_closed_or_the_reader_stopped() {
        let mut chunks = Vec::new();
        let closed = read_until_closed(&b"abcde"[..], 2, "a test", |chunk| {
            chunks.push(chunk.to_vec());
            true
        });
        assert!(closed);
        assert_eq!(chunks, [b"ab".to_vec(), b"cd".to_vec(), b"e".to_vec()]);

        chunks.clear();
        let closed = read_until_closed(&b"abcde"[..], 2, "a test", |chunk| {
            chunks.push(chunk.to_vec());
            false
        });
        assert!(!closed);
        assert_eq!(chunks, [b"ab".to_vec()]);
    }

    #[test]
    fn this_process_is_opened_by_its_id() {
        let process = open(std::process::id(), PROCESS_SYNCHRONIZE).unwrap();
        assert!(!wait(&process, Some(Duration::ZERO)));
        assert!(open(0, PROCESS_SYNCHRONIZE).is_err());
    }
}
