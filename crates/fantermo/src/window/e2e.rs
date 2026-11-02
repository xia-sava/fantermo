//! e2e テストの制御の口のうち、ウィンドウのスレッドで動く側（feature `e2e` のときだけコンパイルする）。
//!
//! 受ける側は `e2e_server`。答えを返してから借用の外ですること（ウィンドウを閉じる、メニューを開くなど）は
//! `Deferred` と同じく [`run_deferred`] に渡す。ウィンドウの大きさの変更はこの場で `WM_SIZE` が届くので、借用の
//! 外でしてから答える。ウィンドウへ送るメッセージも、この場でウィンドウプロシージャが動くので同じ。

use std::{cell::RefCell, ffi::c_void, ops::Range};

use serde_json::json;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, POINT, RECT, TRUE, WPARAM},
        Graphics::Gdi::{
            ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
            MonitorFromWindow, ScreenToClient,
        },
        System::{
            SystemServices::{
                MK_CONTROL, MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MK_SHIFT, MODIFIERKEYS_FLAGS,
            },
            Threading::GetCurrentThreadId,
        },
        UI::{
            Controls::{BST_CHECKED, NMHDR, NMUPDOWN, TDM_CLICK_BUTTON, UDN_DELTAPOS},
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{
                GetFocus, GetKeyboardState, IsWindowEnabled, MAPVK_VSC_TO_VK_EX, MapVirtualKeyW,
                SetKeyboardState, VIRTUAL_KEY, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_MENU, VK_RETURN,
                VK_SHIFT, VK_UP,
            },
            WindowsAndMessaging::{
                BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTORADIOBUTTON, CB_GETCOUNT,
                CB_GETCURSEL, CB_GETLBTEXT, CB_GETLBTEXTLEN, CB_SETCURSEL, CBN_SELCHANGE,
                ENDSESSION_LOGOFF, EndMenu, EnumChildWindows, EnumThreadWindows, GWL_STYLE,
                GetClassNameW, GetDlgCtrlID, GetMenuItemCount, GetMenuItemInfoW,
                GetNextDlgGroupItem, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW,
                GetWindowTextW, HMENU, IsWindow, IsWindowVisible, LB_GETCOUNT, LB_GETCURSEL,
                LB_GETTEXT, LB_GETTEXTLEN, LB_SETCURSEL, LBN_SELCHANGE, MENUITEMINFOW, MFS_CHECKED,
                MFS_GRAYED, MFS_HILITE, MFT_SEPARATOR, MIIM_BITMAP, MIIM_FTYPE, MIIM_STATE,
                MIIM_STRING, MIIM_SUBMENU, MN_GETHMENU, PostMessageW, SWP_NOACTIVATE, SWP_NOMOVE,
                SWP_NOZORDER, SendMessageW, SetWindowPos, SetWindowTextW, WM_CAPTURECHANGED,
                WM_CLOSE, WM_COMMAND, WM_ENDSESSION, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
                WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NOTIFY,
                WM_QUERYENDSESSION, WM_RBUTTONDOWN, WM_RBUTTONUP,
            },
        },
    },
    core::{BOOL, HSTRING, PWSTR},
};

use super::{
    App, Deferred, Launch, SIZE, TabCommand, WithApp, client_size, client_size_for_terminal,
    destroy_window, grid_pixels, run_deferred, tab_title,
};
use crate::{
    clipboard,
    e2e_server::{
        MenuKey, MouseAction, MouseHeld, MouseStep, Pending, Request, Response, WindowMessage,
    },
    keyboard::{self, KeyPosition},
    paint::Area,
    scroll_bar, version, window_state,
};
use fantermo_core::{
    keymap::KeyCombo,
    vt::{Key, KeyAction, Mods},
};

/// 絵を書くファイル。
const FRAME_FILE: &str = "frame.png";

/// `TaskDialogIndirect` とメッセージボックスのウィンドウのクラス。
const SYSTEM_DIALOG_CLASS: &str = "#32770";

/// メニューのウィンドウのクラス。
const MENU_CLASS: &str = "#32768";

/// キーを読み取るダイアログのウィンドウのクラス。
const KEY_CAPTURE_CLASS: &str = "fantermo-key-capture";

/// コントロールのウィンドウのクラス（大文字小文字を区別しない）。
const BUTTON_CLASS: &str = "Button";
const EDIT_CLASS: &str = "Edit";
const COMBO_BOX_CLASS: &str = "ComboBox";
const LIST_BOX_CLASS: &str = "ListBox";
const UP_DOWN_CLASS: &str = "msctls_updown32";

/// ボタンのスタイルのうち、ボタンの種類を表すビット。
const BUTTON_TYPE_MASK: isize = 0xF;

/// スキャンコードの上限。
const MAX_SCAN_CODE: u16 = 0xFF;
/// `MapVirtualKeyW` に拡張キーのスキャンコードを渡すときの接頭辞。
const EXTENDED_SCAN_CODE_PREFIX: u32 = 0xE000;
/// `WM_KEYDOWN` の `lParam` の、スキャンコードの位置。
const SCAN_CODE_SHIFT: u32 = 16;
/// `WM_KEYDOWN` の `lParam` の、拡張キーのビット。
const EXTENDED_KEY_BIT: isize = 1 << 24;

/// `WM_KEYDOWN` の `lParam`: 繰り返しの数が 1。
const KEY_DOWN_LPARAM: isize = 1;
/// `WM_KEYUP` の `lParam`: 繰り返しの数が 1 で、前に押されていて、離した。
const KEY_UP_LPARAM: isize = 0xC000_0001;

/// ウィンドウのクラスの名前の長さの上限（Windows の決まり）。
const MAX_CLASS_NAME: usize = 256;

/// 届いている要求をすべて処理する。ウィンドウが閉じたら残りは処理しない（`Control` と一緒に捨てられ、
/// 待っている側には閉じたと分かる）。
pub(super) fn on_wake(hwnd: HWND, with_app: &WithApp) {
    loop {
        let mut pending = None;
        with_app(&mut |app| {
            pending = app
                .e2e
                .as_ref()
                .and_then(|control| control.requests.try_recv().ok());
        });
        let Some(pending) = pending else {
            return;
        };
        run(hwnd, pending, with_app);
        // SAFETY: ウィンドウのハンドルを調べるだけ。
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return;
        }
    }
}

