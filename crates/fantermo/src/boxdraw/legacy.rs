//! レガシーコンピューティング記号（U+1FB00〜U+1FBAF）のうち、ブロックと斜めの塗りと斜線。
//!
//! セクスタント、斜めの塗り、三角形、1/8 のブロック、網かけ、細線の斜線を描く。模様の塗り
//! （U+1FB95〜U+1FB99）、縦の短い線つきの横線（U+1FBAF）、絵（U+1FBB0〜）はフォントに任せる。
//! 塗りの境目は、セクスタントと斜めの塗りで同じ位置（横の 1/2、縦の 1/3 と 2/3）に揃える。

use super::{Cell, Shape};

/// セルの縁の上の点。横は左・真ん中・右、縦は上・上から 1/3・上から 2/3・下の番号。
type Point = (usize, usize);

const UPPER_LEFT: Point = (0, 0);
const UPPER_CENTRE: Point = (1, 0);
const UPPER_RIGHT: Point = (2, 0);
const UPPER_MIDDLE_LEFT: Point = (0, 1);
const UPPER_MIDDLE_RIGHT: Point = (2, 1);
const LOWER_MIDDLE_LEFT: Point = (0, 2);
const LOWER_MIDDLE_RIGHT: Point = (2, 2);
const LOWER_LEFT: Point = (0, 3);
const LOWER_CENTRE: Point = (1, 3);
const LOWER_RIGHT: Point = (2, 3);

/// U+1FB3C から順に、斜めの塗り（BLOCK DIAGONAL）の、塗る側の角と斜めの線の両端。
/// 文字の名前の「<角> BLOCK DIAGONAL <始点> TO <終点>」をそのまま並べる。
const WEDGES: [(Point, Point, Point); 44] = [
    (LOWER_LEFT, LOWER_MIDDLE_LEFT, LOWER_CENTRE),
    (LOWER_LEFT, LOWER_MIDDLE_LEFT, LOWER_RIGHT),
    (LOWER_LEFT, UPPER_MIDDLE_LEFT, LOWER_CENTRE),
    (LOWER_LEFT, UPPER_MIDDLE_LEFT, LOWER_RIGHT),
    (LOWER_LEFT, UPPER_LEFT, LOWER_CENTRE),
    (LOWER_RIGHT, UPPER_MIDDLE_LEFT, UPPER_CENTRE),
    (LOWER_RIGHT, UPPER_MIDDLE_LEFT, UPPER_RIGHT),
    (LOWER_RIGHT, LOWER_MIDDLE_LEFT, UPPER_CENTRE),
    (LOWER_RIGHT, LOWER_MIDDLE_LEFT, UPPER_RIGHT),
    (LOWER_RIGHT, LOWER_LEFT, UPPER_CENTRE),
    (LOWER_RIGHT, LOWER_MIDDLE_LEFT, UPPER_MIDDLE_RIGHT),
    (LOWER_RIGHT, LOWER_CENTRE, LOWER_MIDDLE_RIGHT),
    (LOWER_RIGHT, LOWER_LEFT, LOWER_MIDDLE_RIGHT),
    (LOWER_RIGHT, LOWER_CENTRE, UPPER_MIDDLE_RIGHT),
    (LOWER_RIGHT, LOWER_LEFT, UPPER_MIDDLE_RIGHT),
    (LOWER_RIGHT, LOWER_CENTRE, UPPER_RIGHT),
    (LOWER_LEFT, UPPER_CENTRE, UPPER_MIDDLE_RIGHT),
    (LOWER_LEFT, UPPER_LEFT, UPPER_MIDDLE_RIGHT),
    (LOWER_LEFT, UPPER_CENTRE, LOWER_MIDDLE_RIGHT),
    (LOWER_LEFT, UPPER_LEFT, LOWER_MIDDLE_RIGHT),
    (LOWER_LEFT, UPPER_CENTRE, LOWER_RIGHT),
    (LOWER_LEFT, UPPER_MIDDLE_LEFT, LOWER_MIDDLE_RIGHT),
    (UPPER_RIGHT, LOWER_MIDDLE_LEFT, LOWER_CENTRE),
    (UPPER_RIGHT, LOWER_MIDDLE_LEFT, LOWER_RIGHT),
    (UPPER_RIGHT, UPPER_MIDDLE_LEFT, LOWER_CENTRE),
    (UPPER_RIGHT, UPPER_MIDDLE_LEFT, LOWER_RIGHT),
    (UPPER_RIGHT, UPPER_LEFT, LOWER_CENTRE),
    (UPPER_LEFT, UPPER_MIDDLE_LEFT, UPPER_CENTRE),
    (UPPER_LEFT, UPPER_MIDDLE_LEFT, UPPER_RIGHT),
    (UPPER_LEFT, LOWER_MIDDLE_LEFT, UPPER_CENTRE),
    (UPPER_LEFT, LOWER_MIDDLE_LEFT, UPPER_RIGHT),
    (UPPER_LEFT, LOWER_LEFT, UPPER_CENTRE),
    (UPPER_LEFT, LOWER_MIDDLE_LEFT, UPPER_MIDDLE_RIGHT),
    (UPPER_LEFT, LOWER_CENTRE, LOWER_MIDDLE_RIGHT),
    (UPPER_LEFT, LOWER_LEFT, LOWER_MIDDLE_RIGHT),
    (UPPER_LEFT, LOWER_CENTRE, UPPER_MIDDLE_RIGHT),
    (UPPER_LEFT, LOWER_LEFT, UPPER_MIDDLE_RIGHT),
    (UPPER_LEFT, LOWER_CENTRE, UPPER_RIGHT),
    (UPPER_RIGHT, UPPER_CENTRE, UPPER_MIDDLE_RIGHT),
    (UPPER_RIGHT, UPPER_LEFT, UPPER_MIDDLE_RIGHT),
    (UPPER_RIGHT, UPPER_CENTRE, LOWER_MIDDLE_RIGHT),
    (UPPER_RIGHT, UPPER_LEFT, LOWER_MIDDLE_RIGHT),
    (UPPER_RIGHT, UPPER_CENTRE, LOWER_RIGHT),
    (UPPER_RIGHT, UPPER_MIDDLE_LEFT, LOWER_MIDDLE_RIGHT),
];

