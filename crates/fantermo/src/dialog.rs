//! 自分で作るダイアログ（設定ダイアログ、入力欄のダイアログ、キーを読み取るダイアログ）に共通の処理と、
//! メッセージボックス。
//!
//! ダイアログの状態は、作ったあとに [`attach_state`] でウィンドウに結び付け、ウィンドウプロシージャは
//! [`state_of`] で読む。`WM_DESTROY` で [`on_destroy`] が結び付けを外し、閉じた印を立てる。印が立つと
//! `modal::run` のループが抜け、呼び出し元が状態を捨てる。状態が持つフォント（[`OwnedFont`]）はそのときに
//! 消えるので、コントロールより先には消えない。作る途中で失敗したダイアログは、[`Unfinished`] が壊す。
//!
//! 寸法は 96 DPI のときのピクセルで決め、ダイアログの DPI に合わせて拡大する（[`scale`]）。

use std::{cell::Cell, sync::OnceLock};

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            COLOR_BTNFACE, CreateFontIndirectW, DeleteObject, GetDC, GetMonitorInfoW, HBRUSH, HDC,
            HFONT, HGDIOBJ, LOGFONTW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
            ReleaseDC, SelectObject,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{
                EM_SETSEL, INITCOMMONCONTROLSEX, INITCOMMONCONTROLSEX_ICC, InitCommonControlsEx,
            },
            HiDpi::AdjustWindowRectExForDpi,
            Input::KeyboardAndMouse::SetFocus,
            WindowsAndMessaging::{
                DC_HASDEFID, DM_GETDEFID, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW,
                GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IDC_ARROW, LoadCursorW,
                MESSAGEBOX_RESULT, MESSAGEBOX_STYLE, MessageBoxW, RegisterClassW, SWP_NOACTIVATE,
                SWP_NOZORDER, SendMessageW, SetWindowLongPtrW, SetWindowPos, SetWindowTextW,
                WINDOW_EX_STYLE, WINDOW_STYLE, WM_NEXTDLGCTL, WM_SETFONT, WNDCLASSW, WNDPROC,
            },
        },
    },
    core::{HSTRING, PCWSTR},
};

use crate::{paint::BASE_DPI, ui_font};

/// `EM_SETSEL` で、すべてを選ぶときの範囲。
const SELECT_FROM_START: usize = 0;
const SELECT_TO_END: isize = -1;

/// ダイアログに結び付ける状態。
pub trait DialogState {
    /// 閉じた印。`WM_DESTROY` で立てる。
    fn closed(&self) -> &Cell<bool>;
}

impl DialogState for Cell<bool> {
    fn closed(&self) -> &Cell<bool> {
        self
    }
}

