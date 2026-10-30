//! タブバー。ウィンドウの上端に、端末ごとのタブを並べる。端末が一つのときも出す。
//!
//! 寸法は端末のフォントによらず DPI から決め、セルの格子によらずピクセルの位置で並べる。色は
//! [`Palette`] にまとめ、既定では背景と表示していないタブを Windows のコントロールの色より少し濃い
//! グレー、表示しているタブをウィンドウの背景の色（白）で上の角を丸め、左・右・上に細い縁の線を引いて描く。
//! 並べ方（どこがどのタブか、タイトルの省略、ベルの印の位置）と、描くもの（[`Paint`]）の組み立ては、
//! ウィンドウや描画に依存しない関数にしてある。

use fantermo_core::{config::TAB_BAR_COLOR_KEYS, frame::Rgb, vt};

use crate::{
    paint::{Area, Corner, Icon, Paint, mix, scale},
    tab_icon::TabIcon,
};

/// タブバーの高さ。
const BAR_HEIGHT: u32 = 26;

/// タブバーの上の辺（ウィンドウのタイトルバーの下）からタブの上の辺までの空き。
const TOP_MARGIN: u32 = 2;

/// 表示しているタブとマウスを載せているタブの、上の角の半径。表示しているタブの縁の線の外側の半径。
const CORNER_RADIUS: u32 = 5;

/// 表示しているタブの左・右・上の縁の線の太さ。線はタブの矩形の内側に引く。
const ACTIVE_TAB_BORDER: u32 = 1;

/// ウィンドウの左端とメニューのボタンの間、ボタンとタブの間のすき間。
const MARGIN: u32 = 6;

/// タブの中の左右の余白。タブの並びの右の余白にも使う。
const PADDING: u32 = 10;

/// 新しいタブのボタンとメニューのボタン（丸い背景）の直径。
const BUTTON: u32 = 18;

/// 新しいタブのボタンの「+」、プロファイルの一覧のボタンの「▾」、メニューのボタンの「≡」の一辺。
const BUTTON_MARK: u32 = 8;

/// タブを閉じるボタン（丸い背景）の直径。
const CLOSE_BUTTON: u32 = 16;

/// 閉じるボタンの「×」の一辺。
const CLOSE_MARK: u32 = 8;

/// ベルの印の円の直径。
const BELL_MARK: u32 = 6;

/// ベルの印とタイトルの間のすき間。
const BELL_GAP: u32 = 6;

/// タブのアイコンの一辺。タブの高さから上下に少しずつ空ける。
const TAB_ICON: u32 = 18;

/// タブの左端とアイコンの間の空き。
const TAB_ICON_MARGIN: u32 = 6;

/// アイコンとその右のもの（ベルの印かタイトル）の間のすき間。
const TAB_ICON_GAP: u32 = 5;

/// タブの間の区切りの線の太さ。
const SEPARATOR_WIDTH: u32 = 1;

/// タブの間の区切りの線の高さ。
const SEPARATOR_HEIGHT: u32 = 12;

/// タブの幅の上限。
const MAX_TAB_WIDTH: u32 = 240;

/// タブが多いときに縮める、タブの幅の下限。これより狭くなるほど多ければ、入りきらないタブを描かない。
const MIN_TAB_WIDTH: u32 = 72;

/// 収まらないタイトルの末尾に付ける印。
const ELLIPSIS: &str = "…";

/// 表示していない間にベルが鳴ったタブの、タイトルの前の印（円）の既定の色。
const BELL_COLOR: Rgb = Rgb {
    r: 0xf0,
    g: 0xb0,
    b: 0x30,
};

/// タブバーの背景の色を、コントロールの背景の色から文字の色へ寄せる割合。表示しているタブ（ウィンドウの
/// 背景の色）との差を大きくする。
const BAR_TINT: f32 = 0.15;

/// マウスを載せている表示していないタブの色を、タブバーの背景の色からウィンドウの背景の色へ寄せる割合。
const HOVERED_TAB_TINT: f32 = 0.5;

/// マウスを載せているボタン（×、+、≡）の丸い背景の色を、ボタンの下の色から文字の色へ寄せる割合。
const HOVERED_BUTTON_TINT: f32 = 0.15;

/// 押しているボタンの丸い背景の色を、ボタンの下の色から文字の色へ寄せる割合。
const PRESSED_BUTTON_TINT: f32 = 0.28;

/// DPI が `dpi` のウィンドウでの、タブバーの高さ（ピクセル）。
pub fn height(dpi: u32) -> u32 {
    Metrics { dpi }.bar_height()
}

/// タブバーの寸法を決めるもの。寸法はウィンドウの DPI に比例させる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub dpi: u32,
}

/// タイトルの文字列の大きさを測るもの。ウィンドウでは Windows の UI のフォントで測る。
pub trait Measure {
    /// `text` を 1 行に組んだ幅（ピクセル）。
    fn width(&self, text: &str) -> u32;
    /// 1 行の高さ（ピクセル）。
    fn height(&self) -> u32;
}

impl Metrics {
    fn scale(self, px: u32) -> u32 {
        scale(px, self.dpi)
    }

    /// タブバーの高さ。
    fn bar_height(self) -> u32 {
        self.scale(BAR_HEIGHT)
    }

    /// タブバーの上の辺からタブの上の辺までの空き。
    fn top_margin(self) -> u32 {
        self.scale(TOP_MARGIN)
    }

    /// 表示しているタブとマウスを載せているタブの、上の角の半径。
    fn corner_radius(self) -> u32 {
        self.scale(CORNER_RADIUS)
    }

    /// 表示しているタブの縁の線の太さ。
    fn active_border(self) -> u32 {
        self.scale(ACTIVE_TAB_BORDER)
    }

    /// ウィンドウの左端とメニューのボタンの間、ボタンとタブの間のすき間。
    fn margin(self) -> u32 {
        self.scale(MARGIN)
    }

    /// タブの中の左右の余白。タブの並びの右の余白にも使う。
    fn padding(self) -> u32 {
        self.scale(PADDING)
    }

    /// ベルの印の円の直径。
    fn mark_size(self) -> u32 {
        self.scale(BELL_MARK)
    }

    /// ベルの印とタイトルの間のすき間。
    fn mark_gap(self) -> u32 {
        self.scale(BELL_GAP)
    }

    /// タブのアイコンの一辺。
    fn tab_icon(self) -> u32 {
        self.scale(TAB_ICON)
    }

    /// タブの左端とアイコンの間の空き。
    fn tab_icon_margin(self) -> u32 {
        self.scale(TAB_ICON_MARGIN)
    }

    /// アイコンとその右のものの間のすき間。
    fn tab_icon_gap(self) -> u32 {
        self.scale(TAB_ICON_GAP)
    }

    /// タブの間の区切りの線の太さと高さ。
    fn separator(self) -> (u32, u32) {
        (self.scale(SEPARATOR_WIDTH), self.scale(SEPARATOR_HEIGHT))
    }

    /// タブを閉じるボタン（丸い背景）の直径。
    fn close_button(self) -> u32 {
        self.scale(CLOSE_BUTTON)
    }

    /// 閉じるボタンの「×」の一辺。
    fn close_mark(self) -> u32 {
        self.scale(CLOSE_MARK)
    }

    /// 新しいタブのボタンとメニューのボタン（丸い背景）の直径。
    fn button(self) -> u32 {
        self.scale(BUTTON)
    }

    /// 新しいタブのボタンの「+」、プロファイルの一覧のボタンの「▾」、メニューのボタンの「≡」の一辺。
    fn plus_mark(self) -> u32 {
        self.scale(BUTTON_MARK)
    }

