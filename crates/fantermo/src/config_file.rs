//! 設定ファイルの場所、読み込み、書き換えの監視。
//!
//! 置き場所は `%APPDATA%\fantermo\config.toml`。起動したときに無ければ、各項目を既定値のまま
//! コメントにして並べたファイルを作る。中身の解釈は `fantermo_core::config` が受け持つ。古い書き方で
//! 書いてあれば、読むたびに今の書き方に読み替え（`fantermo_core::config::migrate`）、変えたところだけを
//! ファイルに書き戻す。
//!
//! 書き換えは、ファイルを置くフォルダを監視するスレッドで知る。エディタは一度の保存で何度も
//! 書き込んだり、別名で書いてから名前を変えたりするので、フォルダの中の名前の変化も見る。

use std::{
    fs::OpenOptions,
    io::{self, Write},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
    thread,
};

use fantermo_core::{
    config::{self, Config, Parsed, Warning},
    config_edit,
};
use windows::{
    Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY,
            FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            ReadDirectoryChangesW,
        },
        UI::Shell::FOLDERID_RoamingAppData,
    },
    core::HSTRING,
};

use crate::{known_folder, window_state};

/// フォルダの変化を受け取るバッファの大きさ（4 バイト単位）。
const NOTIFY_BUFFER_WORDS: usize = 4096;

/// `FILE_NOTIFY_INFORMATION` の、ファイル名より前の部分の大きさ（バイト）。
const NOTIFY_HEADER_BYTES: usize = 12;

/// 開発用。設定ファイルの場所を、この環境変数の値に差し替える。
const CONFIG_PATH_ENV: &str = "FANTERMO_CONFIG";

/// fantermo のファイルを置く、`%APPDATA%` の下のフォルダ。
const APP_DATA_FOLDER: &str = "fantermo";

/// 設定ファイルの名前。
const CONFIG_FILE_NAME: &str = "config.toml";

/// 設定ファイルの場所。[`CONFIG_PATH_ENV`] があればその値、なければ `%APPDATA%\fantermo\config.toml`。
pub fn path() -> Option<PathBuf> {
    app_data_file(CONFIG_PATH_ENV, CONFIG_FILE_NAME)
}

/// fantermo が `%APPDATA%\fantermo` に置くファイル `file_name` の場所。環境変数 `override_env` が
/// あれば、その値に差し替える。
pub fn app_data_file(override_env: &str, file_name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(override_env) {
        return Some(PathBuf::from(path));
    }
    let mut path = known_folder::path(&FOLDERID_RoamingAppData)?;
    path.extend([APP_DATA_FOLDER, file_name]);
    Some(path)
}

/// 読んだ設定と、利用者に見せる警告の文。
#[derive(Debug, Default)]
pub struct Loaded {
    pub config: Config,
    /// 作れなかった・読めなかった・エラーがあったときに、利用者に見せる文。何もなければ `None`。
    pub notice: Option<String>,
    /// 古い書き方を読み替えてファイルに書き戻したときの、書いた中身。書き戻していなければ `None`。
    /// この書き込みの知らせで読み直さないように覚えておく。
    pub written: Option<String>,
}

/// いつ読んだか。エラーがあったときに、どの設定で動いているかの言い方を決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Occasion {
    /// 起動したとき。エラーのある箇所は既定値になる。
    Startup,
    /// 書き換えられて読み直したとき。エラーのある箇所は前の値のまま。
    Reload,
}

impl Occasion {
    /// ファイル全体を読めなかったときに、どの設定で動いているか。
    fn whole(self) -> &'static str {
        match self {
            Occasion::Startup => "既定の設定で動いています",
            Occasion::Reload => "前の設定のまま動いています",
        }
    }

    /// エラーのある項目を、どの値にしたか。
    fn item(self) -> &'static str {
        match self {
            Occasion::Startup => "既定値",
            Occasion::Reload => "前の値",
        }
    }
}

