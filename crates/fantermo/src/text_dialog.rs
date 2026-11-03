//! 複数行の入力欄とボタンを並べたダイアログ。入力欄の上に、文の行とリンクの行を並べられる。
//!
//! 開いている間は呼び出し元のウィンドウを無効にし、メッセージはこのダイアログのループで回す。IDOK の
//! ボタンを押せば入力欄の文字列を返し、IDCANCEL のボタンや Esc なら何も返さない。ほかのボタンを押せば、その
//! ボタンを返して閉じる（`Dialog::on_button` が閉じないと答えたものを除く）。リンクを押せば既定のアプリで
//! 開く。
//!
//! 寸法とフォントは、開くときに呼び出し元のウィンドウのモニターの DPI で決め、開いている間に DPI が変われば
//! （`WM_DPICHANGED`）その DPI で作り直す。

use std::{
    cell::{Cell, RefCell},
    sync::OnceLock,
};

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM},
        Graphics::Gdi::{GetTextExtentPoint32W, HFONT, LOGFONTW},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{
                EM_GETRECT, EM_SETLIMITTEXT, ICC_LINK_CLASS, NM_CLICK, NM_RETURN, NMHDR, WC_LINK,
            },
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_CONTROL, VK_MENU},
            WindowsAndMessaging::{
                BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
                ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, ES_WANTRETURN,
                GetClientRect, HMENU, IDCANCEL, IDOK, MSG, MoveWindow, SW_SHOW, SWP_NOZORDER,
                SendMessageW, SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_EX_STYLE,
                WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_DPICHANGED, WM_KEYDOWN,
                WM_NOTIFY, WM_SIZE, WS_CAPTION, WS_CHILD, WS_DISABLED, WS_EX_CLIENTEDGE,
                WS_EX_CONTROLPARENT, WS_EX_DLGMODALFRAME, WS_HSCROLL, WS_POPUP, WS_SYSMENU,
                WS_TABSTOP, WS_THICKFRAME, WS_VISIBLE, WS_VSCROLL,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{
    dialog::{self, DialogState, OwnedFont, scale},
    font::Font,
    link_opener, modal,
};

const CLASS: PCWSTR = w!("fantermo-text-dialog");

/// 入力欄のコントロール ID。閉じるボタンは IDOK と IDCANCEL を使う。
const EDIT_ID: i32 = 100;
/// 入力欄の上の行のコントロール ID の始まり。上から順に 1 ずつ増やす。
const HEADER_ID: i32 = 200;

/// `EM_SETLIMITTEXT` で上限を外す値。
const NO_TEXT_LIMIT: usize = 0;
/// Ctrl+A の A の仮想キー。
const VK_A: usize = b'A' as usize;

/// 96 DPI のときの寸法（ピクセル）。ウィンドウの DPI に合わせて拡大する。
const MARGIN: i32 = 8;
const BUTTON_WIDTH: i32 = 112;
const BUTTON_HEIGHT: i32 = 30;
/// 呼び出し元のウィンドウに対する、ダイアログの大きさの割合。
const SIZE_RATIO: f32 = 0.7;
/// 端末の文字に対する、入力欄の文字の大きさの割合。
const FONT_RATIO: f32 = 0.75;

/// ダイアログの下に右から並べるボタン。
pub struct Button {
    /// 押したときのコントロール ID。押せばダイアログを閉じる（`Dialog::on_button` が閉じないと答えたものを
    /// 除く）。
    pub id: i32,
    pub label: &'static str,
    /// Enter で押されるボタンとして描く。
    pub default: bool,
    /// 押せる。偽なら灰色で見せる。
    pub enabled: bool,
}

/// 入力欄の上に並べる 1 行。
pub enum HeaderLine<'a> {
    /// 文。
    Text(&'a str),
    /// リンク。URL をそのまま見せ、押せば既定のアプリで開く。
    Link(&'a str),
}

/// 開くダイアログの中身。
pub struct Dialog<'a> {
    pub title: &'a str,
    /// 入力欄の上に、上から順に並べる行。Windows のメッセージのフォントで描く。
    pub header: &'a [HeaderLine<'a>],
    /// 入力欄に入れる文字列。改行は `\n` で渡す。
    pub text: &'a str,
    /// 入力欄を読み取り専用にし、右端で折り返して見せる。選んでコピーはできる。
    pub read_only: bool,
    /// 入力欄に少なくともこの桁数が収まる幅で開く（モニターに収まる範囲で）。
    pub min_columns: Option<u32>,
    /// 左から順に並べるボタン。
    pub buttons: &'a [Button],
    /// IDOK と IDCANCEL のほかのボタンを押したときに、ダイアログとボタンのコントロール ID を渡して呼ぶ。真を
    /// 返したら、ダイアログを閉じない。
    pub on_button: Option<&'a dyn Fn(HWND, i32) -> bool>,
    /// 呼び出し元のウィンドウ宛てに積まれたメッセージを、ほかに渡す前にダイアログとあわせて渡して呼ぶ。真を
    /// 返したら、そのメッセージはほかに渡さない。中でダイアログを閉じてもよい（[`close`]）。
    pub on_owner_message: Option<&'a OwnerMessageHook<'a>>,
}

