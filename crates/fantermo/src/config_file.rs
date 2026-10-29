//! 設定ファイルの場所、読み込み、書き換えの監視。
//!
//! 置き場所は `%APPDATA%\fantermo\config.toml`。無ければ既定値で動き、ファイルは作らない。
//! 中身の解釈は `fantermo_core::config` が受け持つ。
//!
//! 書き換えは、ファイルを置くフォルダを監視するスレッドで知る。エディタは一度の保存で何度も
//! 書き込んだり、別名で書いてから名前を変えたりするので、フォルダの中の名前の変化も見る。

use std::{
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
    thread,
};

use fantermo_core::config::{self, Config, Warning};
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

use crate::known_folder;

/// フォルダの変化を受け取るバッファの大きさ（4 バイト単位）。
const NOTIFY_BUFFER_WORDS: usize = 4096;

/// `FILE_NOTIFY_INFORMATION` の、ファイル名より前の部分の大きさ（バイト）。
const NOTIFY_HEADER_BYTES: usize = 12;

/// 開発用。設定ファイルの場所を、この環境変数の値に差し替える。
const CONFIG_PATH_ENV: &str = "FANTERMO_CONFIG";

/// `%APPDATA%` の下の、設定ファイルの場所。
const CONFIG_PATH_COMPONENTS: [&str; 2] = ["fantermo", "config.toml"];

/// 設定ファイルの場所。[`CONFIG_PATH_ENV`] があればその値、なければ `%APPDATA%\fantermo\config.toml`。
pub fn path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(CONFIG_PATH_ENV) {
        return Some(PathBuf::from(path));
    }
    let mut path = known_folder::path(&FOLDERID_RoamingAppData)?;
    path.extend(CONFIG_PATH_COMPONENTS);
    Some(path)
}

/// 起動したときに設定ファイルを読む。無いときや読めないときは既定値。読めなかった項目は警告を残して
/// 既定値にする。
pub fn load(path: Option<&Path>) -> Config {
    read(path, &Config::default())
}

/// 書き換えられた設定ファイルを読み直す。`previous` はいま使っている設定。無いときは既定値。読めない
/// とき、TOML として読めないときは `previous` のまま。読めなかった項目は警告を残して `previous` の値に
/// する。
pub fn reload(path: Option<&Path>, previous: &Config) -> Config {
    read(path, previous)
}

fn read(path: Option<&Path>, previous: &Config) -> Config {
    let Some(path) = path else {
        return Config::default();
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            tracing::info!("no settings file at {}; using the defaults", path.display());
            return Config::default();
        }
        Err(e) => {
            tracing::warn!("cannot read the settings file {}: {e}", path.display());
            return previous.clone();
        }
    };
    let parsed = config::parse(&text, previous);
    for warning in &parsed.warnings {
        tracing::warn!("{}", log_line(path, warning));
    }
    if parsed.syntax_error {
        tracing::warn!("kept the settings in use; {} was not read", path.display());
    } else {
        tracing::info!("loaded the settings from {}", path.display());
    }
    parsed.config
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
