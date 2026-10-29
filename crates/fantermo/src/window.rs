//! 端末の窓。Win32 のメッセージを受け、端末の状態・バックエンド・描画をつなぐ。

use std::{
    cell::RefCell,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use fantermo_core::{
    backend::{ExitCode, GridSize},
    config::{Config, DEFAULT_FONT_FAMILIES, FontConfig, PasteConfig, PasteConfirm, WideGlyph},
    frame::{Frame, Rgb},
    keymap::{self, Keymap},
    link::Link,
    vt::{
        self, Key, KeyAction, KeyInput, Mods, MouseAction, MouseButton, MouseInput, PasteOutcome,
        PointerPosition,
    },
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::{
            DirectWrite::IDWriteFactory,
            Gdi::{ClientToScreen, InvalidateRect, ScreenToClient, ValidateRect},
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            SystemServices::{MK_CONTROL, MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MK_SHIFT},
        },
        UI::{
            Controls::WM_MOUSELEAVE,
            HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow},
            Input::KeyboardAndMouse::{
                GetDoubleClickTime, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT,
                TrackMouseEvent, VK_F4, VK_PROCESSKEY, VK_SPACE,
            },
            WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
                DestroyWindow, DispatchMessageW, FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO,
                FlashWindowEx, GWLP_USERDATA, GetClientRect, GetForegroundWindow, GetMessageW,
                GetWindowLongPtrW, GetWindowRect, HTCLIENT, IDC_ARROW, IDC_HAND, KillTimer,
                LoadCursorW, MSG, PM_REMOVE, PeekMessageW, PostMessageW, PostQuitMessage,
                RegisterClassW, SPI_GETWHEELSCROLLLINES, SW_SHOWDEFAULT, SWP_NOACTIVATE,
                SWP_NOZORDER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SetCursor,
                SetTimer, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow,
                SystemParametersInfoW, TranslateMessage, WA_INACTIVE, WHEEL_DELTA, WINDOW_EX_STYLE,
                WM_ACTIVATE, WM_APP, WM_CHAR, WM_CONTEXTMENU, WM_DEADCHAR, WM_DESTROY,
                WM_DPICHANGED, WM_GETDPISCALEDSIZE, WM_IME_COMPOSITION, WM_IME_STARTCOMPOSITION,
                WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
                WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WM_RBUTTONDOWN,
                WM_RBUTTONUP, WM_SETCURSOR, WM_SIZE, WM_SYSCHAR, WM_SYSDEADCHAR, WM_SYSKEYDOWN,
                WM_SYSKEYUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{
    clipboard,
    config_file::{self, Loaded},
    context_menu::{self, MenuCommand},
    dump::{DUMP_INTERVAL, ScreenDump},
    font::{self, Font},
    ime::{self, CursorRect},
    keyboard::{self, KeyPosition, Utf16Assembler},
    link_opener,
    notice::{self, Topic},
    paste_editor,
    renderer::{Area, Renderer},
    session::{Session, SessionId},
    tab_bar::{self, TabView},
};

const WINDOW_CLASS: PCWSTR = w!("fantermo");
const WINDOW_TITLE: &str = "fantermo";

/// 端末の出力が届いたときと子プロセスが終了したときに、読むためのスレッドなどが窓のスレッドを
/// 起こすメッセージ。`wparam` はその端末の [`SessionId`]。
const WM_APP_WAKE: u32 = WM_APP + 1;

/// 設定ファイルを監視するスレッドが、書き換えを知らせるメッセージ。
const WM_APP_CONFIG_CHANGED: u32 = WM_APP + 2;

/// 画面の書き出しに使うタイマー。
const DUMP_TIMER_ID: usize = 1;

/// 設定ファイルの書き換えを知らされてから読み直すまでのタイマー。
const CONFIG_TIMER_ID: usize = 2;

/// 設定ファイルの書き換えを知らされてから読み直すまでの間。この間に届いた知らせはまとめる。
const CONFIG_RELOAD_DELAY: Duration = Duration::from_millis(200);

/// 点滅（SGR 5、6）の文字を見せる・隠すを切り替えるタイマー。
const BLINK_TIMER_ID: usize = 3;

/// 点滅の文字を見せる間と隠す間の、それぞれの長さ。
const BLINK_INTERVAL: Duration = Duration::from_millis(500);

/// キーボードから開いた WM_CONTEXTMENU が `lparam` に入れてくる位置。
const KEYBOARD_CONTEXT_MENU_POSITION: (i32, i32) = (-1, -1);

/// 起動したときの大きさ（セルの数）。
const INITIAL_COLS: u16 = 100;
const INITIAL_ROWS: u16 = 30;

/// 開発用。起動したときの窓の DPI を、この環境変数の値とみなす。倍率の違うモニタへ移したときの
/// 作り直しを、モニタを使わずに確かめるために使う。
const DPI_ENV: &str = "FANTERMO_DPI";

/// 引数も設定ファイルの `shell` もないときに起動するコマンドライン。
const DEFAULT_COMMAND_LINE: &str = "powershell.exe -NoLogo";

/// 端末を開くときに起動するコマンドライン。fantermo の引数 `arguments` があればそれ、なければ設定の
/// `shell`、それもなければ既定のシェル。
fn command_line_for(arguments: Option<&str>, shell: Option<&str>) -> String {
    arguments
        .or(shell)
        .unwrap_or(DEFAULT_COMMAND_LINE)
        .to_owned()
}

/// 窓を開き、子プロセスを起動して、窓が閉じるまでメッセージを処理する。最後の端末の子プロセスが終わって
/// 窓を閉じたときはその終了コードを、利用者が最後の端末を閉じたときは 0 を返す。`arguments` は fantermo の引数（子プロセスのコマンドライン）。`config_path` は
/// 設定ファイルの場所で、`loaded` はそこから読んだ設定。読んだときの警告は、窓を出してから見せる。
pub fn run(
    arguments: Option<String>,
    config_path: Option<PathBuf>,
    loaded: Loaded,
) -> Result<u32, Box<dyn std::error::Error>> {
    let Loaded { config, notice } = loaded;
    let dwrite = font::create_factory()?;
    // SAFETY: 引数のない問い合わせ。
    let system_dpi = unsafe { GetDpiForSystem() };
    let font = load_font(&dwrite, &config.font, system_dpi)?;
    log_font(&font, system_dpi);

    let hwnd = create_window(&font, system_dpi)?;
    let app = App::new(
        hwnd,
        dwrite,
        font,
        system_dpi,
        config_path,
        config,
        arguments,
    )?;
    let app = Box::into_raw(Box::new(RefCell::new(app)));
    // SAFETY: hwnd はこのスレッドが作った窓。ポインタは WM_NCDESTROY で取り戻して解放する。
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize) };

    // 窓を置いたモニタの DPI がシステムの DPI と違えば、DPI が変わったときと同じように作り直す。
    let window_dpi = dpi_from_env().unwrap_or_else(|| {
        // SAFETY: hwnd は有効な窓。
        unsafe { GetDpiForWindow(hwnd) }
    });
    if window_dpi != system_dpi {
        change_dpi_in_place(hwnd, window_dpi);
    }

    // SAFETY: hwnd は有効な窓。
    unsafe { _ = ShowWindow(hwnd, SW_SHOWDEFAULT) };
    // メッセージループの外なので、App の借用はない。
    if let Some(text) = notice {
        notice::warn(hwnd, Topic::Settings, text);
    }
    Ok(message_loop())
}

