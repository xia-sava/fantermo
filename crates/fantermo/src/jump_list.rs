//! タスクバーのジャンプリストの「タスク」に、プロファイルを並べる。
//!
//! 項目は、自分の実行ファイルを `--profile <名前>` で起動するショートカット（`IShellLinkW`）で、表示名は
//! プロファイルの名前、作業フォルダはホームのフォルダ。一覧は `ICustomDestinationList` で作り、`AddUserTasks` の
//! 「タスク」の欄にだけ入れる。AppUserModelID は設定しないので、一覧は、タスクバーが実行ファイルのパスでまとめた
//! ボタンのものになる。
//!
//! 項目のアイコンはタブと同じもの（[`TabIcon::for_profiles`]）で、管理者の盾は重ねない。ショートカットには
//! アイコンのファイルのパスしか渡せないので、[`ICON_SIDES`] の大きさごとに描いて ICO に書き出す。置き場所は
//! `%LOCALAPPDATA%\fantermo\jump-list\<実行ファイルのパスのハッシュ>\`、ファイルの名前は ICO の中身のハッシュ
//! （[`icon_file_name`]）。シェルはアイコンをパスで覚えるので、中身が変われば名前も変える。一覧を書き込めたら、
//! そのフォルダの ICO のうち一覧が使っていないものを消す。アイコンを作れない・書き出せない項目は、実行ファイルの
//! 0 番のアイコンにする。
//!
//! COM の呼び出しと、アイコンのファイルを読んで ICO を作ることは、専用のスレッド（`jump-list`）でする。スレッドは
//! 最初の依頼で立て、プロセスが終わるまで残す。ウィンドウのスレッドはプロファイルの並びを送るだけで待たない。
//! 溜まった依頼は最後のものだけを作る。

use std::{
    collections::HashSet,
    fmt, io,
    path::{Path, PathBuf},
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
            DestinationList, EnumerableObjectCollection, FOLDERID_LocalAppData,
            ICustomDestinationList, IShellLinkW,
            PropertiesSystem::IPropertyStore,
            SHStrDupW, ShellLink,
        },
    },
    core::{HSTRING, Interface},
};

use crate::{
    app_files::{self, path_key},
    command_line, config_file, fnv, font, ico, known_folder,
    tab_icon::{Generator, TabIcon},
};

/// 項目のアイコンの ICO に入れる一辺（ピクセル）。ジャンプリストのアイコンは DPI で大きさが変わり、足りない
/// 大きさはシェルが拡大・縮小してぼやけるので、100% から 300% までの小さいアイコンの大きさを描いて入れる。
const ICON_SIDES: [u32; 6] = [16, 20, 24, 32, 40, 48];

/// `%LOCALAPPDATA%` の下の、アイコンのファイルを置くフォルダ。この下に実行ファイルごとのフォルダを作る。
const ICON_DIR_COMPONENTS: [&str; 2] = ["fantermo", "jump-list"];

/// アイコンのファイルの拡張子。
const ICON_EXTENSION: &str = "ico";

/// 「タスク」の欄の項目の一つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    /// 一覧に出す名前。
    pub title: String,
    /// 実行ファイルに渡すコマンドラインの引数。
    pub arguments: String,
    /// 項目のプロファイルの、プロファイルの並びの中の位置。
    pub profile: usize,
}

