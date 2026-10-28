//! 罫線（U+2500〜U+257F）、ブロック要素（U+2580〜U+259F）、点字（U+2800〜U+28FF）、
//! Powerline の記号（U+E0B0〜U+E0BF）、レガシーコンピューティング記号（U+1FB00〜）の一部を、
//! フォントに頼らずに形として組み立てる。
//!
//! フォントのグリフは文字の幅や太さがセルと合わず、隣のセルとつながらないことがある。セルの寸法から
//! 線や矩形を作れば、どのフォントでも端から端までつながる。座標はセルの左上を原点とするピクセル。

mod braille;
mod legacy;
mod powerline;

/// セルの中に描く形。色は白で、`alpha` が覆う割合。
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// 塗りつぶす矩形。
    Rect {
        left: f32,
        top: f32,
        right: f32,
        bottom: f32,
        alpha: f32,
    },
    /// 太さ `width` の線分。
    Line {
        from: (f32, f32),
        to: (f32, f32),
        width: f32,
    },
    /// 太さ `width` で、`from` から `corner` の手前まで直線、四分円で曲がって `to` へ向かう角。
    RoundedCorner {
        from: (f32, f32),
        corner: (f32, f32),
        to: (f32, f32),
        radius: f32,
        width: f32,
    },
    /// 太さ `width` の波線。`left` から `right` まで、高さ `center_y` を中心に振れ幅 `amplitude` で、
    /// `period` ごとに同じ形を繰り返す。`left` では中心から上へ向かって始まる。
    Wave {
        left: f32,
        right: f32,
        center_y: f32,
        amplitude: f32,
        period: f32,
        width: f32,
    },
    /// `points` を順に結んで閉じた多角形を塗りつぶす。
    Polygon { points: Vec<(f32, f32)>, alpha: f32 },
    /// `points` を順に結んだ、太さ `width` の折れ線。閉じない。
    Polyline { points: Vec<(f32, f32)>, width: f32 },
}

/// 罫線の腕一本の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arm {
    None,
    Light,
    Heavy,
    Double,
}

use Arm::{Double as D, Heavy as H, Light as L, None as N};

/// 上・右・下・左の腕。
type Arms = [Arm; 4];

const UP: usize = 0;
const RIGHT: usize = 1;
const DOWN: usize = 2;
const LEFT: usize = 3;

/// `text` が形として描く文字の 1 文字なら、幅 `width`・高さ `height` のセルに描く形を返す。
/// `thickness` は細線の太さ。
pub fn shapes(text: &str, width: f32, height: f32, thickness: f32) -> Option<Vec<Shape>> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let cell = Cell {
        width,
        height,
        thickness: thickness.max(1.0),
    };
    if let Some(arms) = arms(c) {
        return Some(cell.lines(arms));
    }
    cell.special(c)
}

struct Cell {
    width: f32,
    height: f32,
    thickness: f32,
}

impl Cell {
    fn heavy(&self) -> f32 {
        self.thickness * 2.0
    }

    /// 二重線の 2 本の、中心からのずれ。2 本の間に細線 1 本分のすき間を空ける。
    fn double_offset(&self) -> f32 {
        self.thickness
    }

    /// 太さ `t` の線を中心に置いたときの左端（または上端）。ピクセルの境目に揃える。
    fn centered(&self, length: f32, t: f32) -> f32 {
        ((length - t) / 2.0).floor()
    }

    fn rect(left: f32, top: f32, right: f32, bottom: f32) -> Shape {
        Self::shaded_rect(left, top, right, bottom, 1.0)
    }

    fn shaded_rect(left: f32, top: f32, right: f32, bottom: f32, alpha: f32) -> Shape {
        Shape::Rect {
            left,
            top,
            right,
            bottom,
            alpha,
        }
    }

    fn polygon(points: Vec<(f32, f32)>) -> Shape {
        Shape::Polygon { points, alpha: 1.0 }
    }

    /// 細線の太さの折れ線。両端は線の向きに太さの分だけ延ばし、セルの縁で切れたときに欠けを作らない。
    fn stroke(&self, mut points: Vec<(f32, f32)>) -> Shape {
        let t = self.thickness;
        if let [first, second, ..] = points[..] {
            points[0] = extend(second, first, t);
        }
        if let [.., before_last, last] = points[..] {
            let end = points.len() - 1;
            points[end] = extend(before_last, last, t);
        }
        Shape::Polyline {
            points,
            width: self.thickness,
        }
    }

