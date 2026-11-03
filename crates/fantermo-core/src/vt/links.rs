//! 画面のセルにあるリンク（OSC 8 のハイパーリンクと、文字から見つけた URL）を探す。

use libghostty_vt::{
    screen::{CellWide, GridRef},
    terminal::{Point, PointCoordinate, PointSpace},
};

use super::{Vt, VtError};
use crate::link::{self, LineCell, Link};

/// リンクを探すときに、指した行の前後へ折り返しをたどる行数の上限。
pub(crate) const MAX_LINK_LINE_ROWS: u32 = 64;

/// セルの文字（書記素クラスタ）を読むときの、コードポイントの数の初めの見込み。
const GRAPHEME_BUFFER_LEN: usize = 16;

/// OSC 8 の URI を読むときの、バイト数の初めの見込み。
pub(super) const URI_BUFFER_LEN: usize = 256;

impl Vt {
    /// 表示している範囲の `col` 桁目・`row` 行目のセルにあるリンク。OSC 8 のハイパーリンクがあれば
    /// そちらを、なければ画面の文字から見つけた URL を返す。折り返した行はつなげて探す。
    pub fn link_at(&self, col: u16, row: u16) -> Result<Option<Link>, VtError> {
        let size = self.size.get();
        if col >= size.cols || row >= size.rows {
            return Ok(None);
        }
        let Some(viewport_top) =
            self.screen_row(Point::Viewport(PointCoordinate { x: 0, y: 0 }))?
        else {
            return Ok(None);
        };
        let screen_row = viewport_top + u32::from(row);
        let cells = self.logical_line(screen_row, size.cols)?;
        let Some(index) = cells.iter().position(|cell| {
            cell.screen_row == screen_row
                && (cell.col..cell.col + u16::from(cell.width)).contains(&col)
        }) else {
            return Ok(None);
        };
        let Some((uri, range)) = link::link_in_line(&cells, index) else {
            return Ok(None);
        };
        let spans = link::spans_of(&cells, range, viewport_top, size.rows);
        Ok(Some(Link { uri, spans }))
    }

    /// `point` のセルの、スクロールバックの先頭から数えた行。
    fn screen_row(&self, point: Point) -> Result<Option<u32>, VtError> {
        let grid_ref = self.terminal.grid_ref(point)?;
        Ok(self
            .terminal
            .point_from_grid_ref(&grid_ref, PointSpace::Screen)?
            .map(|point| point.y))
    }

    /// `screen_row` の行を含む論理行（折り返しでつながった行）のセルを、先頭から並べる。
    /// たどるのは前後それぞれ [`MAX_LINK_LINE_ROWS`] 行まで。
    fn logical_line(&self, screen_row: u32, cols: u16) -> Result<Vec<LineCell>, VtError> {
        let row_at = |y: u32| {
            self.terminal
                .grid_ref(Point::Screen(PointCoordinate { x: 0, y }))
                .and_then(|grid_ref| grid_ref.row())
        };
        let mut first = screen_row;
        while first > 0
            && screen_row - first < MAX_LINK_LINE_ROWS
            && row_at(first)?.is_wrap_continuation()?
        {
            first -= 1;
        }
        let mut cells = Vec::new();
        let mut y = first;
        loop {
            self.push_row_cells(y, cols, &mut cells)?;
            let wrapped = row_at(y)?.is_wrapped()?;
            if !wrapped || y.saturating_sub(screen_row) >= MAX_LINK_LINE_ROWS {
                break;
            }
            y += 1;
            // 最後の行が折り返しの印を持っていても、その先の行はない。
            if row_at(y).is_err() {
                break;
            }
        }
        Ok(cells)
    }

    /// スクロールバックの先頭から数えて `y` 行目のセルを `cells` に足す。全角の文字の右半分と、
    /// 行末に収まらなかった全角の文字の印は除く。
    fn push_row_cells(&self, y: u32, cols: u16, cells: &mut Vec<LineCell>) -> Result<(), VtError> {
        for x in 0..cols {
            let grid_ref = self
                .terminal
                .grid_ref(Point::Screen(PointCoordinate { x, y }))?;
            let cell = grid_ref.cell()?;
            let width = match cell.wide()? {
                CellWide::Wide => 2,
                CellWide::SpacerTail | CellWide::SpacerHead => continue,
                CellWide::Narrow => 1,
            };
            let uri = if cell.has_hyperlink()? {
                hyperlink_uri(&grid_ref)?
            } else {
                None
            };
            cells.push(LineCell {
                screen_row: y,
                col: x,
                width,
                text: graphemes(&grid_ref)?,
                uri,
            });
        }
        Ok(())
    }
}

/// `grid_ref` のセルの文字（書記素クラスタ）。空のセルは空の文字列。
fn graphemes(grid_ref: &GridRef) -> Result<String, VtError> {
    let mut buf = vec!['\0'; GRAPHEME_BUFFER_LEN];
    let len = match grid_ref.graphemes(&mut buf) {
        Err(VtError::OutOfSpace { required }) => {
            buf.resize(required, '\0');
            grid_ref.graphemes(&mut buf)?
        }
        result => result?,
    };
    Ok(buf[..len].iter().collect())
}

/// `grid_ref` のセルの、OSC 8 のハイパーリンクの URI。なければ `None`。UTF-8 でないバイトは
/// U+FFFD に置き換える。
fn hyperlink_uri(grid_ref: &GridRef) -> Result<Option<String>, VtError> {
    let mut buf = vec![0u8; URI_BUFFER_LEN];
    let len = match grid_ref.hyperlink_uri(&mut buf) {
        Err(VtError::OutOfSpace { required }) => {
            buf.resize(required, 0);
            grid_ref.hyperlink_uri(&mut buf)?
        }
        result => result?,
    };
    Ok((len > 0).then(|| String::from_utf8_lossy(&buf[..len]).into_owned()))
}
