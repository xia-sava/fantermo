//! 端末のウィンドウ。Win32 のメッセージを受け、端末の状態・バックエンド・描画をつなぐ。

use std::{
    cell::RefCell,
    collections::VecDeque,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use fantermo_core::{
    backend::ExitCode,
    background::Random,
    clipboard::ReadPrompts,
    config::{self, Config},
    state::State,
    vt::{Key, MouseButton},
};
use windows::{
    Win32::{
        Foundation::{ERROR_INVALID_WINDOW_HANDLE, HWND, LPARAM, WPARAM},
        Graphics::{DirectWrite::IDWriteFactory, Gdi::InvalidateRect},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{GetDpiForSystem, GetDpiForWindow},
            WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DispatchMessageW,
                GWLP_USERDATA, GetMessageW, IDC_ARROW, LoadCursorW, MSG, PostMessageW,
                RegisterClassExW, SW_SHOWDEFAULT, SW_SHOWMAXIMIZED, SetTimer, SetWindowLongPtrW,
                ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WNDCLASSEXW,
                WS_OVERLAPPEDWINDOW,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{
    app_icon,
    command_line::{Options, TabRequest},
    config_file::{self, Loaded},
    dump::{DUMP_INTERVAL, ScreenDump},
    font::{self, Font},
    instance::Forwarded,
    keyboard::Utf16Assembler,
    notice::{self, Topic},
    paint::Area,
    renderer::Renderer,
    session::{Session, SessionId},
    tab_bar::Hit,
    ui_font::UiFont,
    version, window_state,
};

mod clipboard;
mod dpi;
#[cfg(feature = "e2e")]
mod e2e;
mod keys;
mod layout;
mod menu;
mod mouse;
mod proc;
mod render;
mod scroll_bar;
mod settings;
mod tab_bar;
mod tabs;
mod update;

use dpi::{
    change_dpi_keeping_grid, change_dpi_keeping_window, dpi_from_env, log_font, log_ui_font,
};
use layout::{
    Insets, backdrop_area_in_client, client_size, client_size_for_terminal, grid_pixels, grid_size,
    scroll_bar_area_in_client, terminal_area_in_client, window_size,
};
use mouse::wheel_lines_per_notch;
use proc::window_proc;
use render::{device_removed_poster, random_seed};
use scroll_bar::ScrollBarDrag;
use tabs::{Launch, startup_for, window_title};

pub const WINDOW_CLASS: PCWSTR = w!("fantermo");
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

/// クリップボードの読み取りの依頼が確認を待っているかもしれないことを、ウィンドウに知らせる。
const WM_APP_CLIPBOARD_PROMPT: u32 = WM_APP + 4;

/// e2e テストの制御の口に要求が届いたと知らせるメッセージ。
#[cfg(feature = "e2e")]
const WM_APP_E2E: u32 = WM_APP + 5;

/// 別のプロセスから渡された起動の指定を開くメッセージ。ダイアログやメニューが閉じたあとにも送る。`wparam` が
/// [`LAUNCH_ARRIVED`] なら届いたところで、ウィンドウを前面に出す。
const WM_APP_LAUNCH: u32 = WM_APP + 6;

/// [`WM_APP_LAUNCH`] の `wparam`。届いたところ。
const LAUNCH_ARRIVED: usize = 1;

/// Direct3D のデバイスを失ったと、スレッドプールのスレッドが知らせるメッセージ。知らせた `Renderer` を
/// 作り直したあとに届くこともある。
const WM_APP_DEVICE_REMOVED: u32 = WM_APP + 7;

/// 画面の書き出しに使うタイマー。
const DUMP_TIMER_ID: usize = 1;

/// 設定ファイルの書き換えを知らされてから読み直すまでのタイマー。
const CONFIG_TIMER_ID: usize = 2;

/// 設定ファイルの書き換えを知らされてから読み直すまでの間。この間に届いた知らせはまとめる。
const CONFIG_RELOAD_DELAY: Duration = Duration::from_millis(200);

/// 点滅（SGR 5、6）の文字を見せる・隠すを切り替えるタイマー。
const BLINK_TIMER_ID: usize = 3;

/// スクロールバーのつまみの外を押し続けたときに、ページを送り続けるタイマー。
const SCROLL_BAR_TIMER_ID: usize = 4;

/// 端末の中の画像のアニメーションの次のコマで描き直すタイマー。
const IMAGE_ANIMATION_TIMER_ID: usize = 5;

/// Direct3D のデバイスを失って `Renderer` を作り直せなかったときに、もう一度試すタイマー。
const RENDERER_RETRY_TIMER_ID: usize = 6;

/// 出力を流しきれなかった端末の続きを、キー入力と描き直しを処理させたあとで流すタイマー。
const OUTPUT_TIMER_ID: usize = 7;

/// 起動したときの大きさ（セルの数）。ウィンドウの状態ファイルに大きさを覚えていないときに使う。
const INITIAL_COLS: u16 = 100;
const INITIAL_ROWS: u16 = 30;

/// ウィンドウを開き、子プロセスを起動して、ウィンドウが閉じるまでメッセージを処理する。最後の端末の
/// 子プロセスが終わってウィンドウを閉じたときはその終了コードを、利用者が最後の端末を閉じたときは 0 を
/// 返す。`options` は fantermo の引数。`config_path` は設定ファイルの
/// 場所で、`loaded` はそこから読んだ設定。読んだときの警告と引数の指定の誤りは、ウィンドウを出してから見せる。
/// `state_path` はウィンドウの状態ファイルの場所で、`state` はそこから読んだ状態。ウィンドウを見せる前に、
/// 覚えていた位置・大きさ（ピクセル）に戻し、閉じるときに書く。
pub fn run(
    options: Options,
    config_path: Option<PathBuf>,
    loaded: Loaded,
    state_path: Option<PathBuf>,
    state: State,
) -> Result<u32, Box<dyn std::error::Error>> {
    let Loaded {
        config,
        notice,
        notice_key,
    } = loaded;
    let options_notice = options.notice(&config.profiles);
    let dwrite = font::create_factory()?;
    // SAFETY: 引数のない問い合わせ。
    let system_dpi = unsafe { GetDpiForSystem() };
    let font = font::find_or_default(&dwrite, &config.font, system_dpi)?;
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
    let app = App::new(
        hwnd,
        dwrite,
        font,
        system_dpi,
        config_path,
        config,
        notice_key,
        options.tab,
    )?;
    let data = Box::into_raw(Box::new(WindowData {
        state_path,
        app: RefCell::new(app),
    }));
    // SAFETY: hwnd はこのスレッドが作ったウィンドウ。ポインタは WM_NCDESTROY で取り戻して解放する。
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, data as isize) };

    // ウィンドウを置いたモニタの DPI がシステムの DPI と違えば、DPI が変わったときと同じように作り直す。
    // 覚えていた大きさがあればそのままにして、桁数と行数をその DPI のセルで決め直す。なければ、既定の
    // 桁数と行数がその DPI でちょうど入る大きさにする。
    let window_dpi = dpi_from_env().unwrap_or_else(|| {
        // SAFETY: hwnd は有効なウィンドウ。
        unsafe { GetDpiForWindow(hwnd) }
    });
    if window_dpi != system_dpi {
        if restored.is_some() {
            change_dpi_keeping_window(hwnd, window_dpi);
        } else {
            change_dpi_keeping_grid(hwnd, window_dpi);
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
    if let Some(text) = options_notice {
        tracing::warn!("{text}");
        notice::warn(hwnd, Topic::CommandLine, text);
    }
    Ok(message_loop())
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

/// ウィンドウに結び付けるもの。`GWLP_USERDATA` がこれを指し、`WM_NCDESTROY` で解放する。
///
/// `app` は `window_proc` が `with_app` で借りてから使う。借用の中の同期の呼び出し（ほかのスレッドへの送信で
/// 止まる間など）では、ほかのスレッドから送られたメッセージがこの場で処理され、そのときは借用が重なるので
/// `app` を読むことも書くこともできない。`RefCell` の外に置いた値は作ったあと変えないので、借用が重なっていても
/// 読める。借用の外でも中でもすることに要る値は、ここに置く。
struct WindowData {
    /// ウィンドウの状態ファイルの場所。`None` なら書かない。借用の中でも書くので `App` の外に置く
    /// （`proc::save_window_state`）。
    state_path: Option<PathBuf>,
    app: RefCell<App>,
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
    /// いま使っている設定。`config.font` は、`font` を探したときの設定。
    config: Config,
    /// 確認を待っているクリップボードの読み取りの依頼。
    clipboard_prompts: ReadPrompts<SessionId>,
    /// fantermo の引数の、タブで起動するものの指定。
    arguments: TabRequest,
    /// 別のプロセスから渡され、まだ開いていない起動の指定。ダイアログやメニューの間は待たせる。
    pending_launches: VecDeque<Forwarded>,
    /// ウィンドウが持つ端末。タブの並びの順。少なくとも一つある。
    sessions: Vec<Session>,
    /// 表示している端末の、`sessions` の中の位置。
    active: usize,
    /// 次に開く端末の番号。閉じた端末の番号は使い直さない（閉じたあとに届く知らせを捨てるため）。
    next_session_id: usize,
    /// 出力を流しきれず、[`OUTPUT_TIMER_ID`] のタイマーで続きを流す端末。
    output_continuations: Vec<SessionId>,
    /// ウィンドウのクライアント領域のうち、表示している端末を描く領域。
    terminal_area: Area,
    /// Direct3D で描くもの。デバイスを失ってから、作り直しを待つ間と、描画をやめたあとは `None`。
    renderer: Option<Renderer>,
    /// デバイスを失ってから `Renderer` を作り直そうとした回数（作れなかった回も数える）。描けたら 0 に
    /// 戻す。
    renderer_recreations: u32,
    dump: Option<ScreenDump>,
    /// ウィンドウのタイトルに出している文字列。
    window_title: String,
    /// 最後の端末が終わったときの、その子プロセスの終了コード。ウィンドウを閉じたあとの終了コードにする。
    exit: Option<ExitCode>,
    /// キーを伴わずに届く文字（IME の確定など）を組み立てる。
    text_input: Utf16Assembler,
    /// ホイール 1 ノッチで動かす行数（Windows の設定。起動時と、設定が変わったときに読む）。
    wheel_lines_per_notch: u32,
    /// ダブルクリックの判定と画像のアニメーションに使う時計。
    clock: Instant,
    /// 画像のアニメーションの次のコマで描き直すタイマーが動いている。
    image_animation_timer_running: bool,
    /// マウスの、ウィンドウのクライアント領域の中の位置。ウィンドウの外へ出たら `None`。
    pointer_in_window: Option<(i32, i32)>,
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
    /// ウィンドウのクライアント領域のうち、スクロールバーを描く領域。
    scroll_bar_area: Area,
    /// スクロールバーの上で押したマウスのボタンと、左ボタンならその操作。離すまで、マウスの操作を
    /// 端末へ送らない。
    scroll_bar_press: Option<(MouseButton, Option<ScrollBarDrag>)>,
    /// マウスがスクロールバーの上にある。変わったときだけ描き直す。
    scroll_bar_hover: bool,
    /// マウスのキャプチャを放す。`ReleaseCapture` はこの場で `WM_CAPTURECHANGED` を送るので、借用を手放した
    /// ところで `window_proc` の `with_app` が放す。
    release_capture_pending: bool,
    /// 設定ダイアログを開いている。開いている間は設定ファイルを読み直さない（ダイアログで編集中の
    /// 設定を、書く前の中身で上書きしないため）。
    settings_dialog_open: bool,
    /// 設定ダイアログを開いている間に、設定ファイルが書き換えられた。閉じたら読み直す。
    reload_pending: bool,
    /// 最後に設定ファイルを読んだときの警告の中身（[`Loaded::notice_key`]）。読み直して同じなら、続けて見せない。
    settings_notice_key: Option<String>,
    /// 更新の確認かダウンロードを別のスレッドでしているか、その結果を見せている。その間は新しく始めない。
    update_busy: bool,
    /// 更新のスレッドが結果を送るチャネルの送信側と、ウィンドウが受け取る受信側。送ったら
    /// `WM_APP_UPDATE` で知らせる。
    update_sender: mpsc::Sender<crate::update::Event>,
    update_events: mpsc::Receiver<crate::update::Event>,
    /// e2e テストの制御の口。環境変数で開くよう指定されていなければ `None`。
    #[cfg(feature = "e2e")]
    e2e: Option<crate::e2e_server::Control>,
}

impl App {
    /// `config_path` から読んだ設定が `config` で、その警告の中身を比べる文が `notice_key`
    /// （[`Loaded::notice_key`]）。
    #[allow(clippy::too_many_arguments)]
    fn new(
        hwnd: HWND,
        dwrite: IDWriteFactory,
        font: Font,
        dpi: u32,
        config_path: Option<PathBuf>,
        config: Config,
        notice_key: Option<String>,
        arguments: TabRequest,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (width, height) = client_size(hwnd);
        let insets = Insets::new(dpi, config.scroll_bar.width, config.window.padding);
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
            config.font.wide_glyph,
            device_removed_poster(hwnd),
        )?;
        let first_id = SessionId(0);
        let session = Session::spawn(
            first_id,
            &startup_for(
                &arguments,
                &config.profiles,
                config::default_profile_index(&config.profiles, config.default_profile.as_deref()),
                Launch::Default,
                config_path.as_deref().and_then(Path::parent),
            ),
            size,
            &config.colors.terminal_colors(),
            wake_poster(hwnd, first_id),
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
            config,
            clipboard_prompts: ReadPrompts::default(),
            arguments,
            pending_launches: VecDeque::new(),
            sessions: vec![session],
            active: 0,
            next_session_id: first_id.0 + 1,
            output_continuations: Vec::new(),
            terminal_area,
            renderer: Some(renderer),
            renderer_recreations: 0,
            dump,
            window_title: window_title("", &version::display()),
            exit: None,
            text_input: Utf16Assembler::default(),
            wheel_lines_per_notch: wheel_lines_per_notch(),
            clock: Instant::now(),
            image_animation_timer_running: false,
            pointer_in_window: None,
            random: Random::new(random_seed()),
            backdrop_area,
            suppressed_release: None,
            tab_bar_press: None,
            tab_bar_hover: None,
            window_active: false,
            blink_visible: true,
            blink_timer_running: false,
            scroll_bar_area,
            scroll_bar_press: None,
            scroll_bar_hover: false,
            release_capture_pending: false,
            settings_dialog_open: false,
            reload_pending: false,
            settings_notice_key: notice_key,
            update_busy: false,
            update_sender,
            update_events,
            #[cfg(feature = "e2e")]
            e2e: crate::e2e_server::start(poster(hwnd, WM_APP_E2E, WPARAM(0))),
        };
        app.sessions[0].background_image = app.choose_background_image();
        Ok(app)
    }

    /// 表示している端末。
    fn session(&self) -> &Session {
        &self.sessions[self.active]
    }

    fn session_mut(&mut self) -> &mut Session {
        &mut self.sessions[self.active]
    }

    fn invalidate(&self) {
        // SAFETY: self.hwnd は有効なウィンドウ。
        unsafe { _ = InvalidateRect(Some(self.hwnd), None, false) };
    }
}

/// ほかのスレッドから呼んで、`hwnd` に `message` を `wparam` を添えて積むもの。
fn poster(hwnd: HWND, message: u32, wparam: WPARAM) -> impl Fn() + Send + Sync + 'static {
    // HWND はスレッドをまたいで渡せないので、値だけを持たせる。PostMessageW はどのスレッドからでも呼べる。
    let hwnd_value = hwnd.0 as isize;
    move || post_or_warn(HWND(hwnd_value as *mut _), message, wparam)
}

/// `hwnd` に `message` を `wparam` を添えて積む。失敗は [`warn_post_failed`] で残す。どのスレッドから
/// 呼んでもよい。
fn post_or_warn(hwnd: HWND, message: u32, wparam: WPARAM) {
    if let Err(e) = post(hwnd, message, wparam) {
        warn_post_failed(message, &e);
    }
}

/// 端末 `id` の知らせを積むもの。`hwnd` に [`WM_APP_WAKE`] を積み、積めたら `true` を返す（積めなければ、
/// バックエンドは次の出力でまた知らせ、続きの出力はタイマーで積み直す）。[`Session::spawn`] に渡し、端末ごとに
/// 高々一つ積むよう [`crate::session::WakeMessage`] を通して、ほかのスレッドとウィンドウのスレッドから呼ばれる。
///
/// バックエンドは、破棄したあとはこれを呼ばない（`OutputBudget::sent`）。ウィンドウのスレッドは端末があるときだけ
/// 呼ぶ。端末を閉じたときと、`WM_NCDESTROY` で App を解放するときにバックエンドを破棄し、どちらもウィンドウの
/// ハンドルを解放する前なので、壊れたウィンドウのハンドルの値（ほかのウィンドウに使い回されうる）へ積むことはない。
/// 閉じる前に積んだ知らせが閉じたあとに届いても、[`App::on_wake`] は知らない番号として捨てる（番号は使い回さない）。
///
/// 積めなかったことは端末ごとに一度だけ warn に残し、二度目からは debug にする。子プロセスが終わったあとの知らせは
/// 積めるまで間隔をおいて積み直すので、キューが埋まっている間に warn が続かないようにする。
fn wake_poster(hwnd: HWND, id: SessionId) -> impl Fn() -> bool + Send + Sync + 'static {
    // HWND はスレッドをまたいで渡せないので、値だけを持たせる。
    let hwnd_value = hwnd.0 as isize;
    let warned = AtomicBool::new(false);
    move || match post(HWND(hwnd_value as *mut _), WM_APP_WAKE, WPARAM(id.0)) {
        Ok(()) => true,
        Err(e) => {
            if !window_is_gone(&e) && warned.swap(true, Ordering::Relaxed) {
                tracing::debug!(
                    "failed to post message {WM_APP_WAKE:#06x} for terminal {id:?} again: {e}"
                );
            } else {
                warn_post_failed(WM_APP_WAKE, &e);
            }
            false
        }
    }
}

/// [`post`] が、宛先のウィンドウが壊れていて失敗した。
fn window_is_gone(error: &windows::core::Error) -> bool {
    error.code() == ERROR_INVALID_WINDOW_HANDLE.to_hresult()
}

/// `hwnd` に `message` を `wparam` を添えて積む。どのスレッドから呼んでもよい。
fn post(hwnd: HWND, message: u32, wparam: WPARAM) -> windows::core::Result<()> {
    // SAFETY: PostMessageW はハンドルの値を調べるだけで、このプロセスのメモリを指す引数はない（`lparam` は 0）。
    // ウィンドウがすでに壊れていれば失敗するだけ。
    unsafe { PostMessageW(Some(hwnd), message, wparam, LPARAM(0)) }
}

/// [`post`] の失敗を残す。ウィンドウが壊れたあと（閉じる途中に届いた出力や終了の知らせ）は害がないので
/// debug にとどめる。ほかの失敗（キューがいっぱいなど）は、知らせを失ってそのタブの出力が止まりうるので warn。
/// [`WM_APP_DEVICE_REMOVED`] を失ったときは、デバイスの喪失に描くときまで気づかないことを添える。
fn warn_post_failed(message: u32, error: &windows::core::Error) {
    if window_is_gone(error) {
        tracing::debug!("the window is gone; did not post message {message:#06x}: {error}");
    } else if message == WM_APP_DEVICE_REMOVED {
        tracing::warn!(
            "failed to post message {message:#06x}: {error}; \
             the lost Direct3D device will be noticed only when drawing"
        );
    } else {
        tracing::warn!("failed to post message {message:#06x}: {error}");
    }
}
