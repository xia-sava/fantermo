//! fantermo — libghostty-vt を端末エミュレーションに使う Windows 用の端末。
//!
//! 引数があれば、それを子プロセスのコマンドラインにする（例: `fantermo.exe C:\app\cygwin\bin\zsh.exe -l`）。
//! 引数がなければ設定ファイルの `shell` を、それもなければ PowerShell を起動する。新しいタブも同じ
//! 決め方で起動する。設定ファイルは `%APPDATA%\fantermo\config.toml`（`config_file`）。
//!
//! リリースのビルドはコンソールの窓を持たない GUI のアプリにし、ログはファイルに出す（`logging`）。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod atlas;
mod boxdraw;
mod clipboard;
mod config_file;
mod conpty;
mod context_menu;
mod decoration;
mod dump;
mod font;
mod ime;
mod keyboard;
mod known_folder;
mod link_opener;
mod logging;
mod notice;
mod paste_editor;
mod pipeline;
mod renderer;
mod session;
mod shader;
mod tab_bar;
mod window;

use std::path::Path;

use windows::Win32::{
    System::Environment::GetCommandLineW,
    UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
};

fn main() {
    let log_dir = logging::init();

    // SAFETY: 起動直後、窓を作る前に一度だけ呼ぶ。
    let dpi_awareness =
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if let Err(e) = dpi_awareness {
        tracing::warn!("failed to enable per-monitor DPI awareness: {e}");
    }

    let config_path = config_file::path();
    let loaded = config_file::load(config_path.as_deref());
    match window::run(child_command_line(), config_path, loaded) {
        Ok(code) => std::process::exit(code as i32),
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

/// このプロセスのコマンドラインから、プログラム名を除いた残り。引用符はそのまま子プロセスに渡す。
fn child_command_line() -> Option<String> {
    // SAFETY: プロセスが持つコマンドラインを指すポインタが返り、プロセスが終わるまで有効。
    let raw = unsafe { GetCommandLineW().to_string() }.ok()?;
    let rest = skip_program_name(&raw).trim_start();
    (!rest.is_empty()).then(|| rest.to_owned())
}

/// コマンドラインの先頭のプログラム名を飛ばす。プログラム名は引用符で囲まれていれば次の引用符まで、
/// そうでなければ最初の空白までで、中の `\` は特別扱いしない（`CommandLineToArgvW` と同じ）。
fn skip_program_name(command_line: &str) -> &str {
    if let Some(quoted) = command_line.strip_prefix('"') {
        return quoted.find('"').map_or("", |end| &quoted[end + 1..]);
    }
    command_line
        .find([' ', '\t'])
        .map_or("", |end| &command_line[end..])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{skip_program_name, startup_error_text};

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

    #[test]
    fn program_name_is_skipped() {
        assert_eq!(skip_program_name("fantermo.exe"), "");
        assert_eq!(skip_program_name("fantermo.exe zsh -l"), " zsh -l");
        assert_eq!(
            skip_program_name(r#""C:\Program Files\fantermo.exe" "a b" c"#),
            r#" "a b" c"#
        );
    }
}
