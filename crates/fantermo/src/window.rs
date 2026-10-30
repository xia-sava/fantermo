//! 端末のウィンドウ。Win32 のメッセージを受け、端末の状態・バックエンド・描画をつなぐ。

use std::{
    cell::{Cell, RefCell},
    hash::{BuildHasher, Hasher, RandomState},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, mpsc},
    time::{Duration, Instant, SystemTime},
};

use fantermo_core::{
    backend::{ExitCode, GridSize},
    background::{Candidate, Random},
    clipboard::{ReadDecision, ReadPrompts, ReadRequest, WriteRequest, read_decision},
    config::{
        self, BackgroundConfig, ClipboardConfig, ClipboardRead, ColorsConfig, Config,
        DEFAULT_FONT_FAMILIES, FontConfig, PasteConfig, PasteConfirm, Profile, ScrollBarConfig,
        WideGlyph, WindowConfig,
    },
    frame::Rgb,
    keymap::{self, Keymap},
    link::Link,
    state::State,
    vt::{
        self, Key, KeyAction, KeyInput, Mods, MouseAction, MouseButton, MouseInput, PasteOutcome,
        PointerPosition, ScrollPosition,
    },
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::{
            DirectWrite::IDWriteFactory,
            Gdi::{
                COLOR_BTNFACE, COLOR_BTNSHADOW, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_WINDOW,
                COLOR_WINDOWTEXT, ClientToScreen, GetSysColor, InvalidateRect, SYS_COLOR_INDEX,
                ScreenToClient, ValidateRect,
            },
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            SystemServices::{MK_CONTROL, MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MK_SHIFT},
        },
        UI::{
            Controls::WM_MOUSELEAVE,
            HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow},
            Input::KeyboardAndMouse::{
                GetDoubleClickTime, IsWindowEnabled, ReleaseCapture, SetCapture, TME_LEAVE,
                TRACKMOUSEEVENT, TrackMouseEvent, VK_F4, VK_PROCESSKEY, VK_SPACE,
            },
            WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
                DestroyWindow, DispatchMessageW, FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO,
                FlashWindowEx, GWLP_USERDATA, GetClientRect, GetForegroundWindow, GetMessageW,
                GetWindowLongPtrW, GetWindowRect, HTCLIENT, IDC_ARROW, IDC_HAND, IsWindow,
                KillTimer, LoadCursorW, MSG, PM_REMOVE, PeekMessageW, PostMessageW,
                PostQuitMessage, RegisterClassExW, SPI_GETWHEELSCROLLLINES, SW_SHOWDEFAULT,
                SW_SHOWMAXIMIZED, SWP_NOACTIVATE, SWP_NOZORDER,
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SetCursor, SetTimer,
                SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, SystemParametersInfoW,
                TranslateMessage, WA_INACTIVE, WHEEL_DELTA, WINDOW_EX_STYLE, WINDOW_STYLE,
                WM_ACTIVATE, WM_APP, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_CONTEXTMENU,
                WM_DEADCHAR, WM_DESTROY, WM_DPICHANGED, WM_ENABLE, WM_ENDSESSION,
                WM_GETDPISCALEDSIZE, WM_IME_COMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN,
                WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
                WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WM_QUERYENDSESSION,
                WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SIZE, WM_SYSCHAR, WM_SYSCOLORCHANGE,
                WM_SYSDEADCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WNDCLASSEXW,
                WS_OVERLAPPEDWINDOW,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{
    about, app_icon,
    background::DiskFiles,
    clipboard,
    clipboard_prompt::{self, Choice},
    config_file::{self, Loaded},
    context_menu::{self, Chosen, Menu, MenuCommand, MenuRequest},
    dump::{DUMP_INTERVAL, ScreenDump},
    font::{self, Font},
    ime::{self, CursorRect},
    keyboard::{self, KeyPosition, Utf16Assembler},
    link_opener,
    notice::{self, Topic},
    paint::{Area, Paint, scale},
    paste_editor,
    renderer::{Backdrop, Renderer},
    scroll_bar,
    session::{Session, SessionId, Startup},
    settings_dialog::{self, FontChoice},
    tab_bar::{self, Hit, TabView},
    tab_icon::TabIcon,
    text_dialog,
    ui_font::UiFont,
    update, version, window_state,
};

const WINDOW_CLASS: PCWSTR = w!("fantermo");
const WINDOW_TITLE: &str = "fantermo";

/// ウィンドウのスタイル。
const MAIN_WINDOW_STYLE: WINDOW_STYLE = WS_OVERLAPPEDWINDOW;

/// 端末の出力が届いたときと子プロセスが終了したときに、読むためのスレッドなどがウィンドウのスレッドを
/// 起こすメッセージ。`wparam` はその端末の [`SessionId`]。
const WM_APP_WAKE: u32 = WM_APP + 1;

/// 設定ファイルを監視するスレッドが、書き換えを知らせるメッセージ。
const WM_APP_CONFIG_CHANGED: u32 = WM_APP + 2;

/// 更新の確認かダウンロードをするスレッドが、結果を `App::update_events` に送ったと知らせるメッセージ。
const WM_APP_UPDATE: u32 = WM_APP + 3;

/// クリップボードの読み出しの依頼が確かめを待っているかもしれないことを、ウィンドウに知らせる。
const WM_APP_CLIPBOARD_PROMPT: u32 = WM_APP + 4;

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

/// スクロールバーのつまみの外を押し続けたときに、ページを送り続けるタイマー。
const SCROLL_BAR_TIMER_ID: usize = 4;

/// 端末の中の画像のアニメーションの次のコマで描き直すタイマー。
const IMAGE_ANIMATION_TIMER_ID: usize = 5;

/// 画像のアニメーションで描き直す間隔の下限。
const MIN_IMAGE_ANIMATION_INTERVAL: Duration = Duration::from_millis(8);

/// スクロールバーのつまみの外を押してから、押し続けたとしてページを送り始めるまで。
const SCROLL_BAR_REPEAT_DELAY: Duration = Duration::from_millis(250);

/// スクロールバーのつまみの外を押し続けている間に、ページを送る間隔。
const SCROLL_BAR_REPEAT_INTERVAL: Duration = Duration::from_millis(50);

/// キーボードから開いた WM_CONTEXTMENU が `lparam` に入れてくる位置。
const KEYBOARD_CONTEXT_MENU_POSITION: (i32, i32) = (-1, -1);

/// 起動したときの大きさ（セルの数）。ウィンドウの状態ファイルに大きさを覚えていないときに使う。
const INITIAL_COLS: u16 = 100;
const INITIAL_ROWS: u16 = 30;

/// 開発用。起動したときのウィンドウの DPI を、この環境変数の値とみなす。倍率の違うモニタへ移したときの
/// 作り直しを、モニタを使わずに確かめるために使う。
const DPI_ENV: &str = "FANTERMO_DPI";

/// 新しい端末で起動するもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Launch {
    /// fantermo の引数があればそれ、なければ既定のプロファイル。「+」・キー・メニューの「新しいタブ」。
    Default,
    /// 一覧から選んだ、プロファイルの並びの中の位置がこの値のプロファイル。
    Profile(usize),
}

/// 端末を開くときに起動するもの。`launch` が [`Launch::Default`] なら、fantermo の引数
/// `arguments` があればそのコマンドラインを既定のバックエンドで、なければ既定のプロファイル
/// （`profiles` の `default` 番目）のもの。一覧から選んだプロファイルなら、引数があってもその
/// プロファイルのもの。並びの外なら先頭のもの。プロファイルの相対パスのアイコンは `config_dir` から辿る。
fn startup_for(
    arguments: Option<&str>,
    profiles: &[Profile],
    default: usize,
    launch: Launch,
    config_dir: Option<&Path>,
) -> Startup {
    let index = match (launch, arguments) {
        (Launch::Default, Some(arguments)) => return Startup::conpty(arguments),
        (Launch::Default, None) => default,
        (Launch::Profile(index), _) => index,
    };
    match profiles.get(index).or(profiles.first()) {
        Some(profile) => Startup::from_profile(profile, config_dir),
        None => Startup::conpty(config::DEFAULT_COMMAND_LINE),
    }
}

/// 背景画像を選ぶ乱数の種。起動するたびに変わる。
fn random_seed() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    if let Ok(elapsed) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        hasher.write_u128(elapsed.as_nanos());
    }
    hasher.finish()
}

/// ウィンドウを開き、子プロセスを起動して、ウィンドウが閉じるまでメッセージを処理する。最後の端末の
/// 子プロセスが終わってウィンドウを閉じたときはその終了コードを、利用者が最後の端末を閉じたときは 0 を
/// 返す。`arguments` は fantermo の引数（子プロセスのコマンドライン）。`config_path` は設定ファイルの
/// 場所で、`loaded` はそこから読んだ設定。読んだときの警告は、ウィンドウを出してから見せる。
/// `state_path` はウィンドウの状態ファイルの場所で、`state` はそこから読んだ状態。ウィンドウを見せる前に、
/// 覚えていた位置・大きさに戻し、閉じるときに書く。
pub fn run(
    arguments: Option<String>,
    config_path: Option<PathBuf>,
    loaded: Loaded,
    state_path: Option<PathBuf>,
    state: State,
) -> Result<u32, Box<dyn std::error::Error>> {
    let Loaded {
        config,
        notice,
        written,
    } = loaded;
    let dwrite = font::create_factory()?;
    // SAFETY: 引数のない問い合わせ。
    let system_dpi = unsafe { GetDpiForSystem() };
    let font = load_font(&dwrite, &config.font, system_dpi)?;
    log_font(&font, system_dpi);

    let hwnd = create_window(
        &font,
        system_dpi,
        config.scroll_bar.width,
        config.window.padding,
    )?;
    // 端末の桁数と行数を、戻した大きさから決めさせるため、App を作る前に戻す。
    let restored = state.window;
    if let Some(window) = &restored {
        window_state::restore_hidden(hwnd, window);
    }
    let mut app = App::new(
        hwnd,
        dwrite,
        font,
        system_dpi,
        config_path,
        config,
        arguments,
    )?;
    app.state_path = state_path;
    app.settings_written = written;
    let app = Box::into_raw(Box::new(RefCell::new(app)));
    // SAFETY: hwnd はこのスレッドが作ったウィンドウ。ポインタは WM_NCDESTROY で取り戻して解放する。
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize) };

    // ウィンドウを置いたモニタの DPI がシステムの DPI と違えば、DPI が変わったときと同じように作り直す。
    let window_dpi = dpi_from_env().unwrap_or_else(|| {
        // SAFETY: hwnd は有効なウィンドウ。
        unsafe { GetDpiForWindow(hwnd) }
    });
    if window_dpi != system_dpi {
        change_dpi_in_place(hwnd, window_dpi);
        // 作り直しは桁数と行数を保ってウィンドウの大きさを変えるので、覚えていた大きさに戻し直す。桁数と
        // 行数は、届く WM_SIZE で新しいセルの寸法から決め直す。
        if let Some(window) = &restored {
            window_state::restore_hidden(hwnd, window);
        }
    }

    let show = if restored.is_some_and(|window| window.maximized) {
        SW_SHOWMAXIMIZED
    } else {
        SW_SHOWDEFAULT
    };
    // SAFETY: hwnd は有効なウィンドウ。
    unsafe { _ = ShowWindow(hwnd, show) };
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

