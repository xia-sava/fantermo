//! 描画のために取り出した画面の内容。libghostty-vt の型に依存しない。

use std::borrow::Cow;

use crate::background::{Placement, Rect};

/// 8 ビットの RGB。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// 下線の種類（SGR 4 と 4:0〜4:5）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    /// 波線。
    Curly,
    Dotted,
    Dashed,
}

/// 文字・背景色・装飾のどれかを持つセル。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellView {
    pub col: u16,
    pub row: u16,
    /// セルの文字（書記素クラスタ）。空なら背景だけを描く。
    pub text: String,
    /// 占めるセルの数。全角の文字は 2。
    pub width: u8,
    pub fg: Rgb,
    /// 既定の背景色と違うときだけ持つ。
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: Underline,
    /// 下線の色（SGR 58）。`None` なら文字と同じ色で描く。
    pub underline_color: Option<Rgb>,
    pub strikethrough: bool,
    pub overline: bool,
    /// 点滅（SGR 5 と SGR 6）。
    pub blink: bool,
}

impl CellView {
    /// 下線・取り消し線・上線のどれかを持つ。
    pub fn has_decoration(&self) -> bool {
        self.underline != Underline::None || self.strikethrough || self.overline
    }

    /// 文字と装飾を除き、背景だけを残す。
    pub fn clear_text_and_decoration(&mut self) {
        self.text.clear();
        self.underline = Underline::None;
        self.strikethrough = false;
        self.overline = false;
    }
}

/// 画面に描くカーソル。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorView {
    pub col: u16,
    pub row: u16,
    /// カーソルが載っている文字の幅。
    pub width: u8,
    pub color: Rgb,
}

/// 端末の中の画像がどの仕組みで届いたか。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageSource {
    /// kitty graphics protocol。
    Kitty,
}

/// 端末の中の画像のピクセルを指すもの。ピクセルが変わると `generation` が変わるので、テクスチャを
/// 覚える鍵に使える。`generation` は端末や画面をまたいでも使い回されない。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageKey {
    pub source: ImageSource,
    pub id: u32,
    pub generation: u64,
}

/// 端末の中の画像を、文字に対してどの高さに描くか。下から順に描く。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImageLayer {
    /// セルの背景色の下。
    BelowBackground,
    /// セルの背景色の上で、文字の下。
    BelowText,
    /// 文字の上。
    AboveText,
}

/// 端末の中に描く画像の矩形一つ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageView {
    pub key: ImageKey,
    pub layer: ImageLayer,
    /// 同じ層の中で、小さいものから順に描く。
    pub z: i32,
    /// 左上のセルの桁と行。表示している範囲の左上を 0 とし、上や左へはみ出すと負になる。
    pub col: i32,
    pub row: i32,
    /// 左上のセルの中での位置（ピクセル）。
    pub offset_x: u32,
    pub offset_y: u32,
    /// 描く大きさ（ピクセル）。
    pub width: u32,
    pub height: u32,
    /// 描く画像の範囲（画像のピクセル）。
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
}

impl ImageView {
    /// 大きさ `image` の画像のこの矩形を、左上のセルが `grid` の左上に来る端末に描く位置。`grid` の外は
    /// 切り抜き、残りの画像の範囲を返す。何も見えなければ `None`。`cell` はセルの幅と高さ（ピクセル）。
    pub fn placement(&self, grid: Rect, cell: (u32, u32), image: (u32, u32)) -> Option<Placement> {
        if self.width == 0 || self.height == 0 || image.0 == 0 || image.1 == 0 {
            return None;
        }
        let left = grid.x + self.col as f32 * cell.0 as f32 + self.offset_x as f32;
        let top = grid.y + self.row as f32 * cell.1 as f32 + self.offset_y as f32;
        let (width, height) = (self.width as f32, self.height as f32);
        let visible_left = left.max(grid.x);
        let visible_top = top.max(grid.y);
        let visible_width = (left + width).min(grid.x + grid.width) - visible_left;
        let visible_height = (top + height).min(grid.y + grid.height) - visible_top;
        if visible_width <= 0.0 || visible_height <= 0.0 {
            return None;
        }
        let u_per_pixel = self.source_width as f32 / width / image.0 as f32;
        let v_per_pixel = self.source_height as f32 / height / image.1 as f32;
        Some(Placement {
            rect: Rect {
                x: visible_left,
                y: visible_top,
                width: visible_width,
                height: visible_height,
            },
            uv: Rect {
                x: self.source_x as f32 / image.0 as f32 + (visible_left - left) * u_per_pixel,
                y: self.source_y as f32 / image.1 as f32 + (visible_top - top) * v_per_pixel,
                width: visible_width * u_per_pixel,
                height: visible_height * v_per_pixel,
            },
            repeat: false,
        })
    }
}

