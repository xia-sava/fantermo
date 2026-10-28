//! 点字（U+2800〜U+28FF）。2 列 × 4 段の点。

use super::{Cell, Shape};

/// 符号の下位のビットから順に、点 1〜8 の（列、段）。
const DOTS: [(usize, usize); 8] = [
    (0, 0),
    (0, 1),
    (0, 2),
    (1, 0),
    (1, 1),
    (1, 2),
    (0, 3),
    (1, 3),
];

const COLUMNS: f32 = 2.0;
const ROWS: f32 = 4.0;

impl Cell {
    /// 点は正方形で、セルを 2 列 × 4 段に等分したそれぞれの真ん中に置く。区画の一辺の半分を点の
    /// 一辺にするので、点の間とセルの縁をはさんだ隣の点との間は、ピクセルへの丸めの差を除いて同じだけ空く。
    pub(super) fn braille(&self, c: char) -> Vec<Shape> {
        let column_width = self.width / COLUMNS;
        let row_height = self.height / ROWS;
        let dot = (column_width.min(row_height) / 2.0).floor().max(1.0);
        let start =
            |index: usize, pitch: f32| (pitch * (index as f32 + 0.5) - dot / 2.0).round().max(0.0);
        let bits = c as u32 - 0x2800;
        DOTS.iter()
            .enumerate()
            .filter(|(bit, _)| bits & (1 << bit) != 0)
            .map(|(_, &(column, row))| {
                let left = start(column, column_width);
                let top = start(row, row_height);
                Self::rect(left, top, left + dot, top + dot)
            })
            .collect()
    }
}
