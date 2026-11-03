//! 設定ダイアログ。左にページの一覧、右にそのページの項目のコントロールを並べる。
//!
//! 項目を変えると、その場で [`Host::preview`] に渡して端末に反映させる（入力欄は打ち終わるまで
//! [`PREVIEW_DELAY`] だけ待つ）。「OK」は設定ファイルに書いて閉じ、「適用」は書いて開いたまま、
//! 「キャンセル」（Esc、×）は最後に書いたとき（書いていなければ開いたとき）の設定に戻して閉じる。
//! 設定ファイルには、最後に書いたとき（または開いたとき）から変えた項目だけを書く。
//!
//! 開いている間は呼び出し元のウィンドウを無効にする（`modal`）。端末の出力や描き直しは、ダイアログのループ
//! から届く。

use std::{
    cell::{Cell, Ref, RefCell},
    sync::OnceLock,
    time::Duration,
};

use fantermo_core::{
    config::{self, Config},
    frame::Rgb,
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::HFONT,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{ICC_UPDOWN_CLASS, NMHDR, NMUPDOWN, UDN_DELTAPOS},
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::SetFocus,
            WindowsAndMessaging::{
                BN_CLICKED, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, IDCANCEL, IDOK,
                KillTimer, LBN_SELCHANGE, MB_ICONWARNING, MB_OK, MoveWindow, SW_HIDE, SW_SHOW,
                SWP_NOZORDER, SetTimer, SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WM_CLOSE,
                WM_COMMAND, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DPICHANGED, WM_NOTIFY, WM_TIMER,
                WS_CAPTION, WS_EX_CONTROLPARENT, WS_EX_DLGMODALFRAME, WS_POPUP, WS_SYSMENU,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{
    dialog::{self, DialogState, OwnedFont, scale},
    file_dialog, modal,
    paint::BASE_DPI,
};

mod appearance;
mod background;
mod behavior;
mod colors;
mod keys;
mod profiles;
mod widgets;

use appearance::AppearancePage;
use background::BackgroundPage;
use behavior::BehaviorPage;
use colors::ColorsPage;
use keys::KeysPage;
use profiles::ProfilesPage;
use widgets::{Builder, DipRect, ListBox, NumberInput, set_text};

const CLASS: PCWSTR = w!("fantermo-settings-dialog");
const TITLE: &str = "設定";

/// 入力欄を打ってから、端末に反映するまでの間。この間に打ったものはまとめる。
const PREVIEW_DELAY: Duration = Duration::from_millis(400);
/// 入力欄を打ったあと、端末に反映するまでのタイマー。
const PREVIEW_TIMER_ID: usize = 1;

/// コントロールの ID。ボタンは IDOK と IDCANCEL も使う。
const ID_PAGES: i32 = 100;
const ID_APPLY: i32 = 101;
const ID_ERROR: i32 = 102;

/// 96 DPI のときの寸法（ピクセル）。ウィンドウの DPI に合わせて拡大する。
const CLIENT_WIDTH: i32 = 640;
const CLIENT_HEIGHT: i32 = 540;
const MARGIN: i32 = 12;
const PAGES_WIDTH: i32 = 120;
/// ページの見出しの列の幅。コントロールはその右に置く。
const LABEL_WIDTH: i32 = 150;
const ROW_HEIGHT: i32 = 24;
/// 見出しの文字を、横のコントロールの文字と高さを揃えるための下げ幅。
const LABEL_OFFSET: i32 = 4;
const BUTTON_WIDTH: i32 = 88;
const BUTTON_HEIGHT: i32 = 28;
const BUTTON_GAP: i32 = 8;
const NUMBER_WIDTH: i32 = 72;
/// ページの左端と幅。
const PAGE_LEFT: i32 = MARGIN + PAGES_WIDTH + MARGIN;
const PAGE_WIDTH: i32 = CLIENT_WIDTH - PAGE_LEFT - MARGIN;
/// ページの見出しの右に置くコントロールの左端と幅。
const CONTROL_LEFT: i32 = PAGE_LEFT + LABEL_WIDTH;
const CONTROL_WIDTH: i32 = CLIENT_WIDTH - CONTROL_LEFT - MARGIN;
/// 下に並べるボタン（OK、キャンセル、適用）の上端。
const BUTTON_TOP: i32 = CLIENT_HEIGHT - MARGIN - BUTTON_HEIGHT;

/// ページの `index` 番目の行の上端。
fn row(index: i32) -> i32 {
    MARGIN + index * (ROW_HEIGHT + MARGIN)
}

/// 上端が `top` の行の見出しの矩形。
fn label_rect(top: i32) -> DipRect {
    [PAGE_LEFT, top + LABEL_OFFSET, LABEL_WIDTH, ROW_HEIGHT]
}

/// ページ。一覧の順。
const PAGES: [&str; 6] = ["外観", "背景", "動作", "プロファイル", "キー", "色"];

/// 開いたときに見せるページ。
const PAGE_APPEARANCE: usize = 0;

/// ダイアログの外の、端末と設定ファイルを受け持つもの。
pub trait Host {
    /// 編集中の設定 `config` を端末に反映する。
    fn preview(&mut self, config: &Config);
    /// 設定ファイルに、`base` から `edited` へ変えた項目を書く。書けなければ、利用者に見せる文を返す。
    fn save(&mut self, base: &Config, edited: &Config) -> Result<(), String>;
}

/// フォントの一覧の一つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontChoice {
    /// 見せる名前。選んだら、この名前を設定に書く。
    pub name: String,
    /// システムのフォントの中の番号。同じ書体の別の名前を見分けるのに使う。
    pub index: Option<u32>,
}

/// ダイアログを開くときに渡すもの。
pub struct Request {
    /// いま使っている設定。
    pub current: Config,
    /// `current.font.families` のそれぞれの、システムのフォントの中の番号（無ければ `None`）。
    pub family_indices: Vec<Option<u32>>,
    /// フォントの一覧（等幅の書体）。
    pub fonts: Vec<FontChoice>,
    /// いま使っているフォント。一覧に無ければ、一覧の先頭に足す。
    pub font_in_use: FontChoice,
    /// 色のページで、書いていないタブバーの色として見せる色（システムの色）。
    pub tab_bar_colors: [Rgb; config::TAB_BAR_COLOR_KEYS.len()],
}

/// 設定ダイアログを開き、閉じるまで待つ。
pub fn run(owner: HWND, request: Request, host: &mut dyn Host) {
    if let Err(e) = register_class() {
        tracing::warn!("failed to register the settings dialog: {e}");
        return;
    }
    let Request {
        current,
        family_indices,
        mut fonts,
        font_in_use,
        tab_bar_colors,
    } = request;
    let selected_font = match fonts.iter().position(|font| {
        font.index.is_some() && font.index == font_in_use.index || font.name == font_in_use.name
    }) {
        Some(index) => index,
        None => {
            fonts.insert(0, font_in_use);
            0
        }
    };
    let state = match create(
        owner,
        &current,
        fonts,
        family_indices,
        selected_font,
        tab_bar_colors,
        host,
    ) {
        Ok(state) => state,
        Err(e) => {
            tracing::warn!("failed to open the settings dialog: {e}");
            return;
        }
    };
    let hwnd = state.hwnd;
    // SAFETY: owner は呼び出し元の有効なウィンドウ。
    let dpi = unsafe { GetDpiForWindow(owner) };
    // fill が警告の文の高さをこの DPI のフォントで測れるように、先に決める。
    state.set_dpi(dpi);
    state.fill(&current);
    // SAFETY: state は modal::run が戻るまで生きていて、modal::run はダイアログが壊れる（WM_DESTROY で外す）まで
    // 戻らない。
    unsafe { dialog::attach_state(hwnd, &state) };
    if let Err(e) = place(owner, hwnd, dpi) {
        tracing::warn!("failed to place the settings dialog: {e}");
    }
    state.show_page(PAGE_APPEARANCE);
    // SAFETY: hwnd と一覧は有効なウィンドウ。
    unsafe {
        _ = ShowWindow(hwnd, SW_SHOW);
        _ = SetFocus(Some(state.controls.pages.hwnd()));
    }

    modal::run(owner, hwnd, &state.closed, |_| false);

    // フォントは state と一緒に、ダイアログが壊れたあとで消える。
    state.colors.delete_swatch_brushes();
}

fn register_class() -> windows::core::Result<()> {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    dialog::register_class(
        &REGISTERED,
        CLASS,
        Some(dialog_proc),
        Some(ICC_UPDOWN_CLASS),
    )
}

/// ダイアログ全体のコントロール。ページの中のコントロールは、それぞれのページが持つ。
struct Controls {
    pages: ListBox,
    error: HWND,
    /// すべてのコントロールと、96 DPI のときの矩形。上下の矢印のボタンは入れない（相手の入力欄に
    /// 付けて置く）。
    placed: Vec<(HWND, DipRect)>,
}

/// 設定ダイアログのページ。ページは自分のコントロールと編集中の値を持ち、ダイアログ全体の状態には
/// [`DialogContext`] を通して触る。
trait Page {
    /// ページに置くコントロール。ページを切り替えるときは、これをまとめて見せる・隠す。
    fn controls(&self) -> &[HWND];

    /// 数の入力欄。上下の矢印の配置と、矢印の知らせの振り分けは、すべてのページのこれを回す。
    fn number_fields(&self) -> Vec<&dyn NumberInput> {
        Vec::new()
    }

    /// 数の入力欄が読めないときに、上下の矢印で動かし始める値を取る設定。最後に書いたときの設定。
    fn step_start(&self, ctx: &dyn DialogContext) -> Config {
        ctx.base().clone()
    }

    /// コントロールに設定 `config` の値を入れる。
    fn fill(&self, ctx: &dyn DialogContext, config: &Config);

    /// コントロールの値を `config` に入れる。読めない入力があれば、その文と入力欄を返す。
    fn read(
        &self,
        ctx: &dyn DialogContext,
        config: &mut Config,
    ) -> Result<(), (&'static str, HWND)>;

    /// コントロール `id` から知らせ `code`（`WM_COMMAND`）が届いた。このページのコントロールなら処理して
    /// `true` を返す。
    fn on_command(&self, ctx: &dyn DialogContext, id: i32, code: u32) -> bool;
}

/// ページから触る、ダイアログ全体の状態と操作。
trait DialogContext {
    /// ダイアログの HWND。ページが開くダイアログやメッセージボックスの持ち主にする。
    fn hwnd(&self) -> HWND;
    /// 最後に設定ファイルに書いたとき（書いていなければ開いたとき）の設定。
    fn base(&self) -> Ref<'_, Config>;
    /// コントロールのフォント。
    fn font(&self) -> HFONT;
    /// ダイアログのいまの DPI。
    fn dpi(&self) -> u32;
    /// すべてのコントロールと、96 DPI のときの矩形。
    fn placed(&self) -> &[(HWND, DipRect)];
    /// 編集中の設定を読み、読めれば端末に反映する（前に反映したものと同じなら何もしない）。読めな
    /// ければエラーを見せる。
    fn preview(&self);
    /// 入力欄を打ったので、少し待ってから反映する。
    fn schedule_preview(&self);
}

/// ダイアログの状態。ウィンドウプロシージャからも触るので、共有の参照だけで読み書きする。
struct State<'a> {
    hwnd: HWND,
    controls: Controls,
    appearance: AppearancePage,
    background: BackgroundPage,
    behavior: BehaviorPage,
    profiles: ProfilesPage,
    keys: KeysPage,
    colors: ColorsPage,
    host: RefCell<&'a mut dyn Host>,
    /// 最後に設定ファイルに書いたとき（書いていなければ開いたとき）の設定。キャンセルでここへ戻す。
    base: RefCell<Config>,
    /// 最後に端末に反映した設定。
    previewed: RefCell<Config>,
    font: RefCell<OwnedFont>,
    dpi: Cell<u32>,
    closed: Cell<bool>,
}

impl DialogState for State<'_> {
    fn closed(&self) -> &Cell<bool> {
        &self.closed
    }
}