    /// 横の 8 分の `n` の位置。
    fn eighth_w(&self, n: f32) -> f32 {
        (self.width * n / 8.0).round()
    }

    /// 縦の 8 分の `n` の位置。
    fn eighth_h(&self, n: f32) -> f32 {
        (self.height * n / 8.0).round()
    }

    /// 腕の組み合わせから、矩形を組み立てる。
    fn lines(&self, arms: Arms) -> Vec<Shape> {
        let mut shapes = Vec::new();
        let (w, h) = (self.width, self.height);
        let cx = self.centered(w, 0.0);
        let cy = self.centered(h, 0.0);
        let g = self.double_offset();
        let vertical_double = arms[UP] == D || arms[DOWN] == D;
        let horizontal_double = arms[LEFT] == D || arms[RIGHT] == D;

        for (index, arm) in arms.iter().enumerate() {
            let horizontal = index == LEFT || index == RIGHT;
            match arm {
                N => {}
                L | H => {
                    let t = if *arm == H {
                        self.heavy()
                    } else {
                        self.thickness
                    };
                    // 交わる腕の線の太さ分だけ中心を越えて伸ばし、継ぎ目にすき間を作らない。
                    let reach = if horizontal {
                        self.crossing_reach(arms[UP], arms[DOWN], vertical_double)
                    } else {
                        self.crossing_reach(arms[LEFT], arms[RIGHT], horizontal_double)
                    };
                    shapes.push(self.single_arm(index, t, reach));
                }
                D => shapes.extend(self.double_arm(index, arms, cx, cy, g)),
            }
        }
        shapes
    }

    /// 交わる側の腕のうち、いちばん外まで届く線の端の、中心からの距離。
    fn crossing_reach(&self, a: Arm, b: Arm, double: bool) -> f32 {
        if double {
            return self.double_offset() + self.thickness / 2.0;
        }
        let widest = [a, b]
            .iter()
            .map(|arm| match arm {
                H => self.heavy(),
                L => self.thickness,
                _ => 0.0,
            })
            .fold(0.0, f32::max);
        widest / 2.0
    }

    /// 細線か太線の腕一本。中心から `reach` だけ反対側へはみ出させる。
    fn single_arm(&self, index: usize, t: f32, reach: f32) -> Shape {
        let (w, h) = (self.width, self.height);
        let (cx, cy) = (w / 2.0, h / 2.0);
        let x = self.centered(w, t);
        let y = self.centered(h, t);
        let reach = reach.max(t / 2.0);
        match index {
            UP => Self::rect(x, 0.0, x + t, (cy + reach).ceil()),
            DOWN => Self::rect(x, (cy - reach).floor(), x + t, h),
            LEFT => Self::rect(0.0, y, (cx + reach).ceil(), y + t),
            _ => Self::rect((cx - reach).floor(), y, w, y + t),
        }
    }

    /// 二重線の腕一本（2 本の線）。線の端は、隣の腕が二重線かどうかで決める。
    fn double_arm(&self, index: usize, arms: Arms, cx: f32, cy: f32, g: f32) -> [Shape; 2] {
        let t = self.thickness;
        let (w, h) = (self.width, self.height);
        // 2 本の線の左端（上端）。
        let near = |center: f32| (center - g - t / 2.0).round();
        let far = |center: f32| (center + g - t / 2.0).round();
        // 腕の線の、中心側の端。`outer` は外側の線、`inner` は内側の線（腕の向きから見て）。
        let end = |side_a: Arm, side_b: Arm, center: f32, toward_a: bool| -> f32 {
            // side_a 側の線の端: side_a が二重なら内側で止め、side_b だけ二重なら外側の角まで、
            // どちらも二重でなければ中心まで。
            if side_a == D {
                if toward_a { center + g } else { center - g }
            } else if side_b == D {
                if toward_a { center - g } else { center + g }
            } else {
                center
            }
        };
        // 線の端は、交わる線の太さの半分だけ延ばし、角に欠けも出っ張りも作らない。
        let start = |end: f32| (end - t / 2.0).floor();
        let stop = |end: f32| (end + t / 2.0).ceil();
        match index {
            RIGHT | LEFT => {
                let top = near(cy);
                let bottom = far(cy);
                // 上の線は上の腕、下の線は下の腕の影響を受ける。
                let top_end = end(arms[UP], arms[DOWN], cx, index == RIGHT);
                let bottom_end = end(arms[DOWN], arms[UP], cx, index == RIGHT);
                if index == RIGHT {
                    [
                        Self::rect(start(top_end), top, w, top + t),
                        Self::rect(start(bottom_end), bottom, w, bottom + t),
                    ]
                } else {
                    [
                        Self::rect(0.0, top, stop(top_end), top + t),
                        Self::rect(0.0, bottom, stop(bottom_end), bottom + t),
                    ]
                }
            }
            _ => {
                let left = near(cx);
                let right = far(cx);
                let left_end = end(arms[LEFT], arms[RIGHT], cy, index == DOWN);
                let right_end = end(arms[RIGHT], arms[LEFT], cy, index == DOWN);
                if index == DOWN {
                    [
                        Self::rect(left, start(left_end), left + t, h),
                        Self::rect(right, start(right_end), right + t, h),
                    ]
                } else {
                    [
                        Self::rect(left, 0.0, left + t, stop(left_end)),
                        Self::rect(right, 0.0, right + t, stop(right_end)),
                    ]
                }
            }
        }
    }