fn log_ui_font(font: &UiFont, dpi: u32) {
    tracing::info!(
        "UI font {} {}px at {dpi} DPI: line {} px",
        font.family,
        font.size_px,
        font.height_px()
    );
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

/// ウィンドウの左上を動かさずに、DPI が `dpi` に変わったときの処理（フォントとアトラスの作り直し）を
/// させる。桁数と行数は保つ。DPI が変わらず、フォントの設定が変わったときにも使う。
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
    // SAFETY: hwnd は有効なウィンドウで、rect は書き込める。
    if let Err(e) = unsafe { GetWindowRect(hwnd, &mut rect) } {
        tracing::warn!("failed to get the window position: {e}");
    }
    // 枠の大きさは、いまのウィンドウの枠から取る。DPI_ENV で DPI を差し替えたときも、枠はウィンドウの実際の
    // DPI で付くため。
    let (current_width, current_height) = client_size(hwnd);
    let frame_width = (rect.right - rect.left) - current_width as i32;
    let frame_height = (rect.bottom - rect.top) - current_height as i32;
    rect.right = rect.left + client.cx + frame_width;
    rect.bottom = rect.top + client.cy + frame_height;
    // WM_DPICHANGED の wparam は、下位に横の DPI、上位に縦の DPI。
    let wparam = WPARAM((dpi | (dpi << 16)) as usize);
    // SAFETY: hwnd は有効なウィンドウ。rect は処理が終わるまで生きている。借用は持っていない。
    unsafe {
        SendMessageW(
            hwnd,
            WM_DPICHANGED,
            Some(wparam),
            Some(LPARAM(&raw const rect as isize)),
        )
    };
}

/// Windows のシステムの色。描くたびに問い合わせるので、システムの色が変わればそのまま従う。
fn system_colors() -> tab_bar::SystemColors {
    let color = |index: SYS_COLOR_INDEX| {
        // SAFETY: 決まった番号の色を問い合わせるだけ。
        let colorref = unsafe { GetSysColor(index) };
        // COLORREF は下位のバイトから赤・緑・青。
        Rgb {
            r: (colorref & 0xff) as u8,
            g: ((colorref >> 8) & 0xff) as u8,
            b: ((colorref >> 16) & 0xff) as u8,
        }
    };
    tab_bar::SystemColors {
        face: color(COLOR_BTNFACE),
        text: color(COLOR_BTNTEXT),
        gray_text: color(COLOR_GRAYTEXT),
        shadow: color(COLOR_BTNSHADOW),
        window: color(COLOR_WINDOW),
        window_text: color(COLOR_WINDOWTEXT),
    }
}

/// `font` で `cols` × `rows` のセルを並べた大きさ（ピクセル）。
fn grid_pixels(font: &Font, cols: u16, rows: u16) -> SIZE {
    SIZE {
        cx: (u32::from(cols) * font.cell_width_px) as i32,
        cy: (u32::from(rows) * font.cell_height_px) as i32,
    }
}

/// ウィンドウのクライアント領域のうち、端末の画面の周りに取る場所の寸法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Insets {
    /// 上端のタブバーの高さ（[`tab_bar::height`]）。
    tab_bar_height: u32,
    /// 端末の画面の上・下・左・右の余白。上はタブバーとの間、右はスクロールバーとの間。何も描かず、
    /// 描画が端末の背景色で塗るところに任せる（`Renderer::render` はどの領域にも入らないところを端末の
    /// 背景色で塗る）。
    padding: u32,
    /// 右端のスクロールバーの幅（[`scroll_bar::Metrics::width`]）。
    scroll_bar_width: u32,
}

impl Insets {
    /// DPI が `dpi` のウィンドウでの寸法。`scroll_bar_width` は設定のスクロールバーの幅、`padding` は設定の
    /// 余白（どちらも 96 DPI でのピクセル。DPI に比例させる）。
    fn new(dpi: u32, scroll_bar_width: u32, padding: u32) -> Self {
        Self {
            tab_bar_height: tab_bar::height(dpi),
            // scale は 1 ピクセルより細くしないので、余白なし（0）はそのまま。
            padding: if padding == 0 { 0 } else { scale(padding, dpi) },
            scroll_bar_width: scroll_bar::Metrics {
                dpi,
                base_width: scroll_bar_width,
            }
            .width(),
        }
    }
}

/// 大きさが `width` × `height` のクライアント領域のうち、表示している端末を描く領域。上端のタブバーと
/// 右端のスクロールバーを除き、その内側の上・下・左・右に余白（寸法は `insets`）を空けた残りの全体に
/// 描く。
fn terminal_area_in_client(width: u32, height: u32, insets: Insets) -> Area {
    let top = (insets.tab_bar_height + insets.padding).min(height);
    let available_width = width.saturating_sub(insets.scroll_bar_width);
    Area {
        x: insets.padding.min(available_width),
        y: top,
        width: available_width.saturating_sub(2 * insets.padding),
        height: (height - top).saturating_sub(insets.padding),
    }
}

/// 大きさが `width` のクライアント領域のうち、スクロールバーを描く領域。クライアント領域の右端に幅
/// `insets.scroll_bar_width` で、上下は端末を描く領域 `terminal` に揃える。
fn scroll_bar_area_in_client(width: u32, terminal: Area, insets: Insets) -> Area {
    let x = width.saturating_sub(insets.scroll_bar_width);
    Area {
        x,
        y: terminal.y,
        width: width - x,
        height: terminal.height,
    }
}

/// 大きさが `width` × `height` のクライアント領域のうち、背景画像を敷く領域。上端のタブバーと右端の
/// スクロールバーを除いた残りの全体（端末を描く領域と、その周りの余白）。
fn backdrop_area_in_client(width: u32, height: u32, insets: Insets) -> Area {
    let top = insets.tab_bar_height.min(height);
    Area {
        x: 0,
        y: top,
        width: width.saturating_sub(insets.scroll_bar_width),
        height: height - top,
    }
}

/// 端末を描く領域の大きさが `terminal` になるクライアント領域の大きさ。[`terminal_area_in_client`] と
/// 対になる。
fn client_size_for_terminal(terminal: SIZE, insets: Insets) -> SIZE {
    let padding = 2 * insets.padding as i32;
    SIZE {
        cx: terminal.cx + padding + insets.scroll_bar_width as i32,
        cy: terminal.cy + padding + insets.tab_bar_height as i32,
    }
}

/// 端末を描く領域の左上からの位置 (`x`, `y`) を、大きさが `size` の端末のセルを並べた画面の中の、
/// いちばん近い位置にする。
fn clamp_to_grid(x: i32, y: i32, size: GridSize) -> (i32, i32) {
    let limit = |count: u16, cell_px: u32| (u32::from(count) * cell_px).saturating_sub(1) as i32;
    (
        x.clamp(0, limit(size.cols, size.cell_width_px)),
        y.clamp(0, limit(size.rows, size.cell_height_px)),
    )
}

/// クライアント領域の中の位置を、`area` の左上からの位置にする。
fn client_to_area(area: Area, x: i32, y: i32) -> (i32, i32) {
    (x - area.x as i32, y - area.y as i32)
}

/// `area` の左上からの位置を、クライアント領域の中の位置にする。[`client_to_area`] と対になる。
fn area_to_client(area: Area, x: i32, y: i32) -> (i32, i32) {
    (x + area.x as i32, y + area.y as i32)
}

/// クライアント領域の大きさが `client` になる、DPI が `dpi` のウィンドウの大きさ（枠を含む）。
fn window_size(client: SIZE, dpi: u32) -> windows::core::Result<SIZE> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: client.cx,
        bottom: client.cy,
    };
    // SAFETY: rect は書き込める。
    unsafe {
        AdjustWindowRectExForDpi(&mut rect, MAIN_WINDOW_STYLE, false, WINDOW_EX_STYLE(0), dpi)?
    };
    Ok(SIZE {
        cx: rect.right - rect.left,
        cy: rect.bottom - rect.top,
    })
}

/// ウィンドウを作る。大きさは、`font` のセルを [`INITIAL_COLS`] × [`INITIAL_ROWS`] 並べた端末の画面に、
/// タブバーと幅 `scroll_bar_width` のスクロールバーと余白 `padding`（どちらも設定の値）を足したもの。
fn create_window(
    font: &Font,
    dpi: u32,
    scroll_bar_width: u32,
    padding: u32,
) -> windows::core::Result<HWND> {
    // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
    let instance = unsafe { GetModuleHandleW(None)? };
    // クラスのアイコンはプロセスが終わるまで捨てない。DPI が変わったら WM_SETICON で付け直す。
    let icons = app_icon::load(instance.into(), dpi);
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance.into(),
        hIcon: icons.big.unwrap_or_default(),
        // SAFETY: システムの矢印カーソルを読むだけ。
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpszClassName: WINDOW_CLASS,
        hIconSm: icons.small.unwrap_or_default(),
        ..Default::default()
    };
    // SAFETY: class は初期化済みで、名前は静的な文字列。
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }

    let size = window_size(
        client_size_for_terminal(
            grid_pixels(font, INITIAL_COLS, INITIAL_ROWS),
            Insets::new(dpi, scroll_bar_width, padding),
        ),
        dpi,
    )?;

    // SAFETY: クラスは登録済みで、文字列はこの呼び出しの間は生きている。
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            &HSTRING::from(window_title("", &version::display())),
            MAIN_WINDOW_STYLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            size.cx,
            size.cy,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }?;
    Ok(hwnd)
}

