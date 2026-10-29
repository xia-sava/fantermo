//! ファイルとフォルダを選ぶ Windows のダイアログ（`IFileOpenDialog`）。呼ぶスレッドは COM を初期化して
//! いること（main）。

use std::path::PathBuf;

use windows::{
    Win32::{
        Foundation::{ERROR_CANCELLED, HWND},
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
        UI::Shell::{
            Common::COMDLG_FILTERSPEC, FILEOPENDIALOGOPTIONS, FOS_ALLOWMULTISELECT,
            FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog,
            IFileOpenDialog, IShellItem, SIGDN_FILESYSPATH,
        },
    },
    core::{HRESULT, HSTRING, PCWSTR},
};

/// ファイルの種類の絞り込み。見せる名前と、`*.png;*.jpg` の形の名前の型。
pub struct Filter {
    pub name: String,
    pub spec: String,
}

/// ファイルを一つ以上選ぶダイアログを `owner` の上に開く。選んだファイルの場所を返し、やめたら空。
pub fn open_files(
    owner: HWND,
    title: &str,
    filters: &[Filter],
) -> windows::core::Result<Vec<PathBuf>> {
    let dialog = create(title, FOS_ALLOWMULTISELECT | FOS_FILEMUSTEXIST)?;
    let names: Vec<(HSTRING, HSTRING)> = filters
        .iter()
        .map(|filter| (HSTRING::from(&filter.name), HSTRING::from(&filter.spec)))
        .collect();
    let specs: Vec<COMDLG_FILTERSPEC> = names
        .iter()
        .map(|(name, spec)| COMDLG_FILTERSPEC {
            pszName: PCWSTR(name.as_ptr()),
            pszSpec: PCWSTR(spec.as_ptr()),
        })
        .collect();
    if !specs.is_empty() {
        // SAFETY: specs が指す文字列は names が持ち、この呼び出しの間は生きている。
        unsafe { dialog.SetFileTypes(&specs)? };
    }
    if !show(&dialog, owner)? {
        return Ok(Vec::new());
    }
    // SAFETY: Show が選んで閉じたので、結果を問い合わせられる。
    let results = unsafe { dialog.GetResults()? };
    // SAFETY: results は有効な一覧。
    let count = unsafe { results.GetCount()? };
    let mut paths = Vec::new();
    for index in 0..count {
        // SAFETY: index は一覧の数より小さい。
        let item = unsafe { results.GetItemAt(index)? };
        paths.push(file_system_path(&item)?);
    }
    Ok(paths)
}

/// フォルダを一つ選ぶダイアログを `owner` の上に開く。やめたら `None`。
pub fn open_folder(owner: HWND, title: &str) -> windows::core::Result<Option<PathBuf>> {
    let dialog = create(title, FOS_PICKFOLDERS)?;
    if !show(&dialog, owner)? {
        return Ok(None);
    }
    // SAFETY: Show が選んで閉じたので、結果を問い合わせられる。
    let item = unsafe { dialog.GetResult()? };
    file_system_path(&item).map(Some)
}

/// 題が `title` で、ファイルシステムのものだけを選べるダイアログを作り、`options` を足す。
fn create(title: &str, options: FILEOPENDIALOGOPTIONS) -> windows::core::Result<IFileOpenDialog> {
    // SAFETY: このスレッドは COM を初期化している。
    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)? };
    // SAFETY: dialog は作ったばかりのダイアログで、文字列はこの呼び出しの間は生きている。
    unsafe {
        let current = dialog.GetOptions()?;
        dialog.SetOptions(current | options | FOS_FORCEFILESYSTEM)?;
        dialog.SetTitle(&HSTRING::from(title))?;
    }
    Ok(dialog)
}

/// ダイアログを開き、閉じるまで待つ。選んで閉じたら `true`、やめたら `false`。
fn show(dialog: &IFileOpenDialog, owner: HWND) -> windows::core::Result<bool> {
    // SAFETY: owner は呼び出し元の有効な窓。借用は持っていない。
    match unsafe { dialog.Show(Some(owner)) } {
        Ok(()) => Ok(true),
        Err(e) if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) => Ok(false),
        Err(e) => Err(e),
    }
}

/// `item` のファイルシステムの場所。
fn file_system_path(item: &IShellItem) -> windows::core::Result<PathBuf> {
    // SAFETY: item は有効な項目。返る文字列は CoTaskMemFree で解放する。
    unsafe {
        let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = name.to_string();
        CoTaskMemFree(Some(name.0.cast_const().cast()));
        Ok(PathBuf::from(path?))
    }
}