/// ダイアログのウィンドウのクラス `class` を、ウィンドウプロシージャ `proc` で登録する。`registered` はクラスごとの
/// 印で、二度目からは何もしない。`common_controls` があれば、先にそのコモンコントロールを使えるようにする。
pub fn register_class(
    registered: &OnceLock<()>,
    class: PCWSTR,
    proc: WNDPROC,
    common_controls: Option<INITCOMMONCONTROLSEX_ICC>,
) -> windows::core::Result<()> {
    if registered.get().is_some() {
        return Ok(());
    }
    if let Some(classes) = common_controls {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: classes,
        };
        // SAFETY: controls は大きさを入れて初期化してある。
        if !unsafe { InitCommonControlsEx(&controls) }.as_bool() {
            tracing::warn!("failed to register the common controls {classes:?}");
        }
    }
    // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
    let instance = unsafe { GetModuleHandleW(None)? };
    let window_class = WNDCLASSW {
        lpfnWndProc: proc,
        hInstance: instance.into(),
        // SAFETY: システムの矢印カーソルを読むだけ。
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        // システムの色の番号に 1 を足した値は、その色のブラシとして扱われる。
        hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as isize as *mut _),
        lpszClassName: class,
        ..Default::default()
    };
    // SAFETY: window_class は初期化済みで、名前は静的な文字列。
    if unsafe { RegisterClassW(&window_class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    _ = registered.set(());
    Ok(())
}

/// ダイアログ `dialog` に状態 `state` を結び付ける。
///
/// # Safety
///
/// `state` は、`dialog` の `WM_DESTROY` で [`on_destroy`] が結び付けを外すまで、動かさずに生かしておく。
pub unsafe fn attach_state<S: DialogState>(dialog: HWND, state: &S) {
    // SAFETY: dialog はこのスレッドのウィンドウ。ポインタを覚えるだけで、読むのは state_of。
    unsafe { SetWindowLongPtrW(dialog, GWLP_USERDATA, std::ptr::from_ref(state) as isize) };
}

/// ダイアログ `dialog` に結び付けた状態。結び付けていないか、外したあとなら `None`。
///
/// # Safety
///
/// `dialog` に [`attach_state`] で結び付けたのは `S` の状態か、何も結び付けていない。
pub unsafe fn state_of<'a, S>(dialog: HWND) -> Option<&'a S> {
    // SAFETY: GWLP_USERDATA には attach_state が入れた S のポインタか 0 が入っている。
    let pointer = unsafe { GetWindowLongPtrW(dialog, GWLP_USERDATA) } as *const S;
    // SAFETY: 0 でなければ、attach_state の約束で、WM_DESTROY までは生きている状態を指す。
    unsafe { pointer.as_ref() }
}

/// `WM_DESTROY` で呼ぶ。閉じた印を立て、状態の結び付けを外す（以後 [`state_of`] は `None`）。
pub fn on_destroy<S: DialogState>(dialog: HWND, state: &S) {
    state.closed().set(true);
    // SAFETY: dialog はこのダイアログ。以後、状態のポインタは使わせない。
    unsafe { SetWindowLongPtrW(dialog, GWLP_USERDATA, 0) };
}

/// どのダイアログでも同じに扱うメッセージを処理する。`default_id` は Enter で押されるボタンのコントロール ID。
/// 扱わないメッセージなら `None`。
pub fn handle_common_message(
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    default_id: Option<i32>,
) -> Option<LRESULT> {
    match msg {
        // Enter を自分で使わないコントロールは、既定のボタンをこの二つで問い合わせて押させる。
        DM_GETDEFID => Some(match default_id {
            Some(id) => LRESULT(((DC_HASDEFID as isize) << 16) | id as isize),
            None => LRESULT(0),
        }),
        WM_NEXTDLGCTL => {
            // lparam が 0 でなければ、wparam は移す先の子のウィンドウ。順に移すほうは使われないので
            // 扱わない。
            if lparam.0 & 0xffff != 0 {
                // SAFETY: wparam はこのダイアログの子のウィンドウのハンドルとして渡される。
                unsafe { _ = SetFocus(Some(HWND(wparam.0 as *mut _))) };
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// ダイアログ `dialog` を壊す。`WM_DESTROY` が届き、閉じた印が立つ。もう壊れていれば警告を残すだけ。
pub fn destroy(dialog: HWND) {
    // SAFETY: dialog はこのスレッドのウィンドウ。もう壊れていれば失敗するだけ。
    if let Err(e) = unsafe { DestroyWindow(dialog) } {
        tracing::warn!("failed to close a dialog: {e}");
    }
}

/// 作っている途中のダイアログ。[`Unfinished::finish`] で作り終えないまま捨てると（途中で失敗して戻ると）、
/// ダイアログを壊す。結び付けた状態があれば、`WM_DESTROY` で外れる。
pub struct Unfinished(Option<HWND>);

impl Unfinished {
    pub fn new(dialog: HWND) -> Self {
        Self(Some(dialog))
    }

    /// 作り終えたダイアログを返す。以後、捨てても壊さない。
    pub fn finish(mut self) -> HWND {
        self.0.take().unwrap_or_default()
    }
}

impl Drop for Unfinished {
    fn drop(&mut self) {
        if let Some(dialog) = self.0.take() {
            destroy(dialog);
        }
    }
}

/// 96 DPI のときの `value` ピクセルを、DPI が `dpi` のときのピクセルにする。
pub fn scale(value: i32, dpi: u32) -> i32 {
    (value as f32 * dpi as f32 / BASE_DPI as f32).round() as i32
}

/// クライアント領域が 96 DPI のときに `width` × `height` のダイアログの、DPI が `dpi` のときのウィンドウの
/// 大きさ（枠を含む）。
pub fn window_size(
    width: i32,
    height: i32,
    style: WINDOW_STYLE,
    ex_style: WINDOW_EX_STYLE,
    dpi: u32,
) -> windows::core::Result<(i32, i32)> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: scale(width, dpi),
        bottom: scale(height, dpi),
    };
    // SAFETY: rect は書き込める。
    unsafe { AdjustWindowRectExForDpi(&mut rect, style, false, ex_style, dpi)? };
    Ok((rect.right - rect.left, rect.bottom - rect.top))
}

/// ウィンドウ `window` の位置と大きさ（画面の座標）。
pub fn window_rect(window: HWND) -> windows::core::Result<RECT> {
    let mut rect = RECT::default();
    // SAFETY: window は有効なウィンドウで、rect は書き込める。
    unsafe { GetWindowRect(window, &mut rect)? };
    Ok(rect)
}

/// `owner` の真ん中に `width` × `height` を置くときの左上。
pub fn center_in(owner: &RECT, width: i32, height: i32) -> (i32, i32) {
    (
        owner.left + (owner.right - owner.left - width) / 2,
        owner.top + (owner.bottom - owner.top - height) / 2,
    )
}

/// ウィンドウ `window` があるモニターの作業領域。読めなければ `None`。
pub fn work_area(window: HWND) -> Option<RECT> {
    let mut monitor = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: window は有効なウィンドウで、monitor は cbSize を入れて初期化済み。
    let found = unsafe {
        GetMonitorInfoW(
            MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        )
    };
    found.as_bool().then_some(monitor.rcWork)
}

/// 位置 `start` から長さ `length` の範囲が `low` から `high` までに収まる位置。収まらなければ `low`（タイトルバーを画面に
/// 残す）。
pub fn keep_within(start: i32, length: i32, low: i32, high: i32) -> i32 {
    start.min(high - length).max(low)
}

/// `WM_DPICHANGED` の wparam から、新しい DPI を読む。
pub fn dpi_of(wparam: WPARAM) -> u32 {
    (wparam.0 & 0xffff) as u32
}

/// `WM_DPICHANGED` を受けたダイアログ `dialog` を、lparam が指す推奨の矩形へ移す。
///
/// # Safety
///
/// `lparam` は `WM_DPICHANGED` で受け取ったもの。
pub unsafe fn move_to_suggested_rect(dialog: HWND, lparam: LPARAM) {
    // SAFETY: WM_DPICHANGED の lparam は推奨された矩形を指す。
    let suggested = unsafe { &*(lparam.0 as *const RECT) };
    // SAFETY: dialog はこのスレッドの有効なウィンドウ。
    if let Err(e) = unsafe {
        SetWindowPos(
            dialog,
            None,
            suggested.left,
            suggested.top,
            suggested.right - suggested.left,
            suggested.bottom - suggested.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    } {
        tracing::warn!("failed to move a dialog for the DPI change: {e}");
    }
}

/// ダイアログのために作ったフォント。捨てるときに消す。ダイアログを壊してから捨てる。
#[derive(Default)]
pub struct OwnedFont(HFONT);

impl OwnedFont {
    /// `logfont` のフォントを作る。作れなければ無効なハンドル（コントロールはシステムのフォントで描く）を持つ。
    pub fn create(logfont: &LOGFONTW) -> Self {
        // SAFETY: logfont は初期化済み。作ったフォントは捨てるときに消す。
        Self(unsafe { CreateFontIndirectW(logfont) })
    }

    /// DPI が `dpi` のときの Windows のメッセージのフォント。設定を読めなければ警告を残して `None`。
    pub fn message(dpi: u32) -> Option<Self> {
        match ui_font::message_logfont(dpi) {
            Ok(logfont) => Some(Self::create(&logfont)),
            Err(e) => {
                tracing::warn!("failed to read the message font: {e}");
                None
            }
        }
    }

    pub fn handle(&self) -> HFONT {
        self.0
    }
}

impl Drop for OwnedFont {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: フォントはこの値が作ったもので、使っていたダイアログはもう壊れているか、ほかのフォントに
            // 付け替えてある。
            unsafe { _ = DeleteObject(HGDIOBJ(self.0.0)) };
        }
    }
}

/// `control` の文字のフォントを `font` にする。`redraw` なら、すぐに描き直させる。
pub fn set_font(control: HWND, font: HFONT, redraw: bool) {
    // SAFETY: control はダイアログの子で、フォントはダイアログが閉じるか付け替えるまで消さない。
    unsafe {
        SendMessageW(
            control,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(isize::from(redraw))),
        )
    };
}

/// `control` の DC にフォント `font` を選んで `measure` を呼ぶ。選んだフォントは戻し、DC は返す。DC を
/// 得られなければ `None`。
pub fn with_font_dc<T>(control: HWND, font: HFONT, measure: impl FnOnce(HDC) -> T) -> Option<T> {
    // SAFETY: control は有効なウィンドウ。選んだフォントは戻し、DC は読み終えたら返す。
    unsafe {
        let dc = GetDC(Some(control));
        if dc.is_invalid() {
            return None;
        }
        let previous = SelectObject(dc, HGDIOBJ(font.0));
        let measured = measure(dc);
        SelectObject(dc, previous);
        ReleaseDC(Some(control), dc);
        Some(measured)
    }
}

pub fn set_text(control: HWND, text: &str) {
    // SAFETY: control は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
    unsafe { _ = SetWindowTextW(control, &HSTRING::from(text)) };
}

pub fn get_text(control: HWND) -> String {
    // SAFETY: control は有効なウィンドウ。
    let length = unsafe { GetWindowTextLengthW(control) }.max(0) as usize;
    let mut buffer = vec![0u16; length + 1];
    // SAFETY: buffer は length + 1 個の u16 を書き込める。
    let copied = unsafe { GetWindowTextW(control, &mut buffer) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..copied])
}