/// 一覧に並べる項目。`profiles` の順で、名前が `--` で始まるプロファイルは `--profile` で渡せないので飛ばす。
pub fn tasks(profiles: &[Profile]) -> Vec<Task> {
    profiles
        .iter()
        .enumerate()
        .filter_map(|(index, profile)| {
            Some(Task {
                title: profile.name.clone(),
                arguments: command_line::profile_arguments(&profile.name)?,
                profile: index,
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

/// 一覧を作るスレッドへの依頼。
struct Request {
    /// 設定のプロファイルの並び。`--` で始まる名前のものも含める（頭文字はプロファイル全体を見て決めるため）。
    profiles: Vec<Profile>,
    /// 設定ファイルのフォルダ。相対パスのアイコンの画像はここから辿る。
    config_dir: Option<PathBuf>,
    /// 頭文字を描く書体（タブと同じ UI のフォントの書体）。
    family: String,
}

/// 設定ファイル `config_path` から読んだプロファイル `profiles` で、一覧を作り直す。待たずに戻る。
/// `whole_file_read` は設定ファイル全体を読めたか（[`config_file::Loaded::whole_file_read`]）、`family` は項目の
/// アイコンの頭文字を描く書体。
///
/// 次のときは作り直さない（[`skip_reason`]）。
/// - 設定ファイルの場所を環境変数で差し替えているとき。一覧から起動した fantermo には環境変数が渡らず、別の
///   設定ファイルを読むので、並べたプロファイルと開くプロファイルが食い違う。
/// - 設定ファイルの場所が分からないとき。読む設定ファイルがないので、並べるものが決まらない。
/// - 設定ファイル全体を読めなかったとき。設定は既定値か前の値で、ファイルに書いたプロファイルではないので、
///   それで一覧を上書きしない。
pub fn rebuild(
    config_path: Option<&Path>,
    whole_file_read: bool,
    profiles: &[Profile],
    family: &str,
) {
    let skip = skip_reason(
        config_file::path_overridden(),
        config_path.is_some(),
        whole_file_read,
    );
    match skip {
        None => request(Request {
            profiles: profiles.to_vec(),
            config_dir: config_path.and_then(Path::parent).map(Path::to_path_buf),
            family: family.to_owned(),
        }),
        Some(Skip::NotRead) => {
            tracing::info!("not updating the jump list: the settings file was not read");
        }
        Some(reason) => tracing::debug!("not updating the jump list: {reason:?}"),
    }
}

/// 一覧を作るスレッドへ `request` を送る。スレッドがなければ立てる。
fn request(request: Request) {
    static SENDER: Mutex<Option<mpsc::Sender<Request>>> = Mutex::new(None);
    let mut sender = SENDER.lock().unwrap_or_else(PoisonError::into_inner);
    let request = match sender.as_ref() {
        Some(running) => match running.send(request) {
            Ok(()) => return,
            // スレッドが終わっていれば立て直す（COM を初期化できなかったときなど）。
            Err(mpsc::SendError(request)) => request,
        },
        None => request,
    };
    start_with(&mut sender, request);
}

/// 一覧を作るスレッドを立てて `request` を送り、チャネルの送信側を `sender` に置く。立てられなければ警告を残す。
fn start_with(sender: &mut Option<mpsc::Sender<Request>>, request: Request) {
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
    _ = new_sender.send(request);
    *sender = Some(new_sender);
}

/// 届いた依頼ごとに一覧を作る。COM を初期化できなければ終わる。
fn serve(requests: &mpsc::Receiver<Request>) {
    // SAFETY: このスレッドで一度だけ初期化する。プロセスが終わるまでこのスレッドで使うので、解放しない。
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    if let Err(e) = com.ok() {
        tracing::warn!("failed to initialize COM for the jump list: {e}");
        return;
    }
    let generator = font::create_factory()
        .and_then(|dwrite| Generator::new(&dwrite))
        .inspect_err(|e| tracing::warn!("failed to prepare drawing the jump list icons: {e}"))
        .ok();
    while let Ok(mut request) = requests.recv() {
        // 溜まった依頼は最後のものだけを作る。
        while let Ok(newer) = requests.try_recv() {
            request = newer;
        }
        build(&request, generator.as_ref());
    }
}

/// `request` の一覧を作って書き込む。書き込めたら、使わなくなったアイコンのファイルを消す。
fn build(request: &Request, generator: Option<&Generator>) {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            tracing::warn!("failed to update the jump list: current_exe: {e}");
            return;
        }
    };
    let folder = known_folder::path(&FOLDERID_LocalAppData).map(|local| icon_folder(&local, &exe));
    let tasks = tasks(&request.profiles);
    let icons = TabIcon::for_profiles(&request.profiles, request.config_dir.as_deref());
    let icon_paths: Vec<Option<PathBuf>> = tasks
        .iter()
        .map(|task| {
            icon_file(
                generator,
                folder.as_deref(),
                &icons[task.profile],
                &request.family,
            )
            .inspect_err(|e| {
                tracing::warn!(
                    "using the fantermo icon for the jump list task {:?}: {e}",
                    task.title
                );
            })
            .ok()
        })
        .collect();
    match commit(&exe, &tasks, &icon_paths) {
        Ok(()) => {
            tracing::info!("updated the jump list with {} tasks", tasks.len());
            if let Some(folder) = &folder {
                remove_unused_icons(folder, &icon_paths);
            }
        }
        Err(e) => tracing::warn!("failed to update the jump list: {e}"),
    }
}

/// 実行ファイル `exe` の項目のアイコンを置くフォルダ。`local_app_data` は `%LOCALAPPDATA%`。開発中のビルドと
/// インストールしたものは一覧を別々に持つので、フォルダも実行ファイルごとに分ける（[`folder_name`]）。
fn icon_folder(local_app_data: &Path, exe: &Path) -> PathBuf {
    let mut folder = local_app_data.to_path_buf();
    folder.extend(ICON_DIR_COMPONENTS);
    folder.push(folder_name(exe));
    folder
}

/// 実行ファイル `exe` のアイコンのフォルダの名前。パスの 64 ビットの FNV-1a の 16 進で、パスの大文字と小文字、
/// `/` と `\` の違いでは変わらない（[`path_key`]）。
fn folder_name(exe: &Path) -> String {
    format!("{:016x}", fnv::hash64(path_key(exe).as_bytes()))
}

/// 中身が `ico` のアイコンのファイルの名前。中身の 64 ビットの FNV-1a の 16 進に `.ico` を付けたもの。衝突しても
/// 項目の絵が入れ替わるだけ。
fn icon_file_name(ico: &[u8]) -> String {
    format!("{:016x}.{ICON_EXTENSION}", fnv::hash64(ico))
}

/// 項目のアイコンを作れなかったわけ。
#[derive(Debug)]
enum IconError {
    /// ピクセルを作るものを用意できなかった。
    NoGenerator,
    /// アイコンを置くフォルダが分からない。
    NoFolder,
    /// ピクセルを作れなかった。
    Draw(windows::core::Error),
    /// ICO にできない大きさだった。
    Encode,
    /// ファイルに書けなかった。
    Write(io::Error),
}

impl fmt::Display for IconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoGenerator => f.write_str("the icons cannot be drawn"),
            Self::NoFolder => f.write_str("the folder for the icons is unknown"),
            Self::Draw(e) => write!(f, "failed to draw the icon: {e}"),
            Self::Encode => f.write_str("failed to encode the icon"),
            Self::Write(e) => write!(f, "failed to write the icon: {e}"),
        }
    }
}

