//! IME の変換中の窓と候補の窓を、端末のカーソルの位置に出させる。
//!
//! 変換中の文字列は IME 自身に描かせる。確定した文字列は WM_CHAR で届く。

use windows::Win32::{
    Foundation::{HWND, POINT, RECT},
    UI::Input::Ime::{
        CANDIDATEFORM, CFS_EXCLUDE, CFS_POINT, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext,
        ImmSetCandidateWindow, ImmSetCompositionFontW, ImmSetCompositionWindow,
    },
};

use crate::font::Font;

/// カーソルのセルの、窓のクライアント領域での位置と大きさ（ピクセル）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// 変換中の窓をカーソルの位置に、端末と同じフォントで出させる。候補の窓はカーソルのセルを避けさせる。
pub fn place_windows(hwnd: HWND, cursor: CursorRect, font: &Font) {
    // SAFETY: hwnd は有効な窓。得た入力コンテキストはこの関数の最後で返す。
    let context = unsafe { ImmGetContext(hwnd) };
    if context.is_invalid() {
        return;
    }

    let logfont = font.logfont();
    let composition = COMPOSITIONFORM {
        dwStyle: CFS_POINT,
        ptCurrentPos: POINT {
            x: cursor.x,
            y: cursor.y,
        },
        rcArea: RECT::default(),
    };
    let candidate = CANDIDATEFORM {
        dwIndex: 0,
        dwStyle: CFS_EXCLUDE,
        ptCurrentPos: POINT {
            x: cursor.x,
            y: cursor.y + cursor.height,
        },
        rcArea: RECT {
            left: cursor.x,
            top: cursor.y,
            right: cursor.x + cursor.width,
            bottom: cursor.y + cursor.height,
        },
    };
    // SAFETY: context は有効な入力コンテキストで、構造体は初期化済み。
    unsafe {
        if !ImmSetCompositionFontW(context, &logfont).as_bool() {
            tracing::debug!("the IME did not accept the composition font");
        }
        if !ImmSetCompositionWindow(context, &composition).as_bool() {
            tracing::debug!("the IME did not accept the composition position");
        }
        if !ImmSetCandidateWindow(context, &candidate).as_bool() {
            tracing::debug!("the IME did not accept the candidate position");
        }
        _ = ImmReleaseContext(hwnd, context);
    }
}
