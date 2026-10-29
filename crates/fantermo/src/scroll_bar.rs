//! 端末の右の縦のスクロールバー。タブバーの下、端末の画面の右に、設定の幅（`scroll_bar.width`）の
//! 場所を常に取っておく（スクロールバックの有無で端末の桁数が変わらないように）。
//!
//! つまみの位置と長さで、スクロールバックを含む全体の中の、表示している範囲を示す。つまみは両端の
//! 丸い細い形で、マウスを載せている間とドラッグしている間は太く描き、トラックも薄く塗る。スクロールバックが
//! ない（代替画面も）ときはつまみを描かない。寸法は DPI に比例させ、色は [`Palette`] にまとめる。
//! つまみの位置と長さの計算と、描くもの（[`Paint`]）の組み立ては、窓や描画に依存しない関数にしてある。

use fantermo_core::{config::SCROLL_BAR_COLOR_KEYS, frame::Rgb, vt::ScrollPosition};

use crate::paint::{Area, Icon, Paint, mix, scale};

/// トラックの上下の端とつまみの間の空き（96 DPI でのピクセル）。
const TRACK_MARGIN: u32 = 2;

/// つまみの長さの下限（96 DPI でのピクセル）。スクロールバックが長くても、つまめる長さに保つ。
const MIN_THUMB_LENGTH: u32 = 24;

/// マウスを載せていないときのつまみの太さの、スクロールバーの幅に対する割合（分母）。
const IDLE_THICKNESS_DIVISOR: u32 = 4;

/// マウスを載せているときのつまみの太さの、スクロールバーの幅に対する割合（分母）。
const HOVERED_THICKNESS_DIVISOR: u32 = 2;

/// つまみの太さの下限（ピクセル）。
const MIN_THICKNESS: u32 = 2;

/// マウスを載せていないつまみの色を、端末の背景色から文字の色へ寄せる割合。
const THUMB_TINT: f32 = 0.35;

/// マウスを載せているつまみの色を、端末の背景色から文字の色へ寄せる割合。
const HOVERED_THUMB_TINT: f32 = 0.5;

/// ドラッグしているつまみの色を、端末の背景色から文字の色へ寄せる割合。
const DRAGGED_THUMB_TINT: f32 = 0.65;

/// マウスを載せている間に塗るトラックの色を、端末の背景色から文字の色へ寄せる割合。
const TRACK_TINT: f32 = 0.06;

/// スクロールバーの寸法を決めるもの。寸法は窓の DPI に比例させる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub dpi: u32,
    /// 幅（96 DPI でのピクセル。設定の `scroll_bar.width`）。
    pub base_width: u32,
}

impl Metrics {
    /// スクロールバーの幅（ピクセル）。
    pub fn width(self) -> u32 {
        scale(self.base_width, self.dpi)
    }

    fn track_margin(self) -> u32 {
        scale(TRACK_MARGIN, self.dpi)
    }

    /// つまみの太さ。`expanded` ならマウスを載せているとき・ドラッグしているときの太さ。
    fn thickness(self, expanded: bool) -> u32 {
        let width = self.width();
        let idle = (width / IDLE_THICKNESS_DIVISOR)
            .max(MIN_THICKNESS)
            .min(width);
        if !expanded {
            return idle;
        }
        (width / HOVERED_THICKNESS_DIVISOR).max(idle)
    }

    /// つまみの長さの下限。太いときの丸い両端が収まる長さより短くしない。
    fn min_thumb_length(self) -> u32 {
        scale(MIN_THUMB_LENGTH, self.dpi).max(self.thickness(true))
    }
}

/// つまみが動くトラック。窓のクライアント領域の中の縦の範囲（ピクセル）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Track {
    pub top: u32,
    pub length: u32,
}