/// ダイアログとコントロールを作る。大きさと位置は後で決める（[`place`]）。`current` はいま使っている
/// 設定で、フォントの一覧 `fonts` の `selected_font` 番目がいま使っているフォント（`family_indices` は
/// `current.font.families` のそれぞれの、システムのフォントの中の番号）。
fn create<'a>(
    owner: HWND,
    current: &Config,
    fonts: Vec<FontChoice>,
    family_indices: Vec<Option<u32>>,
    selected_font: usize,
    tab_bar_colors: [Rgb; config::TAB_BAR_COLOR_KEYS.len()],
    host: &'a mut dyn Host,
) -> windows::core::Result<State<'a>> {
    // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
    let instance = unsafe { GetModuleHandleW(None)? };
    // SAFETY: クラスは登録済みで、文字列はこの呼び出しの間は生きている。
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            CLASS,
            &HSTRING::from(TITLE),
            WS_POPUP | WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CLIENT_WIDTH,
            CLIENT_HEIGHT,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        )?
    };
    let unfinished = dialog::Unfinished::new(hwnd);
    let mut b = Builder::new(hwnd, instance.into());

    let pages = b.list(
        ID_PAGES,
        [MARGIN, MARGIN, PAGES_WIDTH, BUTTON_TOP - 2 * MARGIN],
        &[],
    )?;
    for page in PAGES {
        pages.add(page);
    }

    // Tab で移る順は作った順なので、ページの一覧の順に作る。
    let appearance = AppearancePage::create(&mut b, fonts, family_indices, selected_font)?;
    let background = BackgroundPage::create(&mut b)?;
    let behavior = BehaviorPage::create(&mut b)?;
    let profiles = ProfilesPage::create(&mut b)?;
    let keys = KeysPage::create(&mut b)?;
    let colors = ColorsPage::create(&mut b, tab_bar_colors)?;

    // 下のボタンと、入力のエラー
    let error = b.static_text(
        ID_ERROR,
        WINDOW_EX_STYLE(0),
        [
            MARGIN,
            BUTTON_TOP + LABEL_OFFSET,
            CLIENT_WIDTH - 2 * MARGIN - 3 * (BUTTON_WIDTH + BUTTON_GAP),
            ROW_HEIGHT,
        ],
    )?;
    let buttons = [
        (IDOK.0, "OK", true),
        (IDCANCEL.0, "キャンセル", false),
        (ID_APPLY, "適用(&A)", false),
    ];
    let mut button_left = CLIENT_WIDTH
        - MARGIN
        - buttons.len() as i32 * BUTTON_WIDTH
        - (buttons.len() as i32 - 1) * BUTTON_GAP;
    for (id, text, default) in buttons {
        let rect = [button_left, BUTTON_TOP, BUTTON_WIDTH, BUTTON_HEIGHT];
        if default {
            b.default_button(id, text, rect)?;
        } else {
            b.button(id, text, rect)?;
        }
        button_left += BUTTON_WIDTH + BUTTON_GAP;
    }

    Ok(State {
        hwnd: unfinished.finish(),
        controls: Controls {
            pages,
            error,
            placed: b.placed,
        },
        appearance,
        background,
        behavior,
        profiles,
        keys,
        colors,
        host: RefCell::new(host),
        base: RefCell::new(current.clone()),
        previewed: RefCell::new(current.clone()),
        font: RefCell::default(),
        dpi: Cell::new(BASE_DPI),
        closed: Cell::new(false),
    })
}

