//! fantermo — libghostty-vt を端末エミュレーションに使う Windows 用の端末。
//!
//! 引数があれば、それを子プロセスのコマンドラインにする（例: `fantermo.exe C:\app\cygwin\bin\zsh.exe -l`）。
//! 引数の先頭の `--profile <名前>` でプロファイルを指定できる（`command_line`）。
//! どちらもなければ設定ファイルの既定のプロファイルを、プロファイルを書いていなければ PowerShell を起動する。
//! 新しいタブも同じ決め方で起動する。設定ファイルは `%APPDATA%\fantermo\config.toml`（`config_file`）。
//! すでに動いているウィンドウがあれば、新しいウィンドウを開かず、そこへタブを足して終わる（`instance`）。
//! `--new-window` があれば新しいウィンドウを開く。
//! ウィンドウの位置・大きさ・最大化は、閉じるときに同じフォルダの `state.toml` に書き、次に起動した
//! ときに戻す（`window_state`）。
//!
//! リリースのビルドはコンソールのウィンドウを持たない GUI のアプリにし、ログはファイルに出す（`logging`）。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod about;
mod app_files;
mod app_icon;
mod atlas;
mod background;
mod boxdraw;
mod bridge;
mod clipboard;
mod clipboard_prompt;
mod command_line;
mod config_file;
mod conpty;
mod context_menu;
mod d2d_shape;
mod decoration;
mod dialog;
mod dump;
#[cfg(feature = "e2e")]
mod e2e_server;
mod file_dialog;
mod font;
mod image_cache;
mod image_decoder;
mod ime;
mod instance;
mod key_capture;
mod keyboard;
mod known_folder;
mod link_opener;
mod logging;
mod modal;
mod notice;
mod output_budget;
mod paint;
mod paste_editor;
mod pipeline;
mod process;
mod renderer;
mod scroll_bar;
mod session;
mod settings_dialog;
mod shader;
mod tab_bar;
mod tab_icon;
mod text_dialog;
mod text_placement;
mod ui_font;
mod update;
mod version;
mod window;
mod window_state;

use std::path::Path;

use windows::Win32::{
    System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx},
    UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
};

fn main() {
    let log_dir = logging::init();
    tracing::info!("fantermo {}", version::display());

    let options = command_line::Options::of_process();
    let config_path = config_file::path();
    if !options.new_window && instance::forward(&options, config_path.as_deref()) {
        return;
    }

    // SAFETY: 起動直後、ウィンドウを作る前に一度だけ呼ぶ。
    let dpi_awareness =
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if let Err(e) = dpi_awareness {
        tracing::warn!("failed to enable per-monitor DPI awareness: {e}");
    }
    // 背景画像を読む WIC と、ファイルを選ぶダイアログを、ウィンドウのスレッドで使う。プロセスが終わるまで
    // 解放しない。
    // SAFETY: ウィンドウのスレッド（このスレッド）で、ウィンドウを作る前に一度だけ初期化する。
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    if let Err(e) = com.ok() {
        tracing::warn!("failed to initialize COM: {e}");
    }

    let loaded = config_file::load(config_path.as_deref());
    let state_path = window_state::path();
    let state = window_state::load(state_path.as_deref());
    match window::run(options, config_path, loaded, state_path, state) {
        Ok(code) => {
            conpty::wait_for_closing_consoles();
            std::process::exit(code as i32)
        }
        Err(e) => {
            tracing::error!("{e}");
            // 開発中のビルドはコンソールに出たログで分かる。リリースのビルドにはコンソールがない。
            if !cfg!(debug_assertions) {
                notice::error(&startup_error_text(&e.to_string(), log_dir.as_deref()));
            }
            std::process::exit(1);
        }
    }
}

fn startup_error_text(error: &str, log_dir: Option<&Path>) -> String {
    let mut text = format!("fantermo を起動できませんでした。\n\n{error}");
    if let Some(dir) = log_dir {
        text.push_str(&format!("\n\nログ: {}", dir.display()));
    }
    text
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::startup_error_text;

    #[test]
    fn startup_error_names_log_dir_when_logging_to_file() {
        assert_eq!(
            startup_error_text("no font", None),
            "fantermo を起動できませんでした。\n\nno font"
        );
        assert_eq!(
            startup_error_text("no font", Some(Path::new(r"C:\logs"))),
            "fantermo を起動できませんでした。\n\nno font\n\nログ: C:\\logs"
        );
    }
}
