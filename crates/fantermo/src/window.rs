//! 端末の窓。Win32 のメッセージを受け、端末の状態・バックエンド・描画をつなぐ。

use std::{cell::RefCell, sync::Arc};

use fantermo_core::{
    backend::{Backend, ExitCode, GridSize},
    vt::{KeyAction, KeyInput, Vt},
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::{
            DirectWrite::IDWriteFactory,
            Gdi::{InvalidateRect, ValidateRect},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem},
            Input::KeyboardAndMouse::{VK_F4, VK_PROCESSKEY, VK_SPACE},
            WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
                DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW,
                GetWindowLongPtrW, IDC_ARROW, KillTimer, LoadCursorW, MSG, PM_REMOVE, PeekMessageW,
                PostMessageW, PostQuitMessage, RegisterClassW, SW_SHOWDEFAULT, SWP_NOACTIVATE,
                SWP_NOZORDER, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
                TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_CHAR, WM_DEADCHAR, WM_DESTROY,
                WM_DPICHANGED, WM_IME_COMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KEYUP,
                WM_NCDESTROY, WM_PAINT, WM_SIZE, WM_SYSCHAR, WM_SYSDEADCHAR, WM_SYSKEYDOWN,
                WM_SYSKEYUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{
    conpty::{ConPty, SpawnOptions},
    dump::{DUMP_INTERVAL, ScreenDump},
    font::{self, Font},
    ime::{self, CursorRect},
    keyboard::{self, KeyPosition, Utf16Assembler},
    renderer::Renderer,
};

const WINDOW_CLASS: PCWSTR = w!("fantermo");
const WINDOW_TITLE: &str = "fantermo";

/// 読むためのスレッドなどが、窓のスレッドを起こすときに送るメッセージ。
const WM_APP_WAKE: u32 = WM_APP + 1;

/// 画面の書き出しに使うタイマー。
const DUMP_TIMER_ID: usize = 1;

/// 起動したときの大きさ（セルの数）。
const INITIAL_COLS: u16 = 100;
const INITIAL_ROWS: u16 = 30;

/// 順に探すフォント。開発中は Bizin Gothic を既定にする。
const FONT_FAMILIES: &[&str] = &["Bizin Gothic", "Consolas"];
const FONT_SIZE_PT: f32 = 12.0;

/// 子プロセスの `TERM` と、端末が名乗る terminfo の名前。
const TERM: &str = "xterm-256color";

/// 窓を開き、子プロセスを起動して、窓が閉じるまでメッセージを処理する。子プロセスの終了コードを返す。
pub fn run(command_line: &str) -> Result<u32, Box<dyn std::error::Error>> {
    let dwrite = font::create_factory()?;
    // SAFETY: 引数のない問い合わせ。
    let system_dpi = unsafe { GetDpiForSystem() };
    let font = font::find(&dwrite, FONT_FAMILIES, FONT_SIZE_PT, system_dpi)?;
    tracing::info!(
        "font {} {}pt: cell {}x{} px",
        font.family,
        FONT_SIZE_PT,
        font.cell_width_px,
        font.cell_height_px
    );

    let hwnd = create_window(&font, system_dpi)?;
    let app = App::new(hwnd, dwrite, font, command_line)?;
    let app = Box::into_raw(Box::new(RefCell::new(app)));
    // SAFETY: hwnd はこのスレッドが作った窓。ポインタは WM_NCDESTROY で取り戻して解放する。
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize) };

    // SAFETY: hwnd は有効な窓。
    unsafe { _ = ShowWindow(hwnd, SW_SHOWDEFAULT) };
    Ok(message_loop())
}

fn create_window(font: &Font, dpi: u32) -> windows::core::Result<HWND> {
    // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
    let instance = unsafe { GetModuleHandleW(None)? };
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance.into(),
        // SAFETY: システムの矢印カーソルを読むだけ。
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    // SAFETY: class は初期化済みで、名前は静的な文字列。
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }

    let mut rect = RECT {
        left: 0,
        top: 0,
        right: (u32::from(INITIAL_COLS) * font.cell_width_px) as i32,
        bottom: (u32::from(INITIAL_ROWS) * font.cell_height_px) as i32,
    };
    // SAFETY: rect は書き込める。
    unsafe {
        AdjustWindowRectExForDpi(
            &mut rect,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE(0),
            dpi,
        )?
    };

    // SAFETY: クラスは登録済みで、文字列はこの呼び出しの間は生きている。
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            &HSTRING::from(WINDOW_TITLE),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
}

