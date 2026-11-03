//! 組んだ文字の寸法と、グリフアトラスの場所への置き方。DirectWrite で測る部分以外は純粋な計算。

use fantermo_core::config::WideGlyph;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_TEXT_METRICS, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
};

/// [`WideGlyph::Overflow`] と [`WideGlyph::OverflowCenter`] で、文字に割り当てたセルの何倍の幅まで
/// 広げて描くか。
const MAX_OVERFLOW_FACTOR: u8 = 2;

/// インクがセルの左右へはみ出す分を描く余白の上限（セルの幅の何倍か）。
const MAX_INK_MARGIN_FACTOR: f32 = 1.0;

/// インクがセルの外へ出たとみなさない、計算の誤差の分（ピクセル）。
pub const INK_EPSILON_PX: f32 = 1.0 / 64.0;

/// 1 行に組んだ文字の送り幅と、インクの左右の端（ピクセル）。インクの端は文字の左端からの距離で、
/// 左へはみ出していれば `ink_left` が負、右へはみ出していれば `ink_right` が `advance` より大きい。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextExtent {
    pub advance: f32,
    pub ink_left: f32,
    pub ink_right: f32,
}

/// `text` を `format` で 1 行に組んだものと、その送り幅とインクの範囲。送り幅が 0 の文字
/// （結合文字だけのものなど）は 1 ピクセルとして扱う。
pub fn text_layout(
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
    height: f32,
) -> windows::core::Result<(IDWriteTextLayout, TextExtent)> {
    let text: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: text と format はこの呼び出しの間は生きている。
    let layout = unsafe { dwrite.CreateTextLayout(&text, format, f32::MAX, height)? };
    let mut metrics = DWRITE_TEXT_METRICS::default();
    // SAFETY: metrics は書き込める。
    unsafe { layout.GetMetrics(&mut metrics)? };
    let advance = metrics.widthIncludingTrailingWhitespace.max(1.0);
    // はみ出しは組む枠からの距離なので、枠の幅を送り幅に合わせてから測る。左寄せで折り返さない
    // ので、枠の幅を変えても文字の位置は変わらない。
    // SAFETY: layout は有効。
    let overhang = unsafe {
        layout.SetMaxWidth(advance)?;
        layout.GetOverhangMetrics()?
    };
    Ok((
        layout,
        TextExtent {
            advance,
            ink_left: -overhang.left,
            ink_right: advance + overhang.right,
        },
    ))
}

/// 組んだ文字をアトラスの場所に置く方法。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextPlacement {
    /// 文字に割り当てた場所の幅（セルの数）。左右の余白は含まない。
    pub cells: u8,
    /// 横の倍率。
    pub scale: f32,
    /// セルの左端から文字の左端までの距離（ピクセル）。
    pub offset: f32,
    /// インクが左のセルへはみ出す分の余白（ピクセル）。
    pub left_margin: u32,
    /// インクが右のセルへはみ出す分の余白（ピクセル）。
    pub right_margin: u32,
}

impl TextPlacement {
    /// 左右の余白を含めた場所の幅（ピクセル）。
    pub fn slot_width(&self, cell_width: u32) -> u32 {
        self.left_margin + cell_width * u32::from(self.cells) + self.right_margin
    }
}

