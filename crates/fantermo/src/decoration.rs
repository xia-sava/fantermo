//! 文字の装飾（下線、取り消し線、上線）を、セルの中の形として組み立てる。
//!
//! 形はセルごとに同じ模様を繰り返すので、隣り合うセルの線は途切れずにつながる。座標はセルの左上を
//! 原点とするピクセル。

use fantermo_core::frame::Underline;

use crate::{boxdraw::Shape, font::LineMetrics};

/// 装飾の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Decoration {
    Underline(Underline),
    Strikethrough,
    Overline,
}

/// 波線の振れ幅を決めるときに、セルの高さを割る数。
const CURLY_AMPLITUDE_DIVISOR: f32 = 10.0;

/// セルの中での線の位置と太さ（ピクセル）。ピクセルの境目に揃えてある。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub underline_top: f32,
    pub underline_thickness: f32,
    pub strikethrough_top: f32,
    pub strikethrough_thickness: f32,
}

impl Placement {
    /// ベースラインがセルの上端から `baseline` の位置にあるときの、フォントの計測による線の位置。
    pub fn new(baseline: f32, lines: &LineMetrics, cell_height: f32) -> Self {
        let underline_thickness = pixel_thickness(lines.underline_thickness);
        let strikethrough_thickness = pixel_thickness(lines.strikethrough_thickness);
        let underline_top = (baseline + lines.underline_offset)
            .round()
            .min(cell_height - underline_thickness)
            .max(0.0);
        let strikethrough_top = (baseline - lines.strikethrough_offset)
            .round()
            .min(cell_height - strikethrough_thickness)
            .max(0.0);
        Self {
            underline_top,
            underline_thickness,
            strikethrough_top,
            strikethrough_thickness,
        }
    }
}

/// 太さを整数のピクセルに丸める。少なくとも 1 ピクセル。
fn pixel_thickness(thickness: f32) -> f32 {
    thickness.round().max(1.0)
}

/// 幅 `cell_width` のセル `cells` 個分・高さ `height` に、`decoration` を描く形。
pub fn shapes(
    decoration: Decoration,
    cell_width: f32,
    cells: u8,
    height: f32,
    placement: &Placement,
) -> Vec<Shape> {
    let width = cell_width * f32::from(cells);
    let t = placement.underline_thickness;
    let top = placement.underline_top;
    match decoration {
        Decoration::Underline(Underline::None) => Vec::new(),
        Decoration::Underline(Underline::Single) => vec![rect(0.0, top, width, top + t)],
        Decoration::Underline(Underline::Double) => {
            // 2 本の間に 1 本分のすき間を空ける。セルに収まらなければ上へずらす。
            let top = top.min(height - 3.0 * t).max(0.0);
            vec![
                rect(0.0, top, width, top + t),
                rect(0.0, top + 2.0 * t, width, top + 3.0 * t),
            ]
        }
        Decoration::Underline(Underline::Curly) => {
            let amplitude = (height / CURLY_AMPLITUDE_DIVISOR).round().max(t);
            let center_y = (top + t / 2.0)
                .min(height - amplitude - t / 2.0)
                .max(amplitude + t / 2.0);
            vec![Shape::Wave {
                left: 0.0,
                right: width,
                center_y,
                amplitude,
                period: cell_width,
                width: t,
            }]
        }
        Decoration::Underline(Underline::Dotted) => {
            // 点の間に点 1 個分ほどのすき間を空け、セルの中に均等に並べる。
            let per_cell = (cell_width / (2.0 * t)).floor().max(1.0);
            let spacing = cell_width / per_cell;
            each_cell(cells, cell_width, |x| {
                (0..per_cell as u32)
                    .map(|i| {
                        let left = (x + i as f32 * spacing + (spacing - t) / 2.0).round();
                        rect(left, top, left + t, top + t)
                    })
                    .collect()
            })
        }
        Decoration::Underline(Underline::Dashed) => {
            // セルの真ん中の半分を線にし、両側の 1/4 ずつを隣のセルとの間のすき間にする。
            let (start, end) = ((cell_width / 4.0).round(), (cell_width * 3.0 / 4.0).round());
            each_cell(cells, cell_width, |x| {
                vec![rect(x + start, top, x + end, top + t)]
            })
        }
        Decoration::Strikethrough => {
            let top = placement.strikethrough_top;
            vec![rect(
                0.0,
                top,
                width,
                top + placement.strikethrough_thickness,
            )]
        }
        Decoration::Overline => vec![rect(0.0, 0.0, width, t)],
    }
}