/// ダイアログを、DPI が `dpi` のときの大きさにして、呼び出し元のウィンドウの真ん中に置く。
fn place(owner: HWND, hwnd: HWND, dpi: u32) -> windows::core::Result<()> {
    let (width, height) = dialog::window_size(
        CLIENT_WIDTH,
        CLIENT_HEIGHT,
        WS_POPUP | WS_CAPTION | WS_SYSMENU,
        WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
        dpi,
    )?;
    let (left, top) = dialog::center_in(&dialog::window_rect(owner)?, width, height);
    let (left, top) = match dialog::work_area(owner) {
        Some(work) => (
            dialog::keep_within(left, width, work.left, work.right),
            dialog::keep_within(top, height, work.top, work.bottom),
        ),
        None => {
            tracing::warn!("failed to read the work area for the settings dialog");
            (left, top)
        }
    };
    // SAFETY: hwnd は有効なウィンドウ。
    unsafe { SetWindowPos(hwnd, None, left, top, width, height, SWP_NOZORDER) }
}

impl State<'_> {
    /// ページ。[`PAGES`] の順。
    fn pages(&self) -> [&dyn Page; PAGES.len()] {
        [
            &self.appearance,
            &self.background,
            &self.behavior,
            &self.profiles,
            &self.keys,
            &self.colors,
        ]
    }

    /// コントロールに設定 `config` を入れる。
    fn fill(&self, config: &Config) {
        // 読む順（[`Self::read`]）と同じ順に入れる。
        self.appearance.fill(self, config);
        self.behavior.fill(self, config);
        self.profiles.fill(self, config);
        self.background.fill(self, config);
        self.keys.fill(self, config);
        self.colors.fill(self, config);
    }

    /// DPI を `dpi` にし、フォントを作り直してコントロールを並べ直す。
    fn set_dpi(&self, dpi: u32) {
        self.dpi.set(dpi);
        let font = OwnedFont::message(dpi).unwrap_or_default();
        let previous = self.font.replace(font);
        self.layout();
        // 前のフォントは、どのコントロールももう使っていない。
        drop(previous);
    }

    /// コントロールを、いまの DPI の大きさで並べ、フォントを付ける。
    fn layout(&self) {
        let dpi = self.dpi.get();
        let c = &self.controls;
        let font = self.font();
        let number_fields: Vec<&dyn NumberInput> = self
            .pages()
            .into_iter()
            .flat_map(|page| page.number_fields())
            .collect();
        let all = c
            .placed
            .iter()
            .map(|(control, _)| *control)
            .chain(number_fields.iter().map(|field| field.spin()));
        for control in all {
            dialog::set_font(control, font, false);
        }
        for &(control, [left, top, width, height]) in &c.placed {
            // SAFETY: control はダイアログの子の有効なウィンドウ。
            unsafe {
                _ = MoveWindow(
                    control,
                    scale(left, dpi),
                    scale(top, dpi),
                    scale(width, dpi),
                    scale(height, dpi),
                    true,
                );
            }
        }
        // 上下の矢印は、入力欄を置いたあとで付ける（付けるときに入力欄の幅を詰める）。
        for field in &number_fields {
            field.attach_spin();
        }
        self.profiles.show_profile_command_warning(self);
    }

    /// `page` 番目のページのコントロールだけを見せる。
    fn show_page(&self, page: usize) {
        self.controls.pages.select(page);
        for (index, controls) in self.pages().into_iter().map(Page::controls).enumerate() {
            let show = if index == page { SW_SHOW } else { SW_HIDE };
            for &control in controls {
                // SAFETY: control はダイアログの子の有効なウィンドウ。
                unsafe { _ = ShowWindow(control, show) };
            }
        }
    }

    /// コントロールから編集中の設定を組み立てる。読めない入力があれば、その文と入力欄を返す。
    fn read(&self) -> Result<Config, (&'static str, HWND)> {
        let mut config = self.base.borrow().clone();
        // 読めない入力がいくつかあれば、この順で最初のもののエラーを見せる。
        self.appearance.read(self, &mut config)?;
        self.behavior.read(self, &mut config)?;
        self.profiles.read(self, &mut config)?;
        self.background.read(self, &mut config)?;
        self.keys.read(self, &mut config)?;
        self.colors.read(self, &mut config)?;
        Ok(config)
    }

    /// [`DialogContext::preview`] をして、読めた設定を返す。読めなければ、見せた文と入力欄を返す。
    fn preview_edited(&self) -> Result<Config, (&'static str, HWND)> {
        // SAFETY: hwnd は有効なウィンドウ。掛けていないタイマーを止めても害はない。
        unsafe { _ = KillTimer(Some(self.hwnd), PREVIEW_TIMER_ID) };
        let config = match self.read() {
            Ok(config) => config,
            Err((message, control)) => {
                set_text(self.controls.error, message);
                return Err((message, control));
            }
        };
        set_text(self.controls.error, "");
        self.preview_config(&config);
        Ok(config)
    }

    fn preview_config(&self, config: &Config) {
        if *self.previewed.borrow() == *config {
            return;
        }
        let Ok(mut host) = self.host.try_borrow_mut() else {
            tracing::debug!("skipped a nested settings preview");
            return;
        };
        host.preview(config);
        *self.previewed.borrow_mut() = config.clone();
    }

    /// 編集中の設定を反映して設定ファイルに書く。書けたら `true`。
    fn apply(&self) -> bool {
        let edited = match self.preview_edited() {
            Ok(edited) => edited,
            Err((_, control)) => {
                // 読めない入力欄のページを見せて、その入力欄へ移る。エラーメッセージは preview_edited が
                // 見せている。
                if let Some(page) = self
                    .pages()
                    .into_iter()
                    .position(|page| page.controls().contains(&control))
                {
                    self.show_page(page);
                }
                // SAFETY: control はダイアログの子の有効なウィンドウ。
                unsafe { _ = SetFocus(Some(control)) };
                return false;
            }
        };
        let base = self.base.borrow().clone();
        let saved = match self.host.try_borrow_mut() {
            Ok(mut host) => host.save(&base, &edited),
            Err(_) => return false,
        };
        if let Err(text) = saved {
            // 借用は持っていない。
            dialog::message_box(Some(self.hwnd), &text, "fantermo", MB_OK | MB_ICONWARNING);
            return false;
        }
        self.appearance.saved(&base);
        *self.base.borrow_mut() = edited;
        true
    }

    /// 最後に書いたときの設定に戻して閉じる。
    fn cancel(&self) {
        // SAFETY: hwnd は有効なウィンドウ。掛けていないタイマーを止めても害はない。
        unsafe { _ = KillTimer(Some(self.hwnd), PREVIEW_TIMER_ID) };
        let base = self.base.borrow().clone();
        self.preview_config(&base);
        self.close();
    }

    fn close(&self) {
        dialog::destroy(self.hwnd);
    }

    /// 上下の矢印のボタン `spin` が押された。相手の入力欄の数を `delta` の向きに一つ動かす。入力欄が
    /// 読めなければ、その入力欄のページの [`Page::step_start`] の設定の値から動かす。
    fn step(&self, spin: HWND, delta: i32) {
        for page in self.pages() {
            let fields = page.number_fields();
            let Some(field) = fields.iter().find(|field| field.spin() == spin) else {
                continue;
            };
            let start = page.step_start(self);
            // 借用は持っていない（入力欄の EN_CHANGE はこの中で届く）。
            field.step(delta, &start);
            break;
        }
        // 矢印は打ち間違えないので、待たずに反映する。
        self.preview();
    }
}

