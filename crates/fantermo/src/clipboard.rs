//! Windows のクリップボードの文字列。

use windows::Win32::{
    Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND},
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
        },
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        Ole::CF_UNICODETEXT,
    },
};

/// クリップボードを開いている間だけ生きる。破棄すると閉じる。
struct OpenedClipboard;

impl OpenedClipboard {
    fn open(owner: HWND) -> windows::core::Result<Self> {
        // SAFETY: owner は呼び出し元の有効なウィンドウ。閉じるのは Drop で一度だけ。
        unsafe { OpenClipboard(Some(owner))? };
        Ok(Self)
    }
}

impl Drop for OpenedClipboard {
    fn drop(&mut self) {
        // SAFETY: open で開いたクリップボードを閉じる。
        if let Err(e) = unsafe { CloseClipboard() } {
            tracing::warn!("failed to close the clipboard: {e}");
        }
    }
}

/// e2e テストが OS のクリップボードの代わりに使うファイル。テストは利用者のクリップボードを変えない。
#[cfg(feature = "e2e")]
mod stand_in {
    use std::{io::ErrorKind, path::PathBuf};

    use windows::{Win32::Foundation::E_FAIL, core::Error};

    /// 代わりに使うファイルを指定する環境変数。
    const FILE_ENV: &str = "FANTERMO_E2E_CLIPBOARD_FILE";

    pub fn file() -> Option<PathBuf> {
        std::env::var_os(FILE_ENV).map(PathBuf::from)
    }

    fn error(e: &std::io::Error) -> Error {
        Error::new(E_FAIL, e.to_string())
    }

    pub fn read(file: &PathBuf) -> windows::core::Result<Option<String>> {
        match std::fs::read_to_string(file) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(error(&e)),
        }
    }

    pub fn write(file: &PathBuf, text: &str) -> windows::core::Result<()> {
        std::fs::write(file, text).map_err(|e| error(&e))
    }

    pub fn clear(file: &PathBuf) -> windows::core::Result<()> {
        match std::fs::remove_file(file) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(error(&e)),
            _ => Ok(()),
        }
    }
}

/// クリップボードを `text` で置き換える。
pub fn set_text(owner: HWND, text: &str) -> windows::core::Result<()> {
    #[cfg(feature = "e2e")]
    if let Some(file) = stand_in::file() {
        return stand_in::write(&file, text);
    }
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = units.len() * size_of::<u16>();

    // SAFETY: 動かせるメモリを確保するだけ。所有権は SetClipboardData が成功すれば Windows に移る。
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes)? };
    // SAFETY: memory は直前に確保した bytes バイトの領域。
    let locked = unsafe { GlobalLock(memory) };
    if locked.is_null() {
        free(memory);
        return Err(windows::core::Error::from_thread());
    }
    // SAFETY: locked は bytes バイト書き込め、units と重ならない。
    unsafe {
        std::ptr::copy_nonoverlapping(units.as_ptr(), locked.cast::<u16>(), units.len());
        _ = GlobalUnlock(memory);
    }

    let opened = OpenedClipboard::open(owner);
    let result = opened.and_then(|_clipboard| {
        // SAFETY: クリップボードは開いていて、memory は終端付きの UTF-16 を持つ。
        unsafe {
            EmptyClipboard()?;
            SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(memory.0)))?;
        }
        Ok(())
    });
    if result.is_err() {
        // 渡せなかったので、自分で解放する。
        free(memory);
    }
    result
}

/// クリップボードを空にする。
pub fn clear(owner: HWND) -> windows::core::Result<()> {
    #[cfg(feature = "e2e")]
    if let Some(file) = stand_in::file() {
        return stand_in::clear(&file);
    }
    let _clipboard = OpenedClipboard::open(owner)?;
    // SAFETY: クリップボードは開いている。
    unsafe { EmptyClipboard() }
}

/// クリップボードの文字列。文字列がなければ `None`。
pub fn text(owner: HWND) -> windows::core::Result<Option<String>> {
    #[cfg(feature = "e2e")]
    if let Some(file) = stand_in::file() {
        return stand_in::read(&file);
    }
    let _clipboard = OpenedClipboard::open(owner)?;
    // SAFETY: クリップボードは開いている。返るハンドルはクリップボードが持ち、閉じるまで有効。
    let Ok(handle) = (unsafe { GetClipboardData(u32::from(CF_UNICODETEXT.0)) }) else {
        return Ok(None);
    };
    let memory = HGLOBAL(handle.0);
    // SAFETY: memory はクリップボードの文字列の領域。
    let locked = unsafe { GlobalLock(memory) };
    if locked.is_null() {
        return Ok(None);
    }
    // SAFETY: Lock できたので、GlobalSize バイトを読める。読み終えたら Unlock する。
    let text = unsafe {
        let capacity = GlobalSize(memory) / size_of::<u16>();
        let units = std::slice::from_raw_parts(locked.cast::<u16>(), capacity);
        let length = units.iter().position(|&unit| unit == 0).unwrap_or(capacity);
        let text = String::from_utf16_lossy(&units[..length]);
        _ = GlobalUnlock(memory);
        text
    };
    Ok(Some(text))
}

fn free(memory: HGLOBAL) {
    // SAFETY: memory は GlobalAlloc で確保し、まだ誰にも渡していない。
    if let Err(e) = unsafe { GlobalFree(Some(memory)) } {
        tracing::warn!("failed to free clipboard memory: {e}");
    }
}
