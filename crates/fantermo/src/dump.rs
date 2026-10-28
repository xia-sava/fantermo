//! 画面の文字とバックバッファを、環境変数で指定したディレクトリへ定期的に書き出す。
//!
//! Direct3D の窓は窓の DC から撮れないので、画面を使わずに描画を確かめるときに使う。

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use fantermo_core::vt::Vt;

use crate::font::Font;

/// 書き出し先のディレクトリを指定する環境変数。
pub const DUMP_DIR_ENV: &str = "FANTERMO_DUMP";

/// 書き出す間隔。
pub const DUMP_INTERVAL: Duration = Duration::from_millis(500);

const SCREEN_FILE: &str = "screen.txt";
const FRAME_FILE: &str = "frame.png";

pub struct ScreenDump {
    dir: PathBuf,
}

impl ScreenDump {
    pub fn from_env() -> Option<Self> {
        let dir = PathBuf::from(std::env::var_os(DUMP_DIR_ENV)?);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("cannot create the dump directory {}: {e}", dir.display());
            return None;
        }
        Some(Self { dir })
    }

    /// バックバッファの書き出し先。
    pub fn frame_path(&self) -> PathBuf {
        self.dir.join(FRAME_FILE)
    }

    /// 画面の文字を、大きさとフォントの情報を先頭に付けて書き出す。
    pub fn write_screen(&self, vt: &Vt, font: &Font) {
        let text = match vt.plain_text() {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!("failed to format the screen: {e}");
                return;
            }
        };
        let size = vt.size();
        let contents = format!(
            "cols={} rows={} cell={}x{} font={} size={}pt\n{text}",
            size.cols,
            size.rows,
            size.cell_width_px,
            size.cell_height_px,
            font.family,
            font.size_pt
        );
        write_file(&self.dir.join(SCREEN_FILE), contents.as_bytes());
    }
}

fn write_file(path: &Path, contents: &[u8]) {
    if let Err(e) = std::fs::write(path, contents) {
        tracing::warn!("failed to write {}: {e}", path.display());
    }
}