/// セルごとに、左端の x を渡して `cell` で作った形をつなげる。
fn each_cell(cells: u8, cell_width: f32, cell: impl Fn(f32) -> Vec<Shape>) -> Vec<Shape> {
    (0..cells)
        .flat_map(|index| cell(f32::from(index) * cell_width))
        .collect()
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> Shape {
    Shape::Rect {
        left,
        top,
        right,
        bottom,
        alpha: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 10.0;
    const H: f32 = 20.0;

    fn placement() -> Placement {
        Placement::new(
            15.0,
            &LineMetrics {
                underline_offset: 1.8,
                underline_thickness: 1.2,
                strikethrough_offset: 5.4,
                strikethrough_thickness: 0.6,
            },
            H,
        )
    }

    fn draw(decoration: Decoration, cells: u8) -> Vec<Shape> {
        shapes(decoration, W, cells, H, &placement())
    }

    fn rects(shapes: &[Shape]) -> Vec<(f32, f32, f32, f32)> {
        shapes
            .iter()
            .map(|shape| match *shape {
                Shape::Rect {
                    left,
                    top,
                    right,
                    bottom,
                    ..
                } => (left, top, right, bottom),
                _ => panic!("not a rectangle: {shape:?}"),
            })
            .collect()
    }

    #[test]
    fn placement_follows_the_font_and_snaps_to_pixels() {
        let placement = placement();
        assert_eq!(placement.underline_top, 17.0);
        assert_eq!(placement.underline_thickness, 1.0);
        // 取り消し線はベースラインより上。細すぎる線も 1 ピクセルにする。
        assert_eq!(placement.strikethrough_top, 10.0);
        assert_eq!(placement.strikethrough_thickness, 1.0);
    }

    #[test]
    fn lines_below_the_cell_are_pulled_inside() {
        let placement = Placement::new(
            18.0,
            &LineMetrics {
                underline_offset: 4.0,
                underline_thickness: 2.0,
                strikethrough_offset: 5.0,
                strikethrough_thickness: 2.0,
            },
            H,
        );
        assert_eq!(placement.underline_top, H - 2.0);
    }

    #[test]
    fn single_underline_spans_every_cell() {
        assert_eq!(
            rects(&draw(Decoration::Underline(Underline::Single), 2)),
            [(0.0, 17.0, 2.0 * W, 18.0)]
        );
    }

    #[test]
    fn double_underline_leaves_a_gap_and_fits_in_the_cell() {
        assert_eq!(
            rects(&draw(Decoration::Underline(Underline::Double), 1)),
            [(0.0, 17.0, W, 18.0), (0.0, 19.0, W, 20.0)]
        );
        // 太い線で 3 本分が下にはみ出すときは、下端をセルの下端に揃える。
        let thick = Placement {
            underline_thickness: 2.0,
            ..placement()
        };
        assert_eq!(
            rects(&shapes(
                Decoration::Underline(Underline::Double),
                W,
                1,
                H,
                &thick
            )),
            [(0.0, 14.0, W, 16.0), (0.0, 18.0, W, 20.0)]
        );
    }

    #[test]
    fn curly_underline_repeats_once_per_cell() {
        let shapes = draw(Decoration::Underline(Underline::Curly), 2);
        let [
            Shape::Wave {
                left,
                right,
                center_y,
                amplitude,
                period,
                width,
            },
        ] = shapes[..]
        else {
            panic!("not one wave: {shapes:?}");
        };
        // 周期がセルの幅なので、どのセルの左端でも同じ高さと向きで始まり、隣とつながる。
        assert_eq!((left, right, period), (0.0, 2.0 * W, W));
        assert_eq!((amplitude, width), (2.0, 1.0));
        assert!(center_y + amplitude + width / 2.0 <= H);
    }

    #[test]
    fn dotted_and_dashed_repeat_the_same_pattern_in_each_cell() {
        for style in [Underline::Dotted, Underline::Dashed] {
            let one = rects(&draw(Decoration::Underline(style), 1));
            let two = rects(&draw(Decoration::Underline(style), 2));
            let shifted: Vec<_> = one
                .iter()
                .map(|&(left, top, right, bottom)| (left + W, top, right + W, bottom))
                .collect();
            assert_eq!(two, [one.clone(), shifted].concat(), "{style:?}");
            assert!(
                one.iter()
                    .all(|&(left, _, right, _)| 0.0 <= left && right <= W)
            );
        }
    }

    #[test]
    fn dotted_underline_has_gaps() {
        let dots = rects(&draw(Decoration::Underline(Underline::Dotted), 1));
        assert_eq!(dots.len(), 5);
        assert!(dots.windows(2).all(|pair| pair[0].2 < pair[1].0));
    }

    #[test]
    fn strikethrough_and_overline_use_their_own_positions() {
        assert_eq!(
            rects(&draw(Decoration::Strikethrough, 1)),
            [(0.0, 10.0, W, 11.0)]
        );
        assert_eq!(rects(&draw(Decoration::Overline, 1)), [(0.0, 0.0, W, 1.0)]);
        assert!(draw(Decoration::Underline(Underline::None), 1).is_empty());
    }
}
