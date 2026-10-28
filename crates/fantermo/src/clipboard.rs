//! Windows のクリップボードの文字列。

use windows::Win32::{
    Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND},
    System::{
        DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        Ole::CF_UNICODETEXT,
    },
};

/// クリップボードを開いている間だけ生きる。破棄すると閉じる。
struct OpenedClipboard;

impl OpenedClipboard {
    fn open(owner: HWND) -> windows::core::Result<Self> {
        // SAFETY: owner は呼び出し元の有効な窓。閉じるのは Drop で一度だけ。
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

/// クリップボードを `text` で置き換える。
pub fn set_text(owner: HWND, text: &str) -> windows::core::Result<()> {
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

fn free(memory: HGLOBAL) {
    // SAFETY: memory は GlobalAlloc で確保し、まだ誰にも渡していない。
    if let Err(e) = unsafe { GlobalFree(Some(memory)) } {
        tracing::warn!("failed to free clipboard memory: {e}");
    }
}
