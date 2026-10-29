//! 複数行の入力欄とボタンを並べたダイアログ。入力欄の上に、文の行とリンクの行を並べられる。
//!
//! 開いている間は呼び出し元の窓を無効にし、メッセージはこのダイアログのループで回す。IDOK のボタンを
//! 押せば入力欄の文字列を返し、IDCANCEL のボタンや Esc なら何も返さない。ほかのボタンを押せば、その
//! ボタンを返して閉じる。リンクを押せば既定のアプリで開く。

use std::{
    cell::{Cell, RefCell},
    sync::OnceLock,
};

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM},
        Graphics::Gdi::{
            COLOR_BTNFACE, CreateFontIndirectW, DeleteObject, GetDC, GetMonitorInfoW,
            GetTextExtentPoint32W, HBRUSH, HFONT, HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO,
            MonitorFromWindow, ReleaseDC, SelectObject,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{
                ICC_LINK_CLASS, INITCOMMONCONTROLSEX, InitCommonControlsEx, NM_CLICK, NM_RETURN,
                NMHDR, WC_LINK,
            },
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_CONTROL, VK_MENU},
            WindowsAndMessaging::{
                BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CW_USEDEFAULT, CreateWindowExW, DC_HASDEFID,
                DM_GETDEFID, DefWindowProcW, DestroyWindow, ES_AUTOHSCROLL, ES_AUTOVSCROLL,
                ES_MULTILINE, ES_READONLY, ES_WANTRETURN, GWLP_USERDATA, GetClientRect,
                GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, HMENU,
                IDC_ARROW, IDCANCEL, IDOK, LoadCursorW, MSG, MoveWindow, RegisterClassW, SW_SHOW,
                SWP_NOZORDER, SendMessageW, SetWindowLongPtrW, SetWindowPos, SetWindowTextW,
                ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY,
                WM_KEYDOWN, WM_NEXTDLGCTL, WM_NOTIFY, WM_SETFONT, WM_SIZE, WNDCLASSW, WS_CAPTION,
                WS_CHILD, WS_DISABLED, WS_EX_CLIENTEDGE, WS_EX_CONTROLPARENT, WS_EX_DLGMODALFRAME,
                WS_HSCROLL, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_THICKFRAME, WS_VISIBLE,
                WS_VSCROLL,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{font::Font, link_opener, modal, ui_font};

const CLASS: PCWSTR = w!("fantermo-text-dialog");

/// 入力欄の制御 ID。閉じるボタンは IDOK と IDCANCEL を使う。
const EDIT_ID: i32 = 100;
/// 入力欄の上の行の制御 ID の始まり。上から順に 1 ずつ増やす。
const HEADER_ID: i32 = 200;

/// `EM_SETLIMITTEXT` と、その上限を外す値。
const EM_SETLIMITTEXT: u32 = 0x00C5;
const NO_TEXT_LIMIT: usize = 0;
/// `EM_GETRECT`。入力欄の中で文字を並べる矩形を返す。
const EM_GETRECT: u32 = 0x00B2;
/// `EM_SETSEL` と、すべてを選ぶときの範囲。
const EM_SETSEL: u32 = 0x00B1;
const SELECT_FROM_START: usize = 0;
const SELECT_TO_END: isize = -1;
/// Ctrl+A の A の仮想キー。
const VK_A: usize = b'A' as usize;

/// 96 DPI のときの寸法（ピクセル）。窓の DPI に合わせて拡大する。
const BASE_DPI: f32 = 96.0;
const MARGIN: f32 = 8.0;
const BUTTON_WIDTH: f32 = 112.0;
const BUTTON_HEIGHT: f32 = 30.0;
/// 呼び出し元の窓に対する、ダイアログの大きさの割合。
const SIZE_RATIO: f32 = 0.7;
/// 端末の文字に対する、入力欄の文字の大きさの割合。
const FONT_RATIO: f32 = 0.75;

/// ダイアログの下に右から並べるボタン。
pub struct Button {
    /// 押したときの制御 ID。どのボタンを押してもダイアログを閉じる。
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
}

/// ダイアログを閉じたわけ。
#[derive(Debug, PartialEq, Eq)]
pub enum Closed {
    /// IDOK のボタンを押した。入力欄の文字列を添える。改行は `\n`。
    Ok(String),
    /// IDOK と IDCANCEL のほかのボタンを押した。その制御 ID を添える。
    Button(i32),
    /// IDCANCEL のボタン、Esc、窓の × で閉じた。開けなかったときも。
    Cancel,
}

/// `dialog` を開き、閉じるまで待つ。`font` は端末のフォントで、入力欄はその書体を少し小さくして使う。
pub fn run(owner: HWND, dialog: &Dialog, font: &Font) -> Closed {
    if let Err(e) = register_class() {
        tracing::warn!("failed to register the text dialog: {e}");
        return Closed::Cancel;
    }
    let state = DialogState::default();
    let hwnd = match create(owner, dialog, &state, font) {
        Ok(hwnd) => hwnd,
        Err(e) => {
            tracing::warn!("failed to open the text dialog {:?}: {e}", dialog.title);
            return Closed::Cancel;
        }
    };
    // SAFETY: 入力欄はこのダイアログの子で、文字列はこの呼び出しの間は生きている。
    unsafe { _ = SetWindowTextW(state.edit.get(), &HSTRING::from(to_crlf(dialog.text))) };

    run_modal(owner, hwnd, &state);

    for created_font in [state.font.get(), state.header_font.get()] {
        if !created_font.is_invalid() {
            // SAFETY: フォントはダイアログのために作ったもので、ダイアログはもう閉じている。
            unsafe { _ = DeleteObject(HGDIOBJ(created_font.0)) };
        }
    }
    state.result.take().unwrap_or(Closed::Cancel)
}

/// ダイアログの状態。ウィンドウプロシージャからも触るので、共有の参照だけで読み書きする。
#[derive(Default)]
struct DialogState {
    edit: Cell<HWND>,
    /// 入力欄の上の行。上から順に並べる。
    header: RefCell<Vec<HWND>>,
    /// リンクの行と、その URL。
    links: RefCell<Vec<(HWND, String)>>,
    /// 入力欄の上の行の 1 行の高さ（ピクセル）。
    header_line_height: Cell<i32>,
    /// ボタン。左から順に並べる。
    buttons: RefCell<Vec<HWND>>,
    font: Cell<HFONT>,
    /// 入力欄の上の行のフォント。行がなければ作らない。
    header_font: Cell<HFONT>,
    /// 寸法の拡大率（窓の DPI / 96）。
    scale: Cell<f32>,
    /// 閉じたわけ。ボタンで閉じるまでは `None`。
    result: RefCell<Option<Closed>>,
    /// ボタンの制御 ID。入力欄などのほかの子からの WM_COMMAND と見分ける。
    button_ids: RefCell<Vec<i32>>,
    closed: Cell<bool>,
    /// Enter で押されるボタンの制御 ID。
    default_id: Cell<Option<i32>>,
}

fn register_class() -> windows::core::Result<()> {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    if REGISTERED.get().is_some() {
        return Ok(());
    }
    let controls = INITCOMMONCONTROLSEX {
        dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_LINK_CLASS,
    };
    // SAFETY: controls は大きさを入れて初期化してある。
    if !unsafe { InitCommonControlsEx(&controls) }.as_bool() {
        tracing::warn!("failed to register the link control");
    }
    // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
    let instance = unsafe { GetModuleHandleW(None)? };
    let class = WNDCLASSW {
        lpfnWndProc: Some(dialog_proc),
        hInstance: instance.into(),
        // SAFETY: システムの矢印カーソルを読むだけ。
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        // システムの色の番号に 1 を足した値は、その色のブラシとして扱われる。
        hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as isize as *mut _),
        lpszClassName: CLASS,
        ..Default::default()
    };
    // SAFETY: class は初期化済みで、名前は静的な文字列。
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    _ = REGISTERED.set(());
    Ok(())
}