fn run(hwnd: HWND, Pending { request, reply }: Pending, with_app: &WithApp) {
    release_kept_modifiers(hwnd);
    let answer = |response: Response| {
        if reply.send(response).is_err() {
            tracing::warn!("the e2e control stopped waiting for an answer");
        }
    };
    match request {
        Request::Quit => {
            answer(Response::json(&json!({})));
            destroy_window(hwnd);
        }
        Request::ResizeWindow { cols, rows } => {
            answer(resize_window(hwnd, cols, rows, with_app));
        }
        Request::SendMessage { message, wparam } => {
            answer(send_window_message(hwnd, message, wparam));
        }
        // 操作がメニューを開くと、閉じるまで送った先から戻らない。答えを先に返す。続く要求はこのあと（メニューが
        // 開いているあいだはそのループの中で）処理されるので、順番は保たれる。
        Request::Mouse(steps) => {
            answer(Response::json(&json!({})));
            send_mouse(hwnd, &steps);
        }
        Request::MenuItems => answer(menu_items(hwnd)),
        Request::HighlightMenuItem(position) => answer(highlight_menu_item(hwnd, position)),
        Request::SelectMenuItem(position) => answer(select_menu_item(hwnd, position)),
        Request::MenuKey(key) => answer(press_menu_key(hwnd, key)),
        Request::CloseMenu => answer(close_menu(hwnd)),
        Request::DialogControls => answer(dialog_controls(hwnd)),
        Request::SetDialogText { id, text } => answer(set_dialog_text(hwnd, id, &text)),
        Request::SelectDialogItem { id, position } => {
            answer(select_dialog_item(hwnd, id, position));
        }
        Request::CheckDialogRadio(id) => answer(check_dialog_radio(hwnd, id)),
        Request::StepDialogSpin { id, delta } => answer(step_dialog_spin(hwnd, id, delta)),
        Request::PressDialogKey(combo) => answer(press_dialog_key(hwnd, combo)),
        Request::CloseDialog => answer(close_dialog(hwnd)),
        // 押すとダイアログを開くボタンは、閉じるまで送った先から戻らない。答えを先に返す。順番は `Request::Mouse` と
        // 同じく保たれる。
        Request::ClickDialogButton(id) => match button_to_click(hwnd, id) {
            Ok((dialog, button)) => {
                answer(Response::json(&json!({})));
                send_command(dialog, id, BN_CLICKED, button);
            }
            Err(response) => answer(response),
        },
        other => {
            let mut response = None;
            let mut deferred = None;
            let mut other = Some(other);
            with_app(&mut |app| {
                if let Some(request) = other.take() {
                    let (done, after) = app.run_e2e_request(request);
                    response = Some(done);
                    deferred = after;
                }
            });
            answer(response.unwrap_or_else(|| {
                Response::internal_error("the window is busy with another message")
            }));
            run_deferred(hwnd, deferred, with_app);
        }
    }
}

/// 端末が `cols` × `rows` になるようにウィンドウの大きさを変え、そのあとの状態を返す。
fn resize_window(hwnd: HWND, cols: u16, rows: u16, with_app: &WithApp) -> Response {
    let mut client = None;
    with_app(&mut |app| {
        client = Some(client_size_for_terminal(
            grid_pixels(&app.font, cols, rows),
            app.insets(app.dpi),
        ));
    });
    let Some(client) = client else {
        return Response::internal_error("the window is busy with another message");
    };
    let frame = match frame_size(hwnd) {
        Ok(frame) => frame,
        Err(e) => return Response::internal_error(format!("cannot read the window size: {e}")),
    };
    // SAFETY: hwnd は有効なウィンドウ。WM_SIZE がこの場で届くが、借用は持っていない。
    let moved = unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            client.cx + frame.cx,
            client.cy + frame.cy,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
    if let Err(e) = moved {
        return Response::internal_error(format!("cannot resize the window: {e}"));
    }
    let mut response = None;
    with_app(&mut |app| response = Some(Response::json(&app.e2e_state())));
    response.unwrap_or_else(|| Response::internal_error("the window is busy with another message"))
}

/// ウィンドウにメッセージを送り、ウィンドウプロシージャの戻り値を返す。
fn send_window_message(hwnd: HWND, message: WindowMessage, wparam: usize) -> Response {
    let (id, lparam) = match message {
        WindowMessage::QueryEndSession => (WM_QUERYENDSESSION, ENDSESSION_LOGOFF as isize),
        WindowMessage::EndSession => (WM_ENDSESSION, ENDSESSION_LOGOFF as isize),
        // lParam はキャプチャしたウィンドウ。このウィンドウではないことだけが意味を持つ。
        WindowMessage::CaptureChanged => (WM_CAPTURECHANGED, 0),
    };
    // SAFETY: hwnd は有効なウィンドウ。ウィンドウプロシージャがこの場で動くが、借用は持っていない。
    let result = unsafe { SendMessageW(hwnd, id, Some(WPARAM(wparam)), Some(LPARAM(lparam))) };
    Response::json(&json!({ "result": result.0 }))
}

/// `SetKeyboardState` の配列で、キーを押していることを示すビット。
const KEY_DOWN: u8 = 0x80;

/// このスレッドのキーボードの状態で、修飾キーを `held` のとおりにする。破棄すると元に戻す。fantermo は修飾キーを
/// `GetKeyState` で読むので、マウスのメッセージの `wParam` だけでは Alt も、Shift を読む箇所も動かない。押していない
/// キーも離した状態にして、テストを実行している間に利用者が押したキーに左右されないようにする。
struct HeldModifiers {
    saved: [u8; 256],
}

impl HeldModifiers {
    fn press(held: MouseHeld) -> Option<Self> {
        let mut saved = [0; 256];
        // SAFETY: saved は 256 バイトの書き込める配列。
        if let Err(e) = unsafe { GetKeyboardState(&mut saved) } {
            tracing::warn!("failed to read the keyboard state: {e}");
            return None;
        }
        let mut pressed = saved;
        for (down, key) in [
            (held.shift, VK_SHIFT),
            (held.ctrl, VK_CONTROL),
            (held.alt, VK_MENU),
        ] {
            if down {
                pressed[usize::from(key.0)] |= KEY_DOWN;
            } else {
                pressed[usize::from(key.0)] &= !KEY_DOWN;
            }
        }
        // SAFETY: pressed は 256 バイトの配列。変わるのはこのスレッドのキーボードの状態だけ。
        if let Err(e) = unsafe { SetKeyboardState(&pressed) } {
            tracing::warn!("failed to set the keyboard state: {e}");
            return None;
        }
        Some(Self { saved })
    }
}

impl Drop for HeldModifiers {
    fn drop(&mut self) {
        // SAFETY: saved は 256 バイトの配列。
        if let Err(e) = unsafe { SetKeyboardState(&self.saved) } {
            tracing::warn!("failed to restore the keyboard state: {e}");
        }
    }
}