fn message_loop() -> u32 {
    let mut msg = MSG::default();
    // SAFETY: msg は書き込める。窓は問わず、このスレッドのメッセージを取る。
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        // SAFETY: msg は GetMessageW が埋めたもの。
        unsafe {
            _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    msg.wParam.0 as u32
}

struct App {
    hwnd: HWND,
    _dwrite: IDWriteFactory,
    font: Font,
    vt: Vt,
    backend: ConPty,
    renderer: Renderer,
    dump: Option<ScreenDump>,
    exit: Option<ExitCode>,
    /// キーを伴わずに届く文字（IME の確定など）を組み立てる。
    text_input: Utf16Assembler,
}

impl App {
    fn new(
        hwnd: HWND,
        dwrite: IDWriteFactory,
        font: Font,
        command_line: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (width, height) = client_size(hwnd);
        let size = grid_size(&font, width, height);
        let renderer = Renderer::new(hwnd, width, height, &dwrite, &font)?;
        let vt = Vt::new(size, TERM)?;

        // HWND はスレッドをまたいで渡せないので、値だけを持たせる。PostMessageW はどのスレッドからでも呼べる。
        let hwnd_value = hwnd.0 as isize;
        let wake = Arc::new(move || {
            // SAFETY: 窓がすでに壊れていれば失敗するだけで、害はない。
            unsafe {
                _ = PostMessageW(
                    Some(HWND(hwnd_value as *mut _)),
                    WM_APP_WAKE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        });
        let backend = ConPty::spawn(
            &SpawnOptions {
                command_line,
                env: &[("TERM", TERM)],
                size,
            },
            wake,
        )?;

        let dump = ScreenDump::from_env();
        if dump.is_some() {
            // SAFETY: hwnd は有効な窓。
            unsafe {
                SetTimer(
                    Some(hwnd),
                    DUMP_TIMER_ID,
                    DUMP_INTERVAL.as_millis() as u32,
                    None,
                )
            };
        }

        Ok(Self {
            hwnd,
            _dwrite: dwrite,
            font,
            vt,
            backend,
            renderer,
            dump,
            exit: None,
            text_input: Utf16Assembler::default(),
        })
    }

    /// キーを押した・離した。`chars` は同じキーから TranslateMessage が作った文字。
    fn on_key(
        &mut self,
        virtual_key: u16,
        position: KeyPosition,
        action: KeyAction,
        chars: &[char],
    ) {
        let mods = keyboard::current_mods();
        let text = keyboard::printable_text(chars);
        let input = KeyInput {
            key: keyboard::key_from_position(position),
            action,
            mods,
            consumed_mods: keyboard::consumed_mods(mods, text.is_some()),
            unshifted: keyboard::unshifted_codepoint(virtual_key),
            text: text.as_deref(),
        };
        match self.vt.encode_key(&input) {
            Ok(encoded) => self.send(&encoded),
            Err(e) => tracing::warn!("failed to encode {:?}: {e}", input.key),
        }
    }

    /// キーを伴わずに届いた文字の 1 単位（IME の確定など）。
    fn on_text_unit(&mut self, unit: u16) {
        let Some(c) = self.text_input.push(unit) else {
            return;
        };
        if c.is_control() {
            return;
        }
        let mut buf = [0u8; 4];
        self.send(c.encode_utf8(&mut buf).as_bytes());
    }

    /// IME の変換中の窓と候補の窓を、いまのカーソルの位置に出させる。
    fn place_ime_windows(&self) {
        let (col, row) = match self.vt.cursor_cell() {
            Ok(cell) => cell,
            Err(e) => {
                tracing::warn!("failed to read the cursor position: {e}");
                return;
            }
        };
        let cursor = CursorRect {
            x: (u32::from(col) * self.font.cell_width_px) as i32,
            y: (u32::from(row) * self.font.cell_height_px) as i32,
            width: self.font.cell_width_px as i32,
            height: self.font.cell_height_px as i32,
        };
        ime::place_windows(self.hwnd, cursor, &self.font);
    }

    fn send(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if let Err(e) = self.backend.write(data) {
            tracing::warn!("failed to write input to the child: {e}");
        }
    }

    /// 出力を端末に流し込み、応答を返し、子プロセスが終わっていればその終了コードを返す。
    fn on_wake(&mut self) -> Option<ExitCode> {
        let vt = &mut self.vt;
        self.backend.drain_output(&mut |data| vt.feed(data));
        let replies = self.vt.take_replies();
        if !replies.is_empty()
            && let Err(e) = self.backend.write(&replies)
        {
            tracing::warn!("failed to write replies to the child: {e}");
        }
        self.invalidate();

        let exit = self.backend.poll_exit();
        if let Some(code) = exit {
            tracing::info!("child process exited with code {}", code.0);
            self.exit = exit;
        }
        exit
    }

    fn on_resize(&mut self, width: u32, height: u32) {
        if let Err(e) = self.renderer.resize(width, height) {
            tracing::warn!("failed to resize the swap chain: {e}");
        }
        let size = grid_size(&self.font, width, height);
        if size == self.vt.size() {
            return;
        }
        if let Err(e) = self.vt.resize(size) {
            tracing::warn!("failed to resize the terminal: {e}");
        }
        if let Err(e) = self.backend.resize(size) {
            tracing::warn!("failed to resize the pseudo console: {e}");
        }
        self.invalidate();
    }

    fn render(&mut self) {
        self.render_to(None);
    }

    fn write_dump(&mut self) {
        let Some(dump) = &self.dump else {
            return;
        };
        dump.write_screen(&self.vt, &self.font.family);
        let frame_path = dump.frame_path();
        self.render_to(Some(&frame_path));
    }

    /// 描いて表示する。`capture` を渡すと、表示する前の画面を PNG で書き出す。
    fn render_to(&mut self, capture: Option<&std::path::Path>) {
        let frame = match self.vt.frame() {
            Ok(frame) => frame,
            Err(e) => {
                tracing::warn!("failed to read the screen: {e}");
                return;
            }
        };
        if let Err(e) = self.renderer.render(&frame, capture) {
            tracing::warn!("failed to render: {e}");
        }
    }

    fn invalidate(&self) {
        // SAFETY: self.hwnd は有効な窓。
        unsafe { _ = InvalidateRect(Some(self.hwnd), None, false) };
    }
}

/// 窓のクライアント領域に収まる大きさ。少なくとも 1 セル。
fn grid_size(font: &Font, width: u32, height: u32) -> GridSize {
    GridSize {
        cols: (width / font.cell_width_px).clamp(1, u32::from(u16::MAX)) as u16,
        rows: (height / font.cell_height_px).clamp(1, u32::from(u16::MAX)) as u16,
        cell_width_px: font.cell_width_px,
        cell_height_px: font.cell_height_px,
    }
}

fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut rect = RECT::default();
    // SAFETY: hwnd は有効な窓で、rect は書き込める。
    if let Err(e) = unsafe { GetClientRect(hwnd, &mut rect) } {
        tracing::warn!("failed to get the client size: {e}");
    }
    (
        (rect.right - rect.left).max(0) as u32,
        (rect.bottom - rect.top).max(0) as u32,
    )
}

/// WM_KEYDOWN の処理中に、そのキーから TranslateMessage が積んだ文字のメッセージを取り出す。
/// 死にキーだったときは `None`。
fn take_translated_chars(hwnd: HWND) -> Option<Vec<char>> {
    const CHAR_MESSAGES: [u32; 4] = [WM_CHAR, WM_SYSCHAR, WM_DEADCHAR, WM_SYSDEADCHAR];
    let mut assembler = Utf16Assembler::default();
    let mut chars = Vec::new();
    let mut dead = false;
    loop {
        let mut msg = MSG::default();
        // SAFETY: msg は書き込める。この窓の、指定した種類のメッセージだけを取り出す。
        let found = CHAR_MESSAGES.into_iter().find(|&kind| unsafe {
            PeekMessageW(&mut msg, Some(hwnd), kind, kind, PM_REMOVE).as_bool()
        });
        match found {
            None => break,
            Some(WM_DEADCHAR | WM_SYSDEADCHAR) => dead = true,
            Some(_) => chars.extend(assembler.push(msg.wParam.0 as u16)),
        }
    }
    (!dead).then_some(chars)
}

/// 窓に結び付けた App。WM_NCDESTROY で解放されるまで有効。
fn app_of(hwnd: HWND) -> Option<&'static RefCell<App>> {
    // SAFETY: GWLP_USERDATA には run が入れた App のポインタか 0 が入っている。
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<App>;
    // SAFETY: 0 でなければ、WM_NCDESTROY で解放するまで有効なポインタ。
    unsafe { pointer.as_ref() }
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(app) = app_of(hwnd) else {
        // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };

    // 処理の途中で別のメッセージがこの場で届くと、借用が重なる。そのときは処理を飛ばす。
    let with_app = |action: &mut dyn FnMut(&mut App)| match app.try_borrow_mut() {
        Ok(mut app) => action(&mut app),
        Err(_) => tracing::debug!("skipped reentrant message {msg:#06x}"),
    };

    match msg {
        WM_APP_WAKE => {
            let mut exit = None;
            with_app(&mut |app| exit = app.on_wake());
            if exit.is_some() {
                // DestroyWindow はこの場で WM_DESTROY を送るので、借用を手放してから呼ぶ。
                // SAFETY: hwnd は有効な窓。
                if let Err(e) = unsafe { DestroyWindow(hwnd) } {
                    tracing::warn!("failed to destroy the window: {e}");
                }
            }
            LRESULT(0)
        }
        WM_PAINT => {
            // SAFETY: hwnd は有効な窓。描くのは Direct3D なので、更新領域を空にするだけ。
            unsafe { _ = ValidateRect(Some(hwnd), None) };
            with_app(&mut |app| app.render());
            LRESULT(0)
        }
        WM_SIZE => {
            let width = (lparam.0 & 0xffff) as u32;
            let height = ((lparam.0 >> 16) & 0xffff) as u32;
            with_app(&mut |app| app.on_resize(width, height));
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == DUMP_TIMER_ID => {
            with_app(&mut |app| app.write_dump());
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let virtual_key = wparam.0 as u16;
            // IME が処理中のキーと、Alt+F4・Alt+Space（閉じる・システムメニュー）は Windows に任せる。
            let left_to_windows = virtual_key == VK_PROCESSKEY.0
                || (msg == WM_SYSKEYDOWN && (virtual_key == VK_F4.0 || virtual_key == VK_SPACE.0));
            if left_to_windows {
                // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            let position = KeyPosition::from_lparam(lparam.0);
            let Some(chars) = take_translated_chars(hwnd) else {
                // 死にキー。次のキーと合わせた文字が WM_CHAR で届く。
                return LRESULT(0);
            };
            let action = if position.repeated {
                KeyAction::Repeat
            } else {
                KeyAction::Press
            };
            with_app(&mut |app| app.on_key(virtual_key, position, action, &chars));
            LRESULT(0)
        }
        WM_KEYUP | WM_SYSKEYUP => {
            let virtual_key = wparam.0 as u16;
            if virtual_key == VK_PROCESSKEY.0 {
                // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            let position = KeyPosition::from_lparam(lparam.0);
            with_app(&mut |app| app.on_key(virtual_key, position, KeyAction::Release, &[]));
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
        WM_DPICHANGED => {
            // 推奨された位置と大きさに移す。フォントの測り直しは描画の段で足す。
            // SAFETY: WM_DPICHANGED の lparam は推奨された矩形を指す。
            let suggested = unsafe { &*(lparam.0 as *const RECT) };
            // SAFETY: hwnd は有効な窓。WM_SIZE がこの場で届くが、借用は持っていない。
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
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: hwnd は有効な窓。
            unsafe { _ = KillTimer(Some(hwnd), DUMP_TIMER_ID) };
            let mut code = 0;
            with_app(&mut |app| code = app.exit.map_or(0, |code| code.0));
            // SAFETY: このスレッドのメッセージループに終了を伝えるだけ。
            unsafe { PostQuitMessage(code as i32) };
            LRESULT(0)
        }
        WM_NCDESTROY => {
            // SAFETY: hwnd は有効な窓で、ポインタを外すのはここだけ。
            let pointer = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut RefCell<App>;
            // SAFETY: pointer は run が Box::into_raw で作ったもので、ほかに参照は残っていない。
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
