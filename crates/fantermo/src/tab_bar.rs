//! タブバー。窓の上端に、端末ごとのタブを並べる。端末が一つのときも出す。
//!
//! 寸法はセルの幅と高さから決め、セルの格子によらずピクセルの位置で並べる。表示しているタブは
//! 端末と同じ色で上の角を丸め、下の辺を端末へつなげて描く。並べ方（どこがどのタブか、タイトルの
//! 省略、ベルの印の位置）と、描くもの（[`Paint`]）の組み立ては、窓や描画に依存しない関数にしてある。

use fantermo_core::{frame::Rgb, vt};

use crate::paint::{Area, Corner, Icon, Paint, Side};

/// タブの幅の上限（セルの幅の何倍か）。
const MAX_TAB_CELLS: u32 = 24;

/// タブが多いときに縮める、タブの幅の下限（セルの幅の何倍か）。これより狭くなるほど多ければ、
/// 入りきらないタブを描かない。
const MIN_TAB_CELLS: u32 = 6;

/// 収まらないタイトルの末尾に付ける印。
const ELLIPSIS: &str = "…";

/// 表示していない間にベルが鳴ったタブの、タイトルの前の印（円）の色。
const BELL_COLOR: Rgb = Rgb {
    r: 0xf0,
    g: 0xb0,
    b: 0x30,
};

/// タブバーの地の色を、端末の背景色から文字の色へ寄せる割合。
const BAR_TINT: f32 = 0.12;

/// 表示していないタブのタイトルの色を、端末の文字の色から背景色へ寄せる割合。
const INACTIVE_TITLE_FADE: f32 = 0.35;

/// タブの間の区切りの線の色を、端末の背景色から文字の色へ寄せる割合。
const SEPARATOR_TINT: f32 = 0.30;

/// マウスの下の、表示していないタブの色を、端末の背景色から文字の色へ寄せる割合。
const HOVERED_TAB_TINT: f32 = 0.20;

/// マウスの下のボタン（×、+）の丸い地の色を、ボタンの下の色から文字の色へ寄せる割合。
const HOVERED_BUTTON_TINT: f32 = 0.15;

/// 押しているボタンの丸い地の色を、ボタンの下の色から文字の色へ寄せる割合。
const PRESSED_BUTTON_TINT: f32 = 0.28;

/// タブバーの高さ（ピクセル）。セルの高さ `cell_height` に、上下の余白を足す。
pub fn height(cell_height: u32) -> u32 {
    cell_height + 2 * margin(cell_height)
}

/// タブバーの上下の余白と、タブの角の半径（ピクセル）。セルの高さの 1/4。
fn margin(cell_height: u32) -> u32 {
    (cell_height / 4).max(1)
}

/// セルの寸法（ピクセル）。タブバーの寸法はここから決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub cell_width: u32,
    pub cell_height: u32,
}

impl Metrics {
    fn bar_height(self) -> u32 {
        height(self.cell_height)
    }

    /// タブの上の空き。表示しているタブの角の半径にも使う。
    fn margin(self) -> u32 {
        margin(self.cell_height)
    }

    /// タブの中の左右の余白。タブの並びの左右の余白にも使う。
    fn padding(self) -> u32 {
        self.cell_width.max(1)
    }

    /// ベルの印の円の直径。
    fn mark_size(self) -> u32 {
        (self.cell_height * 2 / 5).max(1)
    }

    /// ベルの印とタイトルの間のすき間。
    fn mark_gap(self) -> u32 {
        (self.cell_width / 2).max(1)
    }

    /// タブの間の区切りの線の太さと高さ。
    fn separator(self) -> (u32, u32) {
        ((self.cell_height / 32).max(1), self.cell_height / 2)
    }

    /// タブを閉じるボタン（丸い地）の直径。
    fn close_button(self) -> u32 {
        (self.cell_height * 3 / 4).max(1)
    }

    /// 閉じるボタンの「×」の一辺。
    fn close_mark(self) -> u32 {
        (self.cell_height / 3).max(1)
    }

    /// 新しいタブのボタンとメニューのボタン（丸い地）の直径。
    fn button(self) -> u32 {
        self.cell_height.max(1)
    }