/// 起動したときに設定ファイルを読む。無ければフォルダごと作り、[`config::TEMPLATE`] を書く。作れない
/// とき、読めないときは既定値。読めなかった項目は警告を残して既定値にする。
pub fn load(path: Option<&Path>) -> Loaded {
    if let Some(path) = path
        && let Err(e) = create_if_missing(path)
    {
        tracing::warn!(
            "cannot create the settings file {}, using the defaults: {e}",
            path.display()
        );
        return Loaded {
            config: Config::default(),
            notice: Some(format!(
                "設定ファイルを作れなかったので、{}。\n\n{}\n{e}",
                Occasion::Startup.whole(),
                path.display()
            )),
            written: None,
        };
    }
    read(path, &Config::default(), Occasion::Startup)
}

/// `path` にファイルが無ければ、フォルダごと作って [`config::TEMPLATE`] を書く。あれば何もしない。
fn create_if_missing(path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    // 在るファイルは開かずに失敗させ、書き換えない。
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(()),
        Err(e) => return Err(e),
    };
    if let Err(e) = file.write_all(config::TEMPLATE.as_bytes()) {
        drop(file);
        // 書きかけのファイルを残すと、次に起動したときに読めないファイルとして扱われる。
        if let Err(remove) = std::fs::remove_file(path) {
            tracing::warn!("cannot remove {}: {remove}", path.display());
        }
        return Err(e);
    }
    tracing::info!("created the settings file {}", path.display());
    Ok(())
}

/// 書き換えられた設定ファイルを読み直す。`previous` はいま使っている設定。無いときは既定値。読めない
/// とき、TOML として読めないときは `previous` のまま。読めなかった項目は警告を残して `previous` の値に
/// する。
pub fn reload(path: Option<&Path>, previous: &Config) -> Loaded {
    read(path, previous, Occasion::Reload)
}

fn read(path: Option<&Path>, previous: &Config, occasion: Occasion) -> Loaded {
    let Some(path) = path else {
        return Loaded::default();
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            tracing::info!("no settings file at {}; using the defaults", path.display());
            return Loaded::default();
        }
        Err(e) => {
            tracing::warn!("cannot read the settings file {}: {e}", path.display());
            return Loaded {
                config: previous.clone(),
                notice: Some(format!(
                    "設定ファイルを読めなかったので、{}。\n\n{}\n{e}",
                    occasion.whole(),
                    path.display()
                )),
                written: None,
            };
        }
    };
    let (text, written) = migrate(path, text);
    let parsed = config::parse(&text, previous);
    for warning in &parsed.warnings {
        tracing::warn!("{}", log_line(path, warning));
    }
    if parsed.syntax_error {
        tracing::warn!("kept the settings in use; {} was not read", path.display());
    } else {
        tracing::info!("loaded the settings from {}", path.display());
    }
    Loaded {
        notice: notice_text(path, &parsed, occasion),
        config: parsed.config,
        written,
    }
}

/// 設定ファイル `path` の中身 `text` の古い書き方を今の書き方に読み替え、読み替えたら何をどう変えたかを
/// ログに残してファイルに書き戻す。読み替えた中身と、書き戻したならその中身を返す。書けなければ警告を
/// ログに残し、読み替えた中身で動く（ファイルは前のまま）。
fn migrate(path: &Path, text: String) -> (String, Option<String>) {
    let migrated = config::migrate::migrate(&text);
    if !migrated.changed() {
        return (text, None);
    }
    for (step, change) in &migrated.changes {
        tracing::info!("{}: {}", path.display(), change.message);
        tracing::debug!("{}: {}", step.name, step.description);
        for dropped in &change.dropped {
            tracing::warn!("{}: {dropped}", path.display());
        }
    }
    match window_state::write_replacing(path, &migrated.text) {
        Ok(()) => {
            tracing::info!(
                "rewrote the settings file {} in the current format",
                path.display()
            );
            (migrated.text.clone(), Some(migrated.text))
        }
        Err(e) => {
            tracing::warn!(
                "cannot rewrite the settings file {} in the current format, using the settings read in the current format: {e}",
                path.display()
            );
            (migrated.text, None)
        }
    }
}

