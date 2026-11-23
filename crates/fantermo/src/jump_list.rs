//! タスクバーのジャンプリストの「タスク」に、プロファイルを並べる。
//!
//! 項目は、自分の実行ファイルを `--profile <名前>` で起動するショートカット（`IShellLinkW`）で、表示名は
//! プロファイルの名前、アイコンは実行ファイルの 0 番、作業フォルダはホームのフォルダ。一覧は
//! `ICustomDestinationList` で作り、`AddUserTasks` の「タスク」の欄にだけ入れる。AppUserModelID は設定しないので、
//! 一覧は、タスクバーが実行ファイルのパスでまとめたボタンのものになる。
//!
//! COM の呼び出しは専用のスレッド（`jump-list`）でする。スレッドは最初の依頼で立て、プロセスが終わるまで残す。
//! ウィンドウのスレッドは項目の並びを送るだけで待たない。溜まった依頼は最後のものだけを作る。

use std::{
    path::Path,
    sync::{Mutex, PoisonError, mpsc},
    thread,
};

use fantermo_core::config::Profile;
use windows::{
    Win32::{
        Storage::EnhancedStorage::PKEY_Title,
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                StructuredStorage::PROPVARIANT,
            },
            Variant::VT_LPWSTR,
        },
        UI::Shell::{
            Common::{IObjectArray, IObjectCollection},
            DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW,
            PropertiesSystem::IPropertyStore,
            SHStrDupW, ShellLink,
        },
    },
    core::{HSTRING, Interface},
};

use crate::{command_line, config_file, known_folder};

/// 「タスク」の欄の項目の一つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    /// 一覧に出す名前。
    pub title: String,
    /// 実行ファイルに渡すコマンドラインの引数。
    pub arguments: String,
}

/// 一覧に並べる項目。`profiles` の順で、名前が `--` で始まるプロファイルは `--profile` で渡せないので飛ばす。
pub fn tasks(profiles: &[Profile]) -> Vec<Task> {
    profiles
        .iter()
        .filter_map(|profile| {
            Some(Task {
                title: profile.name.clone(),
                arguments: command_line::profile_arguments(&profile.name)?,
            })
        })
        .collect()
}

/// 設定ファイル `config_path` から読んだプロファイル `profiles` で、一覧を作り直す。待たずに戻る。
/// `whole_file_read` は設定ファイル全体を読めたか（[`config_file::Loaded::whole_file_read`]）。
///
/// 設定ファイルの場所を環境変数で差し替えているとき、場所が分からないとき、全体を読めなかったときは作り直さない。
/// 一覧から起動した fantermo には環境変数が渡らないので、別の設定ファイルのプロファイルを並べることになり、
/// 全体を読めなかったときの設定は既定値か前の値なので、それで一覧を上書きしないため。
pub fn rebuild(config_path: Option<&Path>, whole_file_read: bool, profiles: &[Profile]) {
    if config_file::path_overridden() {
        tracing::debug!("not updating the jump list: the settings file is overridden");
        return;
    }
    if config_path.is_none() {
        tracing::debug!("not updating the jump list: no settings file");
        return;
    }
    if !whole_file_read {
        tracing::info!("not updating the jump list: the settings file was not read");
        return;
    }
    request(tasks(profiles));
}

/// 一覧を作るスレッドへ `tasks` を送る。スレッドがなければ立てる。
fn request(tasks: Vec<Task>) {
    static SENDER: Mutex<Option<mpsc::Sender<Vec<Task>>>> = Mutex::new(None);
    let mut sender = SENDER.lock().unwrap_or_else(PoisonError::into_inner);
    let tasks = match sender.as_ref() {
        Some(running) => match running.send(tasks) {
            Ok(()) => return,
            // スレッドが終わっていれば立て直す（COM を初期化できなかったときなど）。
            Err(mpsc::SendError(tasks)) => tasks,
        },
        None => tasks,
    };
    start_with(&mut sender, tasks);
}

/// 一覧を作るスレッドを立てて `tasks` を送り、チャネルの送信側を `sender` に置く。立てられなければ警告を残す。
fn start_with(sender: &mut Option<mpsc::Sender<Vec<Task>>>, tasks: Vec<Task>) {
    let (new_sender, requests) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name("jump-list".into())
        .spawn(move || serve(&requests));
    if let Err(e) = spawned {
        tracing::warn!("failed to start updating the jump list: {e}");
        *sender = None;
        return;
    }
    // スレッドが受信側を手放していて送れなければ（COM を初期化できなかったとき）、次の依頼で立て直す。
    _ = new_sender.send(tasks);
    *sender = Some(new_sender);
}