/// `step` の、マウスのメッセージの番号と `wParam` と `lParam`。`lParam` は下位がクライアント領域の x、上位が y で、
/// ホイールだけ画面の座標（`WM_MOUSEWHEEL` の決まり）。
fn mouse_message(hwnd: HWND, step: &MouseStep) -> (u32, WPARAM, LPARAM) {
    const NONE: MODIFIERKEYS_FLAGS = MODIFIERKEYS_FLAGS(0);
    let (id, pressed, released) = match step.action {
        MouseAction::Move => (WM_MOUSEMOVE, NONE, NONE),
        MouseAction::Wheel(_) => (WM_MOUSEWHEEL, NONE, NONE),
        MouseAction::LeftDown => (WM_LBUTTONDOWN, MK_LBUTTON, NONE),
        MouseAction::LeftUp => (WM_LBUTTONUP, NONE, MK_LBUTTON),
        MouseAction::MiddleDown => (WM_MBUTTONDOWN, MK_MBUTTON, NONE),
        MouseAction::MiddleUp => (WM_MBUTTONUP, NONE, MK_MBUTTON),
        MouseAction::RightDown => (WM_RBUTTONDOWN, MK_RBUTTON, NONE),
        MouseAction::RightUp => (WM_RBUTTONUP, NONE, MK_RBUTTON),
    };
    let held = step.held;
    let mut flags = pressed.0;
    for (down, flag) in [
        (held.left, MK_LBUTTON),
        (held.middle, MK_MBUTTON),
        (held.right, MK_RBUTTON),
        (held.shift, MK_SHIFT),
        (held.ctrl, MK_CONTROL),
    ] {
        if down {
            flags |= flag.0;
        }
    }
    flags &= !released.0;
    let (mut wparam, mut x, mut y) = (flags as usize, i32::from(step.x), i32::from(step.y));
    if let MouseAction::Wheel(delta) = step.action {
        wparam |= (delta as u16 as usize) << 16;
        let mut point = POINT { x, y };
        // SAFETY: hwnd は有効なウィンドウで、point は書き込める。
        if !unsafe { ClientToScreen(hwnd, &mut point) }.as_bool() {
            tracing::warn!("failed to convert the wheel position");
        }
        (x, y) = (point.x, point.y);
    }
    let lparam = (x as u16 as usize | (y as u16 as usize) << 16) as isize;
    (id, WPARAM(wparam), LPARAM(lparam))
}

/// マウスの操作を順に送る。送った先でウィンドウが閉じたら、残りは送らない。
fn send_mouse(hwnd: HWND, steps: &[MouseStep]) {
    for step in steps {
        // SAFETY: ウィンドウのハンドルを調べるだけ。
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return;
        }
        let (id, wparam, lparam) = mouse_message(hwnd, step);
        let _modifiers = HeldModifiers::press(step.held);
        // SAFETY: hwnd は有効なウィンドウ。ウィンドウプロシージャがこの場で動くが、借用は持っていない。
        unsafe { SendMessageW(hwnd, id, Some(wparam), Some(lparam)) };
    }
}

/// ウィンドウの大きさとクライアント領域の大きさの差。枠の分。
fn frame_size(hwnd: HWND) -> windows::core::Result<SIZE> {
    let mut rect = RECT::default();
    // SAFETY: hwnd は有効なウィンドウで、rect は書き込める。
    unsafe { GetWindowRect(hwnd, &mut rect)? };
    let (width, height) = client_size(hwnd);
    Ok(SIZE {
        cx: (rect.right - rect.left) - width as i32,
        cy: (rect.bottom - rect.top) - height as i32,
    })
}

/// ウィンドウ `hwnd` のタイトルや、コントロールの文字。
fn real_window_title(hwnd: HWND) -> String {
    // SAFETY: hwnd は有効なウィンドウ。既定の処理が答えるので、借用には触らない。
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    let mut buffer = vec![0u16; length as usize + 1];
    // SAFETY: buffer は書き込める。
    let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..copied as usize])
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; MAX_CLASS_NAME];
    // SAFETY: hwnd は有効なウィンドウで、buffer は書き込める。
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length as usize])
}

/// 見えているウィンドウを `lparam` の指す `Vec<HWND>` に足す、ウィンドウを列挙するコールバック。
unsafe extern "system" fn collect_visible(window: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam は、列挙している間だけ生きている `Vec<HWND>` を指す（`open_dialogs` と `control_texts`）。
    let windows = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    // SAFETY: window は列挙で渡された有効なウィンドウ。
    if unsafe { IsWindowVisible(window) }.as_bool() {
        windows.push(window);
    }
    TRUE
}

/// このスレッドが開いているウィンドウのうち、`main` のほかの見えているもの。
fn open_dialogs(main: HWND) -> Vec<HWND> {
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: windows はこの呼び出しの間は生きている。列挙の失敗は、見つからなかったことと同じに扱う。
    unsafe {
        _ = EnumThreadWindows(
            GetCurrentThreadId(),
            Some(collect_visible),
            LPARAM((&raw mut windows) as isize),
        );
    }
    windows.retain(|&window| window != main);
    windows
}

/// ダイアログの見えているコントロールの文字（入力欄の中身、説明の文、ボタン）。
fn control_texts(dialog: HWND) -> Vec<String> {
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: windows はこの呼び出しの間は生きている。列挙の失敗は、コントロールがないことと同じに扱う。
    unsafe {
        _ = EnumChildWindows(
            Some(dialog),
            Some(collect_visible),
            LPARAM((&raw mut windows) as isize),
        );
    }
    windows.into_iter().map(real_window_title).collect()
}

/// 開いているダイアログの `id` のボタンを押す。押す操作は積むだけで、ダイアログが自分のループで処理する。
fn press_dialog_button(main: HWND, id: i32) -> Response {
    let Some(&dialog) = open_dialogs(main).first() else {
        return Response::bad_request("no dialog is open");
    };
    let message = if class_name(dialog) == SYSTEM_DIALOG_CLASS {
        TDM_CLICK_BUTTON.0 as u32
    } else {
        WM_COMMAND
    };
    // SAFETY: dialog はこのスレッドのウィンドウ。積むだけで、この場では処理されない。
    match unsafe { PostMessageW(Some(dialog), message, WPARAM(id as usize), LPARAM(0)) } {
        Ok(()) => Response::json(&json!({})),
        Err(e) => Response::internal_error(format!("cannot press the button: {e}")),
    }
}

/// `window` のすべての子孫のウィンドウ。
fn descendants(window: HWND) -> Vec<HWND> {
    /// 列挙で渡されたウィンドウを `lparam` の指す `Vec<HWND>` に足す。
    unsafe extern "system" fn collect(child: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam は、列挙している間だけ生きている `Vec<HWND>` を指す。
        unsafe { &mut *(lparam.0 as *mut Vec<HWND>) }.push(child);
        TRUE
    }
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: windows はこの呼び出しの間は生きている。列挙の失敗は、子がないことと同じに扱う。
    unsafe {
        _ = EnumChildWindows(
            Some(window),
            Some(collect),
            LPARAM((&raw mut windows) as isize),
        );
    }
    windows
}

