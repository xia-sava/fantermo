//! e2e テストの制御の口のうち、ウィンドウのスレッドで動く側（feature `e2e` のときだけコンパイルする）。
//!
//! 受ける側は `e2e_server`。答えを返してから借用の外ですること（ウィンドウを閉じる、メニューを開くなど）は
//! `Deferred` と同じく [`run_deferred`] に渡す。ウィンドウの大きさの変更はこの場で `WM_SIZE` が届くので、借用の
//! 外でしてから答える。ウィンドウへ送るメッセージも、この場でウィンドウプロシージャが動くので同じ。

use serde_json::json;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, POINT, RECT, TRUE, WPARAM},
        Graphics::Gdi::ClientToScreen,
        System::{
            SystemServices::{
                MK_CONTROL, MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MK_SHIFT, MODIFIERKEYS_FLAGS,
            },
            Threading::GetCurrentThreadId,
        },
        UI::{
            Controls::TDM_CLICK_BUTTON,
            Input::KeyboardAndMouse::{
                GetKeyboardState, SetKeyboardState, VK_CONTROL, VK_MENU, VK_SHIFT,
            },
            WindowsAndMessaging::{
                ENDSESSION_LOGOFF, EndMenu, EnumChildWindows, EnumThreadWindows, GetClassNameW,
                GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsWindow, IsWindowVisible,
                PostMessageW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SendMessageW, SetWindowPos,
                WM_CAPTURECHANGED, WM_COMMAND, WM_ENDSESSION, WM_LBUTTONDOWN, WM_LBUTTONUP,
                WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUERYENDSESSION,
                WM_RBUTTONDOWN, WM_RBUTTONUP,
            },
        },
    },
    core::BOOL,
};

use super::{
    App, Deferred, Launch, SIZE, TabCommand, WithApp, client_size, client_size_for_terminal,
    destroy_window, grid_pixels, run_deferred, tab_title,
};
use crate::{
    clipboard,
    e2e_server::{MouseAction, MouseHeld, MouseStep, Pending, Request, Response, WindowMessage},
    paint::Area,
    scroll_bar, version, window_state,
};
use fantermo_core::vt::KeyAction;

/// 絵を書くファイル。
const FRAME_FILE: &str = "frame.png";

/// `TaskDialogIndirect` とメッセージボックスのウィンドウのクラス。
const SYSTEM_DIALOG_CLASS: &str = "#32770";

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
        Request::CloseMenu => {
            // SAFETY: このスレッドが開いているメニューを閉じる。
            answer(match unsafe { EndMenu() } {
                Ok(()) => Response::json(&json!({})),
                Err(e) => Response::bad_request(format!("no menu is open: {e}")),
            });
        }
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
            | Request::CloseMenu => (
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
    /// 描いているタブだけ。
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
