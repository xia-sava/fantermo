//! 自分で作ったダイアログを、呼び出し元のウィンドウを無効にして開いている間のメッセージのループ。
//!
//! 呼び出し元のウィンドウへのメッセージ（端末の出力、描き直し、タイマーなど）も、このループから届く。

use std::cell::Cell;

use windows::Win32::{
    Foundation::HWND,
    UI::{
        Input::KeyboardAndMouse::EnableWindow,
        WindowsAndMessaging::{
            DestroyWindow, DispatchMessageW, GetMessageW, IsDialogMessageW, MSG, PostQuitMessage,
            SetForegroundWindow, TranslateMessage,
        },
    },
};

/// 呼び出し元のウィンドウ `owner` を無効にし、ダイアログ `dialog` が閉じる（`closed` が立つ）まで
/// メッセージを回す。`intercept` が `true` を返したメッセージは、ほかに渡さない。
pub fn run(
    owner: HWND,
    dialog: HWND,
    closed: &Cell<bool>,
    mut intercept: impl FnMut(&MSG) -> bool,
) {
    // SAFETY: owner は有効なウィンドウ。閉じた後に必ず有効に戻す。
    unsafe { _ = EnableWindow(owner, false) };
    let mut msg = MSG::default();
    while !closed.get() {
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
        if intercept(&msg) {
            continue;
        }
        // Tab での移動、Enter で既定のボタン、Esc でキャンセルを、ダイアログの決まりどおりに扱う。
        // SAFETY: dialog はこのスレッドのウィンドウで、msg は GetMessageW が埋めたもの。
        if unsafe { IsDialogMessageW(dialog, &msg) }.as_bool() {
            continue;
        }
        // SAFETY: msg は GetMessageW が埋めたもの。
        unsafe {
            _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // SAFETY: owner は有効なウィンドウ。
    unsafe {
        _ = EnableWindow(owner, true);
        _ = SetForegroundWindow(owner);
    }
}
