//! 端末の窓。Win32 のメッセージを受け、端末の状態・バックエンド・描画をつなぐ。

use std::{
    cell::RefCell,
    sync::Arc,
    time::{Duration, Instant},
};

use fantermo_core::{
    backend::{Backend, ExitCode, GridSize},
    vt::{
        Key, KeyAction, KeyInput, Mods, MouseAction, MouseButton, MouseInput, PasteOutcome,
        PointerPosition, Vt,
    },
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            DirectWrite::IDWriteFactory,
            Gdi::{ClientToScreen, InvalidateRect, ScreenToClient, ValidateRect},
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            SystemServices::{MK_LBUTTON, MK_MBUTTON, MK_RBUTTON},
        },
        UI::{
            HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem},
            Input::KeyboardAndMouse::{
                GetDoubleClickTime, ReleaseCapture, SetCapture, VK_F4, VK_PROCESSKEY, VK_SPACE,
            },
            WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
                DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW,
                GetWindowLongPtrW, IDC_ARROW, KillTimer, LoadCursorW, MSG, PM_REMOVE, PeekMessageW,
                PostMessageW, PostQuitMessage, RegisterClassW, SPI_GETWHEELSCROLLLINES,
                SW_SHOWDEFAULT, SWP_NOACTIVATE, SWP_NOZORDER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
                SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, SystemParametersInfoW,
                TranslateMessage, WHEEL_DELTA, WINDOW_EX_STYLE, WM_APP, WM_CHAR, WM_DEADCHAR,
                WM_DESTROY, WM_DPICHANGED, WM_IME_COMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN,
                WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
                WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SIZE,
                WM_SYSCHAR, WM_SYSDEADCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WNDCLASSW,
                WS_OVERLAPPEDWINDOW,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{
    clipboard,
    conpty::{ConPty, SpawnOptions},
    context_menu::{self, MenuCommand},
    dump::{DUMP_INTERVAL, ScreenDump},
    font::{self, Font},
    ime::{self, CursorRect},
    keyboard::{self, KeyPosition, Utf16Assembler},
    paste_editor,
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

/// 入力の処理のあとで、窓の借用を手放してからすること。ダイアログやメニューは自分でメッセージを
/// 回すので、借用を持ったまま開くと、その間に届いたメッセージの処理が飛ばされる。
enum Deferred {
    /// 改行を含む貼り付けの内容を、編集のダイアログで確かめる。
    EditPaste { text: String, font: Font },
    /// 画面の (`x`, `y`) にコンテキストメニューを出す。
    ContextMenu { x: i32, y: i32, can_copy: bool },
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
    /// ホイール 1 ノッチで動かす行数（Windows の設定）。
    wheel_lines_per_notch: u32,
    /// 高精度のホイールで、まだ 1 行に満たない回転量。
    wheel_remainder: i32,
    /// アプリへホイールを送るときの、まだ 1 ノッチに満たない回転量。
    wheel_report_remainder: i32,
    /// ダブルクリックの判定に使う時計。
    clock: Instant,
    /// 左ボタンで選んでいる最中。
    selecting: bool,
    /// コピーに使ったので、離したときにもシェルへ送らないキー。
    suppressed_release: Option<Key>,
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
            wheel_lines_per_notch: wheel_lines_per_notch(),
            wheel_remainder: 0,
            wheel_report_remainder: 0,
            clock: Instant::now(),
            selecting: false,
            suppressed_release: None,
        })
    }

    /// キーを押した・離した。`chars` は同じキーから TranslateMessage が作った文字。
    fn on_key(
        &mut self,
        virtual_key: u16,
        position: KeyPosition,
        action: KeyAction,
        chars: &[char],
    ) -> Option<Deferred> {
        let mods = keyboard::current_mods();
        let key = keyboard::key_from_position(position);
        // コピーや貼り付けに使ったキーは、離したときもシェルに送らない。
        if action == KeyAction::Release && self.suppressed_release == Some(key) {
            self.suppressed_release = None;
            return None;
        }
        if action != KeyAction::Release && is_copy_shortcut(key, mods) && self.copy_selection() {
            self.suppressed_release = Some(key);
            return None;
        }
        if action == KeyAction::Press && is_paste_shortcut(key, mods) {
            self.suppressed_release = Some(key);
            return self.paste_clipboard();
        }

        let text = keyboard::printable_text(chars);
        let input = KeyInput {
            key,
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
        None
    }

    /// クリップボードの文字列を貼り付ける。改行を含んで確かめが要るときは、編集のダイアログを開くよう返す。
    fn paste_clipboard(&mut self) -> Option<Deferred> {
        let text = match clipboard::text(self.hwnd) {
            Ok(Some(text)) => paste_editor::from_crlf(&text),
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!("failed to read the clipboard: {e}");
                return None;
            }
        };
        match self.paste(&text, false) {
            Some(PasteOutcome::Unsafe) => Some(Deferred::EditPaste {
                text,
                font: self.font.clone(),
            }),
            _ => None,
        }
    }

    /// `text` を貼り付ける。`allow_unsafe` でなければ、コマンドとして走りうる文字列は送らない。
    fn paste(&mut self, text: &str, allow_unsafe: bool) -> Option<PasteOutcome> {
        let outcome = match self.vt.paste(text, allow_unsafe) {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::warn!("failed to paste: {e}");
                return None;
            }
        };
        let encoded = self.vt.take_replies();
        self.send(&encoded);
        Some(outcome)
    }

    /// 選んでいる範囲をクリップボードにコピーし、選択を外す。選んでいなければ何もせず `false`。
    fn copy_selection(&mut self) -> bool {
        let text = match self.vt.selected_text() {
            Ok(Some(text)) => text,
            Ok(None) => return false,
            Err(e) => {
                tracing::warn!("failed to read the selection: {e}");
                return false;
            }
        };
        if let Err(e) = clipboard::set_text(self.hwnd, &text) {
            tracing::warn!("failed to copy to the clipboard: {e}");
        }
        self.clear_selection();
        true
    }

    fn clear_selection(&mut self) {
        if let Err(e) = self.vt.clear_selection() {
            tracing::warn!("failed to clear the selection: {e}");
        }
        self.invalidate();
    }

    /// 窓のクライアント領域の中の位置を、表示している範囲のセルに直す。窓の外は端のセルにする。
    fn pointer(&self, x: i32, y: i32) -> PointerPosition {
        let size = self.vt.size();
        let cell = |pixel: i32, cell_px: u32, count: u16| -> u16 {
            let index = pixel.max(0) as u32 / cell_px;
            index.min(u32::from(count.saturating_sub(1))) as u16
        };
        PointerPosition {
            col: cell(x, size.cell_width_px, size.cols),
            row: cell(y, size.cell_height_px, size.rows),
            x: f64::from(x),
            y: f64::from(y),
        }
    }

    /// マウスのボタンを押した。アプリがマウスを求めていればアプリへ送り、そうでなければ左ボタンで
    /// 選択を始める。Shift を押していれば、アプリが求めていても選択にする。
    fn on_button_down(&mut self, button: MouseButton, lparam: LPARAM, wparam: WPARAM) {
        // 窓の外までドラッグしても離したことが届くように、マウスを捕まえる。
        // SAFETY: self.hwnd はこのスレッドの有効な窓。
        unsafe { SetCapture(self.hwnd) };
        let (x, y) = client_position(lparam);
        if self.report_mouse(MouseAction::Press, Some(button), x, y, wparam) {
            return;
        }
        if button != MouseButton::Left {
            return;
        }
        let at = self.pointer(x, y);
        // SAFETY: 引数のない問い合わせ。
        let repeat_interval = Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }));
        if let Err(e) = self
            .vt
            .selection_press(at, self.clock.elapsed(), repeat_interval)
        {
            tracing::warn!("failed to start a selection: {e}");
        }
        self.selecting = true;
        self.invalidate();
    }

    fn on_mouse_move(&mut self, lparam: LPARAM, wparam: WPARAM) {
        let (x, y) = client_position(lparam);
        if !self.selecting {
            self.report_mouse(MouseAction::Motion, held_button(wparam), x, y, wparam);
            return;
        }
        let at = self.pointer(x, y);
        // Alt を押しながらなら矩形で選ぶ。
        let rectangle = keyboard::current_mods().contains(Mods::ALT);
        if let Err(e) = self.vt.selection_drag(at, rectangle) {
            tracing::warn!("failed to extend the selection: {e}");
        }
        self.invalidate();
    }

    /// マウスのボタンを離した。アプリへ送らなかった右ボタンなら、コンテキストメニューを出すよう返す。
    fn on_button_up(
        &mut self,
        button: MouseButton,
        lparam: LPARAM,
        wparam: WPARAM,
    ) -> Option<Deferred> {
        let (x, y) = client_position(lparam);
        let mut deferred = None;
        if button == MouseButton::Left && self.selecting {
            self.selecting = false;
            if let Err(e) = self.vt.selection_release(self.pointer(x, y)) {
                tracing::warn!("failed to finish the selection: {e}");
            }
            self.invalidate();
        } else if !self.report_mouse(MouseAction::Release, Some(button), x, y, wparam)
            && button == MouseButton::Right
        {
            let (screen_x, screen_y) = self.client_to_screen(x, y);
            deferred = Some(Deferred::ContextMenu {
                x: screen_x,
                y: screen_y,
                can_copy: self.has_selection(),
            });
        }
        if held_button(wparam).is_none() {
            // SAFETY: このスレッドが捕まえたマウスを放す。
            if let Err(e) = unsafe { ReleaseCapture() } {
                tracing::warn!("failed to release the mouse capture: {e}");
            }
        }
        deferred
    }

    fn has_selection(&self) -> bool {
        self.vt.selected_text().is_ok_and(|text| text.is_some())
    }

    fn select_all(&mut self) {
        if let Err(e) = self.vt.select_all() {
            tracing::warn!("failed to select all: {e}");
        }
        self.invalidate();
    }

    /// 窓のクライアント領域の中の位置を、画面での位置にする。
    fn client_to_screen(&self, x: i32, y: i32) -> (i32, i32) {
        let mut point = POINT { x, y };
        // SAFETY: self.hwnd は有効な窓で、point は書き込める。
        if !unsafe { ClientToScreen(self.hwnd, &mut point) }.as_bool() {
            tracing::warn!("failed to convert the menu position");
        }
        (point.x, point.y)
    }

    /// マウスの操作をアプリへ送るか。アプリが求めていて、Shift を押していないとき。
    fn reports_mouse(&self) -> bool {
        if keyboard::current_mods().contains(Mods::SHIFT) {
            return false;
        }
        self.vt.is_mouse_tracking().unwrap_or_else(|e| {
            tracing::warn!("failed to read the mouse mode: {e}");
            false
        })
    }

    /// 画面での位置（WM_MOUSEWHEEL の `lparam`）を、窓のクライアント領域の中の位置にする。
    fn screen_to_client(&self, lparam: LPARAM) -> (i32, i32) {
        let (x, y) = client_position(lparam);
        let mut point = POINT { x, y };
        // SAFETY: self.hwnd は有効な窓で、point は書き込める。
        if !unsafe { ScreenToClient(self.hwnd, &mut point) }.as_bool() {
            tracing::warn!("failed to convert the wheel position");
        }
        (point.x, point.y)
    }

    /// アプリがマウスを求めていて Shift を押していなければ、操作をアプリへ送って `true` を返す。
    /// `x` と `y` は窓のクライアント領域の中の位置（ピクセル）。
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
        let input = MouseInput {
            action,
            button,
            mods,
            x: x as f32,
            y: y as f32,
            any_button_pressed: held_button(wparam).is_some(),
        };
        match self.vt.encode_mouse(&input) {
            Ok(encoded) => self.send(&encoded),
            Err(e) => tracing::warn!("failed to encode the mouse: {e}"),
        }
        true
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

    /// 利用者の入力を子プロセスへ送る。過去を表示していたら、いちばん新しい行に戻す。
    fn send(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.vt.scroll_to_bottom();
        self.clear_selection();
        if let Err(e) = self.backend.write(data) {
            tracing::warn!("failed to write input to the child: {e}");
        }
    }

    /// マウスのホイールが回った。`delta` は WM_MOUSEWHEEL の回転量（奥へ回すと正）。
    /// マウスのホイールが回った。アプリがマウスを求めていれば、1 ノッチごとにボタン 4（奥）か
    /// 5（手前）の押し離しとして送る。そうでなければスクロールバックを動かす。
    /// `lparam` は WM_MOUSEWHEEL のとおり画面での位置。
    fn on_wheel(&mut self, delta: i32, lparam: LPARAM, wparam: WPARAM) {
        if !self.reports_mouse() {
            self.scroll_by_wheel(delta);
            return;
        }
        let (x, y) = self.screen_to_client(lparam);
        let button = if delta > 0 {
            MouseButton::Four
        } else {
            MouseButton::Five
        };
        let (notches, remainder) = wheel_lines(self.wheel_report_remainder, delta, 1);
        self.wheel_report_remainder = remainder;
        for _ in 0..notches.unsigned_abs() {
            self.report_mouse(MouseAction::Press, Some(button), x, y, wparam);
            self.report_mouse(MouseAction::Release, Some(button), x, y, wparam);
        }
    }

    fn scroll_by_wheel(&mut self, delta: i32) {
        let (lines, remainder) =
            wheel_lines(self.wheel_remainder, delta, self.wheel_lines_per_notch);
        self.wheel_remainder = remainder;
        if lines == 0 {
            return;
        }
        // 奥へ回すと過去（上）へ。
        self.vt.scroll(-(lines as isize));
        self.invalidate();
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

/// 窓に結び付けた App を借りて `action` を呼ぶもの。借りられなければ呼ばない。
type WithApp<'a> = dyn Fn(&mut dyn FnMut(&mut App)) + 'a;

/// 窓の借用を手放した状態で、入力の処理が残したことをする。その結果さらに残ったことも続けてする
/// （メニューで貼り付けを選び、それが編集のダイアログを要する場合など）。
fn run_deferred(hwnd: HWND, deferred: Option<Deferred>, with_app: &WithApp) {
    let mut next = deferred;
    while let Some(deferred) = next.take() {
        match deferred {
            Deferred::EditPaste { text, font } => {
                if let Some(edited) = paste_editor::edit(hwnd, &text, &font) {
                    with_app(&mut |app| {
                        app.paste(&edited, true);
                    });
                }
            }
            Deferred::ContextMenu { x, y, can_copy } => {
                match context_menu::show(hwnd, x, y, can_copy) {
                    Some(MenuCommand::Copy) => with_app(&mut |app| {
                        app.copy_selection();
                    }),
                    Some(MenuCommand::Paste) => with_app(&mut |app| next = app.paste_clipboard()),
                    Some(MenuCommand::SelectAll) => with_app(&mut |app| app.select_all()),
                    None => {}
                }
            }
        }
    }
}

/// マウスのメッセージの `lparam` の x と y（符号付きの 16 ビット）。
fn client_position(lparam: LPARAM) -> (i32, i32) {
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
fn message_button(msg: u32) -> MouseButton {
    match msg {
        WM_MBUTTONDOWN | WM_MBUTTONUP => MouseButton::Middle,
        WM_RBUTTONDOWN | WM_RBUTTONUP => MouseButton::Right,
        _ => MouseButton::Left,
    }
}

/// 選んでいればコピーにするキー。Ctrl+C と Ctrl+Insert（ほかの修飾キーなし）。
fn is_copy_shortcut(key: Key, mods: Mods) -> bool {
    held_mods(mods) == Mods::CTRL && matches!(key, Key::C | Key::Insert)
}

/// 貼り付けにするキー。Shift+Insert（ほかの修飾キーなし）。
fn is_paste_shortcut(key: Key, mods: Mods) -> bool {
    held_mods(mods) == Mods::SHIFT && key == Key::Insert
}

/// 押されている修飾キーだけ。ロックの状態は除く。
fn held_mods(mods: Mods) -> Mods {
    mods & (Mods::CTRL | Mods::SHIFT | Mods::ALT | Mods::SUPER)
}

/// 前回の端数 `remainder` に回転量 `delta` を足し、動かす行数と新しい端数を返す。
/// 高精度のホイールは 1 ノッチ（`WHEEL_DELTA`）より細かい回転量で届くので、端数を持ち越す。
fn wheel_lines(remainder: i32, delta: i32, lines_per_notch: u32) -> (i32, i32) {
    let total = remainder + delta * lines_per_notch as i32;
    let lines = total / WHEEL_DELTA as i32;
    (lines, total - lines * WHEEL_DELTA as i32)
}

/// ホイール 1 ノッチで動かす行数の、Windows の設定。読めなければ既定の 3 行。
fn wheel_lines_per_notch() -> u32 {
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

#[cfg(test)]
mod tests {
    use super::*;

    const NOTCH: i32 = WHEEL_DELTA as i32;

    #[test]
    fn copy_shortcuts_need_ctrl_alone() {
        assert!(is_copy_shortcut(Key::C, Mods::CTRL));
        assert!(is_copy_shortcut(Key::Insert, Mods::CTRL));
        // ロックの状態は関係しない。
        assert!(is_copy_shortcut(Key::C, Mods::CTRL | Mods::CAPS_LOCK));
        assert!(!is_copy_shortcut(Key::C, Mods::CTRL | Mods::SHIFT));
        assert!(!is_copy_shortcut(Key::C, Mods::empty()));
        assert!(!is_copy_shortcut(Key::V, Mods::CTRL));
    }

    #[test]
    fn mouse_positions_keep_their_sign() {
        // 窓の外（左上）へドラッグすると、負の位置で届く。
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
    fn paste_shortcut_is_shift_insert_alone() {
        assert!(is_paste_shortcut(Key::Insert, Mods::SHIFT));
        assert!(is_paste_shortcut(Key::Insert, Mods::SHIFT | Mods::NUM_LOCK));
        assert!(!is_paste_shortcut(Key::Insert, Mods::CTRL));
        assert!(!is_paste_shortcut(Key::Insert, Mods::SHIFT | Mods::CTRL));
        assert!(!is_paste_shortcut(Key::V, Mods::CTRL));
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
