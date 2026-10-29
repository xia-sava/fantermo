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
}

/// タブバーの中のタブ一つ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabSlot {
    /// 端末の並びの中の位置。
    pub index: usize,
    /// タブを描く矩形。上の余白の下から、タブバーの下の辺まで。
    pub area: Area,
}

/// タブバーの並べ方。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    metrics: Metrics,
    /// タブバーの幅（ピクセル）。
    width: u32,
    /// 描くタブ。左から並べた順。入りきらないタブは含まない。
    tabs: Vec<TabSlot>,
}

/// タブバーの上の、押して働くもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// 端末の並びの中の位置がこの値のタブ。
    Tab(usize),
}

/// 幅 `width` のタブバーに、`tabs` 個のタブを左から並べる。タブは同じ幅で、上限はセル
/// [`MAX_TAB_CELLS`] 個分。多ければ [`MIN_TAB_CELLS`] 個分まで縮め、それでも入りきらなければ、
/// 表示しているタブ `active` を含む続きのタブだけを並べる。
pub fn layout(metrics: Metrics, width: u32, tabs: usize, active: usize) -> Layout {
    let mut layout = Layout {
        metrics,
        width,
        tabs: Vec::new(),
    };
    if tabs == 0 {
        return layout;
    }
    let start = metrics.padding();
    let end = width.saturating_sub(metrics.padding());
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
    let top = metrics.margin();
    let tab_height = metrics.bar_height() - top;
    layout.tabs = (0..shown)
        .map(|offset| TabSlot {
            index: first + offset,
            area: Area {
                x: start + tab_width * offset as u32,
                y: top,
                width: tab_width,
                height: tab_height,
            },
        })
        .collect();
    layout
}

impl Layout {
    /// 描くタブ。
    #[cfg(test)]
    fn tabs(&self) -> &[TabSlot] {
        &self.tabs
    }