/// 呼び出し元の窓の真ん中に、その大きさの 7 割でダイアログを作る。`min_columns` があれば、その桁数が
/// 収まるまで幅を広げる。
fn create(
    owner: HWND,
    dialog: &Dialog,
    state: &DialogState,
    font: &Font,
) -> windows::core::Result<HWND> {
    // SAFETY: owner は呼び出し元の有効な窓。
    let dpi = unsafe { GetDpiForWindow(owner) };
    state.scale.set(dpi as f32 / BASE_DPI);

    let mut owner_rect = RECT::default();
    // SAFETY: owner は有効な窓で、owner_rect は書き込める。
    unsafe { GetWindowRect(owner, &mut owner_rect)? };
    let width = ((owner_rect.right - owner_rect.left) as f32 * SIZE_RATIO) as i32;
    let height = ((owner_rect.bottom - owner_rect.top) as f32 * SIZE_RATIO) as i32;
    let left = owner_rect.left + (owner_rect.right - owner_rect.left - width) / 2;
    let top = owner_rect.top + (owner_rect.bottom - owner_rect.top - height) / 2;

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

    let child = |class: PCWSTR, text: &str, style: WINDOW_STYLE, ex_style, id: i32| {
        // SAFETY: hwnd は有効な窓で、文字列はこの呼び出しの間は生きている。
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
    // SAFETY: logfont は初期化済み。作ったフォントはダイアログが閉じた後に消す。
    state.font.set(unsafe { CreateFontIndirectW(&logfont) });
    let controls = std::iter::once(state.edit.get()).chain(state.buttons.borrow().clone());
    for control in controls {
        set_font(control, state.font.get());
    }
    if !dialog.header.is_empty() {
        create_header_font(state, dpi);
    }
    // SAFETY: 入力欄は有効な窓。
    unsafe {
        SendMessageW(
            state.edit.get(),
            EM_SETLIMITTEXT,
            Some(WPARAM(NO_TEXT_LIMIT)),
            None,
        )
    };

    // 状態を結び付けてから大きさを決め、WM_SIZE で子を並べさせる。
    // SAFETY: hwnd は作ったばかりの窓。ポインタは run_modal が戻るまで有効で、WM_DESTROY で外す。
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, std::ptr::from_ref(state) as isize);
        SetWindowPos(hwnd, None, left, top, width, height, SWP_NOZORDER)?;
        layout(hwnd, state);
    }
    if let Some(columns) = dialog.min_columns {
        widen_for_columns(owner, &owner_rect, hwnd, state, columns);
    }
    // SAFETY: hwnd と入力欄は有効な窓。
    unsafe {
        _ = ShowWindow(hwnd, SW_SHOW);
        _ = SetFocus(Some(state.edit.get()));
    }
    Ok(hwnd)
}