/// いちばん上の、利用者が操作できるダイアログ。メッセージボックスや色を選ぶダイアログが上に開いていれば、下の
/// ダイアログは無効なのでそちらになる。
fn top_dialog(main: HWND) -> Result<HWND, Response> {
    open_dialogs(main)
        .into_iter()
        .find(|&window| {
            // SAFETY: window はこのスレッドの有効なウィンドウ。
            class_name(window) != MENU_CLASS && unsafe { IsWindowEnabled(window) }.as_bool()
        })
        .ok_or_else(|| Response::bad_request("no dialog is open"))
}

/// `dialog` の子孫のうち、ID が `id` でクラスが `class` のコントロール。利用者が触れないもの（見えていない、押せない）は
/// 断る。ファイルを選ぶダイアログのように同じ ID の入れ子があるので、クラスでも絞る。
fn usable_control(dialog: HWND, id: i32, class: &str) -> Result<HWND, Response> {
    let control = descendants(dialog)
        .into_iter()
        .find(|&child| {
            // SAFETY: 子のウィンドウの ID を問い合わせるだけ。
            let child_id = unsafe { GetDlgCtrlID(child) };
            child_id == id && class_name(child).eq_ignore_ascii_case(class)
        })
        .ok_or_else(|| Response::bad_request(format!("no {class} of the ID {id}")))?;
    // SAFETY: control はこのスレッドの有効なウィンドウ。
    let usable =
        unsafe { IsWindowVisible(control).as_bool() && IsWindowEnabled(control).as_bool() };
    if !usable {
        return Err(Response::bad_request(format!(
            "the {class} of the ID {id} is hidden or disabled"
        )));
    }
    Ok(control)
}

/// `dialog` へ、コントロール `control`（ID は `id`）からの通知 `code` を `WM_COMMAND` で送る。
fn send_command(dialog: HWND, id: i32, code: u32, control: HWND) {
    let wparam = (id as u16 as usize) | ((code as u16 as usize) << 16);
    // SAFETY: dialog はこのスレッドの有効なウィンドウ。ウィンドウプロシージャがこの場で動くが、借用は持っていない。
    unsafe {
        SendMessageW(
            dialog,
            WM_COMMAND,
            Some(WPARAM(wparam)),
            Some(LPARAM(control.0 as isize)),
        )
    };
}

/// コントロールの文字。
fn control_text(control: HWND) -> String {
    real_window_title(control)
}

/// `count` 個の項目の文字を、項目ごとに長さと中身を問い合わせて読む。
fn list_items(
    control: HWND,
    count_message: u32,
    length_message: u32,
    text_message: u32,
) -> Vec<String> {
    // SAFETY: control は有効なコントロール。
    let count = unsafe { SendMessageW(control, count_message, None, None) }.0;
    (0..count)
        .map(|index| {
            let position = Some(WPARAM(index as usize));
            // SAFETY: control は有効なコントロールで、index は項目の数より小さい。
            let length = unsafe { SendMessageW(control, length_message, position, None) }.0;
            let mut buffer = vec![0u16; length.max(0) as usize + 1];
            // SAFETY: buffer は項目の文字と終端が入る大きさで、この呼び出しの間は生きている。
            let copied = unsafe {
                SendMessageW(
                    control,
                    text_message,
                    position,
                    Some(LPARAM(buffer.as_mut_ptr() as isize)),
                )
            }
            .0;
            String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
        })
        .collect()
}

/// `control` の値。クラスごとに、ボタンは `checked`、コンボボックスと一覧は `selected`（選んでいなければ `null`）と
/// `items`。位置はダイアログのクライアント領域の座標。
fn control_json(dialog: HWND, control: HWND) -> serde_json::Value {
    let class = class_name(control);
    let mut rect = RECT::default();
    // SAFETY: control は有効なウィンドウで、rect は書き込める。
    if let Err(e) = unsafe { GetWindowRect(control, &mut rect) } {
        tracing::warn!("failed to read the rectangle of a control: {e}");
    }
    let mut top_left = POINT {
        x: rect.left,
        y: rect.top,
    };
    // SAFETY: dialog は有効なウィンドウで、top_left は書き込める。
    if !unsafe { ScreenToClient(dialog, &mut top_left) }.as_bool() {
        tracing::warn!("failed to convert the position of a control");
    }
    let mut value = json!({
        // SAFETY: 子のウィンドウの ID を問い合わせるだけ。
        "id": unsafe { GetDlgCtrlID(control) },
        "class": class,
        "text": control_text(control),
        // SAFETY: control は有効なウィンドウ。
        "visible": unsafe { IsWindowVisible(control) }.as_bool(),
        // SAFETY: control は有効なウィンドウ。
        "enabled": unsafe { IsWindowEnabled(control) }.as_bool(),
        "x": top_left.x,
        "y": top_left.y,
        "width": rect.right - rect.left,
        "height": rect.bottom - rect.top,
    });
    let selected = |message: u32| {
        // SAFETY: control は有効なコントロール。
        usize::try_from(unsafe { SendMessageW(control, message, None, None) }.0).ok()
    };
    if class.eq_ignore_ascii_case(BUTTON_CLASS) {
        // SAFETY: control は有効なボタン。
        let state = unsafe { SendMessageW(control, BM_GETCHECK, None, None) }.0;
        value["checked"] = json!(state == BST_CHECKED.0 as isize);
    } else if class.eq_ignore_ascii_case(COMBO_BOX_CLASS) {
        value["selected"] = json!(selected(CB_GETCURSEL));
        value["items"] = json!(list_items(
            control,
            CB_GETCOUNT,
            CB_GETLBTEXTLEN,
            CB_GETLBTEXT
        ));
    } else if class.eq_ignore_ascii_case(LIST_BOX_CLASS) {
        value["selected"] = json!(selected(LB_GETCURSEL));
        value["items"] = json!(list_items(control, LB_GETCOUNT, LB_GETTEXTLEN, LB_GETTEXT));
    }
    value
}

