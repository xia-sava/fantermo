//! e2e テストの制御の口のうち、ウィンドウのスレッドで動く側（feature `e2e` のときだけコンパイルする）。
//!
//! 受ける側は `e2e_server`。答えを返してから借用の外ですること（ウィンドウを閉じる、メニューを開くなど）は
//! `Deferred` と同じく [`run_deferred`] に渡す。ウィンドウの大きさの変更はこの場で `WM_SIZE` が届くので、借用の
//! 外でしてから答える。

use serde_json::json;
use windows::Win32::{
    Foundation::{HWND, RECT},
    UI::WindowsAndMessaging::{
        GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsWindow, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOZORDER, SetWindowPos,
    },
};

use super::{
    App, Deferred, SIZE, TabCommand, WithApp, client_size, client_size_for_terminal,
    destroy_window, grid_pixels, run_deferred, tab_title,
};
use crate::{
    e2e_server::{Pending, Request, Response},
    version,
};
use fantermo_core::vt::KeyAction;

/// 絵を書くファイル。
const FRAME_FILE: &str = "frame.png";

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

/// ウィンドウに実際に設定されているタイトル。
fn real_window_title(hwnd: HWND) -> String {
    // SAFETY: hwnd は有効なウィンドウ。既定の処理が答えるので、借用には触らない。
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    let mut buffer = vec![0u16; length as usize + 1];
    // SAFETY: buffer は書き込める。
    let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..copied as usize])
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
            Request::CloseTab => (done(), self.run_tab_command(TabCommand::Close)),
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

    /// いまの状態。`text` は表示している端末の、スクロールバックを含む全体の文字。
    fn e2e_state(&self) -> serde_json::Value {
        let size = self.session().vt.size();
        let text = self.session().vt.plain_text().unwrap_or_else(|e| {
            tracing::warn!("failed to format the screen: {e}");
            String::new()
        });
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
                })
            })
            .collect();
        json!({
            "cols": size.cols,
            "rows": size.rows,
            "client_width": client_width,
            "client_height": client_height,
            "active": self.active,
            "tabs": tabs,
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
