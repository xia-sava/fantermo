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
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
                CoCreateInstance, CoInitializeEx, StructuredStorage::PROPVARIANT,
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

/// 一覧を作り直さないわけ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Skip {
    /// 設定ファイルの場所を環境変数で差し替えている。
    Overridden,
    /// 設定ファイルの場所が分からない。
    NoSettingsFile,
    /// 設定ファイル全体を読めなかった。
    NotRead,
}

/// 一覧を作り直さないわけ。作り直すなら `None`。`path_overridden` は設定ファイルの場所を環境変数で差し替えているか、
/// `has_path` は設定ファイルの場所が分かるか、`whole_file_read` は設定ファイル全体を読めたか。
fn skip_reason(path_overridden: bool, has_path: bool, whole_file_read: bool) -> Option<Skip> {
    if path_overridden {
        return Some(Skip::Overridden);
    }
    if !has_path {
        return Some(Skip::NoSettingsFile);
    }
    if !whole_file_read {
        return Some(Skip::NotRead);
    }
    None
}

/// 設定ファイル `config_path` から読んだプロファイル `profiles` で、一覧を作り直す。待たずに戻る。
/// `whole_file_read` は設定ファイル全体を読めたか（[`config_file::Loaded::whole_file_read`]）。
///
/// 次のときは作り直さない（[`skip_reason`]）。
/// - 設定ファイルの場所を環境変数で差し替えているとき。一覧から起動した fantermo には環境変数が渡らず、別の
///   設定ファイルを読むので、並べたプロファイルと開くプロファイルが食い違う。
/// - 設定ファイルの場所が分からないとき。読む設定ファイルがないので、並べるものが決まらない。
/// - 設定ファイル全体を読めなかったとき。設定は既定値か前の値で、ファイルに書いたプロファイルではないので、
///   それで一覧を上書きしない。
pub fn rebuild(config_path: Option<&Path>, whole_file_read: bool, profiles: &[Profile]) {
    let skip = skip_reason(
        config_file::path_overridden(),
        config_path.is_some(),
        whole_file_read,
    );
    match skip {
        None => request(tasks(profiles)),
        Some(Skip::NotRead) => {
            tracing::info!("not updating the jump list: the settings file was not read");
        }
        Some(reason) => tracing::debug!("not updating the jump list: {reason:?}"),
    }
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
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
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
    let directory = known_folder::user_profile();
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

    use windows::Win32::{System::Com::CoUninitialize, UI::Shell::SLGP_RAWPATH};

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

    #[test]
    fn the_reasons_to_skip_rebuilding_are_checked_in_order() {
        assert_eq!(skip_reason(false, true, true), None);
        assert_eq!(skip_reason(true, true, true), Some(Skip::Overridden));
        assert_eq!(skip_reason(true, false, false), Some(Skip::Overridden));
        assert_eq!(skip_reason(false, false, true), Some(Skip::NoSettingsFile));
        assert_eq!(skip_reason(false, false, false), Some(Skip::NoSettingsFile));
        assert_eq!(skip_reason(false, true, false), Some(Skip::NotRead));
    }

    /// ショートカットから読み戻す文字列のバッファの大きさ（UTF-16 の単位）。
    const READ_BACK_UNITS: usize = 1024;

    /// NUL で終わる `buffer` の、NUL の前までの文字列。
    fn text_of(buffer: &[u16]) -> String {
        let end = buffer.iter().position(|&unit| unit == 0).unwrap();
        String::from_utf16(&buffer[..end]).unwrap()
    }

    /// 作ったショートカットから、渡した実行ファイル・引数・作業フォルダ・アイコン・表示名が読み戻せる。保存はしない。
    #[test]
    fn the_shell_link_holds_what_was_given() {
        // SAFETY: このテストのスレッドで初期化し、最後に解放する。
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }
            .ok()
            .unwrap();
        let exe = std::env::current_exe().unwrap();
        let directory = known_folder::user_profile().unwrap();
        let exe_text = exe.to_str().unwrap();
        for task in tasks(&profiles(&["a \"quoted\" name", "日本語の名前", "one"])) {
            let link = shell_link(&exe, Some(&directory), &task).unwrap();
            let mut buffer = [0u16; READ_BACK_UNITS];
            let mut icon = -1;
            // SAFETY: buffer は書き込め、ほかの引数は使わない。
            unsafe { link.GetPath(&mut buffer, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32) }
                .unwrap();
            assert_eq!(text_of(&buffer), exe_text);
            // SAFETY: buffer は書き込める。
            unsafe { link.GetArguments(&mut buffer) }.unwrap();
            assert_eq!(text_of(&buffer), task.arguments);
            // SAFETY: buffer は書き込める。
            unsafe { link.GetWorkingDirectory(&mut buffer) }.unwrap();
            assert_eq!(text_of(&buffer), directory.to_str().unwrap());
            // SAFETY: buffer と icon は書き込める。
            unsafe { link.GetIconLocation(&mut buffer, &mut icon) }.unwrap();
            assert_eq!((text_of(&buffer).as_str(), icon), (exe_text, 0));
            let store: IPropertyStore = link.cast().unwrap();
            // SAFETY: 定数の鍵を渡して、値を受け取る。
            let title = unsafe { store.GetValue(&PKEY_Title) }.unwrap();
            assert_eq!(title.vt(), VT_LPWSTR);
            assert_eq!(title.to_string(), task.title);
        }
        // SAFETY: 初期化したこのスレッドで、作ったものをすべて捨ててから解放する。
        unsafe { CoUninitialize() };
    }

    /// 値の型が `VT_LPWSTR` で、指す文字列が渡したものと同じで、写した値（`PropVariantCopy`）と等しい。
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
