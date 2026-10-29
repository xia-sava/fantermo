//! 複数行のテキストボックスとボタンを並べたダイアログ。
//!
//! 開いている間は呼び出し元の窓を無効にし、メッセージはこのダイアログのループで回す。IDOK のボタンを
//! 押せばテキストボックスの文字列を返し、IDCANCEL のボタンや Esc なら何も返さない。

use std::{
    cell::{Cell, RefCell},
    sync::OnceLock,
};

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{COLOR_BTNFACE, CreateFontIndirectW, DeleteObject, HBRUSH, HFONT, HGDIOBJ},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{EnableWindow, SetFocus},
            WindowsAndMessaging::{
                BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
                DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE,
                ES_WANTRETURN, GWLP_USERDATA, GetClientRect, GetMessageW, GetWindowLongPtrW,
                GetWindowRect, GetWindowTextLengthW, GetWindowTextW, HMENU, IDC_ARROW, IDCANCEL,
                IDOK, IsDialogMessageW, LoadCursorW, MSG, MoveWindow, PostQuitMessage,
                RegisterClassW, SW_SHOW, SWP_NOZORDER, SendMessageW, SetForegroundWindow,
                SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage,
                WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_SETFONT,
                WM_SIZE, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_EX_CONTROLPARENT,
                WS_EX_DLGMODALFRAME, WS_HSCROLL, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_THICKFRAME,
                WS_VISIBLE, WS_VSCROLL,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::font::Font;

const CLASS: PCWSTR = w!("fantermo-text-dialog");

/// テキストボックスの制御 ID。ボタンは IDOK と IDCANCEL を使う。
const EDIT_ID: i32 = 100;

/// `EM_SETLIMITTEXT` と、その上限を外す値。
const EM_SETLIMITTEXT: u32 = 0x00C5;
const NO_TEXT_LIMIT: usize = 0;

/// 96 DPI のときの寸法（ピクセル）。窓の DPI に合わせて拡大する。
const BASE_DPI: f32 = 96.0;
const MARGIN: f32 = 8.0;
const BUTTON_WIDTH: f32 = 112.0;
const BUTTON_HEIGHT: f32 = 30.0;
/// 呼び出し元の窓に対する、ダイアログの大きさの割合。
const SIZE_RATIO: f32 = 0.7;
/// 端末の文字に対する、テキストボックスの文字の大きさの割合。
const FONT_RATIO: f32 = 0.75;

/// ダイアログの下に右から並べるボタン。
pub struct Button {
    /// 押したときの制御 ID。IDOK か IDCANCEL。
    pub id: i32,
    pub label: &'static str,
    /// Enter で押されるボタンとして描く。
    pub default: bool,
}

/// 開くダイアログの中身。
pub struct Dialog<'a> {
    pub title: &'a str,
    /// テキストボックスに入れる文字列。改行は `\n` で渡す。
    pub text: &'a str,
    /// 左から順に並べるボタン。
    pub buttons: &'a [Button],
}

/// `dialog` を開き、閉じるまで待つ。IDOK のボタンで閉じたら、テキストボックスの文字列を返す。
/// 改行は `\n` で返す。`font` は端末のフォントで、テキストボックスはその書体を少し小さくして使う。
pub fn run(owner: HWND, dialog: &Dialog, font: &Font) -> Option<String> {
    if let Err(e) = register_class() {
        tracing::warn!("failed to register the text dialog: {e}");
        return None;
    }
    let state = DialogState::default();
    let hwnd = match create(owner, dialog, &state, font) {
        Ok(hwnd) => hwnd,
        Err(e) => {
            tracing::warn!("failed to open the text dialog {:?}: {e}", dialog.title);
            return None;
        }
    };
    // SAFETY: テキストボックスはこのダイアログの子で、文字列はこの呼び出しの間は生きている。
    unsafe { _ = SetWindowTextW(state.edit.get(), &HSTRING::from(to_crlf(dialog.text))) };

    run_modal(owner, hwnd, &state);

    let created_font = state.font.get();
    if !created_font.is_invalid() {
        // SAFETY: フォントはダイアログのために作ったもので、ダイアログはもう閉じている。
        unsafe { _ = DeleteObject(HGDIOBJ(created_font.0)) };
    }
    state.result.take().map(|text| from_crlf(&text))
}

/// ダイアログの状態。ウィンドウプロシージャからも触るので、共有の参照だけで読み書きする。
#[derive(Default)]
struct DialogState {
    edit: Cell<HWND>,
    /// ボタン。左から順に並べる。
    buttons: RefCell<Vec<HWND>>,
    font: Cell<HFONT>,
    /// 寸法の拡大率（窓の DPI / 96）。
    scale: Cell<f32>,
    result: RefCell<Option<String>>,
    closed: Cell<bool>,
}

