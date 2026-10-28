//! タブバー。端末が二つ以上あるとき、窓の上端に端末と同じ書体の 1 行で、端末ごとのタブを並べる。
//!
//! タブの並べ方（どの桁がどのタブか、幅、タイトルの省略）と、描くための画面（[`Frame`]）の組み立ては、
//! 窓や描画に依存しない関数にしてある。

use fantermo_core::{
    frame::{CellView, Frame, Rgb, Underline},
    vt,
};

/// タブバーを出す、端末の数の下限。端末が一つならタブバーを出さない。
const MIN_TABS_TO_SHOW: usize = 2;

/// タブの幅の上限（セル）。
const MAX_TAB_COLS: u16 = 24;

/// タブとタブの間のすき間（セル）。
const TAB_GAP_COLS: u16 = 1;

/// タブの中の、タイトルの左右の余白（セル）。
const TAB_PADDING_COLS: u16 = 1;

/// 収まらないタイトルの末尾に付ける印。
const ELLIPSIS: &str = "…";

/// 表示していない間にベルが鳴ったタブの、タイトルの前の印。
const BELL_MARK: &str = "●";

/// [`BELL_MARK`] を描く幅（セル）。曖昧幅の文字だが、全角の幅でデザインしたフォントでも縮めずに丸く
/// 描けるよう、2 セルを充てる。
const BELL_MARK_WIDTH: u8 = 2;

/// [`BELL_MARK`] と、タイトルとの間のすき間を合わせた幅（セル）。
const BELL_MARK_COLS: u16 = BELL_MARK_WIDTH as u16 + 1;

/// [`BELL_MARK`] の色。
const BELL_COLOR: Rgb = Rgb {
    r: 0xf0,
    g: 0xb0,
    b: 0x30,
};

/// タブバーの背景色を、端末の背景色から文字の色へ寄せる割合。
const BAR_TINT: f32 = 0.10;

/// 表示していないタブの背景色を、端末の背景色から文字の色へ寄せる割合。
const INACTIVE_TAB_TINT: f32 = 0.22;

/// 表示していないタブのタイトルの色を、端末の文字の色から背景色へ寄せる割合。
const INACTIVE_TITLE_FADE: f32 = 0.35;

/// タブバーの高さ（ピクセル）。端末が `tabs` 個のとき、二つ以上なら 1 セル分、一つなら 0。
pub fn height(cell_height_px: u32, tabs: usize) -> u32 {
    if tabs >= MIN_TABS_TO_SHOW {
        cell_height_px
    } else {
        0
    }
}

/// タブバーの中のタブ一つの位置。単位はセル。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabSpan {
    pub col: u16,
    pub cols: u16,
}

/// `tabs` 個のタブを、幅 `cols` のタブバーに左から並べる。タブは同じ幅で、上限は [`MAX_TAB_COLS`]。
/// 収まらないほど多ければ幅を 1 セルまで縮め、それでも右端からはみ出すタブは返さない（返す数が `tabs`
/// より少なくなる）。
pub fn layout(tabs: usize, cols: u16) -> Vec<TabSpan> {
    if tabs == 0 || cols == 0 {
        return Vec::new();
    }
    let tabs_u32 = u32::try_from(tabs).unwrap_or(u32::MAX);
    let per_tab = (u32::from(cols) + u32::from(TAB_GAP_COLS)) / tabs_u32;
    let width = per_tab
        .saturating_sub(u32::from(TAB_GAP_COLS))
        .clamp(1, u32::from(MAX_TAB_COLS)) as u16;
    let mut spans = Vec::new();
    let mut col = 0u16;
    for _ in 0..tabs {
        if col >= cols {
            break;
        }
        spans.push(TabSpan {
            col,
            cols: width.min(cols - col),
        });
        col = col.saturating_add(width).saturating_add(TAB_GAP_COLS);
    }
    spans
}

/// タブバーの `col` の桁にあるタブの番号。タブの間のすき間や右の空きなら `None`。
pub fn tab_at(spans: &[TabSpan], col: u16) -> Option<usize> {
    spans
        .iter()
        .position(|span| (span.col..span.col + span.cols).contains(&col))
}

