//! プロセスの親子を OS の一覧から読む。番号は使い回されるので、作成時刻と組みで同じプロセスかを見る。

use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    thread,
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::{FILETIME, HANDLE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    },
};

/// 条件が成り立つまで待つ上限。タブを閉じるときに子プロセスを終わらせるまで待つ 5 秒より長くする。
const WAIT_TIMEOUT: Duration = Duration::from_secs(60);
/// 条件を確認する間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 動いているプロセス。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Process {
    pub pid: u32,
    /// 作成時刻（`FILETIME` の値）。
    pub created: u64,
    /// 実行ファイルの名前（小文字）。
    pub name: String,
}

/// いま動いているプロセスの番号と、親の番号と、実行ファイルの名前。
fn list() -> Vec<(u32, u32, String)> {
    // SAFETY: 引数はフラグだけ。
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .expect("cannot list the processes");
    // SAFETY: 成功したので、ハンドルは有効で、ほかに持ち主はいない。
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot.0) };
    let handle = HANDLE(snapshot.as_raw_handle());
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut processes = Vec::new();
    // SAFETY: handle は有効で、entry は dwSize を入れてある。
    let mut more = unsafe { Process32FirstW(handle, &mut entry) }.is_ok();
    while more {
        let length = entry
            .szExeFile
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..length]).to_ascii_lowercase();
        processes.push((entry.th32ProcessID, entry.th32ParentProcessID, name));
        // SAFETY: 同上。
        more = unsafe { Process32NextW(handle, &mut entry) }.is_ok();
    }
    processes
}

/// プロセス `pid` の作成時刻。すでに終わっているか、開けなければ `None`。
fn creation_time(pid: u32) -> Option<u64> {
    // SAFETY: 読み取りの権限で開くだけ。
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    // SAFETY: 成功したので、ハンドルは有効で、ほかに持ち主はいない。
    let process = unsafe { OwnedHandle::from_raw_handle(process.0) };
    let mut created = FILETIME::default();
    let (mut exited, mut kernel, mut user) = Default::default();
    // SAFETY: ハンドルは有効で、どの引数も書き込める。
    unsafe {
        GetProcessTimes(
            HANDLE(process.as_raw_handle()),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    }
    .ok()?;
    Some(u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
}

/// プロセス `root` の子孫。親が先に終わった孫は親を辿れないので、含まない。親の番号が使い回されたものは、
/// 親より前に作られたプロセスなので除く。
pub fn descendants(root: u32) -> Vec<Process> {
    let Some(root_created) = creation_time(root) else {
        return Vec::new();
    };
    let all = list();
    let mut found = Vec::new();
    let mut parents = vec![(root, root_created)];
    while let Some((parent, parent_created)) = parents.pop() {
        for (pid, _, name) in all.iter().filter(|(_, ppid, _)| *ppid == parent) {
            let Some(created) = creation_time(*pid) else {
                continue;
            };
            if created < parent_created {
                continue;
            }
            parents.push((*pid, created));
            found.push(Process {
                pid: *pid,
                created,
                name: name.clone(),
            });
        }
    }
    found
}

/// `process` がまだ動いている。
fn is_running(process: &Process) -> bool {
    list().iter().any(|(pid, _, _)| *pid == process.pid)
        && creation_time(process.pid) == Some(process.created)
}

/// `root` の子孫のうち、名前が `name` のものの数が `count` になるまで待ち、そのときの子孫を返す。
#[track_caller]
pub fn wait_descendants_named(root: u32, name: &str, count: usize) -> Vec<Process> {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        let found = descendants(root);
        if found.iter().filter(|process| process.name == name).count() == count {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "waiting for {count} {name} under {root}: {found:#?}"
        );
        thread::sleep(POLL_INTERVAL);
    }
}

/// `processes` がすべて終わるまで待つ。
#[track_caller]
pub fn wait_ended(processes: &[Process]) {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        let left: Vec<&Process> = processes
            .iter()
            .filter(|process| is_running(process))
            .collect();
        if left.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "these processes did not end: {left:#?}"
        );
        thread::sleep(POLL_INTERVAL);
    }
}

/// `processes` がすべて動いている。
#[track_caller]
pub fn assert_running(processes: &[Process]) {
    let left: Vec<&Process> = processes
        .iter()
        .filter(|process| !is_running(process))
        .collect();
    assert!(left.is_empty(), "these processes ended: {left:#?}");
}