fn message_loop() -> u32 {
    let mut msg = MSG::default();
    // SAFETY: msg は書き込める。ウィンドウは問わず、このスレッドのメッセージを取る。
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        // SAFETY: msg は GetMessageW が埋めたもの。
        unsafe {
            _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    msg.wParam.0 as u32
}

/// 入力の処理のあとで、ウィンドウの借用を手放してからすること。ダイアログやメニューは自分でメッセージを
/// 回すので、借用を持ったまま開くと、その間に届いたメッセージの処理が飛ばされる。
enum Deferred {
    /// 改行を含む貼り付けの内容を、編集のダイアログで確かめる。
    EditPaste { text: String, font: Font },
    /// 「fantermo について」のダイアログを開く。`font` は端末のフォント。
    ShowAbout { font: Font },
    /// 設定ダイアログを開く。
    EditSettings,
    /// コンテキストメニューか、タブバーのメニューのボタンのメニューを出す。
    Menu(MenuRequest),
    /// ウィンドウを閉じる。最後の端末を閉じたときと、メニューで終了を選んだときに使う。
    CloseWindow,
    /// `topic` についての警告 `text` を見せる。
    Warn { topic: Topic, text: String },
    /// 更新のスレッドの結果を見せ、次にすることを決める。
    Update(update::Event),
    /// クリップボードの読み出しの依頼を、本文 `text` のダイアログで確かめる。
    AskClipboardRead { text: String },
}

/// スクロールバーの上で左ボタンを押してから離すまでの操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScrollBarDrag {
    /// つまみをドラッグしている。`grab` は、押した位置のつまみの上端からの高さ。動かしても、つまみの
    /// 中の押した位置をマウスの位置に保つ。
    Thumb { grab: i32 },
    /// つまみの外を押し続けている。つまみがマウスの位置 `y` に来るまで、`direction` へページを送る。
    Page {
        direction: scroll_bar::Direction,
        y: i32,
    },
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
    /// タブバーのタイトルの書体（Windows の UI のフォント）。
    ui_font: Rc<UiFont>,
    /// いまのウィンドウの DPI。
    dpi: u32,
    /// 設定ファイルの場所。
    config_path: Option<PathBuf>,
    /// ウィンドウの状態ファイルの場所。ウィンドウを閉じるときと、ログオフ・シャットダウンのときに書く。
    /// `None` なら書かない。
    state_path: Option<PathBuf>,
    /// `font` を探したときの設定。
    font_config: FontConfig,
    /// 貼り付ける前に、編集のダイアログで確かめるとき。
    paste_confirm: PasteConfirm,
    /// アプリがクリップボードの内容を読み出そうとしたときにどうするか。
    clipboard_read: ClipboardRead,
    /// 確かめを待っているクリップボードの読み出しの依頼。
    clipboard_prompts: ReadPrompts<SessionId>,
    /// fantermo の引数（子プロセスのコマンドライン）。
    arguments: Option<String>,
    /// 設定のプロファイル。少なくとも一つある。
    profiles: Vec<Profile>,
    /// 設定の既定のプロファイルの名前。`None` なら先頭。
    default_profile: Option<String>,
    /// ウィンドウが持つ端末。タブの並びの順。少なくとも一つある。
    sessions: Vec<Session>,
    /// 表示している端末の、`sessions` の中の位置。
    active: usize,
    /// 次に開く端末の番号。閉じた端末の番号は使い直さない（閉じたあとに届く知らせを捨てるため）。
    next_session_id: usize,
    /// ウィンドウのクライアント領域のうち、表示している端末を描く領域。
    terminal_area: Area,
    renderer: Renderer,
    dump: Option<ScreenDump>,
    /// ウィンドウのタイトルに出している文字列。
    window_title: String,
    /// 最後の端末が終わったときの、その子プロセスの終了コード。ウィンドウを閉じたあとの終了コードにする。
    exit: Option<ExitCode>,
    /// キーを伴わずに届く文字（IME の確定など）を組み立てる。
    text_input: Utf16Assembler,
    /// ホイール 1 ノッチで動かす行数（Windows の設定）。
    wheel_lines_per_notch: u32,
    /// ダブルクリックの判定と画像のアニメーションに使う時計。
    clock: Instant,
    /// 画像のアニメーションの次のコマで描き直すタイマーが動いている。
    image_animation_timer_running: bool,
    /// マウスの、ウィンドウのクライアント領域の中の位置。ウィンドウの外へ出たら `None`。
    pointer_in_window: Option<(i32, i32)>,
    /// fantermo が受け持つキーの割り当て。
    keymap: Keymap,
    /// 設定で書いたタブバーとスクロールバーの色。
    colors: ColorsConfig,
    /// 背景画像の設定。
    background: BackgroundConfig,
    /// 背景画像を選ぶ乱数。
    random: Random,
    /// ウィンドウのクライアント領域のうち、背景画像を敷く領域。
    backdrop_area: Area,
    /// 割り当ての動作に使ったので、離したときにも端末へ送らないキー。タブを切り替えたあとに
    /// 離したことが届くので、端末ごとではなくウィンドウに持つ。
    suppressed_release: Option<Key>,
    /// タブバーの上で押したマウスのボタンと、そのときマウスを載せていたもの。離すまで、マウスの
    /// 操作を端末へ送らない。
    tab_bar_press: Option<(MouseButton, Option<Hit>)>,
    /// タブバーの上の、マウスを載せているもの。変わったときだけタブバーを描き直す。
    tab_bar_hover: Option<Hit>,
    /// ウィンドウがアクティブ（前面）。WM_ACTIVATE で決める。
    window_active: bool,
    /// 点滅の文字を見せている。点滅させていないときは `true`。
    blink_visible: bool,
    /// 点滅のタイマーを動かしている。
    blink_timer_running: bool,
    /// 設定のスクロールバーの幅（96 DPI でのピクセル）。
    scroll_bar_width: u32,
    /// 設定の端末の画面の余白（96 DPI でのピクセル）。
    padding: u32,
    /// ウィンドウのクライアント領域のうち、スクロールバーを描く領域。
    scroll_bar_area: Area,
    /// スクロールバーの上で押したマウスのボタンと、左ボタンならその操作。離すまで、マウスの操作を
    /// 端末へ送らない。
    scroll_bar_press: Option<(MouseButton, Option<ScrollBarDrag>)>,
    /// マウスがスクロールバーの上にある。変わったときだけ描き直す。
    scroll_bar_hover: bool,
    /// 表示している端末の、最後に描いたときの表示している範囲の位置。変わったら描き直す。
    scroll_position: Option<ScrollPosition>,
    /// 設定ダイアログを開いている。開いている間は設定ファイルを読み直さない（ダイアログで編集中の
    /// 設定を、書く前の中身で上書きしないため）。
    settings_dialog_open: bool,
    /// 設定ダイアログを開いている間に、設定ファイルが書き換えられた。閉じたら読み直す。
    reload_pending: bool,
    /// 設定ダイアログか、古い書き方の読み替えが最後に設定ファイルに書いた中身。読み直すときに中身が
    /// これと同じなら、使っている設定と同じなので読まない。違えば捨てる。
    settings_written: Option<String>,
    /// 更新の確認かダウンロードを別のスレッドでしているか、その結果を見せている。その間は新しく始めない。
    update_busy: bool,
    /// 更新のスレッドが結果を送るチャネルの送信側と、ウィンドウが受け取る受信側。送ったら
    /// `WM_APP_UPDATE` で知らせる。
    update_sender: mpsc::Sender<update::Event>,
    update_events: mpsc::Receiver<update::Event>,
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
        let scroll_bar_width = config.scroll_bar.width;
        let padding = config.window.padding;
        let insets = Insets::new(dpi, scroll_bar_width, padding);
        let terminal_area = terminal_area_in_client(width, height, insets);
        let scroll_bar_area = scroll_bar_area_in_client(width, terminal_area, insets);
        let backdrop_area = backdrop_area_in_client(width, height, insets);
        let size = grid_size(&font, terminal_area);
        let ui_font = Rc::new(UiFont::for_dpi(&dwrite, dpi)?);
        log_ui_font(&ui_font, dpi);
        let renderer = Renderer::new(
            hwnd,
            width,
            height,
            &dwrite,
            &font,
            ui_font.clone(),
            font_config.wide_glyph,
        )?;
        let first_id = SessionId(0);
        let session = Session::spawn(
            first_id,
            &startup_for(
                arguments.as_deref(),
                &config.profiles,
                config::default_profile_index(&config.profiles, config.default_profile.as_deref()),
                Launch::Default,
                config_path.as_deref().and_then(Path::parent),
            ),
            size,
            &config.colors.terminal_colors(),
            Arc::new(poster(hwnd, WM_APP_WAKE, WPARAM(first_id.0))),
        )?;
        if let Some(path) = &config_path {
            config_file::watch(path, poster(hwnd, WM_APP_CONFIG_CHANGED, WPARAM(0)));
        }

        let dump = ScreenDump::from_env();
        if dump.is_some() {
            // SAFETY: hwnd は有効なウィンドウ。
            unsafe {
                SetTimer(
                    Some(hwnd),
                    DUMP_TIMER_ID,
                    DUMP_INTERVAL.as_millis() as u32,
                    None,
                )
            };
        }
        let (update_sender, update_events) = mpsc::channel();

        let mut app = Self {
            hwnd,
            dwrite,
            font,
            ui_font,
            dpi,
            config_path,
            state_path: None,
            font_config,
            paste_confirm: config.paste.confirm,
            clipboard_read: config.clipboard.read,
            clipboard_prompts: ReadPrompts::default(),
            arguments,
            profiles: config.profiles,
            default_profile: config.default_profile,
            sessions: vec![session],
            active: 0,
            next_session_id: first_id.0 + 1,
            terminal_area,
            renderer,
            dump,
            window_title: window_title("", &version::display()),
            exit: None,
            text_input: Utf16Assembler::default(),
            wheel_lines_per_notch: wheel_lines_per_notch(),
            clock: Instant::now(),
            image_animation_timer_running: false,
            pointer_in_window: None,
            keymap: config.keys,
            colors: config.colors,
            background: config.background,
            random: Random::new(random_seed()),
            backdrop_area,
            suppressed_release: None,
            tab_bar_press: None,
            tab_bar_hover: None,
            window_active: false,
            blink_visible: true,
            blink_timer_running: false,
            scroll_bar_width,
            padding,
            scroll_bar_area,
            scroll_bar_press: None,
            scroll_bar_hover: false,
            scroll_position: None,
            settings_dialog_open: false,
            reload_pending: false,
            settings_written: None,
            update_busy: false,
            update_sender,
            update_events,
        };
        app.sessions[0].background_image = app.choose_background_image();
        Ok(app)
    }

    /// 設定の背景画像の一覧から、ランダムに一枚選んで読む。読めない画像は警告を残して飛ばす。一覧が
    /// 空か、どれも読めなければ `None`。
    fn choose_background_image(&mut self) -> Option<Candidate> {
        if self.background.images.is_empty() {
            return None;
        }
        let base = self.config_path.as_deref().and_then(Path::parent);
        let expanded = fantermo_core::background::expand(&self.background.images, base, &DiskFiles);
        for warning in &expanded.warnings {
            tracing::warn!("{warning}");
        }
        let mut candidates = expanded.candidates;
        let renderer = &mut self.renderer;
        let random = &mut self.random;
        let picked = fantermo_core::background::pick(
            &mut candidates,
            &mut |count| random.below(count),
            |candidate| match renderer.load_image(&candidate.path) {
                Ok(()) => Some(()),
                Err(e) => {
                    tracing::warn!(
                        "cannot read the background image {}: {e}",
                        candidate.path.display()
                    );
                    None
                }
            },
        );
        if picked.is_none() {
            tracing::warn!("none of the background images can be read");
        }
        picked.map(|(candidate, ())| candidate)
    }

    /// すべての端末の背景画像を選び直す。
    fn rechoose_background_images(&mut self) {
        for index in 0..self.sessions.len() {
            self.sessions[index].background_image = self.choose_background_image();
        }
        self.drop_unused_background_images();
        self.invalidate();
    }

    /// どの端末も使っていない背景画像を捨てる。
    fn drop_unused_background_images(&mut self) {
        let sessions = &self.sessions;
        self.renderer.retain_images(|path| {
            sessions.iter().any(|session| {
                session
                    .background_image
                    .as_ref()
                    .is_some_and(|image| image.path == path)
            })
        });
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
            // ここから下は、押し続けたときの繰り返しもシェルへ送らない。貼り付け・メニュー・開く・
            // 閉じるは繰り返さず、次・前は繰り返す。
            keymap::Action::Paste => Some((!repeated).then(|| self.paste_clipboard()).flatten()),
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

    /// タブの操作をする。最後の端末を閉じたときは、ウィンドウを閉じるよう返す。
    fn run_tab_command(&mut self, command: TabCommand) -> Option<Deferred> {
        let count = self.sessions.len();
        match command {
            TabCommand::New => self.open_session(Launch::Default),
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

    /// `launch` の新しい端末を開き、タブの並びの最後に足して表示する。起動できなければ警告を残して
    /// 何もしない。
    fn open_session(&mut self, launch: Launch) {
        let id = SessionId(self.next_session_id);
        let startup = startup_for(
            self.arguments.as_deref(),
            &self.profiles,
            self.default_profile_index(),
            launch,
            self.config_path.as_deref().and_then(Path::parent),
        );
        let size = grid_size(&self.font, self.terminal_area);
        let mut session = match Session::spawn(
            id,
            &startup,
            size,
            &self.colors.terminal_colors(),
            Arc::new(poster(self.hwnd, WM_APP_WAKE, WPARAM(id.0))),
        ) {
            Ok(session) => session,
            Err(e) => {
                tracing::warn!(
                    "failed to open a terminal with {:?} ({}): {e}",
                    startup.command_line,
                    startup.backend.name()
                );
                return;
            }
        };
        self.next_session_id += 1;
        session.background_image = self.choose_background_image();
        self.sessions.push(session);
        self.activate(self.sessions.len() - 1);
    }

    /// `sessions` の `index` の端末を閉じる（子プロセスはバックエンドを破棄して終わらせる）。最後の
    /// 端末なら閉じずに `true` を返す（ウィンドウを閉じると一緒に閉じる）。
    fn close_session(&mut self, index: usize) -> bool {
        if self.sessions.len() == 1 {
            return true;
        }
        let was_active = index == self.active;
        if was_active {
            self.leave_active();
        }
        let closed = self.sessions.remove(index);
        if self.clipboard_prompts.discard_tab(closed.id()) {
            clipboard_prompt::close();
        }
        self.renderer.drop_terminal_images(closed.id().0);
        drop(closed);
        self.drop_unused_background_images();
        let sessions = &self.sessions;
        self.renderer
            .retain_tab_icons(|icon| sessions.iter().any(|session| session.tab_icon == *icon));
        self.active = active_after_close(self.active, index, self.sessions.len());
        if was_active {
            self.on_activated();
        }
        // タブバーのタブが減り、マウスを載せているものも変わりうる。
        self.update_tab_bar_hover();
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
        if self.session().selecting || self.scroll_bar_press.is_some() {
            self.end_scroll_bar_press();
            // SAFETY: このスレッドがキャプチャしたマウスを放す。キャプチャしていなければ何もしない。
            if let Err(e) = unsafe { ReleaseCapture() } {
                tracing::warn!("failed to release the mouse capture: {e}");
            }
        }
        self.session_mut().release_pointer();
    }

    /// 表示する端末が替わった。ウィンドウのタイトルとマウスを載せているリンクとタブバーのものを合わせ、
    /// 描き直す。
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
        // タブが入りきらないときは、表示するタブでタブの並びがずれる。
        self.update_tab_bar_hover();
        self.invalidate();
    }

    /// クリップボードの文字列を貼り付ける。設定の `paste.confirm` に従って確かめが要るときは、
    /// 編集のダイアログを開くよう返す。
    fn paste_clipboard(&mut self) -> Option<Deferred> {
        let text = match clipboard::text(self.hwnd) {
            Ok(Some(text)) => text_dialog::from_crlf(&text),
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

    /// `text` を貼り付ける。`allow_unsafe` でなければ、コマンドとして実行されうる文字列は送らない。
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

    /// 選択範囲をクリップボードにコピーし、選択を外す。選んでいなければ何もせず `false`。
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

    /// `sessions` の `index` の端末のアプリからの、クリップボードの書き込みと読み出しの依頼を処理する。
    fn on_clipboard_requests(&mut self, index: usize) {
        for write in self.sessions[index].vt.take_clipboard_writes() {
            let result = match write {
                WriteRequest::Set(text) => clipboard::set_text(self.hwnd, &text),
                WriteRequest::Clear => clipboard::clear(self.hwnd),
            };
            if let Err(e) = result {
                tracing::warn!("failed to write to the clipboard for an application: {e}");
            }
        }
        let id = self.sessions[index].id();
        for request in self.sessions[index].vt.take_clipboard_reads() {
            let allowed = self.sessions[index].clipboard_read_allowed;
            match read_decision(self.clipboard_read, allowed) {
                ReadDecision::Answer => self.answer_clipboard_read(id, request),
                ReadDecision::Refuse => tracing::debug!("refused a clipboard read from {id:?}"),
                ReadDecision::Ask => {
                    tracing::debug!("a clipboard read from {id:?} waits to be asked about");
                    self.clipboard_prompts.push(id, request);
                    self.post_clipboard_prompt();
                }
            }
        }
    }

    /// 確かめを待っている依頼を見るよう、ウィンドウに知らせる。
    fn post_clipboard_prompt(&self) {
        poster(self.hwnd, WM_APP_CLIPBOARD_PROMPT, WPARAM(0))();
    }

    /// いまのクリップボードの文字列を、端末 `id` のアプリへ答える。文字列がなければ空で答える。
    fn answer_clipboard_read(&mut self, id: SessionId, request: ReadRequest) {
        let text = match clipboard::text(self.hwnd) {
            Ok(text) => text.as_deref().map(text_dialog::from_crlf),
            Err(e) => {
                tracing::warn!("failed to read the clipboard for an application: {e}");
                None
            }
        };
        let reply = request.reply(text.as_deref().unwrap_or_default());
        match self.sessions.iter_mut().find(|session| session.id() == id) {
            Some(session) => session.write(&reply),
            None => tracing::debug!("dropping a clipboard answer for a closed terminal {id:?}"),
        }
    }

    /// 次に確かめる依頼があれば、ダイアログを開くよう返す。ほかのダイアログを開いている間（ウィンドウが無効の
    /// 間）は開かず、有効に戻ったときに見直す。待っている間に設定やタブの許可が変わったら、それに従って片づける。
    fn next_clipboard_prompt(&mut self) -> Option<Deferred> {
        // SAFETY: ウィンドウのハンドルを調べるだけ。
        if !unsafe { IsWindowEnabled(self.hwnd) }.as_bool() {
            return None;
        }
        while let Some((id, request)) = self.clipboard_prompts.start_next() {
            let index = self.sessions.iter().position(|session| session.id() == id);
            let allowed = index.is_some_and(|index| self.sessions[index].clipboard_read_allowed);
            match read_decision(self.clipboard_read, allowed) {
                ReadDecision::Answer => self.answer_clipboard_read(id, request),
                ReadDecision::Refuse => tracing::debug!("refused a clipboard read from {id:?}"),
                ReadDecision::Ask => {
                    if let Some(index) = index {
                        tracing::debug!("asking about a clipboard read from {id:?}");
                        let session = &self.sessions[index];
                        return Some(Deferred::AskClipboardRead {
                            text: clipboard_prompt::request_text(
                                index + 1,
                                session.profile_name.as_deref(),
                                index == self.active,
                            ),
                        });
                    }
                }
            }
            self.clipboard_prompts.finish();
        }
        None
    }

    /// 確認のダイアログの答え `choice` に従う。ダイアログの間にタブを閉じていれば、何もしない。
    fn finish_clipboard_prompt(&mut self, choice: Choice) {
        if let Some((id, request)) = self.clipboard_prompts.finish() {
            tracing::debug!("the user answered a clipboard read from {id:?}: {choice:?}");
            let allows = choice != Choice::Deny && self.clipboard_read != ClipboardRead::Deny;
            if allows {
                if choice == Choice::AllowForTab
                    && let Some(session) =
                        self.sessions.iter_mut().find(|session| session.id() == id)
                {
                    session.clipboard_read_allowed = true;
                }
                self.answer_clipboard_read(id, request);
            }
        }
        if self.clipboard_prompts.has_waiting() {
            self.post_clipboard_prompt();
        }
    }

    /// ウィンドウのクライアント領域の中の位置を、端末を描く領域の左上からの位置にする。
    fn client_to_terminal(&self, x: i32, y: i32) -> (i32, i32) {
        client_to_area(self.terminal_area, x, y)
    }

    /// 端末を描く領域の左上からの位置を、ウィンドウのクライアント領域の中の位置にする。
    fn terminal_to_client(&self, x: i32, y: i32) -> (i32, i32) {
        area_to_client(self.terminal_area, x, y)
    }

    /// ウィンドウのクライアント領域の中の位置を、端末を描く領域の左上からの位置にする。クライアント領域の
    /// 中でセルの外（端末の画面の余白など）なら、いちばん近いセルの中の位置にする。クライアント領域の外
    /// （ドラッグでウィンドウの外へ出たとき）はそのまま。マウスの選択とアプリへの報告に使う。
    fn pointer_in_terminal(&self, x: i32, y: i32) -> (i32, i32) {
        let (terminal_x, terminal_y) = self.client_to_terminal(x, y);
        let (width, height) = client_size(self.hwnd);
        if x < 0 || y < 0 || x as u32 >= width || y as u32 >= height {
            return (terminal_x, terminal_y);
        }
        clamp_to_grid(terminal_x, terminal_y, self.session().vt.size())
    }

    /// ウィンドウのクライアント領域の中の位置を、表示している範囲のセルに直す。端末を描く領域の外は端の
    /// セルにする。
    fn pointer(&self, x: i32, y: i32) -> PointerPosition {
        let (x, y) = self.pointer_in_terminal(x, y);
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

    /// ウィンドウのクライアント領域の中の位置にあるリンク。
    fn link_at(&self, x: i32, y: i32) -> Option<Link> {
        let (x, y) = self.client_to_terminal(x, y);
        let size = self.session().vt.size();
        // セルの外（端末の画面の余白、スクロールバー）にはリンクはない。
        if x < 0
            || y < 0
            || x as u32 >= u32::from(size.cols) * size.cell_width_px
            || y as u32 >= u32::from(size.rows) * size.cell_height_px
        {
            return None;
        }
        let col = (x as u32 / size.cell_width_px).min(u32::from(u16::MAX)) as u16;
        let row = (y as u32 / size.cell_height_px).min(u32::from(u16::MAX)) as u16;
        self.session().vt.link_at(col, row).unwrap_or_else(|e| {
            tracing::warn!("failed to look for a link: {e}");
            None
        })
    }

    /// マウスを載せているリンクを探し直す。`modifier_held` はリンクを開く修飾キー（Ctrl）を押している
    /// か。変わったら、下線を引き直し、カーソルの形を変える。
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

    /// 修飾キーをいま押しているかで、マウスを載せているリンクを探し直す。画面の中身や表示している
    /// 範囲が変わったときに使う。
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
    fn on_mouse_leave(&mut self) {
        self.pointer_in_window = None;
        self.refresh_hovered_link(false);
        self.update_tab_bar_hover();
        self.update_scroll_bar_hover();
    }

    /// マウスがスクロールバーの上にあるかを決め直す。変わったら描き直す。
    fn update_scroll_bar_hover(&mut self) {
        let hover = self
            .pointer_in_window
            .is_some_and(|(x, y)| self.scroll_bar_area.contains(x, y));
        if hover == self.scroll_bar_hover {
            return;
        }
        tracing::debug!("pointer on the scroll bar: {hover}");
        self.scroll_bar_hover = hover;
        self.invalidate();
    }

    /// 表示している範囲を示す、いまのつまみ。スクロールバックがなければ `None`。
    fn scroll_bar_thumb(&mut self) -> Option<scroll_bar::Thumb> {
        self.update_scroll_position();
        scroll_bar::thumb_in(
            self.scroll_bar_area,
            self.scroll_bar_metrics(),
            self.scroll_position?,
        )
    }

    /// スクロールバーの上で左ボタンを押した。つまみの上ならドラッグを始め、つまみの外なら、その向きへ
    /// 表示している行数だけ動かして、押し続けたら送り続けるタイマーを掛ける。アプリがマウスを
    /// 求めていても、表示する範囲を動かす。
    fn press_scroll_bar(&mut self, y: i32) -> Option<ScrollBarDrag> {
        let thumb = self.scroll_bar_thumb()?;
        match scroll_bar::part_at(thumb, y) {
            scroll_bar::Part::Thumb => Some(ScrollBarDrag::Thumb {
                grab: y - thumb.top as i32,
            }),
            scroll_bar::Part::Page(direction) => {
                self.scroll_page(direction);
                self.set_scroll_bar_timer(SCROLL_BAR_REPEAT_DELAY);
                Some(ScrollBarDrag::Page { direction, y })
            }
        }
    }

    /// 表示している行数だけ `direction` へ動かす。
    fn scroll_page(&mut self, direction: scroll_bar::Direction) {
        let page = self.session().vt.size().rows as isize;
        self.session_mut().vt.scroll(direction.sign() * page);
        self.on_viewport_moved();
    }

    /// スクロールバーの上で押したまま、マウスが高さ `y` へ動いた。つまみをドラッグしていれば、押した
    /// 位置がマウスの位置に来るようにつまみを動かし、その位置の行を表示する。
    fn drag_scroll_bar(&mut self, y: i32) {
        let Some((button, Some(drag))) = self.scroll_bar_press else {
            return;
        };
        match drag {
            ScrollBarDrag::Thumb { grab } => {
                let Some(thumb) = self.scroll_bar_thumb() else {
                    return;
                };
                let Some(position) = self.scroll_position else {
                    return;
                };
                let track = scroll_bar::track(self.scroll_bar_area, self.scroll_bar_metrics());
                let row = scroll_bar::row_at(track, thumb.length, position, y - grab);
                if row == position.offset {
                    return;
                }
                self.session_mut().vt.scroll_to_row(row);
                self.on_viewport_moved();
            }
            ScrollBarDrag::Page { direction, .. } => {
                self.scroll_bar_press = Some((button, Some(ScrollBarDrag::Page { direction, y })));
            }
        }
    }

    /// つまみの外を押し続けている間のタイマーが来た。つまみがまだマウスの位置に来ていなければ、もう
    /// 1 ページ動かす。
    fn on_scroll_bar_timer(&mut self) {
        let Some((_, Some(ScrollBarDrag::Page { direction, y }))) = self.scroll_bar_press else {
            self.kill_scroll_bar_timer();
            return;
        };
        self.set_scroll_bar_timer(SCROLL_BAR_REPEAT_INTERVAL);
        let Some(thumb) = self.scroll_bar_thumb() else {
            return;
        };
        if scroll_bar::part_at(thumb, y) == scroll_bar::Part::Page(direction) {
            self.scroll_page(direction);
        }
    }

    /// スクロールバーの上で押した操作を終える。
    fn end_scroll_bar_press(&mut self) {
        if self.scroll_bar_press.take().is_none() {
            return;
        }
        self.kill_scroll_bar_timer();
        self.invalidate();
    }

    fn set_scroll_bar_timer(&self, delay: Duration) {
        // SAFETY: self.hwnd は有効なウィンドウ。同じ番号のタイマーは掛け直しになる。
        if unsafe {
            SetTimer(
                Some(self.hwnd),
                SCROLL_BAR_TIMER_ID,
                delay.as_millis() as u32,
                None,
            )
        } == 0
        {
            tracing::warn!(
                "failed to start the scroll bar timer: {}",
                windows::core::Error::from_thread()
            );
        }
    }

    fn kill_scroll_bar_timer(&self) {
        // SAFETY: self.hwnd は有効なウィンドウ。掛けていなければ失敗するだけ。
        if unsafe { KillTimer(Some(self.hwnd), SCROLL_BAR_TIMER_ID) }.is_err() {
            tracing::debug!("the scroll bar timer was not running");
        }
    }

    /// タブバーの上の、マウスを載せているものを決め直す。変わったら描き直す。マウスの位置やタブの
    /// 並びが変わったときに使う。
    fn update_tab_bar_hover(&mut self) {
        let hover = self
            .pointer_in_window
            .and_then(|(x, y)| self.tab_bar_hit(x, y))
            .flatten();
        if hover == self.tab_bar_hover {
            return;
        }
        tracing::debug!("tab bar under the pointer: {hover:?}");
        self.tab_bar_hover = hover;
        self.invalidate();
    }

    /// マウスのボタンを押した。Ctrl を押しながらリンクの上で左ボタンを押したら、離すまで何もしない
    /// （アプリへも送らず、選択も始めない）。そうでなければ、アプリがマウスを求めていればアプリへ送り、
    /// 求めていなければ左ボタンで選択を始める。Shift を押していれば、アプリが求めていても選択にする。
    fn on_button_down(&mut self, button: MouseButton, lparam: LPARAM, wparam: WPARAM) {
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

    fn on_mouse_move(&mut self, lparam: LPARAM, wparam: WPARAM) {
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
    fn on_button_up(
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
            // SAFETY: このスレッドがキャプチャしたマウスを放す。
            if let Err(e) = unsafe { ReleaseCapture() } {
                tracing::warn!("failed to release the mouse capture: {e}");
            }
        }
        deferred
    }

    /// ウィンドウのクライアント領域の中の (`x`, `y`) がタブバーの上なら、そこにあるもの（タブの間や右の
    /// 空きなら `None`）を `Some` で返す。タブバーの外なら `None`。
    fn tab_bar_hit(&self, x: i32, y: i32) -> Option<Option<Hit>> {
        self.tab_bar_area()
            .contains(x, y)
            .then(|| self.tab_bar_layout().hit(x, y))
    }

    /// タブバーの並べ方。
    fn tab_bar_layout(&self) -> tab_bar::Layout {
        tab_bar::layout(
            tab_bar::Metrics { dpi: self.dpi },
            self.tab_bar_area().width,
            self.sessions.len(),
            self.active,
        )
    }

    /// タブバーの上で押した `button` を (`x`, `y`) で離した。押したときにマウスを載せていたものは
    /// `pressed`。
    /// 左ボタンは押したのと同じボタンの上で離したら、閉じるボタンならそのタブを閉じ、新しいタブのボタン
    /// なら新しいタブを開き、プロファイルの一覧のボタンなら一覧を出すよう返す（タブは押したときに
    /// 切り替えてある）。中ボタンを同じタブの上で離したらそのタブを閉じる。右ボタンは、新しいタブと
    /// プロファイルの一覧のボタンの上で押して離したら一覧を、ほかはコンテキストメニューを出すよう返す。
    /// 最後のタブを閉じたら、ウィンドウを閉じるよう返す。
    fn on_tab_bar_click(
        &mut self,
        button: MouseButton,
        pressed: Option<Hit>,
        x: i32,
        y: i32,
    ) -> Option<Deferred> {
        let released = self.tab_bar_hit(x, y).flatten();
        match button {
            MouseButton::Left => {
                // 押している色を戻す。
                self.invalidate();
                if released != pressed {
                    return None;
                }
                match pressed? {
                    Hit::Close(index) => self.close_session(index).then_some(Deferred::CloseWindow),
                    Hit::NewTab => {
                        self.open_session(Launch::Default);
                        None
                    }
                    Hit::Profiles => Some(self.profile_menu()),
                    Hit::Menu => Some(self.main_menu()),
                    Hit::Tab(_) => None,
                }
            }
            MouseButton::Middle => {
                let index = pressed?.tab()?;
                if released.and_then(Hit::tab) != Some(index) {
                    return None;
                }
                self.close_session(index).then_some(Deferred::CloseWindow)
            }
            // 新しいタブのボタンとプロファイルの一覧のボタンの右クリックは、プロファイルの一覧。
            MouseButton::Right
                if matches!(pressed, Some(Hit::NewTab | Hit::Profiles)) && released == pressed =>
            {
                Some(self.profile_menu())
            }
            MouseButton::Right => Some(self.context_menu(x, y)),
            _ => None,
        }
    }

    /// 既定のプロファイルの、`profiles` の中の位置。
    fn default_profile_index(&self) -> usize {
        config::default_profile_index(&self.profiles, self.default_profile.as_deref())
    }

    /// タブバーの新しいタブのボタンとプロファイルの一覧のボタンの下に、ボタンを隠さずにプロファイルの
    /// 一覧を出すよう返す。
    fn profile_menu(&mut self) -> Deferred {
        let (new_tab, profiles) = self.tab_bar_layout().new_tab_buttons();
        let side = tab_bar::tab_icon_size(self.dpi);
        let config_dir = self.config_path.as_deref().and_then(Path::parent);
        let renderer = &mut self.renderer;
        let profile_icons = self
            .profiles
            .iter()
            .map(|profile| {
                let icon = TabIcon::for_profile(profile, config_dir);
                match renderer.tab_icon_pixels(&icon, side) {
                    Ok(pixels) => Some(pixels),
                    Err(e) => {
                        tracing::warn!("failed to make the icon of {:?}: {e}", profile.name);
                        None
                    }
                }
            })
            .collect();
        let (left, top) = self.client_to_screen(new_tab.x as i32, new_tab.y as i32);
        let (right, bottom) =
            self.client_to_screen(profiles.right() as i32, profiles.bottom() as i32);
        Deferred::Menu(MenuRequest {
            profiles: self
                .profiles
                .iter()
                .map(|profile| profile.name.clone())
                .collect(),
            default_profile: self.default_profile_index(),
            profile_icons,
            exclude: Some(RECT {
                left,
                top,
                right,
                bottom,
            }),
            x: left,
            y: bottom,
            ..self.menu_request(Menu::Profiles)
        })
    }

    /// `menu` を出す要求。位置はウィンドウの左上で、隠さない矩形はない。
    fn menu_request(&self, menu: Menu) -> MenuRequest {
        MenuRequest {
            keys: self.keymap.clone(),
            menu,
            x: 0,
            y: 0,
            exclude: None,
            can_copy: self.has_selection(),
            can_open_settings: self.config_path.is_some(),
            profiles: Vec::new(),
            default_profile: 0,
            profile_icons: Vec::new(),
        }
    }

    /// ウィンドウのクライアント領域の中の (`x`, `y`) にコンテキストメニューを出すよう返す。
    fn context_menu(&self, x: i32, y: i32) -> Deferred {
        let (screen_x, screen_y) = self.client_to_screen(x, y);
        Deferred::Menu(MenuRequest {
            x: screen_x,
            y: screen_y,
            ..self.menu_request(Menu::Context)
        })
    }

    /// タブバーのメニューのボタンの下に、ボタンを隠さずにメニューを出すよう返す。
    fn main_menu(&self) -> Deferred {
        let button = self.tab_bar_layout().menu();
        let (left, top) = self.client_to_screen(button.x as i32, button.y as i32);
        let (right, bottom) = self.client_to_screen(button.right() as i32, button.bottom() as i32);
        Deferred::Menu(MenuRequest {
            x: left,
            y: bottom,
            exclude: Some(RECT {
                left,
                top,
                right,
                bottom,
            }),
            ..self.menu_request(Menu::Main)
        })
    }

    /// 設定ファイルを既定のアプリで開く。
    fn open_settings(&self) {
        match &self.config_path {
            Some(path) => link_opener::open_settings(path.clone()),
            None => tracing::warn!("the settings file location is unknown"),
        }
    }

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

    /// ウィンドウのクライアント領域の中の位置を、画面での位置にする。
    fn client_to_screen(&self, x: i32, y: i32) -> (i32, i32) {
        let mut point = POINT { x, y };
        // SAFETY: self.hwnd は有効なウィンドウで、point は書き込める。
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

    /// 画面での位置（WM_MOUSEWHEEL の `lparam`）を、ウィンドウのクライアント領域の中の位置にする。
    fn screen_to_client(&self, lparam: LPARAM) -> (i32, i32) {
        let (x, y) = client_position(lparam);
        let mut point = POINT { x, y };
        // SAFETY: self.hwnd は有効なウィンドウで、point は書き込める。
        if !unsafe { ScreenToClient(self.hwnd, &mut point) }.as_bool() {
            tracing::warn!("failed to convert the wheel position");
        }
        (point.x, point.y)
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

    /// IME の変換中のウィンドウと候補のウィンドウを、いまのカーソルの位置に出させる。
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
        // シェルが何も返さないキーでも、戻した範囲を描き、つまみを合わせる。
        if self.update_scroll_position() {
            self.invalidate();
        }
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
        self.on_viewport_moved();
    }

    /// 表示する範囲を動かした。マウスを載せているリンクを探し直し、描き直す。
    fn on_viewport_moved(&mut self) {
        self.refresh_hovered_link_now();
        self.update_scroll_position();
        self.invalidate();
    }

    /// 表示している端末の表示している範囲の位置を読み直す。最後に描いたときから変わったら `true` を
    /// 返す。
    fn update_scroll_position(&mut self) -> bool {
        let position = match self.session().vt.scroll_position() {
            Ok(position) => position,
            Err(e) => {
                tracing::warn!("failed to read the scroll position: {e}");
                return false;
            }
        };
        if self.scroll_position == Some(position) {
            return false;
        }
        self.scroll_position = Some(position);
        true
    }

    /// 端末 `id` に出力が届いたか、子プロセスが終わった。出力を流し込み、タイトルとベルを反映する。
    /// 表示していない端末の出力も流し込む（描くのは表示している端末だけ）。ウィンドウを閉じるときは
    /// `true` を返す。
    fn on_wake(&mut self, id: SessionId) -> bool {
        let Some(index) = self.sessions.iter().position(|session| session.id() == id) else {
            tracing::debug!("ignoring a wake for an unknown terminal {id:?}");
            return false;
        };
        let events = self.sessions[index].on_output();
        self.on_clipboard_requests(index);
        let shown = index == self.active;
        if events.bell {
            self.ring_bell();
        }
        if shown {
            if events.title_changed {
                self.update_window_title();
            }
            // マウスを載せている文字が変わったかもしれない。
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
    /// それをウィンドウの終了コードにし、ウィンドウを閉じるよう `true` を返す。
    fn on_session_ended(&mut self, index: usize, code: ExitCode) -> bool {
        if !self.close_session(index) {
            return false;
        }
        self.exit = Some(code);
        true
    }

    /// ウィンドウのタイトルを、表示している端末のタイトルとバージョンから決める（`window_title`）。
    /// 変わったときだけウィンドウに設定する。
    fn update_window_title(&mut self) {
        let title = window_title(self.session().title(), &version::display());
        if title == self.window_title {
            return;
        }
        // SAFETY: self.hwnd は有効なウィンドウ。文字列はこの呼び出しの間は生きている。
        if let Err(e) = unsafe { SetWindowTextW(self.hwnd, &HSTRING::from(&title)) } {
            tracing::warn!("failed to set the window title: {e}");
            return;
        }
        self.window_title = title;
    }

    /// ベル。ウィンドウが前面になければ、前面に来るまでタスクバーのボタンを点滅させる。音は鳴らさない。
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
        // SAFETY: flash は初期化済みで、self.hwnd は有効なウィンドウ。戻り値は前の点滅の状態で、失敗では
        // ない。
        unsafe { _ = FlashWindowEx(&flash) };
    }

    /// ウィンドウのクライアント領域の大きさが変わった。端末を描く領域と、端末の大きさを合わせる。
    fn on_resize(&mut self, width: u32, height: u32) {
        if let Err(e) = self.renderer.resize(width, height) {
            tracing::warn!("failed to resize the swap chain: {e}");
        }
        self.layout(width, height);
    }

    /// クライアント領域の大きさが `width` × `height` のときの、端末を描く領域に合わせる。
    fn layout(&mut self, width: u32, height: u32) {
        let insets = self.insets(self.dpi);
        self.terminal_area = terminal_area_in_client(width, height, insets);
        self.scroll_bar_area = scroll_bar_area_in_client(width, self.terminal_area, insets);
        self.backdrop_area = backdrop_area_in_client(width, height, insets);
        self.resize_sessions();
        self.update_tab_bar_hover();
        self.update_scroll_bar_hover();
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

    /// DPI が `dpi` に変わった。端末のフォントと UI のフォントを測り直し、グリフアトラスを作り直す。
    /// 端末の大きさは、このあと届く WM_SIZE（[`App::on_resize`]）で合わせる。失敗したら前のフォントの
    /// まま描く（UI のフォントだけを作れなければ、UI のフォントだけ前のまま）。
    fn apply_dpi(&mut self, dpi: u32) {
        self.dpi = dpi;
        let font = match load_font(&self.dwrite, &self.font_config, dpi) {
            Ok(font) => font,
            Err(e) => {
                tracing::warn!("failed to measure the font for {dpi} DPI: {e}");
                return;
            }
        };
        let ui_font = match UiFont::for_dpi(&self.dwrite, dpi) {
            Ok(ui_font) => Rc::new(ui_font),
            Err(e) => {
                tracing::warn!("failed to create the UI font for {dpi} DPI: {e}");
                self.ui_font.clone()
            }
        };
        if let Err(e) = self.renderer.set_font(
            &self.dwrite,
            &font,
            ui_font.clone(),
            self.font_config.wide_glyph,
        ) {
            tracing::warn!("failed to rebuild the glyph atlas for {dpi} DPI: {e}");
            return;
        }
        log_font(&font, dpi);
        log_ui_font(&ui_font, dpi);
        self.font = font;
        self.ui_font = ui_font;
        self.invalidate();
    }

    /// 設定ファイルを読み直す。フォントの設定が変わっていて、そのフォントを測れたら設定を入れ替え、
    /// 作り直しに使う DPI を返す。作り直しは借用を手放してから [`change_dpi_in_place`] で行う。
    /// スクロールバーの幅や余白が変わったときも、端末の桁数と行数を保ってウィンドウの大きさを変えるので、
    /// 同じく返す。
    /// グリフの描き方だけが変わったときは、セルの寸法は変わらないので、この場でアトラスだけを作り直す。
    /// 貼り付けの設定はすぐに入れ替える。プロファイルは次に開く端末から使う。
    /// エラーがあれば、それを見せることを返す（借用を手放してから [`run_deferred`] で見せる）。エラーが
    /// なければ、設定の警告を取り下げる（開いているメッセージボックスも閉じさせる）。
    ///
    /// 設定ダイアログを開いている間は読まず、閉じてから読む。設定ダイアログが書いた中身のままなら読まない。
    /// 古い書き方を読み替えて書き戻したら、その中身を覚え、書き戻した知らせでは読み直さない。
    fn reload_config(&mut self) -> (Option<u32>, Option<Deferred>) {
        if self.settings_dialog_open {
            self.reload_pending = true;
            return (None, None);
        }
        // 一度の書き込みの知らせが、間を置いて二度に分かれて届くこともあるので、中身が変わるまで覚えておく。
        if let Some(written) = self.settings_written.take()
            && let Some(path) = &self.config_path
            && std::fs::read_to_string(path).is_ok_and(|text| text == written)
        {
            tracing::debug!("the settings file is as fantermo wrote it");
            self.settings_written = Some(written);
            return (None, None);
        }
        let Loaded {
            config,
            notice,
            written,
        } = config_file::reload(self.config_path.as_deref(), &self.config_in_use());
        if written.is_some() {
            self.settings_written = written;
        }
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

    /// 読み直した設定、または設定ダイアログで編集中の設定を入れ替える。作り直しが要れば、その DPI を
    /// 返す（[`App::reload_config`]）。
    fn apply_config(&mut self, config: Config) -> Option<u32> {
        self.paste_confirm = config.paste.confirm;
        self.clipboard_read = config.clipboard.read;
        self.profiles = config.profiles;
        self.default_profile = config.default_profile;
        self.keymap = config.keys;
        if config.background != self.background {
            // 項目ごとの表示方法と不透明度だけが変わったなら、選んだ画像のまま描き直す（描くときに項目から
            // 決める）。
            let paths_changed = !config.background.same_paths(&self.background);
            self.background = config.background;
            if paths_changed {
                self.rechoose_background_images();
            }
            self.invalidate();
        }
        if config.colors != self.colors {
            let terminal_colors = config.colors.terminal_colors();
            if terminal_colors != self.colors.terminal_colors() {
                for session in &mut self.sessions {
                    if let Err(e) = session.vt.set_default_colors(&terminal_colors) {
                        tracing::warn!("failed to change the terminal colors: {e}");
                    }
                }
            }
            self.colors = config.colors;
            self.invalidate();
        }
        let insets_changed = config.scroll_bar.width != self.scroll_bar_width
            || config.window.padding != self.padding;
        self.scroll_bar_width = config.scroll_bar.width;
        self.padding = config.window.padding;
        let font_changed = self.apply_font_config(config.font);
        (font_changed || insets_changed).then_some(self.dpi)
    }

    /// 読み直したフォントの設定を入れ替える。フォントを測り直す作り直しが要れば `true`。
    fn apply_font_config(&mut self, font: FontConfig) -> bool {
        if font == self.font_config {
            return false;
        }
        let same_face =
            font.families == self.font_config.families && font.size_pt == self.font_config.size_pt;
        if same_face {
            self.set_wide_glyph(font.wide_glyph);
            return false;
        }
        if let Err(e) = load_font(&self.dwrite, &font, self.dpi) {
            tracing::warn!("keeping the current font: {e}");
            return false;
        }
        self.font_config = font;
        true
    }

    /// いま使っている設定。フォントは、いまのフォントを探したときの設定。
    fn config_in_use(&self) -> Config {
        Config {
            font: self.font_config.clone(),
            profiles: self.profiles.clone(),
            default_profile: self.default_profile.clone(),
            paste: PasteConfig {
                confirm: self.paste_confirm,
            },
            clipboard: ClipboardConfig {
                read: self.clipboard_read,
            },
            scroll_bar: ScrollBarConfig {
                width: self.scroll_bar_width,
            },
            window: WindowConfig {
                padding: self.padding,
            },
            colors: self.colors.clone(),
            background: self.background.clone(),
            keys: self.keymap.clone(),
        }
    }

    /// 設定ダイアログに渡すもの。いま使っている設定と、フォントの一覧。
    fn settings_request(&mut self) -> settings_dialog::Request {
        let current = self.config_in_use();
        let tab_bar_colors = tab_bar::Palette::from_system(system_colors()).colors();
        let fonts = match font::monospaced_families(&self.dwrite) {
            Ok(families) => families
                .into_iter()
                .map(|family| FontChoice {
                    name: family.name,
                    index: Some(family.index),
                })
                .collect(),
            Err(e) => {
                tracing::warn!("failed to list the fonts: {e}");
                Vec::new()
            }
        };
        let family_indices = current
            .font
            .families
            .iter()
            .map(|family| font::family_index(&self.dwrite, family))
            .collect();
        let font_in_use = FontChoice {
            name: self.font.family.clone(),
            index: font::family_index(&self.dwrite, &self.font.family),
        };
        settings_dialog::Request {
            current,
            family_indices,
            fonts,
            font_in_use,
            tab_bar_colors,
        }
    }

    /// セルより広いグリフの描き方を変え、グリフアトラスを作り直す。失敗したら前の描き方のまま。
    fn set_wide_glyph(&mut self, wide_glyph: WideGlyph) {
        if let Err(e) =
            self.renderer
                .set_font(&self.dwrite, &self.font, self.ui_font.clone(), wide_glyph)
        {
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
            self.insets(dpi),
        ))
    }

    /// DPI が `dpi` に変わったときに、いまの桁数と行数がそのまま収まるウィンドウの大きさ（枠を含む）。
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
            self.session()
                .background_image
                .as_ref()
                .map(|image| image.path.as_path()),
        );
        let frame_path = dump.frame_path();
        self.render_to(Some(&frame_path));
    }

    /// 表示している端末を端末を描く領域に、タブバーをその上に描いて表示する。表示している範囲の位置も
    /// 読み直す（出力、ウィンドウの大きさの変更、タブの切り替えなど、描き直すことはどれも位置を動かし
    /// うる）。
    /// `capture` を渡すと、表示する前の画面を PNG で書き出す。
    fn render_to(&mut self, capture: Option<&std::path::Path>) {
        self.update_scroll_position();
        self.tick_image_animations();
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
        let mut paints = self.tab_bar_paints();
        let scroll_bar_colors =
            scroll_bar::Palette::from_terminal(frame.background, frame.foreground)
                .with_colors(&self.colors.scroll_bar);
        paints.extend(self.scroll_bar_paints(&scroll_bar_colors));
        let backdrop = self.sessions[self.active]
            .background_image
            .as_ref()
            .map(|image| {
                let (fit, image_opacity) = self.background.settings_for(image.entry);
                Backdrop {
                    image: &image.path,
                    region: self.backdrop_area,
                    fit,
                    image_opacity,
                }
            });
        let session = &self.sessions[self.active];
        self.renderer
            .prepare_terminal_images(session.id().0, &frame.images, &session.vt);
        if let Err(e) = self.renderer.render(
            &[(&frame, self.terminal_area)],
            backdrop.as_ref(),
            &paints,
            capture,
        ) {
            tracing::warn!("failed to render: {e}");
        }
    }

    /// タブバーに描くもの。色は Windows のシステムの色から決め、設定で書いた色を上書きする。
    fn tab_bar_paints(&self) -> Vec<Paint> {
        let tabs: Vec<TabView> = self
            .sessions
            .iter()
            .map(|session| TabView {
                title: tab_title(session),
                bell: session.bell_marked,
                icon: &session.tab_icon,
            })
            .collect();
        let pressed = match self.tab_bar_press {
            Some((MouseButton::Left, hit)) => hit,
            _ => None,
        };
        tab_bar::paints(
            &self.tab_bar_layout(),
            &tabs,
            self.active,
            tab_bar::Highlight {
                hovered: self.tab_bar_hover,
                pressed,
            },
            &tab_bar::Palette::from_system(system_colors()).with_colors(&self.colors.tab_bar),
            &*self.ui_font,
        )
    }

    /// スクロールバーに描くもの。色は `colors`。
    fn scroll_bar_paints(&self, colors: &scroll_bar::Palette) -> Vec<Paint> {
        let Some(position) = self.scroll_position else {
            return Vec::new();
        };
        let drag = self.scroll_bar_press.and_then(|(_, drag)| drag);
        let dragging = matches!(drag, Some(ScrollBarDrag::Thumb { .. }));
        // つまみの外を押し続けている間は、マウスがスクロールバーの外へ出ても、載せているときの形にする。
        let paging = matches!(drag, Some(ScrollBarDrag::Page { .. }));
        scroll_bar::paints(
            self.scroll_bar_area,
            self.scroll_bar_metrics(),
            position,
            scroll_bar::Highlight {
                hovered: self.scroll_bar_hover || paging,
                dragging,
            },
            colors,
        )
    }

    /// スクロールバーの寸法。
    fn scroll_bar_metrics(&self) -> scroll_bar::Metrics {
        scroll_bar::Metrics {
            dpi: self.dpi,
            base_width: self.scroll_bar_width,
        }
    }

    /// DPI が `dpi` のときの、端末の画面の周りに取る場所の寸法。
    fn insets(&self, dpi: u32) -> Insets {
        Insets::new(dpi, self.scroll_bar_width, self.padding)
    }

    /// タブバーを描く領域。ウィンドウの上端に、ウィンドウの幅いっぱい。その下の端末の画面の上の余白は
    /// 含まない（余白の上のマウスは、ほかの余白と同じく端末のいちばん近いセルとみなす）。
    fn tab_bar_area(&self) -> Area {
        Area {
            x: 0,
            y: 0,
            width: self.scroll_bar_area.right(),
            height: self
                .insets(self.dpi)
                .tab_bar_height
                .min(self.terminal_area.y),
        }
    }

    /// 表示している端末の画像のアニメーションを進め、次のコマで描き直すタイマーを動かす。再生中の
    /// ものがなければ止める。
    fn tick_image_animations(&mut self) {
        let now = self.clock.elapsed();
        let next = match self.session_mut().vt.tick_image_animations(now) {
            Ok(next) => next,
            Err(e) => {
                tracing::warn!("failed to advance the image animations: {e}");
                None
            }
        };
        let Some(delay) = next else {
            if self.image_animation_timer_running {
                self.stop_image_animation_timer();
            }
            return;
        };
        let delay = delay.max(MIN_IMAGE_ANIMATION_INTERVAL);
        let millis = u32::try_from(delay.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: self.hwnd は有効なウィンドウ。同じ番号のタイマーは置き換わる。
        let started =
            unsafe { SetTimer(Some(self.hwnd), IMAGE_ANIMATION_TIMER_ID, millis, None) } != 0;
        if !started {
            tracing::warn!(
                "failed to start the image animation timer: {}",
                windows::core::Error::from_thread()
            );
        }
        self.image_animation_timer_running = started;
    }

    fn stop_image_animation_timer(&mut self) {
        // SAFETY: self.hwnd は有効なウィンドウ。
        if let Err(e) = unsafe { KillTimer(Some(self.hwnd), IMAGE_ANIMATION_TIMER_ID) } {
            tracing::warn!("failed to stop the image animation timer: {e}");
        }
        self.image_animation_timer_running = false;
    }

    /// 画像のアニメーションの次のコマの時刻になった。一度だけのタイマーとして止めて描き直す。
    fn on_image_animation_timer(&mut self) {
        self.stop_image_animation_timer();
        self.invalidate();
    }

    /// 点滅のタイマーを動かす・止める。止めたら点滅の文字を見せた状態に戻す。動かし始めも見せた状態から。
    fn set_blinking(&mut self, blinking: bool) {
        if blinking == self.blink_timer_running {
            return;
        }
        self.blink_visible = true;
        if blinking {
            // SAFETY: self.hwnd は有効なウィンドウ。
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
            // SAFETY: self.hwnd は有効なウィンドウ。
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

    /// ウィンドウがアクティブになった・でなくなった。アクティブでない間は点滅を止め、文字を見せておく。
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
        // SAFETY: self.hwnd は有効なウィンドウ。
        unsafe { _ = InvalidateRect(Some(self.hwnd), None, false) };
    }
}

/// 端末のタブに出すタイトル。アプリが付けたタイトル、付けていなければ既定のタイトル。
fn tab_title(session: &Session) -> &str {
    match session.title() {
        "" => WINDOW_TITLE,
        title => title,
    }
}

/// ウィンドウに出すタイトル。アプリが付けたタイトル `tab_title` があれば
/// `<tab_title> - fantermo <version>`、なければ（空なら）`fantermo <version>`。
fn window_title(tab_title: &str, version: &str) -> String {
    if tab_title.is_empty() {
        format!("{WINDOW_TITLE} {version}")
    } else {
        format!("{tab_title} - {WINDOW_TITLE} {version}")
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
        // SAFETY: ウィンドウがすでに壊れていれば失敗するだけで、害はない。
        unsafe {
            _ = PostMessageW(Some(HWND(hwnd_value as *mut _)), message, wparam, LPARAM(0));
        }
    }
}

fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut rect = RECT::default();
    // SAFETY: hwnd は有効なウィンドウで、rect は書き込める。
    if let Err(e) = unsafe { GetClientRect(hwnd, &mut rect) } {
        tracing::warn!("failed to get the client size: {e}");
    }
    (
        (rect.right - rect.left).max(0) as u32,
        (rect.bottom - rect.top).max(0) as u32,
    )
}

/// ウィンドウに結び付けた App を借りて `action` を呼ぶもの。借りられなければ呼ばない。
type WithApp<'a> = dyn Fn(&mut dyn FnMut(&mut App)) + 'a;

/// ウィンドウの借用を手放した状態で、入力の処理が残したことをする。その結果さらに残ったことも続けてする
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
            Deferred::ShowAbout { font } => next = show_about(hwnd, &font, with_app),
            Deferred::EditSettings => edit_settings(hwnd, with_app),
            Deferred::Menu(request) => match context_menu::show(hwnd, &request) {
                Some(Chosen::Command(command)) => next = run_menu_command(command, with_app),
                Some(Chosen::Profile(index)) => {
                    with_app(&mut |app| app.open_session(Launch::Profile(index)))
                }
                None => {}
            },
            // DestroyWindow はこの場で WM_DESTROY を送るので、借用を手放してから呼ぶ。
            Deferred::CloseWindow => destroy_window(hwnd),
            Deferred::Warn { topic, text } => notice::warn(hwnd, topic, text),
            Deferred::Update(event) => {
                next = continue_update(update::prompt::on_event(hwnd, event), with_app);
            }
            Deferred::AskClipboardRead { text } => {
                let choice = clipboard_prompt::ask(hwnd, &text);
                // ダイアログの間に最後の端末が終わると、ウィンドウはもうなく、App も解放されている。
                // SAFETY: ウィンドウのハンドルを調べるだけ。
                if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                    with_app(&mut |app| app.finish_clipboard_prompt(choice));
                }
            }
        }
    }
}

/// 「fantermo について」を開き、閉じるまで待つ。「更新を確認」を押したら、確かめる仕事を始め、届いた結果を
/// 「fantermo について」の上に見せる。更新に進むと決めたら「fantermo について」を閉じ、続けてすることを返す。
fn show_about(hwnd: HWND, font: &Font, with_app: &WithApp) -> Option<Deferred> {
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
fn continue_update(next: update::prompt::Next, with_app: &WithApp) -> Option<Deferred> {
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

/// メニューで選ばれた `command` をする。続けてすることがあれば返す。
fn run_menu_command(command: MenuCommand, with_app: &WithApp) -> Option<Deferred> {
    let mut next = None;
    match command {
        MenuCommand::Copy => with_app(&mut |app| {
            app.copy_selection();
        }),
        MenuCommand::Paste => with_app(&mut |app| next = app.paste_clipboard()),
        MenuCommand::SelectAll => with_app(&mut |app| app.select_all()),
        MenuCommand::NewTab => with_app(&mut |app| next = app.run_tab_command(TabCommand::New)),
        MenuCommand::CloseTab => with_app(&mut |app| next = app.run_tab_command(TabCommand::Close)),
        MenuCommand::EditSettings => next = Some(Deferred::EditSettings),
        MenuCommand::OpenSettings => with_app(&mut |app| app.open_settings()),
        MenuCommand::ShowAbout => with_app(&mut |app| {
            next = Some(Deferred::ShowAbout {
                font: app.font.clone(),
            })
        }),
        MenuCommand::Exit => next = Some(Deferred::CloseWindow),
    }
    next
}

/// 設定ファイルを読み直し、フォントやスクロールバーの幅が変わっていれば作り直す。エラーがあれば見せる。
fn reload_settings(hwnd: HWND, with_app: &WithApp) {
    let mut reloaded = (None, None);
    with_app(&mut |app| reloaded = app.reload_config());
    let (rebuild_dpi, deferred) = reloaded;
    // WM_DPICHANGED をこの場で送るので、借用を手放してから呼ぶ。
    if let Some(dpi) = rebuild_dpi {
        change_dpi_in_place(hwnd, dpi);
    }
    run_deferred(hwnd, deferred, with_app);
}

/// 設定ダイアログの編集を端末と設定ファイルにつなぐもの。
struct SettingsHost<'a, 'b> {
    hwnd: HWND,
    with_app: &'a WithApp<'b>,
    config_path: PathBuf,
}

impl settings_dialog::Host for SettingsHost<'_, '_> {
    fn preview(&mut self, config: &Config) {
        let mut rebuild_dpi = None;
        (self.with_app)(&mut |app| rebuild_dpi = app.apply_config(config.clone()));
        // WM_DPICHANGED をこの場で送るので、借用を手放してから呼ぶ。
        if let Some(dpi) = rebuild_dpi {
            change_dpi_in_place(self.hwnd, dpi);
        }
    }

    fn save(&mut self, base: &Config, edited: &Config) -> Result<(), String> {
        let written = config_file::save(&self.config_path, base, edited)?;
        (self.with_app)(&mut |app| app.settings_written = Some(written.clone()));
        Ok(())
    }
}

/// 設定ダイアログを開き、閉じるまで待つ。開いている間に設定ファイルが書き換えられていたら、閉じて
/// から読み直す。
fn edit_settings(hwnd: HWND, with_app: &WithApp) {
    let mut opened = None;
    with_app(&mut |app| {
        let Some(path) = app.config_path.clone() else {
            tracing::warn!("the settings file location is unknown");
            return;
        };
        app.settings_dialog_open = true;
        opened = Some((path, app.settings_request()));
    });
    let Some((config_path, request)) = opened else {
        return;
    };
    let mut host = SettingsHost {
        hwnd,
        with_app,
        config_path,
    };
    settings_dialog::run(hwnd, request, &mut host);
    // 開いている間に最後の端末が終わってウィンドウが閉じたら、App はもう無い。
    if app_of(hwnd).is_none() {
        return;
    }
    let mut pending = false;
    with_app(&mut |app| {
        app.settings_dialog_open = false;
        pending = std::mem::take(&mut app.reload_pending);
    });
    if pending {
        reload_settings(hwnd, with_app);
    }
}

/// ウィンドウの位置・大きさ・最大化を状態ファイルに書いてから、ウィンドウを閉じる。
fn destroy_window(hwnd: HWND) {
    save_window_state(hwnd);
    // SAFETY: hwnd は有効なウィンドウ。借用は持っていない。
    if let Err(e) = unsafe { DestroyWindow(hwnd) } {
        tracing::warn!("failed to destroy the window: {e}");
    }
}

/// ウィンドウのいまの位置・大きさ・最大化を状態ファイルに書く。
fn save_window_state(hwnd: HWND) {
    let Some(app) = app_of(hwnd) else {
        return;
    };
    let path = match app.try_borrow() {
        Ok(app) => app.state_path.clone(),
        Err(e) => {
            tracing::warn!("not saving the window state: {e}");
            return;
        }
    };
    let Some(path) = path else {
        return;
    };
    let Some(window) = window_state::current(hwnd) else {
        return;
    };
    window_state::save(
        &path,
        &State {
            window: Some(window),
        },
    );
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
    /// 端末に渡す。`allow_unsafe` でなければ、コマンドとして実行されうる文字列は端末が送らずに返す。
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
/// デッドキーだったときは `None`。
fn take_translated_chars(hwnd: HWND) -> Option<Vec<char>> {
    const CHAR_MESSAGES: [u32; 4] = [WM_CHAR, WM_SYSCHAR, WM_DEADCHAR, WM_SYSDEADCHAR];
    let mut assembler = Utf16Assembler::default();
    let mut chars = Vec::new();
    let mut dead = false;
    loop {
        let mut msg = MSG::default();
        // SAFETY: msg は書き込める。このウィンドウの、指定した種類のメッセージだけを取り出す。
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

/// ウィンドウに結び付けた App。WM_NCDESTROY で解放されるまで有効。
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
            // SAFETY: hwnd は有効なウィンドウ。描くのは Direct3D なので、更新領域を空にするだけ。
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
        WM_APP_CLIPBOARD_PROMPT => {
            let mut deferred = None;
            with_app(&mut |app| deferred = app.next_clipboard_prompt());
            run_deferred(hwnd, deferred, &with_app);
            LRESULT(0)
        }
        // ダイアログを閉じてウィンドウが有効に戻ったら、待っていた確かめを始める。この場ではダイアログの片づけの
        // 途中なので、積んであとで見る。
        WM_ENABLE => {
            if wparam.0 != 0 {
                poster(hwnd, WM_APP_CLIPBOARD_PROMPT, WPARAM(0))();
            }
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
        WM_TIMER if wparam.0 == BLINK_TIMER_ID => {
            with_app(&mut |app| app.on_blink_timer());
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == SCROLL_BAR_TIMER_ID => {
            with_app(&mut |app| app.on_scroll_bar_timer());
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == IMAGE_ANIMATION_TIMER_ID => {
            with_app(&mut |app| app.on_image_animation_timer());
            LRESULT(0)
        }
        // ほかのウィンドウにマウスのキャプチャが移ったら（Alt+Tab など）、離したことが届かないので、
        // スクロールバーの操作を終える。lparam はマウスをキャプチャするウィンドウ。
        WM_CAPTURECHANGED if lparam.0 != hwnd.0 as isize => {
            with_app(&mut |app| app.end_scroll_bar_press());
            LRESULT(0)
        }
        // 下位の語が WA_INACTIVE なら、アクティブでなくなった。
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xffff) as u32 != WA_INACTIVE;
            with_app(&mut |app| app.on_activate(active));
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す（フォーカスをウィンドウに移してもらう）。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_TIMER if wparam.0 == CONFIG_TIMER_ID => {
            // SAFETY: hwnd は有効なウィンドウ。
            if let Err(e) = unsafe { KillTimer(Some(hwnd), CONFIG_TIMER_ID) } {
                tracing::warn!("failed to stop the settings timer: {e}");
            }
            reload_settings(hwnd, &with_app);
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
        // DPI が変わる前に、変わった後のウィンドウの大きさを問われる。いまの桁数と行数が収まる大きさを
        // 返す。
        WM_GETDPISCALEDSIZE => {
            let dpi = wparam.0 as u32;
            let mut scaled = None;
            with_app(&mut |app| match app.scaled_window_size(dpi) {
                Ok(size) => scaled = Some(size),
                Err(e) => tracing::warn!("failed to compute the window size for {dpi} DPI: {e}"),
            });
            let Some(scaled) = scaled else {
                // 0 を返すと、Windows がウィンドウの大きさを DPI の比で拡大する。
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
            // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
            match unsafe { GetModuleHandleW(None) } {
                Ok(instance) => app_icon::set(hwnd, instance.into(), dpi),
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
            let (width, height) = client_size(hwnd);
            with_app(&mut |app| app.on_resize(width, height));
            LRESULT(0)
        }
        // × と Alt+F4。メニューの終了と、最後の端末が終わったときと同じく、状態を書いてから閉じる。
        WM_CLOSE => {
            destroy_window(hwnd);
            LRESULT(0)
        }
        // ログオフ・シャットダウン・再起動。ウィンドウは閉じられないまま、WM_ENDSESSION から戻ったあとで
        // プロセスごと終わらされるので、閉じるときと同じく状態を書く。問われた時点でも書いておく
        // （ENDSESSION_CRITICAL では WM_ENDSESSION のあとに時間がほとんどないため）。書くのは一時
        // ファイルからの置き換えなので、二度書いても、取りやめになっても害はない。終わるのを止めない。
        // ウィンドウは壊さず、端末と子プロセスはほかのプロセスと一緒に終わらされるのに任せる。
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
            };
            let mut code = 0;
            with_app(&mut |app| code = app.exit.map_or(0, |code| code.0));
            // SAFETY: このスレッドのメッセージループに終了を伝えるだけ。
            unsafe { PostQuitMessage(code as i32) };
            LRESULT(0)
        }
        WM_NCDESTROY => {
            // SAFETY: hwnd は有効なウィンドウで、ポインタを外すのはここだけ。
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
    use fantermo_core::config::BackendKind;

    use super::*;

    const NOTCH: i32 = WHEEL_DELTA as i32;

    #[test]
    fn window_titles_end_with_the_version() {
        assert_eq!(
            window_title("vim", "0.0.1 (dev-abc1234)"),
            "vim - fantermo 0.0.1 (dev-abc1234)"
        );
        assert_eq!(window_title("vim", "0.0.1"), "vim - fantermo 0.0.1");
    }

    #[test]
    fn window_titles_without_a_tab_title_are_only_the_version() {
        assert_eq!(window_title("", "0.0.1"), "fantermo 0.0.1");
        assert_eq!(
            window_title("", "0.0.1 (dev-abc1234)"),
            "fantermo 0.0.1 (dev-abc1234)"
        );
    }

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
    fn command_line_prefers_arguments_then_the_default_profile() {
        let profiles = [Profile::builtin(), Profile::from_shell("zsh -l")];
        let line = |arguments, default, launch| {
            let startup = startup_for(arguments, &profiles, default, launch, None);
            assert_eq!(startup.backend, BackendKind::Conpty);
            startup.command_line
        };
        assert_eq!(line(Some("cmd.exe"), 1, Launch::Default), "cmd.exe");
        assert_eq!(line(None, 1, Launch::Default), "zsh -l");
        assert_eq!(line(None, 0, Launch::Default), "powershell.exe -NoLogo");
        // 一覧から選んだプロファイルは、引数があってもそのプロファイル。
        assert_eq!(line(Some("cmd.exe"), 0, Launch::Profile(1)), "zsh -l");
        // 並びの外なら先頭。
        assert_eq!(line(None, 5, Launch::Default), "powershell.exe -NoLogo");
        assert_eq!(line(None, 0, Launch::Profile(7)), "powershell.exe -NoLogo");
        assert_eq!(
            startup_for(None, &[], 0, Launch::Default, None),
            Startup::conpty(config::DEFAULT_COMMAND_LINE)
        );
        // ブリッジのプロファイルは、つなぐ先も渡す。
        let wsl = Profile {
            distribution: Some("Ubuntu".to_owned()),
            ..Profile::new("u", "", BackendKind::Wsl)
        };
        assert_eq!(
            startup_for(
                Some("cmd.exe"),
                std::slice::from_ref(&wsl),
                0,
                Launch::Profile(0),
                None
            ),
            Startup {
                backend: BackendKind::Wsl,
                command_line: String::new(),
                cygwin_root: None,
                distribution: Some("Ubuntu".to_owned()),
                tab_icon: TabIcon::for_profile(&wsl, None),
                profile_name: Some(wsl.name.clone()),
            }
        );
    }

    #[test]
    fn tabs_get_the_icon_of_their_profile() {
        let profiles = [Profile {
            icon: Some("zsh.png".to_owned()),
            ..Profile::new("zsh", "zsh -l", BackendKind::Cygwin)
        }];
        let folder = Path::new(r"C:\conf");
        let icon =
            |arguments, launch| startup_for(arguments, &profiles, 0, launch, Some(folder)).tab_icon;
        let from_profile = TabIcon::for_profile(&profiles[0], Some(folder));
        assert_eq!(icon(None, Launch::Default), from_profile);
        assert_eq!(from_profile.image, Some(folder.join("zsh.png")));
        // 引数のコマンドラインで開いたタブはプロファイルなし。一覧から選べばプロファイルのもの。
        assert_eq!(
            icon(Some("cmd.exe"), Launch::Default),
            TabIcon::without_profile()
        );
        assert_eq!(icon(Some("cmd.exe"), Launch::Profile(0)), from_profile);
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
    fn the_terminal_is_surrounded_by_the_padding() {
        // 96 DPI ではタブバーの 26、既定の余白 4、既定の幅のスクロールバー 12。192 DPI では 52、8、24。
        // 設定の余白は 0 から、DPI に比例させる。
        for (dpi, setting, bar, padding, scroll_bar) in [
            (96, fantermo_core::config::DEFAULT_PADDING, 26, 4, 12),
            (192, fantermo_core::config::DEFAULT_PADDING, 52, 8, 24),
            (96, 0, 26, 0, 12),
            (96, 20, 26, 20, 12),
            (192, 20, 52, 40, 24),
        ] {
            let insets = Insets::new(
                dpi,
                fantermo_core::config::DEFAULT_SCROLL_BAR_WIDTH,
                setting,
            );
            assert_eq!(
                insets,
                Insets {
                    tab_bar_height: bar,
                    padding,
                    scroll_bar_width: scroll_bar
                }
            );
            let terminal = SIZE { cx: 800, cy: 480 };
            let client = client_size_for_terminal(terminal, insets);
            assert_eq!(
                (client.cx as u32, client.cy as u32),
                (800 + 2 * padding + scroll_bar, 480 + bar + 2 * padding)
            );
            let area = terminal_area_in_client(client.cx as u32, client.cy as u32, insets);
            let top = bar + padding;
            assert_eq!(
                area,
                Area {
                    x: padding,
                    y: top,
                    width: 800,
                    height: 480
                }
            );
            // スクロールバーはクライアント領域の右端で、端末との間に余白。上下は端末に揃える。
            assert_eq!(
                scroll_bar_area_in_client(client.cx as u32, area, insets),
                Area {
                    x: 800 + 2 * padding,
                    y: top,
                    width: scroll_bar,
                    height: 480
                }
            );
            // 端末の左上のセルの左上は、クライアント領域の (padding, top)。余白の中は端末の外。
            let (left, top) = (padding as i32, top as i32);
            assert_eq!(client_to_area(area, left, top), (0, 0));
            assert_eq!(client_to_area(area, left - 1, top - 1), (-1, -1));
            assert_eq!(client_to_area(area, left + 800, top + 480), (800, 480));
            assert_eq!(area_to_client(area, 16, 32), (left + 16, top + 32));
            // 背景画像は端末と余白に敷く。
            assert_eq!(
                backdrop_area_in_client(client.cx as u32, client.cy as u32, insets),
                Area {
                    x: 0,
                    y: bar,
                    width: 800 + 2 * padding,
                    height: 480 + 2 * padding
                }
            );
        }
        // タブバーより低いクライアント領域では、端末を描く領域は空。スクロールバーと左右の余白より
        // 狭ければ、端末の幅は 0 で、スクロールバーはクライアント領域の右端に設定の幅。
        let insets = Insets {
            tab_bar_height: 26,
            padding: 4,
            scroll_bar_width: 12,
        };
        assert_eq!(terminal_area_in_client(800, 20, insets).height, 0);
        assert_eq!(terminal_area_in_client(800, 32, insets).height, 0);
        let narrow = terminal_area_in_client(18, 100, insets);
        assert_eq!((narrow.x, narrow.width), (4, 0));
        assert_eq!(
            scroll_bar_area_in_client(18, narrow, insets),
            Area {
                x: 6,
                y: 30,
                width: 12,
                height: 66
            }
        );
        assert_eq!(scroll_bar_area_in_client(8, narrow, insets).width, 8);
        // 設定の幅は DPI に比例させる。
        assert_eq!(Insets::new(144, 20, 4).scroll_bar_width, 30);
        assert_eq!(Insets::new(144, 12, 5).padding, 8);
    }

    #[test]
    fn positions_outside_the_cells_go_to_the_nearest_cell() {
        // 10 × 5 のセル、一つが 8 × 16 ピクセル。
        let size = GridSize {
            cols: 10,
            rows: 5,
            cell_width_px: 8,
            cell_height_px: 16,
        };
        // 左と上の余白は先頭の列と行の端、右と下の余白は最後の列と行の端。
        assert_eq!(clamp_to_grid(-4, -4, size), (0, 0));
        assert_eq!(clamp_to_grid(85, 90, size), (79, 79));
        assert_eq!(clamp_to_grid(-1, 40, size), (0, 40));
        // セルの中はそのまま。
        assert_eq!(clamp_to_grid(0, 0, size), (0, 0));
        assert_eq!(clamp_to_grid(79, 79, size), (79, 79));
        assert_eq!(clamp_to_grid(33, 17, size), (33, 17));
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
