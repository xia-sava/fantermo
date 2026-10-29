//! ログの出し先。
//!
//! 開発中のビルド（`debug_assertions` があるとき）はコンソールに出す。リリースのビルドはコンソールを
//! 持たないので、`%LOCALAPPDATA%\fantermo\logs\` の下に日付ごとのファイルで出す。どちらも環境変数
//! `RUST_LOG` で水準を変えられる。

use std::path::PathBuf;

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use windows::Win32::UI::Shell::FOLDERID_LocalAppData;

use crate::known_folder;

/// ログの既定の水準。環境変数 `RUST_LOG` で変えられる。
const DEFAULT_LOG_FILTER: &str = "info";

/// `%LOCALAPPDATA%` の下に作る、ログを置くフォルダ。
const LOG_DIR_COMPONENTS: [&str; 2] = ["fantermo", "logs"];

/// ログのファイルの名前の前半。後ろに `.<年-月-日>.log` が付く。
const LOG_FILE_PREFIX: &str = "fantermo";

/// ログのファイルの拡張子。
const LOG_FILE_SUFFIX: &str = "log";

/// 残しておくログのファイルの数。1 日に 1 つできるので、動かした日の数にあたる。
const MAX_LOG_FILES: usize = 14;

/// ログの出し先を設定する。リリースのビルドでファイルに出せたときは、そのフォルダを返す。
///
/// ファイルに出せないとき（フォルダを作れないなど）は、ログを出さずに続ける。
pub fn init() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        tracing_subscriber::fmt().with_env_filter(filter()).init();
        return None;
    }
    let dir = log_dir()?;
    // 古いファイルを消すときにフォルダを読むので、先に作っておく。
    std::fs::create_dir_all(&dir).ok()?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix(LOG_FILE_SUFFIX)
        .max_log_files(MAX_LOG_FILES)
        .build(&dir)
        .ok()?;
    tracing_subscriber::fmt()
        .with_env_filter(filter())
        .with_ansi(false)
        .with_writer(appender)
        .init();
    log_panics();
    Some(dir)
}

fn filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER))
}

/// ログを置くフォルダ（`%LOCALAPPDATA%\fantermo\logs`）。
pub fn log_dir() -> Option<PathBuf> {
    let mut dir = known_folder::path(&FOLDERID_LocalAppData)?;
    dir.extend(LOG_DIR_COMPONENTS);
    Some(dir)
}

/// パニックの内容をログに残す。コンソールがないと、標準エラー出力に出たパニックは見えない。
fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("{info}");
        previous(info);
    }));
}