/// 端末の中の画像のピクセル。8 ビットの RGBA（アルファは乗算していない）で、行の間は詰めてある。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImagePixels<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: Cow<'a, [u8]>,
}

impl ImagePixels<'_> {
    /// 乗算済みアルファの BGRA に並べ直す。
    pub fn to_premultiplied_bgra(&self) -> Vec<u8> {
        let premultiply = |channel: u8, alpha: u8| -> u8 {
            ((u32::from(channel) * u32::from(alpha) + u32::from(u8::MAX) / 2) / u32::from(u8::MAX))
                as u8
        };
        let mut bgra = Vec::with_capacity(self.rgba.len());
        for &[r, g, b, a] in self.rgba.as_chunks::<4>().0 {
            bgra.extend_from_slice(&[premultiply(b, a), premultiply(g, a), premultiply(r, a), a]);
        }
        bgra
    }
}

/// 1 フレーム分の画面。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub cols: u16,
    pub rows: u16,
    pub background: Rgb,
    pub foreground: Rgb,
    /// 文字・背景色・装飾のどれかを持つセルだけを並べる。全角の文字の右半分は含めない。
    pub cells: Vec<CellView>,
    /// 表示中で、画面の中にあるときだけ持つ。
    pub cursor: Option<CursorView>,
    /// 表示している範囲に少しでも入る画像。層ごとに下から描く順に並ぶ。
    pub images: Vec<ImageView>,
}

impl Frame {
    /// 点滅させると見た目が変わるセルを持つ。点滅の属性を持っていても、空白だけで装飾もないセルは数えない。
    pub fn has_blink(&self) -> bool {
        self.cells
            .iter()
            .any(|cell| cell.blink && (!cell.text.trim().is_empty() || cell.has_decoration()))
    }

