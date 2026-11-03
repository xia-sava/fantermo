//! 背景画像の候補を実際のファイルシステムから探す。候補の展開と選び方、表示方法は
//! `fantermo_core::background`、画像のファイルを読む処理は `image_decoder`。

use std::path::{Path, PathBuf};

use fantermo_core::background::Files;

/// 実際のファイルシステム。
pub struct DiskFiles;

impl Files for DiskFiles {
    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn files_in(&self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
        Ok(entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect())
    }
}
