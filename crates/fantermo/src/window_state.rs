//! ウィンドウの状態ファイルの場所と読み書き、ウィンドウの位置・大きさ・最大化との受け渡し。
//!
//! 置き場所は `%APPDATA%\fantermo\state.toml`（設定ファイルと同じフォルダ）。ウィンドウを閉じるときと、
//! ログオフ・シャットダウンで終わらされるときに書き、
//! 起動したときに一度だけ読む（書き換えを見張らない）。中身の型と解釈は `fantermo_core::state` が
//! 受け持つ。

use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

use fantermo_core::state::{self, ParseError, State, WindowState};
use windows::Win32::{
    Foundation::{HWND, POINT, RECT},
    UI::WindowsAndMessaging::{
        GetWindowPlacement, SW_HIDE, SW_SHOWMAXIMIZED, SetWindowPlacement, WINDOWPLACEMENT,
        WINDOWPLACEMENT_FLAGS,
    },
};

use crate::config_file;

/// 開発用。状態ファイルの場所を、この環境変数の値に差し替える。
const STATE_PATH_ENV: &str = "FANTERMO_STATE";

/// 状態ファイルの名前。
const STATE_FILE_NAME: &str = "state.toml";

/// 読めなかった状態ファイルを写しておくファイルの、名前の後ろに足すもの。
const BROKEN_SUFFIX: &str = ".broken";

/// 書き込みの途中のファイルの、名前の後ろに足すもの。書き終えてから状態ファイルと置き換える。
const TEMPORARY_SUFFIX: &str = ".tmp";

/// `WINDOWPLACEMENT` の、最大化したときの位置を指定しないことを表す値。
const UNSPECIFIED_POSITION: POINT = POINT { x: -1, y: -1 };

/// 状態ファイルの場所。[`STATE_PATH_ENV`] があればその値、なければ `%APPDATA%\fantermo\state.toml`。
pub fn path() -> Option<PathBuf> {
    config_file::app_data_file(STATE_PATH_ENV, STATE_FILE_NAME)
}

/// 状態ファイルを読む。無ければ（初回の起動）既定値。TOML として読めなければ、中身を
/// `state.toml.broken` へ写してから既定値にする（原本は動かさない）。
pub fn load(path: Option<&Path>) -> State {
    let Some(path) = path else {
        return State::default();
    };
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            tracing::info!("no window state at {}; using the defaults", path.display());
            return State::default();
        }
        Err(e) => {
            tracing::warn!("cannot read the window state {}: {e}", path.display());
            return State::default();
        }
    };
    let parsed = match std::str::from_utf8(&bytes) {
        Ok(text) => state::parse(text),
        Err(e) => Err(ParseError::Syntax(e.to_string())),
    };
    match parsed {
        Ok(state) => {
            tracing::info!("loaded the window state from {}", path.display());
            state
        }
        Err(ParseError::Syntax(message)) => {
            set_aside_broken(path, &bytes);
            tracing::warn!(
                "cannot parse the window state {}, using the defaults: {message}",
                path.display()
            );
            State::default()
        }
        Err(ParseError::Invalid(message)) => {
            tracing::warn!("ignoring the window state {}: {message}", path.display());
            State::default()
        }
    }
}

/// `path` の名前の後ろに `suffix` を足した場所。
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

/// 読めなかった中身 `bytes` を、`path` の名前に `.broken` を足したファイルへ写す。
fn set_aside_broken(path: &Path, bytes: &[u8]) {
    let broken = with_suffix(path, BROKEN_SUFFIX);
    match std::fs::write(&broken, bytes) {
        Ok(()) => tracing::warn!("copied the unreadable window state to {}", broken.display()),
        Err(e) => tracing::warn!("cannot write {}: {e}", broken.display()),
    }
}

/// 状態を書く。フォルダが無ければ作る。同じフォルダの一時ファイルに書いてから置き換えるので、
/// 途中で止まっても前の状態ファイルは壊れない。
pub fn save(path: &Path, state: &State) {
    match write_replacing(path, &state::to_toml(state)) {
        Ok(()) => tracing::info!("saved the window state to {}", path.display()),
        Err(e) => tracing::warn!("cannot save the window state {}: {e}", path.display()),
    }
}

/// `path` に `text` を書く。フォルダが無ければ作る。同じフォルダの一時ファイルに書いてから置き換える
/// ので、途中で止まっても前のファイルは壊れない。
pub fn write_replacing(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let temporary = with_suffix(path, TEMPORARY_SUFFIX);
    std::fs::write(&temporary, text)?;
    if let Err(e) = std::fs::rename(&temporary, path) {
        if let Err(remove) = std::fs::remove_file(&temporary) {
            tracing::warn!("cannot remove {}: {remove}", temporary.display());
        }
        return Err(e);
    }
    Ok(())
}

