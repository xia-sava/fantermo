//! ウィンドウプロシージャと、`App` の借用の決まり。メッセージごとの振り分け（`window_proc`）、`App` を借りる
//! `with_app`、借用の外ですること（`Deferred` と `run_deferred`）、借用が重なって飛ばしたメッセージの積み直し、
//! ウィンドウを閉じる処理。

use std::cell::RefCell;

use fantermo_core::{state::State, vt::KeyAction};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::ValidateRect,
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        Controls::WM_MOUSELEAVE,
        Input::KeyboardAndMouse::{ReleaseCapture, VK_F4, VK_PROCESSKEY, VK_SPACE},
        WindowsAndMessaging::{
            DefWindowProcW, DestroyWindow, GUI_INMENUMODE, GUI_POPUPMENUMODE, GUITHREADINFO,
            GWLP_USERDATA, GetGUIThreadInfo, GetLastActivePopup, GetWindowLongPtrW, HTCLIENT,
            IsIconic, KillTimer, PostQuitMessage, SIZE_MINIMIZED, SW_RESTORE, SWP_NOACTIVATE,
            SWP_NOZORDER, SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos,
            ShowWindow, WA_INACTIVE, WM_ACTIVATE, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE,
            WM_CONTEXTMENU, WM_COPYDATA, WM_DESTROY, WM_DPICHANGED, WM_ENABLE, WM_ENDSESSION,
            WM_EXITMENULOOP, WM_IME_COMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KEYUP,
            WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
            WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WM_QUERYENDSESSION, WM_RBUTTONDOWN,
            WM_RBUTTONUP, WM_SETCURSOR, WM_SETTINGCHANGE, WM_SIZE, WM_SYSCHAR, WM_SYSCOLORCHANGE,
            WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
        },
    },
};

use super::{
    App, BLINK_TIMER_ID, CONFIG_RELOAD_DELAY, CONFIG_TIMER_ID, DUMP_TIMER_ID,
    IMAGE_ANIMATION_TIMER_ID, LAUNCH_ARRIVED, OUTPUT_TIMER_ID, RENDERER_RETRY_TIMER_ID,
    SCROLL_BAR_TIMER_ID, WM_APP_CLIPBOARD_PROMPT, WM_APP_CONFIG_CHANGED, WM_APP_DEVICE_REMOVED,
    WM_APP_LAUNCH, WM_APP_UPDATE, WM_APP_WAKE, WindowData,
    keys::take_translated_chars,
    menu::{open_elevated, run_menu_command},
    mouse::{
        KEYBOARD_CONTEXT_MENU_POSITION, client_position, message_button, wheel_lines_per_notch,
    },
    post, post_or_warn,
    settings::{edit_settings, reload_settings},
    tabs::{Launch, start_output_timer},
    update::{continue_update, show_about},
    warn_post_failed, window_is_gone,
};
#[cfg(feature = "e2e")]
use super::{WM_APP_E2E, e2e};
use crate::{
    app_icon, clipboard_prompt,
    context_menu::{self, Chosen, MenuRequest},
    elevate,
    font::Font,
    instance,
    keyboard::KeyPosition,
    notice::{self, Topic},
    paste_editor,
    session::SessionId,
    window_state,
};

/// 入力の処理のあとで、ウィンドウの借用を手放してからすること。ダイアログやメニューは自分でメッセージを
/// 回すので、借用を持ったまま開くと、その間に届いたメッセージの処理が飛ばされる。
pub(super) enum Deferred {
    /// 改行を含む貼り付けの内容を、編集のダイアログで確認する。
    EditPaste {
        text: String,
        font: Font,
        /// 大きなテキストであることをダイアログに添える。
        large: bool,
    },
    /// 「fantermo について」のダイアログを開く。`font` は端末のフォント。
    ShowAbout { font: Font },
    /// 設定ダイアログを開く。
    EditSettings,
    /// コンテキストメニューか、タブバーのメニューのボタンのメニューを出す。
    Menu(MenuRequest),
    /// ウィンドウを閉じる。利用者が最後の端末を閉じたとき（キー、メニュー、タブバーの閉じるボタンと中ボタン）、
    /// メニューで終了を選んだとき、更新を適用するためにウィンドウを閉じるときに使う。
    CloseWindow,
    /// `topic` についての警告 `text` を見せる。
    Warn { topic: Topic, text: String },
    /// 更新のスレッドの結果を見せ、次にすることを決める。
    Update(crate::update::Event),
    /// クリップボードの読み取りの依頼を、本文 `text` のダイアログで確認する。
    AskClipboardRead { text: String },
}