/// 設定のフォントを先頭から探し、`dpi` で測る。どれも見つからなければ、既定のフォントから探す。
fn load_font(
    dwrite: &IDWriteFactory,
    settings: &FontConfig,
    dpi: u32,
) -> windows::core::Result<Font> {
    let families: Vec<&str> = settings.families.iter().map(String::as_str).collect();
    match font::find(dwrite, &families, settings.size_pt, dpi) {
        Ok(font) => Ok(font),
        Err(e) if families != DEFAULT_FONT_FAMILIES => {
            tracing::warn!("none of the fonts {families:?} is available, trying the defaults: {e}");
            font::find(dwrite, &DEFAULT_FONT_FAMILIES, settings.size_pt, dpi)
        }
        Err(e) => Err(e),
    }
}

fn log_font(font: &Font, dpi: u32) {
    tracing::info!(
        "font {} {}pt at {dpi} DPI: cell {}x{} px",
        font.family,
        font.size_pt,
        font.cell_width_px,
        font.cell_height_px
    );
}

/// [`DPI_ENV`] で指定された DPI。指定がないか、正の整数でなければ `None`。
fn dpi_from_env() -> Option<u32> {
    let value = std::env::var(DPI_ENV).ok()?;
    match value.parse::<u32>() {
        Ok(dpi) if dpi > 0 => Some(dpi),
        _ => {
            tracing::warn!("ignoring {DPI_ENV}={value:?}: not a positive integer");
            None
        }
    }
}

/// 窓の左上を動かさずに、DPI が `dpi` に変わったときの処理（フォントとアトラスの作り直し）をさせる。
/// 桁数と行数は保つ。DPI が変わらず、フォントの設定が変わったときにも使う。
fn change_dpi_in_place(hwnd: HWND, dpi: u32) {
    let Some(app) = app_of(hwnd) else {
        return;
    };
    let client = app.borrow().scaled_client_size(dpi);
    let client = match client {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!("failed to compute the window size for {dpi} DPI: {e}");
            return;
        }
    };
    let mut rect = RECT::default();
    // SAFETY: hwnd は有効な窓で、rect は書き込める。
    if let Err(e) = unsafe { GetWindowRect(hwnd, &mut rect) } {
        tracing::warn!("failed to get the window position: {e}");
    }
    // 枠の大きさは、いまの窓の枠から取る。DPI_ENV で DPI を差し替えたときも、枠は窓の実際の
    // DPI で付くため。
    let (current_width, current_height) = client_size(hwnd);
    let frame_width = (rect.right - rect.left) - current_width as i32;
    let frame_height = (rect.bottom - rect.top) - current_height as i32;
    rect.right = rect.left + client.cx + frame_width;
    rect.bottom = rect.top + client.cy + frame_height;
    // WM_DPICHANGED の wparam は、下位に横の DPI、上位に縦の DPI。
    let wparam = WPARAM((dpi | (dpi << 16)) as usize);
    // SAFETY: hwnd は有効な窓。rect は処理が終わるまで生きている。借用は持っていない。
    unsafe {
        SendMessageW(
            hwnd,
            WM_DPICHANGED,
            Some(wparam),
            Some(LPARAM(&raw const rect as isize)),
        )
    };
}

/// `font` で `cols` × `rows` のセルを並べた大きさ（ピクセル）。
fn grid_pixels(font: &Font, cols: u16, rows: u16) -> SIZE {
    SIZE {
        cx: (u32::from(cols) * font.cell_width_px) as i32,
        cy: (u32::from(rows) * font.cell_height_px) as i32,
    }
}

/// 大きさが `width` × `height` のクライアント領域のうち、表示している端末を描く領域。上端の高さ
/// `tab_bar_height` のタブバーを除いた残りの全体に描く。
fn terminal_area_in_client(width: u32, height: u32, tab_bar_height: u32) -> Area {
    let tab_bar_height = tab_bar_height.min(height);
    Area {
        x: 0,
        y: tab_bar_height,
        width,
        height: height - tab_bar_height,
    }
}

/// 端末を描く領域の大きさが `terminal` になるクライアント領域の大きさ。[`terminal_area_in_client`] と
/// 対になる。
fn client_size_for_terminal(terminal: SIZE, tab_bar_height: u32) -> SIZE {
    SIZE {
        cx: terminal.cx,
        cy: terminal.cy + tab_bar_height as i32,
    }
}

/// クライアント領域の大きさが `client` になる、DPI が `dpi` の窓の大きさ（枠を含む）。
fn window_size(client: SIZE, dpi: u32) -> windows::core::Result<SIZE> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: client.cx,
        bottom: client.cy,
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
    Ok(SIZE {
        cx: rect.right - rect.left,
        cy: rect.bottom - rect.top,
    })
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

    // 起動したときの端末は一つなので、タブバーは出さない。
    let size = window_size(
        client_size_for_terminal(grid_pixels(font, INITIAL_COLS, INITIAL_ROWS), 0),
        dpi,
    )?;

    // SAFETY: クラスは登録済みで、文字列はこの呼び出しの間は生きている。
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            &HSTRING::from(WINDOW_TITLE),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            size.cx,
            size.cy,
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
    /// 窓を閉じる。最後の端末を閉じたときに使う。
    CloseWindow,
    /// `topic` についての警告 `text` を見せる。
    Warn { topic: Topic, text: String },
}

/// タブの操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabCommand {
    /// 新しい端末を開いて表示する。
    New,
    /// 表示している端末を閉じる。
    Close,
    /// 右隣の端末を表示する。右端なら左端へ。
    Next,
    /// 左隣の端末を表示する。左端なら右端へ。
    Previous,
}

struct App {
    hwnd: HWND,
    dwrite: IDWriteFactory,
    font: Font,
    /// いまの窓の DPI。
    dpi: u32,
    /// 設定ファイルの場所。
    config_path: Option<PathBuf>,
    /// `font` を探したときの設定。
    font_config: FontConfig,
    /// 貼り付ける前に、編集のダイアログで確かめるとき。
    paste_confirm: PasteConfirm,
    /// fantermo の引数（子プロセスのコマンドライン）。
    arguments: Option<String>,
    /// 設定の `shell`。
    shell: Option<String>,
    /// 窓が持つ端末。タブの並びの順。少なくとも一つある。
    sessions: Vec<Session>,
    /// 表示している端末の、`sessions` の中の位置。
    active: usize,
    /// 次に開く端末の番号。閉じた端末の番号は使い直さない（閉じたあとに届く知らせを捨てるため）。
    next_session_id: usize,
    /// 窓のクライアント領域のうち、表示している端末を描く領域。
    terminal_area: Area,
    renderer: Renderer,
    dump: Option<ScreenDump>,
    /// 窓のタイトルに出している文字列。
    window_title: String,
    /// 最後の端末が終わったときの、その子プロセスの終了コード。窓を閉じたあとの終了コードにする。
    exit: Option<ExitCode>,
    /// キーを伴わずに届く文字（IME の確定など）を組み立てる。
    text_input: Utf16Assembler,
    /// ホイール 1 ノッチで動かす行数（Windows の設定）。
    wheel_lines_per_notch: u32,
    /// ダブルクリックの判定に使う時計。
    clock: Instant,
    /// マウスの、窓のクライアント領域の中の位置。窓の外へ出たら `None`。
    pointer_in_window: Option<(i32, i32)>,
    /// fantermo が受け持つキーの割り当て。
    keymap: Keymap,
    /// 割り当ての動作に使ったので、離したときにも端末へ送らないキー。タブを切り替えたあとに
    /// 離したことが届くので、端末ごとではなく窓に持つ。
    suppressed_release: Option<Key>,
    /// タブバーの上で押したマウスのボタンと、そのときマウスの下にあったタブの番号。離すまで、マウスの
    /// 操作を端末へ送らない。
    tab_bar_press: Option<(MouseButton, Option<usize>)>,
    /// 窓がアクティブ（前面）。WM_ACTIVATE で決める。
    window_active: bool,
    /// 点滅の文字を見せている。点滅させていないときは `true`。
    blink_visible: bool,
    /// 点滅のタイマーを動かしている。
    blink_timer_running: bool,
}