    /// タブバーの中の (`x`, `y`) にあるもの。タブの上の余白も、そのタブとみなす。何もなければ
    /// `None`。
    pub fn hit(&self, x: i32, y: i32) -> Option<Hit> {
        self.tabs
            .iter()
            .find(|tab| column(tab.area, self.metrics).contains(x, y))
            .map(|tab| Hit::Tab(tab.index))
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

/// 矩形 `tab` のタブの中に、ベルの印（`bell` のとき）とタイトルを置く。印はタイトルの左に置き、
/// タイトルを省略する前にその幅を取る。タイトルに残る幅がなくても印は消さず、余白に収まらなければ
/// タブの真ん中に置き、タブより大きければタブの幅に縮める。
fn tab_content(metrics: Metrics, tab: Area, bell: bool) -> TabContent {
    let left = tab.x + metrics.padding();
    let right = tab.right().saturating_sub(metrics.padding());
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
        }
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
/// いるタブの位置。色は端末の背景色 `background` と文字の色 `foreground` から決める。表示している
/// タブのタイトルは太字にする。
pub fn paints(
    layout: &Layout,
    tabs: &[TabView],
    active: usize,
    background: Rgb,
    foreground: Rgb,
) -> Vec<Paint> {
    let metrics = layout.metrics;
    let colors = Colors::new(background, foreground);
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
        if left.index == active || right.index == active {
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
    if let Some(tab) = layout.tabs.iter().find(|tab| tab.index == active) {
        push_raised_tab(&mut paints, layout, tab.area, colors.active_tab);
    }
    for slot in &layout.tabs {
        let Some(tab) = tabs.get(slot.index) else {
            continue;
        };
        let is_active = slot.index == active;
        let title_color = if is_active {
            colors.active_title
        } else {
            colors.inactive_title
        };
        let content = tab_content(metrics, slot.area, tab.bell);
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
    }
    paints
}

/// 矩形 `area` に、上の角を丸め、下の辺の左右を外へ反らせたタブを `color` で描く。
fn push_raised_tab(paints: &mut Vec<Paint>, layout: &Layout, area: Area, color: Rgb) {
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

    #[test]
    fn a_single_tab_is_laid_out() {
        let layout = layout(METRICS, WIDTH, 1, 0);
        assert_eq!(xs(&layout), vec![(0, 16, 24 * 16)]);
        let tab = layout.tabs()[0].area;
        // 上の余白の下から、タブバーの下の辺まで。
        assert_eq!((tab.y, tab.bottom()), (8, 48));
    }

    #[test]
    fn few_tabs_have_the_widest_width() {
        let layout = layout(METRICS, WIDTH, 2, 1);
        assert_eq!(xs(&layout), vec![(0, 16, 384), (1, 400, 384)]);
    }

    #[test]
    fn many_tabs_share_the_bar_equally() {
        // 左右の余白を除いた 1568 ピクセルを 5 等分する。
        let layout = layout(METRICS, WIDTH, 5, 0);
        assert_eq!(layout.tabs().len(), 5);
        assert!(layout.tabs().iter().all(|tab| tab.area.width == 313));
        let last = layout.tabs()[4].area;
        assert!(last.right() <= WIDTH - 16);
    }

    #[test]
    fn tabs_that_do_not_fit_leave_the_active_one_shown() {
        // 1568 ピクセルにはセル 6 個分（96）のタブが 16 個まで入る。
        let first = layout(METRICS, WIDTH, 40, 0);
        assert_eq!(first.tabs().len(), 16);
        assert_eq!(first.tabs()[0].index, 0);
        assert!(first.tabs().iter().all(|tab| tab.area.width == 98));
        // 入りきらない位置のタブを表示していれば、それが右端に来るまで並びをずらす。
        let middle = layout(METRICS, WIDTH, 40, 20);
        let indexes: Vec<usize> = middle.tabs().iter().map(|tab| tab.index).collect();
        assert_eq!(indexes, (5..=20).collect::<Vec<_>>());
        let last = layout(METRICS, WIDTH, 40, 39);
        assert_eq!(last.tabs()[15].index, 39);
        // 入りきるうちの位置なら左端から。
        assert_eq!(layout(METRICS, WIDTH, 40, 15).tabs()[0].index, 0);
    }

    #[test]
    fn a_narrow_bar_still_shows_one_tab() {
        let layout = layout(METRICS, 64, 3, 2);
        assert_eq!(xs(&layout), vec![(2, 16, 32)]);
        assert_eq!(super::layout(METRICS, 32, 3, 0).tabs(), &[]);
        assert_eq!(super::layout(METRICS, WIDTH, 0, 0).tabs(), &[]);
    }

    #[test]
    fn positions_map_to_tabs() {
        let layout = layout(METRICS, WIDTH, 2, 0);
        assert_eq!(layout.hit(16, 20), Some(Hit::Tab(0)));
        // タブの上の余白もそのタブ。
        assert_eq!(layout.hit(399, 0), Some(Hit::Tab(0)));
        assert_eq!(layout.hit(400, 47), Some(Hit::Tab(1)));
        // 左の余白と、右の空き。
        assert_eq!(layout.hit(15, 20), None);
        assert_eq!(layout.hit(784, 20), None);
        assert_eq!(layout.hit(400, 48), None);
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

    fn tab(x: u32, width: u32) -> Area {
        Area {
            x,
            y: 8,
            width,
            height: 40,
        }
    }

    #[test]
    fn a_bell_mark_comes_before_the_title() {
        let plain = tab_content(METRICS, tab(16, 384), false);
        assert_eq!(plain.mark, None);
        assert_eq!((plain.title_x, plain.title_cols), (32, 22));
        // タイトルはタブの高さの真ん中に置く。
        assert_eq!(plain.text_y, 12);
        let marked = tab_content(METRICS, tab(16, 384), true);
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
        assert_eq!((marked.title_x, marked.title_cols), (52, 20));
    }

    #[test]
    fn a_bell_mark_stays_in_narrow_tabs() {
        // タブの幅ごとに、印の左端と直径、タイトルに残る幅（セル）。
        let cases: [(u32, (u32, u32), u16); 5] = [
            // 6 セル分: 余白、印、すき間、タイトル 2 セル、余白。
            (96, (16, 12), 2),
            // 余白の中に印が収まる。タイトルに残る幅はない。
            (44, (16, 12), 0),
            // 余白に収まらなければ真ん中に置く。
            (40, (14, 12), 0),
            (12, (0, 12), 0),
            // タブより大きければタブの幅に縮める。
            (8, (0, 8), 0),
        ];
        for (width, (x, size), cols) in cases {
            let content = tab_content(METRICS, tab(0, width), true);
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
        let paints = paints(&layout, &tabs, 1, BLACK, WHITE);
        assert_eq!(dots(&paints).len(), 1);
        assert!(dots(&paints)[0].right() <= 400);
        let first: String = titles(&paints)
            .iter()
            .filter(|(x, ..)| *x < 400)
            .map(|(_, text, ..)| text.as_str())
            .collect();
        assert_eq!(first, r"C:\WINDOWS\System32…");
        // 印のないタブのタイトルは、印の分だけ長い。
        let second: String = titles(&paints)
            .iter()
            .filter(|(x, ..)| *x >= 400)
            .map(|(_, text, ..)| text.as_str())
            .collect();
        assert_eq!(second, r"C:\WINDOWS\System32\W…");
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
        let paints = paints(&layout, &tabs, 3, BLACK, WHITE);
        // 描く 16 個のうち、偶数番の 8 個に印。
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
        let paints = paints(&layout, &tabs, 1, BLACK, WHITE);
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
        let square = |x, y| Area {
            x,
            y,
            width: 8,
            height: 8,
        };
        assert_eq!(
            icons,
            vec![
                (Icon::Corner(Corner::TopLeft), square(400, 8)),
                (Icon::Corner(Corner::TopRight), square(776, 8)),
                (Icon::Flare(Side::Left), square(392, 40)),
                (Icon::Flare(Side::Right), square(784, 40)),
            ]
        );
        // 表示しているタブのタイトルは太字で端末の文字の色、ほかは薄い色。
        assert_eq!(
            titles(&paints),
            vec![
                (32, "o".to_owned(), false, colors.inactive_title),
                (48, "n".to_owned(), false, colors.inactive_title),
                (64, "e".to_owned(), false, colors.inactive_title),
                (416, "端".to_owned(), true, WHITE),
                (448, "末".to_owned(), true, WHITE),
            ]
        );
    }

    #[test]
    fn separators_stand_between_tabs_that_are_not_shown() {
        let tabs = [TabView {
            title: "a",
            bell: false,
        }; 4];
        let layout = layout(METRICS, WIDTH, 4, 1);
        let paints = paints(&layout, &tabs, 1, BLACK, WHITE);
        let colors = Colors::new(BLACK, WHITE);
        let separators: Vec<u32> = paints
            .iter()
            .filter_map(|paint| match paint {
                Paint::Fill { area, color } if *color == colors.separator => Some(area.x),
                _ => None,
            })
            .collect();
        // 表示している 1 番の左右には引かない。2 番と 3 番の間だけ。
        assert_eq!(separators, vec![layout.tabs()[3].area.x]);
    }
}
