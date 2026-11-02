//! e2e テストの制御の口のうち、ウィンドウのスレッドで動く側（feature `e2e` のときだけコンパイルする）。
//!
//! 受ける側は `e2e_server`。答えを返してから借用の外ですること（ウィンドウを閉じる、メニューを開くなど）は
//! `Deferred` と同じく [`run_deferred`] に渡す。ウィンドウの大きさの変更はこの場で `WM_SIZE` が届くので、借用の
//! 外でしてから答える。

use serde_json::json;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, RECT, TRUE, WPARAM},
        System::Threading::GetCurrentThreadId,
        UI::{
            Controls::TDM_CLICK_BUTTON,
            WindowsAndMessaging::{
                EnumChildWindows, EnumThreadWindows, GetClassNameW, GetWindowRect,
                GetWindowTextLengthW, GetWindowTextW, IsWindow, IsWindowVisible, PostMessageW,
                SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetWindowPos, WM_COMMAND,
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
    e2e_server::{Pending, Request, Response},
    version,
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
            Request::Quit | Request::ResizeWindow { .. } => (
                Response::internal_error("handled outside of the borrow"),
                None,
            ),
        }
    }

    /// いまの状態。`text` は表示している端末の、スクロールバックを含む全体の文字。`images` は表示している端末の
    /// 画面に出ている画像の数。`dialogs` はこのウィンドウの上に開いているダイアログ。`controls` はその
    /// コントロールの文字で、`TaskDialogIndirect` の本文は入らない。
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
        json!({
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
