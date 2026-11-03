//! 「fantermo について」と、更新の確認と適用の始め方・結果の見せ方。

use std::cell::Cell;

use windows::Win32::{
    Foundation::{HWND, WPARAM},
    UI::WindowsAndMessaging::MSG,
};

use super::{
    App, WM_APP_UPDATE, poster,
    proc::{Deferred, WithApp},
};
use crate::{about, font::Font, update};

impl App {
    /// 更新の確認かダウンロードを別のスレッドで始める。前のものが終わっていなければ始めない。始めたら真を
    /// 返す。
    fn start_update(&mut self, work: update::Work) -> bool {
        if self.update_busy {
            tracing::info!("an update check or download is already running");
            return false;
        }
        self.update_busy = update::start(
            work,
            self.update_sender.clone(),
            poster(self.hwnd, WM_APP_UPDATE, WPARAM(0)),
        );
        self.update_busy
    }
}

/// 「fantermo について」を開き、閉じるまで待つ。「更新を確認」を押したら、確認する仕事を始め、届いた結果を
/// 「fantermo について」の上に見せる。更新に進むと決めたら「fantermo について」を閉じ、続けてすることを返す。
pub(super) fn show_about(hwnd: HWND, font: &Font, with_app: &WithApp) -> Option<Deferred> {
    let mut busy = false;
    with_app(&mut |app| busy = app.update_busy);
    let start_check = || {
        let mut started = false;
        with_app(&mut |app| started = app.start_update(update::Work::Check));
        started
    };
    // 更新に進むと決めたときの次のこと。「fantermo について」を閉じてから続ける。
    let after = Cell::new(None);
    let on_owner_message = |dialog: HWND, msg: &MSG| {
        if msg.message != WM_APP_UPDATE {
            return about::Received::Other;
        }
        let mut event = None;
        with_app(&mut |app| event = app.update_events.try_recv().ok());
        let Some(event) = event else {
            return about::Received::Other;
        };
        let next = update::prompt::on_event(dialog, event);
        if next.continues() {
            after.set(Some(next));
            about::Received::Close
        } else {
            with_app(&mut |app| app.update_busy = false);
            about::Received::Shown
        }
    };
    about::show(
        hwnd,
        font,
        &about::UpdateHooks {
            busy,
            start_check: &start_check,
            on_owner_message: &on_owner_message,
        },
    );
    after
        .take()
        .and_then(|next| continue_update(next, with_app))
}

/// 更新の結果を見せたあとの `next` をする。続けてすることがあれば返す。
pub(super) fn continue_update(next: update::prompt::Next, with_app: &WithApp) -> Option<Deferred> {
    match next {
        update::prompt::Next::Nothing => {
            with_app(&mut |app| app.update_busy = false);
            None
        }
        update::prompt::Next::Download(release) => {
            let mut work = Some(update::Work::Download(release));
            with_app(&mut |app| {
                app.update_busy = false;
                if let Some(work) = work.take() {
                    app.start_update(work);
                }
            });
            None
        }
        // ウィンドウを閉じるので、新しく始めないよう `update_busy` は立てたままにする。
        update::prompt::Next::Quit => Some(Deferred::CloseWindow),
    }
}