/// `extent` の文字を、`cells` セルの文字として置く方法を決める。縮めるか・広げるかは送り幅で決め、
/// 左右の余白はインクの範囲で決める。
///
/// 送り幅が収まる文字はセルの真ん中に置く。収まらない文字は `wide_glyph` に従い、
/// [`WideGlyph::Overflow`] なら場所を右へ広げる。[`WideGlyph::OverflowCenter`] なら場所は
/// 広げずに文字をセルの真ん中に置き、左右へはみ出した分をインクのはみ出しと同じ余白で描く。
/// どちらも [`MAX_OVERFLOW_FACTOR`] 倍の幅までで、それでも収まらなければその幅に縮める。
/// 置いた結果、インクが場所の左右へはみ出せば、その分の余白を足す（セルの幅の
/// [`MAX_INK_MARGIN_FACTOR`] 倍まで）。[`WideGlyph::Clip`] で切った文字には余白を足さない。
pub fn place_text(
    extent: TextExtent,
    cells: u8,
    cell_width: f32,
    wide_glyph: WideGlyph,
) -> TextPlacement {
    let width = cell_width * f32::from(cells);
    let (cells, scale, offset) = if extent.advance <= width {
        (cells, 1.0, (width - extent.advance) / 2.0)
    } else {
        match wide_glyph {
            WideGlyph::Shrink => (cells, width / extent.advance, 0.0),
            WideGlyph::Clip => {
                return TextPlacement {
                    cells,
                    scale: 1.0,
                    offset: 0.0,
                    left_margin: 0,
                    right_margin: 0,
                };
            }
            WideGlyph::Overflow => {
                let needed = (extent.advance / cell_width).ceil();
                let limit = f32::from(cells.saturating_mul(MAX_OVERFLOW_FACTOR));
                let cells = needed.min(limit) as u8;
                let scale = (cell_width * f32::from(cells) / extent.advance).min(1.0);
                (cells, scale, 0.0)
            }
            WideGlyph::OverflowCenter => {
                let limit = width * f32::from(MAX_OVERFLOW_FACTOR);
                let scale = (limit / extent.advance).min(1.0);
                (cells, scale, (width - extent.advance * scale) / 2.0)
            }
        }
    };
    let width = cell_width * f32::from(cells);
    let ink_left = offset + extent.ink_left * scale;
    let ink_right = offset + extent.ink_right * scale;
    let margin = |outside: f32| {
        ((outside - INK_EPSILON_PX).ceil()).clamp(0.0, cell_width * MAX_INK_MARGIN_FACTOR) as u32
    };
    TextPlacement {
        cells,
        scale,
        offset,
        left_margin: margin(-ink_left),
        right_margin: margin(ink_right - width),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use windows::{
        Win32::Graphics::DirectWrite::{
            DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_WEIGHT,
            DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_GLYPH_METRICS,
            IDWriteFontFace,
        },
        core::{BOOL, w},
    };

    use crate::font;

    const CELL_WIDTH: f32 = 16.0;

    fn placement(cells: u8, scale: f32, offset: f32) -> TextPlacement {
        TextPlacement {
            cells,
            scale,
            offset,
            left_margin: 0,
            right_margin: 0,
        }
    }

    /// インクが送り幅にちょうど収まる文字。
    fn upright(advance: f32) -> TextExtent {
        TextExtent {
            advance,
            ink_left: 0.0,
            ink_right: advance,
        }
    }

    #[test]
    fn narrow_glyphs_are_centered_in_every_mode() {
        for wide_glyph in WideGlyph::ALL {
            assert_eq!(
                place_text(upright(10.0), 1, CELL_WIDTH, wide_glyph),
                placement(1, 1.0, 3.0)
            );
            assert_eq!(
                place_text(upright(32.0), 2, CELL_WIDTH, wide_glyph),
                placement(2, 1.0, 0.0)
            );
        }
    }

    #[test]
    fn wide_glyphs_shrink_into_their_cells() {
        assert_eq!(
            place_text(upright(32.0), 1, CELL_WIDTH, WideGlyph::Shrink),
            placement(1, 0.5, 0.0)
        );
    }

    #[test]
    fn clipped_glyphs_keep_their_size_at_the_left() {
        assert_eq!(
            place_text(upright(32.0), 1, CELL_WIDTH, WideGlyph::Clip),
            placement(1, 1.0, 0.0)
        );
        // 切る文字は、インクがはみ出しても余白を足さない。
        let slanted = TextExtent {
            advance: 32.0,
            ink_left: -2.0,
            ink_right: 35.0,
        };
        assert_eq!(
            place_text(slanted, 1, CELL_WIDTH, WideGlyph::Clip),
            placement(1, 1.0, 0.0)
        );
    }

    #[test]
    fn overflowing_glyphs_widen_the_slot_up_to_a_limit() {
        assert_eq!(
            place_text(upright(32.0), 1, CELL_WIDTH, WideGlyph::Overflow),
            placement(2, 1.0, 0.0)
        );
        // 少しでもはみ出せば、次のセルまで広げる。
        assert_eq!(
            place_text(upright(17.0), 1, CELL_WIDTH, WideGlyph::Overflow),
            placement(2, 1.0, 0.0)
        );
        // 2 倍を超える分は縮める。
        assert_eq!(
            place_text(upright(64.0), 1, CELL_WIDTH, WideGlyph::Overflow),
            placement(2, 0.5, 0.0)
        );
        assert_eq!(
            place_text(upright(48.0), 2, CELL_WIDTH, WideGlyph::Overflow),
            placement(3, 1.0, 0.0)
        );
    }

    /// `left` と `right` の余白を持つ置き方。
    fn with_margins(placement: TextPlacement, left: u32, right: u32) -> TextPlacement {
        TextPlacement {
            left_margin: left,
            right_margin: right,
            ..placement
        }
    }

    #[test]
    fn centered_overflowing_glyphs_spill_to_both_sides_up_to_a_limit() {
        let placed = place_text(upright(32.0), 1, CELL_WIDTH, WideGlyph::OverflowCenter);
        assert_eq!(placed, with_margins(placement(1, 1.0, -8.0), 8, 8));
        assert_eq!(placed.slot_width(CELL_WIDTH as u32), 32);
        // はみ出した分だけ余白を足す。半端なピクセルは切り上げる。
        assert_eq!(
            place_text(upright(17.0), 1, CELL_WIDTH, WideGlyph::OverflowCenter),
            with_margins(placement(1, 1.0, -0.5), 1, 1)
        );
        // 2 倍を超える分は縮める。
        assert_eq!(
            place_text(upright(64.0), 1, CELL_WIDTH, WideGlyph::OverflowCenter),
            with_margins(placement(1, 0.5, -8.0), 8, 8)
        );
        assert_eq!(
            place_text(upright(48.0), 2, CELL_WIDTH, WideGlyph::OverflowCenter),
            with_margins(placement(2, 1.0, -8.0), 8, 8)
        );
        // インクが送り幅の外へ出ていれば、その分も余白にする。
        let slanted = TextExtent {
            advance: 32.0,
            ink_left: -1.0,
            ink_right: 35.0,
        };
        assert_eq!(
            place_text(slanted, 1, CELL_WIDTH, WideGlyph::OverflowCenter),
            with_margins(placement(1, 1.0, -8.0), 9, 11)
        );
        // インクが送り幅より狭ければ、余白はインクの分だけ。
        let round = TextExtent {
            advance: 32.0,
            ink_left: 4.0,
            ink_right: 28.0,
        };
        assert_eq!(
            place_text(round, 1, CELL_WIDTH, WideGlyph::OverflowCenter),
            with_margins(placement(1, 1.0, -8.0), 4, 4)
        );
    }

    #[test]
    fn ink_outside_the_cell_gets_margins_without_shrinking() {
        // 送り幅はセルと同じで、インクが左へ 1.5、右へ 2.25 ピクセルはみ出す。
        let slanted = TextExtent {
            advance: 16.0,
            ink_left: -1.5,
            ink_right: 18.25,
        };
        for wide_glyph in WideGlyph::ALL {
            let placed = place_text(slanted, 1, CELL_WIDTH, wide_glyph);
            assert_eq!(
                placed,
                TextPlacement {
                    left_margin: 2,
                    right_margin: 3,
                    ..placement(1, 1.0, 0.0)
                }
            );
            assert_eq!(placed.slot_width(CELL_WIDTH as u32), 21);
        }
    }

    #[test]
    fn ink_margins_are_measured_after_shrinking_and_limited() {
        // 縮めたあとのインクで余白を決める。
        let slanted = TextExtent {
            advance: 32.0,
            ink_left: 0.0,
            ink_right: 36.0,
        };
        assert_eq!(
            place_text(slanted, 1, CELL_WIDTH, WideGlyph::Shrink),
            TextPlacement {
                right_margin: 2,
                ..placement(1, 0.5, 0.0)
            }
        );
        // 余白はセルの幅まで。
        let far = TextExtent {
            advance: 0.0,
            ink_left: -40.0,
            ink_right: 0.0,
        };
        assert_eq!(
            place_text(far, 1, CELL_WIDTH, WideGlyph::Shrink).left_margin,
            16
        );
        // 計算の誤差ほどのはみ出しには余白を足さない。
        let touching = TextExtent {
            advance: 16.0,
            ink_left: -0.001,
            ink_right: 16.001,
        };
        assert_eq!(
            place_text(touching, 1, CELL_WIDTH, WideGlyph::Shrink),
            placement(1, 1.0, 0.0)
        );
    }

    /// Consolas の斜体（太字なら太字斜体）の書体。
    fn italic_face(factory: &IDWriteFactory, weight: DWRITE_FONT_WEIGHT) -> IDWriteFontFace {
        let mut collection = None;
        let mut index = 0u32;
        let mut exists = BOOL::default();
        // SAFETY: 引数はどれも書き込める。FindFamilyName が見つけた番号で書体を取り出す。
        unsafe {
            factory
                .GetSystemFontCollection(&mut collection, false)
                .unwrap();
            let collection = collection.unwrap();
            collection
                .FindFamilyName(w!("Consolas"), &mut index, &mut exists)
                .unwrap();
            assert!(exists.as_bool());
            collection
                .GetFontFamily(index)
                .unwrap()
                .GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC)
                .unwrap()
                .CreateFontFace()
                .unwrap()
        }
    }

    /// `face` の `c` のインクの左右の端（ピクセル）。フォントの設計上の左右のサイドベアリングから求める。
    fn designed_ink(face: &IDWriteFontFace, c: char, size_px: f32) -> (f32, f32) {
        let codepoint = u32::from(c);
        let mut glyph = 0u16;
        let mut glyph_metrics = DWRITE_GLYPH_METRICS::default();
        let mut font_metrics = Default::default();
        // SAFETY: 引数はどれも 1 要素の配列として読み書きできる。
        unsafe {
            face.GetGlyphIndices(&codepoint, 1, &mut glyph).unwrap();
            face.GetDesignGlyphMetrics(&glyph, 1, &mut glyph_metrics, false)
                .unwrap();
            face.GetMetrics(&mut font_metrics);
        }
        let to_px = |units: f32| units * size_px / f32::from(font_metrics.designUnitsPerEm);
        let advance = glyph_metrics.advanceWidth as f32;
        (
            to_px(glyph_metrics.leftSideBearing as f32),
            to_px(advance - glyph_metrics.rightSideBearing as f32),
        )
    }

    #[test]
    fn italic_slots_contain_the_ink() {
        const DPI: u32 = 192;
        let factory = font::create_factory().unwrap();
        let font = font::find(&factory, &["Consolas"], 12.0, DPI).unwrap();
        let cell_width = font.cell_width_px as f32;
        let mut overhangs = (false, false);
        for weight in [DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_BOLD] {
            let format = font::text_format(
                &factory,
                &font.family,
                font.size_px,
                weight,
                DWRITE_FONT_STYLE_ITALIC,
            )
            .unwrap();
            let face = italic_face(&factory, weight);
            for c in ['d', 'f', 'W'] {
                let (_, extent) = text_layout(
                    &factory,
                    &format,
                    &c.to_string(),
                    font.cell_height_px as f32,
                )
                .unwrap();
                let placed = place_text(extent, 1, cell_width, WideGlyph::Shrink);
                assert_eq!(placed.scale, 1.0, "{c:?} is not shrunk");
                // 場所の左端と右端を、セルの左端からの距離で表す。
                let slot_left = -(placed.left_margin as f32);
                let slot_right = cell_width + placed.right_margin as f32;
                let (ink_left, ink_right) = designed_ink(&face, c, font.size_px);
                let (ink_left, ink_right) = (placed.offset + ink_left, placed.offset + ink_right);
                assert!(
                    slot_left <= ink_left && ink_right <= slot_right,
                    "{c:?} (weight {}): ink {ink_left}..{ink_right} is outside {slot_left}..{slot_right}",
                    weight.0
                );
                overhangs.0 |= ink_left < 0.0;
                overhangs.1 |= ink_right > cell_width;
            }
        }
        // 確認した文字のインクには、セルの左へも右へもはみ出すものがある。
        assert_eq!(overhangs, (true, true));
    }
}
