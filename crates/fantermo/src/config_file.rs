//! 設定ファイルの場所と読み込み。
//!
//! 置き場所は `%APPDATA%\fantermo\config.toml`。無ければ既定値で動き、ファイルは作らない。
//! 中身の解釈は `fantermo_core::config` が受け持つ。

use std::{
    io,
    path::{Path, PathBuf},
};

use fantermo_core::config::{self, Config};
use windows::Win32::UI::Shell::FOLDERID_RoamingAppData;

use crate::known_folder;

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

/// 設定ファイルを読む。無いときや読めないときは既定値。読めなかった項目は警告を残して既定値にする。
pub fn load(path: Option<&Path>) -> Config {
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
            tracing::warn!(
                "cannot read the settings file {}, using the defaults: {e}",
                path.display()
            );
            return Config::default();
        }
    };
    let parsed = config::parse(&text);
    for warning in &parsed.warnings {
        tracing::warn!("{}: {warning}", path.display());
    }
    tracing::info!("loaded the settings from {}", path.display());
    parsed.config
}