/// スクロールバーを描く領域 `bar` の中のトラック。上下の端から少し空ける。
pub fn track(bar: Area, metrics: Metrics) -> Track {
    let margin = metrics.track_margin().min(bar.height / 2);
    Track {
        top: bar.y + margin,
        length: bar.height - 2 * margin,
    }
}

/// つまみ。窓のクライアント領域の中の縦の範囲（ピクセル）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thumb {
    pub top: u32,
    pub length: u32,
}

impl Thumb {
    pub fn bottom(self) -> u32 {
        self.top + self.length
    }
}

/// 表示している範囲の位置 `position` を示すつまみ。長さはトラックの長さを全体の行数と表示している
/// 行数の比で分けたもの（`min_length` より短くしない）、位置は、つまみが動ける長さを表示している
/// 範囲の先頭で分けたところ。いちばん古い行でトラックの上端、いちばん新しい行でトラックの下端に
/// 付く。表示している範囲が全体を覆えば（スクロールバックがない、代替画面）`None`。
pub fn thumb(track: Track, position: ScrollPosition, min_length: u32) -> Option<Thumb> {
    let scrollable = position.total.checked_sub(position.len)?;
    if scrollable == 0 || track.length == 0 {
        return None;
    }
    let proportional = u64::from(track.length) * position.len as u64 / position.total as u64;
    let length = (proportional as u32).max(min_length).min(track.length);
    let travel = u64::from(track.length - length);
    let offset = position.offset.min(scrollable) as u64;
    Some(Thumb {
        top: track.top + (travel * offset / scrollable as u64) as u32,
        length,
    })
}

/// つまみの上端を `top`（窓のクライアント領域の中の y。トラックからはみ出してもよい）に動かした
/// ときに、表示する範囲の先頭の行。[`thumb`] と対になる。長さ `length` のつまみがトラックの上端で
/// 0、下端でいちばん新しい行（`total - len`）。
pub fn row_at(track: Track, length: u32, position: ScrollPosition, top: i32) -> usize {
    let scrollable = position.total.saturating_sub(position.len) as u64;
    let travel = i64::from(track.length.saturating_sub(length));
    if travel == 0 {
        return 0;
    }
    let moved = (i64::from(top) - i64::from(track.top)).clamp(0, travel) as u64;
    let travel = travel as u64;
    ((moved * scrollable + travel / 2) / travel) as usize
}

/// 表示している行数ずつ動かす向き。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// 過去（上）へ。
    Up,
    /// 新しい行（下）へ。
    Down,
}

impl Direction {
    /// 表示する範囲を動かす行数の符号。
    pub fn sign(self) -> isize {
        match self {
            Direction::Up => -1,
            Direction::Down => 1,
        }
    }
}

/// スクロールバーの中で、つまみに対してどこにあるか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// つまみの上。ドラッグで動かす。
    Thumb,
    /// つまみの上か下のトラック。押すと、その向きへ表示している行数ずつ動かす。
    Page(Direction),
}

/// スクロールバーの中の高さ `y`（窓のクライアント領域の中）が、つまみ `thumb` に対してどこにあるか。
/// 押し続けてページを送るときは、つまみが `y` まで来たら（`Page` の向きが変わったら）止める。
pub fn part_at(thumb: Thumb, y: i32) -> Part {
    if y < thumb.top as i32 {
        Part::Page(Direction::Up)
    } else if y >= thumb.bottom() as i32 {
        Part::Page(Direction::Down)
    } else {
        Part::Thumb
    }
}

/// スクロールバーの色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// マウスを載せている間とドラッグしている間のトラック。それ以外は塗らず、端末の背景色のまま。
    pub track: Rgb,
    /// マウスを載せていないつまみ。
    pub thumb: Rgb,
    /// マウスを載せているつまみ。
    pub hovered_thumb: Rgb,
    /// ドラッグしているつまみ。
    pub dragged_thumb: Rgb,
}