/// ウィンドウのいまの位置・大きさ・最大化。最小化していれば、最大化していないものとする。
pub fn current(hwnd: HWND) -> Option<WindowState> {
    let mut placement = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: hwnd は有効なウィンドウで、placement は length を入れた書き込める領域。
    if let Err(e) = unsafe { GetWindowPlacement(hwnd, &mut placement) } {
        tracing::warn!("failed to get the window placement: {e}");
        return None;
    }
    Some(from_placement(&placement))
}

fn from_placement(placement: &WINDOWPLACEMENT) -> WindowState {
    let rect = placement.rcNormalPosition;
    WindowState {
        left: rect.left,
        top: rect.top,
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
        maximized: placement.showCmd == SW_SHOWMAXIMIZED.0 as u32,
    }
}

/// ウィンドウを見せずに、最大化していないときの位置と大きさを `window` にする。最大化は見せるときに
/// `ShowWindow` で行う。どのモニタにも重ならない位置（モニタを外したときなど）は、`SetWindowPlacement`
/// が作業領域の中へ寄せる。
pub fn restore_hidden(hwnd: HWND, window: &WindowState) {
    let placement = to_hidden_placement(window);
    // SAFETY: hwnd は有効なウィンドウで、placement は length を入れて初期化してある。
    if let Err(e) = unsafe { SetWindowPlacement(hwnd, &placement) } {
        tracing::warn!("failed to restore the window placement: {e}");
    }
}

fn to_hidden_placement(window: &WindowState) -> WINDOWPLACEMENT {
    WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        flags: WINDOWPLACEMENT_FLAGS(0),
        showCmd: SW_HIDE.0 as u32,
        ptMinPosition: UNSPECIFIED_POSITION,
        ptMaxPosition: UNSPECIFIED_POSITION,
        rcNormalPosition: RECT {
            left: window.left,
            top: window.top,
            right: window.left + window.width,
            bottom: window.top + window.height,
        },
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWMINIMIZED, SW_SHOWNORMAL};

    use super::*;

    /// テストごとに別の、空のフォルダ（一時フォルダの下）。
    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fantermo-state-test-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        dir
    }

    const WINDOW: WindowState = WindowState {
        left: 10,
        top: 20,
        width: 300,
        height: 200,
        maximized: true,
    };

    #[test]
    fn saved_state_loads_back() {
        let dir = scratch_dir("roundtrip");
        let path = dir.join("fantermo").join(STATE_FILE_NAME);
        let state = State {
            window: Some(WINDOW),
        };
        save(&path, &state);
        assert_eq!(load(Some(&path)), state);
        assert!(!with_suffix(&path, TEMPORARY_SUFFIX).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_file_is_the_default() {
        let dir = scratch_dir("missing");
        let path = dir.join(STATE_FILE_NAME);
        assert_eq!(load(Some(&path)), State::default());
        assert!(!dir.exists());
        assert_eq!(load(None), State::default());
    }

    #[test]
    fn broken_file_is_copied_aside_and_kept() {
        let dir = scratch_dir("broken");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(STATE_FILE_NAME);
        std::fs::write(&path, "[window\n").unwrap();
        assert_eq!(load(Some(&path)), State::default());
        let broken = dir.join("state.toml.broken");
        assert_eq!(std::fs::read_to_string(&broken).unwrap(), "[window\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[window\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_values_are_not_copied_aside() {
        let dir = scratch_dir("invalid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(STATE_FILE_NAME);
        std::fs::write(
            &path,
            "[window]\nleft = 0\ntop = 0\nwidth = -1\nheight = 1\nmaximized = false\n",
        )
        .unwrap();
        assert_eq!(load(Some(&path)), State::default());
        assert!(!dir.join("state.toml.broken").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn placement_converts_both_ways() {
        let placement = to_hidden_placement(&WINDOW);
        assert_eq!(placement.showCmd, SW_HIDE.0 as u32);
        assert_eq!(
            placement.rcNormalPosition,
            RECT {
                left: 10,
                top: 20,
                right: 310,
                bottom: 220,
            }
        );
        let maximized = WINDOWPLACEMENT {
            showCmd: SW_SHOWMAXIMIZED.0 as u32,
            ..placement
        };
        assert_eq!(from_placement(&maximized), WINDOW);
        for show in [SW_SHOWNORMAL, SW_SHOWMINIMIZED] {
            let placement = WINDOWPLACEMENT {
                showCmd: show.0 as u32,
                ..placement
            };
            assert_eq!(
                from_placement(&placement),
                WindowState {
                    maximized: false,
                    ..WINDOW
                }
            );
        }
    }
}
