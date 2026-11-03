//! fantermo が `%APPDATA%\fantermo` に置くファイルの場所と、ファイルの書き込み。
//!
//! 設定ファイル（`config_file`）と状態ファイル（`window_state`）、画面の書き出し（`dump`）が共有する。

use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

use windows::Win32::UI::Shell::FOLDERID_RoamingAppData;

use crate::known_folder;

/// fantermo のファイルを置く、`%APPDATA%` の下のフォルダ。
const APP_DATA_FOLDER: &str = "fantermo";

/// 書き込みの途中のファイルの、名前の後ろに足す拡張子。書き終えてから本来のファイルと置き換える。
const TEMPORARY_EXTENSION: &str = "tmp";

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

/// `path` の名前の後ろに `suffix` を足した場所。
pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

/// `path` を書き換えるときの一時ファイルの場所。名前は `<名前>.<プロセスの番号>.tmp`。
/// 二つの fantermo が同時に書いても、互いの一時ファイルを使わない。
fn temporary_path(path: &Path, process_id: u32) -> PathBuf {
    with_suffix(path, &format!(".{process_id}.{TEMPORARY_EXTENSION}"))
}

/// `path` に `text` を書く。フォルダが無ければ作る。同じフォルダの一時ファイルに書いてから置き換える
/// ので、途中で止まっても前のファイルは壊れない。
pub fn write_replacing(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let temporary = temporary_path(path, std::process::id());
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, path))
        .inspect_err(|_| remove_temporary(&temporary))
}

/// 置き換えに失敗した一時ファイル `temporary` を消す。名前にプロセスの番号が入っていて、
/// 二度と上書きされないため。
fn remove_temporary(temporary: &Path) {
    match std::fs::remove_file(temporary) {
        Ok(()) => {}
        // 書き始める前に失敗していれば、ファイルはない。
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("cannot remove {}: {e}", temporary.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テストごとに別の、空のフォルダ（一時フォルダの下）。
    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fantermo-files-test-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        dir
    }

    #[test]
    fn temporary_path_carries_the_process_id() {
        let path = Path::new("dir").join("state.toml");
        assert_eq!(
            temporary_path(&path, 1234),
            Path::new("dir").join("state.toml.1234.tmp")
        );
        assert_ne!(temporary_path(&path, 1), temporary_path(&path, 2));
    }

    #[test]
    fn write_replacing_creates_the_folder_and_leaves_no_temporary_file() {
        let dir = scratch_dir("create");
        let path = dir.join("fantermo").join("state.toml");
        write_replacing(&path, "text").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "text");
        assert!(!temporary_path(&path, std::process::id()).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_replacement_removes_the_temporary_file() {
        let dir = scratch_dir("failed-replacement");
        // 置き換え先がフォルダだと、書くのは成功して置き換えが失敗する。
        let path = dir.join("state.toml");
        std::fs::create_dir_all(&path).unwrap();
        assert!(write_replacing(&path, "text").is_err());
        assert!(!temporary_path(&path, std::process::id()).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