/// 設定ファイルの、`base` から `edited` へ変えた項目だけを書き換える（`fantermo_core::config_edit`）。
/// ファイルが無ければ [`config::TEMPLATE`] を書き換えて作る。変える項目がなければ書かない。ファイルの
/// 中身を返す。読めない・TOML として
/// 読めない・書けないときは、利用者に見せる文を返し、ファイルは変えない。
pub fn save(path: &Path, base: &Config, edited: &Config) -> Result<String, String> {
    let failed = |reason: &str, e: &dyn std::fmt::Display| {
        tracing::warn!("cannot save the settings to {}: {e}", path.display());
        format!("{reason}\n\n{}\n{e}", path.display())
    };
    let (text, exists) = match std::fs::read_to_string(path) {
        Ok(text) => (text, true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => (config::TEMPLATE.to_owned(), false),
        Err(e) => {
            return Err(failed(
                "設定ファイルを読めなかったので、書き換えませんでした。",
                &e,
            ));
        }
    };
    let updated = config_edit::update(&text, base, edited).map_err(|e| {
        failed(
            "設定ファイルを TOML として読めなかったので、書き換えませんでした。",
            &e,
        )
    })?;
    // 何も変わらなければ書かない（書き換えの知らせを出さない）。
    if exists && updated == text {
        return Ok(updated);
    }
    if let Err(e) = window_state::write_replacing(path, &updated) {
        return Err(failed("設定ファイルに書けませんでした。", &e));
    }
    tracing::info!("saved the settings to {}", path.display());
    Ok(updated)
}

/// 解釈した結果の警告を、利用者に見せる文にする。警告がなければ `None`。
fn notice_text(path: &Path, parsed: &Parsed, occasion: Occasion) -> Option<String> {
    if parsed.warnings.is_empty() {
        return None;
    }
    let lead = if parsed.syntax_error {
        format!(
            "設定ファイルを TOML として読めなかったので、{}。",
            occasion.whole()
        )
    } else {
        format!(
            "設定ファイルの次の箇所を無視しました。エラーのある項目は{}のままです。",
            occasion.item()
        )
    };
    let mut text = format!("{lead}\n\n{}", path.display());
    for warning in &parsed.warnings {
        text.push('\n');
        if let Some(line) = warning.line {
            text.push_str(&format!("{line} 行目: "));
        }
        text.push_str(&warning.message);
    }
    Some(text)
}

/// ログに残す、警告の 1 行。ファイルの場所と行を添える。
fn log_line(path: &Path, warning: &Warning) -> String {
    match warning.line {
        Some(line) => format!("{}:{line}: {}", path.display(), warning.message),
        None => format!("{}: {}", path.display(), warning.message),
    }
}

/// `path` のファイルが作られた・書き換えられた・消されたときに `notify` を呼ぶスレッドを立てる。
/// 一度の保存で何度も呼ぶことがある。ファイルを置くフォルダが無ければ監視しない。
pub fn watch(path: &Path, notify: impl Fn() + Send + 'static) {
    let Some(name) = path.file_name() else {
        tracing::warn!("not watching {}: no file name", path.display());
        return;
    };
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let handle = match open_directory(dir) {
        Ok(handle) => handle,
        Err(e) => {
            tracing::info!("not watching the settings in {}: {e}", dir.display());
            return;
        }
    };
    let name = name.to_string_lossy().to_lowercase();
    let spawned = thread::Builder::new()
        .name("config-watcher".into())
        .spawn(move || watch_directory(&handle, &name, &notify));
    if let Err(e) = spawned {
        tracing::warn!("failed to start watching the settings: {e}");
    }
}

fn open_directory(dir: &Path) -> windows::core::Result<OwnedHandle> {
    // SAFETY: パスの文字列はこの呼び出しの間は生きている。フォルダを開くには
    // FILE_FLAG_BACKUP_SEMANTICS が要る。ほかのプログラムの読み書きと削除は妨げない。
    let handle = unsafe {
        CreateFileW(
            &HSTRING::from(dir),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )?
    };
    // SAFETY: handle は CreateFileW が返した有効なハンドルで、ほかに持ち主はいない。
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}

/// フォルダの変化を待ち続け、名前が `name`（小文字）のファイルが変わるたびに `notify` を呼ぶ。
fn watch_directory(dir: &OwnedHandle, name: &str, notify: &dyn Fn()) {
    // FILE_NOTIFY_INFORMATION は 4 バイトの境界に並ぶので、u32 の配列で受ける。
    let mut buffer = vec![0u32; NOTIFY_BUFFER_WORDS];
    let buffer_bytes = (buffer.len() * size_of::<u32>()) as u32;
    loop {
        let mut returned = 0u32;
        // SAFETY: dir はフォルダを FILE_LIST_DIRECTORY で開いたハンドル。buffer は buffer_bytes
        // バイト書き込め、同期で呼ぶので戻るまで生きている。
        let result = unsafe {
            ReadDirectoryChangesW(
                HANDLE(dir.as_raw_handle()),
                buffer.as_mut_ptr().cast(),
                buffer_bytes,
                false,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_SIZE,
                Some(&mut returned),
                None,
                None,
            )
        };
        if let Err(e) = result {
            tracing::warn!("stopped watching the settings: {e}");
            return;
        }
        // 0 バイトは、変化がバッファに入りきらなかったとき。どのファイルか分からないので知らせる。
        if returned == 0 {
            notify();
            continue;
        }
        // SAFETY: buffer の先頭 returned バイトは ReadDirectoryChangesW が書いた。u32 の配列を
        // バイトの並びとして読むだけ。
        let bytes =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), returned as usize) };
        if changed_names(bytes).any(|changed| changed.to_lowercase() == name) {
            notify();
        }
    }
}