/// 届いた依頼ごとに一覧を作る。COM を初期化できなければ終わる。
fn serve(requests: &mpsc::Receiver<Vec<Task>>) {
    // SAFETY: このスレッドで一度だけ初期化する。プロセスが終わるまでこのスレッドで使うので、解放しない。
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if let Err(e) = com.ok() {
        tracing::warn!("failed to initialize COM for the jump list: {e}");
        return;
    }
    while let Ok(mut tasks) = requests.recv() {
        // 溜まった依頼は最後のものだけを作る。
        while let Ok(newer) = requests.try_recv() {
            tasks = newer;
        }
        match commit(&tasks) {
            Ok(()) => tracing::info!("updated the jump list with {} tasks", tasks.len()),
            Err(e) => tracing::warn!("failed to update the jump list: {e}"),
        }
    }
}

/// 一覧を作れなかったわけ。失敗した呼び出しと、そのエラー。
#[derive(Debug)]
struct Failure {
    call: &'static str,
    error: windows::core::Error,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.call, self.error)
    }
}

/// エラーを、呼び出し `call` の [`Failure`] にする。
fn failed(call: &'static str) -> impl FnOnce(windows::core::Error) -> Failure {
    move |error| Failure { call, error }
}

/// 一覧の「タスク」の欄を `tasks` にして書き込む。`tasks` が空なら欄を空にする。
fn commit(tasks: &[Task]) -> Result<(), Failure> {
    let exe = std::env::current_exe().map_err(|e| failed("current_exe")(e.into()))?;
    let directory = known_folder::profile();
    // SAFETY: COM を初期化したこのスレッドで、プロセスの中のオブジェクトを作る。
    let list: ICustomDestinationList =
        unsafe { CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER) }
            .map_err(failed("CoCreateInstance(DestinationList)"))?;
    let mut slots = 0;
    // SAFETY: slots は書き込める。外された項目の並びは使わない（自分の見出しを持たないため）。
    let _removed: IObjectArray =
        unsafe { list.BeginList(&mut slots) }.map_err(failed("BeginList"))?;
    let written = add_tasks(&list, &exe, directory.as_deref(), tasks)
        // SAFETY: BeginList が成功した一覧に書き込む。
        .and_then(|()| unsafe { list.CommitList() }.map_err(failed("CommitList")));
    if written.is_err() {
        // SAFETY: BeginList が成功した一覧の書き込みを取りやめる。
        if let Err(e) = unsafe { list.AbortList() } {
            tracing::debug!("failed to abort the jump list: {e}");
        }
    }
    written
}

/// `list` の「タスク」の欄に `tasks` を入れる。項目は、実行ファイル `exe` を作業フォルダ `directory` で起動する。
fn add_tasks(
    list: &ICustomDestinationList,
    exe: &Path,
    directory: Option<&Path>,
    tasks: &[Task],
) -> Result<(), Failure> {
    if tasks.is_empty() {
        return Ok(());
    }
    // SAFETY: COM を初期化したこのスレッドで、プロセスの中のオブジェクトを作る。
    let collection: IObjectCollection =
        unsafe { CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER) }
            .map_err(failed("CoCreateInstance(EnumerableObjectCollection)"))?;
    for task in tasks {
        let link = shell_link(exe, directory, task).map_err(failed("IShellLinkW"))?;
        // SAFETY: link は作ったショートカットで、collection が参照を持つ。
        unsafe { collection.AddObject(&link) }.map_err(failed("AddObject"))?;
    }
    let array: IObjectArray = collection.cast().map_err(failed("IObjectArray"))?;
    // SAFETY: BeginList が成功した一覧に、ショートカットの並びを渡す。
    unsafe { list.AddUserTasks(&array) }.map_err(failed("AddUserTasks"))
}