/// `control` の文字のフォントを `font` にする。
fn set_font(control: HWND, font: HFONT) {
    // SAFETY: control はダイアログの子で、フォントはダイアログが閉じるまで消さない。
    unsafe {
        SendMessageW(
            control,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        )
    };
}

/// 入力欄の上の行に、DPI が `dpi` のときの Windows のメッセージのフォントを付け、1 行の高さを測る。
fn create_header_font(state: &DialogState, dpi: u32) {
    let logfont = match ui_font::message_logfont(dpi) {
        Ok(logfont) => logfont,
        Err(e) => {
            tracing::warn!("failed to read the message font: {e}");
            return;
        }
    };
    // SAFETY: logfont は初期化済み。作ったフォントはダイアログが閉じた後に消す。
    let font = unsafe { CreateFontIndirectW(&logfont) };
    state.header_font.set(font);
    let header = state.header.borrow();
    for &control in header.iter() {
        set_font(control, font);
    }
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
    // SAFETY: control は有効な窓。選んだフォントは戻し、DC は読み終えたら返す。
    unsafe {
        let dc = GetDC(Some(control));
        if dc.is_invalid() {
            return None;
        }
        let previous = SelectObject(dc, HGDIOBJ(font.0));
        let mut size = SIZE::default();
        let measured = GetTextExtentPoint32W(dc, &sample, &mut size).as_bool();
        SelectObject(dc, previous);
        ReleaseDC(Some(control), dc);
        measured.then_some(size.cy)
    }
}