/// いちばん上のダイアログの大きさと、コントロールの値。`width` と `height` は枠を含むウィンドウの大きさ、`work_area` は
/// ウィンドウのあるモニターの作業領域で、どちらも画面の座標のピクセル。`dpi` はダイアログの DPI。`focus` はフォーカスの
/// あるコントロールの ID。
fn dialog_controls(main: HWND) -> Response {
    let dialog = match top_dialog(main) {
        Ok(dialog) => dialog,
        Err(response) => return response,
    };
    let mut rect = RECT::default();
    // SAFETY: dialog は有効なウィンドウで、rect は書き込める。
    if let Err(e) = unsafe { GetWindowRect(dialog, &mut rect) } {
        return Response::internal_error(format!("cannot read the dialog size: {e}"));
    }
    let (client_width, client_height) = client_size(dialog);
    let mut monitor = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: dialog は有効なウィンドウで、monitor は cbSize を入れて初期化済み。
    let found = unsafe {
        GetMonitorInfoW(
            MonitorFromWindow(dialog, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        )
    };
    if !found.as_bool() {
        return Response::internal_error("cannot read the work area");
    }
    let work = monitor.rcWork;
    let controls: Vec<serde_json::Value> = descendants(dialog)
        .into_iter()
        .map(|control| control_json(dialog, control))
        .collect();
    // SAFETY: このスレッドのフォーカスを問い合わせるだけ。
    let focus = unsafe { GetFocus() };
    Response::json(&json!({
        "class": class_name(dialog),
        "title": real_window_title(dialog),
        // SAFETY: dialog は有効なウィンドウ。
        "dpi": unsafe { GetDpiForWindow(dialog) },
        "left": rect.left,
        "top": rect.top,
        "width": rect.right - rect.left,
        "height": rect.bottom - rect.top,
        "client_width": client_width,
        "client_height": client_height,
        "work_area": {
            "left": work.left,
            "top": work.top,
            "right": work.right,
            "bottom": work.bottom,
        },
        // SAFETY: 無効なハンドルなら 0 が返る。
        "focus": unsafe { GetDlgCtrlID(focus) },
        "controls": controls,
    }))
}

fn set_dialog_text(main: HWND, id: i32, text: &str) -> Response {
    let control = top_dialog(main).and_then(|dialog| usable_control(dialog, id, EDIT_CLASS));
    let control = match control {
        Ok(control) => control,
        Err(response) => return response,
    };
    // SAFETY: control は有効なウィンドウ。親に EN_CHANGE がこの場で届くが、借用は持っていない。
    match unsafe { SetWindowTextW(control, &HSTRING::from(text)) } {
        Ok(()) => Response::json(&json!({})),
        Err(e) => Response::internal_error(format!("cannot set the text: {e}")),
    }
}

/// コンボボックスか一覧の `position` 番目の項目を選び、親に選択の変更を知らせる。
fn select_dialog_item(main: HWND, id: i32, position: usize) -> Response {
    let dialog = match top_dialog(main) {
        Ok(dialog) => dialog,
        Err(response) => return response,
    };
    let is_combo_box = descendants(dialog).into_iter().any(|child| {
        // SAFETY: 子のウィンドウの ID を問い合わせるだけ。
        let child_id = unsafe { GetDlgCtrlID(child) };
        child_id == id && class_name(child).eq_ignore_ascii_case(COMBO_BOX_CLASS)
    });
    let found = if is_combo_box {
        usable_control(dialog, id, COMBO_BOX_CLASS)
            .map(|control| (control, CB_GETCOUNT, CB_SETCURSEL, CBN_SELCHANGE))
    } else {
        usable_control(dialog, id, LIST_BOX_CLASS)
            .map(|control| (control, LB_GETCOUNT, LB_SETCURSEL, LBN_SELCHANGE))
    };
    let (control, count_message, select_message, changed) = match found {
        Ok(found) => found,
        Err(response) => return response,
    };
    // SAFETY: control は有効なコントロール。
    let count = unsafe { SendMessageW(control, count_message, None, None) }.0;
    if !usize::try_from(count).is_ok_and(|count| position < count) {
        return Response::bad_request(format!("no item at {position} of {count}"));
    }
    // SAFETY: control は有効なコントロールで、position は項目の数より小さい。
    unsafe { SendMessageW(control, select_message, Some(WPARAM(position)), None) };
    send_command(dialog, id, changed, control);
    Response::json(&json!({}))
}

/// ラジオボタンを選ぶ。同じ並びのほかのラジオボタンを外し、親に押したことを知らせる。別のスレッドからの `BM_CLICK` と
/// 違い、ダイアログがアクティブでなくても届く。
fn check_dialog_radio(main: HWND, id: i32) -> Response {
    let dialog = match top_dialog(main) {
        Ok(dialog) => dialog,
        Err(response) => return response,
    };
    let radio = match usable_control(dialog, id, BUTTON_CLASS) {
        Ok(radio) => radio,
        Err(response) => return response,
    };
    let is_radio = |button: HWND| {
        // SAFETY: button は有効なウィンドウ。
        let style = unsafe { GetWindowLongPtrW(button, GWL_STYLE) };
        class_name(button).eq_ignore_ascii_case(BUTTON_CLASS)
            && style & BUTTON_TYPE_MASK == BS_AUTORADIOBUTTON as isize
    };
    if !is_radio(radio) {
        return Response::bad_request(format!("the ID {id} is not a radio button"));
    }
    let mut member = radio;
    loop {
        // SAFETY: dialog は member の親。並びの最後の次は先頭へ戻る。
        member = match unsafe { GetNextDlgGroupItem(dialog, Some(member), false) } {
            Ok(next) => next,
            Err(e) => return Response::internal_error(format!("cannot walk the group: {e}")),
        };
        if member == radio {
            break;
        }
        if is_radio(member) {
            // SAFETY: member は有効なボタン。
            unsafe { SendMessageW(member, BM_SETCHECK, Some(WPARAM(0)), None) };
        }
    }
    // SAFETY: radio は有効なボタン。
    unsafe {
        SendMessageW(
            radio,
            BM_SETCHECK,
            Some(WPARAM(BST_CHECKED.0 as usize)),
            None,
        )
    };
    send_command(dialog, id, BN_CLICKED, radio);
    Response::json(&json!({}))
}

/// 押せるボタン `id` と、そのダイアログ。
fn button_to_click(main: HWND, id: i32) -> Result<(HWND, HWND), Response> {
    let dialog = top_dialog(main)?;
    Ok((dialog, usable_control(dialog, id, BUTTON_CLASS)?))
}

/// 上下の矢印 `id` を `delta` の分だけ押したことを親に知らせる。矢印のウィンドウへマウスのメッセージを送ると
/// キャプチャを取るので、矢印が親へ送るのと同じ通知を送る。
fn step_dialog_spin(main: HWND, id: i32, delta: i32) -> Response {
    let dialog = match top_dialog(main) {
        Ok(dialog) => dialog,
        Err(response) => return response,
    };
    let spin = match usable_control(dialog, id, UP_DOWN_CLASS) {
        Ok(spin) => spin,
        Err(response) => return response,
    };
    let mut notification = NMUPDOWN {
        hdr: NMHDR {
            hwndFrom: spin,
            idFrom: id as usize,
            code: UDN_DELTAPOS,
        },
        iPos: 0,
        iDelta: delta,
    };
    // SAFETY: dialog は有効なウィンドウで、notification はこの呼び出しの間は生きている。ウィンドウプロシージャが
    // この場で動くが、借用は持っていない。
    unsafe {
        SendMessageW(
            dialog,
            WM_NOTIFY,
            Some(WPARAM(id as usize)),
            Some(LPARAM((&raw mut notification) as isize)),
        )
    };
    Response::json(&json!({}))
}

/// `key` を打つ位置を、スキャンコードの順に探す。`(スキャンコード, 拡張キー)`。
fn position_of_key(key: Key) -> Option<(u16, bool)> {
    (0..=MAX_SCAN_CODE)
        .flat_map(|scan_code| [(scan_code, false), (scan_code, true)])
        .find(|&(scan_code, extended)| {
            keyboard::key_from_position(KeyPosition {
                scan_code,
                extended,
                repeated: false,
            }) == key
        })
}

thread_local! {
    /// キーを読み取るダイアログが閉じるまで保つ、キーボードの状態。
    static KEPT_MODIFIERS: RefCell<Option<HeldModifiers>> = const { RefCell::new(None) };
}

/// キーを読み取るダイアログが閉じていれば、保っていたキーボードの状態を元に戻す。
fn release_kept_modifiers(main: HWND) {
    let capturing = open_dialogs(main)
        .into_iter()
        .any(|window| class_name(window) == KEY_CAPTURE_CLASS);
    if !capturing {
        KEPT_MODIFIERS.take();
    }
}

/// いちばん上のダイアログのループへ `combo` のキーを積む。フォーカスのあるコントロールに届いた形にする。修飾キーは、
/// 積んだキーをダイアログのループが読むときに押している必要がある。読むのはこの要求の後なので、キーを読み取るダイアログ
/// なら閉じるまで保つ（`release_kept_modifiers`）。
fn press_dialog_key(main: HWND, combo: KeyCombo) -> Response {
    let dialog = match top_dialog(main) {
        Ok(dialog) => dialog,
        Err(response) => return response,
    };
    let Some((scan_code, extended)) = position_of_key(combo.key) else {
        return Response::bad_request(format!("no position for the key {combo}"));
    };
    let extended_prefix = if extended {
        EXTENDED_SCAN_CODE_PREFIX
    } else {
        0
    };
    let virtual_key = {
        // SAFETY: スキャンコードの変換を問い合わせるだけ。
        unsafe { MapVirtualKeyW(u32::from(scan_code) | extended_prefix, MAPVK_VSC_TO_VK_EX) }
    };
    let mut lparam = KEY_DOWN_LPARAM | ((scan_code as isize) << SCAN_CODE_SHIFT);
    if extended {
        lparam |= EXTENDED_KEY_BIT;
    }
    let held = HeldModifiers::press(MouseHeld {
        shift: combo.mods.contains(Mods::SHIFT),
        ctrl: combo.mods.contains(Mods::CTRL),
        alt: combo.mods.contains(Mods::ALT),
        ..MouseHeld::default()
    });
    // SAFETY: このスレッドのフォーカスを問い合わせるだけ。
    let focus = unsafe { GetFocus() };
    let target = if focus.is_invalid() { dialog } else { focus };
    // SAFETY: target はこのスレッドのウィンドウ。積むだけで、この場では処理されない。
    let posted = unsafe {
        PostMessageW(
            Some(target),
            WM_KEYDOWN,
            WPARAM(virtual_key as usize),
            LPARAM(lparam),
        )
    };
    if let Err(e) = posted {
        return Response::internal_error(format!("cannot send the key: {e}"));
    }
    if class_name(dialog) == KEY_CAPTURE_CLASS {
        KEPT_MODIFIERS.set(held);
    }
    Response::json(&json!({}))
}

/// いちばん上のダイアログの × を押したことにする。積むだけで、ダイアログが自分のループで処理する。
fn close_dialog(main: HWND) -> Response {
    let dialog = match top_dialog(main) {
        Ok(dialog) => dialog,
        Err(response) => return response,
    };
    // SAFETY: dialog はこのスレッドのウィンドウ。積むだけで、この場では処理されない。
    match unsafe { PostMessageW(Some(dialog), WM_CLOSE, WPARAM(0), LPARAM(0)) } {
        Ok(()) => Response::json(&json!({})),
        Err(e) => Response::internal_error(format!("cannot close the dialog: {e}")),
    }
}

/// 開いているメニューの項目。
struct MenuItem {
    /// 項目の文字。アクセラレータの `&` と、キーの組み合わせの前のタブも含む。
    text: String,
    separator: bool,
    enabled: bool,
    checked: bool,
    /// 矢印のキーなどで選んでいる。
    highlighted: bool,
    submenu: bool,
    /// ビットマップ（プロファイルのアイコン）が付いている。
    icon: bool,
}

/// `menu` の `position` 番目の項目を読む。
fn read_menu_item(menu: HMENU, position: u32) -> windows::core::Result<MenuItem> {
    let mut info = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_FTYPE | MIIM_STATE | MIIM_SUBMENU | MIIM_BITMAP | MIIM_STRING,
        ..Default::default()
    };
    // SAFETY: menu は有効なメニューで、info は cbSize を入れて初期化済み。文字の置き場所がないので、長さだけが入る。
    unsafe { GetMenuItemInfoW(menu, position, true, &mut info)? };
    let mut buffer = vec![0u16; info.cch as usize + 1];
    info.dwTypeData = PWSTR(buffer.as_mut_ptr());
    info.cch = buffer.len() as u32;
    // SAFETY: dwTypeData は cch 文字分の書き込める領域を指し、buffer はこの呼び出しの間は生きている。
    unsafe { GetMenuItemInfoW(menu, position, true, &mut info)? };
    Ok(MenuItem {
        text: String::from_utf16_lossy(&buffer[..info.cch as usize]),
        separator: info.fType.contains(MFT_SEPARATOR),
        enabled: (info.fState.0 & MFS_GRAYED.0) == 0,
        checked: info.fState.contains(MFS_CHECKED),
        highlighted: info.fState.contains(MFS_HILITE),
        submenu: !info.hSubMenu.is_invalid(),
        icon: !info.hbmpItem.is_invalid(),
    })
}

