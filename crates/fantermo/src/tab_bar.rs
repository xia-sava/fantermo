//! タブバー。窓の上端に、端末ごとのタブを並べる。端末が一つのときも出す。
//!
//! 寸法は端末のフォントによらず DPI から決め、セルの格子によらずピクセルの位置で並べる。地は Windows の
//! コントロールの色で塗り、表示しているタブは端末と同じ色で上の角を丸め、下の辺を端末へつなげて描く。並べ方（どこがどのタブか、タイトルの
//! 省略、ベルの印の位置）と、描くもの（[`Paint`]）の組み立ては、窓や描画に依存しない関数にしてある。

use fantermo_core::{frame::Rgb, vt};

use crate::paint::{Area, Corner, Icon, Paint, Side};

/// 寸法の定数を決めた DPI。定数はこの DPI でのピクセルで、窓の DPI に比例させて使う。
const BASE_DPI: u32 = 96;

/// タブバーの高さ。
const BAR_HEIGHT: u32 = 32;

/// タブの上の空き。表示しているタブの角の半径と、ボタンとタブの間のすき間にも使う。
const MARGIN: u32 = 6;

/// タブの中の左右の余白。タブの並びの右の余白にも使う。
const PADDING: u32 = 10;

/// 新しいタブのボタンとメニューのボタン（丸い地）の直径。
const BUTTON: u32 = 22;

/// 新しいタブのボタンの「+」と、メニューのボタンの「≡」の一辺。
const BUTTON_MARK: u32 = 10;

/// タブを閉じるボタン（丸い地）の直径。
const CLOSE_BUTTON: u32 = 18;

/// 閉じるボタンの「×」の一辺。
const CLOSE_MARK: u32 = 8;

/// ベルの印の円の直径。
const BELL_MARK: u32 = 8;

/// ベルの印とタイトルの間のすき間。
const BELL_GAP: u32 = 6;

/// タブの間の区切りの線の太さ。
const SEPARATOR_WIDTH: u32 = 1;

/// タブの間の区切りの線の高さ。
const SEPARATOR_HEIGHT: u32 = 14;

/// タブの幅の上限。
const MAX_TAB_WIDTH: u32 = 240;

/// タブが多いときに縮める、タブの幅の下限。これより狭くなるほど多ければ、入りきらないタブを描かない。
const MIN_TAB_WIDTH: u32 = 72;

/// 収まらないタイトルの末尾に付ける印。
const ELLIPSIS: &str = "…";

/// 表示していない間にベルが鳴ったタブの、タイトルの前の印（円）の色。
const BELL_COLOR: Rgb = Rgb {
    r: 0xf0,
    g: 0xb0,
    b: 0x30,
};

/// マウスの下の、表示していないタブの色を、コントロールの地の色から明るい縁の色へ寄せる割合。
const HOVERED_TAB_LIGHTEN: f32 = 0.6;

/// マウスの下のボタン（×、+、≡）の丸い地の色を、ボタンの下の色から文字の色へ寄せる割合。
const HOVERED_BUTTON_TINT: f32 = 0.15;

/// 押しているボタンの丸い地の色を、ボタンの下の色から文字の色へ寄せる割合。
const PRESSED_BUTTON_TINT: f32 = 0.28;

/// DPI が `dpi` の窓での、[`BASE_DPI`] のときに `px` ピクセルの長さ。少なくとも 1 ピクセル。
fn scale(px: u32, dpi: u32) -> u32 {
    ((px * dpi + BASE_DPI / 2) / BASE_DPI).max(1)
}

/// DPI が `dpi` の窓での、タブバーの高さ（ピクセル）。
pub fn height(dpi: u32) -> u32 {
    scale(BAR_HEIGHT, dpi)
}

/// タブバーの寸法を決めるもの。寸法は窓の DPI に比例させる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub dpi: u32,
}

/// タイトルの文字列の大きさを測るもの。窓では Windows の UI のフォントで測る。
pub trait Measure {
    /// `text` を 1 行に組んだ幅（ピクセル）。`bold` なら太字で組む。
    fn width(&self, text: &str, bold: bool) -> u32;
    /// 1 行の高さ（ピクセル）。
    fn height(&self) -> u32;
}

impl Metrics {
    fn scale(self, px: u32) -> u32 {
        scale(px, self.dpi)
    }

    fn bar_height(self) -> u32 {
        height(self.dpi)
    }