    /// タブの幅の下限と上限。
    fn tab_width_range(self) -> (u32, u32) {
        (self.scale(MIN_TAB_WIDTH), self.scale(MAX_TAB_WIDTH))
    }
}

/// タブバーの中のタブ一つ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabSlot {
    /// 端末の並びの中の位置。
    pub index: usize,
    /// タブを描く矩形。上の空きの下から、タブバーの下の辺まで。
    pub area: Area,
    /// タブを閉じるボタン。タブの右端に置く。
    pub close: Area,
}

/// タブバーの並べ方。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    metrics: Metrics,
    /// タブバーの幅（ピクセル）。
    width: u32,
    /// 描くタブ。左から並べた順。入りきらないタブは含まない。
    tabs: Vec<TabSlot>,
    /// 新しいタブのボタン。タブの並びの右に置く。
    new_tab: Area,
    /// プロファイルの一覧のボタン。新しいタブのボタンのすぐ右に置く。
    profiles: Area,
    /// メニューのボタン。左端に置く。
    menu: Area,
}

/// タブバーの上の、押して働くもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// 端末の並びの中の位置がこの値のタブ。
    Tab(usize),
    /// 端末の並びの中の位置がこの値のタブの、閉じるボタン。
    Close(usize),
    /// 新しいタブのボタン。
    NewTab,
    /// プロファイルの一覧のボタン。
    Profiles,
    /// メニューのボタン。
    Menu,
}

impl Hit {
    /// タブかその閉じるボタンなら、そのタブの位置。
    pub fn tab(self) -> Option<usize> {
        match self {
            Hit::Tab(index) | Hit::Close(index) => Some(index),
            Hit::NewTab | Hit::Profiles | Hit::Menu => None,
        }
    }
}

/// 幅 `width` のタブバーの左端にメニューのボタンを置き、その右に `tabs` 個のタブを並べ、さらに右に
/// 新しいタブのボタンと、そのすぐ右にプロファイルの一覧のボタンを置く。タブは同じ幅で、上限は [`MAX_TAB_WIDTH`]。多ければ [`MIN_TAB_WIDTH`]
/// まで縮め、それでも入りきらなければ、表示しているタブ `active` を含む続きのタブだけを並べる。
pub fn layout(metrics: Metrics, width: u32, tabs: usize, active: usize) -> Layout {
    let top = metrics.top_margin();
    let tab_height = metrics.bar_height() - top;
    // ボタンは、タブの高さの真ん中に置く。
    let button_size = metrics.button();
    let button_at = |x: u32| Area {
        x,
        y: top + tab_height.saturating_sub(button_size) / 2,
        width: button_size,
        height: button_size,
    };
    // ウィンドウの左端とメニューのボタンの間、ボタンとタブの間には同じすき間を空ける。
    let gap = metrics.margin();
    let menu = button_at(gap);
    let start = menu.right() + gap;
    let mut layout = Layout {
        metrics,
        width,
        tabs: Vec::new(),
        new_tab: button_at(start),
        profiles: button_at(start + button_size),
        menu,
    };
    if tabs == 0 {
        return layout;
    }
    // 新しいタブとプロファイルの一覧のボタンと、タブとの間のすき間の分を右に残す。
    let end = width.saturating_sub(metrics.padding() + gap + 2 * button_size);
    let room = end.saturating_sub(start);
    let (min_width, max_width) = metrics.tab_width_range();
    let fitting = usize::try_from(room / min_width)
        .unwrap_or(usize::MAX)
        .max(1);
    let shown = tabs.min(fitting);
    let tab_width = (room / u32::try_from(shown).unwrap_or(u32::MAX)).min(max_width);
    if tab_width == 0 {
        return layout;
    }
    let first = active
        .min(tabs - 1)
        .saturating_sub(shown - 1)
        .min(tabs - shown);
    let close_size = metrics.close_button();
    layout.tabs = (0..shown)
        .map(|offset| {
            let area = Area {
                x: start + tab_width * offset as u32,
                y: top,
                width: tab_width,
                height: tab_height,
            };
            let close_x = area
                .right()
                .saturating_sub(metrics.padding() / 2 + close_size)
                .max(area.x);
            TabSlot {
                index: first + offset,
                area,
                close: Area {
                    x: close_x,
                    y: area.y + area.height.saturating_sub(close_size) / 2,
                    width: close_size.min(area.right() - close_x),
                    height: close_size,
                },
            }
        })
        .collect();
    if let Some(last) = layout.tabs.last() {
        layout.new_tab = button_at(last.area.right() + gap);
        layout.profiles = button_at(layout.new_tab.right());
    }
    layout
}

impl Layout {
    /// メニューのボタン。
    pub fn menu(&self) -> Area {
        self.menu
    }

    /// 新しいタブのボタンと、プロファイルの一覧のボタン。
    pub fn new_tab_buttons(&self) -> (Area, Area) {
        (self.new_tab, self.profiles)
    }

    /// 描くタブ。
    #[cfg(test)]
    fn tabs(&self) -> &[TabSlot] {
        &self.tabs
    }

    /// タブバーの中の (`x`, `y`) にあるもの。タブとボタンの上下の空きも、そのタブやボタンとみなす。
    /// 何もなければ `None`。
    pub fn hit(&self, x: i32, y: i32) -> Option<Hit> {
        if let Some(tab) = self.tabs.iter().find(|tab| tab.close.contains(x, y)) {
            return Some(Hit::Close(tab.index));
        }
        if let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| column(tab.area, self.metrics).contains(x, y))
        {
            return Some(Hit::Tab(tab.index));
        }
        if column(self.new_tab, self.metrics).contains(x, y) {
            return Some(Hit::NewTab);
        }
        if column(self.profiles, self.metrics).contains(x, y) {
            return Some(Hit::Profiles);
        }
        // メニューのボタンは、ウィンドウの左端からとみなす。
        let menu = Area {
            x: 0,
            width: self.menu.right(),
            ..self.menu
        };
        column(menu, self.metrics)
            .contains(x, y)
            .then_some(Hit::Menu)
    }
}

/// `area` の横の範囲で、タブバーの上から下まで。
fn column(area: Area, metrics: Metrics) -> Area {
    Area {
        y: 0,
        height: metrics.bar_height(),
        ..area
    }
}

/// `title` を幅 `width`（ピクセル）に収めた文字列。幅は `measure` で測る。収まらなければ、書記素
/// クラスタの切れ目で切って末尾を [`ELLIPSIS`] にする（[`ELLIPSIS`] も入らなければ空）。幅のない書記素
/// クラスタ（制御文字など）は除く。
pub fn fit_title(title: &str, width: u32, measure: &impl Measure) -> String {
    let graphemes: Vec<vt::Grapheme> = vt::split_graphemes(title)
        .into_iter()
        .filter(|grapheme| grapheme.width > 0)
        .collect();
    let prefix = |count: usize| -> String {
        graphemes[..count]
            .iter()
            .map(|grapheme| grapheme.text.as_str())
            .collect()
    };
    let whole = prefix(graphemes.len());
    if measure.width(&whole) <= width {
        return whole;
    }
    let shortened = |count: usize| prefix(count) + ELLIPSIS;
    if measure.width(ELLIPSIS) > width {
        return String::new();
    }
    // 先頭の `fitting` 個に印を付けたものは収まり、`too_many` 個では収まらない。
    let (mut fitting, mut too_many) = (0, graphemes.len());
    while too_many - fitting > 1 {
        let middle = (fitting + too_many) / 2;
        if measure.width(&shortened(middle)) <= width {
            fitting = middle;
        } else {
            too_many = middle;
        }
    }
    shortened(fitting)
}