/// ウィンドウに結び付けた App を借りて `action` を呼ぶもの。呼ばれるたびに App を読み直すので、ダイアログや
/// メッセージボックスの間にウィンドウが閉じたあとでも呼んでよい。ウィンドウがもうないときや、借りられない
/// ときは呼ばない。
pub(super) type WithApp<'a> = dyn Fn(&mut dyn FnMut(&mut App)) + 'a;

/// ウィンドウの借用を手放した状態で、入力の処理が残したことをする。その結果さらに残ったことも続けてする
/// （メニューで貼り付けを選び、それが編集のダイアログを要する場合など）。
pub(super) fn run_deferred(hwnd: HWND, deferred: Option<Deferred>, with_app: &WithApp) {
    let mut next = deferred;
    while let Some(deferred) = next.take() {
        match deferred {
            Deferred::EditPaste { text, font, large } => {
                if let Some(edited) = paste_editor::edit(hwnd, &text, &font, large) {
                    with_app(&mut |app| {
                        app.paste(&edited, true);
                    });
                }
            }
            Deferred::ShowAbout { font } => next = show_about(hwnd, &font, with_app),
            Deferred::EditSettings => edit_settings(hwnd, with_app),
            Deferred::Menu(request) => match context_menu::show(hwnd, &request) {
                Some(Chosen::Command(command)) => next = run_menu_command(command, with_app),
                Some(Chosen::Profile(index)) => {
                    with_app(&mut |app| app.open_session(Launch::Profile(index)))
                }
                Some(Chosen::ElevatedProfile(index)) => match request.profiles.get(index) {
                    Some(name) => next = open_elevated(hwnd, name, with_app),
                    None => tracing::warn!("no profile {index} in the menu"),
                },
                None => {}
            },
            // DestroyWindow はこの場で WM_DESTROY を送るので、借用を手放してから呼ぶ。
            Deferred::CloseWindow => destroy_window(hwnd, with_app),
            Deferred::Warn { topic, text } => notice::warn(hwnd, topic, text),
            Deferred::Update(event) => {
                next = continue_update(crate::update::prompt::on_event(hwnd, event), with_app);
            }
            Deferred::AskClipboardRead { text } => {
                let choice = clipboard_prompt::ask(hwnd, &text);
                with_app(&mut |app| app.finish_clipboard_prompt(choice));
            }
        }
    }
}

/// ウィンドウの位置・大きさ・最大化を状態ファイルに書いてから、ウィンドウを閉じる。
///
/// `with_app` が App を借りられたときだけ閉じる（状態ファイルを書くのも閉じるときだけ）。借りられないのは、
/// ウィンドウがもうないときと、外側の `window_proc` が App を借りている間に、その中の同期の呼び出し（ほかの
/// スレッドへの送信で止まる間など）でこのメッセージが処理されたとき。後者で閉じると、外側が借りている App を
/// `WM_NCDESTROY` で解放するので、閉じずに戻る。そのメッセージが `WM_CLOSE` なら、`with_app` が覚えて、借用を
/// 手放したところで積み直す（[`is_repostable`]）。
pub(super) fn destroy_window(hwnd: HWND, with_app: &WithApp) {
    let mut borrowed = false;
    with_app(&mut |_| borrowed = true);
    if !borrowed {
        tracing::debug!("not destroying the window while the App is unavailable");
        return;
    }
    save_window_state(hwnd);
    // SAFETY: hwnd は有効なウィンドウ。with_app が App を借りられたので、外側に借用はなく、自分の借用も
    // 手放している。WM_NCDESTROY がこの場で App を解放しても、借りているものはない。
    if let Err(e) = unsafe { DestroyWindow(hwnd) } {
        tracing::warn!("failed to destroy the window: {e}");
    }
}

