//! 端末の画面（[`fantermo_core::frame::Frame`]）のほかに窓に描くもの。セルの格子によらず、ピクセルの
//! 位置で塗りつぶし・形・文字を並べる。タブバーに使う。

use std::f32::consts::{FRAC_PI_2, PI};

use fantermo_core::frame::Rgb;

use crate::boxdraw::Shape;

/// 円や円弧を多角形にするときの、一周の頂点の数。
const CIRCLE_SEGMENTS: usize = 48;

/// バックバッファ（窓のクライアント領域）の中の矩形。単位はピクセル。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Area {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Area {
    pub fn right(&self) -> u32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> u32 {
        self.y + self.height
    }

    /// (`x`, `y`) がこの矩形の中にある。
    pub fn contains(&self, x: i32, y: i32) -> bool {
        let inside = |value: i32, start: u32, length: u32| {
            i64::from(value) >= i64::from(start) && i64::from(value) < i64::from(start + length)
        };
        inside(x, self.x, self.width) && inside(y, self.y, self.height)
    }
}

/// 描くもの一つ。並べた順に描く。
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    /// `area` を `color` で塗る。
    Fill { area: Area, color: Rgb },
    /// `area` に `icon` を `color` で描く。形は `area` の大きさで組み立てる。
    Icon { icon: Icon, area: Area, color: Rgb },
    /// 左上が (`x`, `y`) のところに、Windows の UI のフォントで `text` を 1 行に描く。
    Label {
        x: u32,
        y: u32,
        text: String,
        bold: bool,
        color: Rgb,
    },
}

/// 矩形の角。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Corner {
    TopLeft,
    TopRight,
}

/// [`Paint::Icon`] で描く形。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    /// 四分円で丸めた角。描く矩形を、その角が丸い矩形の角として塗る。
    Corner(Corner),
    /// 塗りつぶした円。
    Dot,
    /// タブを閉じるボタンの「×」。
    Close,
    /// 新しいタブのボタンの「+」。
    Plus,
    /// メニューのボタンの「≡」（横線 3 本）。
    Menu,
}

impl Icon {
    /// 幅 `width`・高さ `height` の矩形に描く形。座標は矩形の左上からのピクセル。
    pub fn shapes(self, width: f32, height: f32) -> Vec<Shape> {
        match self {
            Icon::Corner(corner) => vec![corner_shape(corner, width, height)],
            Icon::Close => {
                let thickness = stroke(width.min(height));
                // 線の端の角が矩形からはみ出さないよう、太さの半分だけ内へ寄せる。
                let inset = thickness / 2.0;
                vec![
                    Shape::Line {
                        from: (inset, inset),
                        to: (width - inset, height - inset),
                        width: thickness,
                    },
                    Shape::Line {
                        from: (width - inset, inset),
                        to: (inset, height - inset),
                        width: thickness,
                    },
                ]
            }
            Icon::Plus => {
                // 縦横の線をピクセルの境目に合わせ、にじませない。
                let thickness = stroke(width.min(height)).round();
                let x = ((width - thickness) / 2.0).round();
                let y = ((height - thickness) / 2.0).round();
                vec![
                    rect(0.0, y, width, y + thickness),
                    rect(x, 0.0, x + thickness, height),
                ]
            }
            Icon::Menu => {
                let thickness = stroke(width.min(height)).round();
                let middle = ((height - thickness) / 2.0).round();
                let bottom = height - thickness;
                [0.0, middle, bottom]
                    .into_iter()
                    .map(|top| rect(0.0, top, width, top + thickness))
                    .collect()
            }
            Icon::Dot => {
                let radius = width.min(height) / 2.0;
                let center = (width / 2.0, height / 2.0);
                vec![Shape::Polygon {
                    points: arc(center, radius, 0.0, 2.0 * PI),
                    alpha: 1.0,
                }]
            }
        }
    }
}