/// 開いているメニューのウィンドウと、その項目。メニューが開いていなければ、返す答えを `Err` に入れて返す。
fn open_menu(main: HWND) -> Result<(HWND, Vec<MenuItem>), Response> {
    let window = open_dialogs(main)
        .into_iter()
        .find(|&window| class_name(window) == MENU_CLASS)
        .ok_or_else(|| Response::bad_request("no menu is open"))?;
    // SAFETY: window はこのスレッドのメニューのウィンドウ。答えはそのメニューのハンドル。
    let menu = HMENU(unsafe { SendMessageW(window, MN_GETHMENU, None, None) }.0 as *mut c_void);
    // SAFETY: メニューのハンドルは調べるだけで、無効なら -1 が返る。
    let count = u32::try_from(unsafe { GetMenuItemCount(Some(menu)) })
        .map_err(|_| Response::internal_error("cannot read the number of the menu items"))?;
    let items = (0..count)
        .map(|position| read_menu_item(menu, position))
        .collect::<windows::core::Result<_>>()
        .map_err(|e| Response::internal_error(format!("cannot read the menu items: {e}")))?;
    Ok((window, items))
}

fn menu_items(main: HWND) -> Response {
    let items = match open_menu(main) {
        Ok((_, items)) => items,
        Err(response) => return response,
    };
    let items: Vec<serde_json::Value> = items
        .iter()
        .map(|item| {
            json!({
                "text": item.text,
                "separator": item.separator,
                "enabled": item.enabled,
                "checked": item.checked,
                "highlighted": item.highlighted,
                "submenu": item.submenu,
                "icon": item.icon,
            })
        })
        .collect();
    Response::json(&json!({ "items": items }))
}