/// ウィンドウのいまの位置・大きさ・最大化を状態ファイルに書く。状態ファイルの場所がなければ書かない。
///
/// App を借りないので、App を借りている間の同期の呼び出しの中で届いたメッセージ（ログオフの
/// `WM_QUERYENDSESSION`・`WM_ENDSESSION` など）からも書ける。場所は [`WindowData`] の `RefCell` の外から読み、
/// 書く中身（位置・大きさ・最大化）は Win32 から読む。App の中の値は使わない。借用が重なっているときは App を
/// 読むことも書くこともできないので、状態ファイルに App の中の値を足すなら、それも `RefCell` の外に置く。
/// `hwnd` は [`WindowData`] を結び付けたウィンドウか、0 を返すハンドルに限る（[`window_data_of`]）。
fn save_window_state(hwnd: HWND) {
    let Some(path) = window_data_of(hwnd).and_then(|data| data.state_path.as_deref()) else {
        return;
    };
    let Some(placement) = window_state::current(hwnd) else {
        return;
    };
    window_state::save(
        path,
        &State {
            window: Some(placement),
        },
    );
}

/// このスレッドがメニューを開いているか。
pub(super) fn in_menu() -> bool {
    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: info は cbSize を入れてあり、書き込める。
    let read = unsafe { GetGUIThreadInfo(GetCurrentThreadId(), &mut info) };
    if let Err(e) = read {
        tracing::warn!("failed to read the state of the window thread: {e}");
        return false;
    }
    info.flags.contains(GUI_INMENUMODE) || info.flags.contains(GUI_POPUPMENUMODE)
}

/// ウィンドウを前面に出す。最小化していれば戻す。ダイアログを開いていれば、そちらを前面に出す。
/// 起動したプロセスが前面に出す権利を渡していなければ、Windows がタスクバーで知らせるにとどまる。
fn bring_to_front(hwnd: HWND) {
    // SAFETY: hwnd は有効なウィンドウ。この場でメッセージが届くが、借用は持っていない。
    unsafe {
        if IsIconic(hwnd).as_bool() {
            _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if !SetForegroundWindow(GetLastActivePopup(hwnd)).as_bool() {
            tracing::debug!("the window could not come to the front");
        }
    }
}

thread_local! {
    /// 借用が重なって飛ばした知らせ（[`is_repostable`] のメッセージ）の、宛先のウィンドウとメッセージと
    /// `wparam`。
    static SKIPPED_MESSAGES: RefCell<Vec<(HWND, u32, WPARAM)>> = const { RefCell::new(Vec::new()) };
    /// [`SKIPPED_MESSAGES`] のうち、積み直せずに warn を残したもの。積み直せるまで、二度目からは debug にする。
    static WARNED_REPOSTS: RefCell<Vec<(HWND, u32, WPARAM)>> = const { RefCell::new(Vec::new()) };
}

/// 借用が重なって飛ばしたら、借用を手放したところで積み直すメッセージ。
///
/// - [`WM_APP_WAKE`]。飛ばした知らせは [`App::on_wake`] を通らないので、端末の印
///   （[`crate::session::WakeMessage`]）は立ったままで、その端末の知らせはほかに積まれない。読むためのスレッドは
///   上限に達すれば読むのも待つので、飛ばしたままにするとそのタブの出力が止まる。積み直すときは印を通さずに
///   そのまま積む（印は立ったままなので、積んである知らせは一つのまま）。
/// - [`WM_APP_DEVICE_REMOVED`]。一度しか届かないので、飛ばしたままにすると描くときまで作り直さない。
/// - `WM_CLOSE`。ほかのスレッドから送られると、借用の中の同期の呼び出し（`GetClipboardData` が相手のスレッドへ
///   送る間など）で届く。そこで閉じると借りている App を解放するので [`destroy_window`] は閉じずに戻り、
///   飛ばしたままにすると閉じる操作が失われる。
///
/// 積み直したメッセージは、借用の外でだけ処理される。積み直すのは借用を手放したところだけで、積んだメッセージを
/// 取り出すのは `GetMessageW`・`PeekMessageW` だけ。借用の中ではメッセージを回さない（ダイアログやメニューは
/// [`run_deferred`] で借用の外で開く）ので、借用の中の同期の呼び出しが処理するのは送られたメッセージだけで、
/// 積んだメッセージは取り出されない。同じ知らせは重ねて覚えない（[`remember_skipped_message`]）ので、借用が
/// 重なるたびに増えることもない。
fn is_repostable(msg: u32) -> bool {
    msg == WM_APP_WAKE || msg == WM_APP_DEVICE_REMOVED || msg == WM_CLOSE
}

/// 借用が重なってメッセージを飛ばしたことを覚える。同じ宛先の同じ知らせは重ねない。
fn remember_skipped_message(hwnd: HWND, msg: u32, wparam: WPARAM) {
    SKIPPED_MESSAGES.with_borrow_mut(|messages| {
        if !messages.contains(&(hwnd, msg, wparam)) {
            messages.push((hwnd, msg, wparam));
        }
    });
}

/// 借用が重なって飛ばしたメッセージを積み直す。借用を手放したところで呼ぶ。積めなかったものは残し、
/// [`OUTPUT_TIMER_ID`] のタイマーを動かす（タイマーの処理が借用を手放したところで、ここをもう一度通る）。
/// 積めなかったことは、同じメッセージを続けて積めない間は一度だけ warn に残す（タイマーの刻みごとに続けない）。
fn repost_skipped_messages() {
    let mut failed = Vec::new();
    for message @ (hwnd, msg, wparam) in SKIPPED_MESSAGES.take() {
        match post(hwnd, msg, wparam) {
            Ok(()) => forget_repost_warning(message),
            Err(e) if window_is_gone(&e) => {
                warn_post_failed(msg, &e);
                forget_repost_warning(message);
            }
            Err(e) => {
                if remember_repost_warning(message) {
                    warn_post_failed(msg, &e);
                } else {
                    tracing::debug!("failed to repost message {msg:#06x} again: {e}");
                }
                failed.push(message);
            }
        }
    }
    for (hwnd, msg, wparam) in failed {
        remember_skipped_message(hwnd, msg, wparam);
        start_output_timer(hwnd);
    }
}

/// `message` を積み直せなかったことを warn に残したと覚える。まだ残していなければ `true`。
fn remember_repost_warning(message: (HWND, u32, WPARAM)) -> bool {
    WARNED_REPOSTS.with_borrow_mut(|warned| {
        let first = !warned.contains(&message);
        if first {
            warned.push(message);
        }
        first
    })
}

/// `message` を積み直せたか、宛先のウィンドウが壊れた。次に積めなかったときは、また warn に残す。
fn forget_repost_warning(message: (HWND, u32, WPARAM)) {
    WARNED_REPOSTS.with_borrow_mut(|warned| warned.retain(|other| *other != message));
}

/// `hwnd` の `GWLP_USERDATA` に入っている値を、[`WindowData`] のポインタとして読む。参照は作らないので、
/// [`WindowData`] を結び付けていないウィンドウや、破棄したウィンドウ（0 が返る）のハンドルでもよい。
fn window_data_pointer(hwnd: HWND) -> *const WindowData {
    // SAFETY: 値を読むだけ。無効なハンドルには 0 が返る。
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowData }
}