/// タブ一つの中の、アイコンとベルの印とタイトルの位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TabContent {
    /// アイコンを描く正方形。タブが狭くて入らないときは `None`。
    icon: Option<Area>,
    /// ベルの印の円を描く矩形。印を付けないときは `None`。
    mark: Option<Area>,
    /// タイトルの左端。
    title_x: u32,
    /// タイトルに充てる幅（ピクセル）。
    title_width: u32,
    /// タイトルの上端。
    text_y: u32,
}

/// タブ `slot` の中の、閉じるボタンより左に、アイコンと、ベルの印（`bell` のとき）と、1 行の高さが
/// `text_height` のタイトルを左から順に置く。閉じるボタンを出していないときも、その幅は空けておく（マウスを
/// 載せてもタイトルの省略が変わらないように）。アイコンと印の幅は、タイトルを省略する前に取る。アイコンと
/// 印が閉じるボタンの左に並ばないほどタブが狭ければ、状態を知らせる印を残してアイコンを出さない。タイトルに
/// 残る幅がなくても印は消さず、余白に収まらなければタブの真ん中に置き、タブより大きければタブの幅に
/// 縮める。
fn tab_content(metrics: Metrics, slot: &TabSlot, bell: bool, text_height: u32) -> TabContent {
    let tab = slot.area;
    let right = slot.close.x;
    let text_y = tab.y + tab.height.saturating_sub(text_height) / 2;
    let icon_size = metrics.tab_icon();
    let icon_x = tab.x + metrics.tab_icon_margin();
    let needed_after_icon = if bell {
        metrics.tab_icon_gap() + metrics.mark_size()
    } else {
        0
    };
    let icon =
        (icon_size <= tab.height && icon_x + icon_size + needed_after_icon <= right).then(|| {
            Area {
                x: icon_x,
                y: tab.y + (tab.height - icon_size) / 2,
                width: icon_size,
                height: icon_size,
            }
        });
    let left = match icon {
        Some(icon) => icon.right() + metrics.tab_icon_gap(),
        None => tab.x + metrics.padding(),
    };
    let mut title_x = left;
    let mut mark = None;
    if bell {
        let size = metrics.mark_size().min(tab.width).min(tab.height);
        let x = if left + size <= right {
            left
        } else {
            tab.x + (tab.width - size) / 2
        };
        mark = Some(Area {
            x,
            y: tab.y + (tab.height - size) / 2,
            width: size,
            height: size,
        });
        title_x = x + size + metrics.mark_gap();
    }
    TabContent {
        icon,
        mark,
        title_x,
        title_width: right.saturating_sub(title_x),
        text_y,
    }
}

/// タブ一つに描くもの。
#[derive(Clone, Copy, Debug)]
pub struct TabView<'a> {
    pub title: &'a str,
    /// 表示していない間にベルが鳴った。タイトルの前に印を付ける。
    pub bell: bool,
    /// タイトルの左に出すアイコン。
    pub icon: &'a TabIcon,
}

/// Windows のシステムの色（`GetSysColor`）。タブバーの既定の色（[`Palette::from_system`]）を決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemColors {
    /// コントロールの背景（`COLOR_BTNFACE`）。
    pub face: Rgb,
    /// コントロールの文字（`COLOR_BTNTEXT`）。
    pub text: Rgb,
    /// 薄い文字（`COLOR_GRAYTEXT`）。
    pub gray_text: Rgb,
    /// コントロールの暗い縁（`COLOR_BTNSHADOW`）。
    pub shadow: Rgb,
    /// ウィンドウの背景（`COLOR_WINDOW`）。
    pub window: Rgb,
    /// ウィンドウの文字（`COLOR_WINDOWTEXT`）。
    pub window_text: Rgb,
}

/// タブバーの色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// タブバーの背景。表示していないタブも、マウスを載せていなければこの色のまま。
    pub bar: Rgb,
    /// 表示しているタブ。
    pub active_tab: Rgb,
    /// 表示しているタブの左・右・上の縁の線。
    pub active_border: Rgb,
    /// 表示しているタブのタイトルと閉じるボタンの「×」。
    pub active_title: Rgb,
    /// 表示していないタブのタイトルと閉じるボタンの「×」、マウスを載せている「+」「▾」「≡」。
    pub inactive_title: Rgb,
    /// 表示していないタブの間の区切りの線。
    pub separator: Rgb,
    /// マウスを載せている表示していないタブ。
    pub hovered_tab: Rgb,
    /// マウスを載せていない、新しいタブのボタンの「+」、プロファイルの一覧のボタンの「▾」、メニューの
    /// ボタンの「≡」。
    pub idle_mark: Rgb,
    /// 表示していない間にベルが鳴ったタブの印。
    pub bell: Rgb,
}

impl Palette {
    /// Windows のシステムの色から決める既定の色。背景と表示していないタブはコントロールの背景の色を文字の
    /// 色へ少し寄せた色、表示しているタブはウィンドウの背景と文字の色で、縁の線はコントロールの暗い縁の色。
    /// マウスを載せているタブは、背景とウィンドウの背景の間の色。
    pub fn from_system(system: SystemColors) -> Self {
        let bar = mix(system.face, system.text, BAR_TINT);
        Self {
            bar,
            active_tab: system.window,
            active_border: system.shadow,
            active_title: system.window_text,
            inactive_title: system.text,
            separator: system.shadow,
            hovered_tab: mix(bar, system.window, HOVERED_TAB_TINT),
            idle_mark: system.gray_text,
            bell: BELL_COLOR,
        }
    }

    /// 設定の色 `colors`（[`TAB_BAR_COLOR_KEYS`] の順）のうち、書いた色だけで上書きした色。
    pub fn with_colors(mut self, colors: &[Option<Rgb>]) -> Self {
        for (field, color) in self.fields_mut().into_iter().zip(colors) {
            if let Some(color) = color {
                *field = *color;
            }
        }
        self
    }

    /// 色の並び。[`TAB_BAR_COLOR_KEYS`] の順。
    pub fn colors(&self) -> [Rgb; TAB_BAR_COLOR_KEYS.len()] {
        let mut copy = *self;
        copy.fields_mut().map(|field| *field)
    }

    /// 色の欄。[`TAB_BAR_COLOR_KEYS`] の順。
    fn fields_mut(&mut self) -> [&mut Rgb; TAB_BAR_COLOR_KEYS.len()] {
        [
            &mut self.bar,
            &mut self.active_tab,
            &mut self.active_border,
            &mut self.active_title,
            &mut self.inactive_title,
            &mut self.separator,
            &mut self.hovered_tab,
            &mut self.idle_mark,
            &mut self.bell,
        ]
    }

    /// 色 `base` の上に、色 `ink` の印で描くボタンの丸い背景の色。`pressed` なら押しているとき、
    /// そうでなければマウスを載せているとき。
    fn button(&self, base: Rgb, ink: Rgb, pressed: bool) -> Rgb {
        let tint = if pressed {
            PRESSED_BUTTON_TINT
        } else {
            HOVERED_BUTTON_TINT
        };
        mix(base, ink, tint)
    }
}

/// マウスを載せているものと、押しているもの。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Highlight {
    pub hovered: Option<Hit>,
    /// 左ボタンを押したときにマウスを載せていたもの。マウスを載せているときだけ、押している色で描く。
    pub pressed: Option<Hit>,
}

impl Highlight {
    /// `hit` の丸い背景の色を決めるとき、押しているか。マウスを載せていなければ `None`。
    fn state(&self, hit: Hit) -> Option<bool> {
        (self.hovered == Some(hit)).then_some(self.pressed == Some(hit))
    }