/// 一辺 `size` の「×」と「+」の線の太さ。
fn stroke(size: f32) -> f32 {
    (size / 8.0).max(1.0)
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

/// 中心 `center`、半径 `radius` の円周の、角度 `from` から `to` まで（ラジアン。y が下向きなので、
/// 増えると右回り）の点。両端を含む。
fn arc(center: (f32, f32), radius: f32, from: f32, to: f32) -> Vec<(f32, f32)> {
    let steps = (((to - from).abs() / (2.0 * PI)) * CIRCLE_SEGMENTS as f32).ceil() as usize;
    let steps = steps.max(1);
    (0..=steps)
        .map(|step| {
            let angle = from + (to - from) * step as f32 / steps as f32;
            (
                center.0 + radius * angle.cos(),
                center.1 + radius * angle.sin(),
            )
        })
        .collect()
}

/// 幅 `width`・高さ `height` の矩形を、`corner` の角が丸い矩形の角として塗る形。
fn corner_shape(corner: Corner, width: f32, height: f32) -> Shape {
    // 楕円にしないよう、円の半径は短い辺に合わせる。
    let radius = width.min(height);
    let (center, start) = match corner {
        Corner::TopLeft => ((width, height), PI),
        Corner::TopRight => ((width - radius, height), PI + FRAC_PI_2),
    };
    let mut points = arc(center, radius, start, start + FRAC_PI_2);
    points.push(center);
    Shape::Polygon { points, alpha: 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polygon_points(shape: &Shape) -> &[(f32, f32)] {
        match shape {
            Shape::Polygon { points, .. } => points,
            other => panic!("not a polygon: {other:?}"),
        }
    }

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }

    #[test]
    fn areas_contain_their_inside() {
        let area = Area {
            x: 10,
            y: 20,
            width: 5,
            height: 3,
        };
        assert!(area.contains(10, 20));
        assert!(area.contains(14, 22));
        assert!(!area.contains(15, 22));
        assert!(!area.contains(14, 23));
        assert!(!area.contains(9, 20));
        assert!(!area.contains(-1, -1));
        assert_eq!((area.right(), area.bottom()), (15, 23));
    }

    #[test]
    fn a_top_left_corner_bulges_toward_the_top_left() {
        let shapes = Icon::Corner(Corner::TopLeft).shapes(8.0, 8.0);
        let points = polygon_points(&shapes[0]);
        // 左下 (0, 8) から右上 (8, 0) へ円弧を描き、右下の中心へ戻る。
        assert!(near(points[0], (0.0, 8.0)));
        assert!(near(*points.last().unwrap(), (8.0, 8.0)));
        assert!(points.iter().any(|&p| near(p, (8.0, 0.0))));
        // どの点も右下の中心から半径の内にあり、左上の角を通らない（丸めてある）。
        assert!(
            points
                .iter()
                .all(|&(x, y)| { ((x - 8.0).powi(2) + (y - 8.0).powi(2)).sqrt() <= 8.0 + 1e-3 })
        );
        assert!(!points.iter().any(|&p| near(p, (0.0, 0.0))));
    }

    #[test]
    fn every_corner_stays_inside_its_square() {
        for corner in [Corner::TopLeft, Corner::TopRight] {
            let shapes = Icon::Corner(corner).shapes(6.0, 6.0);
            for &(x, y) in polygon_points(&shapes[0]) {
                assert!((-1e-3..=6.0 + 1e-3).contains(&x), "{corner:?}");
                assert!((-1e-3..=6.0 + 1e-3).contains(&y), "{corner:?}");
            }
        }
    }

    #[test]
    fn the_close_mark_crosses_the_square() {
        let shapes = Icon::Close.shapes(16.0, 16.0);
        assert_eq!(
            shapes,
            vec![
                Shape::Line {
                    from: (1.0, 1.0),
                    to: (15.0, 15.0),
                    width: 2.0
                },
                Shape::Line {
                    from: (15.0, 1.0),
                    to: (1.0, 15.0),
                    width: 2.0
                },
            ]
        );
    }

    #[test]
    fn the_plus_mark_lies_on_whole_pixels() {
        assert_eq!(
            Icon::Plus.shapes(13.0, 13.0),
            vec![rect(0.0, 6.0, 13.0, 8.0), rect(6.0, 0.0, 8.0, 13.0)]
        );
    }

    #[test]
    fn the_menu_mark_has_three_bars() {
        assert_eq!(
            Icon::Menu.shapes(12.0, 12.0),
            vec![
                rect(0.0, 0.0, 12.0, 2.0),
                rect(0.0, 5.0, 12.0, 7.0),
                rect(0.0, 10.0, 12.0, 12.0),
            ]
        );
    }

    #[test]
    fn a_dot_is_a_circle_in_the_middle() {
        let shapes = Icon::Dot.shapes(10.0, 10.0);
        for &(x, y) in polygon_points(&shapes[0]) {
            let distance = ((x - 5.0).powi(2) + (y - 5.0).powi(2)).sqrt();
            assert!((distance - 5.0).abs() < 1e-3);
        }
    }
}
