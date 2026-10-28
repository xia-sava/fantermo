//! Powerline の記号（U+E0B0〜U+E0BF）。三角形、山形、半円、斜線。
//!
//! 塗りつぶす形は平らな辺をセルの縁いっぱいに置き、隣のセルの背景と継ぎ目なくつながる。枝や鍵の
//! ような絵（U+E0A0〜U+E0A3）はフォントに任せる。

use std::f32::consts::FRAC_PI_2;

use super::{Cell, Shape};

/// 四分円 1 つを折れ線で近似するときの辺の数。
const ARC_SEGMENTS: u32 = 16;

impl Cell {
    pub(super) fn powerline(&self, c: char) -> Option<Vec<Shape>> {
        let (w, h) = (self.width, self.height);
        let shape = match c {
            '\u{e0b0}' => Self::polygon(vec![(0.0, 0.0), (w, h / 2.0), (0.0, h)]),
            '\u{e0b1}' => self.chevron(),
            '\u{e0b2}' => self.mirrored(Self::polygon(vec![(0.0, 0.0), (w, h / 2.0), (0.0, h)])),
            '\u{e0b3}' => self.mirrored(self.chevron()),
            '\u{e0b4}' => Self::polygon(self.half_circle(0.0)),
            '\u{e0b5}' => self.stroke(self.half_circle(self.thickness / 2.0)),
            '\u{e0b6}' => self.mirrored(Self::polygon(self.half_circle(0.0))),
            '\u{e0b7}' => self.mirrored(self.stroke(self.half_circle(self.thickness / 2.0))),
            '\u{e0b8}' => Self::polygon(vec![(0.0, 0.0), (w, h), (0.0, h)]),
            '\u{e0ba}' => Self::polygon(vec![(w, 0.0), (w, h), (0.0, h)]),
            '\u{e0bc}' => Self::polygon(vec![(0.0, 0.0), (w, 0.0), (0.0, h)]),
            '\u{e0be}' => Self::polygon(vec![(0.0, 0.0), (w, 0.0), (w, h)]),
            '\u{e0b9}' | '\u{e0bf}' => self.diagonal((0.0, 0.0), (w, h)),
            '\u{e0bb}' | '\u{e0bd}' => self.diagonal((w, 0.0), (0.0, h)),
            _ => return None,
        };
        Some(vec![shape])
    }

    /// 左の上下の角から右の縁の真ん中へ向かう、右向きの山形。
    fn chevron(&self) -> Shape {
        let (w, h) = (self.width, self.height);
        self.stroke(vec![(0.0, 0.0), (w, h / 2.0), (0.0, h)])
    }

    /// 平らな辺を左の縁に置き、右へふくらむ半円の縁。半径はセルの幅と高さの半分の小さいほうで、
    /// 高さが余れば上下の四分円を縦の線でつなぐ。`inset` だけ内側へ寄せる（縁取りの線の中心）。
    fn half_circle(&self, inset: f32) -> Vec<(f32, f32)> {
        let h = self.height;
        let radius = self.width.min(h / 2.0);
        let r = radius - inset;
        let quarter = |center_y: f32, from: f32| {
            (0..=ARC_SEGMENTS).map(move |i| {
                let angle = from + FRAC_PI_2 * i as f32 / ARC_SEGMENTS as f32;
                (r * angle.cos(), center_y + r * angle.sin())
            })
        };
        let mut points: Vec<(f32, f32)> = quarter(radius, -FRAC_PI_2).collect();
        for point in quarter(h - radius, 0.0) {
            if points.last() != Some(&point) {
                points.push(point);
            }
        }
        // 両端は縁の上で、cos の誤差で縁からずれないようにする。
        let last = points.len() - 1;
        points[0].0 = 0.0;
        points[last].0 = 0.0;
        points
    }

    /// 左右を入れ替えた形。
    fn mirrored(&self, shape: Shape) -> Shape {
        let flip = |points: Vec<(f32, f32)>| -> Vec<(f32, f32)> {
            points
                .into_iter()
                .map(|(x, y)| (self.width - x, y))
                .collect()
        };
        match shape {
            Shape::Polygon { points, alpha } => Shape::Polygon {
                points: flip(points),
                alpha,
            },
            Shape::Polyline { points, width } => Shape::Polyline {
                points: flip(points),
                width,
            },
            other => other,
        }
    }
}
