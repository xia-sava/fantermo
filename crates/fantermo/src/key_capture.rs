//! 押したキーの組み合わせを読み取る小さなダイアログ。設定ダイアログのキーの割り当てに使う。
//!
//! 開いている間のキーのメッセージは、ウィンドウに届ける前にループで読み取る（Tab や Enter や Alt の
//! 組み合わせも、ダイアログの操作やメニューにせずに組み合わせとして読む）。修飾キーだけを押している間は
//! 待ち、ほかのキーを押したらその組み合わせで閉じる。修飾キーなしの Esc はやめる。名前を持たないキー
//! （設定ファイルに書けないキー）は読まずに待つ。

use std::{cell::Cell, sync::OnceLock};

use fantermo_core::{
    keymap::KeyCombo,
    vt::{Key, Mods},
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{COLOR_BTNFACE, HBRUSH, HFONT},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::AdjustWindowRectExForDpi,
            Input::KeyboardAndMouse::SetFocus,
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW,
                GetWindowRect, HMENU, IDC_ARROW, LoadCursorW, MSG, RegisterClassW, SW_SHOW,
                SendMessageW, SetWindowLongPtrW, ShowWindow, WINDOW_EX_STYLE, WM_CHAR, WM_CLOSE,
                WM_DEADCHAR, WM_DESTROY, WM_KEYDOWN, WM_KEYUP, WM_SETFONT, WM_SYSCHAR,
                WM_SYSDEADCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW, WS_CAPTION, WS_CHILD,
                WS_EX_DLGMODALFRAME, WS_POPUP, WS_SYSMENU, WS_VISIBLE,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{
    keyboard::{self, KeyPosition},
    modal,
};

const CLASS: PCWSTR = w!("fantermo-key-capture");
const TITLE: &str = "キーの割り当て";

/// 96 DPI のときの寸法（ピクセル）。
const BASE_DPI: u32 = 96;
const CLIENT_WIDTH: i32 = 360;
const CLIENT_HEIGHT: i32 = 72;
const MARGIN: i32 = 12;

/// `owner` の真ん中に、`prompt` を見せるダイアログを開き、押したキーの組み合わせを返す。やめたら
/// `None`。`font` はダイアログの文字のフォント、`dpi` は寸法を決める DPI。
pub fn capture(owner: HWND, prompt: &str, font: HFONT, dpi: u32) -> Option<KeyCombo> {
    if let Err(e) = register_class() {
        tracing::warn!("failed to register the key capture dialog: {e}");
        return None;
    }
    let hwnd = match create(owner, prompt, font, dpi) {
        Ok(hwnd) => hwnd,
        Err(e) => {
            tracing::warn!("failed to open the key capture dialog: {e}");
            return None;
        }
    };
    let closed = Cell::new(false);
    // SAFETY: hwnd は作ったばかりのウィンドウ。ポインタは modal::run が戻るまで有効で、WM_DESTROY で外す。
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, std::ptr::from_ref(&closed) as isize) };
    // SAFETY: hwnd は有効なウィンドウ。
    unsafe {
        _ = ShowWindow(hwnd, SW_SHOW);
        _ = SetFocus(Some(hwnd));
    }
    let captured = Cell::new(None);
    modal::run(owner, hwnd, &closed, |msg| {
        let Some(read) = read_key(msg) else {
            return false;
        };
        if let Read::Done(combo) = read {
            captured.set(combo);
            // SAFETY: hwnd はこのダイアログ。閉じると WM_DESTROY で closed が立つ。
            unsafe { _ = DestroyWindow(hwnd) };
        }
        true
    });
    captured.get()
}

/// キーのメッセージを読んだ結果。
enum Read {
    /// 読み取りを終える。組み合わせか、やめたなら `None`。
    Done(Option<KeyCombo>),
    /// ウィンドウには渡さず、待ち続ける。
    Wait,
}

/// キーのメッセージ `msg` を読む。キーのメッセージでなければ `None`（ほかの処理に渡す）。
fn read_key(msg: &MSG) -> Option<Read> {
    match msg.message {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let position = KeyPosition::from_lparam(msg.lParam.0);
            if position.repeated {
                return Some(Read::Wait);
            }
            let key = keyboard::key_from_position(position);
            Some(combo_read(KeyCombo::new(keyboard::current_mods(), key)))
        }
        WM_KEYUP | WM_SYSKEYUP | WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR => {
            Some(Read::Wait)
        }
        _ => None,
    }
}