    /// タブの上の空き。表示しているタブの角の半径にも使う。
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

    /// タブの間の区切りの線の太さと高さ。
    fn separator(self) -> (u32, u32) {
        (self.scale(SEPARATOR_WIDTH), self.scale(SEPARATOR_HEIGHT))
    }

    /// タブを閉じるボタン（丸い地）の直径。
    fn close_button(self) -> u32 {
        self.scale(CLOSE_BUTTON)
    }

    /// 閉じるボタンの「×」の一辺。
    fn close_mark(self) -> u32 {
        self.scale(CLOSE_MARK)
    }

    /// 新しいタブのボタンとメニューのボタン（丸い地）の直径。
    fn button(self) -> u32 {
        self.scale(BUTTON)
    }

    /// 新しいタブのボタンの「+」と、メニューのボタンの「≡」の一辺。
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
    /// タブを描く矩形。上の余白の下から、タブバーの下の辺まで。
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
    /// メニューのボタン。
    Menu,
}

impl Hit {
    /// タブかその閉じるボタンなら、そのタブの位置。
    pub fn tab(self) -> Option<usize> {
        match self {
            Hit::Tab(index) | Hit::Close(index) => Some(index),
            Hit::NewTab | Hit::Menu => None,
        }
    }
}

/// 幅 `width` のタブバーの左端にメニューのボタンを置き、その右に `tabs` 個のタブを並べ、さらに右に
/// 新しいタブのボタンを置く。タブは同じ幅で、上限は [`MAX_TAB_WIDTH`]。多ければ [`MIN_TAB_WIDTH`]
/// まで縮め、それでも入りきらなければ、表示しているタブ `active` を含む続きのタブだけを並べる。
pub fn layout(metrics: Metrics, width: u32, tabs: usize, active: usize) -> Layout {
    let top = metrics.margin();
    let tab_height = metrics.bar_height() - top;
    // ボタンは、タブの高さの真ん中に置く。
    let button_size = metrics.button();
    let button_at = |x: u32| Area {
        x,
        y: top + tab_height.saturating_sub(button_size) / 2,
        width: button_size,
        height: button_size,
    };
    // ボタンの左右とタブとの間には、上の余白と同じすき間を空ける。
    let menu = button_at(top);
    let start = menu.right() + top;
    let mut layout = Layout {
        metrics,
        width,
        tabs: Vec::new(),
        new_tab: button_at(start),
        menu,
    };
    if tabs == 0 {
        return layout;
    }
    // 新しいタブのボタンと、タブとの間のすき間の分を右に残す。
    let end = width.saturating_sub(metrics.padding() + top + button_size);
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
        layout.new_tab = button_at(last.area.right() + top);
    }
    layout
}

impl Layout {
    /// メニューのボタン。
    pub fn menu(&self) -> Area {
        self.menu
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
        // メニューのボタンは、窓の左端からとみなす。
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

/// `title` を幅 `width`（ピクセル）に収めた文字列。幅は `measure` で測り、`bold` なら太字で測る。
/// 収まらなければ、書記素クラスタの切れ目で切って末尾を [`ELLIPSIS`] にする（[`ELLIPSIS`] も入らなければ
/// 空）。幅のない書記素クラスタ（制御文字など）は除く。
pub fn fit_title(title: &str, width: u32, bold: bool, measure: &impl Measure) -> String {
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
    if measure.width(&whole, bold) <= width {
        return whole;
    }
    let shortened = |count: usize| prefix(count) + ELLIPSIS;
    if measure.width(ELLIPSIS, bold) > width {
        return String::new();
    }
    // 先頭の `fitting` 個に印を付けたものは収まり、`too_many` 個では収まらない。
    let (mut fitting, mut too_many) = (0, graphemes.len());
    while too_many - fitting > 1 {
        let middle = (fitting + too_many) / 2;
        if measure.width(&shortened(middle), bold) <= width {
            fitting = middle;
        } else {
            too_many = middle;
        }
    }
    shortened(fitting)
}

/// タブ一つの中の、ベルの印とタイトルの位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TabContent {
    /// ベルの印の円を描く矩形。印を付けないときは `None`。
    mark: Option<Area>,
    /// タイトルの左端。
    title_x: u32,
    /// タイトルに充てる幅（ピクセル）。
    title_width: u32,
    /// タイトルの上端。
    text_y: u32,
}

/// タブ `slot` の中の、閉じるボタンより左に、ベルの印（`bell` のとき）と、1 行の高さが `text_height`
/// のタイトルを置く。閉じるボタンを出していないときも、その幅は空けておく（マウスを載せてもタイトルの
/// 省略が変わらないように）。印はタイトルの左に置き、タイトルを省略する前にその幅を取る。タイトルに
/// 残る幅がなくても印は消さず、余白に収まらなければタブの真ん中に置き、タブより大きければタブの幅に
/// 縮める。
fn tab_content(metrics: Metrics, slot: &TabSlot, bell: bool, text_height: u32) -> TabContent {
    let tab = slot.area;
    let left = tab.x + metrics.padding();
    let right = slot.close.x;
    let text_y = tab.y + tab.height.saturating_sub(text_height) / 2;
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
}

/// Windows のコントロールの色（`GetSysColor`）。タブバーの地と、表示していないタブ・ボタンに使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemColors {
    /// 地（`COLOR_BTNFACE`）。
    pub face: Rgb,
    /// 文字（`COLOR_BTNTEXT`）。
    pub text: Rgb,
    /// 薄い文字（`COLOR_GRAYTEXT`）。
    pub gray_text: Rgb,
    /// 明るい縁（`COLOR_BTNHIGHLIGHT`）。
    pub highlight: Rgb,
    /// 暗い縁（`COLOR_BTNSHADOW`）。
    pub shadow: Rgb,
}