    /// マウスを載せているタブの位置。
    fn hovered_tab(&self) -> Option<usize> {
        self.hovered.and_then(Hit::tab)
    }
}

/// `layout` のとおりに並べたタブバーに描くもの。`tabs` は端末の並びの順のタブ、`active` は表示して
/// いるタブの位置、`highlight` はマウスを載せているものと押しているもの。色は `colors`、タイトルの幅は
/// `measure` で測る。閉じるボタンは、表示しているタブと
/// マウスを載せているタブにだけ出す。描くのはタブバーの高さまでで、その下の端末の画面の余白には描かない。
pub fn paints(
    layout: &Layout,
    tabs: &[TabView],
    active: usize,
    highlight: Highlight,
    colors: &Palette,
    measure: &impl Measure,
) -> Vec<Paint> {
    let metrics = layout.metrics;
    let hovered = highlight.hovered_tab();
    let raised = |index: usize| index == active || Some(index) == hovered;
    let mut paints = vec![Paint::Fill {
        area: Area {
            x: 0,
            y: 0,
            width: layout.width,
            height: metrics.bar_height(),
        },
        color: colors.bar,
    }];
    for pair in layout.tabs.windows(2) {
        let [left, right] = pair else { continue };
        if raised(left.index) || raised(right.index) {
            continue;
        }
        let (width, height) = metrics.separator();
        let area = right.area;
        paints.push(Paint::Fill {
            area: Area {
                x: area.x.saturating_sub(width / 2),
                y: area.y + (area.height - height) / 2,
                width,
                height,
            },
            color: colors.separator,
        });
    }
    if let Some(tab) = layout
        .tabs
        .iter()
        .find(|tab| Some(tab.index) == hovered && tab.index != active)
    {
        push_raised_tab(
            &mut paints,
            tab.area,
            metrics.corner_radius(),
            colors.hovered_tab,
        );
    }
    if let Some(tab) = layout.tabs.iter().find(|tab| tab.index == active) {
        push_active_tab(&mut paints, metrics, tab.area, colors);
    }
    for slot in &layout.tabs {
        let Some(tab) = tabs.get(slot.index) else {
            continue;
        };
        let is_active = slot.index == active;
        let (tab_color, title_color) = if is_active {
            (colors.active_tab, colors.active_title)
        } else if Some(slot.index) == hovered {
            (colors.hovered_tab, colors.inactive_title)
        } else {
            (colors.bar, colors.inactive_title)
        };
        let content = tab_content(metrics, slot, tab.bell, measure.height());
        if let Some(area) = content.icon {
            paints.push(Paint::TabIcon {
                icon: tab.icon.clone(),
                area,
            });
        }
        if let Some(mark) = content.mark {
            paints.push(Paint::Icon {
                icon: Icon::Dot,
                area: mark,
                color: colors.bell,
            });
        }
        let title = fit_title(tab.title, content.title_width, measure);
        if !title.is_empty() {
            paints.push(Paint::Label {
                x: content.title_x,
                y: content.text_y,
                text: title,
                color: title_color,
            });
        }
        if raised(slot.index) {
            push_button(
                &mut paints,
                colors,
                Button {
                    area: slot.close,
                    mark: (Icon::Close, metrics.close_mark(), title_color),
                    state: highlight.state(Hit::Close(slot.index)),
                    base: tab_color,
                    ink: title_color,
                },
            );
        }
    }
    for (hit, area, icon) in [
        (Hit::NewTab, layout.new_tab, Icon::Plus),
        (Hit::Profiles, layout.profiles, Icon::Chevron),
        (Hit::Menu, layout.menu, Icon::Menu),
    ] {
        let mark_color = if highlight.hovered == Some(hit) {
            colors.inactive_title
        } else {
            colors.idle_mark
        };
        push_button(
            &mut paints,
            colors,
            Button {
                area,
                mark: (icon, metrics.plus_mark(), mark_color),
                state: highlight.state(hit),
                base: colors.bar,
                ink: colors.inactive_title,
            },
        );
    }
    paints
}

/// タブバーのボタン（×、+、▾、≡）一つ。
struct Button {
    /// ボタンの矩形。マウスを載せていれば、ここに丸い背景を描く。
    area: Area,
    /// 真ん中に描く印と、その一辺と色。
    mark: (Icon, u32, Rgb),
    /// マウスを載せていれば、押しているか。
    state: Option<bool>,
    /// ボタンの下の色。丸い背景の色は、ここから `ink` へ寄せて決める。
    base: Rgb,
    /// ボタンの下の背景に置く文字の色。
    ink: Rgb,
}

fn push_button(paints: &mut Vec<Paint>, colors: &Palette, button: Button) {
    let Button {
        area,
        mark: (icon, mark_size, mark_color),
        state,
        base,
        ink,
    } = button;
    if let Some(pressed) = state {
        paints.push(Paint::Icon {
            icon: Icon::Dot,
            area,
            color: colors.button(base, ink, pressed),
        });
    }
    let size = mark_size.min(area.width).min(area.height);
    paints.push(Paint::Icon {
        icon,
        area: Area {
            x: area.x + (area.width - size) / 2,
            y: area.y + (area.height - size) / 2,
            width: size,
            height: size,
        },
        color: mark_color,
    });
}

/// 矩形 `area` に、表示しているタブを描く。左・右・上の縁の線を矩形の内側に引き、その内側を
/// 表示しているタブの色で塗る。縁の線の角は、タブの丸めた角に沿わせる（内側の角の半径を線の太さの
/// 分だけ小さくし、同じ中心の円にする）。下の辺には線を引かない。
fn push_active_tab(paints: &mut Vec<Paint>, metrics: Metrics, area: Area, colors: &Palette) {
    let radius = metrics.corner_radius();
    let border = metrics.active_border();
    push_raised_tab(paints, area, radius, colors.active_border);
    let inner = Area {
        x: area.x + border,
        y: area.y + border,
        width: area.width.saturating_sub(border * 2),
        height: area.height.saturating_sub(border),
    };
    push_raised_tab(
        paints,
        inner,
        radius.saturating_sub(border),
        colors.active_tab,
    );
}

