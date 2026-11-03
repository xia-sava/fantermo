//! マウス。ボタンと動きとホイール、選択、リンク、アプリへの報告。

use std::time::Duration;

use fantermo_core::{
    keymap,
    link::Link,
    vt::{Mods, MouseAction, MouseButton, MouseInput},
};
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    System::SystemServices::{MK_CONTROL, MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MK_SHIFT},
    UI::{
        Input::KeyboardAndMouse::{
            GetDoubleClickTime, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
        },
        WindowsAndMessaging::{
            IDC_ARROW, IDC_HAND, LoadCursorW, SPI_GETWHEELSCROLLLINES,
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetCursor, SystemParametersInfoW, WHEEL_DELTA,
            WM_MBUTTONDOWN, WM_MBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP,
        },
    },
};

use super::{App, Deferred};
use crate::{keyboard, link_opener, tab_bar::Hit};

/// キーボードから開いた WM_CONTEXTMENU が `lparam` に入れてくる位置。
pub(super) const KEYBOARD_CONTEXT_MENU_POSITION: (i32, i32) = (-1, -1);

impl App {
    /// ウィンドウのクライアント領域の中の位置にあるリンク。
    fn link_at(&self, x: i32, y: i32) -> Option<Link> {
        let (col, row) = self.cell_at(x, y)?;
        self.link_in_cell(col, row)
    }

    /// セルにあるリンク。
    fn link_in_cell(&self, col: u16, row: u16) -> Option<Link> {
        self.session().vt.link_at(col, row).unwrap_or_else(|e| {
            tracing::warn!("failed to look for a link: {e}");
            None
        })
    }

    /// マウスを載せているリンクを探し直す。`modifier_held` はリンクを開く修飾キー（Ctrl）を押している
    /// か。変わったら、下線を引き直し、カーソルの形を変える。前回と同じセルの上なら探し直さない。
    pub(super) fn refresh_hovered_link(&mut self, modifier_held: bool) {
        let cell = match self.pointer_in_window {
            Some((x, y)) if modifier_held && !self.session().selecting => self.cell_at(x, y),
            _ => None,
        };
        if cell.is_some() && cell == self.session().hovered_cell {
            return;
        }
        self.session_mut().hovered_cell = cell;
        let link = cell.and_then(|(col, row)| self.link_in_cell(col, row));
        if link == self.session().hovered_link {
            return;
        }
        match &link {
            Some(link) => {
                tracing::debug!("link under the pointer: {:?} {:?}", link.uri, link.spans)
            }
            None => tracing::debug!("no link under the pointer"),
        }
        let session = self.session_mut();
        session.vt.set_link_underline(
            link.as_ref()
                .map_or_else(Vec::new, |link| link.spans.clone()),
        );
        session.hovered_link = link;
        self.invalidate();
        if self.pointer_in_window.is_some() {
            self.set_cursor_shape();
        }
    }

    /// 修飾キーをいま押しているかで、マウスを載せているリンクを探し直す。画面の中身や表示している
    /// 範囲が変わったときに使う。
    pub(super) fn refresh_hovered_link_now(&mut self) {
        self.session_mut().hovered_cell = None;
        if self.pointer_in_window.is_none() && self.session().hovered_link.is_none() {
            return;
        }
        self.refresh_hovered_link(is_link_modifier(keyboard::current_mods()));
    }

    /// マウスのカーソルを、リンクの上なら手の形、そうでなければ矢印にする。
    pub(super) fn set_cursor_shape(&self) {
        let shape = if self.session().hovered_link.is_some() {
            IDC_HAND
        } else {
            IDC_ARROW
        };
        // SAFETY: システムのカーソルを読むだけ。
        match unsafe { LoadCursorW(None, shape) } {
            // SAFETY: 読んだシステムのカーソルを設定する。
            Ok(cursor) => unsafe { _ = SetCursor(Some(cursor)) },
            Err(e) => tracing::warn!("failed to load the mouse cursor: {e}"),
        }
    }