/// メニューのウィンドウへキーを積む。メニューのループが、利用者が押したキーと同じように処理する。
fn post_menu_key(window: HWND, key: VIRTUAL_KEY) -> Result<(), Response> {
    let code = WPARAM(usize::from(key.0));
    for (message, lparam) in [(WM_KEYDOWN, KEY_DOWN_LPARAM), (WM_KEYUP, KEY_UP_LPARAM)] {
        // SAFETY: window はこのスレッドのウィンドウ。積むだけで、この場では処理されない。
        unsafe { PostMessageW(Some(window), message, code, LPARAM(lparam)) }
            .map_err(|e| Response::internal_error(format!("cannot send the menu key: {e}")))?;
    }
    Ok(())
}

/// `items` の `position` 番目の項目へ移るキーを積む。区切りのほかはすべて矢印のキーの一歩で、区切りは飛ばされる。
fn post_keys_to_menu_item(
    window: HWND,
    items: &[MenuItem],
    position: usize,
) -> Result<(), Response> {
    if items.get(position).is_none_or(|item| item.separator) {
        return Err(Response::bad_request(format!("no menu item at {position}")));
    }
    let steps = |range: Range<usize>| items[range].iter().filter(|item| !item.separator).count();
    let (key, count) = match items.iter().position(|item| item.highlighted) {
        None => (VK_DOWN, steps(0..position + 1)),
        Some(current) if current <= position => (VK_DOWN, steps(current + 1..position + 1)),
        Some(current) => (VK_UP, steps(position..current)),
    };
    for _ in 0..count {
        post_menu_key(window, key)?;
    }
    Ok(())
}

fn highlight_menu_item(main: HWND, position: usize) -> Response {
    let moved = open_menu(main)
        .and_then(|(window, items)| post_keys_to_menu_item(window, &items, position));
    match moved {
        Ok(()) => Response::json(&json!({})),
        Err(response) => response,
    }
}

fn select_menu_item(main: HWND, position: usize) -> Response {
    let selected = open_menu(main).and_then(|(window, items)| {
        post_keys_to_menu_item(window, &items, position)?;
        post_menu_key(window, VK_RETURN)
    });
    match selected {
        Ok(()) => Response::json(&json!({})),
        Err(response) => response,
    }
}

fn press_menu_key(main: HWND, key: MenuKey) -> Response {
    let key = match key {
        MenuKey::Escape => VK_ESCAPE,
        MenuKey::Accelerator(character) => VIRTUAL_KEY(character as u16),
    };
    match open_menu(main).and_then(|(window, _)| post_menu_key(window, key)) {
        Ok(()) => Response::json(&json!({})),
        Err(response) => response,
    }
}

/// 開いているメニューを閉じる。`EndMenu` はメニューがなくても成功を返すので、先に調べる。
fn close_menu(main: HWND) -> Response {
    if let Err(response) = open_menu(main) {
        return response;
    }
    // SAFETY: このスレッドが開いているメニューを閉じる。
    match unsafe { EndMenu() } {
        Ok(()) => Response::json(&json!({})),
        Err(e) => Response::internal_error(format!("cannot close the menu: {e}")),
    }
}

fn area_json(area: Area) -> serde_json::Value {
    json!({ "x": area.x, "y": area.y, "width": area.width, "height": area.height })
}

impl App {
    /// 借用の中で済む操作をする。続けて借用の外ですることがあれば、一緒に返す。
    fn run_e2e_request(&mut self, request: Request) -> (Response, Option<Deferred>) {
        let done = || Response::json(&json!({}));
        match request {
            Request::State => (Response::json(&self.e2e_state()), None),
            Request::Frame => (self.e2e_frame(), None),
            Request::Input(bytes) => {
                self.send(&bytes);
                (done(), None)
            }
            Request::Text(text) => {
                for unit in text.encode_utf16() {
                    self.on_text_unit(unit);
                }
                (done(), None)
            }
            Request::Key { combo, unshifted } => {
                let deferred =
                    self.on_key_input(combo.key, combo.mods, unshifted, KeyAction::Press, &[]);
                (done(), deferred)
            }
            Request::NewTab => (done(), self.run_tab_command(TabCommand::New)),
            Request::NewProfileTab(index) => {
                if index >= self.profiles.len() {
                    return (
                        Response::bad_request(format!("no profile at {index}")),
                        None,
                    );
                }
                self.open_session(Launch::Profile(index));
                (done(), None)
            }
            Request::CloseTab => (done(), self.run_tab_command(TabCommand::Close)),
            Request::Paste(text) => (done(), self.paste_text(text)),
            Request::PressDialogButton(id) => (press_dialog_button(self.hwnd, id), None),
            Request::ReadClipboard => match clipboard::text(self.hwnd) {
                Ok(text) => (Response::json(&json!({ "text": text })), None),
                Err(e) => (
                    Response::internal_error(format!("cannot read the clipboard: {e}")),
                    None,
                ),
            },
            Request::WriteClipboard(text) => match clipboard::set_text(self.hwnd, &text) {
                Ok(()) => (done(), None),
                Err(e) => (
                    Response::internal_error(format!("cannot write the clipboard: {e}")),
                    None,
                ),
            },
            Request::SelectTab(index) => {
                if index >= self.sessions.len() {
                    return (Response::bad_request(format!("no tab at {index}")), None);
                }
                self.activate(index);
                (done(), None)
            }
            Request::Quit
            | Request::ResizeWindow { .. }
            | Request::SendMessage { .. }
            | Request::Mouse(_)
            | Request::MenuItems
            | Request::HighlightMenuItem(_)
            | Request::SelectMenuItem(_)
            | Request::MenuKey(_)
            | Request::CloseMenu
            | Request::DialogControls
            | Request::SetDialogText { .. }
            | Request::SelectDialogItem { .. }
            | Request::CheckDialogRadio(_)
            | Request::ClickDialogButton(_)
            | Request::StepDialogSpin { .. }
            | Request::PressDialogKey(_)
            | Request::CloseDialog => (
                Response::internal_error("handled outside of the borrow"),
                None,
            ),
        }
    }