/// `task` の項目にするショートカット。
fn shell_link(
    exe: &Path,
    directory: Option<&Path>,
    task: &Task,
) -> windows::core::Result<IShellLinkW> {
    let exe = HSTRING::from(exe);
    // SAFETY: COM を初期化したこのスレッドで、プロセスの中のオブジェクトを作る。渡す文字列は各呼び出しの間は
    // 生きている。
    let link: IShellLinkW = unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&exe)?;
        link.SetArguments(&HSTRING::from(task.arguments.as_str()))?;
        link.SetIconLocation(&exe, 0)?;
        if let Some(directory) = directory {
            link.SetWorkingDirectory(&HSTRING::from(directory))?;
        }
        link
    };
    let store: IPropertyStore = link.cast()?;
    let title = string_value(&task.title)?;
    // SAFETY: title は初期化した値で、呼び出しの間は生きている。SetValue は値を写して持つ。
    unsafe {
        store.SetValue(&PKEY_Title, &title)?;
        store.Commit()?;
    }
    Ok(link)
}

/// `text` の写しを持つ `VT_LPWSTR` の値（`InitPropVariantFromString` と同じもの）。写しは `CoTaskMemAlloc` で
/// 確保したもので、`PROPVARIANT` の `Drop`（`PropVariantClear`）が解放する。
fn string_value(text: &str) -> windows::core::Result<PROPVARIANT> {
    // SAFETY: 渡す文字列は呼び出しの間は生きている。写しは CoTaskMemAlloc で確保され、呼び出した側が持つ。
    let copy = unsafe { SHStrDupW(&HSTRING::from(text)) }?;
    let mut value = PROPVARIANT::default();
    // SAFETY: 既定の値は空（VT_EMPTY）で、型と写しのポインタを書くだけ。写しの持ち主は value になる。
    unsafe {
        let typed = &mut value.Anonymous.Anonymous;
        typed.vt = VT_LPWSTR;
        typed.Anonymous.pwszVal = copy;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use fantermo_core::config::BackendKind;

    use super::*;
    use crate::command_line::Options;

    fn profiles(names: &[&str]) -> Vec<Profile> {
        names
            .iter()
            .map(|name| Profile::new(name, "cmd.exe", BackendKind::Conpty))
            .collect()
    }

    #[test]
    fn tasks_follow_the_profiles_in_order() {
        let tasks = tasks(&profiles(&["PowerShell", "cmd", "zsh"]));
        let titles: Vec<&str> = tasks.iter().map(|task| task.title.as_str()).collect();
        assert_eq!(titles, ["PowerShell", "cmd", "zsh"]);
        assert_eq!(tasks[0].arguments, r#"--profile "PowerShell""#);
    }

    #[test]
    fn names_starting_with_two_dashes_are_skipped() {
        let tasks = tasks(&profiles(&["--new-window", "one", "--", "-x"]));
        let titles: Vec<&str> = tasks.iter().map(|task| task.title.as_str()).collect();
        assert_eq!(titles, ["one", "-x"]);
    }

    #[test]
    fn the_arguments_are_read_back_as_the_same_name() {
        let names = [
            "one",
            "a b",
            " leading",
            "R&D",
            r#"a"b"#,
            r"a\",
            r#"a\"b"#,
            r"C:\x\\",
            "日本語",
            "-x",
        ];
        let tasks = tasks(&profiles(&names));
        assert_eq!(tasks.len(), names.len());
        for task in tasks {
            let options = Options::parse(&task.arguments);
            assert_eq!(
                options.tab.profile.as_deref(),
                Some(task.title.as_str()),
                "{}",
                task.arguments
            );
            assert_eq!(options.tab.command, None, "{}", task.arguments);
            assert!(!options.profile_without_name, "{}", task.arguments);
            assert!(
                !options.new_window && !options.elevate,
                "{}",
                task.arguments
            );
        }
    }

    /// 値は文字列の写しを持ち、捨てるときに `PropVariantClear` で解放できる。
    #[test]
    fn the_title_is_a_copied_wide_string() {
        let value = string_value("名前 a\"b").unwrap();
        assert_eq!(value.vt(), VT_LPWSTR);
        // SAFETY: string_value が VT_LPWSTR の写しを書いた。
        let string = unsafe { value.Anonymous.Anonymous.Anonymous.pwszVal };
        // SAFETY: string は NUL で終わる写しを指す。
        assert_eq!(unsafe { string.to_string() }.unwrap(), "名前 a\"b");
        let copied = value.clone();
        assert_eq!(copied, value);
    }
}