    /// 腕の表で表せない文字（破線、角丸、斜線、ブロック要素、網かけ、点字、Powerline の記号、
    /// レガシーコンピューティング記号）。
    fn special(&self, c: char) -> Option<Vec<Shape>> {
        let (w, h) = (self.width, self.height);
        let t = self.thickness;
        let shapes = match c {
            '┄' => self.dashes(true, 3, t),
            '┅' => self.dashes(true, 3, self.heavy()),
            '┆' => self.dashes(false, 3, t),
            '┇' => self.dashes(false, 3, self.heavy()),
            '┈' => self.dashes(true, 4, t),
            '┉' => self.dashes(true, 4, self.heavy()),
            '┊' => self.dashes(false, 4, t),
            '┋' => self.dashes(false, 4, self.heavy()),
            '╌' => self.dashes(true, 2, t),
            '╍' => self.dashes(true, 2, self.heavy()),
            '╎' => self.dashes(false, 2, t),
            '╏' => self.dashes(false, 2, self.heavy()),
            '╭' => vec![self.rounded((w, h / 2.0), (0.0, h))],
            '╮' => vec![self.rounded((0.0, h / 2.0), (0.0, h))],
            '╯' => vec![self.rounded((0.0, h / 2.0), (0.0, 0.0))],
            '╰' => vec![self.rounded((w, h / 2.0), (0.0, 0.0))],
            '╱' => vec![self.diagonal((w, 0.0), (0.0, h))],
            '╲' => vec![self.diagonal((0.0, 0.0), (w, h))],
            '╳' => vec![
                self.diagonal((w, 0.0), (0.0, h)),
                self.diagonal((0.0, 0.0), (w, h)),
            ],
            '\u{2580}'..='\u{259f}' => self.block(c)?,
            '\u{2800}'..='\u{28ff}' => self.braille(c),
            '\u{e0b0}'..='\u{e0bf}' => self.powerline(c)?,
            '\u{1fb00}'..='\u{1fbaf}' => self.legacy(c)?,
            _ => return None,
        };
        Some(shapes)
    }

    /// 中心を通る破線。`count` 本の線を、それぞれの区間の 6 割の長さで並べる。
    fn dashes(&self, horizontal: bool, count: u32, t: f32) -> Vec<Shape> {
        const DASH_RATIO: f32 = 0.6;
        let length = if horizontal { self.width } else { self.height };
        let across = if horizontal { self.height } else { self.width };
        let offset = self.centered(across, t);
        let segment = length / count as f32;
        (0..count)
            .map(|i| {
                let start = (i as f32 * segment + segment * (1.0 - DASH_RATIO) / 2.0).round();
                let end = (start + segment * DASH_RATIO).round().max(start + 1.0);
                if horizontal {
                    Self::rect(start, offset, end, offset + t)
                } else {
                    Self::rect(offset, start, offset + t, end)
                }
            })
            .collect()
    }

    /// 横の辺の端 `edge` から入り、中心で曲がって縦の辺 `vertical_edge` へ抜ける角丸。
    /// `vertical_edge` は x を使わず、y（0 か高さ）だけを使う。
    fn rounded(&self, edge: (f32, f32), vertical_edge: (f32, f32)) -> Shape {
        let t = self.thickness;
        let cx = self.centered(self.width, t) + t / 2.0;
        let cy = self.centered(self.height, t) + t / 2.0;
        let radius = (self.width.min(self.height) / 2.0).floor().max(t);
        Shape::RoundedCorner {
            from: (edge.0, cy),
            corner: (cx, cy),
            to: (cx, vertical_edge.1),
            radius,
            width: t,
        }
    }