/// タブバーの色を決めるもの。Windows のコントロールの色と、端末の既定の背景色・文字の色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub system: SystemColors,
    pub background: Rgb,
    pub foreground: Rgb,
}

/// タブバーの色。地と表示していないタブはコントロールの色、表示しているタブは端末の色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Colors {
    bar: Rgb,
    active_tab: Rgb,
    active_title: Rgb,
    inactive_title: Rgb,
    separator: Rgb,
    hovered_tab: Rgb,
    /// マウスの下にない、新しいタブのボタンとメニューのボタンの印。
    idle_mark: Rgb,
}

impl Colors {
    fn new(palette: Palette) -> Self {
        let system = palette.system;
        Self {
            bar: system.face,
            // 表示しているタブは端末と同じ色にして、端末とつながって見えるようにする。
            active_tab: palette.background,
            active_title: palette.foreground,
            inactive_title: system.text,
            separator: system.shadow,
            hovered_tab: mix(system.face, system.highlight, HOVERED_TAB_LIGHTEN),
            idle_mark: system.gray_text,
        }
    }

    /// 色 `base` の上に、色 `ink` の印で描くボタンの丸い地の色。`pressed` なら押しているとき、
    /// そうでなければマウスの下にあるとき。
    fn button(&self, base: Rgb, ink: Rgb, pressed: bool) -> Rgb {
        let tint = if pressed {
            PRESSED_BUTTON_TINT
        } else {
            HOVERED_BUTTON_TINT
        };
        mix(base, ink, tint)
    }
}

/// マウスの下にあるものと、押しているもの。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Highlight {
    pub hovered: Option<Hit>,
    /// 左ボタンを押したときにマウスの下にあったもの。マウスの下にあるときだけ、押している色で描く。
    pub pressed: Option<Hit>,
}

impl Highlight {
    /// `hit` の丸い地の色を決めるとき、押しているか。マウスの下になければ `None`。
    fn state(&self, hit: Hit) -> Option<bool> {
        (self.hovered == Some(hit)).then_some(self.pressed == Some(hit))
    }

    /// マウスの下にあるタブの位置。
    fn hovered_tab(&self) -> Option<usize> {
        self.hovered.and_then(Hit::tab)
    }
}

/// `from` を `to` へ `ratio`（0.0〜1.0）だけ寄せた色。
fn mix(from: Rgb, to: Rgb, ratio: f32) -> Rgb {
    let channel = |a: u8, b: u8| -> u8 {
        (f32::from(a) + (f32::from(b) - f32::from(a)) * ratio).round() as u8
    };
    Rgb {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
    }
}