fn register_class() -> windows::core::Result<()> {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    if REGISTERED.get().is_some() {
        return Ok(());
    }
    // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
    let instance = unsafe { GetModuleHandleW(None)? };
    let class = WNDCLASSW {
        lpfnWndProc: Some(dialog_proc),
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

/// 呼び出し元の窓の真ん中に、その大きさの 7 割でダイアログを作る。
fn create(
    owner: HWND,
    dialog: &Dialog,
    state: &DialogState,
    font: &Font,
) -> windows::core::Result<HWND> {
    // SAFETY: owner は呼び出し元の有効な窓。
    let dpi = unsafe { GetDpiForWindow(owner) };
    state.scale.set(dpi as f32 / BASE_DPI);

    let mut owner_rect = RECT::default();
    // SAFETY: owner は有効な窓で、owner_rect は書き込める。
    unsafe { GetWindowRect(owner, &mut owner_rect)? };
    let width = ((owner_rect.right - owner_rect.left) as f32 * SIZE_RATIO) as i32;
    let height = ((owner_rect.bottom - owner_rect.top) as f32 * SIZE_RATIO) as i32;
    let left = owner_rect.left + (owner_rect.right - owner_rect.left - width) / 2;
    let top = owner_rect.top + (owner_rect.bottom - owner_rect.top - height) / 2;

    // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
    let instance = unsafe { GetModuleHandleW(None)? };
    // SAFETY: クラスは登録済みで、文字列はこの呼び出しの間は生きている。
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            CLASS,
            &HSTRING::from(dialog.title),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            width,
            height,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        )?
    };

    let child = |class: PCWSTR, text: &str, style: WINDOW_STYLE, ex_style, id: i32| {
        // SAFETY: hwnd は有効な窓で、文字列はこの呼び出しの間は生きている。
        unsafe {
            CreateWindowExW(
                ex_style,
                class,
                &HSTRING::from(text),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(id as isize as *mut _)),
                Some(instance.into()),
                None,
            )
        }
    };
    let edit_style =
        WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_WANTRETURN) as u32);
    state.edit.set(child(
        w!("EDIT"),
        "",
        WS_VSCROLL | WS_HSCROLL | edit_style,
        WS_EX_CLIENTEDGE,
        EDIT_ID,
    )?);
    for button in dialog.buttons {
        let style = if button.default {
            BS_DEFPUSHBUTTON
        } else {
            BS_PUSHBUTTON
        };
        let control = child(
            w!("BUTTON"),
            button.label,
            WINDOW_STYLE(style as u32),
            WINDOW_EX_STYLE(0),
            button.id,
        )?;
        state.buttons.borrow_mut().push(control);
    }

    // テキストボックスは端末と同じ書体にして、少し小さくする。
    let mut logfont = font.logfont();
    logfont.lfHeight = (logfont.lfHeight as f32 * FONT_RATIO) as i32;
    // SAFETY: logfont は初期化済み。作ったフォントはダイアログが閉じた後に消す。
    state.font.set(unsafe { CreateFontIndirectW(&logfont) });
    let controls = std::iter::once(state.edit.get()).chain(state.buttons.borrow().clone());
    for control in controls {
        // SAFETY: control は hwnd の子で、フォントは hwnd が閉じるまで消さない。
        unsafe {
            SendMessageW(
                control,
                WM_SETFONT,
                Some(WPARAM(state.font.get().0 as usize)),
                Some(LPARAM(1)),
            )
        };
    }
    // SAFETY: テキストボックスは有効な窓。
    unsafe {
        SendMessageW(
            state.edit.get(),
            EM_SETLIMITTEXT,
            Some(WPARAM(NO_TEXT_LIMIT)),
            None,
        )
    };

    // 状態を結び付けてから大きさを決め、WM_SIZE で子を並べさせる。
    // SAFETY: hwnd は作ったばかりの窓。ポインタは run_modal が戻るまで有効で、WM_DESTROY で外す。
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, std::ptr::from_ref(state) as isize);
        SetWindowPos(hwnd, None, left, top, width, height, SWP_NOZORDER)?;
        layout(hwnd, state);
        _ = ShowWindow(hwnd, SW_SHOW);
        _ = SetFocus(Some(state.edit.get()));
    }
    Ok(hwnd)
}