    fn diagonal(&self, from: (f32, f32), to: (f32, f32)) -> Shape {
        Shape::Line {
            from,
            to,
            width: self.thickness,
        }
    }

    /// ブロック要素（U+2580〜U+259F）。
    fn block(&self, c: char) -> Option<Vec<Shape>> {
        let (w, h) = (self.width, self.height);
        let eighth_h = |n: f32| self.eighth_h(n);
        let eighth_w = |n: f32| self.eighth_w(n);
        let (half_w, half_h) = ((w / 2.0).round(), (h / 2.0).round());
        let quadrant = |left: bool, top: bool| {
            let (x0, x1) = if left { (0.0, half_w) } else { (half_w, w) };
            let (y0, y1) = if top { (0.0, half_h) } else { (half_h, h) };
            Self::rect(x0, y0, x1, y1)
        };
        let shade = |alpha: f32| Self::shaded_rect(0.0, 0.0, w, h, alpha);
        let shapes = match c {
            '▀' => vec![Self::rect(0.0, 0.0, w, half_h)],
            '▁'..='▇' => {
                let eighths = f32::from(c as u8 - 0x80);
                vec![Self::rect(0.0, h - eighth_h(eighths), w, h)]
            }
            '█' => vec![Self::rect(0.0, 0.0, w, h)],
            '▉'..='▏' => {
                // U+2589 が 7/8、U+258F が 1/8。
                let eighths = f32::from(0x90 - (c as u32 & 0xff) as u8);
                vec![Self::rect(0.0, 0.0, eighth_w(eighths), h)]
            }
            '▐' => vec![Self::rect(half_w, 0.0, w, h)],
            '░' => vec![shade(0.25)],
            '▒' => vec![shade(0.5)],
            '▓' => vec![shade(0.75)],
            '▔' => vec![Self::rect(0.0, 0.0, w, eighth_h(1.0))],
            '▕' => vec![Self::rect(w - eighth_w(1.0), 0.0, w, h)],
            '▖' => vec![quadrant(true, false)],
            '▗' => vec![quadrant(false, false)],
            '▘' => vec![quadrant(true, true)],
            '▙' => vec![
                quadrant(true, true),
                quadrant(true, false),
                quadrant(false, false),
            ],
            '▚' => vec![quadrant(true, true), quadrant(false, false)],
            '▛' => vec![
                quadrant(true, true),
                quadrant(false, true),
                quadrant(true, false),
            ],
            '▜' => vec![
                quadrant(true, true),
                quadrant(false, true),
                quadrant(false, false),
            ],
            '▝' => vec![quadrant(false, true)],
            '▞' => vec![quadrant(false, true), quadrant(true, false)],
            '▟' => vec![
                quadrant(false, true),
                quadrant(true, false),
                quadrant(false, false),
            ],
            _ => return None,
        };
        Some(shapes)
    }
}

/// `from` から `to` へ向かう線を、`to` の先へ `by` だけ延ばした端。
fn extend(from: (f32, f32), to: (f32, f32), by: f32) -> (f32, f32) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt();
    if length == 0.0 {
        return to;
    }
    (to.0 + dx / length * by, to.1 + dy / length * by)
}