/// `icon` を [`ICON_SIDES`] の大きさごとに `generator` で描き（頭文字は書体 `family`）、ICO にしてフォルダ
/// `folder` に置いたファイルのパス。
fn icon_file(
    generator: Option<&Generator>,
    folder: Option<&Path>,
    icon: &TabIcon,
    family: &str,
) -> Result<PathBuf, IconError> {
    let generator = generator.ok_or(IconError::NoGenerator)?;
    let folder = folder.ok_or(IconError::NoFolder)?;
    let images = ICON_SIDES
        .iter()
        .map(|&side| generator.pixels(icon, side, family))
        .collect::<Result<Vec<_>, _>>()
        .map_err(IconError::Draw)?;
    let ico = ico::encode(&images).ok_or(IconError::Encode)?;
    write_icon(folder, &ico).map_err(IconError::Write)
}

/// 中身が `ico` のアイコンのファイルをフォルダ `folder` に置き、そのパスを返す。同じ名前のファイル（同じ中身）が
/// すでにあれば書かない。フォルダがなければ作る。
fn write_icon(folder: &Path, ico: &[u8]) -> io::Result<PathBuf> {
    let path = folder.join(icon_file_name(ico));
    if !path.is_file() {
        app_files::write_replacing(&path, ico)?;
    }
    Ok(path)
}