    /// マウスがウィンドウのクライアント領域の中の (`x`, `y`) にある。ウィンドウの外へ出たことが届くように
    /// する。
    fn track_pointer(&mut self, x: i32, y: i32) {
        if self.pointer_in_window.is_none() {
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: track は初期化済みで、self.hwnd はこのスレッドの有効なウィンドウ。
            if let Err(e) = unsafe { TrackMouseEvent(&mut track) } {
                tracing::warn!("failed to track the mouse leaving the window: {e}");
            }
        }
        self.pointer_in_window = Some((x, y));
    }

    /// マウスがウィンドウのクライアント領域の外へ出た。
    pub(super) fn on_mouse_leave(&mut self) {
        self.pointer_in_window = None;
        self.refresh_hovered_link(false);
        self.update_tab_bar_hover();
        self.update_scroll_bar_hover();
    }

    /// マウスのボタンを押した。Ctrl を押しながらリンクの上で左ボタンを押したら、離すまで何もしない
    /// （アプリへも送らず、選択も始めない）。そうでなければ、アプリがマウスを求めていればアプリへ送り、
    /// 求めていなければ左ボタンで選択を始める。Shift を押していれば、アプリが求めていても選択にする。
    pub(super) fn on_button_down(&mut self, button: MouseButton, lparam: LPARAM, wparam: WPARAM) {
        // ウィンドウの外までドラッグしても離したことが届くように、マウスをキャプチャする。
        // SAFETY: self.hwnd はこのスレッドの有効なウィンドウ。
        unsafe { SetCapture(self.hwnd) };
        let (x, y) = client_position(lparam);
        if let Some(hit) = self.tab_bar_hit(x, y) {
            self.tab_bar_press = Some((button, hit));
            if button == MouseButton::Left {
                // タブは押したときに表示する。ボタンは離したときに働くので、押している色で描き直す。
                match hit {
                    Some(Hit::Tab(index)) => self.activate(index),
                    Some(_) => self.invalidate(),
                    None => {}
                }
            }
            return;
        }
        // スクロールバーの上は、アプリへ送らず、選択も始めない。
        if self.scroll_bar_area.contains(x, y) {
            let drag = if button == MouseButton::Left {
                self.press_scroll_bar(y)
            } else {
                None
            };
            self.scroll_bar_press = Some((button, drag));
            self.invalidate();
            return;
        }
        if button == MouseButton::Left
            && is_link_modifier(mouse_mods(wparam))
            && let Some(link) = self.link_at(x, y)
        {
            self.session_mut().pressed_link = Some(link.uri);
            return;
        }
        if self.report_mouse(MouseAction::Press, Some(button), x, y, wparam) {
            return;
        }
        if button != MouseButton::Left {
            return;
        }
        let at = self.pointer(x, y);
        // SAFETY: 引数のない問い合わせ。
        let repeat_interval = Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }));
        let elapsed = self.clock.elapsed();
        let session = self.session_mut();
        if let Err(e) = session.vt.selection_press(at, elapsed, repeat_interval) {
            tracing::warn!("failed to start a selection: {e}");
        }
        session.selecting = true;
        self.invalidate();
    }

    pub(super) fn on_mouse_move(&mut self, lparam: LPARAM, wparam: WPARAM) {
        let (x, y) = client_position(lparam);
        self.track_pointer(x, y);
        self.update_tab_bar_hover();
        self.update_scroll_bar_hover();
        self.refresh_hovered_link(is_link_modifier(mouse_mods(wparam)));
        if self.scroll_bar_press.is_some() {
            self.drag_scroll_bar(y);
            return;
        }
        // リンクやタブバーを押している間の動きと、タブバーとスクロールバーの上の動きは、アプリへ
        // 送らない。
        if self.session().pressed_link.is_some() || self.tab_bar_press.is_some() {
            return;
        }
        if !self.session().selecting {
            if self.tab_bar_hit(x, y).is_some() || self.scroll_bar_area.contains(x, y) {
                return;
            }
            self.report_mouse(MouseAction::Motion, held_button(wparam), x, y, wparam);
            return;
        }
        let at = self.pointer(x, y);
        // Alt を押しながらなら矩形で選ぶ。
        let rectangle = keyboard::current_mods().contains(Mods::ALT);
        if let Err(e) = self.session_mut().vt.selection_drag(at, rectangle) {
            tracing::warn!("failed to extend the selection: {e}");
        }
        self.invalidate();
    }

    /// マウスのボタンを離した。リンクを押していて、同じリンクの上で離したら開く。アプリへ送らなかった
    /// 右ボタンなら、コンテキストメニューを出すよう返す。
    pub(super) fn on_button_up(
        &mut self,
        button: MouseButton,
        lparam: LPARAM,
        wparam: WPARAM,
    ) -> Option<Deferred> {
        let (x, y) = client_position(lparam);
        let mut deferred = None;
        if let Some((pressed, _)) = self.scroll_bar_press {
            // スクロールバーの上で押したボタンを離すまでは、ほかのボタンも端末へ送らない。
            if pressed == button {
                self.end_scroll_bar_press();
            }
        } else if let Some((pressed, hit)) = self.tab_bar_press
            && pressed == button
        {
            self.tab_bar_press = None;
            deferred = self.on_tab_bar_click(button, hit, x, y);
        } else if button == MouseButton::Left
            && let Some(uri) = self.session_mut().pressed_link.take()
        {
            if self.link_at(x, y).is_some_and(|link| link.uri == uri) {
                link_opener::open(&uri);
            }
        } else if button == MouseButton::Left && self.session().selecting {
            let at = self.pointer(x, y);
            let session = self.session_mut();
            session.selecting = false;
            if let Err(e) = session.vt.selection_release(at) {
                tracing::warn!("failed to finish the selection: {e}");
            }
            self.invalidate();
        } else if !self.report_mouse(MouseAction::Release, Some(button), x, y, wparam)
            && button == MouseButton::Right
        {
            deferred = Some(self.context_menu(x, y));
        }
        if held_button(wparam).is_none() {
            self.release_capture_pending = true;
        }
        deferred
    }

    /// マウスの操作をアプリへ送るか。アプリが求めていて、Shift を押していないとき。
    fn reports_mouse(&self) -> bool {
        if keyboard::current_mods().contains(Mods::SHIFT) {
            return false;
        }
        self.session().vt.is_mouse_tracking().unwrap_or_else(|e| {
            tracing::warn!("failed to read the mouse mode: {e}");
            false
        })
    }

    /// アプリがマウスを求めていて Shift を押していなければ、操作をアプリへ送って `true` を返す。
    /// `x` と `y` はウィンドウのクライアント領域の中の位置（ピクセル）。
    fn report_mouse(
        &mut self,
        action: MouseAction,
        button: Option<MouseButton>,
        x: i32,
        y: i32,
        wparam: WPARAM,
    ) -> bool {
        if !self.reports_mouse() {
            return false;
        }
        let mods = keyboard::current_mods();
        let (x, y) = self.pointer_in_terminal(x, y);
        let input = MouseInput {
            action,
            button,
            mods,
            x: x as f32,
            y: y as f32,
            any_button_pressed: held_button(wparam).is_some(),
        };
        match self.session_mut().vt.encode_mouse(&input) {
            Ok(encoded) => self.send(&encoded),
            Err(e) => tracing::warn!("failed to encode the mouse: {e}"),
        }
        true
    }

    /// マウスのホイールが回った。アプリがマウスを求めていれば、1 ノッチごとにボタン 4（奥）か
    /// 5（手前）の押下として送り、解放は送らない（xterm と同じ）。アプリがマウスを求めていない代替画面
    /// ならカーソルキーに変えて送る。そうでなければスクロールバックを動かす。
    /// `delta` は WM_MOUSEWHEEL の回転量（奥へ回すと正）、`lparam` は WM_MOUSEWHEEL のとおり画面での位置。
    pub(super) fn on_wheel(&mut self, delta: i32, lparam: LPARAM, wparam: WPARAM) {
        let (x, y) = self.screen_to_client(lparam);
        // タブバーの上では何もしない。
        if self.tab_bar_hit(x, y).is_some() {
            return;
        }
        if !self.reports_mouse() {
            let keys = self.session().vt.wheel_sends_cursor_keys();
            match keys {
                Ok(true) => self.send_wheel_as_cursor_keys(delta),
                Ok(false) => self.scroll_by_wheel(delta),
                Err(e) => tracing::warn!("failed to read the terminal modes: {e}"),
            }
            return;
        }
        let button = if delta > 0 {
            MouseButton::Four
        } else {
            MouseButton::Five
        };
        let (notches, remainder) = wheel_lines(self.session().wheel_report_remainder, delta, 1);
        self.session_mut().wheel_report_remainder = remainder;
        for _ in 0..notches.unsigned_abs() {
            self.report_mouse(MouseAction::Press, Some(button), x, y, wparam);
        }
    }

    /// ホイールの回転量 `delta` を動かす行数にする。端数は次の回転へ持ち越す。
    fn take_wheel_lines(&mut self, delta: i32) -> i32 {
        let (lines, remainder) = wheel_lines(
            self.session().wheel_remainder,
            delta,
            self.wheel_lines_per_notch,
        );
        self.session_mut().wheel_remainder = remainder;
        lines
    }

    /// 奥へ回すと上、手前へ回すと下のカーソルキーを、動かす行数だけ送る。
    fn send_wheel_as_cursor_keys(&mut self, delta: i32) {
        let lines = self.take_wheel_lines(delta);
        if lines == 0 {
            return;
        }
        match self.session_mut().vt.encode_wheel_as_cursor_keys(lines) {
            Ok(encoded) => self.send(&encoded),
            Err(e) => tracing::warn!("failed to encode the wheel as cursor keys: {e}"),
        }
    }

    fn scroll_by_wheel(&mut self, delta: i32) {
        let lines = self.take_wheel_lines(delta);
        if lines == 0 {
            return;
        }
        // 奥へ回すと過去（上）へ。
        self.session_mut().vt.scroll(-(lines as isize));
        self.on_viewport_moved();
    }
}