/// `title` を幅 `cols` に収めた書記素クラスタの並び。収まらなければ末尾を [`ELLIPSIS`] にする。
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

/// タブ一つに描くもの。
#[derive(Clone, Copy, Debug)]
pub struct TabView<'a> {
    pub title: &'a str,
    /// 表示していない間にベルが鳴った。タイトルの前に [`BELL_MARK`] を付ける。
    pub bell: bool,
}

/// タブバーの色。端末の背景色と文字の色から決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Colors {
    bar: Rgb,
    active_tab: Rgb,
    active_title: Rgb,
    inactive_tab: Rgb,
    inactive_title: Rgb,
}

impl Colors {
    fn new(background: Rgb, foreground: Rgb) -> Self {
        Self {
            bar: mix(background, foreground, BAR_TINT),
            // 表示しているタブは端末と同じ色にして、端末とつながって見えるようにする。
            active_tab: background,
            active_title: foreground,
            inactive_tab: mix(background, foreground, INACTIVE_TAB_TINT),
            inactive_title: mix(foreground, background, INACTIVE_TITLE_FADE),
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

/// 幅 `cols` のタブバーを 1 行の画面として組み立てる。`active` は表示しているタブの番号。色は端末の
/// 背景色 `background` と文字の色 `foreground` から決める。表示しているタブのタイトルは太字にする。
pub fn frame(
    tabs: &[TabView],
    active: usize,
    cols: u16,
    background: Rgb,
    foreground: Rgb,
) -> Frame {
    let colors = Colors::new(background, foreground);
    let mut cells = Vec::new();
    for (index, (tab, span)) in tabs.iter().zip(layout(tabs.len(), cols)).enumerate() {
        let is_active = index == active;
        let (tab_bg, title_fg) = if is_active {
            (colors.active_tab, colors.active_title)
        } else {
            (colors.inactive_tab, colors.inactive_title)
        };
        let blank = |col: u16| CellView {
            col,
            row: 0,
            text: String::new(),
            width: 1,
            fg: title_fg,
            bg: Some(tab_bg),
            bold: is_active,
            italic: false,
            underline: Underline::None,
            underline_color: None,
            strikethrough: false,
            overline: false,
        };
        let end = span.col + span.cols;
        let mut text_cols = span.cols.saturating_sub(TAB_PADDING_COLS * 2);
        let mut col = span.col;
        while col < (span.col + TAB_PADDING_COLS).min(end) {
            cells.push(blank(col));
            col += 1;
        }
        if tab.bell && text_cols >= BELL_MARK_COLS {
            cells.push(CellView {
                text: BELL_MARK.to_owned(),
                width: BELL_MARK_WIDTH,
                fg: BELL_COLOR,
                ..blank(col)
            });
            cells.push(blank(col + u16::from(BELL_MARK_WIDTH)));
            col += BELL_MARK_COLS;
            text_cols -= BELL_MARK_COLS;
        }
        for grapheme in fit_title(tab.title, text_cols) {
            cells.push(CellView {
                text: grapheme.text,
                width: grapheme.width,
                ..blank(col)
            });
            col += u16::from(grapheme.width);
        }
        while col < end {
            cells.push(blank(col));
            col += 1;
        }
    }
    Frame {
        cols,
        rows: 1,
        background: colors.bar,
        foreground,
        cells,
        cursor: None,
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

    fn span(col: u16, cols: u16) -> TabSpan {
        TabSpan { col, cols }
    }

    fn text(graphemes: &[vt::Grapheme]) -> String {
        graphemes.iter().map(|g| g.text.as_str()).collect()
    }

    #[test]
    fn the_bar_appears_with_two_tabs() {
        assert_eq!(height(32, 1), 0);
        assert_eq!(height(32, 2), 32);
        assert_eq!(height(32, 5), 32);
    }

    #[test]
    fn few_tabs_have_the_widest_width() {
        assert_eq!(layout(2, 100), vec![span(0, 24), span(25, 24)]);
    }

    #[test]
    fn many_tabs_share_the_bar_equally() {
        // (100 + 1) / 5 - 1 = 19 セルずつ、すき間 1 セル。
        let spans = layout(5, 100);
        assert_eq!(spans.len(), 5);
        assert!(spans.iter().all(|span| span.cols == 19));
        assert_eq!(spans[4], span(80, 19));
        assert!(spans[4].col + spans[4].cols <= 100);
    }

    #[test]
    fn tabs_that_do_not_fit_are_left_out() {
        // 1 セルずつに縮めても 3 セルの中には 2 つしか入らない。
        assert_eq!(layout(3, 3), vec![span(0, 1), span(2, 1)]);
        assert_eq!(layout(2, 0), vec![]);
        assert_eq!(layout(0, 10), vec![]);
    }

    #[test]
    fn columns_map_to_tabs() {
        let spans = layout(2, 100);
        assert_eq!(tab_at(&spans, 0), Some(0));
        assert_eq!(tab_at(&spans, 23), Some(0));
        // すき間。
        assert_eq!(tab_at(&spans, 24), None);
        assert_eq!(tab_at(&spans, 25), Some(1));
        assert_eq!(tab_at(&spans, 48), Some(1));
        assert_eq!(tab_at(&spans, 49), None);
        assert_eq!(tab_at(&spans, 99), None);
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

    #[test]
    fn a_bell_puts_a_mark_before_the_title() {
        let tabs = [
            TabView {
                title: "abcdefghijklmnopqrstuvwxyz",
                bell: true,
            },
            TabView {
                title: "b",
                bell: false,
            },
        ];
        let frame = frame(&tabs, 1, 100, BLACK, WHITE);
        let titled: Vec<(u16, &str, Rgb)> = frame
            .cells
            .iter()
            .filter(|cell| cell.col < 24 && !cell.text.is_empty())
            .map(|cell| (cell.col, cell.text.as_str(), cell.fg))
            .collect();
        assert_eq!(titled[0], (1, BELL_MARK, BELL_COLOR));
        // 印（2 セル）とすき間の 3 セルだけ、タイトルが短くなる。
        assert_eq!(titled[1].0, 4);
        let title: String = titled[1..].iter().map(|(_, text, _)| *text).collect();
        assert_eq!(title, "abcdefghijklmnopqr…");
        assert!(
            frame
                .cells
                .iter()
                .filter(|cell| cell.col >= 25)
                .all(|cell| cell.text != BELL_MARK)
        );
    }

    #[test]
    fn the_frame_fills_each_tab_and_marks_the_active_one() {
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
        let frame = frame(&tabs, 1, 60, BLACK, WHITE);
        assert_eq!((frame.cols, frame.rows), (60, 1));
        assert_eq!(frame.cursor, None);
        // 表示しているタブは端末の背景色、ほかのタブとタブバーは文字の色へ寄せた色。
        let colors = Colors::new(BLACK, WHITE);
        assert_eq!(frame.background, colors.bar);
        assert_ne!(colors.bar, colors.inactive_tab);
        let first = frame.cells.iter().filter(|cell| cell.col < 24);
        assert!(
            first
                .clone()
                .all(|cell| cell.bg == Some(colors.inactive_tab))
        );
        assert!(first.clone().all(|cell| !cell.bold));
        assert_eq!(first.count(), 24);
        let second: Vec<&CellView> = frame.cells.iter().filter(|cell| cell.col >= 25).collect();
        assert!(
            second
                .iter()
                .all(|cell| cell.bg == Some(BLACK) && cell.bold)
        );
        // 左の余白の次からタイトル。全角の文字は 2 セルを占め、右半分のセルは持たない。
        let titled: Vec<(u16, &str)> = second
            .iter()
            .filter(|cell| !cell.text.is_empty())
            .map(|cell| (cell.col, cell.text.as_str()))
            .collect();
        assert_eq!(titled, vec![(26, "端"), (28, "末")]);
        let covered: u32 = second.iter().map(|cell| u32::from(cell.width)).sum();
        assert_eq!(covered, 24);
    }
}