/// [`Dialog::on_owner_message`] で呼ぶもの。
pub type OwnerMessageHook<'a> = dyn Fn(HWND, &MSG) -> bool + 'a;

/// ダイアログを閉じたわけ。
#[derive(Debug, PartialEq, Eq)]
pub enum Closed {
    /// IDOK のボタンを押した。入力欄の文字列を添える。改行は `\n`。
    Ok(String),
    /// IDOK と IDCANCEL のほかのボタンを押した。そのコントロール ID を添える。
    Button(i32),
    /// IDCANCEL のボタン、Esc、ウィンドウの × で閉じた。開けなかったときも。
    Cancel,
}

/// `dialog` を開き、閉じるまで待つ。`font` は端末のフォントで、入力欄はその書体を少し小さくして使う。
pub fn run(owner: HWND, dialog: &Dialog, font: &Font) -> Closed {
    if let Err(e) = register_class() {
        tracing::warn!("failed to register the text dialog: {e}");
        return Closed::Cancel;
    }
    let state = TextDialogState {
        on_button: dialog.on_button,
        ..TextDialogState::default()
    };
    let hwnd = match create(owner, dialog, &state, font) {
        Ok(hwnd) => hwnd,
        Err(e) => {
            tracing::warn!("failed to open the text dialog {:?}: {e}", dialog.title);
            return Closed::Cancel;
        }
    };
    // SAFETY: 入力欄はこのダイアログの子で、文字列はこの呼び出しの間は生きている。
    unsafe { _ = SetWindowTextW(state.edit.get(), &HSTRING::from(to_crlf(dialog.text))) };

    run_modal(owner, hwnd, &state, dialog.on_owner_message);

    // フォントは state と一緒に、ダイアログが壊れたあとで消える。
    state.result.take().unwrap_or(Closed::Cancel)
}

/// 開いているダイアログ `dialog` を閉じる。閉じたわけは [`Closed::Cancel`] になる。
pub fn close(dialog: HWND) {
    dialog::destroy(dialog);
}

/// ダイアログの状態。ウィンドウプロシージャからも使うので、共有の参照だけで読み書きする。
#[derive(Default)]
struct TextDialogState<'a> {
    /// [`Dialog::on_button`]。
    on_button: Option<&'a dyn Fn(HWND, i32) -> bool>,
    edit: Cell<HWND>,
    /// 入力欄の上の行。上から順に並べる。
    header: RefCell<Vec<HWND>>,
    /// リンクの行と、その URL。
    links: RefCell<Vec<(HWND, String)>>,
    /// 入力欄の上の行の 1 行の高さ（ピクセル）。
    header_line_height: Cell<i32>,
    /// ボタン。左から順に並べる。
    buttons: RefCell<Vec<HWND>>,
    /// 入力欄とボタンのフォント。
    font: RefCell<OwnedFont>,
    /// 開いたときの入力欄のフォントと、そのときの DPI。DPI が変わったら、これを拡大・縮小して作り直す。
    opened_font: Cell<(LOGFONTW, u32)>,
    /// 入力欄の上の行のフォント。行がなければ作らない。
    header_font: RefCell<OwnedFont>,
    /// 寸法とフォントを決める DPI。開いたときはモニターの DPI で、開いている間に DPI が変われば
    /// （`WM_DPICHANGED`）その DPI にする。
    dpi: Cell<u32>,
    /// 閉じたわけ。ボタンで閉じるまでは `None`。
    result: RefCell<Option<Closed>>,
    /// ボタンのコントロール ID。入力欄などのほかの子からの WM_COMMAND と見分ける。
    button_ids: RefCell<Vec<i32>>,
    closed: Cell<bool>,
    /// Enter で押されるボタンのコントロール ID。
    default_id: Cell<Option<i32>>,
}