/// `layout` のとおりに並べたタブバーに描くもの。`tabs` は端末の並びの順のタブ、`active` は表示して
/// いるタブの位置、`highlight` はマウスの下にあるものと押しているもの。色は `palette` から決め、
/// タイトルの幅は `measure` で測る。表示しているタブのタイトルは太字にする。閉じるボタンは、表示して
/// いるタブとマウスの下のタブにだけ出す。
pub fn paints(
    layout: &Layout,
    tabs: &[TabView],
    active: usize,
    highlight: Highlight,
    palette: Palette,
    measure: &impl Measure,
) -> Vec<Paint> {
    let metrics = layout.metrics;
    let colors = Colors::new(palette);
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
        push_raised_tab(&mut paints, layout, tab.area, colors.hovered_tab, false);
    }
    if let Some(tab) = layout.tabs.iter().find(|tab| tab.index == active) {
        push_raised_tab(&mut paints, layout, tab.area, colors.active_tab, true);
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
        if let Some(mark) = content.mark {
            paints.push(Paint::Icon {
                icon: Icon::Dot,
                area: mark,
                color: BELL_COLOR,
            });
        }
        let title = fit_title(tab.title, content.title_width, is_active, measure);
        if !title.is_empty() {
            paints.push(Paint::Label {
                x: content.title_x,
                y: content.text_y,
                text: title,
                bold: is_active,
                color: title_color,
            });
        }
        if raised(slot.index) {
            push_button(
                &mut paints,
                &colors,
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
        (Hit::Menu, layout.menu, Icon::Menu),
    ] {
        let mark_color = if highlight.hovered == Some(hit) {
            colors.inactive_title
        } else {
            colors.idle_mark
        };
        push_button(
            &mut paints,
            &colors,
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

/// タブバーのボタン（×、+）一つ。
struct Button {
    /// ボタンの矩形。マウスの下にあれば、ここに丸い地を描く。
    area: Area,
    /// 真ん中に描く印と、その一辺と色。
    mark: (Icon, u32, Rgb),
    /// マウスの下にあれば、押しているか。
    state: Option<bool>,
    /// ボタンの下の色。丸い地の色は、ここから `ink` へ寄せて決める。
    base: Rgb,
    /// ボタンの下の地に置く文字の色。
    ink: Rgb,
}

fn push_button(paints: &mut Vec<Paint>, colors: &Colors, button: Button) {
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

/// 矩形 `area` に、上の角を丸めたタブを `color` で描く。`flared` なら、下の辺の左右を外へ反らせる。
fn push_raised_tab(paints: &mut Vec<Paint>, layout: &Layout, area: Area, color: Rgb, flared: bool) {
    let radius = layout.metrics.margin();
    if area.width < radius * 2 || area.height < radius {
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
    if !flared {
        return;
    }
    let flare_y = area.bottom() - radius;
    if area.x >= radius {
        paints.push(Paint::Icon {
            icon: Icon::Flare(Side::Left),
            area: square(area.x - radius, flare_y),
            color,
        });
    }
    if area.right() + radius <= layout.width {
        paints.push(Paint::Icon {
            icon: Icon::Flare(Side::Right),
            area: square(area.right(), flare_y),
            color,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };
    const WHITE: Rgb = Rgb {
        r: 200,
        g: 200,
        b: 200,
    };

    const fn gray(level: u8) -> Rgb {
        Rgb {
            r: level,
            g: level,
            b: level,
        }
    }

    /// Windows 11 の既定に近いコントロールの色と、黒地に白の端末。コントロールの文字の色は、端末の
    /// 背景色と見分けられるよう黒にしない。
    const PALETTE: Palette = Palette {
        system: SystemColors {
            face: gray(0xf0),
            text: gray(0x20),
            gray_text: gray(0x6d),
            highlight: gray(0xff),
            shadow: gray(0xa0),
        },
        background: BLACK,
        foreground: WHITE,
    };

    /// 192 DPI の寸法。タブバーの高さ 64、上の空き 12、余白 20、ボタン 44、閉じるボタン 36、
    /// ベルの印 16、すき間 12、タブの幅 144〜480。
    const METRICS: Metrics = Metrics { dpi: 192 };

    /// セルの幅が 16 ピクセルの 100 桁の窓のタブバーの幅。
    const WIDTH: u32 = 100 * 16;

    /// 幅が ASCII の文字と [`ELLIPSIS`] は 10、ほかの文字は 20 ピクセル（太字なら 1 ずつ広い）、
    /// 1 行の高さが 32 ピクセルの書体。
    struct FixedWidth;

    impl Measure for FixedWidth {
        fn width(&self, text: &str, bold: bool) -> u32 {
            text.chars()
                .map(|c| {
                    let width = if c.is_ascii() || ELLIPSIS.contains(c) {
                        10
                    } else {
                        20
                    };
                    width + u32::from(bold)
                })
                .sum()
        }

        fn height(&self) -> u32 {
            32
        }
    }

    fn fit(title: &str, width: u32) -> String {
        fit_title(title, width, false, &FixedWidth)
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
        assert_eq!(height(96), 32);
        assert_eq!(height(144), 48);
        assert_eq!(height(192), 64);
        assert_eq!(height(120), 40);
        assert_eq!(height(1), 1);
        let base = Metrics { dpi: 96 };
        assert_eq!((base.button(), METRICS.button()), (22, 44));
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
        // メニューのボタンは、左端に上の空きと同じすき間を空けて、タブの高さの真ん中に置く。
        assert_eq!(layout.menu(), square(12, 16, 44));
        // タブは、メニューのボタンの右に同じすき間を空けて置く。
        assert_eq!(xs(&layout), vec![(0, 68, 480)]);
        let tab = layout.tabs()[0];
        // 上の空きの下から、タブバーの下の辺まで。
        assert_eq!((tab.area.y, tab.area.bottom()), (12, 64));
        // 閉じるボタンは右の余白の半分を残してタブの右端に、高さの真ん中に置く。
        assert_eq!(tab.close, square(502, 20, 36));
        // 新しいタブのボタンは、タブの右に同じすき間を空けて置く。
        assert_eq!(layout.new_tab, square(560, 16, 44));
    }

    #[test]
    fn few_tabs_have_the_widest_width() {
        let layout = layout(METRICS, WIDTH, 2, 1);
        assert_eq!(xs(&layout), vec![(0, 68, 480), (1, 548, 480)]);
        assert_eq!(layout.new_tab.x, 1040);
    }

    #[test]
    fn many_tabs_share_the_bar_equally() {
        // メニューのボタンまでと、右の余白・すき間・新しいタブのボタンを除いた 1456 ピクセルを
        // 5 等分する。
        let layout = layout(METRICS, WIDTH, 5, 0);
        assert_eq!(layout.tabs().len(), 5);
        assert!(layout.tabs().iter().all(|tab| tab.area.width == 291));
        assert!(layout.new_tab.right() <= WIDTH - 20);
    }

    #[test]
    fn tabs_that_do_not_fit_leave_the_active_one_shown() {
        // 1456 ピクセルには下限の 144 のタブが 10 個まで入る。
        let first = layout(METRICS, WIDTH, 40, 0);
        assert_eq!(first.tabs().len(), 10);
        assert_eq!(first.tabs()[0].index, 0);
        assert!(first.tabs().iter().all(|tab| tab.area.width == 145));
        assert!(first.new_tab.right() <= WIDTH - 20);
        // 入りきらない位置のタブを表示していれば、それが右端に来るまで並びをずらす。
        let middle = layout(METRICS, WIDTH, 40, 20);
        let indexes: Vec<usize> = middle.tabs().iter().map(|tab| tab.index).collect();
        assert_eq!(indexes, (11..=20).collect::<Vec<_>>());
        let last = layout(METRICS, WIDTH, 40, 39);
        assert_eq!(last.tabs()[9].index, 39);
        // 入りきるうちの位置なら左端から。
        assert_eq!(layout(METRICS, WIDTH, 40, 9).tabs()[0].index, 0);
    }

    #[test]
    fn a_narrow_bar_still_shows_one_tab() {
        let layout = layout(METRICS, 224, 3, 2);
        assert_eq!(xs(&layout), vec![(2, 68, 80)]);
        assert_eq!(layout.tabs()[0].close, square(102, 20, 36));
        // 閉じるボタンはタブからはみ出さない。
        let narrower = super::layout(METRICS, 174, 3, 2);
        assert_eq!(xs(&narrower), vec![(2, 68, 30)]);
        assert_eq!(
            narrower.tabs()[0].close,
            Area {
                x: 68,
                y: 20,
                width: 30,
                height: 36
            }
        );
        assert_eq!(super::layout(METRICS, 144, 3, 0).tabs(), &[]);
        assert_eq!(super::layout(METRICS, WIDTH, 0, 0).tabs(), &[]);
    }

    #[test]
    fn positions_map_to_tabs_and_buttons() {
        let layout = layout(METRICS, WIDTH, 2, 0);
        assert_eq!(layout.hit(68, 30), Some(Hit::Tab(0)));
        // タブの上の空きもそのタブ。
        assert_eq!(layout.hit(547, 0), Some(Hit::Tab(0)));
        assert_eq!(layout.hit(548, 63), Some(Hit::Tab(1)));
        // 閉じるボタンと、その上下はタブ。
        assert_eq!(layout.hit(502, 20), Some(Hit::Close(0)));
        assert_eq!(layout.hit(537, 55), Some(Hit::Close(0)));
        assert_eq!(layout.hit(520, 19), Some(Hit::Tab(0)));
        assert_eq!(layout.hit(1000, 40), Some(Hit::Close(1)));
        // 新しいタブのボタンは、上から下まで。
        assert_eq!(layout.hit(1040, 0), Some(Hit::NewTab));
        assert_eq!(layout.hit(1083, 63), Some(Hit::NewTab));
        // メニューのボタンは、窓の左端から、上から下まで。
        assert_eq!(layout.hit(0, 0), Some(Hit::Menu));
        assert_eq!(layout.hit(55, 63), Some(Hit::Menu));
        // メニューのボタンとタブの間、タブとボタンの間、右の空き、タブバーの下。
        assert_eq!(layout.hit(60, 30), None);
        assert_eq!(layout.hit(1030, 30), None);
        assert_eq!(layout.hit(1084, 30), None);
        assert_eq!(layout.hit(548, 64), None);
        assert_eq!(Hit::Close(1).tab(), Some(1));
        assert_eq!(Hit::NewTab.tab(), None);
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
    fn bold_titles_are_measured_in_bold() {
        // 太字は 1 文字 11 ピクセル。100 ピクセルには 8 文字と省略の印。
        assert_eq!(fit_title("abcdefghij", 100, true, &FixedWidth), "abcdefgh…");
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

    /// 左端が `x`、幅が `width` のタブ。閉じるボタンは [`layout`] と同じく右端から 46 ピクセル。
    fn tab(x: u32, width: u32) -> TabSlot {
        let close_x = (x + width).saturating_sub(46).max(x);
        TabSlot {
            index: 0,
            area: Area {
                x,
                y: 12,
                width,
                height: 52,
            },
            close: square(close_x, 20, 36),
        }
    }

    #[test]
    fn a_bell_mark_comes_before_the_title() {
        let plain = tab_content(METRICS, &tab(20, 480), false, 32);
        assert_eq!(plain.mark, None);
        // タイトルは閉じるボタンの左まで。
        assert_eq!((plain.title_x, plain.title_width), (40, 414));
        // タイトルはタブの高さの真ん中に置く。
        assert_eq!(plain.text_y, 22);
        assert_eq!(tab_content(METRICS, &tab(20, 480), false, 20).text_y, 28);
        let marked = tab_content(METRICS, &tab(20, 480), true, 32);
        // 直径 16 の円を左の余白の次に置き、すき間 12 を空けてタイトル。
        assert_eq!(
            marked.mark,
            Some(Area {
                x: 40,
                y: 30,
                width: 16,
                height: 16
            })
        );
        assert_eq!((marked.title_x, marked.title_width), (68, 386));
    }

    #[test]
    fn a_bell_mark_stays_in_narrow_tabs() {
        // タブの幅ごとに、印の左端と直径、タイトルに残る幅（ピクセル）。
        let cases: [(u32, (u32, u32), u32); 5] = [
            // 幅の下限: 余白、印、すき間、タイトル、閉じるボタン。
            (144, (20, 16), 50),
            // 余白と閉じるボタンの間に印が収まる。タイトルに残る幅はない。
            (82, (20, 16), 0),
            // 収まらなければ真ん中に置く。
            (60, (22, 16), 0),
            (16, (0, 16), 0),
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

    fn titles(paints: &[Paint]) -> Vec<(u32, u32, String, bool, Rgb)> {
        paints
            .iter()
            .filter_map(|paint| match paint {
                Paint::Label {
                    x,
                    y,
                    text,
                    bold,
                    color,
                } => Some((*x, *y, text.clone(), *bold, *color)),
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
            },
            TabView {
                title: powershell,
                bell: false,
            },
            TabView {
                title: "zsh",
                bell: false,
            },
        ];
        let layout = layout(METRICS, WIDTH, 3, 2);
        let paints = paints(
            &layout,
            &tabs,
            2,
            Highlight::default(),
            PALETTE,
            &FixedWidth,
        );
        assert_eq!(dots(&paints).len(), 1);
        assert!(dots(&paints)[0].right() <= 548);
        let titles: Vec<String> = titles(&paints)
            .into_iter()
            .map(|(_, _, text, ..)| text)
            .collect();
        // 印のあるタブは 386 ピクセル、ないタブは 414 ピクセルに収める。
        assert_eq!(
            titles,
            vec![
                r"C:\WINDOWS\System32\WindowsPowerShell…",
                r"C:\WINDOWS\System32\WindowsPowerShell\v1…",
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
            })
            .collect();
        let layout = layout(METRICS, WIDTH, tabs.len(), 3);
        let paints = paints(
            &layout,
            &tabs,
            3,
            Highlight::default(),
            PALETTE,
            &FixedWidth,
        );
        // 描く 10 個のうち、偶数番の 5 個に印。
        assert_eq!(dots(&paints).len(), 5);
        for (slot, dot) in layout.tabs().iter().step_by(2).zip(dots(&paints)) {
            assert!(dot.x >= slot.area.x && dot.right() <= slot.area.right());
        }
    }

    #[test]
    fn the_active_tab_rises_in_the_terminal_colour() {
        let tabs = [
            TabView {
                title: "one",
                bell: false,
            },
            TabView {
                title: "端末",
                bell: false,
            },
        ];
        let layout = layout(METRICS, WIDTH, 2, 1);
        let paints = paints(
            &layout,
            &tabs,
            1,
            Highlight::default(),
            PALETTE,
            &FixedWidth,
        );
        let colors = Colors::new(PALETTE);
        // 地はタブバーの全体で、コントロールの地の色。
        assert_eq!(colors.bar, PALETTE.system.face);
        assert_eq!(
            paints[0],
            Paint::Fill {
                area: Area {
                    x: 0,
                    y: 0,
                    width: WIDTH,
                    height: 64
                },
                color: colors.bar
            }
        );
        // 表示しているタブは端末の背景色で、上の角を丸め、下の左右を外へ反らせる。
        let icons: Vec<(Icon, Area)> = paints
            .iter()
            .filter_map(|paint| match paint {
                Paint::Icon { icon, area, color } if *color == BLACK => Some((*icon, *area)),
                _ => None,
            })
            .collect();
        assert_eq!(
            icons,
            vec![
                (Icon::Corner(Corner::TopLeft), square(548, 12, 12)),
                (Icon::Corner(Corner::TopRight), square(1016, 12, 12)),
                (Icon::Flare(Side::Left), square(536, 52, 12)),
                (Icon::Flare(Side::Right), square(1028, 52, 12)),
            ]
        );
        // タイトルは一つの文字列として、余白の右、タブの高さの真ん中に置く。表示しているタブの
        // タイトルは太字で端末の文字の色、ほかはコントロールの文字の色。
        let text = PALETTE.system.text;
        assert_eq!(
            titles(&paints),
            vec![
                (88, 22, "one".to_owned(), false, text),
                (568, 22, "端末".to_owned(), true, WHITE),
            ]
        );
        // メニューのボタンの「≡」は、一辺 20 でボタンの真ん中に、薄い文字の色で。
        assert_eq!(
            icons_of(&paints, Icon::Menu),
            vec![(square(24, 28, 20), PALETTE.system.gray_text)]
        );
    }

    #[test]
    fn separators_stand_between_tabs_that_are_not_shown() {
        let tabs = [TabView {
            title: "a",
            bell: false,
        }; 4];
        let layout = layout(METRICS, WIDTH, 4, 1);
        let colors = Colors::new(PALETTE);
        let separators = |highlight: Highlight| -> Vec<u32> {
            paints(&layout, &tabs, 1, highlight, PALETTE, &FixedWidth)
                .iter()
                .filter_map(|paint| match paint {
                    Paint::Fill { area, color } if *color == colors.separator => Some(area.x),
                    _ => None,
                })
                .collect()
        };
        // 表示している 1 番の左右には引かない。2 番と 3 番の間だけ。太さ 2 の線を境目の真ん中に。
        assert_eq!(
            separators(Highlight::default()),
            vec![layout.tabs()[3].area.x - 1]
        );
        // マウスの下のタブの左右にも引かない。
        let hovered = Highlight {
            hovered: Some(Hit::Close(3)),
            pressed: None,
        };
        assert_eq!(separators(hovered), vec![]);
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
        }; 3];
        let layout = layout(METRICS, WIDTH, 3, 0);
        let colors = Colors::new(PALETTE);
        let close_marks = |highlight: Highlight| -> Vec<u32> {
            icons_of(
                &paints(&layout, &tabs, 0, highlight, PALETTE, &FixedWidth),
                Icon::Close,
            )
            .iter()
            .map(|(area, _)| area.x)
            .collect()
        };
        // 「×」は一辺 16 で、ボタンの真ん中に。
        let close = layout.tabs()[0].close;
        assert_eq!(close_marks(Highlight::default()), vec![close.x + 10]);
        let hovered = Highlight {
            hovered: Some(Hit::Tab(2)),
            pressed: None,
        };
        assert_eq!(
            close_marks(hovered),
            vec![close.x + 10, layout.tabs()[2].close.x + 10]
        );
        // マウスの下のタブは、上の角を丸めて地より明るい色で描く。下の左右は反らせない。
        let paints = paints(&layout, &tabs, 0, hovered, PALETTE, &FixedWidth);
        let corners = icons_of(&paints, Icon::Corner(Corner::TopLeft));
        assert!(colors.hovered_tab.r > colors.bar.r && colors.hovered_tab.r < 0xff);
        assert_eq!(
            corners,
            vec![
                (square(layout.tabs()[2].area.x, 12, 12), colors.hovered_tab),
                (square(68, 12, 12), BLACK),
            ]
        );
        let flares: Vec<(Area, Rgb)> = [Side::Left, Side::Right]
            .into_iter()
            .flat_map(|side| icons_of(&paints, Icon::Flare(side)))
            .collect();
        assert!(flares.iter().all(|(_, color)| *color == BLACK));
        assert_eq!(flares.len(), 2);
    }

    #[test]
    fn buttons_under_the_pointer_get_a_round_background() {
        let tabs = [TabView {
            title: "a",
            bell: false,
        }; 2];
        let layout = layout(METRICS, WIDTH, 2, 0);
        let colors = Colors::new(PALETTE);
        let backgrounds = |highlight: Highlight| -> Vec<(Area, Rgb)> {
            icons_of(
                &paints(&layout, &tabs, 0, highlight, PALETTE, &FixedWidth),
                Icon::Dot,
            )
        };
        assert_eq!(backgrounds(Highlight::default()), vec![]);
        let close = layout.tabs()[0].close;
        let over_close = Highlight {
            hovered: Some(Hit::Close(0)),
            pressed: None,
        };
        // 表示しているタブの上では、端末の背景色を文字の色へ寄せる。
        assert_eq!(
            backgrounds(over_close),
            vec![(close, colors.button(BLACK, WHITE, false))]
        );
        // 押したままボタンの上にあれば押している色、外へ出れば地を描かない。
        let pressed = Highlight {
            pressed: Some(Hit::Close(0)),
            ..over_close
        };
        assert_eq!(
            backgrounds(pressed),
            vec![(close, colors.button(BLACK, WHITE, true))]
        );
        let pressed_elsewhere = Highlight {
            hovered: Some(Hit::Tab(0)),
            pressed: Some(Hit::Close(0)),
        };
        assert_eq!(backgrounds(pressed_elsewhere), vec![]);
        // 新しいタブのボタン。地の上では、地の色をコントロールの文字の色へ寄せる。「+」は一辺 20 で
        // 真ん中に、マウスの下ならコントロールの文字の色。
        let over_new_tab = Highlight {
            hovered: Some(Hit::NewTab),
            pressed: None,
        };
        let text = PALETTE.system.text;
        assert_eq!(
            backgrounds(over_new_tab),
            vec![(layout.new_tab, colors.button(colors.bar, text, false))]
        );
        let plus = icons_of(
            &paints(&layout, &tabs, 0, over_new_tab, PALETTE, &FixedWidth),
            Icon::Plus,
        );
        assert_eq!(
            plus,
            vec![(
                square(layout.new_tab.x + 12, layout.new_tab.y + 12, 20),
                text
            )]
        );
        assert_ne!(
            colors.button(BLACK, WHITE, false),
            colors.button(BLACK, WHITE, true)
        );
    }
}
