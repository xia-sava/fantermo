//! タブバーの並べ方。タブとボタンの位置、当たり判定、タブ一つの中のアイコンとベルの印とタイトルの位置。

use super::Metrics;
use crate::paint::Area;

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
    pub(super) metrics: Metrics,
    /// タブバーの幅（ピクセル）。
    pub(super) width: u32,
    /// 描くタブ。左から並べた順。入りきらないタブは含まない。
    pub(super) tabs: Vec<TabSlot>,
    /// 新しいタブのボタン。タブの並びの右に置く。
    pub(super) new_tab: Area,
    /// プロファイルの一覧のボタン。新しいタブのボタンのすぐ右に置く。
    pub(super) profiles: Area,
    /// メニューのボタン。左端に置く。
    pub(super) menu: Area,
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
/// 新しいタブのボタンと、そのすぐ右にプロファイルの一覧のボタンを置く。タブは同じ幅で、上限は [`super::MAX_TAB_WIDTH`]。多ければ [`super::MIN_TAB_WIDTH`]
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
    #[cfg(any(test, feature = "e2e"))]
    pub fn tabs(&self) -> &[TabSlot] {
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

/// タブ一つの中の、アイコンとベルの印とタイトルの位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TabContent {
    /// アイコンを描く正方形。タブが狭くて入らないときは `None`。
    pub(super) icon: Option<Area>,
    /// ベルの印の円を描く矩形。印を付けないときは `None`。
    pub(super) mark: Option<Area>,
    /// タイトルの左端。
    pub(super) title_x: u32,
    /// タイトルに充てる幅（ピクセル）。
    pub(super) title_width: u32,
    /// タイトルの上端。
    pub(super) text_y: u32,
}

/// タブ `slot` の中の、閉じるボタンより左に、アイコンと、ベルの印（`bell` のとき）と、1 行の高さが
/// `text_height` のタイトルを左から順に置く。閉じるボタンを出していないときも、その幅は空けておく（マウスを
/// 載せてもタイトルの省略が変わらないように）。アイコンと印の幅は、タイトルを省略する前に取る。アイコンと
/// 印が閉じるボタンの左に並ばないほどタブが狭ければ、状態を知らせる印を残してアイコンを出さない。タイトルに
/// 残る幅がなくても印は消さず、余白に収まらなければタブの真ん中に置き、タブより大きければタブの幅に
/// 縮める。
pub(super) fn tab_content(
    metrics: Metrics,
    slot: &TabSlot,
    bell: bool,
    text_height: u32,
) -> TabContent {
    let tab = slot.area;
    let right = slot.close.x;
    let text_y = tab.y + tab.height.saturating_sub(text_height) / 2;
    let icon_size = metrics.tab_icon();
    let icon_x = tab.x + metrics.tab_icon_margin();
    let needed_after_icon = if bell {
        metrics.tab_icon_gap() + metrics.bell_mark()
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
        let size = metrics.bell_mark().min(tab.width).min(tab.height);
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
        title_x = x + size + metrics.bell_gap();
    }
    TabContent {
        icon,
        mark,
        title_x,
        title_width: right.saturating_sub(title_x),
        text_y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tab_bar::fixtures::{METRICS, WIDTH, square};

    fn xs(layout: &Layout) -> Vec<(usize, u32, u32)> {
        layout
            .tabs()
            .iter()
            .map(|tab| (tab.index, tab.area.x, tab.area.width))
            .collect()
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
}