/// 矩形 `area` に、上の角を半径 `radius` で丸めたタブを `color` で描く。下の辺はまっすぐ。
fn push_raised_tab(paints: &mut Vec<Paint>, area: Area, radius: u32, color: Rgb) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if radius == 0 || area.width < radius * 2 || area.height < radius {
        paints.push(Paint::Fill { area, color });
        return;
    }
    let square = |x: u32, y: u32| Area {
        x,
        y,
        width: radius,
        height: radius,
    };
    paints.push(Paint::Fill {
        area: Area {
            x: area.x + radius,
            y: area.y,
            width: area.width - radius * 2,
            height: radius,
        },
        color,
    });
    paints.push(Paint::Fill {
        area: Area {
            y: area.y + radius,
            height: area.height - radius,
            ..area
        },
        color,
    });
    paints.push(Paint::Icon {
        icon: Icon::Corner(Corner::TopLeft),
        area: square(area.x, area.y),
        color,
    });
    paints.push(Paint::Icon {
        icon: Icon::Corner(Corner::TopRight),
        area: square(area.right() - radius, area.y),
        color,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn gray(level: u8) -> Rgb {
        Rgb {
            r: level,
            g: level,
            b: level,
        }
    }

    /// Windows 11 の既定に近いシステムの色。色で描くものを見分けられるよう、どれも違う色にする。
    const SYSTEM: SystemColors = SystemColors {
        face: gray(0xf0),
        text: gray(0x20),
        gray_text: gray(0x6d),
        shadow: gray(0xa0),
        window: gray(0xfe),
        window_text: gray(0x10),
    };

    /// タブのアイコン。
    static ICON: TabIcon = TabIcon::without_profile();

    /// 表示しているタブの色。
    const WINDOW: Rgb = SYSTEM.window;

    /// 表示しているタブのタイトルの色。
    const WINDOW_TEXT: Rgb = SYSTEM.window_text;

    fn palette() -> Palette {
        Palette::from_system(SYSTEM)
    }

    /// 192 DPI の寸法。タブバーの高さ 52（その下の端末の画面の上の余白 8 は別）、上の空き 4、角の半径
    /// 10、すき間 12、余白 20、ボタン 36、閉じるボタン 32、アイコン 36（タブの左端から 12、右に 10 空ける）、
    /// ベルの印 12、印のすき間 12、タブの幅 144〜480。
    const METRICS: Metrics = Metrics { dpi: 192 };

    /// セルの幅が 16 ピクセルの 100 桁のウィンドウのタブバーの幅。
    const WIDTH: u32 = 100 * 16;

    /// 幅が ASCII の文字と [`ELLIPSIS`] は 10、ほかの文字は 20 ピクセル、1 行の高さが 32 ピクセルの書体。
    struct FixedWidth;

    impl Measure for FixedWidth {
        fn width(&self, text: &str) -> u32 {
            text.chars()
                .map(|c| {
                    if c.is_ascii() || ELLIPSIS.contains(c) {
                        10
                    } else {
                        20
                    }
                })
                .sum()
        }

        fn height(&self) -> u32 {
            32
        }
    }

    fn fit(title: &str, width: u32) -> String {
        fit_title(title, width, &FixedWidth)
    }

    fn xs(layout: &Layout) -> Vec<(usize, u32, u32)> {
        layout
            .tabs()
            .iter()
            .map(|tab| (tab.index, tab.area.x, tab.area.width))
            .collect()
    }

    #[test]
    fn the_bar_height_follows_the_dpi() {
        // 96 DPI でタブバーの高さ 26。
        assert_eq!(height(96), 26);
        assert_eq!(height(144), 39);
        assert_eq!(height(192), 52);
        assert_eq!(height(120), 33);
        assert_eq!(height(1), 1);
        assert_eq!(METRICS.bar_height(), 52);
        let base = Metrics { dpi: 96 };
        assert_eq!((base.button(), METRICS.button()), (18, 36));
        assert_eq!((base.top_margin(), METRICS.top_margin()), (2, 4));
    }

    fn square(x: u32, y: u32, size: u32) -> Area {
        Area {
            x,
            y,
            width: size,
            height: size,
        }
    }

    #[test]
    fn a_single_tab_is_laid_out() {
        let layout = layout(METRICS, WIDTH, 1, 0);
        // メニューのボタンは、左端からすき間を空けて、タブの高さの真ん中に置く。
        assert_eq!(layout.menu(), square(12, 10, 36));
        // タブは、メニューのボタンの右に同じすき間を空けて置く。
        assert_eq!(xs(&layout), vec![(0, 60, 480)]);
        let tab = layout.tabs()[0];
        // 上の空きの下から、タブバーの下の辺まで。
        assert_eq!((tab.area.y, tab.area.bottom()), (4, 52));
        // 閉じるボタンは右の余白の半分を残してタブの右端に、高さの真ん中に置く。
        assert_eq!(tab.close, square(498, 12, 32));
        // 新しいタブのボタンは、タブの右に同じすき間を空けて置き、そのすぐ右にプロファイルの一覧の
        // ボタン。
        assert_eq!(layout.new_tab, square(552, 10, 36));
        assert_eq!(layout.profiles, square(588, 10, 36));
        assert_eq!(layout.new_tab_buttons(), (layout.new_tab, layout.profiles));
    }

    #[test]
    fn few_tabs_have_the_widest_width() {
        let layout = layout(METRICS, WIDTH, 2, 1);
        assert_eq!(xs(&layout), vec![(0, 60, 480), (1, 540, 480)]);
        assert_eq!(layout.new_tab.x, 1032);
    }

    #[test]
    fn many_tabs_share_the_bar_equally() {
        // メニューのボタンまでと、右の余白・すき間・新しいタブとプロファイルの一覧のボタンを除いた
        // 1436 ピクセルを 5 等分する。
        let layout = layout(METRICS, WIDTH, 5, 0);
        assert_eq!(layout.tabs().len(), 5);
        assert!(layout.tabs().iter().all(|tab| tab.area.width == 287));
        assert!(layout.profiles.right() <= WIDTH - 20);
    }

    #[test]
    fn tabs_that_do_not_fit_leave_the_active_one_shown() {
        // 1436 ピクセルには下限の 144 のタブが 9 個まで入る。
        let first = layout(METRICS, WIDTH, 40, 0);
        assert_eq!(first.tabs().len(), 9);
        assert_eq!(first.tabs()[0].index, 0);
        assert!(first.tabs().iter().all(|tab| tab.area.width == 159));
        assert!(first.profiles.right() <= WIDTH - 20);
        // 入りきらない位置のタブを表示していれば、それが右端に来るまで並びをずらす。
        let middle = layout(METRICS, WIDTH, 40, 20);
        let indexes: Vec<usize> = middle.tabs().iter().map(|tab| tab.index).collect();
        assert_eq!(indexes, (12..=20).collect::<Vec<_>>());
        let last = layout(METRICS, WIDTH, 40, 39);
        assert_eq!(last.tabs()[8].index, 39);
        // 入りきるうちの位置なら左端から。
        assert_eq!(layout(METRICS, WIDTH, 40, 8).tabs()[0].index, 0);
    }

    #[test]
    fn a_narrow_bar_still_shows_one_tab() {
        let layout = layout(METRICS, 260, 3, 2);
        assert_eq!(xs(&layout), vec![(2, 60, 96)]);
        assert_eq!(layout.tabs()[0].close, square(114, 12, 32));
        // 閉じるボタンはタブからはみ出さない。
        let narrower = super::layout(METRICS, 194, 3, 2);
        assert_eq!(xs(&narrower), vec![(2, 60, 30)]);
        assert_eq!(
            narrower.tabs()[0].close,
            Area {
                x: 60,
                y: 12,
                width: 30,
                height: 32
            }
        );
        assert_eq!(super::layout(METRICS, 164, 3, 0).tabs(), &[]);
        assert_eq!(super::layout(METRICS, WIDTH, 0, 0).tabs(), &[]);
    }

    #[test]
    fn positions_map_to_tabs_and_buttons() {
        let layout = layout(METRICS, WIDTH, 2, 0);
        assert_eq!(layout.hit(60, 30), Some(Hit::Tab(0)));
        // タブの上の空きもそのタブ。
        assert_eq!(layout.hit(539, 0), Some(Hit::Tab(0)));
        assert_eq!(layout.hit(540, 51), Some(Hit::Tab(1)));
        // 閉じるボタンと、その上下はタブ。
        assert_eq!(layout.hit(498, 12), Some(Hit::Close(0)));
        assert_eq!(layout.hit(529, 43), Some(Hit::Close(0)));
        assert_eq!(layout.hit(520, 11), Some(Hit::Tab(0)));
        assert_eq!(layout.hit(520, 44), Some(Hit::Tab(0)));
        assert_eq!(layout.hit(1000, 40), Some(Hit::Close(1)));
        // 新しいタブのボタンは、上から下まで。
        assert_eq!(layout.hit(1032, 0), Some(Hit::NewTab));
        assert_eq!(layout.hit(1067, 51), Some(Hit::NewTab));
        // プロファイルの一覧のボタンは、そのすぐ右から、上から下まで。
        assert_eq!(layout.hit(1068, 30), Some(Hit::Profiles));
        assert_eq!(layout.hit(1103, 0), Some(Hit::Profiles));
        // メニューのボタンは、ウィンドウの左端から、上から下まで。
        assert_eq!(layout.hit(0, 0), Some(Hit::Menu));
        assert_eq!(layout.hit(47, 51), Some(Hit::Menu));
        // メニューのボタンとタブの間、タブとボタンの間、右の空き、その下の端末の画面の余白。
        assert_eq!(layout.hit(54, 30), None);
        assert_eq!(layout.hit(1025, 30), None);
        assert_eq!(layout.hit(1104, 30), None);
        assert_eq!(layout.hit(540, 52), None);
        assert_eq!(layout.hit(0, 59), None);
        assert_eq!(Hit::Close(1).tab(), Some(1));
        assert_eq!(Hit::NewTab.tab(), None);
        assert_eq!(Hit::Profiles.tab(), None);
        assert_eq!(Hit::Menu.tab(), None);
    }

    #[test]
    fn long_titles_end_with_an_ellipsis() {
        assert_eq!(fit("zsh", 100), "zsh");
        assert_eq!(fit("abcdefghij", 100), "abcdefghij");
        assert_eq!(fit("abcdefghijk", 100), "abcdefghi…");
        assert_eq!(fit("abcdefghijk", 109), "abcdefghi…");
        assert_eq!(fit("abc", 10), "…");
        // 省略の印も入らなければ空。
        assert_eq!(fit("abc", 9), "");
        assert_eq!(fit("abc", 0), "");
        assert_eq!(fit("", 0), "");
    }

    #[test]
    fn wide_characters_are_not_split() {
        // 「端末」は 40 ピクセル。30 ピクセルに収めるなら「端」と省略の印。
        assert_eq!(fit("端末", 40), "端末");
        assert_eq!(fit("端末", 30), "端…");
        // 20 ピクセルなら「端」は入らず、省略の印だけ。
        assert_eq!(fit("端末x", 20), "…");
        // 結合文字は前の文字から離さない。
        assert_eq!(fit("e\u{301}e\u{301}e", 40), "e\u{301}…");
    }

    #[test]
    fn control_characters_are_dropped() {
        assert_eq!(fit("a\u{7}b", 100), "ab");
    }

    /// 左端が `x`、幅が `width` のタブ。閉じるボタンは [`layout`] と同じく右端から 42 ピクセル。
    fn tab(x: u32, width: u32) -> TabSlot {
        let close_x = (x + width).saturating_sub(42).max(x);
        TabSlot {
            index: 0,
            area: Area {
                x,
                y: 4,
                width,
                height: 48,
            },
            close: square(close_x, 12, 32),
        }
    }

    #[test]
    fn a_bell_mark_comes_before_the_title() {
        let plain = tab_content(METRICS, &tab(20, 480), false, 32);
        assert_eq!(plain.mark, None);
        // アイコンはタブの左端から 12 空け、タブの高さの真ん中に置く。
        assert_eq!(plain.icon, Some(square(32, 10, 36)));
        // タイトルはアイコンの右から、閉じるボタンの左まで。
        assert_eq!((plain.title_x, plain.title_width), (78, 380));
        // タイトルはタブの高さの真ん中に置く。
        assert_eq!(plain.text_y, 12);
        assert_eq!(tab_content(METRICS, &tab(20, 480), false, 20).text_y, 18);
        let marked = tab_content(METRICS, &tab(20, 480), true, 32);
        assert_eq!(marked.icon, plain.icon);
        // 直径 12 の円をアイコンの次に置き、すき間 12 を空けてタイトル。
        assert_eq!(
            marked.mark,
            Some(Area {
                x: 78,
                y: 22,
                width: 12,
                height: 12
            })
        );
        assert_eq!((marked.title_x, marked.title_width), (102, 356));
    }

    #[test]
    fn the_icon_gives_way_in_narrow_tabs() {
        let has_icon = |width: u32, bell: bool| {
            tab_content(METRICS, &tab(0, width), bell, 32)
                .icon
                .is_some()
        };
        // 閉じるボタンは右端から 42。アイコンの右端 48 まで入れば出す。
        assert!(has_icon(90, false));
        assert!(!has_icon(89, false));
        // 印があれば、印の右端 70 まで入らなければアイコンをやめる。
        assert!(has_icon(112, true));
        assert!(!has_icon(111, true));
        // 入らなければ、タイトルは余白の次から。
        let content = tab_content(METRICS, &tab(0, 89), false, 32);
        assert_eq!((content.title_x, content.title_width), (20, 27));
        // タブの高さより大きければ出さない。
        let short = TabSlot {
            area: Area {
                height: 30,
                ..tab(0, 480).area
            },
            ..tab(0, 480)
        };
        assert_eq!(tab_content(METRICS, &short, false, 32).icon, None);
    }

    #[test]
    fn a_bell_mark_stays_in_narrow_tabs() {
        // タブの幅ごとに、印の左端と直径、タイトルに残る幅（ピクセル）。
        let cases: [(u32, (u32, u32), u32); 7] = [
            // 幅の下限: アイコン、印、すき間、タイトル、閉じるボタン。
            (144, (58, 12), 20),
            // アイコンと閉じるボタンの間に印が収まる。タイトルに残る幅はない。
            (112, (58, 12), 0),
            // 収まらなければアイコンをやめる。
            (111, (20, 12), 25),
            // 余白と閉じるボタンの間に印が収まる。タイトルに残る幅はない。
            (76, (20, 12), 0),
            // 収まらなければ真ん中に置く。
            (60, (24, 12), 0),
            (12, (0, 12), 0),
            // タブより大きければタブの幅に縮める。
            (10, (0, 10), 0),
        ];
        for (width, (x, size), title_width) in cases {
            let content = tab_content(METRICS, &tab(0, width), true, 32);
            let mark = content.mark.expect("a bell mark");
            assert_eq!((mark.x, mark.width), (x, size), "width {width}");
            assert!(mark.right() <= width, "width {width}");
            assert_eq!(content.title_width, title_width, "width {width}");
        }
    }

    fn titles(paints: &[Paint]) -> Vec<(u32, u32, String, Rgb)> {
        paints
            .iter()
            .filter_map(|paint| match paint {
                Paint::Label { x, y, text, color } => Some((*x, *y, text.clone(), *color)),
                _ => None,
            })
            .collect()
    }

    fn dots(paints: &[Paint]) -> Vec<Area> {
        paints
            .iter()
            .filter_map(|paint| match paint {
                Paint::Icon {
                    icon: Icon::Dot,
                    area,
                    color,
                } if *color == BELL_COLOR => Some(*area),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_bell_mark_survives_a_long_title() {
        let powershell = r"C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe";
        let tabs = [
            TabView {
                title: powershell,
                bell: true,
                icon: &ICON,
            },
            TabView {
                title: powershell,
                bell: false,
                icon: &ICON,
            },
            TabView {
                title: "zsh",
                bell: false,
                icon: &ICON,
            },
        ];
        let layout = layout(METRICS, WIDTH, 3, 2);
        let paints = paints(
            &layout,
            &tabs,
            2,
            Highlight::default(),
            &palette(),
            &FixedWidth,
        );
        assert_eq!(dots(&paints).len(), 1);
        assert!(dots(&paints)[0].right() <= 540);
        let titles: Vec<String> = titles(&paints)
            .into_iter()
            .map(|(_, _, text, ..)| text)
            .collect();
        // タブの幅は 478。印のあるタブは 354 ピクセル、ないタブは 378 ピクセルに収める。
        assert_eq!(
            titles,
            vec![
                r"C:\WINDOWS\System32\WindowsPowerSh…",
                r"C:\WINDOWS\System32\WindowsPowerShel…",
                "zsh",
            ]
        );
    }

    #[test]
    fn many_tabs_keep_their_bell_marks() {
        let tabs: Vec<TabView> = (0..40)
            .map(|index| TabView {
                title: "abcdefghijklmnopqrstuvwxyz",
                bell: index % 2 == 0,
                icon: &ICON,
            })
            .collect();
        let layout = layout(METRICS, WIDTH, tabs.len(), 3);
        let paints = paints(
            &layout,
            &tabs,
            3,
            Highlight::default(),
            &palette(),
            &FixedWidth,
        );
        // 描く 10 個のうち、偶数番の 5 個に印。
        assert_eq!(dots(&paints).len(), 5);
        for (slot, dot) in layout.tabs().iter().step_by(2).zip(dots(&paints)) {
            assert!(dot.x >= slot.area.x && dot.right() <= slot.area.right());
        }
    }

    /// 設定のキーと、その色の欄。
    type ColorField = (&'static str, fn(&Palette) -> Rgb);

    const COLOR_FIELDS: [ColorField; 9] = [
        ("bar", |p| p.bar),
        ("active_tab", |p| p.active_tab),
        ("active_border", |p| p.active_border),
        ("active_title", |p| p.active_title),
        ("inactive_title", |p| p.inactive_title),
        ("separator", |p| p.separator),
        ("hovered_tab", |p| p.hovered_tab),
        ("idle_mark", |p| p.idle_mark),
        ("bell", |p| p.bell),
    ];

    #[test]
    fn colors_from_the_settings_replace_only_the_written_ones() {
        let fields = COLOR_FIELDS;
        let written = Rgb { r: 1, g: 2, b: 3 };
        for (index, (name, field)) in fields.iter().enumerate() {
            assert_eq!(TAB_BAR_COLOR_KEYS[index], *name);
            let mut colors = [None; TAB_BAR_COLOR_KEYS.len()];
            colors[index] = Some(written);
            let palette = palette().with_colors(&colors);
            assert_eq!(field(&palette), written, "{name}");
            for (other, (_, other_field)) in fields.iter().enumerate() {
                if other != index {
                    assert_eq!(other_field(&palette), other_field(&super::tests::palette()));
                }
            }
        }
        assert_eq!(palette().with_colors(&[None; 9]), palette());
    }

    #[test]
    fn template_examples_are_the_windows_11_defaults() {
        let windows_11 = Palette::from_system(SystemColors {
            face: gray(0xf0),
            text: gray(0),
            gray_text: gray(0x6d),
            shadow: gray(0xa0),
            window: gray(0xff),
            window_text: gray(0),
        });
        let examples: Vec<String> = COLOR_FIELDS
            .iter()
            .map(|(_, field)| fantermo_core::config::format_color(field(&windows_11)))
            .collect();
        assert_eq!(
            examples,
            [
                "#cccccc", "#ffffff", "#a0a0a0", "#000000", "#000000", "#a0a0a0", "#e6e6e6",
                "#6d6d6d", "#f0b030"
            ]
        );
    }

    #[test]
    fn default_colours_come_from_the_system() {
        let colors = palette();
        // 背景はコントロールの背景の色を、文字の色へ 15% 寄せる。
        assert_eq!(colors.bar, mix(SYSTEM.face, SYSTEM.text, BAR_TINT));
        assert!(colors.bar.r < SYSTEM.face.r && colors.bar.r > SYSTEM.text.r);
        assert_eq!(
            (colors.active_tab, colors.active_title),
            (WINDOW, WINDOW_TEXT)
        );
        assert_eq!(colors.active_border, SYSTEM.shadow);
        assert_eq!(colors.inactive_title, SYSTEM.text);
        assert_eq!(colors.separator, SYSTEM.shadow);
        assert_eq!(colors.idle_mark, SYSTEM.gray_text);
        assert_eq!(colors.bell, BELL_COLOR);
        // マウスを載せているタブは、背景とウィンドウの背景の間。
        assert!(colors.hovered_tab.r > colors.bar.r && colors.hovered_tab.r < WINDOW.r);
    }

    #[test]
    fn windows_11_colours_make_a_darker_bar() {
        let colors = Palette::from_system(SystemColors {
            face: gray(0xf0),
            text: gray(0x00),
            gray_text: gray(0x6d),
            shadow: gray(0xa0),
            window: gray(0xff),
            window_text: gray(0x00),
        });
        assert_eq!(colors.bar, gray(0xcc));
        assert_eq!(colors.hovered_tab, gray(0xe6));
    }

    #[test]
    fn the_active_tab_rises_in_the_window_colour() {
        let tabs = [
            TabView {
                title: "one",
                bell: false,
                icon: &ICON,
            },
            TabView {
                title: "端末",
                bell: false,
                icon: &ICON,
            },
        ];
        let layout = layout(METRICS, WIDTH, 2, 1);
        let colors = palette();
        let paints = paints(
            &layout,
            &tabs,
            1,
            Highlight::default(),
            &colors,
            &FixedWidth,
        );
        // 背景はタブバーの全体で、その下の端末の画面の余白は含まない。
        assert_eq!(
            paints[0],
            Paint::Fill {
                area: Area {
                    x: 0,
                    y: 0,
                    width: WIDTH,
                    height: 52
                },
                color: colors.bar
            }
        );
        let icons = |wanted: Rgb| -> Vec<(Icon, Area)> {
            paints
                .iter()
                .filter_map(|paint| match paint {
                    Paint::Icon { icon, area, color } if *color == wanted => Some((*icon, *area)),
                    _ => None,
                })
                .collect()
        };
        let fills = |wanted: Rgb| -> Vec<Area> {
            paints
                .iter()
                .filter_map(|paint| match paint {
                    Paint::Fill { area, color } if *color == wanted => Some(*area),
                    _ => None,
                })
                .collect()
        };
        // 表示しているタブは、まず縁の線の色でタブの矩形の全体を、上の角を半径 10 で丸めて塗る。
        assert_eq!(
            icons(colors.active_border),
            vec![
                (Icon::Corner(Corner::TopLeft), square(540, 4, 10)),
                (Icon::Corner(Corner::TopRight), square(1010, 4, 10)),
            ]
        );
        assert_eq!(
            fills(colors.active_border),
            vec![
                Area {
                    x: 550,
                    y: 4,
                    width: 460,
                    height: 10
                },
                Area {
                    x: 540,
                    y: 14,
                    width: 480,
                    height: 38
                },
            ]
        );
        // その内側を、左・右・上を線の太さ 2 だけ空けてウィンドウの背景の色で塗る。角は同じ中心
        // (550, 14) と (1010, 14) の半径 8 の円。下の辺はまっすぐで、タブバーの下の辺まで（下には線が
        // ない）。
        assert_eq!(
            icons(WINDOW),
            vec![
                (Icon::Corner(Corner::TopLeft), square(542, 6, 8)),
                (Icon::Corner(Corner::TopRight), square(1010, 6, 8)),
            ]
        );
        assert_eq!(
            fills(WINDOW),
            vec![
                Area {
                    x: 550,
                    y: 6,
                    width: 460,
                    height: 8
                },
                Area {
                    x: 542,
                    y: 14,
                    width: 476,
                    height: 38
                },
            ]
        );
        // 縁の線を先に、ウィンドウの背景の色を後に描く。
        let first_window = paints
            .iter()
            .position(|paint| matches!(paint, Paint::Fill { color, .. } if *color == WINDOW));
        let last_border = paints.iter().rposition(
            |paint| matches!(paint, Paint::Icon { color, .. } if *color == colors.active_border),
        );
        assert!(last_border < first_window);
        // どれもタブバーの中に描く。
        assert!(paints.iter().all(|paint| match paint {
            Paint::Fill { area, .. } | Paint::Icon { area, .. } | Paint::TabIcon { area, .. } => {
                area.bottom() <= 52
            }
            Paint::Label { y, .. } => y + FixedWidth.height() <= 52,
        }));
        // アイコンは、タブごとにタイトルの左に。
        let tab_icons: Vec<Area> = paints
            .iter()
            .filter_map(|paint| match paint {
                Paint::TabIcon { icon, area } if *icon == ICON => Some(*area),
                _ => None,
            })
            .collect();
        assert_eq!(tab_icons, vec![square(72, 10, 36), square(552, 10, 36)]);
        // タイトルは一つの文字列として、アイコンの右、タブの高さの真ん中に置く。表示しているタブの
        // タイトルはウィンドウの文字の色、ほかはコントロールの文字の色。
        assert_eq!(
            titles(&paints),
            vec![
                (118, 12, "one".to_owned(), SYSTEM.text),
                (598, 12, "端末".to_owned(), WINDOW_TEXT),
            ]
        );
        // メニューのボタンの「≡」は、一辺 16 でボタンの真ん中に、薄い文字の色で。
        assert_eq!(
            icons_of(&paints, Icon::Menu),
            vec![(square(22, 20, 16), SYSTEM.gray_text)]
        );
        // プロファイルの一覧のボタンの「▾」も、同じ大きさと色で新しいタブのボタンのすぐ右に。
        let profiles = layout.profiles;
        assert_eq!(profiles.x, layout.new_tab.right());
        assert_eq!(
            icons_of(&paints, Icon::Chevron),
            vec![(
                square(profiles.x + 10, profiles.y + 10, 16),
                SYSTEM.gray_text
            )]
        );
    }

    #[test]
    fn separators_stand_between_tabs_that_are_not_shown() {
        let tabs = [TabView {
            title: "a",
            bell: false,
            icon: &ICON,
        }; 4];
        let layout = layout(METRICS, WIDTH, 4, 1);
        let colors = palette();
        let separators = |highlight: Highlight| -> Vec<u32> {
            paints(&layout, &tabs, 1, highlight, &colors, &FixedWidth)
                .iter()
                .filter_map(|paint| match paint {
                    // 表示しているタブの縁の線も同じ色なので、太さ 2 の線だけを数える。
                    Paint::Fill { area, color }
                        if *color == colors.separator && area.width == 2 =>
                    {
                        Some(area.x)
                    }
                    _ => None,
                })
                .collect()
        };
        // 表示している 1 番の左右には引かない。2 番と 3 番の間だけ。太さ 2 の線を境目の真ん中に。
        assert_eq!(
            separators(Highlight::default()),
            vec![layout.tabs()[3].area.x - 1]
        );
        // マウスを載せているタブの左右にも引かない。
        let hovered = Highlight {
            hovered: Some(Hit::Close(3)),
            pressed: None,
        };
        assert!(separators(hovered).is_empty());
    }

    /// `paints` のうち、`icon` を描くものの矩形と色。
    fn icons_of(paints: &[Paint], wanted: Icon) -> Vec<(Area, Rgb)> {
        paints
            .iter()
            .filter_map(|paint| match paint {
                Paint::Icon { icon, area, color } if *icon == wanted => Some((*area, *color)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn close_buttons_show_on_the_active_and_the_hovered_tab() {
        let tabs = [TabView {
            title: "a",
            bell: false,
            icon: &ICON,
        }; 3];
        let layout = layout(METRICS, WIDTH, 3, 0);
        let colors = palette();
        let close_marks = |highlight: Highlight| -> Vec<u32> {
            icons_of(
                &paints(&layout, &tabs, 0, highlight, &colors, &FixedWidth),
                Icon::Close,
            )
            .iter()
            .map(|(area, _)| area.x)
            .collect()
        };
        // 「×」は一辺 16 で、ボタンの真ん中に。
        let close = layout.tabs()[0].close;
        assert_eq!(close_marks(Highlight::default()), vec![close.x + 8]);
        let hovered = Highlight {
            hovered: Some(Hit::Tab(2)),
            pressed: None,
        };
        assert_eq!(
            close_marks(hovered),
            vec![close.x + 8, layout.tabs()[2].close.x + 8]
        );
        // マウスを載せているタブも、表示しているタブと同じく上の角を丸めて描く。
        let paints = paints(&layout, &tabs, 0, hovered, &colors, &FixedWidth);
        let corners = icons_of(&paints, Icon::Corner(Corner::TopLeft));
        assert_eq!(
            corners,
            vec![
                (square(layout.tabs()[2].area.x, 4, 10), colors.hovered_tab),
                (square(60, 4, 10), colors.active_border),
                (square(62, 6, 8), WINDOW),
            ]
        );
    }

    #[test]
    fn buttons_under_the_pointer_get_a_round_background() {
        let tabs = [TabView {
            title: "a",
            bell: false,
            icon: &ICON,
        }; 2];
        let layout = layout(METRICS, WIDTH, 2, 0);
        let colors = palette();
        let backgrounds = |highlight: Highlight| -> Vec<(Area, Rgb)> {
            icons_of(
                &paints(&layout, &tabs, 0, highlight, &colors, &FixedWidth),
                Icon::Dot,
            )
        };
        assert_eq!(backgrounds(Highlight::default()), vec![]);
        let close = layout.tabs()[0].close;
        let over_close = Highlight {
            hovered: Some(Hit::Close(0)),
            pressed: None,
        };
        // 表示しているタブの上では、ウィンドウの背景の色をウィンドウの文字の色へ寄せる。
        assert_eq!(
            backgrounds(over_close),
            vec![(close, colors.button(WINDOW, WINDOW_TEXT, false))]
        );
        // 押したままボタンの上にあれば押している色、外へ出れば背景を描かない。
        let pressed = Highlight {
            pressed: Some(Hit::Close(0)),
            ..over_close
        };
        assert_eq!(
            backgrounds(pressed),
            vec![(close, colors.button(WINDOW, WINDOW_TEXT, true))]
        );
        let pressed_elsewhere = Highlight {
            hovered: Some(Hit::Tab(0)),
            pressed: Some(Hit::Close(0)),
        };
        assert_eq!(backgrounds(pressed_elsewhere), vec![]);
        // 新しいタブのボタン。背景の上では、背景の色をコントロールの文字の色へ寄せる。「+」は一辺 16 で
        // 真ん中に、マウスを載せていればコントロールの文字の色。
        let over_new_tab = Highlight {
            hovered: Some(Hit::NewTab),
            pressed: None,
        };
        let text = SYSTEM.text;
        assert_eq!(
            backgrounds(over_new_tab),
            vec![(layout.new_tab, colors.button(colors.bar, text, false))]
        );
        let plus = icons_of(
            &paints(&layout, &tabs, 0, over_new_tab, &colors, &FixedWidth),
            Icon::Plus,
        );
        assert_eq!(
            plus,
            vec![(
                square(layout.new_tab.x + 10, layout.new_tab.y + 10, 16),
                text
            )]
        );
        assert_ne!(
            colors.button(WINDOW, WINDOW_TEXT, false),
            colors.button(WINDOW, WINDOW_TEXT, true)
        );
    }
}