/// 入力欄の文字を並べる幅に `columns` 桁が収まらなければ、足りない分だけダイアログを広げ、
/// 呼び出し元の窓 `owner`（場所は `owner_rect`）の真ん中に置き直す。モニターからははみ出させない。
fn widen_for_columns(
    owner: HWND,
    owner_rect: &RECT,
    dialog: HWND,
    state: &DialogState,
    columns: u32,
) {
    let Some(text_width) = text_width(state, columns) else {
        tracing::warn!("failed to measure the text dialog font");
        return;
    };
    let mut format = RECT::default();
    // SAFETY: 入力欄は有効な窓で、format は書き込める。
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
    let mut window = RECT::default();
    // SAFETY: dialog は有効な窓で、window は書き込める。
    if let Err(e) = unsafe { GetWindowRect(dialog, &mut window) } {
        tracing::warn!("failed to read the text dialog position: {e}");
        return;
    }
    let width = window.right - window.left + shortfall;
    let left = owner_rect.left + (owner_rect.right - owner_rect.left - width) / 2;
    let (left, width) = fit_horizontally(owner, left, width);
    let height = window.bottom - window.top;
    // SAFETY: dialog は有効な窓。WM_SIZE で子を並べ直す。
    let moved =
        unsafe { SetWindowPos(dialog, None, left, window.top, width, height, SWP_NOZORDER) };
    if let Err(e) = moved {
        tracing::warn!("failed to widen the text dialog: {e}");
    }
}

/// 入力欄のフォントで `columns` 桁を並べた幅（ピクセル）。測れなければ `None`。
fn text_width(state: &DialogState, columns: u32) -> Option<i32> {
    let edit = state.edit.get();
    // 平均の文字幅は実際の送り幅より小さいことがあるので、その桁数の文字列の幅を測る。
    let line: Vec<u16> = std::iter::repeat_n(u16::from(b'0'), columns as usize).collect();
    // SAFETY: edit は有効な窓。選んだフォントは戻し、DC は読み終えたら返す。
    unsafe {
        let dc = GetDC(Some(edit));
        if dc.is_invalid() {
            return None;
        }
        let previous = SelectObject(dc, HGDIOBJ(state.font.get().0));
        let mut size = SIZE::default();
        let measured = GetTextExtentPoint32W(dc, &line, &mut size).as_bool();
        SelectObject(dc, previous);
        ReleaseDC(Some(edit), dc);
        measured.then_some(size.cx)
    }
}

/// 横の位置 `left` と幅 `width` を、呼び出し元の窓があるモニターの作業領域に収める。
fn fit_horizontally(owner: HWND, left: i32, width: i32) -> (i32, i32) {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: owner は有効な窓で、info は大きさを入れて初期化してある。
    let found = unsafe {
        let monitor = MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, &mut info).as_bool()
    };
    if !found {
        tracing::warn!("failed to read the monitor of the window");
        return (left, width);
    }
    let work = info.rcWork;
    let width = width.min(work.right - work.left);
    let left = left.clamp(work.left, work.right - width);
    (left, width)
}