/// 入力欄 `edit` の文字をすべて選ぶ。
pub fn select_all(edit: HWND) {
    // SAFETY: edit は有効な入力欄。
    unsafe {
        SendMessageW(
            edit,
            EM_SETSEL,
            Some(WPARAM(SELECT_FROM_START)),
            Some(LPARAM(SELECT_TO_END)),
        )
    };
}

/// タイトル `title` で文 `text` のメッセージボックスを開き、閉じるまで待って押したボタンを返す。`owner` は持ち主の
/// ウィンドウで、`None` なら持ち主なしで開く。自分でメッセージを回すので、借用を手放してから呼ぶ。開けなければ
/// 警告を残し、0 の値を返す。
pub fn message_box(
    owner: Option<HWND>,
    text: &str,
    title: &str,
    style: MESSAGEBOX_STYLE,
) -> MESSAGEBOX_RESULT {
    // SAFETY: owner は None か有効なウィンドウで、文字列はこの呼び出しの間は生きている。
    let result = unsafe { MessageBoxW(owner, &HSTRING::from(text), &HSTRING::from(title), style) };
    if result.0 == 0 {
        tracing::warn!(
            "failed to show a message box: {}",
            windows::core::Error::from_thread()
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialog_position_stays_within_the_work_area() {
        assert_eq!(keep_within(100, 50, 0, 1000), 100, "already inside");
        assert_eq!(keep_within(-30, 50, 0, 1000), 0, "past the start");
        assert_eq!(keep_within(980, 50, 0, 1000), 950, "past the end");
        assert_eq!(keep_within(100, 2000, 0, 1000), 0, "longer than the area");
    }

    #[test]
    fn a_dialog_is_centered_on_its_owner() {
        let owner = RECT {
            left: 100,
            top: 50,
            right: 500,
            bottom: 350,
        };
        assert_eq!(center_in(&owner, 200, 100), (200, 150));
        // 呼び出し元より大きければ、はみ出す分を左右と上下に分ける。
        assert_eq!(center_in(&owner, 600, 400), (0, 0));
    }

    #[test]
    fn sizes_scale_with_the_dpi() {
        assert_eq!(scale(30, BASE_DPI), 30);
        assert_eq!(scale(30, 2 * BASE_DPI), 60);
        assert_eq!(scale(12, 120), 15);
        assert_eq!(scale(7, 144), 11);
    }
}