/// ウィンドウに結び付けた [`WindowData`]。`'static` は形だけで、参照を使ってよいのは、この場でメッセージを
/// 回すもの（ダイアログ、メニュー、メッセージボックス、`DestroyWindow`）を呼ぶまで。その間に WM_NCDESTROY で
/// 解放されうるので、呼んだあとは読み直す。`hwnd` は [`WindowData`] を結び付けたウィンドウか、0 を返す
/// ハンドルに限る。
fn window_data_of(hwnd: HWND) -> Option<&'static WindowData> {
    let pointer = window_data_pointer(hwnd);
    // SAFETY: GWLP_USERDATA には run が入れた WindowData のポインタか 0 が入っている。0 でなければ、
    // WM_NCDESTROY が 0 に戻してから解放するので、まだ解放されていない。
    unsafe { pointer.as_ref() }
}

/// ウィンドウに結び付けた App。使ってよい間と `hwnd` の決まりは [`window_data_of`] と同じ。
pub(super) fn app_of(hwnd: HWND) -> Option<&'static RefCell<App>> {
    window_data_of(hwnd).map(|data| &data.app)
}

pub(super) extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // 入口で読んだ WindowData は、この場でメッセージを回すものを呼ぶと解放されうるので、ポインタとしてだけ持つ。
    let data = window_data_pointer(hwnd);
    if data.is_null() {
        // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    // 呼ばれるたびに GWLP_USERDATA を読み直す。処理の途中でメッセージを回し、その間にウィンドウが破棄されると
    // WindowData（と App）は解放され、同じハンドルの値がほかのウィンドウに使われることもある。入口で取った
    // ポインタがまだ結び付いているときだけ借りる。ポインタで比べてから参照にするのは、ほかのウィンドウが
    // GWLP_USERDATA に入れたポインタを WindowData の参照にしないため。
    // 処理の途中で別のメッセージがこの場で届くと、借用が重なる。そのときも処理を飛ばす。
    let with_app = |action: &mut dyn FnMut(&mut App)| {
        if !std::ptr::eq(window_data_pointer(hwnd), data) {
            tracing::debug!("the window is gone; skipped message {msg:#06x}");
            return;
        }
        // SAFETY: GWLP_USERDATA にまだ run が入れたポインタが入っているので、WM_NCDESTROY で解放されていない。
        // DestroyWindow を呼ぶのは destroy_window だけで、そこは with_app が App を借りられたとき（外側に借用が
        // ないとき）に、借用を手放してから呼ぶ。WM_CLOSE も destroy_window を通る。ほかのスレッドは
        // このウィンドウを DestroyWindow できない。なので action の間も解放されない。
        let data = unsafe { &*data };
        match data.app.try_borrow_mut() {
            Ok(mut borrowed) => {
                action(&mut borrowed);
                let release_capture = std::mem::take(&mut borrowed.release_capture_pending);
                drop(borrowed);
                if release_capture {
                    // SAFETY: このスレッドがキャプチャしたマウスを放す。キャプチャしていなければ何もしない。
                    // この場で届く WM_CAPTURECHANGED は、借用を手放したあとなので処理される。
                    if let Err(e) = unsafe { ReleaseCapture() } {
                        tracing::warn!("failed to release the mouse capture: {e}");
                    }
                }
                repost_skipped_messages();
            }
            Err(_) => {
                tracing::debug!("skipped reentrant message {msg:#06x}");
                if is_repostable(msg) {
                    remember_skipped_message(hwnd, msg, wparam);
                }
            }
        }
    };

    match msg {
        WM_APP_WAKE => {
            let mut close = false;
            let id = SessionId(wparam.0);
            with_app(&mut |app| close = app.on_wake(id));
            if close {
                // DestroyWindow はこの場で WM_DESTROY を送るので、借用を手放してから呼ぶ。
                destroy_window(hwnd, &with_app);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            // SAFETY: hwnd は有効なウィンドウ。描くのは Direct3D なので、更新領域を空にするだけ。
            unsafe { _ = ValidateRect(Some(hwnd), None) };
            with_app(&mut |app| app.render());
            LRESULT(0)
        }
        // 最小化の大きさ 0 を伝えると、端末が 1×1 になり、シェルや TUI のアプリへ届く。
        WM_SIZE if wparam.0 as u32 == SIZE_MINIMIZED => LRESULT(0),
        WM_SIZE => {
            let width = (lparam.0 & 0xffff) as u32;
            let height = ((lparam.0 >> 16) & 0xffff) as u32;
            with_app(&mut |app| app.on_resize(width, height));
            LRESULT(0)
        }
        WM_TIMER => {
            match wparam.0 {
                DUMP_TIMER_ID => with_app(&mut |app| app.write_dump()),
                BLINK_TIMER_ID => with_app(&mut |app| app.on_blink_timer()),
                SCROLL_BAR_TIMER_ID => with_app(&mut |app| app.on_scroll_bar_timer()),
                IMAGE_ANIMATION_TIMER_ID => with_app(&mut |app| app.on_image_animation_timer()),
                RENDERER_RETRY_TIMER_ID => with_app(&mut |app| app.on_renderer_retry_timer()),
                OUTPUT_TIMER_ID => with_app(&mut |app| app.on_output_timer()),
                CONFIG_TIMER_ID => {
                    // SAFETY: hwnd は有効なウィンドウ。
                    if let Err(e) = unsafe { KillTimer(Some(hwnd), CONFIG_TIMER_ID) } {
                        tracing::warn!("failed to stop the settings timer: {e}");
                    }
                    reload_settings(hwnd, &with_app);
                }
                // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
                _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
            }
            LRESULT(0)
        }
        WM_APP_UPDATE => {
            let mut event = None;
            with_app(&mut |app| event = app.update_events.try_recv().ok());
            // 「fantermo について」を開いている間は、そのループが先に受け取って上に見せる（`show_about`）。
            // ここに届くのは開いていないときで、ウィンドウの上に見せる。メッセージボックスを開くので、借用を
            // 手放してから見せる。
            if let Some(event) = event {
                run_deferred(hwnd, Some(Deferred::Update(event)), &with_app);
            }
            LRESULT(0)
        }
        #[cfg(feature = "e2e")]
        WM_APP_E2E => {
            e2e::on_wake(hwnd, &with_app);
            LRESULT(0)
        }
        WM_APP_CLIPBOARD_PROMPT => {
            let mut deferred = None;
            with_app(&mut |app| deferred = app.next_clipboard_prompt());
            run_deferred(hwnd, deferred, &with_app);
            LRESULT(0)
        }
        // ダイアログを閉じてウィンドウが有効に戻ったら、待っていた確認を始める。この場ではダイアログの片づけの
        // 途中なので、積んであとで見る。
        WM_ENABLE => {
            if wparam.0 != 0 {
                post_or_warn(hwnd, WM_APP_CLIPBOARD_PROMPT, WPARAM(0));
                post_or_warn(hwnd, WM_APP_LAUNCH, WPARAM(0));
            }
            LRESULT(0)
        }
        // 別のプロセスからの起動の指定。受け取ったかどうかを答えるだけで、開くのは積んだメッセージで。受け取れなければ、
        // 送った側が新しいウィンドウを開く。
        WM_COPYDATA => {
            // SAFETY: lparam は WM_COPYDATA で受け取ったもので、戻る前に呼ぶ。
            let mut forwarded = unsafe { instance::received(lparam) };
            let mut accepted = false;
            with_app(&mut |app| {
                if let Some(forwarded) = forwarded.take() {
                    accepted = app.accept_launch(forwarded);
                }
            });
            if !accepted {
                return LRESULT(0);
            }
            post_or_warn(hwnd, WM_APP_LAUNCH, WPARAM(LAUNCH_ARRIVED));
            LRESULT(instance::ACCEPTED)
        }
        WM_APP_LAUNCH => {
            if wparam.0 == LAUNCH_ARRIVED {
                bring_to_front(hwnd);
            }
            let mut deferred = None;
            with_app(&mut |app| deferred = app.open_pending_launches());
            run_deferred(hwnd, deferred, &with_app);
            LRESULT(0)
        }
        // メニューを閉じたら、待っていた確認と起動を見直す。
        WM_EXITMENULOOP => {
            post_or_warn(hwnd, WM_APP_CLIPBOARD_PROMPT, WPARAM(0));
            post_or_warn(hwnd, WM_APP_LAUNCH, WPARAM(0));
            LRESULT(0)
        }
        // 知らせが続けて届くたびにタイマーを掛け直し、止んでから一度だけ読み直す。
        WM_APP_CONFIG_CHANGED => {
            // SAFETY: hwnd は有効なウィンドウ。同じ番号のタイマーは掛け直しになる。
            if unsafe {
                SetTimer(
                    Some(hwnd),
                    CONFIG_TIMER_ID,
                    CONFIG_RELOAD_DELAY.as_millis() as u32,
                    None,
                )
            } == 0
            {
                tracing::warn!(
                    "failed to schedule reloading the settings: {}",
                    windows::core::Error::from_thread()
                );
            }
            LRESULT(0)
        }
        WM_APP_DEVICE_REMOVED => {
            with_app(&mut |app| app.on_device_removed_message());
            LRESULT(0)
        }
        // ほかのウィンドウにマウスのキャプチャが移ったら（Alt+Tab など）、離したことが届かないので、
        // スクロールバーの操作と選択のドラッグを終える（選択範囲は残す）。lparam はマウスをキャプチャするウィンドウ。
        // 自分で放したとき（lparam は 0）も届くが、そのときはどちらもボタンを離した処理で終えている。
        WM_CAPTURECHANGED if lparam.0 != hwnd.0 as isize => {
            with_app(&mut |app| {
                app.end_scroll_bar_press();
                app.session_mut().selecting = false;
            });
            LRESULT(0)
        }
        // 下位の語が WA_INACTIVE なら、アクティブでなくなった。
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xffff) as u32 != WA_INACTIVE;
            with_app(&mut |app| app.on_activate(active));
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す（フォーカスをウィンドウに移してもらう）。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let virtual_key = wparam.0 as u16;
            // IME が処理中のキーと、Alt を含む F4・Space（閉じる・システムメニュー）は Windows に任せる。
            let left_to_windows = virtual_key == VK_PROCESSKEY.0
                || (msg == WM_SYSKEYDOWN && (virtual_key == VK_F4.0 || virtual_key == VK_SPACE.0));
            if left_to_windows {
                // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            let position = KeyPosition::from_lparam(lparam.0);
            let Some(chars) = take_translated_chars(hwnd) else {
                // デッドキー。次のキーと合わせた文字が WM_CHAR で届く。
                return LRESULT(0);
            };
            let action = if position.repeated {
                KeyAction::Repeat
            } else {
                KeyAction::Press
            };
            let mut deferred = None;
            with_app(&mut |app| deferred = app.on_key(virtual_key, position, action, &chars));
            run_deferred(hwnd, deferred, &with_app);
            LRESULT(0)
        }
        WM_KEYUP | WM_SYSKEYUP => {
            let virtual_key = wparam.0 as u16;
            if virtual_key == VK_PROCESSKEY.0 {
                // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            let position = KeyPosition::from_lparam(lparam.0);
            with_app(&mut |app| {
                app.on_key(virtual_key, position, KeyAction::Release, &[]);
            });
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_MBUTTONDOWN | WM_RBUTTONDOWN => {
            let button = message_button(msg);
            with_app(&mut |app| app.on_button_down(button, lparam, wparam));
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_MBUTTONUP | WM_RBUTTONUP => {
            let button = message_button(msg);
            let mut deferred = None;
            with_app(&mut |app| deferred = app.on_button_up(button, lparam, wparam));
            run_deferred(hwnd, deferred, &with_app);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            with_app(&mut |app| app.on_mouse_move(lparam, wparam));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_app(&mut |app| app.on_mouse_leave());
            LRESULT(0)
        }
        // タブバーはシステムの色で描くので、描き直す。
        WM_SYSCOLORCHANGE => {
            with_app(&mut |app| app.invalidate());
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // Ctrl を離さないままウィンドウを離れると、離したことが届かないので、リンクの下線を消しておく。
        WM_KILLFOCUS => {
            with_app(&mut |app| app.refresh_hovered_link(false));
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // リンクの上では手の形にする。既定の処理は、マウスが動くたびにウィンドウのクラスの矢印に戻す。
        WM_SETCURSOR if (lparam.0 & 0xffff) as u32 == HTCLIENT => {
            let mut on_link = false;
            with_app(&mut |app| {
                on_link = app.session().hovered_link.is_some();
                if on_link {
                    app.set_cursor_shape();
                }
            });
            if on_link {
                return LRESULT(1);
            }
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // メニューキーと Shift+F10 は WM_KEYDOWN で扱い、既定の処理に渡さないので、ここへは来ない。
        // 来るのは外から送られたときで、位置が (-1, -1) ならキーボードからとしてカーソルの位置に出す。
        // 位置のあるもの（タイトルバーの右クリックなど）は、システムメニューを出す既定の処理に任せる。
        WM_CONTEXTMENU if client_position(lparam) == KEYBOARD_CONTEXT_MENU_POSITION => {
            let mut deferred = None;
            with_app(&mut |app| deferred = Some(app.context_menu_at_cursor()));
            run_deferred(hwnd, deferred, &with_app);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = i32::from(((wparam.0 >> 16) & 0xffff) as u16 as i16);
            with_app(&mut |app| app.on_wheel(delta, lparam, wparam));
            LRESULT(0)
        }
        WM_CHAR | WM_SYSCHAR => {
            with_app(&mut |app| app.on_text_unit(wparam.0 as u16));
            LRESULT(0)
        }
        WM_IME_STARTCOMPOSITION | WM_IME_COMPOSITION => {
            with_app(&mut |app| app.place_ime_windows());
            // 変換中の文字列は IME に描かせ、確定した文字列は既定の処理で WM_CHAR にしてもらう。
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // 変わった後のウィンドウの大きさ（WM_GETDPISCALEDSIZE）は既定の処理に任せ、Windows が DPI の比で
        // 拡大・縮小した大きさを推奨の矩形として受け取る。
        WM_DPICHANGED => {
            // フォントとアトラスを作り直してから、推奨された位置と大きさに移す。移すとこの場で
            // WM_SIZE が届き、新しいセルの寸法で桁数と行数を計算し直す。
            let dpi = (wparam.0 & 0xffff) as u32;
            with_app(&mut |app| app.apply_dpi(dpi));
            // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
            match unsafe { GetModuleHandleW(None) } {
                Ok(instance) => app_icon::set(
                    hwnd,
                    instance.into(),
                    dpi,
                    elevate::this_process() == Some(true),
                ),
                Err(e) => tracing::warn!("failed to get the module handle: {e}"),
            }
            // SAFETY: WM_DPICHANGED の lparam は推奨された矩形を指す。
            let suggested = unsafe { &*(lparam.0 as *const RECT) };
            // SAFETY: hwnd は有効なウィンドウ。WM_SIZE がこの場で届くが、借用は持っていない。
            if let Err(e) = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            } {
                tracing::warn!("failed to apply the DPI change: {e}");
            }
            // クライアント領域の大きさが変わらず WM_SIZE が届かなかったときも、セルの寸法を合わせる。
            with_app(&mut |app| app.fit_to_client());
            LRESULT(0)
        }
        // 設定が変わったら、次のホイールから Windows の設定の行数で動かす。読み直しは軽く、
        // レジストリを書いて wparam 0 で知らせる道具の変更も拾えるので、変わった設定の種類は見ない。
        WM_SETTINGCHANGE => {
            with_app(&mut |app| app.wheel_lines_per_notch = wheel_lines_per_notch());
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // × と Alt+F4。メニューの終了と、最後の端末が終わったときと同じく、状態を書いてから閉じる。
        // ほかのスレッドから送られた WM_CLOSE は、App を借りている間の同期の呼び出しの中でも届く。そのときは
        // 閉じずに、借用を手放したところで積み直す（destroy_window）。
        WM_CLOSE => {
            destroy_window(hwnd, &with_app);
            LRESULT(0)
        }
        // ログオフ・シャットダウン・再起動。ウィンドウは閉じられないまま、WM_ENDSESSION から戻ったあとで
        // プロセスごと終わらされるので、閉じるときと同じく状態を書く。問われた時点でも書いておく
        // （ENDSESSION_CRITICAL では WM_ENDSESSION のあとに時間がほとんどないため）。書くのは一時
        // ファイルからの置き換えなので、二度書いても、取りやめになっても害はない。終わるのを止めない。
        // ウィンドウは壊さず、端末と子プロセスはほかのプロセスと一緒に終わらされるのに任せる。
        // App を借りている間の同期の呼び出しの中で届いても書く（save_window_state）。
        WM_QUERYENDSESSION => {
            tracing::info!("the session may end (flags {:#x})", lparam.0);
            save_window_state(hwnd);
            LRESULT(1)
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                tracing::info!("the session is ending (flags {:#x})", lparam.0);
                save_window_state(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: hwnd は有効なウィンドウ。
            unsafe {
                _ = KillTimer(Some(hwnd), DUMP_TIMER_ID);
                _ = KillTimer(Some(hwnd), CONFIG_TIMER_ID);
                _ = KillTimer(Some(hwnd), BLINK_TIMER_ID);
                _ = KillTimer(Some(hwnd), SCROLL_BAR_TIMER_ID);
                _ = KillTimer(Some(hwnd), IMAGE_ANIMATION_TIMER_ID);
                _ = KillTimer(Some(hwnd), RENDERER_RETRY_TIMER_ID);
                _ = KillTimer(Some(hwnd), OUTPUT_TIMER_ID);
            };
            let mut code = 0;
            with_app(&mut |app| {
                code = app.exit.map_or(0, |code| code.0);
                for forwarded in app.pending_launches.drain(..) {
                    forwarded.start_in_new_window();
                }
            });
            // SAFETY: このスレッドのメッセージループに終了を伝えるだけ。
            unsafe { PostQuitMessage(code as i32) };
            LRESULT(0)
        }
        WM_NCDESTROY => {
            // SAFETY: hwnd は有効なウィンドウで、ポインタを外すのはここだけ。
            let pointer = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut WindowData;
            // SAFETY: pointer は run が Box::into_raw で作ったもの。入れ子のループの外側の window_proc が
            // ポインタを持っていることはあるが、with_app は GWLP_USERDATA（いま 0 にした）と比べてから参照にする
            // ので、これより後には使わない。window_data_of の参照は、メッセージを回すものを呼ぶまでしか使わない。
            // DestroyWindow は destroy_window が借用のないことを確認してから呼ぶので、借用も残っていない。
            drop(unsafe { Box::from_raw(pointer) });
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => {
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}