// セルの辺。三角形の 1/4 のブロックで、どの辺に底辺を置くか。番号は `Cell::corners` の
// 角 `side` から角 `side + 1` への辺。
const UPPER: usize = 0;
const RIGHT: usize = 1;
const LOWER: usize = 2;
const LEFT: usize = 3;

/// 細線の斜線（U+1FBA0〜）の端。セルの縁の真ん中で、罫線の線の中心と同じ位置。
#[derive(Clone, Copy)]
enum Edge {
    Up,
    Right,
    Down,
    Left,
}

use Edge::{Down as DN, Left as LT, Right as RT, Up as UP};

/// 網かけの濃さ（U+2592 と同じ）。白黒を反転した網かけも、覆う割合は同じなのでこれで描く。
const MEDIUM_SHADE: f32 = 0.5;

impl Cell {
    pub(super) fn legacy(&self, c: char) -> Option<Vec<Shape>> {
        let (w, h) = (self.width, self.height);
        let (half_w, half_h) = ((w / 2.0).round(), (h / 2.0).round());
        let eighths = |from: f32, to: f32, vertical: bool| {
            if vertical {
                Self::rect(self.eighth_w(from), 0.0, self.eighth_w(to), h)
            } else {
                Self::rect(0.0, self.eighth_h(from), w, self.eighth_h(to))
            }
        };
        let code = c as u32;
        let shapes = match c {
            '\u{1fb00}'..='\u{1fb3b}' => self.sextant(code - 0x1fb00),
            '\u{1fb3c}'..='\u{1fb67}' => {
                let (corner, from, to) = WEDGES[(code - 0x1fb3c) as usize];
                vec![self.wedge(corner, from, to)]
            }
            '\u{1fb68}'..='\u{1fb6b}' => {
                // 名前に出てこない辺の三角形を欠いた、3/4 のブロック。
                const MISSING: [usize; 4] = [LEFT, UPPER, RIGHT, LOWER];
                vec![self.three_quarters(MISSING[(code - 0x1fb68) as usize])]
            }
            '\u{1fb6c}'..='\u{1fb6f}' => {
                const SIDES: [usize; 4] = [LEFT, UPPER, RIGHT, LOWER];
                vec![self.quarter(SIDES[(code - 0x1fb6c) as usize])]
            }
            '\u{1fb70}'..='\u{1fb75}' => {
                let column = (code - 0x1fb70 + 1) as f32;
                vec![eighths(column, column + 1.0, true)]
            }
            '\u{1fb76}'..='\u{1fb7b}' => {
                let row = (code - 0x1fb76 + 1) as f32;
                vec![eighths(row, row + 1.0, false)]
            }
            '\u{1fb7c}' => vec![eighths(0.0, 1.0, true), eighths(7.0, 8.0, false)],
            '\u{1fb7d}' => vec![eighths(0.0, 1.0, true), eighths(0.0, 1.0, false)],
            '\u{1fb7e}' => vec![eighths(7.0, 8.0, true), eighths(0.0, 1.0, false)],
            '\u{1fb7f}' => vec![eighths(7.0, 8.0, true), eighths(7.0, 8.0, false)],
            '\u{1fb80}' => vec![eighths(0.0, 1.0, false), eighths(7.0, 8.0, false)],
            '\u{1fb81}' => [0.0, 2.0, 4.0, 7.0]
                .iter()
                .map(|&row| eighths(row, row + 1.0, false))
                .collect(),
            '\u{1fb82}'..='\u{1fb86}' => {
                const UPPER_EIGHTHS: [f32; 5] = [2.0, 3.0, 5.0, 6.0, 7.0];
                vec![eighths(
                    0.0,
                    UPPER_EIGHTHS[(code - 0x1fb82) as usize],
                    false,
                )]
            }
            '\u{1fb87}'..='\u{1fb8b}' => {
                const RIGHT_EIGHTHS: [f32; 5] = [2.0, 3.0, 5.0, 6.0, 7.0];
                let n = RIGHT_EIGHTHS[(code - 0x1fb87) as usize];
                vec![eighths(8.0 - n, 8.0, true)]
            }
            '\u{1fb8c}' => vec![Self::shaded_rect(0.0, 0.0, half_w, h, MEDIUM_SHADE)],
            '\u{1fb8d}' => vec![Self::shaded_rect(half_w, 0.0, w, h, MEDIUM_SHADE)],
            '\u{1fb8e}' => vec![Self::shaded_rect(0.0, 0.0, w, half_h, MEDIUM_SHADE)],
            '\u{1fb8f}' => vec![Self::shaded_rect(0.0, half_h, w, h, MEDIUM_SHADE)],
            '\u{1fb90}' => vec![Self::shaded_rect(0.0, 0.0, w, h, MEDIUM_SHADE)],
            '\u{1fb91}' => vec![
                Self::rect(0.0, 0.0, w, half_h),
                Self::shaded_rect(0.0, half_h, w, h, MEDIUM_SHADE),
            ],
            '\u{1fb92}' => vec![
                Self::shaded_rect(0.0, 0.0, w, half_h, MEDIUM_SHADE),
                Self::rect(0.0, half_h, w, h),
            ],
            '\u{1fb94}' => vec![
                Self::shaded_rect(0.0, 0.0, half_w, h, MEDIUM_SHADE),
                Self::rect(half_w, 0.0, w, h),
            ],
            '\u{1fb9a}' => vec![self.quarter(UPPER), self.quarter(LOWER)],
            '\u{1fb9b}' => vec![self.quarter(LEFT), self.quarter(RIGHT)],
            '\u{1fb9c}'..='\u{1fb9f}' => {
                // 左上・右上・右下・左下の角を含む、対角線で半分にした三角形。
                let corners = self.corners();
                let corner = (code - 0x1fb9c) as usize;
                let points = (0..3).map(|i| corners[(corner + 3 + i) % 4]).collect();
                vec![Shape::Polygon {
                    points,
                    alpha: MEDIUM_SHADE,
                }]
            }
            '\u{1fba0}'..='\u{1fbae}' => self.light_diagonals(code - 0x1fba0),
            _ => return None,
        };
        Some(shapes)
    }