/// マウスのメッセージの `lparam` の x と y（符号付きの 16 ビット）。
pub(super) fn client_position(lparam: LPARAM) -> (i32, i32) {
    let x = i32::from((lparam.0 & 0xffff) as u16 as i16);
    let y = i32::from(((lparam.0 >> 16) & 0xffff) as u16 as i16);
    (x, y)
}

/// マウスのメッセージの `wparam` で押されているボタンのうち、一つ（左、中、右の順）。
fn held_button(wparam: WPARAM) -> Option<MouseButton> {
    let held = wparam.0 as u32;
    [
        (MK_LBUTTON, MouseButton::Left),
        (MK_MBUTTON, MouseButton::Middle),
        (MK_RBUTTON, MouseButton::Right),
    ]
    .into_iter()
    .find(|(flag, _)| held & flag.0 != 0)
    .map(|(_, button)| button)
}

/// ボタンを押した・離したメッセージの、そのボタン。
pub(super) fn message_button(msg: u32) -> MouseButton {
    match msg {
        WM_MBUTTONDOWN | WM_MBUTTONUP => MouseButton::Middle,
        WM_RBUTTONDOWN | WM_RBUTTONUP => MouseButton::Right,
        _ => MouseButton::Left,
    }
}

/// リンクを開く修飾キー。Ctrl（ほかの修飾キーなし）。Ctrl+Alt は配列によって AltGr になるので含めない。
pub(super) fn is_link_modifier(mods: Mods) -> bool {
    keymap::held_mods(mods) == Mods::CTRL
}