/// フォルダの直下のファイルの名前 `names` のうち、消すアイコンのファイル。名前が `.ico` で終わり、`used`
/// （一覧の項目が使っているファイルの名前を小文字にしたもの）にないもの。大文字と小文字は区別しない。
fn unused_icons<'a>(names: &'a [String], used: &HashSet<String>) -> Vec<&'a str> {
    names
        .iter()
        .filter(|name| {
            Path::new(name.as_str())
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case(ICON_EXTENSION))
        })
        .filter(|name| !used.contains(&name.to_lowercase()))
        .map(String::as_str)
        .collect()
}

/// フォルダ `folder` の直下の、一覧の項目のアイコン `icon_paths` が使っていない ICO を、名前を指定して消す。
/// ほかのファイルとフォルダには触らない。消せなければ警告を残す。
fn remove_unused_icons(folder: &Path, icon_paths: &[Option<PathBuf>]) {
    let used: HashSet<String> = icon_paths
        .iter()
        .flatten()
        .filter_map(|path| path.file_name()?.to_str().map(str::to_lowercase))
        .collect();
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        // 一つも書いていなければフォルダはない。
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!(
                "cannot list the jump list icons in {}: {e}",
                folder.display()
            );
            return;
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => {
                let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
                if is_file && let Some(name) = entry.file_name().to_str() {
                    names.push(name.to_owned());
                }
            }
            Err(e) => {
                tracing::warn!(
                    "cannot list the jump list icons in {}: {e}",
                    folder.display()
                );
            }
        }
    }
    for name in unused_icons(&names, &used) {
        let path = folder.join(name);
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!("cannot remove {}: {e}", path.display());
        }
    }
}

/// 一覧を作れなかったわけ。失敗した呼び出しと、そのエラー。
#[derive(Debug)]
struct Failure {
    call: &'static str,
    error: windows::core::Error,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.call, self.error)
    }
}

/// エラーを、呼び出し `call` の [`Failure`] にする。
fn failed(call: &'static str) -> impl FnOnce(windows::core::Error) -> Failure {
    move |error| Failure { call, error }
}

/// 一覧の「タスク」の欄を `tasks` にして書き込む。項目は実行ファイル `exe` を起動し、アイコンは `icons` の同じ
/// 位置のファイル（`None` なら実行ファイルの 0 番）。`tasks` が空なら欄を空にする。
fn commit(exe: &Path, tasks: &[Task], icons: &[Option<PathBuf>]) -> Result<(), Failure> {
    let directory = known_folder::user_profile();
    // SAFETY: COM を初期化したこのスレッドで、プロセスの中のオブジェクトを作る。
    let list: ICustomDestinationList =
        unsafe { CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER) }
            .map_err(failed("CoCreateInstance(DestinationList)"))?;
    let mut slots = 0;
    // SAFETY: slots は書き込める。外された項目の並びは使わない（自分の見出しを持たないため）。
    let _removed: IObjectArray =
        unsafe { list.BeginList(&mut slots) }.map_err(failed("BeginList"))?;
    let written = add_tasks(&list, exe, directory.as_deref(), tasks, icons)
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

/// `list` の「タスク」の欄に `tasks` を入れる。項目は、実行ファイル `exe` を作業フォルダ `directory` で起動し、
/// アイコンは `icons` の同じ位置のファイル。
fn add_tasks(
    list: &ICustomDestinationList,
    exe: &Path,
    directory: Option<&Path>,
    tasks: &[Task],
    icons: &[Option<PathBuf>],
) -> Result<(), Failure> {
    if tasks.is_empty() {
        return Ok(());
    }
    // SAFETY: COM を初期化したこのスレッドで、プロセスの中のオブジェクトを作る。
    let collection: IObjectCollection =
        unsafe { CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER) }
            .map_err(failed("CoCreateInstance(EnumerableObjectCollection)"))?;
    for (index, task) in tasks.iter().enumerate() {
        let icon = icons.get(index).and_then(Option::as_deref);
        let link = shell_link(exe, directory, task, icon).map_err(failed("IShellLinkW"))?;
        // SAFETY: link は作ったショートカットで、collection が参照を持つ。
        unsafe { collection.AddObject(&link) }.map_err(failed("AddObject"))?;
    }
    let array: IObjectArray = collection.cast().map_err(failed("IObjectArray"))?;
    // SAFETY: BeginList が成功した一覧に、ショートカットの並びを渡す。
    unsafe { list.AddUserTasks(&array) }.map_err(failed("AddUserTasks"))
}

