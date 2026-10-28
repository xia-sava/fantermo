//! fantermo — libghostty-vt を端末エミュレーションに使う Windows 用の端末。
//!
//! 引数がなければ PowerShell を起動する。引数があれば、それを子プロセスのコマンドラインにする
//! （例: `fantermo.exe C:\app\cygwin\bin\zsh.exe -l`）。

mod atlas;
mod boxdraw;
mod clipboard;
mod conpty;
mod context_menu;
mod decoration;
mod dump;
mod font;
mod ime;
mod keyboard;
mod paste_editor;
mod pipeline;
mod renderer;
mod shader;
mod window;

use tracing_subscriber::EnvFilter;
use windows::Win32::{
    System::Environment::GetCommandLineW,
    UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
};

/// 引数がないときに起動するコマンドライン。
const DEFAULT_COMMAND_LINE: &str = "powershell.exe -NoLogo";

/// ログの既定の水準。環境変数 `RUST_LOG` で変えられる。
const DEFAULT_LOG_FILTER: &str = "info";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER)),
        )
        .init();

    // SAFETY: 起動直後、窓を作る前に一度だけ呼ぶ。
    let dpi_awareness =
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if let Err(e) = dpi_awareness {
        tracing::warn!("failed to enable per-monitor DPI awareness: {e}");
    }

    let command_line = child_command_line().unwrap_or_else(|| DEFAULT_COMMAND_LINE.to_owned());
    match window::run(&command_line) {
        Ok(code) => std::process::exit(code as i32),
        Err(e) => {
            tracing::error!("{e}");
            std::process::exit(1);
        }
    }
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
    use super::skip_program_name;

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