    /// いまの状態。`text` は表示している端末の、スクロールバックを含む全体の文字。`images` は表示している端末の
    /// 画面に出ている画像の数。`dialogs` はこのウィンドウの上に開いているダイアログ。`controls` はその
    /// コントロールの文字で、`TaskDialogIndirect` の本文は入らない。`window` は状態ファイルに書くのと同じ、
    /// ウィンドウの位置・大きさ・最大化。`background` は背景画像を敷く範囲（クライアント領域の座標）と、表示している
    /// タブが選んだ画像のファイルの名前。`selection` は選択範囲の文字列（なければ `null`）。`mouse_tracking` は
    /// アプリがマウスを求めているか。`scroll` は表示している範囲（`total` 全体の行数、`offset` 先頭の行、`len` 行数）と、
    /// ホイール 1 ノッチの行数 `lines_per_notch`。`scroll_bar` はスクロールバーの範囲と、つまみ `thumb`（`top`、
    /// `length`。なければ `null`）、押している間だけ真の `pressed`。`tab_bar` はタブとボタンの範囲で、`tabs` は
    /// 描いているタブだけ。`window_enabled` はウィンドウが入力を受けるか（ダイアログの間は偽）。
    fn e2e_state(&mut self) -> serde_json::Value {
        let size = self.session().vt.size();
        let text = self.session().vt.plain_text().unwrap_or_else(|e| {
            tracing::warn!("failed to format the screen: {e}");
            String::new()
        });
        let images = self.session_mut().vt.frame().map_or_else(
            |e| {
                tracing::warn!("failed to read the screen: {e}");
                0
            },
            |frame| frame.images.len(),
        );
        let dialogs: Vec<serde_json::Value> = open_dialogs(self.hwnd)
            .into_iter()
            .map(|dialog| {
                json!({
                    "class": class_name(dialog),
                    "title": real_window_title(dialog),
                    "controls": control_texts(dialog),
                })
            })
            .collect();
        let (client_width, client_height) = client_size(self.hwnd);
        let tabs: Vec<serde_json::Value> = self
            .sessions
            .iter()
            .map(|session| {
                let size = session.vt.size();
                json!({
                    "title": tab_title(session),
                    "profile": session.profile_name,
                    "cols": size.cols,
                    "rows": size.rows,
                    "bell": session.bell_marked,
                })
            })
            .collect();
        let profiles: Vec<&str> = self
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect();
        let background_image = self
            .session()
            .background_image
            .as_ref()
            .and_then(|image| image.path.file_name())
            .map(|name| name.to_string_lossy().into_owned());
        let selection = self.session().vt.selected_text().unwrap_or_else(|e| {
            tracing::warn!("failed to read the selection: {e}");
            None
        });
        let mouse_tracking = self.session().vt.is_mouse_tracking().unwrap_or_else(|e| {
            tracing::warn!("failed to read the mouse mode: {e}");
            false
        });
        // 描き直すかを決める覚えの値（`App::scroll_position`）を、状態を読むだけで変えないよう、端末から直に読む。
        let position = self.session().vt.scroll_position().map_or_else(
            |e| {
                tracing::warn!("failed to read the scroll position: {e}");
                None
            },
            Some,
        );
        let thumb = position.and_then(|position| {
            scroll_bar::thumb_in(self.scroll_bar_area, self.scroll_bar_metrics(), position)
        });
        let layout = self.tab_bar_layout();
        let (new_tab, profile_list) = layout.new_tab_buttons();
        let tab_slots: Vec<serde_json::Value> = layout
            .tabs()
            .iter()
            .map(|slot| {
                json!({
                    "index": slot.index,
                    "area": area_json(slot.area),
                    "close": area_json(slot.close),
                })
            })
            .collect();
        json!({
            "selection": selection,
            "mouse_tracking": mouse_tracking,
            "scroll": position.map(|position| json!({
                "total": position.total,
                "offset": position.offset,
                "len": position.len,
                "lines_per_notch": self.wheel_lines_per_notch,
            })),
            "scroll_bar": {
                "x": self.scroll_bar_area.x,
                "y": self.scroll_bar_area.y,
                "width": self.scroll_bar_area.width,
                "height": self.scroll_bar_area.height,
                "thumb": thumb.map(|thumb| json!({ "top": thumb.top, "length": thumb.length })),
                "pressed": self.scroll_bar_press.is_some(),
            },
            "tab_bar": {
                "menu": area_json(layout.menu()),
                "new_tab": area_json(new_tab),
                "profiles": area_json(profile_list),
                "tabs": tab_slots,
            },
            "background": {
                "x": self.backdrop_area.x,
                "y": self.backdrop_area.y,
                "width": self.backdrop_area.width,
                "height": self.backdrop_area.height,
                "image": background_image,
            },
            "pid": std::process::id(),
            "cols": size.cols,
            "rows": size.rows,
            "client_width": client_width,
            "client_height": client_height,
            "terminal_x": self.terminal_area.x,
            "terminal_y": self.terminal_area.y,
            "cell_width": self.font.cell_width_px,
            "cell_height": self.font.cell_height_px,
            "active": self.active,
            "tabs": tabs,
            "profiles": profiles,
            "dialogs": dialogs,
            "images": images,
            "window": window_state::current(self.hwnd),
            // SAFETY: self.hwnd は有効なウィンドウ。
            "window_enabled": unsafe { IsWindowEnabled(self.hwnd) }.as_bool(),
            "window_title": real_window_title(self.hwnd),
            "version": version::display(),
            "text": text,
        })
    }

    /// いまの画面を PNG にして返す。
    fn e2e_frame(&mut self) -> Response {
        let Some(path) = self
            .e2e
            .as_ref()
            .map(|control| control.dir.join(FRAME_FILE))
        else {
            return Response::internal_error("the e2e control is not open");
        };
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Response::internal_error(format!("cannot replace the frame file: {e}"));
        }
        self.render_to(Some(&path));
        match std::fs::read(&path) {
            Ok(bytes) => Response::png(bytes),
            Err(e) => Response::internal_error(format!(
                "the frame was not written ({e}); see the log for the failure to render"
            )),
        }
    }
}
