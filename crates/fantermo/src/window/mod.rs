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
    config::{
        self, BackgroundConfig, ClipboardRead, ColorsConfig, Config, FontConfig, PasteConfig,
        Profile,
    },
    keymap::Keymap,
    state::State,
    vt::{Key, KeyAction, MouseButton},
};
use windows::{
    Win32::{
        Foundation::{ERROR_INVALID_WINDOW_HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::{
            DirectWrite::IDWriteFactory,
            Gdi::{InvalidateRect, ValidateRect},
        },
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{
            Controls::WM_MOUSELEAVE,
            HiDpi::{GetDpiForSystem, GetDpiForWindow},
            Input::KeyboardAndMouse::{ReleaseCapture, VK_F4, VK_PROCESSKEY, VK_SPACE},
            WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
                DestroyWindow, DispatchMessageW, GUI_INMENUMODE, GUI_POPUPMENUMODE, GUITHREADINFO,
                GWLP_USERDATA, GetGUIThreadInfo, GetLastActivePopup, GetMessageW,
                GetWindowLongPtrW, HTCLIENT, IDC_ARROW, IsIconic, KillTimer, LoadCursorW, MSG,
                PostMessageW, PostQuitMessage, RegisterClassExW, SIZE_MINIMIZED, SW_RESTORE,
                SW_SHOWDEFAULT, SW_SHOWMAXIMIZED, SWP_NOACTIVATE, SWP_NOZORDER,
                SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
                TranslateMessage, WA_INACTIVE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ACTIVATE, WM_APP,
                WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_CONTEXTMENU, WM_COPYDATA, WM_DESTROY,
                WM_DPICHANGED, WM_ENABLE, WM_ENDSESSION, WM_EXITMENULOOP, WM_IME_COMPOSITION,
                WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN,
                WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
                WM_NCDESTROY, WM_PAINT, WM_QUERYENDSESSION, WM_RBUTTONDOWN, WM_RBUTTONUP,
                WM_SETCURSOR, WM_SETTINGCHANGE, WM_SIZE, WM_SYSCHAR, WM_SYSCOLORCHANGE,
                WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{
    app_icon, clipboard_prompt,
    command_line::{Options, TabRequest},
    config_file::{self, Loaded},
    context_menu::{self, Chosen, MenuRequest},
    dump::{DUMP_INTERVAL, ScreenDump},
    font::{self, Font},
    instance::{self, Forwarded},
    keyboard::{KeyPosition, Utf16Assembler},
    notice::{self, Topic},
    paint::Area,
    paste_editor,
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
mod render;
mod scroll_bar;
mod settings;
mod tab_bar;
mod tabs;
mod update;

use dpi::{
    change_dpi_keeping_grid, change_dpi_keeping_window, dpi_from_env, load_font, log_font,
    log_ui_font,
};
use keys::take_translated_chars;
use layout::{
    Insets, backdrop_area_in_client, client_size, client_size_for_terminal, grid_pixels, grid_size,
    scroll_bar_area_in_client, terminal_area_in_client, window_size,
};
use menu::run_menu_command;
use mouse::{
    KEYBOARD_CONTEXT_MENU_POSITION, client_position, message_button, wheel_lines_per_notch,
};
use render::{device_removed_poster, random_seed};
use scroll_bar::ScrollBarDrag;
use settings::{edit_settings, reload_settings};
use tabs::{Launch, start_output_timer, startup_for, window_title};
use update::{continue_update, show_about};

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
    let app = App::new(
        hwnd,
        dwrite,
        font,
        system_dpi,
        config_path,
        config,
        notice_key,
        state_path,
        options.tab,
    )?;
    let app = Box::into_raw(Box::new(RefCell::new(app)));
    // SAFETY: hwnd はこのスレッドが作ったウィンドウ。ポインタは WM_NCDESTROY で取り戻して解放する。
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize) };

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

/// 入力の処理のあとで、ウィンドウの借用を手放してからすること。ダイアログやメニューは自分でメッセージを
/// 回すので、借用を持ったまま開くと、その間に届いたメッセージの処理が飛ばされる。
enum Deferred {
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
    /// ウィンドウを閉じる。最後の端末を閉じたときと、メニューで終了を選んだときに使う。
    CloseWindow,
    /// `topic` についての警告 `text` を見せる。
    Warn { topic: Topic, text: String },
    /// 更新のスレッドの結果を見せ、次にすることを決める。
    Update(crate::update::Event),
    /// クリップボードの読み取りの依頼を、本文 `text` のダイアログで確認する。
    AskClipboardRead { text: String },
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
    /// 貼り付ける前に、編集のダイアログで確認するとき。
    paste_config: PasteConfig,
    /// アプリがクリップボードの内容を読み取ろうとしたときにどうするか。
    clipboard_read: ClipboardRead,
    /// 確認を待っているクリップボードの読み取りの依頼。
    clipboard_prompts: ReadPrompts<SessionId>,
    /// fantermo の引数の、タブで起動するものの指定。
    arguments: TabRequest,
    /// 別のプロセスから渡され、まだ開いていない起動の指定。ダイアログやメニューの間は待たせる。
    pending_launches: VecDeque<Forwarded>,
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
    /// （[`Loaded::notice_key`]）。`state_path` はウィンドウの状態ファイルの場所。
    #[allow(clippy::too_many_arguments)]
    fn new(
        hwnd: HWND,
        dwrite: IDWriteFactory,
        font: Font,
        dpi: u32,
        config_path: Option<PathBuf>,
        config: Config,
        notice_key: Option<String>,
        state_path: Option<PathBuf>,
        arguments: TabRequest,
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
            state_path,
            font_config,
            paste_config: config.paste,
            clipboard_read: config.clipboard.read,
            clipboard_prompts: ReadPrompts::default(),
            arguments,
            pending_launches: VecDeque::new(),
            profiles: config.profiles,
            default_profile: config.default_profile,
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

/// ウィンドウに結び付けた App を借りて `action` を呼ぶもの。呼ばれるたびに App を読み直すので、ダイアログや
/// メッセージボックスの間にウィンドウが閉じたあとでも呼んでよい。ウィンドウがもうないときや、借りられない
/// ときは呼ばない。
type WithApp<'a> = dyn Fn(&mut dyn FnMut(&mut App)) + 'a;

/// ウィンドウの借用を手放した状態で、入力の処理が残したことをする。その結果さらに残ったことも続けてする
/// （メニューで貼り付けを選び、それが編集のダイアログを要する場合など）。
fn run_deferred(hwnd: HWND, deferred: Option<Deferred>, with_app: &WithApp) {
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
/// `with_app` が App を借りられたときだけ閉じる。借りられないのは、ウィンドウがもうないときと、外側の
/// `window_proc` が App を借りている間に、その中の同期の呼び出し（ほかのスレッドへの送信で止まる間など）で
/// このメッセージが処理されたとき。後者で閉じると、外側が借りている App を `WM_NCDESTROY` で解放するので、
/// 閉じずに戻る。そのメッセージが `WM_CLOSE` なら、`with_app` が覚えて、借用を手放したところで積み直す
/// （[`is_repostable`]）。
fn destroy_window(hwnd: HWND, with_app: &WithApp) {
    if !save_window_state(with_app) {
        tracing::debug!("not destroying the window while the App is unavailable");
        return;
    }
    // SAFETY: hwnd は有効なウィンドウ。with_app が App を借りられたので、外側に借用はなく、自分の借用も
    // 手放している。WM_NCDESTROY がこの場で App を解放しても、借りているものはない。
    if let Err(e) = unsafe { DestroyWindow(hwnd) } {
        tracing::warn!("failed to destroy the window: {e}");
    }
}

/// App を借りて、ウィンドウのいまの位置・大きさ・最大化を状態ファイルに書く。App を借りられなければ
/// 書かずに `false` を返す（状態ファイルの場所がなくて書かないときは `true`）。
fn save_window_state(with_app: &WithApp) -> bool {
    let mut borrowed = false;
    with_app(&mut |app| {
        borrowed = true;
        write_window_state(app);
    });
    borrowed
}

/// ウィンドウのいまの位置・大きさ・最大化を、`app` の状態ファイルに書く。
fn write_window_state(app: &App) {
    let Some(path) = &app.state_path else {
        return;
    };
    let Some(window) = window_state::current(app.hwnd) else {
        return;
    };
    window_state::save(
        path,
        &State {
            window: Some(window),
        },
    );
}

/// このスレッドがメニューを開いているか。
fn in_menu() -> bool {
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

/// `hwnd` の `GWLP_USERDATA` に入っている値を、App のポインタとして読む。参照は作らないので、App を結び付けて
/// いないウィンドウや、破棄したウィンドウ（0 が返る）のハンドルでもよい。
fn app_pointer(hwnd: HWND) -> *const RefCell<App> {
    // SAFETY: 値を読むだけ。無効なハンドルには 0 が返る。
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App> }
}

/// ウィンドウに結び付けた App。`'static` は形だけで、参照を使ってよいのは、この場でメッセージを回すもの
/// （ダイアログ、メニュー、メッセージボックス、`DestroyWindow`）を呼ぶまで。その間に WM_NCDESTROY で
/// 解放されうるので、呼んだあとは読み直す。`hwnd` は App を結び付けたウィンドウか、0 を返すハンドルに限る。
fn app_of(hwnd: HWND) -> Option<&'static RefCell<App>> {
    let pointer = app_pointer(hwnd);
    // SAFETY: GWLP_USERDATA には run が入れた App のポインタか 0 が入っている。0 でなければ、WM_NCDESTROY が
    // 0 に戻してから解放するので、まだ解放されていない。
    unsafe { pointer.as_ref() }
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // 入口で読んだ App は、この場でメッセージを回すものを呼ぶと解放されうるので、ポインタとしてだけ持つ。
    let app = app_pointer(hwnd);
    if app.is_null() {
        // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    // 呼ばれるたびに GWLP_USERDATA を読み直す。処理の途中でメッセージを回し、その間にウィンドウが破棄されると App は
    // 解放され、同じハンドルの値がほかのウィンドウに使われることもある。入口で取ったポインタがまだ結び付いて
    // いるときだけ借りる。ポインタで比べてから参照にするのは、ほかのウィンドウが GWLP_USERDATA に入れた
    // ポインタを App の参照にしないため。
    // 処理の途中で別のメッセージがこの場で届くと、借用が重なる。そのときも処理を飛ばす。
    let with_app = |action: &mut dyn FnMut(&mut App)| {
        if !std::ptr::eq(app_pointer(hwnd), app) {
            tracing::debug!("the window is gone; skipped message {msg:#06x}");
            return;
        }
        // SAFETY: GWLP_USERDATA にまだ run が入れたポインタが入っているので、WM_NCDESTROY で解放されていない。
        // DestroyWindow を呼ぶのは destroy_window だけで、そこは with_app が App を借りられたとき（外側に借用が
        // ないとき）に、借用を手放してから呼ぶ。WM_CLOSE も destroy_window を通る。ほかのスレッドは
        // このウィンドウを DestroyWindow できない。なので action の間も解放されない。
        let app = unsafe { &*app };
        match app.try_borrow_mut() {
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
        WM_QUERYENDSESSION => {
            tracing::info!("the session may end (flags {:#x})", lparam.0);
            if !save_window_state(&with_app) {
                tracing::warn!("not saving the window state while the App is unavailable");
            }
            LRESULT(1)
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                tracing::info!("the session is ending (flags {:#x})", lparam.0);
                if !save_window_state(&with_app) {
                    tracing::warn!("not saving the window state while the App is unavailable");
                }
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
            let pointer = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut RefCell<App>;
            // SAFETY: pointer は run が Box::into_raw で作ったもの。入れ子のループの外側の window_proc が
            // ポインタを持っていることはあるが、with_app は GWLP_USERDATA（いま 0 にした）と比べてから参照にする
            // ので、これより後には使わない。DestroyWindow は destroy_window が借用のないことを確認してから
            // 呼ぶので、借用も残っていない。
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