impl Palette {
    /// 端末の背景色 `background` と文字の色 `foreground` から決める既定の色。トラックは背景色にわずかに
    /// 文字の色を混ぜた色、つまみは背景色と文字の色の間の色で、マウスを載せる・ドラッグするほど文字の
    /// 色に寄せる。
    pub fn from_terminal(background: Rgb, foreground: Rgb) -> Self {
        let toward_text = |ratio| mix(background, foreground, ratio);
        Self {
            track: toward_text(TRACK_TINT),
            thumb: toward_text(THUMB_TINT),
            hovered_thumb: toward_text(HOVERED_THUMB_TINT),
            dragged_thumb: toward_text(DRAGGED_THUMB_TINT),
        }
    }

    /// 設定の色 `colors`（[`SCROLL_BAR_COLOR_KEYS`] の順）のうち、書いた色だけで上書きした色。
    pub fn with_colors(mut self, colors: &[Option<Rgb>]) -> Self {
        for (field, color) in self.fields_mut().into_iter().zip(colors) {
            if let Some(color) = color {
                *field = *color;
            }
        }
        self
    }

    /// 色の欄。[`SCROLL_BAR_COLOR_KEYS`] の順。
    fn fields_mut(&mut self) -> [&mut Rgb; SCROLL_BAR_COLOR_KEYS.len()] {
        [
            &mut self.track,
            &mut self.thumb,
            &mut self.hovered_thumb,
            &mut self.dragged_thumb,
        ]
    }
}

/// マウスを載せているか、つまみをドラッグしているか。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Highlight {
    /// マウスがスクロールバーの上にある。
    pub hovered: bool,
    /// つまみをドラッグしている（マウスがスクロールバーの外に出ても）。
    pub dragging: bool,
}

impl Highlight {
    /// つまみを太く描き、トラックを塗るか。
    fn expanded(self) -> bool {
        self.hovered || self.dragging
    }
}

/// 領域 `bar` に描くスクロールバーの、表示している範囲の位置 `position` を示すつまみ。
pub fn thumb_in(bar: Area, metrics: Metrics, position: ScrollPosition) -> Option<Thumb> {
    thumb(track(bar, metrics), position, metrics.min_thumb_length())
}

/// 領域 `bar` に描くスクロールバーの、表示している範囲の位置が `position` のときに描くもの。
pub fn paints(
    bar: Area,
    metrics: Metrics,
    position: ScrollPosition,
    highlight: Highlight,
    colors: &Palette,
) -> Vec<Paint> {
    let mut paints = Vec::new();
    let expanded = highlight.expanded();
    if expanded {
        paints.push(Paint::Fill {
            area: bar,
            color: colors.track,
        });
    }
    let Some(thumb) = thumb_in(bar, metrics, position) else {
        return paints;
    };
    let color = if highlight.dragging {
        colors.dragged_thumb
    } else if highlight.hovered {
        colors.hovered_thumb
    } else {
        colors.thumb
    };
    let thickness = metrics.thickness(expanded).min(bar.width);
    let x = bar.x + (bar.width - thickness) / 2;
    paints.extend(capsule(x, thumb, thickness, color));
    paints
}