impl DialogState for TextDialogState<'_> {
    fn closed(&self) -> &Cell<bool> {
        &self.closed
    }
}

fn register_class() -> windows::core::Result<()> {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    dialog::register_class(&REGISTERED, CLASS, Some(dialog_proc), Some(ICC_LINK_CLASS))
}

/// 呼び出し元のウィンドウの真ん中に、その大きさの 7 割でダイアログを作る。`min_columns` があれば、その
/// 桁数が収まるまで幅を広げる。
fn create(
    owner: HWND,
    dialog: &Dialog,
    state: &TextDialogState<'_>,
    font: &Font,
) -> windows::core::Result<HWND> {
    // SAFETY: owner は呼び出し元の有効なウィンドウ。
    let dpi = unsafe { GetDpiForWindow(owner) };
    state.dpi.set(dpi);

    let owner_rect = dialog::window_rect(owner)?;
    let width = ((owner_rect.right - owner_rect.left) as f32 * SIZE_RATIO) as i32;
    let height = ((owner_rect.bottom - owner_rect.top) as f32 * SIZE_RATIO) as i32;
    let (left, top) = dialog::center_in(&owner_rect, width, height);

    // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
    let instance = unsafe { GetModuleHandleW(None)? };
    // SAFETY: クラスは登録済みで、文字列はこの呼び出しの間は生きている。
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            CLASS,
            &HSTRING::from(dialog.title),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            width,
            height,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        )?
    };
    let unfinished = dialog::Unfinished::new(hwnd);

    let child = |class: PCWSTR, text: &str, style: WINDOW_STYLE, ex_style, id: i32| {
        // SAFETY: hwnd は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
        unsafe {
            CreateWindowExW(
                ex_style,
                class,
                &HSTRING::from(text),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(id as isize as *mut _)),
                Some(instance.into()),
                None,
            )
        }
    };
    for (index, line) in dialog.header.iter().enumerate() {
        let id = HEADER_ID + index as i32;
        let control = match line {
            HeaderLine::Text(text) => {
                child(w!("STATIC"), text, WINDOW_STYLE(0), WINDOW_EX_STYLE(0), id)?
            }
            HeaderLine::Link(url) => {
                let control = child(
                    WC_LINK,
                    &format!("<a href=\"{url}\">{url}</a>"),
                    WS_TABSTOP,
                    WINDOW_EX_STYLE(0),
                    id,
                )?;
                state.links.borrow_mut().push((control, (*url).to_owned()));
                control
            }
        };
        state.header.borrow_mut().push(control);
    }
    // 読み取り専用のときは、右端で折り返して見せ、Enter は既定のボタンに渡す。
    let edit_style = if dialog.read_only {
        WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY) as u32)
    } else {
        WS_TABSTOP
            | WS_VSCROLL
            | WS_HSCROLL
            | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_WANTRETURN) as u32)
    };
    state.edit.set(child(
        w!("EDIT"),
        "",
        edit_style,
        WS_EX_CLIENTEDGE,
        EDIT_ID,
    )?);
    for button in dialog.buttons {
        let mut style = WS_TABSTOP
            | WINDOW_STYLE(if button.default {
                BS_DEFPUSHBUTTON
            } else {
                BS_PUSHBUTTON
            } as u32);
        if !button.enabled {
            style |= WS_DISABLED;
        }
        let control = child(
            w!("BUTTON"),
            button.label,
            style,
            WINDOW_EX_STYLE(0),
            button.id,
        )?;
        state.buttons.borrow_mut().push(control);
        state.button_ids.borrow_mut().push(button.id);
        if button.default {
            state.default_id.set(Some(button.id));
        }
    }

    // 入力欄は端末と同じ書体にして、少し小さくする。
    let mut logfont = font.logfont();
    logfont.lfHeight = (logfont.lfHeight as f32 * FONT_RATIO) as i32;
    state.opened_font.set((logfont, dpi));
    state.font.replace(OwnedFont::create(&logfont));
    set_control_font(state);
    if !dialog.header.is_empty() {
        create_header_font(state, dpi);
    }
    // SAFETY: 入力欄は有効なウィンドウ。
    unsafe {
        SendMessageW(
            state.edit.get(),
            EM_SETLIMITTEXT,
            Some(WPARAM(NO_TEXT_LIMIT)),
            None,
        )
    };

    // 状態を結び付けてから大きさを決め、WM_SIZE で子を並べさせる。
    // SAFETY: state は run の中にあり、run は run_modal でダイアログが壊れる（WM_DESTROY で外す）まで待つ。
    // この先で失敗すれば、unfinished が戻る前にダイアログを壊す。
    unsafe { dialog::attach_state(hwnd, state) };
    // SAFETY: hwnd は作ったばかりのウィンドウ。
    unsafe { SetWindowPos(hwnd, None, left, top, width, height, SWP_NOZORDER)? };
    layout(hwnd, state);
    if let Some(columns) = dialog.min_columns {
        widen_for_columns(owner, &owner_rect, hwnd, state, columns);
    }
    // SAFETY: hwnd と入力欄は有効なウィンドウ。
    unsafe {
        _ = ShowWindow(hwnd, SW_SHOW);
        _ = SetFocus(Some(state.edit.get()));
    }
    Ok(unfinished.finish())
}