/// 呼び出し元の窓を無効にし、ダイアログが閉じるまでメッセージを回す。
fn run_modal(owner: HWND, dialog: HWND, state: &DialogState) {
    // SAFETY: owner は有効な窓。閉じた後に必ず有効に戻す。
    unsafe { _ = EnableWindow(owner, false) };
    let mut msg = MSG::default();
    while !state.closed.get() {
        // SAFETY: msg は書き込める。
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if !got.as_bool() {
            // アプリの終了を受け取ったので、外側のループにも伝えてから抜ける。
            // SAFETY: 受け取った終了コードを積み直し、まだ開いているダイアログを閉じる。
            unsafe {
                PostQuitMessage(msg.wParam.0 as i32);
                _ = DestroyWindow(dialog);
            }
            break;
        }
        // Tab での移動、Enter で既定のボタン、Esc でキャンセルを、ダイアログの決まりどおりに扱う。
        // SAFETY: dialog はこのスレッドの窓で、msg は GetMessageW が埋めたもの。
        if unsafe { IsDialogMessageW(dialog, &msg) }.as_bool() {
            continue;
        }
        // SAFETY: msg は GetMessageW が埋めたもの。
        unsafe {
            _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // SAFETY: owner は有効な窓。
    unsafe {
        _ = EnableWindow(owner, true);
        _ = SetForegroundWindow(owner);
    }
}

/// テキストボックスを広く取り、ボタンを右下に並べる。
fn layout(dialog: HWND, state: &DialogState) {
    let mut client = RECT::default();
    // SAFETY: dialog は有効な窓で、client は書き込める。
    if unsafe { GetClientRect(dialog, &mut client) }.is_err() {
        return;
    }
    let scale = state.scale.get();
    let px = |value: f32| (value * scale).round() as i32;
    let (margin, button_w, button_h) = (px(MARGIN), px(BUTTON_WIDTH), px(BUTTON_HEIGHT));
    let button_top = client.bottom - margin - button_h;
    // SAFETY: テキストボックスは dialog の子の有効な窓。
    unsafe {
        _ = MoveWindow(
            state.edit.get(),
            margin,
            margin,
            client.right - 2 * margin,
            button_top - 2 * margin,
            true,
        );
    }
    let mut button_left = client.right;
    for &button in state.buttons.borrow().iter().rev() {
        button_left -= margin + button_w;
        // SAFETY: button は dialog の子の有効な窓。
        unsafe { _ = MoveWindow(button, button_left, button_top, button_w, button_h, true) };
    }
}

fn read_text(edit: HWND) -> String {
    // SAFETY: edit は有効なテキストボックス。
    let length = unsafe { GetWindowTextLengthW(edit) }.max(0) as usize;
    let mut buffer = vec![0u16; length + 1];
    // SAFETY: buffer は length + 1 個の u16 を書き込める。
    let copied = unsafe { GetWindowTextW(edit, &mut buffer) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..copied])
}

/// ダイアログに結び付けた状態。WM_DESTROY で外すまで有効。
fn state_of<'a>(dialog: HWND) -> Option<&'a DialogState> {
    // SAFETY: GWLP_USERDATA には create が入れた DialogState のポインタか 0 が入っている。
    let pointer = unsafe { GetWindowLongPtrW(dialog, GWLP_USERDATA) } as *const DialogState;
    // SAFETY: 0 でなければ、run の中の DialogState を指し、ダイアログが閉じるまで有効。
    unsafe { pointer.as_ref() }
}

extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(state) = state_of(hwnd) else {
        // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    match msg {
        WM_SIZE => {
            layout(hwnd, state);
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            if id == IDOK.0 {
                state.result.replace(Some(read_text(state.edit.get())));
            }
            if id == IDOK.0 || id == IDCANCEL.0 {
                // SAFETY: hwnd はこのダイアログ。
                unsafe { _ = DestroyWindow(hwnd) };
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            // SAFETY: hwnd はこのダイアログ。
            unsafe { _ = DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            state.closed.set(true);
            // SAFETY: hwnd はこのダイアログ。以後、状態のポインタは使わせない。
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            LRESULT(0)
        }
        _ => {
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

/// テキストボックスに見せるため、改行を `\r\n` にする。
fn to_crlf(text: &str) -> String {
    from_crlf(text).replace('\n', "\r\n")
}

/// テキストボックスやクリップボードの `\r\n` を `\n` にする。
pub fn from_crlf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_round_trip() {
        assert_eq!(to_crlf("a\nb"), "a\r\nb");
        assert_eq!(to_crlf("a\r\nb"), "a\r\nb");
        assert_eq!(from_crlf("a\r\nb\r\n"), "a\nb\n");
    }
}
