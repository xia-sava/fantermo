//! タブバー。ウィンドウの上端に、端末ごとのタブを並べる。端末が一つのときも出す。
//!
//! 寸法は端末のフォントによらず DPI から決め、セルの格子によらずピクセルの位置で並べる。色は
//! [`Palette`] にまとめ、既定では背景と表示していないタブを Windows のコントロールの色より少し濃い
//! グレー、表示しているタブをウィンドウの背景の色（白）で上の角を丸め、左・右・上に細い縁の線を引いて描く。
//! 並べ方（どこがどのタブか、タイトルの省略、ベルの印の位置）と、描くもの（[`crate::paint::Paint`]）の
//! 組み立ては、ウィンドウや描画に依存しない関数にしてある。
//!
//! 定数と寸法（[`Metrics`]）とタイトルの省略をこのファイルに置き、並べ方を `layout`、色と描くものを
//! `paints` に分ける。依存の向きは `paints` → `layout` → このファイル。

mod layout;
mod paints;

pub use layout::{Hit, Layout, layout};
pub use paints::{Highlight, Palette, SystemColors, TabView, paints};

use fantermo_core::{frame::Rgb, vt};

use crate::paint::scale;

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

/// DPI が `dpi` のウィンドウでの、タブのアイコンの一辺（ピクセル）。
pub fn tab_icon_size(dpi: u32) -> u32 {
    Metrics { dpi }.tab_icon()
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
    fn bell_mark(self) -> u32 {
        self.scale(BELL_MARK)
    }

    /// ベルの印とタイトルの間のすき間。
    fn bell_gap(self) -> u32 {
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
    fn button_mark(self) -> u32 {
        self.scale(BUTTON_MARK)
    }

    /// タブの幅の下限と上限。
    fn tab_width_range(self) -> (u32, u32) {
        (self.scale(MIN_TAB_WIDTH), self.scale(MAX_TAB_WIDTH))
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

/// 並べ方と描くものの単体のテストで共有する寸法と書体。
#[cfg(test)]
mod fixtures {
    use super::{ELLIPSIS, Measure, Metrics};
    use crate::paint::Area;

    /// 192 DPI の寸法。タブバーの高さ 52（その下の端末の画面の上の余白 8 は別）、上の空き 4、角の半径
    /// 10、すき間 12、余白 20、ボタン 36、閉じるボタン 32、アイコン 36（タブの左端から 12、右に 10 空ける）、
    /// ベルの印 12、印のすき間 12、タブの幅 144〜480。
    pub const METRICS: Metrics = Metrics { dpi: 192 };

    /// セルの幅が 16 ピクセルの 100 桁のウィンドウのタブバーの幅。
    pub const WIDTH: u32 = 100 * 16;

    /// 幅が ASCII の文字と [`ELLIPSIS`] は 10、ほかの文字は 20 ピクセル、1 行の高さが 32 ピクセルの書体。
    pub struct FixedWidth;

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

    pub fn square(x: u32, y: u32, size: u32) -> Area {
        Area {
            x,
            y,
            width: size,
            height: size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{FixedWidth, METRICS};
    use super::*;

    fn fit(title: &str, width: u32) -> String {
        fit_title(title, width, &FixedWidth)
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

    #[test]
    fn the_icon_size_follows_the_dpi() {
        assert_eq!(tab_icon_size(96), 18);
        assert_eq!(tab_icon_size(144), 27);
        assert_eq!(tab_icon_size(192), 36);
    }
}