/// DPI を `dpi` にし、入力欄とボタンのフォントと、入力欄の上の行のフォントを作り直す。コントロールを
/// 並べ直すのは呼び出し元。
fn set_dpi(state: &TextDialogState, dpi: u32) {
    state.dpi.set(dpi);
    let (mut logfont, opened_dpi) = state.opened_font.get();
    logfont.lfHeight = font_height_at(logfont.lfHeight, opened_dpi, dpi);
    let previous = state.font.replace(OwnedFont::create(&logfont));
    set_control_font(state);
    // 前のフォントは、どのコントロールももう使っていない。
    drop(previous);
    if !state.header.borrow().is_empty() {
        create_header_font(state, dpi);
    }
}

/// DPI が `from` のときのフォントの高さ `height`（`LOGFONTW` の `lfHeight`）を、DPI が `to` のときの高さにする。
fn font_height_at(height: i32, from: u32, to: u32) -> i32 {
    if from == 0 {
        return height;
    }
    (height as f32 * to as f32 / from as f32).round() as i32
}

/// 入力欄とボタンに、入力欄のフォントを付ける。
fn set_control_font(state: &TextDialogState) {
    let font = state.font.borrow().handle();
    let controls = std::iter::once(state.edit.get()).chain(state.buttons.borrow().clone());
    for control in controls {
        dialog::set_font(control, font, true);
    }
}

/// 入力欄の上の行に、DPI が `dpi` のときの Windows のメッセージのフォントを付け、1 行の高さを測る。
/// フォントの設定を読めなければ、いまのフォントのままにする。
fn create_header_font(state: &TextDialogState, dpi: u32) {
    let Some(font) = OwnedFont::message(dpi) else {
        return;
    };
    let previous = state.header_font.replace(font);
    let font = state.header_font.borrow().handle();
    let header = state.header.borrow();
    for &control in header.iter() {
        dialog::set_font(control, font, true);
    }
    // 前のフォントは、どの行ももう使っていない。
    drop(previous);
    let Some(&first) = header.first() else {
        return;
    };
    match line_height(first, font) {
        Some(height) => state.header_line_height.set(height),
        None => tracing::warn!("failed to measure the message font"),
    }
}

/// `control` に `font` で文字を 1 行描いたときの高さ（ピクセル）。測れなければ `None`。
fn line_height(control: HWND, font: HFONT) -> Option<i32> {
    let sample: Vec<u16> = "Ag版".encode_utf16().collect();
    let mut size = SIZE::default();
    // SAFETY: dc は with_font_dc が font を選んだ DC で、size は書き込める。
    let measured = dialog::with_font_dc(control, font, |dc| unsafe {
        GetTextExtentPoint32W(dc, &sample, &mut size).as_bool()
    })?;
    measured.then_some(size.cy)
}