/// 左端が `x`、縦の範囲が `thumb`、幅が `thickness` の、上下の端を丸めた形。丸い端は直径が幅の円で、
/// 間を矩形で塗る（円の大きさが太さだけで決まるので、つまみの長さが変わってもアトラスの場所が増えない）。
fn capsule(x: u32, thumb: Thumb, thickness: u32, color: Rgb) -> Vec<Paint> {
    let fill = |y: u32, height: u32| Paint::Fill {
        area: Area {
            x,
            y,
            width: thickness,
            height,
        },
        color,
    };
    if thumb.length < thickness {
        return vec![fill(thumb.top, thumb.length)];
    }
    let end = |y: u32| Paint::Icon {
        icon: Icon::Dot,
        area: Area {
            x,
            y,
            width: thickness,
            height: thickness,
        },
        color,
    };
    let radius = thickness / 2;
    vec![
        fill(thumb.top + radius, thumb.length - 2 * radius),
        end(thumb.top),
        end(thumb.bottom() - thickness),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(total: usize, offset: usize, len: usize) -> ScrollPosition {
        ScrollPosition { total, offset, len }
    }

    const TRACK: Track = Track {
        top: 32,
        length: 480,
    };

    /// 既定の幅（96 DPI で 12）のスクロールバーの、DPI が `dpi` の窓での寸法。
    fn metrics(dpi: u32) -> Metrics {
        Metrics {
            dpi,
            base_width: 12,
        }
    }

    #[test]
    fn the_width_follows_the_dpi_and_the_setting() {
        assert_eq!(metrics(96).width(), 12);
        assert_eq!(metrics(144).width(), 18);
        assert_eq!(metrics(192).width(), 24);
        assert_eq!(metrics(96).thickness(false), 3);
        assert_eq!(metrics(96).thickness(true), 6);
        assert_eq!(metrics(192).thickness(true), 12);
        // 細い設定でも、つまみは 2 ピクセルより細くしない。
        let narrow = Metrics {
            dpi: 96,
            base_width: 4,
        };
        assert_eq!(narrow.width(), 4);
        assert_eq!((narrow.thickness(false), narrow.thickness(true)), (2, 2));
        let wide = Metrics {
            dpi: 192,
            base_width: 20,
        };
        assert_eq!(wide.width(), 40);
        assert_eq!((wide.thickness(false), wide.thickness(true)), (10, 20));
    }

    #[test]
    fn the_track_leaves_a_margin_at_both_ends() {
        let bar = Area {
            x: 800,
            y: 30,
            width: 12,
            height: 484,
        };
        assert_eq!(
            track(bar, metrics(96)),
            Track {
                top: 32,
                length: 480
            }
        );
        // 空きより低い領域でも、トラックの長さは負にならない。
        let low = Area { height: 3, ..bar };
        assert_eq!(track(low, metrics(96)).length, 1);
    }

    #[test]
    fn the_thumb_shows_the_rows_in_view() {
        // 30 行の画面と 70 行のスクロールバック。つまみの長さはトラックの 3 割。
        let oldest = thumb(TRACK, position(100, 0, 30), 24).unwrap();
        assert_eq!(
            oldest,
            Thumb {
                top: 32,
                length: 144
            }
        );
        // いちばん新しい行では、つまみはトラックの下端に付く（割り算の端数でずれない）。
        let newest = thumb(TRACK, position(100, 70, 30), 24).unwrap();
        assert_eq!(newest.bottom(), TRACK.top + TRACK.length);
        let middle = thumb(TRACK, position(100, 35, 30), 24).unwrap();
        assert_eq!(middle.top, 32 + (480 - 144) / 2);
    }

    #[test]
    fn a_long_scrollback_keeps_the_thumb_long_enough_to_grab() {
        let thumb = thumb(TRACK, position(100_000, 99_970, 30), 24).unwrap();
        assert_eq!(thumb.length, 24);
        assert_eq!(thumb.bottom(), TRACK.top + TRACK.length);
    }

    #[test]
    fn without_scrollback_there_is_no_thumb() {
        // スクロールバックがない・代替画面では、全体が表示している範囲と同じ。
        assert_eq!(thumb(TRACK, position(30, 0, 30), 24), None);
        assert_eq!(thumb(TRACK, position(0, 0, 0), 24), None);
        let empty = Track { top: 0, length: 0 };
        assert_eq!(thumb(empty, position(100, 0, 30), 24), None);
    }

    #[test]
    fn dragging_the_thumb_to_either_end_reaches_the_end() {
        let at = position(100, 40, 30);
        let length = thumb(TRACK, at, 24).unwrap().length;
        let travel = (TRACK.length - length) as i32;
        let top = TRACK.top as i32;
        assert_eq!(row_at(TRACK, length, at, top), 0);
        assert_eq!(row_at(TRACK, length, at, top + travel), 70);
        // トラックの外へ出しても端で止まる。
        assert_eq!(row_at(TRACK, length, at, top - 50), 0);
        assert_eq!(row_at(TRACK, length, at, top + travel + 50), 70);
    }

    #[test]
    fn dragging_the_thumb_returns_to_its_row() {
        // つまみの位置から行に戻すと、元の行になる。
        for offset in 0..=70 {
            let at = position(100, offset, 30);
            let shown = thumb(TRACK, at, 24).unwrap();
            assert_eq!(
                row_at(TRACK, shown.length, at, shown.top as i32),
                offset,
                "{offset}"
            );
        }
        // つまみがトラックいっぱいなら動かせない。
        let at = position(100, 0, 30);
        assert_eq!(row_at(Track { top: 0, length: 10 }, 10, at, 5), 0);
    }

    #[test]
    fn clicks_above_and_below_the_thumb_page() {
        let thumb = Thumb {
            top: 100,
            length: 50,
        };
        assert_eq!(part_at(thumb, 99), Part::Page(Direction::Up));
        assert_eq!(part_at(thumb, -5), Part::Page(Direction::Up));
        assert_eq!(part_at(thumb, 100), Part::Thumb);
        assert_eq!(part_at(thumb, 149), Part::Thumb);
        assert_eq!(part_at(thumb, 150), Part::Page(Direction::Down));
        assert_eq!(Direction::Up.sign(), -1);
        assert_eq!(Direction::Down.sign(), 1);
    }

    #[test]
    fn paging_stops_when_the_thumb_reaches_the_pointer() {
        // 下を押し続けると、つまみが押した位置に来たところで向きが変わって止まる。
        let at = |offset| position(100, offset, 30);
        let pointer = 400;
        let mut offset = 0;
        while let Part::Page(Direction::Down) =
            part_at(thumb(TRACK, at(offset), 24).unwrap(), pointer)
        {
            offset = (offset + 30).min(70);
        }
        let stopped = thumb(TRACK, at(offset), 24).unwrap();
        assert!(stopped.top <= pointer as u32 && pointer as u32 <= stopped.bottom());
    }

    const COLORS: Palette = Palette {
        track: Rgb { r: 1, g: 1, b: 1 },
        thumb: Rgb { r: 2, g: 2, b: 2 },
        hovered_thumb: Rgb { r: 3, g: 3, b: 3 },
        dragged_thumb: Rgb { r: 4, g: 4, b: 4 },
    };

    const BAR: Area = Area {
        x: 800,
        y: 30,
        width: 12,
        height: 484,
    };

    fn areas(paints: &[Paint]) -> Vec<(Area, Rgb)> {
        paints
            .iter()
            .map(|paint| match paint {
                Paint::Fill { area, color } | Paint::Icon { area, color, .. } => (*area, *color),
                Paint::Label { .. } => panic!("unexpected label"),
            })
            .collect()
    }

    #[test]
    fn an_idle_thumb_is_thin_and_rounded() {
        let paints = paints(
            BAR,
            metrics(96),
            position(100, 70, 30),
            Highlight::default(),
            &COLORS,
        );
        // トラックは塗らない。幅 3 のつまみを真ん中に、上下の端に直径 3 の円。
        let thumb = Thumb {
            top: 32 + 480 - 144,
            length: 144,
        };
        let at = |y, height| Area {
            x: 804,
            y,
            width: 3,
            height,
        };
        assert_eq!(
            areas(&paints),
            vec![
                (at(thumb.top + 1, 142), COLORS.thumb),
                (at(thumb.top, 3), COLORS.thumb),
                (at(thumb.bottom() - 3, 3), COLORS.thumb),
            ]
        );
        assert!(matches!(
            paints[1],
            Paint::Icon {
                icon: Icon::Dot,
                ..
            }
        ));
    }

    #[test]
    fn a_hovered_bar_shows_its_track_and_a_thick_thumb() {
        let hovered = paints(
            BAR,
            metrics(96),
            position(100, 0, 30),
            Highlight {
                hovered: true,
                dragging: false,
            },
            &COLORS,
        );
        let areas = areas(&hovered);
        assert_eq!(areas[0], (BAR, COLORS.track));
        assert_eq!(
            areas[1],
            (
                Area {
                    x: 803,
                    y: 35,
                    width: 6,
                    height: 138
                },
                COLORS.hovered_thumb
            )
        );
        let dragged = paints(
            BAR,
            metrics(96),
            position(100, 0, 30),
            Highlight {
                hovered: false,
                dragging: true,
            },
            &COLORS,
        );
        assert_eq!(dragged[1..].len(), 3);
        assert!(
            self::areas(&dragged)[1..]
                .iter()
                .all(|&(area, color)| area.width == 6 && color == COLORS.dragged_thumb)
        );
    }

    #[test]
    fn without_scrollback_only_the_hovered_track_is_drawn() {
        let metrics = metrics(96);
        let full = position(30, 0, 30);
        assert!(paints(BAR, metrics, full, Highlight::default(), &COLORS).is_empty());
        let hovered = Highlight {
            hovered: true,
            dragging: false,
        };
        assert_eq!(
            areas(&paints(BAR, metrics, full, hovered, &COLORS)),
            vec![(BAR, COLORS.track)]
        );
    }

    #[test]
    fn colors_lean_from_the_background_toward_the_text() {
        let black = Rgb { r: 0, g: 0, b: 0 };
        let white = Rgb {
            r: 255,
            g: 255,
            b: 255,
        };
        let colors = Palette::from_terminal(black, white);
        let gray = |color: Rgb| color.r;
        assert!(gray(colors.track) < gray(colors.thumb));
        assert!(gray(colors.thumb) < gray(colors.hovered_thumb));
        assert!(gray(colors.hovered_thumb) < gray(colors.dragged_thumb));
        assert!(gray(colors.dragged_thumb) < 255);
    }

    /// 設定のキーと、その色の欄。
    type ColorField = (&'static str, fn(&Palette) -> Rgb);

    #[test]
    fn colors_from_the_settings_replace_only_the_written_ones() {
        let fields: [ColorField; 4] = [
            ("track", |p| p.track),
            ("thumb", |p| p.thumb),
            ("hovered_thumb", |p| p.hovered_thumb),
            ("dragged_thumb", |p| p.dragged_thumb),
        ];
        let black = Rgb { r: 0, g: 0, b: 0 };
        let white = Rgb {
            r: 255,
            g: 255,
            b: 255,
        };
        let defaults = Palette::from_terminal(black, white);
        let written = Rgb { r: 1, g: 2, b: 3 };
        for (index, (name, field)) in fields.iter().enumerate() {
            assert_eq!(SCROLL_BAR_COLOR_KEYS[index], *name);
            let mut colors = [None; SCROLL_BAR_COLOR_KEYS.len()];
            colors[index] = Some(written);
            let palette = defaults.with_colors(&colors);
            assert_eq!(field(&palette), written, "{name}");
            for (other, (_, other_field)) in fields.iter().enumerate() {
                if other != index {
                    assert_eq!(other_field(&palette), other_field(&defaults));
                }
            }
        }
        // ひな形に例として書いた値は、黒の背景と白の文字のときの既定の色。
        let examples: Vec<String> = fields
            .iter()
            .map(|(_, field)| fantermo_core::config::format_color(field(&defaults)))
            .collect();
        assert_eq!(examples, ["#0f0f0f", "#595959", "#808080", "#a6a6a6"]);
    }
}
