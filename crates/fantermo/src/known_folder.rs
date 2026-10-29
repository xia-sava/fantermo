//! Windows の既知のフォルダ（`%APPDATA%` など）の場所。

use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};

use windows::{
    Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath},
    },
    core::GUID,
};

/// いまの利用者の、`id` が指す既知のフォルダ。求められなければ `None`。
pub fn path(id: &GUID) -> Option<PathBuf> {
    // SAFETY: 既知のフォルダの GUID を指すポインタを渡す問い合わせ。トークンを渡さないのでいまの利用者のもの。
    let path = match unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) } {
        Ok(path) => path,
        Err(e) => {
            tracing::warn!("failed to get the known folder {id:?}: {e}");
            return None;
        }
    };
    // SAFETY: SHGetKnownFolderPath が成功したので、path は NUL で終わる文字列を指す。
    let wide = OsString::from_wide(unsafe { path.as_wide() });
    // SAFETY: SHGetKnownFolderPath が CoTaskMemAlloc で確保した文字列で、ほかからは使わない。
    unsafe { CoTaskMemFree(Some(path.0 as *const _)) };
    Some(PathBuf::from(wide))
}
