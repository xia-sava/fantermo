//! アプリの中からの更新。
//!
//! 更新できるのは、インストーラ（MSI）で `%LOCALAPPDATA%\Programs\fantermo` に入れたリリースのビルド
//! だけ。zip の版と開発中のビルドでは「fantermo について」の「更新を確認」を押せない。

use std::path::Path;

use windows::Win32::UI::Shell::FOLDERID_UserProgramFiles;

use crate::{known_folder, version};

/// インストーラが fantermo を入れる、`%LOCALAPPDATA%\Programs` の下のフォルダ。
const INSTALL_DIR_NAME: &str = "fantermo";

/// この fantermo を更新できるか。インストーラで入れたリリースのビルドなら真。
pub fn can_update() -> bool {
    if !version::RELEASE {
        return false;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            tracing::warn!("failed to get the path of the executable: {e}");
            return false;
        }
    };
    let Some(programs) = known_folder::path(&FOLDERID_UserProgramFiles) else {
        return false;
    };
    is_installed_build(version::RELEASE, &exe, &programs)
}

/// リリースのビルド（`release`）で、実行ファイル `exe` がインストーラの入れる場所（`programs` の下の
/// `fantermo`）にあるか。Windows のパスなので、大文字と小文字は区別しない。
fn is_installed_build(release: bool, exe: &Path, programs: &Path) -> bool {
    let Some(dir) = exe.parent() else {
        return false;
    };
    let install_dir = programs.join(INSTALL_DIR_NAME);
    release && same_path(dir, &install_dir)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_lowercase()
    };
    normalize(a) == normalize(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROGRAMS: &str = r"C:\Users\xia\AppData\Local\Programs";

    #[test]
    fn only_release_builds_in_the_install_dir_can_update() {
        let programs = Path::new(PROGRAMS);
        let installed = Path::new(r"C:\Users\xia\AppData\Local\Programs\fantermo\fantermo.exe");
        assert!(is_installed_build(true, installed, programs));
        assert!(!is_installed_build(false, installed, programs));
        let unzipped = Path::new(r"C:\Users\xia\Downloads\fantermo-0.0.1-x64\fantermo.exe");
        assert!(!is_installed_build(true, unzipped, programs));
        let nested = Path::new(r"C:\Users\xia\AppData\Local\Programs\fantermo\bin\fantermo.exe");
        assert!(!is_installed_build(true, nested, programs));
    }

    #[test]
    fn install_dir_ignores_case_and_trailing_separators() {
        let exe = Path::new(r"c:\users\XIA\appdata\local\programs\Fantermo\fantermo.exe");
        assert!(is_installed_build(true, exe, Path::new(PROGRAMS)));
        let programs = format!(r"{PROGRAMS}\");
        assert!(is_installed_build(true, exe, Path::new(&programs)));
    }
}