    /// 横の位置（左・真ん中・右）と縦の位置（上・1/3・2/3・下）。ピクセルの境目に揃える。
    fn grid(&self) -> ([f32; 3], [f32; 4]) {
        let (w, h) = (self.width, self.height);
        (
            [0.0, (w / 2.0).round(), w],
            [0.0, (h / 3.0).round(), (h * 2.0 / 3.0).round(), h],
        )
    }

    fn grid_point(&self, (x, y): Point) -> (f32, f32) {
        let (xs, ys) = self.grid();
        (xs[x], ys[y])
    }

    /// セクスタント（2 列 × 3 段）。`index` は U+1FB00 からの番号。
    fn sextant(&self, index: u32) -> Vec<Shape> {
        let (xs, ys) = self.grid();
        let bits = sextant_bits(index);
        (0..6)
            .filter(|bit| bits & (1 << bit) != 0)
            .map(|bit| {
                let (column, row) = (bit % 2, bit / 2);
                Self::rect(xs[column], ys[row], xs[column + 1], ys[row + 1])
            })
            .collect()
    }

    /// `from` から `to` への線でセルを分け、角 `corner` の側を塗る。
    fn wedge(&self, corner: Point, from: Point, to: Point) -> Shape {
        let a = self.grid_point(from);
        let b = self.grid_point(to);
        let side = |p: (f32, f32)| (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
        let keep = side(self.grid_point(corner)).signum();
        let corners = self.corners();
        let mut points = Vec::new();
        for (i, &p) in corners.iter().enumerate() {
            let q = corners[(i + 1) % 4];
            let (sp, sq) = (side(p) * keep, side(q) * keep);
            if sp >= 0.0 {
                points.push(p);
            }
            if sp * sq < 0.0 {
                let t = sp / (sp - sq);
                points.push((p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t));
            }
        }
        Self::polygon(points)
    }

    /// 左上から右回りの、セルの 4 つの角。
    fn corners(&self) -> [(f32, f32); 4] {
        let (w, h) = (self.width, self.height);
        [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]
    }

    fn center(&self) -> (f32, f32) {
        ((self.width / 2.0).round(), (self.height / 2.0).round())
    }

    /// 辺 `side` を底辺にし、セルの中心を頂点にした三角形。
    fn quarter(&self, side: usize) -> Shape {
        let corners = self.corners();
        Self::polygon(vec![corners[side], corners[(side + 1) % 4], self.center()])
    }

    /// セルから [`Cell::quarter`] の `side` の三角形を除いた形。
    fn three_quarters(&self, side: usize) -> Shape {
        let corners = self.corners();
        let mut points: Vec<_> = (1..=4).map(|i| corners[(side + i) % 4]).collect();
        points.push(self.center());
        Self::polygon(points)
    }

    /// 細線の斜線（U+1FBA0〜U+1FBAE）。`index` は U+1FBA0 からの番号。
    fn light_diagonals(&self, index: u32) -> Vec<Shape> {
        const SEGMENTS: [&[(Edge, Edge)]; 15] = [
            &[(UP, LT)],
            &[(UP, RT)],
            &[(LT, DN)],
            &[(RT, DN)],
            &[(UP, LT), (LT, DN)],
            &[(UP, RT), (RT, DN)],
            &[(LT, DN), (DN, RT)],
            &[(LT, UP), (UP, RT)],
            &[(UP, LT), (RT, DN)],
            &[(UP, RT), (LT, DN)],
            &[(UP, RT), (RT, DN), (DN, LT)],
            &[(UP, LT), (LT, DN), (DN, RT)],
            &[(LT, UP), (UP, RT), (RT, DN)],
            &[(RT, UP), (UP, LT), (LT, DN)],
            &[(UP, RT), (RT, DN), (DN, LT), (LT, UP)],
        ];
        let t = self.thickness;
        let cx = self.centered(self.width, t) + t / 2.0;
        let cy = self.centered(self.height, t) + t / 2.0;
        let point = |edge: Edge| match edge {
            Edge::Up => (cx, 0.0),
            Edge::Right => (self.width, cy),
            Edge::Down => (cx, self.height),
            Edge::Left => (0.0, cy),
        };
        SEGMENTS[index as usize]
            .iter()
            .map(|&(from, to)| self.stroke(vec![point(from), point(to)]))
            .collect()
    }
}

/// U+1FB00 からの番号を、セクスタントの点のビット（下位から左上・右上・左中・右中・左下・右下）に
/// する。どれも塗らない形、左半分（U+258C）、右半分（U+2590）、全部（U+2588）は飛ばして並んでいる。
fn sextant_bits(index: u32) -> u32 {
    const LEFT_HALF: u32 = 0b01_0101;
    const RIGHT_HALF: u32 = 0b10_1010;
    let mut bits = index + 1;
    if bits >= LEFT_HALF {
        bits += 1;
    }
    if bits >= RIGHT_HALF {
        bits += 1;
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sextants_skip_the_patterns_of_other_blocks() {
        let all: Vec<u32> = (0..60).map(sextant_bits).collect();
        assert_eq!(all[0], 0b00_0001);
        // U+1FB14 は SEXTANT-235、U+1FB28 は SEXTANT-1246、U+1FB3B は SEXTANT-23456。
        assert_eq!(all[0x14], 0b01_0110);
        assert_eq!(all[0x28], 0b10_1011);
        assert_eq!(all[59], 0b11_1110);
        assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!all.contains(&0b01_0101) && !all.contains(&0b10_1010));
    }
}