    /// 点滅の文字を隠す。点滅するセルの文字と装飾を除き、背景だけを残す。
    pub fn hide_blinking_text(&mut self) {
        for cell in self.cells.iter_mut().filter(|cell| cell.blink) {
            cell.clear_text_and_decoration();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgb = Rgb {
        r: 255,
        g: 255,
        b: 255,
    };
    const BLUE: Rgb = Rgb { r: 0, g: 0, b: 255 };

    fn cell(col: u16, text: &str, blink: bool) -> CellView {
        CellView {
            col,
            row: 0,
            text: text.to_owned(),
            width: 1,
            fg: WHITE,
            bg: Some(BLUE),
            bold: false,
            italic: false,
            underline: Underline::Curly,
            underline_color: None,
            strikethrough: true,
            overline: true,
            blink,
        }
    }

    fn frame(cells: Vec<CellView>) -> Frame {
        Frame {
            cols: 10,
            rows: 1,
            background: Rgb { r: 0, g: 0, b: 0 },
            foreground: WHITE,
            cells,
            cursor: None,
            images: Vec::new(),
        }
    }

    /// 8×16 のセルで (1, 1) のセルから 40×32 ピクセルに描く、80×64 の画像の全体。
    fn image_view() -> ImageView {
        ImageView {
            key: ImageKey {
                source: ImageSource::Kitty,
                id: 1,
                generation: 1,
            },
            layer: ImageLayer::AboveText,
            z: 0,
            col: 1,
            row: 1,
            offset_x: 0,
            offset_y: 0,
            width: 40,
            height: 32,
            source_x: 0,
            source_y: 0,
            source_width: 80,
            source_height: 64,
        }
    }

    const GRID: Rect = Rect {
        x: 100.0,
        y: 200.0,
        width: 80.0,
        height: 80.0,
    };

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn an_image_inside_the_grid_is_placed_from_its_cell() {
        let placement = image_view().placement(GRID, (8, 16), (80, 64)).unwrap();
        assert_eq!(placement.rect, rect(108.0, 216.0, 40.0, 32.0));
        assert_eq!(placement.uv, rect(0.0, 0.0, 1.0, 1.0));
        assert!(!placement.repeat);
    }

    #[test]
    fn the_offset_in_the_cell_moves_the_image() {
        let view = ImageView {
            offset_x: 3,
            offset_y: 5,
            ..image_view()
        };
        let placement = view.placement(GRID, (8, 16), (80, 64)).unwrap();
        assert_eq!(placement.rect, rect(111.0, 221.0, 40.0, 32.0));
    }

    #[test]
    fn the_parts_outside_the_grid_are_cut_with_their_source() {
        // 上へ 1 行（16 ピクセル）はみ出す。
        let above = ImageView {
            row: -1,
            ..image_view()
        };
        let placement = above.placement(GRID, (8, 16), (80, 64)).unwrap();
        assert_eq!(placement.rect, rect(108.0, 200.0, 40.0, 16.0));
        assert_eq!(placement.uv, rect(0.0, 0.5, 1.0, 0.5));

        // 右へ 8 ピクセルはみ出す。
        let right = ImageView {
            col: 6,
            ..image_view()
        };
        let placement = right.placement(GRID, (8, 16), (80, 64)).unwrap();
        assert_eq!(placement.rect, rect(148.0, 216.0, 32.0, 32.0));
        assert_eq!(placement.uv, rect(0.0, 0.0, 0.8, 1.0));
    }

    #[test]
    fn a_cropped_image_keeps_its_source() {
        let view = ImageView {
            source_x: 20,
            source_y: 16,
            source_width: 40,
            source_height: 32,
            ..image_view()
        };
        let placement = view.placement(GRID, (8, 16), (80, 64)).unwrap();
        assert_eq!(placement.uv, rect(0.25, 0.25, 0.5, 0.5));
    }

    #[test]
    fn images_outside_the_grid_or_without_size_are_not_placed() {
        let above = ImageView {
            row: -2,
            ..image_view()
        };
        assert_eq!(above.placement(GRID, (8, 16), (80, 64)), None);
        let below = ImageView {
            row: 5,
            ..image_view()
        };
        assert_eq!(below.placement(GRID, (8, 16), (80, 64)), None);
        let empty = ImageView {
            width: 0,
            ..image_view()
        };
        assert_eq!(empty.placement(GRID, (8, 16), (80, 64)), None);
        assert_eq!(image_view().placement(GRID, (8, 16), (0, 64)), None);
    }

    #[test]
    fn pixels_are_premultiplied_and_reordered_to_bgra() {
        let pixels = ImagePixels {
            width: 2,
            height: 1,
            rgba: Cow::Borrowed(&[255, 128, 0, 255, 200, 100, 50, 128]),
        };
        assert_eq!(
            pixels.to_premultiplied_bgra(),
            [0, 128, 255, 255, 25, 50, 100, 128]
        );
    }

    #[test]
    fn hiding_blinking_text_keeps_the_background_and_other_cells() {
        let mut frame = frame(vec![cell(0, "a", true), cell(1, "b", false)]);
        assert!(frame.has_blink());
        frame.hide_blinking_text();
        let hidden = &frame.cells[0];
        assert_eq!(hidden.text, "");
        assert!(!hidden.has_decoration());
        assert_eq!(hidden.bg, Some(BLUE));
        assert_eq!(frame.cells[1], cell(1, "b", false));
    }

    #[test]
    fn frames_without_blinking_cells_have_no_blink() {
        assert!(!frame(vec![cell(0, "a", false)]).has_blink());
        assert!(!frame(Vec::new()).has_blink());
        // 空白だけで装飾もない点滅のセルは、隠しても見た目が変わらない。
        let mut space = CellView {
            underline: Underline::None,
            strikethrough: false,
            overline: false,
            ..cell(0, " ", true)
        };
        assert!(!frame(vec![space.clone()]).has_blink());
        space.underline = Underline::Single;
        assert!(frame(vec![space]).has_blink());
    }
}