/// ReadDirectoryChangesW が書いた `FILE_NOTIFY_INFORMATION` の並びから、変わったファイルの名前を取り出す。
/// 壊れた並びは、読めたところまでにする。
fn changed_names(bytes: &[u8]) -> impl Iterator<Item = String> + '_ {
    let read_u32 = |at: usize| -> Option<u32> {
        let field = bytes.get(at..at + size_of::<u32>())?;
        Some(u32::from_le_bytes(field.try_into().ok()?))
    };
    let mut offset = Some(0usize);
    std::iter::from_fn(move || {
        let at = offset?;
        let next = read_u32(at)? as usize;
        let name_bytes = read_u32(at + 2 * size_of::<u32>())? as usize;
        let name_start = at + NOTIFY_HEADER_BYTES;
        let name = bytes.get(name_start..name_start + name_bytes)?;
        let units: Vec<u16> = name
            .chunks_exact(size_of::<u16>())
            .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
            .collect();
        offset = (next != 0).then_some(at + next);
        Some(String::from_utf16_lossy(&units))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `FILE_NOTIFY_INFORMATION` の一つ分。`last` でなければ、次の位置を 4 バイトの境界に合わせて指す。
    fn entry(action: u32, name: &str, last: bool) -> Vec<u8> {
        let name: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let len = (NOTIFY_HEADER_BYTES + name.len()).next_multiple_of(4);
        let next = if last { 0 } else { len as u32 };
        let mut bytes = Vec::new();
        bytes.extend(next.to_le_bytes());
        bytes.extend(action.to_le_bytes());
        bytes.extend((name.len() as u32).to_le_bytes());
        bytes.extend(name);
        bytes.resize(len, 0);
        bytes
    }

    /// テストごとに別の、空のフォルダ（一時フォルダの下）。
    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fantermo-config-test-{}-{name}",
            std::process::id()
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        dir
    }

    #[test]
    fn missing_file_is_created_with_its_folder() {
        let dir = scratch_dir("create");
        let path = dir.join("fantermo").join("config.toml");
        create_if_missing(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), config::TEMPLATE);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn existing_file_is_left_as_it_is() {
        let dir = scratch_dir("keep");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[font]\nsize = 9\n").unwrap();
        create_if_missing(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[font]\nsize = 9\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_writes_only_the_changes() {
        let dir = scratch_dir("save");
        let path = dir.join("fantermo").join("config.toml");
        let mut edited = Config::default();
        edited.font.size_pt = 14.0;
        let written = save(&path, &Config::default(), &edited).unwrap();
        assert_eq!(
            written,
            config::TEMPLATE.replace("# size = 12", "size = 14")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);

        std::fs::write(&path, "# mine\n[font]\nsize = 9\n").unwrap();
        let base = config::parse("[font]\nsize = 9\n", &Config::default()).config;
        save(&path, &base, &edited).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# mine\n[font]\nsize = 14\n"
        );
        // 変える項目がなければ書かない。
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(
            save(&path, &edited, &edited).unwrap(),
            "# mine\n[font]\nsize = 14\n"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );

        std::fs::write(&path, "[font\n").unwrap();
        let error = save(&path, &base, &Config::default()).unwrap_err();
        assert!(
            error.starts_with("設定ファイルを TOML として読めなかった"),
            "{error}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[font\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn old_forms_are_rewritten_in_the_file() {
        let dir = scratch_dir("migrate");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let old = "# mine\n[background]\nimages = ['a.png']\nopacity = 0.8\n";
        let current =
            "# mine\n[background]\nimage_opacity = 0.2\n\n[[background.images]]\npath = 'a.png'\n";
        std::fs::write(&path, old).unwrap();
        let loaded = load(Some(&path));
        assert_eq!(loaded.notice, None);
        assert_eq!(loaded.written.as_deref(), Some(current));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
        assert_eq!(loaded.config.background.images[0].path, "a.png");
        assert_eq!(loaded.config.background.image_opacity, 0.2);
        // 今の書き方なら書かない。
        let again = reload(Some(&path), &loaded.config);
        assert_eq!(again.written, None);
        assert_eq!(again.config, loaded.config);

        // 書けなければファイルはそのままで、読み替えた中身で動く。
        std::fs::write(&path, old).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions.clone()).unwrap();
        let loaded = load(Some(&path));
        assert_eq!(loaded.written, None);
        assert_eq!(loaded.notice, None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
        assert_eq!(loaded.config, again.config);
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn notice_names_the_file_and_the_lines() {
        let path = Path::new(r"C:\settings\config.toml");
        let parsed = config::parse("shell = 1\n[font]\nsize = 0\n", &Config::default());
        assert_eq!(
            notice_text(path, &parsed, Occasion::Reload).as_deref(),
            Some(
                "設定ファイルの次の箇所を無視しました。エラーのある項目は前の値のままです。\n\n\
                 C:\\settings\\config.toml\n\
                 1 行目: ignoring shell: invalid type: integer `1`, expected a string\n\
                 3 行目: ignoring font.size = 0: must be between 1 and 128"
            )
        );

        let parsed = config::parse("[font\n", &Config::default());
        let text = notice_text(path, &parsed, Occasion::Startup).unwrap();
        assert!(
            text.starts_with(
                "設定ファイルを TOML として読めなかったので、既定の設定で動いています。\n\n\
                 C:\\settings\\config.toml\n1 行目: cannot parse the settings: "
            ),
            "{text}"
        );

        let parsed = config::parse(config::TEMPLATE, &Config::default());
        assert_eq!(notice_text(path, &parsed, Occasion::Startup), None);
    }

    #[test]
    fn names_are_read_from_each_entry() {
        const FILE_ACTION_RENAMED_OLD_NAME: u32 = 4;
        const FILE_ACTION_RENAMED_NEW_NAME: u32 = 5;
        let mut bytes = entry(FILE_ACTION_RENAMED_OLD_NAME, "config.toml~", false);
        bytes.extend(entry(FILE_ACTION_RENAMED_NEW_NAME, "Config.toml", true));
        let names: Vec<String> = changed_names(&bytes).collect();
        assert_eq!(names, ["config.toml~", "Config.toml"]);
    }

    #[test]
    fn truncated_entries_stop_the_names() {
        const FILE_ACTION_MODIFIED: u32 = 3;
        let mut bytes = entry(FILE_ACTION_MODIFIED, "a.toml", false);
        bytes.extend(&entry(FILE_ACTION_MODIFIED, "b.toml", true)[..14]);
        let names: Vec<String> = changed_names(&bytes).collect();
        assert_eq!(names, ["a.toml"]);
        assert_eq!(changed_names(&[]).count(), 0);
    }
}