/// 入力欄の文字を並べる幅に `columns` 桁が収まらなければ、足りない分だけダイアログを広げ、
/// 呼び出し元のウィンドウ `owner`（場所は `owner_rect`）の真ん中に置き直す。モニターからははみ出させない。
fn widen_for_columns(
    owner: HWND,
    owner_rect: &RECT,
    dialog: HWND,
    state: &TextDialogState,
    columns: u32,
) {
    let Some(text_width) = text_width(state, columns) else {
        tracing::warn!("failed to measure the text dialog font");
        return;
    };
    let mut format = RECT::default();
    // SAFETY: 入力欄は有効なウィンドウで、format は書き込める。
    unsafe {
        SendMessageW(
            state.edit.get(),
            EM_GETRECT,
            None,
            Some(LPARAM(std::ptr::from_mut(&mut format) as isize)),
        )
    };
    let shortfall = text_width - (format.right - format.left);
    if shortfall <= 0 {
        return;
    }
    let window = match dialog::window_rect(dialog) {
        Ok(window) => window,
        Err(e) => {
            tracing::warn!("failed to read the text dialog position: {e}");
            return;
        }
    };
    let width = window.right - window.left + shortfall;
    let left = owner_rect.left + (owner_rect.right - owner_rect.left - width) / 2;
    let (left, width) = fit_horizontally(owner, left, width);
    let height = window.bottom - window.top;
    // SAFETY: dialog は有効なウィンドウ。WM_SIZE で子を並べ直す。
    let moved =
        unsafe { SetWindowPos(dialog, None, left, window.top, width, height, SWP_NOZORDER) };
    if let Err(e) = moved {
        tracing::warn!("failed to widen the text dialog: {e}");
    }
}

/// 入力欄のフォントで `columns` 桁を並べた幅（ピクセル）。測れなければ `None`。
fn text_width(state: &TextDialogState, columns: u32) -> Option<i32> {
    // 平均の文字幅は実際の送り幅より小さいことがあるので、その桁数の文字列の幅を測る。
    let line: Vec<u16> = std::iter::repeat_n(u16::from(b'0'), columns as usize).collect();
    let mut size = SIZE::default();
    let font = state.font.borrow().handle();
    // SAFETY: dc は with_font_dc が font を選んだ DC で、size は書き込める。
    let measured = dialog::with_font_dc(state.edit.get(), font, |dc| unsafe {
        GetTextExtentPoint32W(dc, &line, &mut size).as_bool()
    })?;
    measured.then_some(size.cx)
}

/// 横の位置 `left` と幅 `width` を、呼び出し元のウィンドウがあるモニターの作業領域に収める。幅が作業領域より
/// 広ければ、作業領域の幅に縮める。
fn fit_horizontally(owner: HWND, left: i32, width: i32) -> (i32, i32) {
    let Some(work) = dialog::work_area(owner) else {
        tracing::warn!("failed to read the monitor of the window");
        return (left, width);
    };
    let width = width.min(work.right - work.left);
    (
        dialog::keep_within(left, width, work.left, work.right),
        width,
    )
}

/// 呼び出し元のウィンドウを無効にし、ダイアログが閉じるまでメッセージを回す。入力欄での Ctrl+A は、すべてを
/// 選ぶ。呼び出し元のウィンドウ宛てのメッセージは、先に `on_owner_message` に渡す。
fn run_modal(
    owner: HWND,
    dialog: HWND,
    state: &TextDialogState<'_>,
    on_owner_message: Option<&OwnerMessageHook<'_>>,
) {
    modal::run(owner, dialog, &state.closed, |msg| {
        if msg.hwnd == owner
            && let Some(on_owner_message) = on_owner_message
        {
            return on_owner_message(dialog, msg);
        }
        if !is_select_all(msg, state.edit.get()) {
            return false;
        }
        dialog::select_all(state.edit.get());
        true
    });
}

/// `msg` が入力欄 `edit` での Ctrl+A（Alt なし）か。複数行の入力欄は Ctrl+A で
/// すべてを選ばないので、ループで受け持つ。
fn is_select_all(msg: &MSG, edit: HWND) -> bool {
    if msg.hwnd != edit || msg.message != WM_KEYDOWN || msg.wParam.0 != VK_A {
        return false;
    }
    // SAFETY: このスレッドのキーの状態を読むだけ。
    let pressed = |key| unsafe { GetKeyState(i32::from(key)) } < 0;
    pressed(VK_CONTROL.0) && !pressed(VK_MENU.0)
}