/// 押した組み合わせ `combo` をどう扱うか。修飾キーなしの Esc はやめ、名前を持たないキー（修飾キー
/// そのものなど）は待つ。
fn combo_read(combo: KeyCombo) -> Read {
    if combo.key == Key::Escape && combo.mods == Mods::empty() {
        return Read::Done(None);
    }
    if combo.is_nameable() {
        Read::Done(Some(combo))
    } else {
        Read::Wait
    }
}

fn register_class() -> windows::core::Result<()> {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    if REGISTERED.get().is_some() {
        return Ok(());
    }
    // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
    let instance = unsafe { GetModuleHandleW(None)? };
    let class = WNDCLASSW {
        lpfnWndProc: Some(capture_proc),
        hInstance: instance.into(),
        // SAFETY: システムの矢印カーソルを読むだけ。
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        // システムの色の番号に 1 を足した値は、その色のブラシとして扱われる。
        hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as isize as *mut _),
        lpszClassName: CLASS,
        ..Default::default()
    };
    // SAFETY: class は初期化済みで、名前は静的な文字列。
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    _ = REGISTERED.set(());
    Ok(())
}

fn scale(value: i32, dpi: u32) -> i32 {
    (value as f32 * dpi as f32 / BASE_DPI as f32).round() as i32
}

/// ダイアログを作り、`owner` の真ん中に置く。
fn create(owner: HWND, prompt: &str, font: HFONT, dpi: u32) -> windows::core::Result<HWND> {
    let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: scale(CLIENT_WIDTH, dpi),
        bottom: scale(CLIENT_HEIGHT, dpi),
    };
    // SAFETY: rect は書き込める。
    unsafe { AdjustWindowRectExForDpi(&mut rect, style, false, WS_EX_DLGMODALFRAME, dpi)? };
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    let mut owner_rect = RECT::default();
    // SAFETY: owner は有効なウィンドウで、owner_rect は書き込める。
    unsafe { GetWindowRect(owner, &mut owner_rect)? };
    let left = owner_rect.left + (owner_rect.right - owner_rect.left - width) / 2;
    let top = owner_rect.top + (owner_rect.bottom - owner_rect.top - height) / 2;
    // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
    let instance = unsafe { GetModuleHandleW(None)? };
    // SAFETY: クラスは登録済みで、文字列はこの呼び出しの間は生きている。
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            CLASS,
            &HSTRING::from(TITLE),
            style,
            left,
            top,
            width,
            height,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        )?
    };
    let text = format!("{prompt}\nEsc でやめる。");
    // SAFETY: hwnd は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
    let label = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            &HSTRING::from(text),
            WS_CHILD | WS_VISIBLE,
            scale(MARGIN, dpi),
            scale(MARGIN, dpi),
            scale(CLIENT_WIDTH - 2 * MARGIN, dpi),
            scale(CLIENT_HEIGHT - 2 * MARGIN, dpi),
            Some(hwnd),
            Some(HMENU(std::ptr::null_mut())),
            Some(instance.into()),
            None,
        )?
    };
    // SAFETY: label は有効なウィンドウで、フォントは呼び出し元のダイアログが閉じるまで消さない。
    unsafe {
        SendMessageW(
            label,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(0)),
        )
    };
    Ok(hwnd)
}

extern "system" fn capture_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => {
            // SAFETY: hwnd はこのダイアログ。
            unsafe { _ = DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: GWLP_USERDATA には capture が入れた Cell<bool> のポインタか 0 が入っている。
            let closed = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Cell<bool>;
            // SAFETY: 0 でなければ capture の中の Cell を指し、modal::run が戻るまで有効。
            if let Some(closed) = unsafe { closed.as_ref() } {
                closed.set(true);
            }
            // SAFETY: hwnd はこのダイアログ。以後、ポインタは使わせない。
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            LRESULT(0)
        }
        // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Option<Option<KeyCombo>> {
        match combo_read(KeyCombo::parse(text).unwrap()) {
            Read::Done(combo) => Some(combo),
            Read::Wait => None,
        }
    }

    #[test]
    fn named_combos_end_the_capture_and_escape_alone_cancels() {
        assert_eq!(
            read("ctrl+shift+t"),
            Some(Some(KeyCombo::parse("ctrl+shift+t").unwrap()))
        );
        assert_eq!(read("escape"), Some(None));
        assert_eq!(
            read("shift+escape"),
            Some(Some(KeyCombo::parse("shift+escape").unwrap()))
        );
        // 修飾キーそのものは名前を持たないので待つ。
        assert!(matches!(
            combo_read(KeyCombo::new(Mods::CTRL, Key::ControlLeft)),
            Read::Wait
        ));
    }
}