/// 呼び出し元の窓を無効にし、ダイアログが閉じるまでメッセージを回す。入力欄での Ctrl+A は、すべてを
/// 選ぶ。
fn run_modal(owner: HWND, dialog: HWND, state: &DialogState) {
    modal::run(owner, dialog, &state.closed, |msg| {
        if !is_select_all(msg, state.edit.get()) {
            return false;
        }
        // SAFETY: 入力欄は有効な窓。
        unsafe {
            SendMessageW(
                state.edit.get(),
                EM_SETSEL,
                Some(WPARAM(SELECT_FROM_START)),
                Some(LPARAM(SELECT_TO_END)),
            )
        };
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
fn layout(dialog: HWND, state: &DialogState) {
    let mut client = RECT::default();
    // SAFETY: dialog は有効な窓で、client は書き込める。
    if unsafe { GetClientRect(dialog, &mut client) }.is_err() {
        return;
    }
    let scale = state.scale.get();
    let px = |value: f32| (value * scale).round() as i32;
    let (margin, button_w, button_h) = (px(MARGIN), px(BUTTON_WIDTH), px(BUTTON_HEIGHT));
    let button_top = client.bottom - margin - button_h;
    let line_height = state.header_line_height.get();
    let mut top = margin;
    for &line in state.header.borrow().iter() {
        // SAFETY: line は dialog の子の有効な窓。
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
    // SAFETY: 入力欄は dialog の子の有効な窓。
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
        // SAFETY: button は dialog の子の有効な窓。
        unsafe { _ = MoveWindow(button, button_left, button_top, button_w, button_h, true) };
    }
}

fn read_text(edit: HWND) -> String {
    // SAFETY: edit は有効な入力欄。
    let length = unsafe { GetWindowTextLengthW(edit) }.max(0) as usize;
    let mut buffer = vec![0u16; length + 1];
    // SAFETY: buffer は length + 1 個の u16 を書き込める。
    let copied = unsafe { GetWindowTextW(edit, &mut buffer) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..copied])
}

/// ダイアログに結び付けた状態。WM_DESTROY で外すまで有効。
fn state_of<'a>(dialog: HWND) -> Option<&'a DialogState> {
    // SAFETY: GWLP_USERDATA には create が入れた DialogState のポインタか 0 が入っている。
    let pointer = unsafe { GetWindowLongPtrW(dialog, GWLP_USERDATA) } as *const DialogState;
    // SAFETY: 0 でなければ、run の中の DialogState を指し、ダイアログが閉じるまで有効。
    unsafe { pointer.as_ref() }
}

extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(state) = state_of(hwnd) else {
        // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    match msg {
        WM_SIZE => {
            layout(hwnd, state);
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            let closed = if id == IDOK.0 {
                Some(Closed::Ok(from_crlf(&read_text(state.edit.get()))))
            } else if id == IDCANCEL.0 {
                Some(Closed::Cancel)
            } else if state.button_ids.borrow().contains(&id) {
                Some(Closed::Button(id))
            } else {
                None
            };
            if let Some(closed) = closed {
                state.result.replace(Some(closed));
                // SAFETY: hwnd はこのダイアログ。
                unsafe { _ = DestroyWindow(hwnd) };
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
        // Enter を自分で使わない入力欄は、既定のボタンをこの二つで問い合わせて押させる。
        DM_GETDEFID => match state.default_id.get() {
            Some(id) => LRESULT(((DC_HASDEFID as isize) << 16) | id as isize),
            None => LRESULT(0),
        },
        WM_NEXTDLGCTL => {
            // lparam が 0 でなければ、wparam は移す先の子の窓。順に移すほうは使われないので扱わない。
            if lparam.0 & 0xffff != 0 {
                // SAFETY: wparam はこのダイアログの子の窓のハンドルとして渡される。
                unsafe { _ = SetFocus(Some(HWND(wparam.0 as *mut _))) };
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            // SAFETY: hwnd はこのダイアログ。
            unsafe { _ = DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            state.closed.set(true);
            // SAFETY: hwnd はこのダイアログ。以後、状態のポインタは使わせない。
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
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
pub fn from_crlf(text: &str) -> String {
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
}