/// 入力欄の上の行を上から並べ、入力欄を広く取り、ボタンを右下に並べる。
fn layout(dialog: HWND, state: &TextDialogState<'_>) {
    let mut client = RECT::default();
    // SAFETY: dialog は有効なウィンドウで、client は書き込める。
    if unsafe { GetClientRect(dialog, &mut client) }.is_err() {
        return;
    }
    let px = |value: i32| scale(value, state.dpi.get());
    let (margin, button_w, button_h) = (px(MARGIN), px(BUTTON_WIDTH), px(BUTTON_HEIGHT));
    let button_top = client.bottom - margin - button_h;
    let line_height = state.header_line_height.get();
    let mut top = margin;
    for &line in state.header.borrow().iter() {
        // SAFETY: line は dialog の子の有効なウィンドウ。
        unsafe {
            _ = MoveWindow(
                line,
                margin,
                top,
                client.right - 2 * margin,
                line_height,
                true,
            )
        };
        top += line_height;
    }
    if !state.header.borrow().is_empty() {
        top += margin;
    }
    // SAFETY: 入力欄は dialog の子の有効なウィンドウ。
    unsafe {
        _ = MoveWindow(
            state.edit.get(),
            margin,
            top,
            client.right - 2 * margin,
            button_top - margin - top,
            true,
        );
    }
    let mut button_left = client.right;
    for &button in state.buttons.borrow().iter().rev() {
        button_left -= margin + button_w;
        // SAFETY: button は dialog の子の有効なウィンドウ。
        unsafe { _ = MoveWindow(button, button_left, button_top, button_w, button_h, true) };
    }
}

extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: create が結び付けたのは TextDialogState。
    let Some(state) = (unsafe { dialog::state_of::<TextDialogState>(hwnd) }) else {
        // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    if let Some(result) = dialog::handle_common_message(msg, wparam, lparam, state.default_id.get())
    {
        return result;
    }
    match msg {
        WM_SIZE => {
            layout(hwnd, state);
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            let closed = if id == IDOK.0 {
                Some(Closed::Ok(from_crlf(&dialog::get_text(state.edit.get()))))
            } else if id == IDCANCEL.0 {
                Some(Closed::Cancel)
            } else if state.button_ids.borrow().contains(&id) {
                let keep_open = state.on_button.is_some_and(|on_button| on_button(hwnd, id));
                (!keep_open).then_some(Closed::Button(id))
            } else {
                None
            };
            if let Some(closed) = closed {
                state.result.replace(Some(closed));
                dialog::destroy(hwnd);
            }
            LRESULT(0)
        }
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY の lparam は NMHDR で始まる構造体を指す。
            let header = unsafe { &*(lparam.0 as *const NMHDR) };
            if header.code == NM_CLICK || header.code == NM_RETURN {
                let links = state.links.borrow();
                if let Some((_, url)) = links.iter().find(|(link, _)| *link == header.hwndFrom) {
                    link_opener::open(url);
                }
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            set_dpi(state, dialog::dpi_of(wparam));
            // SAFETY: lparam は WM_DPICHANGED で受け取ったもの。
            unsafe { dialog::move_to_suggested_rect(hwnd, lparam) };
            // 大きさが変わると WM_SIZE でも並べ直すが、変わらなければ届かないので、ここでも並べる。
            layout(hwnd, state);
            LRESULT(0)
        }
        WM_CLOSE => {
            dialog::destroy(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            dialog::on_destroy(hwnd, state);
            LRESULT(0)
        }
        _ => {
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

/// 入力欄に見せるため、改行を `\r\n` にする。
fn to_crlf(text: &str) -> String {
    from_crlf(text).replace('\n', "\r\n")
}

/// 入力欄やクリップボードの `\r\n` を `\n` にする。
fn from_crlf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_round_trip() {
        assert_eq!(to_crlf("a\nb"), "a\r\nb");
        assert_eq!(to_crlf("a\r\nb"), "a\r\nb");
        assert_eq!(from_crlf("a\r\nb\r\n"), "a\nb\n");
    }

    #[test]
    fn the_font_height_follows_the_dpi() {
        assert_eq!(font_height_at(-24, 192, 384), -48);
        assert_eq!(font_height_at(-24, 192, 96), -12);
        assert_eq!(font_height_at(-24, 192, 144), -18);
        // 開いたときの DPI なら、開いたときの高さのまま。開いたときの DPI が 0 なら変えない。
        assert_eq!(font_height_at(-13, 96, 96), -13);
        assert_eq!(font_height_at(-13, 0, 96), -13);
    }
}