impl DialogContext for State<'_> {
    fn hwnd(&self) -> HWND {
        self.hwnd
    }

    fn base(&self) -> Ref<'_, Config> {
        self.base.borrow()
    }

    fn font(&self) -> HFONT {
        self.font.borrow().handle()
    }

    fn dpi(&self) -> u32 {
        self.dpi.get()
    }

    fn placed(&self) -> &[(HWND, DipRect)] {
        &self.controls.placed
    }

    fn preview(&self) {
        _ = self.preview_edited();
    }

    fn schedule_preview(&self) {
        // SAFETY: hwnd は有効なウィンドウ。同じ番号のタイマーは掛け直しになる。
        if unsafe {
            SetTimer(
                Some(self.hwnd),
                PREVIEW_TIMER_ID,
                PREVIEW_DELAY.as_millis() as u32,
                None,
            )
        } == 0
        {
            tracing::warn!(
                "failed to schedule the settings preview: {}",
                windows::core::Error::from_thread()
            );
            self.preview();
        }
    }
}

/// 拡張子が `extensions` の画像と、すべてのファイルの絞り込み。
fn filters_for<'a>(extensions: impl Iterator<Item = &'a str>) -> Vec<file_dialog::Filter> {
    let spec = extensions
        .map(|extension| format!("*.{extension}"))
        .collect::<Vec<_>>()
        .join(";");
    vec![
        file_dialog::Filter {
            name: "画像".to_owned(),
            spec,
        },
        file_dialog::Filter {
            name: "すべてのファイル".to_owned(),
            spec: "*.*".to_owned(),
        },
    ]
}

extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: run が結び付けたのは State。
    let Some(state) = (unsafe { dialog::state_of::<State>(hwnd) }) else {
        // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    if let Some(result) = dialog::handle_common_message(msg, wparam, lparam, Some(IDOK.0)) {
        return result;
    }
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            let code = ((wparam.0 >> 16) & 0xffff) as u32;
            match (id, code) {
                (id, _) if id == IDOK.0 => {
                    if state.apply() {
                        state.close();
                    }
                }
                (id, _) if id == IDCANCEL.0 => state.cancel(),
                (ID_APPLY, BN_CLICKED) => {
                    state.apply();
                }
                (ID_PAGES, LBN_SELCHANGE) => {
                    if let Some(page) = state.controls.pages.selected()
                        && page < PAGES.len()
                    {
                        state.show_page(page);
                    }
                }
                // ページのコントロールの ID は、ページごとの範囲（各ページの `ID_RANGE`）に収まり、ページの
                // 間で重ならないので、受け持つページが処理したら止める。
                _ => {
                    _ = state
                        .pages()
                        .into_iter()
                        .any(|page| page.on_command(state, id, code));
                }
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => match state.colors.swatch_brush(HWND(lparam.0 as *mut _)) {
            Some(brush) => LRESULT(brush.0 as isize),
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
            None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        },
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY の lparam は NMHDR で始まる構造体を指す。
            let header = unsafe { &*(lparam.0 as *const NMHDR) };
            if header.code == UDN_DELTAPOS {
                // SAFETY: UDN_DELTAPOS の lparam は NMUPDOWN を指す。
                let updown = unsafe { &*(lparam.0 as *const NMUPDOWN) };
                state.step(header.hwndFrom, updown.iDelta);
                // 矢印のボタン自身の位置は動かさない（入力欄の数はこちらで決める）。
                return LRESULT(1);
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == PREVIEW_TIMER_ID => {
            state.preview();
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // SAFETY: lparam は WM_DPICHANGED で受け取ったもの。
            unsafe { dialog::move_to_suggested_rect(hwnd, lparam) };
            state.set_dpi(dialog::dpi_of(wparam));
            LRESULT(0)
        }
        WM_CLOSE => {
            state.cancel();
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

#[cfg(test)]
mod tests {
    use super::*;
    use super::{
        appearance::{FONT_SIZE_KIND, PADDING_KIND, SCROLL_BAR_WIDTH_KIND},
        background::{BACKGROUND_ITEM_OPACITY_KIND, BACKGROUND_OPACITY_KIND},
        behavior::PASTE_CONFIRM_SIZE_KIND,
        widgets::NumberKind,
    };

    #[test]
    fn number_fields_read_what_they_show() {
        // `parse` に渡す項目の名前の誤りは、このテストだけが見つける。`NumberKind` の定数を足したら、
        // ここにも足す（`Page::number_fields` はウィンドウが要るので使えない）。
        fn reads_back<T: Copy + PartialEq + std::fmt::Debug>(kind: &NumberKind<T>) {
            let value = (kind.start)(&Config::default());
            assert_eq!(
                (kind.parse)(&(kind.format)(value)),
                Some(value),
                "{}",
                kind.message
            );
        }
        reads_back(&FONT_SIZE_KIND);
        reads_back(&SCROLL_BAR_WIDTH_KIND);
        reads_back(&PADDING_KIND);
        reads_back(&PASTE_CONFIRM_SIZE_KIND);
        reads_back(&BACKGROUND_OPACITY_KIND);
        reads_back(&BACKGROUND_ITEM_OPACITY_KIND);
    }

    #[test]
    fn the_pages_use_separate_ranges_of_ids() {
        // 各ページの ID がその範囲に収まることは、それぞれのページのテストが見る。
        let ranges = [
            appearance::ID_RANGE,
            background::ID_RANGE,
            behavior::ID_RANGE,
            profiles::ID_RANGE,
            keys::ID_RANGE,
            colors::ID_RANGE,
        ];
        for (index, range) in ranges.iter().enumerate() {
            for other in &ranges[index + 1..] {
                assert!(
                    range.end() < other.start() || other.end() < range.start(),
                    "{range:?} and {other:?}"
                );
            }
            for id in [IDOK.0, IDCANCEL.0, ID_PAGES, ID_APPLY, ID_ERROR] {
                assert!(!range.contains(&id), "{id} in {range:?}");
            }
        }
    }
}