/// マウスのメッセージのときに押されている修飾キー。Ctrl と Shift は `wparam` から、そのほかは
/// いまのキーボードの状態から取る。
fn mouse_mods(wparam: WPARAM) -> Mods {
    let held = wparam.0 as u32;
    let mut mods = keyboard::current_mods() & !(Mods::CTRL | Mods::SHIFT);
    if held & MK_CONTROL.0 != 0 {
        mods |= Mods::CTRL;
    }
    if held & MK_SHIFT.0 != 0 {
        mods |= Mods::SHIFT;
    }
    mods
}

/// 前回の端数 `remainder` に回転量 `delta` を足し、動かす行数と新しい端数を返す。
/// 高精度のホイールは 1 ノッチ（`WHEEL_DELTA`）より細かい回転量で届くので、端数を持ち越す。
fn wheel_lines(remainder: i32, delta: i32, lines_per_notch: u32) -> (i32, i32) {
    let total = remainder + delta * lines_per_notch as i32;
    let lines = total / WHEEL_DELTA as i32;
    (lines, total - lines * WHEEL_DELTA as i32)
}

/// ホイール 1 ノッチで動かす行数の、Windows の設定。読めなければ既定の 3 行。
pub(super) fn wheel_lines_per_notch() -> u32 {
    const DEFAULT_LINES: u32 = 3;
    let mut lines = DEFAULT_LINES;
    // SAFETY: lines は u32 を書き込める領域を指す。
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some((&raw mut lines).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    if let Err(e) = read {
        tracing::warn!("failed to read the wheel scroll lines: {e}");
        return DEFAULT_LINES;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTCH: i32 = WHEEL_DELTA as i32;

    #[test]
    fn mouse_positions_keep_their_sign() {
        // ウィンドウの外（左上）へドラッグすると、負の位置で届く。
        let lparam = LPARAM((((-5i16 as u16) as isize) << 16) | ((-3i16 as u16) as isize));
        assert_eq!(client_position(lparam), (-3, -5));
    }

    #[test]
    fn held_buttons_are_read_from_wparam() {
        assert_eq!(held_button(WPARAM(0)), None);
        assert_eq!(
            held_button(WPARAM(MK_RBUTTON.0 as usize)),
            Some(MouseButton::Right)
        );
        assert_eq!(
            held_button(WPARAM((MK_LBUTTON.0 | MK_RBUTTON.0) as usize)),
            Some(MouseButton::Left)
        );
        assert_eq!(message_button(WM_MBUTTONUP), MouseButton::Middle);
    }

    #[test]
    fn keyboard_context_menu_position_is_read_from_lparam() {
        // MAKELPARAM(-1, -1) は 64 ビットでは 0xffffffff（正の値）で届く。
        assert_eq!(
            client_position(LPARAM(0xffff_ffff)),
            KEYBOARD_CONTEXT_MENU_POSITION
        );
        assert_eq!(client_position(LPARAM(-1)), KEYBOARD_CONTEXT_MENU_POSITION);
    }

    #[test]
    fn links_open_with_ctrl_alone() {
        assert!(is_link_modifier(Mods::CTRL));
        assert!(is_link_modifier(Mods::CTRL | Mods::CAPS_LOCK));
        assert!(!is_link_modifier(Mods::empty()));
        assert!(!is_link_modifier(Mods::CTRL | Mods::SHIFT));
        assert!(!is_link_modifier(Mods::CTRL | Mods::ALT));
    }

    #[test]
    fn one_notch_moves_the_configured_lines() {
        assert_eq!(wheel_lines(0, NOTCH, 3), (3, 0));
        assert_eq!(wheel_lines(0, -NOTCH, 3), (-3, 0));
    }

    #[test]
    fn fine_wheel_steps_carry_the_remainder() {
        // 1/4 ノッチずつ届くと、3 行の設定では 1 回目は端数だけが残る。
        let (lines, remainder) = wheel_lines(0, NOTCH / 4, 3);
        assert_eq!(lines, 0);
        let (lines, remainder) = wheel_lines(remainder, NOTCH / 4, 3);
        assert_eq!(lines, 1);
        let (lines, _) = wheel_lines(remainder, NOTCH / 2, 3);
        assert_eq!(lines, 2);
    }
}