impl App {
    fn new(
        hwnd: HWND,
        dwrite: IDWriteFactory,
        font: Font,
        dpi: u32,
        config_path: Option<PathBuf>,
        config: Config,
        arguments: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let font_config = config.font;
        let (width, height) = client_size(hwnd);
        let terminal_area =
            terminal_area_in_client(width, height, tab_bar::height(font.cell_height_px, 1));
        let size = grid_size(&font, terminal_area);
        let renderer = Renderer::new(hwnd, width, height, &dwrite, &font, font_config.wide_glyph)?;
        let first_id = SessionId(0);
        let session = Session::spawn(
            first_id,
            &command_line_for(arguments.as_deref(), config.shell.as_deref()),
            size,
            Arc::new(poster(hwnd, WM_APP_WAKE, WPARAM(first_id.0))),
        )?;
        if let Some(path) = &config_path {
            config_file::watch(path, poster(hwnd, WM_APP_CONFIG_CHANGED, WPARAM(0)));
        }

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
            dwrite,
            font,
            dpi,
            config_path,
            font_config,
            paste_confirm: config.paste.confirm,
            arguments,
            shell: config.shell,
            sessions: vec![session],
            active: 0,
            next_session_id: first_id.0 + 1,
            terminal_area,
            renderer,
            dump,
            window_title: WINDOW_TITLE.to_owned(),
            exit: None,
            text_input: Utf16Assembler::default(),
            wheel_lines_per_notch: wheel_lines_per_notch(),
            clock: Instant::now(),
            pointer_in_window: None,
            keymap: Keymap::default(),
            suppressed_release: None,
            tab_bar_press: None,
            window_active: false,
            blink_visible: true,
            blink_timer_running: false,
        })
    }

    /// 表示している端末。
    fn session(&self) -> &Session {
        &self.sessions[self.active]
    }

    fn session_mut(&mut self) -> &mut Session {
        &mut self.sessions[self.active]
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
        // Ctrl を押した・離したときは、マウスが動かなくてもリンクの下線を出し入れする。押し続けたときの
        // 繰り返しでは、状態が変わらないので探し直さない。
        if is_modifier_key(key) && action != KeyAction::Repeat {
            self.refresh_hovered_link(is_link_modifier(mods));
        }
        // メニューやダイアログが離した側を受け取ると、止める印が残る。押し直したなら印は古い。
        if action == KeyAction::Press && self.suppressed_release == Some(key) {
            self.suppressed_release = None;
        }
        // 割り当ての動作に使ったキーは、離したときもシェルに送らない。
        if action == KeyAction::Release && self.suppressed_release == Some(key) {
            self.suppressed_release = None;
            return None;
        }
        if action != KeyAction::Release
            && let Some(bound) = self.keymap.lookup(key, mods)
            && let Some(deferred) = self.run_bound_action(bound, action)
        {
            self.suppressed_release = Some(key);
            return deferred;
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
        match self.session_mut().vt.encode_key(&input) {
            Ok(encoded) => self.send(&encoded),
            Err(e) => tracing::warn!("failed to encode {:?}: {e}", input.key),
        }
        None
    }

    /// キーに割り当てた動作 `bound` を、押した・押し続けた（`action`）ときに行う。動作が成り立たず、
    /// キーをシェルへ送るときは `None`。
    fn run_bound_action(
        &mut self,
        bound: keymap::Action,
        action: KeyAction,
    ) -> Option<Option<Deferred>> {
        let repeated = action == KeyAction::Repeat;
        match bound {
            // 選んでいなければシェルへ送る。押し続けたときも同じ。
            keymap::Action::Copy => self.copy_selection().then_some(None),
            // 押し続けたときの繰り返しはシェルへ送る。
            keymap::Action::Paste => (!repeated).then(|| self.paste_clipboard()),
            // ここから下は、押し続けたときの繰り返しもシェルへ送らない。メニュー・開く・閉じるは
            // 繰り返さず、次・前は繰り返す。
            keymap::Action::ContextMenu => Some((!repeated).then(|| self.context_menu_at_cursor())),
            keymap::Action::NewTab => Some(
                (!repeated)
                    .then(|| self.run_tab_command(TabCommand::New))
                    .flatten(),
            ),
            keymap::Action::CloseTab => Some(
                (!repeated)
                    .then(|| self.run_tab_command(TabCommand::Close))
                    .flatten(),
            ),
            keymap::Action::NextTab => Some(self.run_tab_command(TabCommand::Next)),
            keymap::Action::PrevTab => Some(self.run_tab_command(TabCommand::Previous)),
        }
    }

    /// タブの操作をする。最後の端末を閉じたときは、窓を閉じるよう返す。
    fn run_tab_command(&mut self, command: TabCommand) -> Option<Deferred> {
        let count = self.sessions.len();
        match command {
            TabCommand::New => self.open_session(),
            TabCommand::Close => {
                if self.close_session(self.active) {
                    return Some(Deferred::CloseWindow);
                }
            }
            TabCommand::Next => self.activate((self.active + 1) % count),
            TabCommand::Previous => self.activate((self.active + count - 1) % count),
        }
        None
    }

    /// 新しい端末を開き、タブの並びの最後に足して表示する。起動できなければ警告を残して何もしない。
    fn open_session(&mut self) {
        let id = SessionId(self.next_session_id);
        let command_line = command_line_for(self.arguments.as_deref(), self.shell.as_deref());
        // タブバーが出ると端末を描く領域が狭まるので、足したあとの領域の大きさで起動する。
        let (width, height) = client_size(self.hwnd);
        let area = terminal_area_in_client(
            width,
            height,
            tab_bar::height(self.font.cell_height_px, self.sessions.len() + 1),
        );
        let size = grid_size(&self.font, area);
        let session = match Session::spawn(
            id,
            &command_line,
            size,
            Arc::new(poster(self.hwnd, WM_APP_WAKE, WPARAM(id.0))),
        ) {
            Ok(session) => session,
            Err(e) => {
                tracing::warn!("failed to open a terminal with {command_line:?}: {e}");
                return;
            }
        };
        self.next_session_id += 1;
        self.sessions.push(session);
        self.relayout();
        self.activate(self.sessions.len() - 1);
    }

    /// `sessions` の `index` の端末を閉じる（子プロセスは疑似コンソールを閉じて終わらせる）。最後の
    /// 端末なら閉じずに `true` を返す（窓を閉じると一緒に閉じる）。
    fn close_session(&mut self, index: usize) -> bool {
        if self.sessions.len() == 1 {
            return true;
        }
        let was_active = index == self.active;
        if was_active {
            self.leave_active();
        }
        drop(self.sessions.remove(index));
        self.active = active_after_close(self.active, index, self.sessions.len());
        self.relayout();
        if was_active {
            self.on_activated();
        }
        // タブバーのタブが減った。
        self.invalidate();
        false
    }

    /// `sessions` の `index` の端末を表示する。
    fn activate(&mut self, index: usize) {
        if index == self.active {
            return;
        }
        self.leave_active();
        self.active = index;
        self.on_activated();
    }

    /// 表示している端末を表示から外す前に、マウスの操作の途中の状態を終える。
    fn leave_active(&mut self) {
        if self.session().selecting {
            // SAFETY: このスレッドが捕まえたマウスを放す。捕まえていなければ何もしない。
            if let Err(e) = unsafe { ReleaseCapture() } {
                tracing::warn!("failed to release the mouse capture: {e}");
            }
        }
        self.session_mut().release_pointer();
    }

    /// 表示する端末が替わった。窓のタイトルとマウスの下のリンクを合わせ、描き直す。
    fn on_activated(&mut self) {
        tracing::debug!(
            "showing terminal {:?} ({} of {})",
            self.session().id(),
            self.active + 1,
            self.sessions.len()
        );
        self.session_mut().bell_marked = false;
        self.update_window_title();
        self.refresh_hovered_link_now();
        self.invalidate();
    }

    /// クリップボードの文字列を貼り付ける。設定の `paste.confirm` に従って確かめが要るときは、
    /// 編集のダイアログを開くよう返す。
    fn paste_clipboard(&mut self) -> Option<Deferred> {
        let text = match clipboard::text(self.hwnd) {
            Ok(Some(text)) => paste_editor::from_crlf(&text),
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!("failed to read the clipboard: {e}");
                return None;
            }
        };
        let needs_confirmation = match paste_action(self.paste_confirm, &text) {
            PasteAction::Confirm => true,
            PasteAction::Paste { allow_unsafe } => {
                self.paste(&text, allow_unsafe) == Some(PasteOutcome::Unsafe)
            }
        };
        needs_confirmation.then(|| Deferred::EditPaste {
            text,
            font: self.font.clone(),
        })
    }

    /// `text` を貼り付ける。`allow_unsafe` でなければ、コマンドとして走りうる文字列は送らない。
    fn paste(&mut self, text: &str, allow_unsafe: bool) -> Option<PasteOutcome> {
        let outcome = match self.session_mut().vt.paste(text, allow_unsafe) {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::warn!("failed to paste: {e}");
                return None;
            }
        };
        let encoded = self.session_mut().vt.take_replies();
        self.send(&encoded);
        Some(outcome)
    }

    /// 選んでいる範囲をクリップボードにコピーし、選択を外す。選んでいなければ何もせず `false`。
    fn copy_selection(&mut self) -> bool {
        let text = match self.session().vt.selected_text() {
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
        if let Err(e) = self.session_mut().vt.clear_selection() {
            tracing::warn!("failed to clear the selection: {e}");
        }
        self.invalidate();
    }

    /// 窓のクライアント領域の中の位置を、端末を描く領域の左上からの位置にする。
    fn client_to_terminal(&self, x: i32, y: i32) -> (i32, i32) {
        (
            x - self.terminal_area.x as i32,
            y - self.terminal_area.y as i32,
        )
    }

    /// 端末を描く領域の左上からの位置を、窓のクライアント領域の中の位置にする。
    fn terminal_to_client(&self, x: i32, y: i32) -> (i32, i32) {
        (
            x + self.terminal_area.x as i32,
            y + self.terminal_area.y as i32,
        )
    }

    /// 窓のクライアント領域の中の位置を、表示している範囲のセルに直す。端末を描く領域の外は端のセルに
    /// する。
    fn pointer(&self, x: i32, y: i32) -> PointerPosition {
        let (x, y) = self.client_to_terminal(x, y);
        let size = self.session().vt.size();
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

    /// 窓のクライアント領域の中の位置にあるリンク。
    fn link_at(&self, x: i32, y: i32) -> Option<Link> {
        let (x, y) = self.client_to_terminal(x, y);
        if x < 0 || y < 0 {
            return None;
        }
        let size = self.session().vt.size();
        let col = (x as u32 / size.cell_width_px).min(u32::from(u16::MAX)) as u16;
        let row = (y as u32 / size.cell_height_px).min(u32::from(u16::MAX)) as u16;
        self.session().vt.link_at(col, row).unwrap_or_else(|e| {
            tracing::warn!("failed to look for a link: {e}");
            None
        })
    }

    /// マウスの下のリンクを探し直す。`modifier_held` はリンクを開く修飾キー（Ctrl）を押しているか。
    /// 変わったら、下線を引き直し、カーソルの形を変える。
    fn refresh_hovered_link(&mut self, modifier_held: bool) {
        let link = match self.pointer_in_window {
            Some((x, y)) if modifier_held && !self.session().selecting => self.link_at(x, y),
            _ => None,
        };
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

    /// 修飾キーをいま押しているかで、マウスの下のリンクを探し直す。画面の中身や表示している範囲が
    /// 変わったときに使う。
    fn refresh_hovered_link_now(&mut self) {
        if self.pointer_in_window.is_none() && self.session().hovered_link.is_none() {
            return;
        }
        self.refresh_hovered_link(is_link_modifier(keyboard::current_mods()));
    }

    /// マウスのカーソルを、リンクの上なら手の形、そうでなければ矢印にする。
    fn set_cursor_shape(&self) {
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

    /// マウスが窓のクライアント領域の中の (`x`, `y`) にある。窓の外へ出たことが届くようにする。
    fn track_pointer(&mut self, x: i32, y: i32) {
        if self.pointer_in_window.is_none() {
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: track は初期化済みで、self.hwnd はこのスレッドの有効な窓。
            if let Err(e) = unsafe { TrackMouseEvent(&mut track) } {
                tracing::warn!("failed to track the mouse leaving the window: {e}");
            }
        }
        self.pointer_in_window = Some((x, y));
    }

    /// マウスが窓のクライアント領域の外へ出た。
    fn on_mouse_leave(&mut self) {
        self.pointer_in_window = None;
        self.refresh_hovered_link(false);
    }

    /// マウスのボタンを押した。Ctrl を押しながらリンクの上で左ボタンを押したら、離すまで何もしない
    /// （アプリへも送らず、選択も始めない）。そうでなければ、アプリがマウスを求めていればアプリへ送り、
    /// 求めていなければ左ボタンで選択を始める。Shift を押していれば、アプリが求めていても選択にする。
    fn on_button_down(&mut self, button: MouseButton, lparam: LPARAM, wparam: WPARAM) {
        // 窓の外までドラッグしても離したことが届くように、マウスを捕まえる。
        // SAFETY: self.hwnd はこのスレッドの有効な窓。
        unsafe { SetCapture(self.hwnd) };
        let (x, y) = client_position(lparam);
        if let Some(tab) = self.tab_bar_hit(x, y) {
            self.tab_bar_press = Some((button, tab));
            if button == MouseButton::Left
                && let Some(index) = tab
            {
                self.activate(index);
            }
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

    fn on_mouse_move(&mut self, lparam: LPARAM, wparam: WPARAM) {
        let (x, y) = client_position(lparam);
        self.track_pointer(x, y);
        self.refresh_hovered_link(is_link_modifier(mouse_mods(wparam)));
        // リンクやタブバーを押している間の動きと、タブバーの上の動きは、アプリへ送らない。
        if self.session().pressed_link.is_some() || self.tab_bar_press.is_some() {
            return;
        }
        if !self.session().selecting {
            if self.tab_bar_hit(x, y).is_some() {
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
    fn on_button_up(
        &mut self,
        button: MouseButton,
        lparam: LPARAM,
        wparam: WPARAM,
    ) -> Option<Deferred> {
        let (x, y) = client_position(lparam);
        let mut deferred = None;
        if let Some((pressed, tab)) = self.tab_bar_press
            && pressed == button
        {
            self.tab_bar_press = None;
            deferred = self.on_tab_bar_click(button, tab, x, y);
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
            // SAFETY: このスレッドが捕まえたマウスを放す。
            if let Err(e) = unsafe { ReleaseCapture() } {
                tracing::warn!("failed to release the mouse capture: {e}");
            }
        }
        deferred
    }

    /// 窓のクライアント領域の中の (`x`, `y`) がタブバーの上なら、そこにあるタブの番号（タブの間のすき間や
    /// 右の空きなら `None`）を `Some` で返す。タブバーの外なら `None`。
    fn tab_bar_hit(&self, x: i32, y: i32) -> Option<Option<usize>> {
        let area = self.tab_bar_area()?;
        let inside = (0..area.width as i32).contains(&x) && (0..area.height as i32).contains(&y);
        if !inside {
            return None;
        }
        let col = (x as u32 / self.font.cell_width_px).min(u32::from(u16::MAX)) as u16;
        let spans = tab_bar::layout(self.sessions.len(), self.tab_bar_cols());
        Some(tab_bar::tab_at(&spans, col))
    }

    /// タブバーの上で押した `button` を (`x`, `y`) で離した。押したときのマウスの下のタブは `pressed_tab`。
    /// 中ボタンを同じタブの上で離したらそのタブを閉じ、右ボタンならコンテキストメニューを出すよう返す。
    /// 左ボタンは押したときに切り替えてある。
    fn on_tab_bar_click(
        &mut self,
        button: MouseButton,
        pressed_tab: Option<usize>,
        x: i32,
        y: i32,
    ) -> Option<Deferred> {
        match button {
            MouseButton::Middle => {
                let index = pressed_tab?;
                if self.tab_bar_hit(x, y) != Some(Some(index)) {
                    return None;
                }
                self.close_session(index).then_some(Deferred::CloseWindow)
            }
            MouseButton::Right => Some(self.context_menu(x, y)),
            _ => None,
        }
    }

    /// 窓のクライアント領域の中の (`x`, `y`) にコンテキストメニューを出すよう返す。
    fn context_menu(&self, x: i32, y: i32) -> Deferred {
        let (screen_x, screen_y) = self.client_to_screen(x, y);
        Deferred::ContextMenu {
            x: screen_x,
            y: screen_y,
            can_copy: self.has_selection(),
        }
    }

    /// テキストカーソルのセルの左下にコンテキストメニューを出すよう返す。キーボードから開くときに使う。
    fn context_menu_at_cursor(&self) -> Deferred {
        let (col, row) = self.session().vt.cursor_cell().unwrap_or_else(|e| {
            tracing::warn!("failed to read the cursor position: {e}");
            (0, 0)
        });
        let (x, y) = self.terminal_to_client(
            (u32::from(col) * self.font.cell_width_px) as i32,
            ((u32::from(row) + 1) * self.font.cell_height_px) as i32,
        );
        self.context_menu(x, y)
    }

    fn has_selection(&self) -> bool {
        self.session()
            .vt
            .selected_text()
            .is_ok_and(|text| text.is_some())
    }

    fn select_all(&mut self) {
        if let Err(e) = self.session_mut().vt.select_all() {
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
        self.session().vt.is_mouse_tracking().unwrap_or_else(|e| {
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
        let (x, y) = self.client_to_terminal(x, y);
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
        let (col, row) = match self.session().vt.cursor_cell() {
            Ok(cell) => cell,
            Err(e) => {
                tracing::warn!("failed to read the cursor position: {e}");
                return;
            }
        };
        let (x, y) = self.terminal_to_client(
            (u32::from(col) * self.font.cell_width_px) as i32,
            (u32::from(row) * self.font.cell_height_px) as i32,
        );
        let cursor = CursorRect {
            x,
            y,
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
        self.session_mut().vt.scroll_to_bottom();
        self.refresh_hovered_link_now();
        self.clear_selection();
        self.session_mut().write(data);
    }

    /// マウスのホイールが回った。アプリがマウスを求めていれば、1 ノッチごとにボタン 4（奥）か
    /// 5（手前）の押し離しとして送る。そうでなければスクロールバックを動かす。
    /// `delta` は WM_MOUSEWHEEL の回転量（奥へ回すと正）、`lparam` は WM_MOUSEWHEEL のとおり画面での位置。
    fn on_wheel(&mut self, delta: i32, lparam: LPARAM, wparam: WPARAM) {
        let (x, y) = self.screen_to_client(lparam);
        // タブバーの上では何もしない。
        if self.tab_bar_hit(x, y).is_some() {
            return;
        }
        if !self.reports_mouse() {
            self.scroll_by_wheel(delta);
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
            self.report_mouse(MouseAction::Release, Some(button), x, y, wparam);
        }
    }

    fn scroll_by_wheel(&mut self, delta: i32) {
        let (lines, remainder) = wheel_lines(
            self.session().wheel_remainder,
            delta,
            self.wheel_lines_per_notch,
        );
        self.session_mut().wheel_remainder = remainder;
        if lines == 0 {
            return;
        }
        // 奥へ回すと過去（上）へ。
        self.session_mut().vt.scroll(-(lines as isize));
        self.refresh_hovered_link_now();
        self.invalidate();
    }

    /// 端末 `id` に出力が届いたか、子プロセスが終わった。出力を流し込み、タイトルとベルを反映する。
    /// 表示していない端末の出力も流し込む（描くのは表示している端末だけ）。窓を閉じるときは `true` を
    /// 返す。
    fn on_wake(&mut self, id: SessionId) -> bool {
        let Some(index) = self.sessions.iter().position(|session| session.id() == id) else {
            tracing::debug!("ignoring a wake for an unknown terminal {id:?}");
            return false;
        };
        let events = self.sessions[index].on_output();
        let shown = index == self.active;
        if events.bell {
            self.ring_bell();
        }
        if shown {
            if events.title_changed {
                self.update_window_title();
            }
            // マウスの下の文字が変わったかもしれない。
            self.refresh_hovered_link_now();
            self.invalidate();
        } else if events.title_changed || events.bell {
            // タブバーのタイトルとベルの印を描き直す。
            if events.bell {
                self.sessions[index].bell_marked = true;
            }
            self.invalidate();
        }
        events
            .exited
            .is_some_and(|code| self.on_session_ended(index, code))
    }

    /// `sessions` の `index` の端末の子プロセスが `code` で終わった。その端末を閉じる。最後の端末なら、
    /// それを窓の終了コードにし、窓を閉じるよう `true` を返す。
    fn on_session_ended(&mut self, index: usize, code: ExitCode) -> bool {
        if !self.close_session(index) {
            return false;
        }
        self.exit = Some(code);
        true
    }

    /// 窓のタイトルを、表示している端末のタイトルにする。アプリが付けていなければ既定のタイトル。
    /// 変わったときだけ窓に設定する。
    fn update_window_title(&mut self) {
        let title = tab_title(self.session());
        if title == self.window_title {
            return;
        }
        let title = title.to_owned();
        // SAFETY: self.hwnd は有効な窓。文字列はこの呼び出しの間は生きている。
        if let Err(e) = unsafe { SetWindowTextW(self.hwnd, &HSTRING::from(&title)) } {
            tracing::warn!("failed to set the window title: {e}");
            return;
        }
        self.window_title = title;
    }

    /// ベル。窓が前面になければ、前面に来るまでタスクバーのボタンを点滅させる。音は鳴らさない。
    fn ring_bell(&self) {
        // SAFETY: 引数のない問い合わせ。
        if unsafe { GetForegroundWindow() } == self.hwnd {
            return;
        }
        let flash = FLASHWINFO {
            cbSize: size_of::<FLASHWINFO>() as u32,
            hwnd: self.hwnd,
            dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
            // FLASHW_TIMERNOFG で、回数ではなく前面に来るまで点滅を続ける。
            uCount: 0,
            // 0 でカーソルの点滅の間隔を使う。
            dwTimeout: 0,
        };
        // SAFETY: flash は初期化済みで、self.hwnd は有効な窓。戻り値は前の点滅の状態で、失敗ではない。
        unsafe { _ = FlashWindowEx(&flash) };
    }

    /// 窓のクライアント領域の大きさが変わった。端末を描く領域と、端末の大きさを合わせる。
    fn on_resize(&mut self, width: u32, height: u32) {
        if let Err(e) = self.renderer.resize(width, height) {
            tracing::warn!("failed to resize the swap chain: {e}");
        }
        self.layout(width, height);
    }

    /// 端末の数が変わった。タブバーを出し入れし、端末を描く領域と端末の大きさを合わせる。窓の大きさは
    /// 変えない（タブバーの分だけ端末の行数が変わる）。
    fn relayout(&mut self) {
        let (width, height) = client_size(self.hwnd);
        self.layout(width, height);
    }

    /// クライアント領域の大きさが `width` × `height` のときの、端末を描く領域に合わせる。
    fn layout(&mut self, width: u32, height: u32) {
        self.terminal_area =
            terminal_area_in_client(width, height, self.tab_bar_height(&self.font));
        self.resize_sessions();
    }

    /// `font` で描くときのタブバーの高さ。
    fn tab_bar_height(&self, font: &Font) -> u32 {
        tab_bar::height(font.cell_height_px, self.sessions.len())
    }

    /// すべての端末の大きさを、端末を描く領域に合わせる。表示していない端末も同じ大きさにする。
    fn resize_sessions(&mut self) {
        let size = grid_size(&self.font, self.terminal_area);
        let mut changed = false;
        for session in &mut self.sessions {
            changed |= session.resize(size);
        }
        if !changed {
            return;
        }
        self.refresh_hovered_link_now();
        self.invalidate();
    }

    /// DPI が `dpi` に変わった。フォントを測り直し、グリフアトラスを作り直す。端末の大きさは、
    /// このあと届く WM_SIZE（[`App::on_resize`]）で合わせる。失敗したら前のフォントのまま描く。
    fn apply_dpi(&mut self, dpi: u32) {
        self.dpi = dpi;
        let font = match load_font(&self.dwrite, &self.font_config, dpi) {
            Ok(font) => font,
            Err(e) => {
                tracing::warn!("failed to measure the font for {dpi} DPI: {e}");
                return;
            }
        };
        if let Err(e) = self
            .renderer
            .set_font(&self.dwrite, &font, self.font_config.wide_glyph)
        {
            tracing::warn!("failed to rebuild the glyph atlas for {dpi} DPI: {e}");
            return;
        }
        log_font(&font, dpi);
        self.font = font;
        self.invalidate();
    }

    /// 設定ファイルを読み直す。フォントの設定が変わっていて、そのフォントを測れたら設定を入れ替え、
    /// 作り直しに使う DPI を返す。作り直しは借用を手放してから [`change_dpi_in_place`] で行う。
    /// グリフの描き方だけが変わったときは、セルの寸法は変わらないので、この場でアトラスだけを作り直す。
    /// 貼り付けの設定はすぐに入れ替える。`shell` は次に開く端末から使う。
    /// 誤りがあれば、それを見せることを返す（借用を手放してから [`run_deferred`] で見せる）。誤りが
    /// なければ、設定の警告を取り下げる（開いているメッセージボックスも閉じさせる）。
    fn reload_config(&mut self) -> (Option<u32>, Option<Deferred>) {
        let Loaded { config, notice } =
            config_file::reload(self.config_path.as_deref(), &self.config_in_use());
        let warn = match notice {
            Some(text) => Some(Deferred::Warn {
                topic: Topic::Settings,
                text,
            }),
            None => {
                notice::withdraw(self.hwnd, Topic::Settings);
                None
            }
        };
        (self.apply_config(config), warn)
    }

    /// 読み直した設定を入れ替える。作り直しが要れば、その DPI を返す（[`App::reload_config`]）。
    fn apply_config(&mut self, config: Config) -> Option<u32> {
        self.paste_confirm = config.paste.confirm;
        self.shell = config.shell;
        if config.font == self.font_config {
            return None;
        }
        let same_face = config.font.families == self.font_config.families
            && config.font.size_pt == self.font_config.size_pt;
        if same_face {
            self.set_wide_glyph(config.font.wide_glyph);
            return None;
        }
        if let Err(e) = load_font(&self.dwrite, &config.font, self.dpi) {
            tracing::warn!("keeping the current font: {e}");
            return None;
        }
        self.font_config = config.font;
        Some(self.dpi)
    }

    /// いま使っている設定。フォントは、いまのフォントを探したときの設定。
    fn config_in_use(&self) -> Config {
        Config {
            font: self.font_config.clone(),
            shell: self.shell.clone(),
            paste: PasteConfig {
                confirm: self.paste_confirm,
            },
        }
    }

    /// セルより広いグリフの描き方を変え、グリフアトラスを作り直す。失敗したら前の描き方のまま。
    fn set_wide_glyph(&mut self, wide_glyph: WideGlyph) {
        if let Err(e) = self.renderer.set_font(&self.dwrite, &self.font, wide_glyph) {
            tracing::warn!("failed to rebuild the glyph atlas for {wide_glyph:?}: {e}");
            return;
        }
        self.font_config.wide_glyph = wide_glyph;
        self.invalidate();
    }

    /// DPI が `dpi` に変わったときに、いまの桁数と行数がそのまま収まるクライアント領域の大きさ。
    fn scaled_client_size(&self, dpi: u32) -> windows::core::Result<SIZE> {
        let font = load_font(&self.dwrite, &self.font_config, dpi)?;
        let size = self.session().vt.size();
        Ok(client_size_for_terminal(
            grid_pixels(&font, size.cols, size.rows),
            self.tab_bar_height(&font),
        ))
    }

    /// DPI が `dpi` に変わったときに、いまの桁数と行数がそのまま収まる窓の大きさ（枠を含む）。
    fn scaled_window_size(&self, dpi: u32) -> windows::core::Result<SIZE> {
        window_size(self.scaled_client_size(dpi)?, dpi)
    }

    fn render(&mut self) {
        self.render_to(None);
    }

    fn write_dump(&mut self) {
        let Some(dump) = &self.dump else {
            return;
        };
        dump.write_screen(
            &self.session().vt,
            &self.font,
            self.sessions.len(),
            self.active,
        );
        let frame_path = dump.frame_path();
        self.render_to(Some(&frame_path));
    }

    /// 表示している端末を端末を描く領域に、端末が二つ以上ならタブバーをその上に描いて表示する。
    /// `capture` を渡すと、表示する前の画面を PNG で書き出す。
    fn render_to(&mut self, capture: Option<&std::path::Path>) {
        let mut frame = match self.session_mut().vt.frame() {
            Ok(frame) => frame,
            Err(e) => {
                tracing::warn!("failed to read the screen: {e}");
                return;
            }
        };
        self.set_blinking(self.window_active && frame.has_blink());
        if !self.blink_visible {
            frame.hide_blinking_text();
        }
        let tab_bar = self.tab_bar_frame(frame.background, frame.foreground);
        let mut layers = vec![(&frame, self.terminal_area)];
        if let Some((tab_bar, area)) = &tab_bar {
            layers.push((tab_bar, *area));
        }
        if let Err(e) = self.renderer.render(&layers, capture) {
            tracing::warn!("failed to render: {e}");
        }
    }

    /// タブバーの画面と、それを描く領域。タブバーを出さないときは `None`。色は端末の背景色
    /// `background` と文字の色 `foreground` から決める。
    fn tab_bar_frame(&self, background: Rgb, foreground: Rgb) -> Option<(Frame, Area)> {
        let area = self.tab_bar_area()?;
        let tabs: Vec<TabView> = self
            .sessions
            .iter()
            .map(|session| TabView {
                title: tab_title(session),
                bell: session.bell_marked,
            })
            .collect();
        let frame = tab_bar::frame(
            &tabs,
            self.active,
            self.tab_bar_cols(),
            background,
            foreground,
        );
        Some((frame, area))
    }

    /// タブバーを描く領域。端末を描く領域の上の、窓の幅いっぱい。タブバーを出さないときは `None`。
    fn tab_bar_area(&self) -> Option<Area> {
        (self.terminal_area.y > 0).then_some(Area {
            x: 0,
            y: 0,
            width: self.terminal_area.width,
            height: self.terminal_area.y,
        })
    }

    /// タブバーの幅（セル）。
    fn tab_bar_cols(&self) -> u16 {
        (self.terminal_area.width / self.font.cell_width_px).min(u32::from(u16::MAX)) as u16
    }

    /// 点滅のタイマーを動かす・止める。止めたら点滅の文字を見せた状態に戻す。動かし始めも見せた状態から。
    fn set_blinking(&mut self, blinking: bool) {
        if blinking == self.blink_timer_running {
            return;
        }
        self.blink_visible = true;
        if blinking {
            // SAFETY: self.hwnd は有効な窓。
            let started = unsafe {
                SetTimer(
                    Some(self.hwnd),
                    BLINK_TIMER_ID,
                    BLINK_INTERVAL.as_millis() as u32,
                    None,
                )
            } != 0;
            if !started {
                tracing::warn!(
                    "failed to start the blink timer: {}",
                    windows::core::Error::from_thread()
                );
                return;
            }
        } else {
            // SAFETY: self.hwnd は有効な窓。
            if let Err(e) = unsafe { KillTimer(Some(self.hwnd), BLINK_TIMER_ID) } {
                tracing::warn!("failed to stop the blink timer: {e}");
            }
        }
        self.blink_timer_running = blinking;
        tracing::debug!("blink timer running: {blinking}");
    }

    /// 点滅のタイマーが来た。見せる・隠すを切り替えて描き直す。
    fn on_blink_timer(&mut self) {
        self.blink_visible = !self.blink_visible;
        self.invalidate();
    }

    /// 窓がアクティブになった・でなくなった。アクティブでない間は点滅を止め、文字を見せておく。
    /// 点滅を始めるかどうかは、次に描くときに画面に点滅の文字があるかで決める。
    fn on_activate(&mut self, active: bool) {
        self.window_active = active;
        if !active {
            let was_hidden = !self.blink_visible;
            self.set_blinking(false);
            if !was_hidden {
                return;
            }
        }
        self.invalidate();
    }

    fn invalidate(&self) {
        // SAFETY: self.hwnd は有効な窓。
        unsafe { _ = InvalidateRect(Some(self.hwnd), None, false) };
    }
}

/// 端末のタブと、表示しているときの窓に出すタイトル。アプリが付けたタイトル、付けていなければ既定の
/// タイトル。
fn tab_title(session: &Session) -> &str {
    match session.title() {
        "" => WINDOW_TITLE,
        title => title,
    }
}

/// 端末を描く領域 `area` に収まる大きさ。少なくとも 1 セル。
fn grid_size(font: &Font, area: Area) -> GridSize {
    GridSize {
        cols: (area.width / font.cell_width_px).clamp(1, u32::from(u16::MAX)) as u16,
        rows: (area.height / font.cell_height_px).clamp(1, u32::from(u16::MAX)) as u16,
        cell_width_px: font.cell_width_px,
        cell_height_px: font.cell_height_px,
    }
}

/// ほかのスレッドから呼んで、`hwnd` に `message` を `wparam` を添えて積むもの。
fn poster(hwnd: HWND, message: u32, wparam: WPARAM) -> impl Fn() + Send + Sync + 'static {
    // HWND はスレッドをまたいで渡せないので、値だけを持たせる。PostMessageW はどのスレッドからでも呼べる。
    let hwnd_value = hwnd.0 as isize;
    move || {
        // SAFETY: 窓がすでに壊れていれば失敗するだけで、害はない。
        unsafe {
            _ = PostMessageW(Some(HWND(hwnd_value as *mut _)), message, wparam, LPARAM(0));
        }
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
                    Some(MenuCommand::NewTab) => {
                        with_app(&mut |app| next = app.run_tab_command(TabCommand::New))
                    }
                    Some(MenuCommand::CloseTab) => {
                        with_app(&mut |app| next = app.run_tab_command(TabCommand::Close))
                    }
                    None => {}
                }
            }
            // DestroyWindow はこの場で WM_DESTROY を送るので、借用を手放してから呼ぶ。
            Deferred::CloseWindow => destroy_window(hwnd),
            Deferred::Warn { topic, text } => notice::warn(hwnd, topic, text),
        }
    }
}

fn destroy_window(hwnd: HWND) {
    // SAFETY: hwnd は有効な窓。借用は持っていない。
    if let Err(e) = unsafe { DestroyWindow(hwnd) } {
        tracing::warn!("failed to destroy the window: {e}");
    }
}

/// `closed` の位置の端末を閉じて `remaining` 個になったあとの、表示する端末の位置。表示していた
/// `active` を閉じたら、右隣（右端なら新しい右端）を表示する。
fn active_after_close(active: usize, closed: usize, remaining: usize) -> usize {
    if closed < active {
        active - 1
    } else if closed == active {
        closed.min(remaining.saturating_sub(1))
    } else {
        active
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

/// 貼り付けるときにすること。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PasteAction {
    /// 送らずに、編集のダイアログで確かめる。
    Confirm,
    /// 端末に渡す。`allow_unsafe` でなければ、コマンドとして走りうる文字列は端末が送らずに返す。
    Paste { allow_unsafe: bool },
}

/// 確かめ方の設定が `confirm` のときに、`text` を貼り付けるためにすること。
fn paste_action(confirm: PasteConfirm, text: &str) -> PasteAction {
    match confirm {
        PasteConfirm::Unsafe => PasteAction::Paste {
            allow_unsafe: false,
        },
        PasteConfirm::Multiline if !vt::is_safe_to_paste(text) => PasteAction::Confirm,
        PasteConfirm::Multiline => PasteAction::Paste {
            allow_unsafe: false,
        },
        PasteConfirm::Never => PasteAction::Paste { allow_unsafe: true },
    }
}

/// リンクを開く修飾キー。Ctrl（ほかの修飾キーなし）。Ctrl+Alt は配列によって AltGr になるので含めない。
fn is_link_modifier(mods: Mods) -> bool {
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

/// 修飾キーそのもの。押した・離したときに、リンクの下線を出し入れする。
fn is_modifier_key(key: Key) -> bool {
    matches!(
        key,
        Key::ControlLeft
            | Key::ControlRight
            | Key::ShiftLeft
            | Key::ShiftRight
            | Key::AltLeft
            | Key::AltRight
            | Key::MetaLeft
            | Key::MetaRight
    )
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
            let mut close = false;
            let id = SessionId(wparam.0);
            with_app(&mut |app| close = app.on_wake(id));
            if close {
                // DestroyWindow はこの場で WM_DESTROY を送るので、借用を手放してから呼ぶ。
                destroy_window(hwnd);
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
        // 知らせが続けて届くたびにタイマーを掛け直し、止んでから一度だけ読み直す。
        WM_APP_CONFIG_CHANGED => {
            // SAFETY: hwnd は有効な窓。同じ番号のタイマーは掛け直しになる。
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
        WM_TIMER if wparam.0 == BLINK_TIMER_ID => {
            with_app(&mut |app| app.on_blink_timer());
            LRESULT(0)
        }
        // 下位の語が WA_INACTIVE なら、アクティブでなくなった。
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xffff) as u32 != WA_INACTIVE;
            with_app(&mut |app| app.on_activate(active));
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す（フォーカスを窓に移してもらう）。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_TIMER if wparam.0 == CONFIG_TIMER_ID => {
            // SAFETY: hwnd は有効な窓。
            if let Err(e) = unsafe { KillTimer(Some(hwnd), CONFIG_TIMER_ID) } {
                tracing::warn!("failed to stop the settings timer: {e}");
            }
            let mut reloaded = (None, None);
            with_app(&mut |app| reloaded = app.reload_config());
            let (rebuild_dpi, deferred) = reloaded;
            // WM_DPICHANGED をこの場で送るので、借用を手放してから呼ぶ。
            if let Some(dpi) = rebuild_dpi {
                change_dpi_in_place(hwnd, dpi);
            }
            run_deferred(hwnd, deferred, &with_app);
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
        WM_MOUSELEAVE => {
            with_app(&mut |app| app.on_mouse_leave());
            LRESULT(0)
        }
        // Ctrl を離さないまま窓を離れると、離したことが届かないので、リンクの下線を消しておく。
        WM_KILLFOCUS => {
            with_app(&mut |app| app.refresh_hovered_link(false));
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // リンクの上では手の形にする。既定の処理は、マウスが動くたびに窓のクラスの矢印に戻す。
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
        // DPI が変わる前に、変わった後の窓の大きさを問われる。いまの桁数と行数が収まる大きさを返す。
        WM_GETDPISCALEDSIZE => {
            let dpi = wparam.0 as u32;
            let mut scaled = None;
            with_app(&mut |app| match app.scaled_window_size(dpi) {
                Ok(size) => scaled = Some(size),
                Err(e) => tracing::warn!("failed to compute the window size for {dpi} DPI: {e}"),
            });
            let Some(scaled) = scaled else {
                // 0 を返すと、Windows が窓の大きさを DPI の比で拡大する。
                return LRESULT(0);
            };
            // SAFETY: WM_GETDPISCALEDSIZE の lparam は、新しい大きさを書き込む SIZE を指す。
            unsafe { *(lparam.0 as *mut SIZE) = scaled };
            LRESULT(1)
        }
        WM_DPICHANGED => {
            // フォントとアトラスを作り直してから、推奨された位置と大きさに移す。移すとこの場で
            // WM_SIZE が届き、新しいセルの寸法で桁数と行数を計算し直す。
            let dpi = (wparam.0 & 0xffff) as u32;
            with_app(&mut |app| app.apply_dpi(dpi));
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
            // クライアント領域の大きさが変わらず WM_SIZE が届かなかったときも、セルの寸法を合わせる。
            let (width, height) = client_size(hwnd);
            with_app(&mut |app| app.on_resize(width, height));
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: hwnd は有効な窓。
            unsafe {
                _ = KillTimer(Some(hwnd), DUMP_TIMER_ID);
                _ = KillTimer(Some(hwnd), CONFIG_TIMER_ID);
                _ = KillTimer(Some(hwnd), BLINK_TIMER_ID);
            };
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
    fn paste_confirmation_follows_the_setting() {
        let as_is = PasteAction::Paste {
            allow_unsafe: false,
        };
        let unchecked = PasteAction::Paste { allow_unsafe: true };
        // 既定は、端末が bracketed paste のモードを見て断ったときだけ確かめる。
        assert_eq!(paste_action(PasteConfirm::Unsafe, "a\nb"), as_is);
        assert_eq!(
            paste_action(PasteConfirm::Multiline, "a\nb"),
            PasteAction::Confirm
        );
        assert_eq!(
            paste_action(PasteConfirm::Multiline, "a\rb"),
            PasteAction::Confirm
        );
        assert_eq!(paste_action(PasteConfirm::Multiline, "echo hi"), as_is);
        assert_eq!(paste_action(PasteConfirm::Never, "a\nb"), unchecked);
        assert_eq!(paste_action(PasteConfirm::Never, "echo hi"), unchecked);
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
    fn closing_a_tab_shows_its_right_neighbour() {
        // 表示しているタブを閉じたら、右隣が繰り上がる。右端なら新しい右端。
        assert_eq!(active_after_close(1, 1, 3), 1);
        assert_eq!(active_after_close(2, 2, 2), 1);
        assert_eq!(active_after_close(0, 0, 1), 0);
        // 表示していないタブを閉じても、表示しているタブはそのまま。
        assert_eq!(active_after_close(2, 0, 3), 1);
        assert_eq!(active_after_close(0, 2, 2), 0);
    }

    #[test]
    fn command_line_prefers_arguments_then_the_shell_setting() {
        assert_eq!(command_line_for(Some("zsh -l"), Some("cmd.exe")), "zsh -l");
        assert_eq!(command_line_for(None, Some("cmd.exe")), "cmd.exe");
        assert_eq!(command_line_for(None, None), DEFAULT_COMMAND_LINE);
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