    /// 新しいタブのボタンの「+」と、メニューのボタンの「≡」の一辺。
    fn plus_mark(self) -> u32 {
        (self.cell_height * 2 / 5).max(1)
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
/// 新しいタブのボタンを置く。タブは同じ幅で、上限はセル [`MAX_TAB_CELLS`] 個分。多ければ
/// [`MIN_TAB_CELLS`] 個分まで縮め、それでも入りきらなければ、表示しているタブ `active` を含む続きの
/// タブだけを並べる。
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
    let min_width = MIN_TAB_CELLS * metrics.cell_width.max(1);
    let max_width = MAX_TAB_CELLS * metrics.cell_width.max(1);
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

/// `title` を幅 `cols`（セル）に収めた書記素クラスタの並び。収まらなければ末尾を [`ELLIPSIS`] にする。
/// 幅のない書記素クラスタ（制御文字など）は除く。
pub fn fit_title(title: &str, cols: u16) -> Vec<vt::Grapheme> {
    let graphemes: Vec<vt::Grapheme> = vt::split_graphemes(title)
        .into_iter()
        .filter(|grapheme| grapheme.width > 0)
        .collect();
    let total: u32 = graphemes.iter().map(|g| u32::from(g.width)).sum();
    if total <= u32::from(cols) {
        return graphemes;
    }
    if cols == 0 {
        return Vec::new();
    }
    let ellipsis = vt::split_graphemes(ELLIPSIS);
    let ellipsis_cols: u32 = ellipsis.iter().map(|g| u32::from(g.width)).sum();
    let room = u32::from(cols).saturating_sub(ellipsis_cols);
    let mut fitted = Vec::new();
    let mut used = 0u32;
    for grapheme in graphemes {
        if used + u32::from(grapheme.width) > room {
            break;
        }
        used += u32::from(grapheme.width);
        fitted.push(grapheme);
    }
    fitted.extend(ellipsis);
    fitted
}

/// タブ一つの中の、ベルの印とタイトルの位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TabContent {
    /// ベルの印の円を描く矩形。印を付けないときは `None`。
    mark: Option<Area>,
    /// タイトルの左端。
    title_x: u32,
    /// タイトルに充てる幅（セル）。
    title_cols: u16,
    /// タイトルの上端。
    text_y: u32,
}

/// タブ `slot` の中の、閉じるボタンより左に、ベルの印（`bell` のとき）とタイトルを置く。閉じるボタンを
/// 出していないときも、その幅は空けておく（マウスを載せてもタイトルの省略が変わらないように）。
/// 印はタイトルの左に置き、タイトルを省略する前にその幅を取る。タイトルに残る幅がなくても印は消さず、
/// 余白に収まらなければタブの真ん中に置き、タブより大きければタブの幅に縮める。
fn tab_content(metrics: Metrics, slot: &TabSlot, bell: bool) -> TabContent {
    let tab = slot.area;
    let left = tab.x + metrics.padding();
    let right = slot.close.x;
    let text_y = tab.y + tab.height.saturating_sub(metrics.cell_height) / 2;
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
    let title_cols = right.saturating_sub(title_x) / metrics.cell_width.max(1);
    TabContent {
        mark,
        title_x,
        title_cols: title_cols.min(u32::from(u16::MAX)) as u16,
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

/// タブバーの色。端末の背景色と文字の色から決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Colors {
    bar: Rgb,
    active_tab: Rgb,
    active_title: Rgb,
    inactive_title: Rgb,
    separator: Rgb,
    hovered_tab: Rgb,
    foreground: Rgb,
}

impl Colors {
    fn new(background: Rgb, foreground: Rgb) -> Self {
        Self {
            bar: mix(background, foreground, BAR_TINT),
            // 表示しているタブは端末と同じ色にして、端末とつながって見えるようにする。
            active_tab: background,
            active_title: foreground,
            inactive_title: mix(foreground, background, INACTIVE_TITLE_FADE),
            separator: mix(background, foreground, SEPARATOR_TINT),
            hovered_tab: mix(background, foreground, HOVERED_TAB_TINT),
            foreground,
        }
    }

    /// 色 `base` の上に置いたボタンの丸い地の色。`pressed` なら押しているとき、そうでなければ
    /// マウスの下にあるとき。
    fn button(&self, base: Rgb, pressed: bool) -> Rgb {
        let tint = if pressed {
            PRESSED_BUTTON_TINT
        } else {
            HOVERED_BUTTON_TINT
        };
        mix(base, self.foreground, tint)
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
/// いるタブの位置、`highlight` はマウスの下にあるものと押しているもの。色は端末の背景色 `background`
/// と文字の色 `foreground` から決める。表示しているタブのタイトルは太字にする。閉じるボタンは、
/// 表示しているタブとマウスの下のタブにだけ出す。
pub fn paints(
    layout: &Layout,
    tabs: &[TabView],
    active: usize,
    highlight: Highlight,
    background: Rgb,
    foreground: Rgb,
) -> Vec<Paint> {
    let metrics = layout.metrics;
    let colors = Colors::new(background, foreground);
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
            (colors.hovered_tab, colors.active_title)
        } else {
            (colors.bar, colors.inactive_title)
        };
        let content = tab_content(metrics, slot, tab.bell);
        if let Some(mark) = content.mark {
            paints.push(Paint::Icon {
                icon: Icon::Dot,
                area: mark,
                color: BELL_COLOR,
            });
        }
        let mut x = content.title_x;
        for grapheme in fit_title(tab.title, content.title_cols) {
            paints.push(Paint::Text {
                x,
                y: content.text_y,
                text: grapheme.text,
                cells: grapheme.width,
                bold: is_active,
                color: title_color,
            });
            x += u32::from(grapheme.width) * metrics.cell_width;
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
                },
            );
        }
    }
    for (hit, area, icon) in [
        (Hit::NewTab, layout.new_tab, Icon::Plus),
        (Hit::Menu, layout.menu, Icon::Menu),
    ] {
        let mark_color = if highlight.hovered == Some(hit) {
            colors.active_title
        } else {
            colors.inactive_title
        };
        push_button(
            &mut paints,
            &colors,
            Button {
                area,
                mark: (icon, metrics.plus_mark(), mark_color),
                state: highlight.state(hit),
                base: colors.bar,
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
    /// ボタンの下の色。丸い地の色はここから決める。
    base: Rgb,
}

fn push_button(paints: &mut Vec<Paint>, colors: &Colors, button: Button) {
    let Button {
        area,
        mark: (icon, mark_size, mark_color),
        state,
        base,
    } = button;
    if let Some(pressed) = state {
        paints.push(Paint::Icon {
            icon: Icon::Dot,
            area,
            color: colors.button(base, pressed),
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

    /// セルが 16×32 ピクセルのときの寸法。
    const METRICS: Metrics = Metrics {
        cell_width: 16,
        cell_height: 32,
    };

    /// 100 桁の窓のタブバーの幅。
    const WIDTH: u32 = 100 * 16;

    fn text(graphemes: &[vt::Grapheme]) -> String {
        graphemes.iter().map(|g| g.text.as_str()).collect()
    }

    fn xs(layout: &Layout) -> Vec<(usize, u32, u32)> {
        layout
            .tabs()
            .iter()
            .map(|tab| (tab.index, tab.area.x, tab.area.width))
            .collect()
    }

    #[test]
    fn the_bar_height_follows_the_cell_height() {
        // 上下に 1/4 ずつの余白。
        assert_eq!(height(32), 48);
        assert_eq!(height(24), 36);
        assert_eq!(height(16), 24);
        assert_eq!(height(2), 4);
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
        // メニューのボタンは、左端に上の余白と同じすき間を空けて、タブの高さの真ん中に置く。
        assert_eq!(layout.menu(), square(8, 12, 32));
        // タブは、メニューのボタンの右に同じすき間を空けて置く。
        assert_eq!(xs(&layout), vec![(0, 48, 24 * 16)]);
        let tab = layout.tabs()[0];
        // 上の余白の下から、タブバーの下の辺まで。
        assert_eq!((tab.area.y, tab.area.bottom()), (8, 48));
        // 閉じるボタンは右の余白の半分を残してタブの右端に、高さの真ん中に置く。
        assert_eq!(tab.close, square(400, 16, 24));
        // 新しいタブのボタンは、タブの右に同じすき間を空けて置く。
        assert_eq!(layout.new_tab, square(440, 12, 32));
    }

    #[test]
    fn few_tabs_have_the_widest_width() {
        let layout = layout(METRICS, WIDTH, 2, 1);
        assert_eq!(xs(&layout), vec![(0, 48, 384), (1, 432, 384)]);
        assert_eq!(layout.new_tab.x, 824);
    }

    #[test]
    fn many_tabs_share_the_bar_equally() {
        // メニューのボタンまでと、右の余白・すき間・新しいタブのボタンを除いた 1496 ピクセルを
        // 5 等分する。
        let layout = layout(METRICS, WIDTH, 5, 0);
        assert_eq!(layout.tabs().len(), 5);
        assert!(layout.tabs().iter().all(|tab| tab.area.width == 299));
        assert!(layout.new_tab.right() <= WIDTH - 16);
    }

    #[test]
    fn tabs_that_do_not_fit_leave_the_active_one_shown() {
        // 1496 ピクセルにはセル 6 個分（96）のタブが 15 個まで入る。
        let first = layout(METRICS, WIDTH, 40, 0);
        assert_eq!(first.tabs().len(), 15);
        assert_eq!(first.tabs()[0].index, 0);
        assert!(first.tabs().iter().all(|tab| tab.area.width == 99));
        assert!(first.new_tab.right() <= WIDTH - 16);
        // 入りきらない位置のタブを表示していれば、それが右端に来るまで並びをずらす。
        let middle = layout(METRICS, WIDTH, 40, 20);
        let indexes: Vec<usize> = middle.tabs().iter().map(|tab| tab.index).collect();
        assert_eq!(indexes, (6..=20).collect::<Vec<_>>());
        let last = layout(METRICS, WIDTH, 40, 39);
        assert_eq!(last.tabs()[14].index, 39);
        // 入りきるうちの位置なら左端から。
        assert_eq!(layout(METRICS, WIDTH, 40, 14).tabs()[0].index, 0);
    }

    #[test]
    fn a_narrow_bar_still_shows_one_tab() {
        let layout = layout(METRICS, 144, 3, 2);
        assert_eq!(xs(&layout), vec![(2, 48, 40)]);
        // 閉じるボタンはタブからはみ出さない。
        assert_eq!(layout.tabs()[0].close, square(56, 16, 24));
        assert_eq!(super::layout(METRICS, 104, 3, 0).tabs(), &[]);
        assert_eq!(super::layout(METRICS, WIDTH, 0, 0).tabs(), &[]);
    }

    #[test]
    fn positions_map_to_tabs_and_buttons() {
        let layout = layout(METRICS, WIDTH, 2, 0);
        assert_eq!(layout.hit(48, 20), Some(Hit::Tab(0)));
        // タブの上の余白もそのタブ。
        assert_eq!(layout.hit(431, 0), Some(Hit::Tab(0)));
        assert_eq!(layout.hit(432, 47), Some(Hit::Tab(1)));
        // 閉じるボタンと、その上下はタブ。
        assert_eq!(layout.hit(400, 16), Some(Hit::Close(0)));
        assert_eq!(layout.hit(423, 39), Some(Hit::Close(0)));
        assert_eq!(layout.hit(410, 15), Some(Hit::Tab(0)));
        assert_eq!(layout.hit(790, 30), Some(Hit::Close(1)));
        // 新しいタブのボタンは、上から下まで。
        assert_eq!(layout.hit(824, 0), Some(Hit::NewTab));
        assert_eq!(layout.hit(855, 47), Some(Hit::NewTab));
        // メニューのボタンは、窓の左端から、上から下まで。
        assert_eq!(layout.hit(0, 0), Some(Hit::Menu));
        assert_eq!(layout.hit(39, 47), Some(Hit::Menu));
        // メニューのボタンとタブの間、タブとボタンの間、右の空き、タブバーの下。
        assert_eq!(layout.hit(44, 20), None);
        assert_eq!(layout.hit(816, 20), None);
        assert_eq!(layout.hit(856, 20), None);
        assert_eq!(layout.hit(432, 48), None);
        assert_eq!(Hit::Close(1).tab(), Some(1));
        assert_eq!(Hit::NewTab.tab(), None);
        assert_eq!(Hit::Menu.tab(), None);
    }

    #[test]
    fn long_titles_end_with_an_ellipsis() {
        assert_eq!(text(&fit_title("zsh", 10)), "zsh");
        assert_eq!(text(&fit_title("abcdefghij", 10)), "abcdefghij");
        assert_eq!(text(&fit_title("abcdefghijk", 10)), "abcdefghi…");
        assert_eq!(text(&fit_title("abc", 0)), "");
        assert_eq!(text(&fit_title("abc", 1)), "…");
    }

    #[test]
    fn wide_characters_are_not_split() {
        // 「端末」は 4 セル。3 セルに収めるなら「端」と省略の印。
        assert_eq!(text(&fit_title("端末", 4)), "端末");
        assert_eq!(text(&fit_title("端末", 3)), "端…");
        // 2 セルなら「端」は入らず、省略の印だけ。
        assert_eq!(text(&fit_title("端末x", 2)), "…");
    }

    #[test]
    fn control_characters_are_dropped() {
        assert_eq!(text(&fit_title("a\u{7}b", 10)), "ab");
    }

    /// 左端が `x`、幅が `width` のタブ。閉じるボタンは [`layout`] と同じく右端から 32 ピクセル。
    fn tab(x: u32, width: u32) -> TabSlot {
        let close_x = (x + width).saturating_sub(32).max(x);
        TabSlot {
            index: 0,
            area: Area {
                x,
                y: 8,
                width,
                height: 40,
            },
            close: square(close_x, 16, 24),
        }
    }

    #[test]
    fn a_bell_mark_comes_before_the_title() {
        let plain = tab_content(METRICS, &tab(16, 384), false);
        assert_eq!(plain.mark, None);
        // タイトルは閉じるボタンの左まで。
        assert_eq!((plain.title_x, plain.title_cols), (32, 21));
        // タイトルはタブの高さの真ん中に置く。
        assert_eq!(plain.text_y, 12);
        let marked = tab_content(METRICS, &tab(16, 384), true);
        // 直径 12 の円を左の余白の次に置き、すき間 8 を空けてタイトル。
        assert_eq!(
            marked.mark,
            Some(Area {
                x: 32,
                y: 22,
                width: 12,
                height: 12
            })
        );
        assert_eq!((marked.title_x, marked.title_cols), (52, 19));
    }

    #[test]
    fn a_bell_mark_stays_in_narrow_tabs() {
        // タブの幅ごとに、印の左端と直径、タイトルに残る幅（セル）。
        let cases: [(u32, (u32, u32), u16); 5] = [
            // 6 セル分: 余白、印、すき間、タイトル 1 セル、閉じるボタン。
            (96, (16, 12), 1),
            // 余白と閉じるボタンの間に印が収まる。タイトルに残る幅はない。
            (60, (16, 12), 0),
            // 収まらなければ真ん中に置く。
            (40, (14, 12), 0),
            (12, (0, 12), 0),
            // タブより大きければタブの幅に縮める。
            (8, (0, 8), 0),
        ];
        for (width, (x, size), cols) in cases {
            let content = tab_content(METRICS, &tab(0, width), true);
            let mark = content.mark.expect("a bell mark");
            assert_eq!((mark.x, mark.width), (x, size), "width {width}");
            assert!(mark.right() <= width, "width {width}");
            assert_eq!(content.title_cols, cols, "width {width}");
        }
    }

    fn titles(paints: &[Paint]) -> Vec<(u32, String, bool, Rgb)> {
        paints
            .iter()
            .filter_map(|paint| match paint {
                Paint::Text {
                    x,
                    text,
                    bold,
                    color,
                    ..
                } => Some((*x, text.clone(), *bold, *color)),
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
        ];
        let layout = layout(METRICS, WIDTH, 2, 1);
        let paints = paints(&layout, &tabs, 1, Highlight::default(), BLACK, WHITE);
        assert_eq!(dots(&paints).len(), 1);
        assert!(dots(&paints)[0].right() <= 432);
        let first: String = titles(&paints)
            .iter()
            .filter(|(x, ..)| *x < 432)
            .map(|(_, text, ..)| text.as_str())
            .collect();
        assert_eq!(first, r"C:\WINDOWS\System3…");
        // 印のないタブのタイトルは、印の分だけ長い。
        let second: String = titles(&paints)
            .iter()
            .filter(|(x, ..)| *x >= 432)
            .map(|(_, text, ..)| text.as_str())
            .collect();
        assert_eq!(second, r"C:\WINDOWS\System32\…");
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
        let paints = paints(&layout, &tabs, 3, Highlight::default(), BLACK, WHITE);
        // 描く 15 個のうち、偶数番の 8 個に印。
        assert_eq!(dots(&paints).len(), 8);
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
        let paints = paints(&layout, &tabs, 1, Highlight::default(), BLACK, WHITE);
        let colors = Colors::new(BLACK, WHITE);
        // 地はタブバーの全体。
        assert_eq!(
            paints[0],
            Paint::Fill {
                area: Area {
                    x: 0,
                    y: 0,
                    width: WIDTH,
                    height: 48
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
                (Icon::Corner(Corner::TopLeft), square(432, 8, 8)),
                (Icon::Corner(Corner::TopRight), square(808, 8, 8)),
                (Icon::Flare(Side::Left), square(424, 40, 8)),
                (Icon::Flare(Side::Right), square(816, 40, 8)),
            ]
        );
        // 表示しているタブのタイトルは太字で端末の文字の色、ほかは薄い色。
        assert_eq!(
            titles(&paints),
            vec![
                (64, "o".to_owned(), false, colors.inactive_title),
                (80, "n".to_owned(), false, colors.inactive_title),
                (96, "e".to_owned(), false, colors.inactive_title),
                (448, "端".to_owned(), true, WHITE),
                (480, "末".to_owned(), true, WHITE),
            ]
        );
        // メニューのボタンの「≡」は、一辺 12 でボタンの真ん中に。
        assert_eq!(
            icons_of(&paints, Icon::Menu),
            vec![(square(18, 22, 12), colors.inactive_title)]
        );
    }

    #[test]
    fn separators_stand_between_tabs_that_are_not_shown() {
        let tabs = [TabView {
            title: "a",
            bell: false,
        }; 4];
        let layout = layout(METRICS, WIDTH, 4, 1);
        let colors = Colors::new(BLACK, WHITE);
        let separators = |highlight: Highlight| -> Vec<u32> {
            paints(&layout, &tabs, 1, highlight, BLACK, WHITE)
                .iter()
                .filter_map(|paint| match paint {
                    Paint::Fill { area, color } if *color == colors.separator => Some(area.x),
                    _ => None,
                })
                .collect()
        };
        // 表示している 1 番の左右には引かない。2 番と 3 番の間だけ。
        assert_eq!(
            separators(Highlight::default()),
            vec![layout.tabs()[3].area.x]
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
        let colors = Colors::new(BLACK, WHITE);
        let close_marks = |highlight: Highlight| -> Vec<u32> {
            icons_of(
                &paints(&layout, &tabs, 0, highlight, BLACK, WHITE),
                Icon::Close,
            )
            .iter()
            .map(|(area, _)| area.x)
            .collect()
        };
        // 「×」は一辺 10 で、ボタンの真ん中に。
        let close = layout.tabs()[0].close;
        assert_eq!(close_marks(Highlight::default()), vec![close.x + 7]);
        let hovered = Highlight {
            hovered: Some(Hit::Tab(2)),
            pressed: None,
        };
        assert_eq!(
            close_marks(hovered),
            vec![close.x + 7, layout.tabs()[2].close.x + 7]
        );
        // マウスの下のタブは、上の角を丸めて少し明るい色で描く。下の左右は反らせない。
        let paints = paints(&layout, &tabs, 0, hovered, BLACK, WHITE);
        let corners = icons_of(&paints, Icon::Corner(Corner::TopLeft));
        assert_eq!(
            corners,
            vec![
                (square(layout.tabs()[2].area.x, 8, 8), colors.hovered_tab),
                (square(48, 8, 8), BLACK),
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
        let colors = Colors::new(BLACK, WHITE);
        let backgrounds = |highlight: Highlight| -> Vec<(Area, Rgb)> {
            icons_of(
                &paints(&layout, &tabs, 0, highlight, BLACK, WHITE),
                Icon::Dot,
            )
        };
        assert_eq!(backgrounds(Highlight::default()), vec![]);
        let close = layout.tabs()[0].close;
        let over_close = Highlight {
            hovered: Some(Hit::Close(0)),
            pressed: None,
        };
        assert_eq!(
            backgrounds(over_close),
            vec![(close, colors.button(BLACK, false))]
        );
        // 押したままボタンの上にあれば押している色、外へ出れば地を描かない。
        let pressed = Highlight {
            pressed: Some(Hit::Close(0)),
            ..over_close
        };
        assert_eq!(
            backgrounds(pressed),
            vec![(close, colors.button(BLACK, true))]
        );
        let pressed_elsewhere = Highlight {
            hovered: Some(Hit::Tab(0)),
            pressed: Some(Hit::Close(0)),
        };
        assert_eq!(backgrounds(pressed_elsewhere), vec![]);
        // 新しいタブのボタン。「+」は一辺 12 で真ん中に、マウスの下なら文字の色。
        let over_new_tab = Highlight {
            hovered: Some(Hit::NewTab),
            pressed: None,
        };
        assert_eq!(
            backgrounds(over_new_tab),
            vec![(layout.new_tab, colors.button(colors.bar, false))]
        );
        let plus = icons_of(
            &paints(&layout, &tabs, 0, over_new_tab, BLACK, WHITE),
            Icon::Plus,
        );
        assert_eq!(
            plus,
            vec![(
                square(layout.new_tab.x + 10, layout.new_tab.y + 10, 12),
                WHITE
            )]
        );
        assert_ne!(colors.button(BLACK, false), colors.button(BLACK, true));
    }
}
