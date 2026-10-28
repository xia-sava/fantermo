//! 罫線（U+2500〜U+257F）とブロック要素（U+2580〜U+259F）を、フォントに頼らずに形として組み立てる。
//!
//! フォントのグリフは文字の幅や太さがセルと合わず、隣のセルとつながらないことがある。セルの寸法から
//! 線や矩形を作れば、どのフォントでも端から端までつながる。座標はセルの左上を原点とするピクセル。

/// セルの中に描く形。色は白で、`alpha` が覆う割合。
#[derive(Clone, Copy, Debug, PartialEq)]
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

/// `text` が罫線かブロック要素の 1 文字なら、幅 `width`・高さ `height` のセルに描く形を返す。
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
        Shape::Rect {
            left,
            top,
            right,
            bottom,
            alpha: 1.0,
        }
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

    /// 腕の表で表せない文字（破線、角丸、斜線、ブロック要素、網かけ）。
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
        let eighth_h = |n: f32| (h * n / 8.0).round();
        let eighth_w = |n: f32| (w * n / 8.0).round();
        let (half_w, half_h) = ((w / 2.0).round(), (h / 2.0).round());
        let quadrant = |left: bool, top: bool| {
            let (x0, x1) = if left { (0.0, half_w) } else { (half_w, w) };
            let (y0, y1) = if top { (0.0, half_h) } else { (half_h, h) };
            Self::rect(x0, y0, x1, y1)
        };
        let shade = |alpha: f32| Shape::Rect {
            left: 0.0,
            top: 0.0,
            right: w,
            bottom: h,
            alpha,
        };
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

    /// 形の矩形をすべて塗ったときに、(x, y) のピクセルが塗られるか。
    fn covered(shapes: &[Shape], x: f32, y: f32) -> bool {
        shapes.iter().any(|shape| match *shape {
            Shape::Rect {
                left,
                top,
                right,
                bottom,
                ..
            } => left <= x && x < right && top <= y && y < bottom,
            _ => false,
        })
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
    }
}