/// 腕の表で表せる罫線の文字の、上・右・下・左の腕。
fn arms(c: char) -> Option<Arms> {
    let arms = match c {
        '─' => [N, L, N, L],
        '━' => [N, H, N, H],
        '│' => [L, N, L, N],
        '┃' => [H, N, H, N],
        '┌' => [N, L, L, N],
        '┍' => [N, H, L, N],
        '┎' => [N, L, H, N],
        '┏' => [N, H, H, N],
        '┐' => [N, N, L, L],
        '┑' => [N, N, L, H],
        '┒' => [N, N, H, L],
        '┓' => [N, N, H, H],
        '└' => [L, L, N, N],
        '┕' => [L, H, N, N],
        '┖' => [H, L, N, N],
        '┗' => [H, H, N, N],
        '┘' => [L, N, N, L],
        '┙' => [L, N, N, H],
        '┚' => [H, N, N, L],
        '┛' => [H, N, N, H],
        '├' => [L, L, L, N],
        '┝' => [L, H, L, N],
        '┞' => [H, L, L, N],
        '┟' => [L, L, H, N],
        '┠' => [H, L, H, N],
        '┡' => [H, H, L, N],
        '┢' => [L, H, H, N],
        '┣' => [H, H, H, N],
        '┤' => [L, N, L, L],
        '┥' => [L, N, L, H],
        '┦' => [H, N, L, L],
        '┧' => [L, N, H, L],
        '┨' => [H, N, H, L],
        '┩' => [H, N, L, H],
        '┪' => [L, N, H, H],
        '┫' => [H, N, H, H],
        '┬' => [N, L, L, L],
        '┭' => [N, L, L, H],
        '┮' => [N, H, L, L],
        '┯' => [N, H, L, H],
        '┰' => [N, L, H, L],
        '┱' => [N, L, H, H],
        '┲' => [N, H, H, L],
        '┳' => [N, H, H, H],
        '┴' => [L, L, N, L],
        '┵' => [L, L, N, H],
        '┶' => [L, H, N, L],
        '┷' => [L, H, N, H],
        '┸' => [H, L, N, L],
        '┹' => [H, L, N, H],
        '┺' => [H, H, N, L],
        '┻' => [H, H, N, H],
        '┼' => [L, L, L, L],
        '┽' => [L, L, L, H],
        '┾' => [L, H, L, L],
        '┿' => [L, H, L, H],
        '╀' => [H, L, L, L],
        '╁' => [L, L, H, L],
        '╂' => [H, L, H, L],
        '╃' => [H, L, L, H],
        '╄' => [H, H, L, L],
        '╅' => [L, L, H, H],
        '╆' => [L, H, H, L],
        '╇' => [H, H, L, H],
        '╈' => [L, H, H, H],
        '╉' => [H, L, H, H],
        '╊' => [H, H, H, L],
        '╋' => [H, H, H, H],
        '═' => [N, D, N, D],
        '║' => [D, N, D, N],
        '╒' => [N, D, L, N],
        '╓' => [N, L, D, N],
        '╔' => [N, D, D, N],
        '╕' => [N, N, L, D],
        '╖' => [N, N, D, L],
        '╗' => [N, N, D, D],
        '╘' => [L, D, N, N],
        '╙' => [D, L, N, N],
        '╚' => [D, D, N, N],
        '╛' => [L, N, N, D],
        '╜' => [D, N, N, L],
        '╝' => [D, N, N, D],
        '╞' => [L, D, L, N],
        '╟' => [D, L, D, N],
        '╠' => [D, D, D, N],
        '╡' => [L, N, L, D],
        '╢' => [D, N, D, L],
        '╣' => [D, N, D, D],
        '╤' => [N, D, L, D],
        '╥' => [N, L, D, L],
        '╦' => [N, D, D, D],
        '╧' => [L, D, N, D],
        '╨' => [D, L, N, L],
        '╩' => [D, D, N, D],
        '╪' => [L, D, L, D],
        '╫' => [D, L, D, L],
        '╬' => [D, D, D, D],
        '╴' => [N, N, N, L],
        '╵' => [L, N, N, N],
        '╶' => [N, L, N, N],
        '╷' => [N, N, L, N],
        '╸' => [N, N, N, H],
        '╹' => [H, N, N, N],
        '╺' => [N, H, N, N],
        '╻' => [N, N, H, N],
        '╼' => [N, H, N, L],
        '╽' => [L, N, H, N],
        '╾' => [N, L, N, H],
        '╿' => [H, N, L, N],
        _ => return None,
    };
    Some(arms)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 16.0;
    const H: f32 = 32.0;
    const T: f32 = 2.0;

    /// 形の矩形と多角形をすべて塗ったときに、(x, y) の点が塗られるか。
    fn covered(shapes: &[Shape], x: f32, y: f32) -> bool {
        shapes.iter().any(|shape| match *shape {
            Shape::Rect {
                left,
                top,
                right,
                bottom,
                ..
            } => left <= x && x < right && top <= y && y < bottom,
            Shape::Polygon { ref points, .. } => inside(points, x, y),
            _ => false,
        })
    }

    /// (x, y) が多角形の内側にあるか（右へ伸ばした半直線が辺と交わる回数で決める）。
    fn inside(points: &[(f32, f32)], x: f32, y: f32) -> bool {
        let mut inside = false;
        for (i, &(x0, y0)) in points.iter().enumerate() {
            let (x1, y1) = points[(i + 1) % points.len()];
            if (y0 > y) != (y1 > y) && x < x0 + (y - y0) * (x1 - x0) / (y1 - y0) {
                inside = !inside;
            }
        }
        inside
    }

    /// 多角形の面積。
    fn area(shape: &Shape) -> f32 {
        let Shape::Polygon { points, .. } = shape else {
            panic!("not a polygon: {shape:?}");
        };
        let twice: f32 = (0..points.len())
            .map(|i| {
                let (x0, y0) = points[i];
                let (x1, y1) = points[(i + 1) % points.len()];
                x0 * y1 - x1 * y0
            })
            .sum();
        twice.abs() / 2.0
    }

    fn polyline(shape: &Shape) -> &[(f32, f32)] {
        let Shape::Polyline { points, .. } = shape else {
            panic!("not a polyline: {shape:?}");
        };
        points
    }

    fn character(code: u32) -> char {
        char::from_u32(code).unwrap()
    }

    fn draw(c: char) -> Vec<Shape> {
        shapes(&c.to_string(), W, H, T).unwrap()
    }

    #[test]
    fn horizontal_line_reaches_both_edges() {
        let shapes = draw('─');
        assert!(covered(&shapes, 0.0, H / 2.0));
        assert!(covered(&shapes, W - 1.0, H / 2.0));
        assert!(!covered(&shapes, W / 2.0, 0.0));
    }

    #[test]
    fn vertical_line_reaches_both_edges() {
        let shapes = draw('│');
        assert!(covered(&shapes, W / 2.0, 0.0));
        assert!(covered(&shapes, W / 2.0, H - 1.0));
    }

    #[test]
    fn corner_joins_without_a_gap() {
        let shapes = draw('┌');
        assert!(covered(&shapes, W - 1.0, H / 2.0));
        assert!(covered(&shapes, W / 2.0, H - 1.0));
        // 曲がり角のピクセル。
        assert!(covered(&shapes, W / 2.0 - 1.0, H / 2.0 - 1.0));
        assert!(!covered(&shapes, 0.0, H / 2.0));
        assert!(!covered(&shapes, W / 2.0, 0.0));
    }

    #[test]
    fn double_corner_leaves_the_inside_open() {
        let shapes = draw('╔');
        // 外側の 2 本は角でつながり、内側のすき間（中心）は塗らない。
        assert!(covered(&shapes, W - 1.0, H / 2.0 - T));
        assert!(covered(&shapes, W - 1.0, H / 2.0 + T));
        assert!(!covered(&shapes, W / 2.0, H / 2.0));
    }

    #[test]
    fn double_cross_keeps_every_gap() {
        let shapes = draw('╬');
        assert!(!covered(&shapes, W / 2.0, H / 2.0));
        assert!(!covered(&shapes, W - 1.0, H / 2.0));
        assert!(!covered(&shapes, W / 2.0, 0.0));
    }

    #[test]
    fn blocks_fill_their_part_of_the_cell() {
        assert!(covered(&draw('█'), 0.0, 0.0));
        assert!(covered(&draw('█'), W - 1.0, H - 1.0));
        let lower_half = draw('▄');
        assert!(covered(&lower_half, 0.0, H - 1.0));
        assert!(!covered(&lower_half, 0.0, 0.0));
        let left_eighth = draw('▏');
        assert!(covered(&left_eighth, 0.0, 0.0));
        assert!(!covered(&left_eighth, W / 2.0, 0.0));
    }

    #[test]
    fn other_characters_are_left_to_the_font() {
        assert!(shapes("a", W, H, T).is_none());
        assert!(shapes("─a", W, H, T).is_none());
        assert!(shapes("○", W, H, T).is_none());
        // Powerline の絵、Nerd Font のアイコン、レガシーコンピューティング記号の模様と絵。
        for code in [
            0xe0a0, 0xe0a1, 0xe0a2, 0xe0a3, 0xe0c0, 0xe0d2, 0xf015, 0x1fb93, 0x1fb95, 0x1fb99,
            0x1fbaf, 0x1fbb0, 0x1fbf0,
        ] {
            let c = character(code);
            assert!(shapes(&c.to_string(), W, H, T).is_none(), "U+{code:X}");
        }
    }

    #[test]
    fn every_braille_and_powerline_character_is_drawn() {
        for code in (0x2800..=0x28ff).chain(0xe0b0..=0xe0bf) {
            let c = character(code);
            assert!(shapes(&c.to_string(), W, H, T).is_some(), "U+{code:X}");
        }
    }

    #[test]
    fn braille_dots_follow_the_bit_order() {
        assert!(draw('\u{2800}').is_empty());
        // 点 1 は左上、点 4 は右上、点 7 は左下、点 8 は右下。
        let dot = |c: char| match draw(c)[..] {
            [
                Shape::Rect {
                    left, top, right, ..
                },
            ] => (left, top, right - left),
            _ => panic!("not one dot"),
        };
        assert_eq!(dot('⠁'), (2.0, 2.0, 4.0));
        assert_eq!(dot('⠈'), (10.0, 2.0, 4.0));
        assert_eq!(dot('\u{2840}'), (2.0, 26.0, 4.0));
        assert_eq!(dot('\u{2880}'), (10.0, 26.0, 4.0));
        assert_eq!(draw('\u{28ff}').len(), 8);
    }

    #[test]
    fn braille_dots_keep_the_same_gap_across_cells() {
        // 隣のセルの点との間（左右の余白の和）が、セルの中の点の間と同じ。
        let dots = draw('\u{28ff}');
        let rect = |index: usize| match dots[index] {
            Shape::Rect {
                left,
                top,
                right,
                bottom,
                ..
            } => (left, top, right, bottom),
            _ => panic!("not a dot"),
        };
        // 点 1（左上）、点 4（右上）、点 2（左の 2 段目）、点 7（左下）。
        let (first, right_column, second_row, last_row) = (rect(0), rect(3), rect(1), rect(6));
        assert_eq!(right_column.0 - first.2, first.0 + W - right_column.2);
        assert_eq!(second_row.1 - first.3, first.1 + H - last_row.3);
    }

    #[test]
    fn powerline_triangles_cover_the_flat_edge() {
        // 縁の上下の端の近くまで塗り、反対の縁は真ん中だけに届く。
        let right = draw('\u{e0b0}');
        assert!(covered(&right, 0.5, 2.0));
        assert!(covered(&right, 0.5, H - 2.0));
        assert!(covered(&right, W - 0.5, H / 2.0));
        assert!(!covered(&right, W - 0.5, 2.0));
        let left = draw('\u{e0b2}');
        assert!(covered(&left, W - 0.5, 2.0));
        assert!(covered(&left, W - 0.5, H - 2.0));
        assert!(covered(&left, 0.5, H / 2.0));
        assert!(!covered(&left, 0.5, 2.0));
        assert!(covered(&draw('\u{e0b8}'), 0.5, H - 0.5));
        assert!(!covered(&draw('\u{e0b8}'), W - 0.5, 0.5));
        assert!(covered(&draw('\u{e0ba}'), W - 0.5, H - 0.5));
        assert!(covered(&draw('\u{e0bc}'), 0.5, 0.5));
        assert!(covered(&draw('\u{e0be}'), W - 0.5, 0.5));
        assert!(!covered(&draw('\u{e0be}'), 0.5, H - 0.5));
    }

    #[test]
    fn powerline_half_circles_bulge_away_from_the_flat_edge() {
        let right = draw('\u{e0b4}');
        assert!(covered(&right, 0.5, 0.5));
        assert!(covered(&right, 0.5, H - 0.5));
        assert!(covered(&right, W - 0.5, H / 2.0));
        assert!(!covered(&right, W - 0.5, 1.0));
        let left = draw('\u{e0b6}');
        assert!(covered(&left, W - 0.5, 0.5));
        assert!(covered(&left, 0.5, H / 2.0));
        assert!(!covered(&left, 0.5, 1.0));
        // 縁取りは内側に寄せ、両端はセルの縁より外へ延ばしてある。
        let outline = polyline(&draw('\u{e0b5}')[0]).to_vec();
        assert!(outline.first().unwrap().0 < 0.0 && outline.last().unwrap().0 < 0.0);
        assert!(outline.iter().all(|&(x, _)| x <= W - T / 2.0 + 0.01));
        let mirrored = polyline(&draw('\u{e0b7}')[0]).to_vec();
        assert!(mirrored.first().unwrap().0 > W);
    }

    #[test]
    fn powerline_chevron_points_to_the_middle_of_the_edge() {
        let points = polyline(&draw('\u{e0b1}')[0]).to_vec();
        assert_eq!(points[1], (W, H / 2.0));
        assert!(points[0].0 < 0.0 && points[2].0 < 0.0);
        assert_eq!(polyline(&draw('\u{e0b3}')[0])[1], (0.0, H / 2.0));
    }

    #[test]
    fn sextants_fill_their_thirds() {
        // U+1FB00 は左上、U+1FB3B（SEXTANT-23456）は左上だけ空く。
        let upper_left = draw('\u{1fb00}');
        assert!(covered(&upper_left, 0.0, 0.0));
        assert!(!covered(&upper_left, W / 2.0, 0.0));
        assert!(!covered(&upper_left, 0.0, (H / 3.0).round()));
        let all_but_one = draw('\u{1fb3b}');
        assert!(!covered(&all_but_one, 0.0, 0.0));
        assert!(covered(&all_but_one, W - 1.0, 0.0));
        assert!(covered(&all_but_one, 0.0, H - 1.0));
    }

    #[test]
    fn diagonal_blocks_and_their_complements_fill_the_cell() {
        // U+1FB3C〜U+1FB51 と U+1FB52〜U+1FB67 は、同じ線で分けた反対側どうし。
        for offset in 0..22 {
            let one = &draw(character(0x1fb3c + offset))[0];
            let other = &draw(character(0x1fb52 + offset))[0];
            let total = area(one) + area(other);
            assert!((total - W * H).abs() < 0.01, "U+{:X}", 0x1fb3c + offset);
            assert!(area(one) > 0.0 && area(other) > 0.0);
        }
        // LOWER LEFT BLOCK DIAGONAL LOWER MIDDLE LEFT TO LOWER CENTRE: 左下の小さな三角形。
        let small = draw('\u{1fb3c}');
        assert!(covered(&small, 0.5, H - 0.5));
        assert!(!covered(&small, W - 0.5, H - 0.5));
        assert!(!covered(&small, 0.5, H / 2.0));
    }

    #[test]
    fn triangular_blocks_meet_at_the_center() {
        let left = draw('\u{1fb6c}');
        assert!(covered(&left, 0.5, H / 2.0));
        assert!(!covered(&left, W - 0.5, H / 2.0));
        assert_eq!(area(&left[0]), W * H / 4.0);
        // 3/4 のブロックは左の三角形を欠く。
        let three = draw('\u{1fb68}');
        assert_eq!(area(&three[0]), W * H * 3.0 / 4.0);
        assert!(!covered(&three, 0.5, H / 2.0));
        assert!(covered(&three, W - 0.5, H / 2.0));
        let hourglass = draw('\u{1fb9a}');
        assert!(covered(&hourglass, W / 2.0, 0.5));
        assert!(covered(&hourglass, W / 2.0, H - 0.5));
        assert!(!covered(&hourglass, 0.5, H / 2.0));
    }

    #[test]
    fn eighth_blocks_take_their_column_or_row() {
        // VERTICAL ONE EIGHTH BLOCK-2 は 2 列目（16 ピクセルの幅で 2〜4）。
        let column = draw('\u{1fb70}');
        assert!(covered(&column, 2.0, 0.0) && covered(&column, 3.0, H - 1.0));
        assert!(!covered(&column, 1.0, 0.0) && !covered(&column, 4.0, 0.0));
        let row = draw('\u{1fb7b}');
        assert!(covered(&row, 0.0, 24.0) && !covered(&row, 0.0, 28.0));
        let corner = draw('\u{1fb7f}');
        assert!(covered(&corner, W - 1.0, 0.0) && covered(&corner, 0.0, H - 1.0));
        assert!(!covered(&corner, 0.0, 0.0));
        let upper = draw('\u{1fb82}');
        assert!(covered(&upper, 0.0, 7.0) && !covered(&upper, 0.0, 8.0));
        let right = draw('\u{1fb8b}');
        assert!(covered(&right, 2.0, 0.0) && !covered(&right, 1.0, 0.0));
    }

    #[test]
    fn light_diagonals_end_at_the_middle_of_the_edges() {
        let shapes = draw('\u{1fbae}');
        assert_eq!(shapes.len(), 4);
        let center = W / 2.0;
        // 端を延ばす前の線は、罫線の線の中心（上下の縁の真ん中、左右の縁の真ん中）を通る。
        for shape in &shapes {
            let points = polyline(shape);
            let (a, b) = (points[0], points[1]);
            let on_line =
                |x: f32, y: f32| ((b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0)).abs() < 0.01;
            let ends = [(center, 0.0), (W, H / 2.0), (center, H), (0.0, H / 2.0)];
            assert_eq!(ends.iter().filter(|&&(x, y)| on_line(x, y)).count(), 2);
        }
    }
}