/// `task` の項目にするショートカット。アイコンはファイル `icon`、`None` なら実行ファイルの 0 番。
fn shell_link(
    exe: &Path,
    directory: Option<&Path>,
    task: &Task,
    icon: Option<&Path>,
) -> windows::core::Result<IShellLinkW> {
    let exe = HSTRING::from(exe);
    let icon = icon.map_or_else(|| exe.clone(), HSTRING::from);
    // SAFETY: COM を初期化したこのスレッドで、プロセスの中のオブジェクトを作る。渡す文字列は各呼び出しの間は
    // 生きている。
    let link: IShellLinkW = unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&exe)?;
        link.SetArguments(&HSTRING::from(task.arguments.as_str()))?;
        link.SetIconLocation(&icon, 0)?;
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

    /// テストごとに別の、空のフォルダ（一時フォルダの下）。
    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fantermo-jump-list-test-{}-{name}",
            std::process::id()
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        dir
    }

    #[test]
    fn tasks_follow_the_profiles_in_order() {
        let tasks = tasks(&profiles(&["PowerShell", "cmd", "zsh"]));
        let titles: Vec<&str> = tasks.iter().map(|task| task.title.as_str()).collect();
        assert_eq!(titles, ["PowerShell", "cmd", "zsh"]);
        assert_eq!(tasks[0].arguments, r#"--profile "PowerShell""#);
        let positions: Vec<usize> = tasks.iter().map(|task| task.profile).collect();
        assert_eq!(positions, [0, 1, 2]);
    }

    #[test]
    fn names_starting_with_two_dashes_are_skipped() {
        let tasks = tasks(&profiles(&["--new-window", "one", "--", "-x"]));
        let titles: Vec<&str> = tasks.iter().map(|task| task.title.as_str()).collect();
        assert_eq!(titles, ["one", "-x"]);
        // 項目のプロファイルの位置は、飛ばしたものも数えた並びの中の位置。
        let positions: Vec<usize> = tasks.iter().map(|task| task.profile).collect();
        assert_eq!(positions, [1, 3]);
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

    #[test]
    fn icon_files_are_named_by_their_contents() {
        let name = icon_file_name(b"one");
        assert_eq!(name, icon_file_name(b"one"));
        assert_ne!(name, icon_file_name(b"two"));
        // 64 ビットの FNV-1a の 16 進に拡張子。
        assert_eq!(icon_file_name(b""), "cbf29ce484222325.ico");
        assert_eq!(icon_file_name(b"a"), "af63dc4c8601ec8c.ico");
    }

    #[test]
    fn the_icon_folder_is_the_same_for_the_same_executable() {
        let local = Path::new(r"C:\Users\x\AppData\Local");
        let folder = icon_folder(local, Path::new(r"C:\Apps\fantermo\fantermo.exe"));
        assert_eq!(
            folder.parent(),
            Some(Path::new(r"C:\Users\x\AppData\Local\fantermo\jump-list"))
        );
        // 大文字と小文字、`/` と `\` が違っても同じ実行ファイル。
        assert_eq!(
            folder,
            icon_folder(local, Path::new("c:/apps/FANTERMO/fantermo.EXE"))
        );
        // 違う実行ファイルは違うフォルダ。
        assert_ne!(
            folder,
            icon_folder(
                local,
                Path::new(r"C:\devel\fantermo\target\debug\fantermo.exe")
            )
        );
        let name = folder.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), 16);
        assert!(name.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn only_icons_that_the_list_does_not_use_are_removed() {
        let names: Vec<String> = [
            "aaaa.ico",
            "bbbb.ico",
            "CCCC.ICO",
            "notes.txt",
            "dddd.ico.123.tmp",
            "ico",
        ]
        .map(str::to_owned)
        .to_vec();
        let used: HashSet<String> = ["aaaa.ico", "cccc.ico"].map(str::to_owned).into();
        assert_eq!(unused_icons(&names, &used), ["bbbb.ico"]);
        // 使っているものがなければ ICO をすべて消す。
        assert_eq!(
            unused_icons(&names, &HashSet::new()),
            ["aaaa.ico", "bbbb.ico", "CCCC.ICO"]
        );
    }

    #[test]
    fn icons_are_written_once_and_unused_ones_are_removed() {
        let folder = scratch_dir("icons").join("0123456789abcdef");
        let first = write_icon(&folder, b"first").unwrap();
        assert_eq!(first, folder.join(icon_file_name(b"first")));
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        // 同じ名前のファイルがあれば書き直さない。
        std::fs::write(&first, b"changed").unwrap();
        assert_eq!(write_icon(&folder, b"first").unwrap(), first);
        assert_eq!(std::fs::read(&first).unwrap(), b"changed");
        let second = write_icon(&folder, b"second").unwrap();
        let other = folder.join("notes.txt");
        std::fs::write(&other, b"").unwrap();
        let inner = folder.join("inner.ico");
        std::fs::create_dir(&inner).unwrap();
        remove_unused_icons(&folder, &[Some(second.clone()), None]);
        assert!(!first.exists());
        assert!(second.is_file());
        // ICO でないファイルとフォルダは残す。
        assert!(other.is_file());
        assert!(inner.is_dir());
        // フォルダがなければ何もしない。
        remove_unused_icons(&folder.join("missing"), &[]);
        std::fs::remove_dir_all(folder.parent().unwrap()).unwrap();
    }

    /// 描いたアイコンの ICO から、どの大きさもタブと同じピクセルが読み戻せる。同じアイコンは同じファイル。
    #[test]
    fn drawn_icons_are_written_at_every_size() {
        use crate::image_decoder::Decoder;

        /// 乗算済みのアルファとの往復で許す差。
        const TOLERANCE: u8 = 2;
        // SAFETY: テストのスレッドで初期化する。二度目の初期化は S_FALSE が返るだけ。
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .unwrap();
        let generator = Generator::new(&font::create_factory().unwrap()).unwrap();
        let folder = scratch_dir("drawn").join("0123456789abcdef");
        let icon = TabIcon {
            initial: Some("Z".to_owned()),
            ..TabIcon::without_profile()
        };
        let family = "Segoe UI";
        let path = icon_file(Some(&generator), Some(&folder), &icon, family).unwrap();
        assert_eq!(path.parent(), Some(folder.as_path()));
        assert_eq!(
            icon_file(Some(&generator), Some(&folder), &icon, family).unwrap(),
            path
        );
        let decoder = Decoder::new().unwrap();
        for side in ICON_SIDES {
            let drawn = generator.pixels(&icon, side, family).unwrap();
            let read = decoder.decode_icon(&path, side).unwrap();
            assert_eq!((read.width(), read.height()), (side, side));
            assert!(
                read.pixels()
                    .iter()
                    .zip(drawn.pixels())
                    .all(|(read, drawn)| read.abs_diff(*drawn) <= TOLERANCE),
                "{side} px"
            );
        }
        // 描けない・置き場所が分からないときはファイルを作らない。
        assert!(matches!(
            icon_file(None, Some(&folder), &icon, family),
            Err(IconError::NoGenerator)
        ));
        assert!(matches!(
            icon_file(Some(&generator), None, &icon, family),
            Err(IconError::NoFolder)
        ));
        std::fs::remove_dir_all(folder.parent().unwrap()).unwrap();
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
        let icon_file = std::env::temp_dir().join("fantermo-jump-list-test.ico");
        let icon_text = icon_file.to_str().unwrap();
        for (task, icon) in tasks(&profiles(&["a \"quoted\" name", "日本語の名前", "one"]))
            .into_iter()
            .zip([None, Some(icon_file.as_path()), None])
        {
            let link = shell_link(&exe, Some(&directory), &task, icon).unwrap();
            let mut buffer = [0u16; READ_BACK_UNITS];
            let mut index = -1;
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
            // SAFETY: buffer と index は書き込める。
            unsafe { link.GetIconLocation(&mut buffer, &mut index) }.unwrap();
            let expected_icon = if icon.is_some() { icon_text } else { exe_text };
            assert_eq!((text_of(&buffer).as_str(), index), (expected_icon, 0));
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
