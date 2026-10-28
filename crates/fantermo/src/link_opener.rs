//! 画面のリンクを開く。
//!
//! `http`・`https`・`mailto` は既定のアプリに渡し、`file` はエクスプローラーで見せる（ファイルそのものは
//! 開かない。実行ファイルを指していても走らせないため）。開いてよいかは `fantermo_core::link` で決める。

use std::path::Path;

use fantermo_core::link::{self, OpenTarget};
use windows::{
    Win32::{
        System::Com::{
            COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
        },
        UI::{
            Shell::{FOLDERID_Windows, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::known_folder;

/// エクスプローラーの実行ファイル。Windows のフォルダの中にある。
const EXPLORER: &str = "explorer.exe";

/// `uri` を開く。開いてよいリンクでなければ、警告を残して何もしない。
///
/// 既定のアプリを探して起動するまで時間がかかり、その間にメッセージを回すこともあるので、
/// 窓のスレッドではなく別のスレッドで開く。
pub fn open(uri: &str) {
    let target = match link::open_target(uri) {
        Ok(target) => target,
        Err(reason) => {
            tracing::warn!("refusing to open a link: {reason}");
            tracing::debug!("refused link: {uri:?}");
            return;
        }
    };
    let spawned = std::thread::Builder::new()
        .name("link-opener".into())
        .spawn(move || open_on_this_thread(&target));
    if let Err(e) = spawned {
        tracing::warn!("failed to start a thread to open a link: {e}");
    }
}

fn open_on_this_thread(target: &OpenTarget) {
    // ShellExecuteExW はシェルの拡張を COM で呼ぶことがあるので、呼ぶスレッドで COM を初期化しておく。
    // SAFETY: このスレッドで一度だけ初期化し、終わりに対にして解放する。
    let initialized =
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    if let Err(e) = initialized.ok() {
        tracing::warn!("failed to initialize COM to open a link: {e}");
        return;
    }
    let result = match target {
        OpenTarget::Uri(uri) => shell_execute(uri, None).map_err(Into::into),
        OpenTarget::Path(path) => show_in_explorer(path),
    };
    if let Err(e) = result {
        tracing::warn!("failed to open a link: {e}");
    }
    // SAFETY: 上で初期化に成功したので、対にして解放する。
    unsafe { CoUninitialize() };
}

/// フォルダならエクスプローラーで開き、ファイルなら入っているフォルダを開いてそのファイルを選ぶ。
fn show_in_explorer(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = std::fs::metadata(Path::new(path))?;
    // パスに `"` は入らない（`link::open_target` が除く）ので、引用符で囲めば一つの引数になる。
    let parameters = if metadata.is_dir() {
        format!("\"{path}\"")
    } else {
        format!("/select,\"{path}\"")
    };
    // 名前だけで渡すと作業フォルダから探されるので、Windows のフォルダの中のものを場所で指す。
    let explorer = known_folder::path(&FOLDERID_Windows)
        .ok_or("the Windows folder is unknown")?
        .join(EXPLORER);
    shell_execute(&explorer.to_string_lossy(), Some(&parameters))?;
    Ok(())
}

/// `file` を既定の動作（open）で開く。`parameters` はコマンドラインの引数。
fn shell_execute(file: &str, parameters: Option<&str>) -> windows::core::Result<()> {
    let file = HSTRING::from(file);
    let parameters = parameters.map(HSTRING::from);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // 起動を待ってから戻る。このスレッドはすぐに終わるため。
        fMask: SEE_MASK_NOASYNC,
        lpVerb: w!("open"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: parameters
            .as_ref()
            .map_or(PCWSTR::null(), |parameters| PCWSTR(parameters.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: info は初期化済みで、指す文字列はこの呼び出しの間は生きている。
    unsafe { ShellExecuteExW(&mut info) }
}
