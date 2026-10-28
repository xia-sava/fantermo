//! DirectWrite でフォントを探し、セルの寸法を測る。

use windows::{
    Win32::Graphics::{
        DirectWrite::{
            DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_METRICS, DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_GLYPH_METRICS,
            DWriteCreateFactory, IDWriteFactory, IDWriteFontFace,
        },
        Gdi::{DEFAULT_CHARSET, LOGFONTW},
    },
    core::{BOOL, HSTRING},
};

/// 1 インチあたりのポイント数。
const POINTS_PER_INCH: f32 = 72.0;

/// セルの幅を決めるときに測る文字。
const CELL_WIDTH_REFERENCE: char = 'M';

/// 見つかったフォントと、その大きさで描くときのセルの寸法。
#[derive(Clone, Debug)]
pub struct Font {
    pub family: String,
    /// フォントの大きさ（ポイント）。
    pub size_pt: f32,
    /// フォントの大きさ（ピクセル）。
    pub size_px: f32,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
    pub lines: LineMetrics,
}

/// 下線と取り消し線の位置と太さ（ピクセル）。フォントが決めた値で、丸めていない。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineMetrics {
    /// ベースラインから下線の上端までの距離。下向きが正。
    pub underline_offset: f32,
    pub underline_thickness: f32,
    /// ベースラインから取り消し線の上端までの距離。上向きが正。
    pub strikethrough_offset: f32,
    pub strikethrough_thickness: f32,
}

impl Font {
    /// 同じ書体と大きさの LOGFONTW（GDI や IME に渡すため）。
    pub fn logfont(&self) -> LOGFONTW {
        let mut logfont = LOGFONTW {
            // 負の値は、行の間隔を含まない文字の高さ（ピクセル）。
            lfHeight: -(self.size_px.round() as i32),
            lfCharSet: DEFAULT_CHARSET,
            ..Default::default()
        };
        // 書体名は終端の NUL を含めて配列に収まるところまで。
        let capacity = logfont.lfFaceName.len() - 1;
        for (slot, unit) in logfont
            .lfFaceName
            .iter_mut()
            .zip(self.family.encode_utf16().take(capacity))
        {
            *slot = unit;
        }
        logfont
    }
}

/// `families` を順に探し、最初に見つかったフォントを `size_pt` ポイント、`dpi` で測る。
pub fn find(
    factory: &IDWriteFactory,
    families: &[&str],
    size_pt: f32,
    dpi: u32,
) -> windows::core::Result<Font> {
    let mut last_error = None;
    for family in families {
        match font_face(factory, family) {
            Ok(face) => return Ok(measure(family, face, size_pt, dpi)),
            Err(e) => {
                tracing::warn!("font {family:?} is not available: {e}");
                last_error = Some(e);
            }
        }
    }
    Err(last_error.unwrap_or_else(windows::core::Error::empty))
}

pub fn create_factory() -> windows::core::Result<IDWriteFactory> {
    // SAFETY: 共有のファクトリを作るだけで、引数に前提はない。
    unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
}

fn font_face(factory: &IDWriteFactory, family: &str) -> windows::core::Result<IDWriteFontFace> {
    let mut collection = None;
    // SAFETY: collection は書き込める Option を指す。
    unsafe { factory.GetSystemFontCollection(&mut collection, false)? };
    let collection = collection.ok_or_else(windows::core::Error::empty)?;

    let mut index = 0u32;
    let mut exists = BOOL::default();
    // SAFETY: index と exists は書き込める。
    unsafe { collection.FindFamilyName(&HSTRING::from(family), &mut index, &mut exists)? };
    if !exists.as_bool() {
        return Err(windows::core::Error::new(
            windows::Win32::Foundation::E_INVALIDARG,
            format!("font family {family:?} not found"),
        ));
    }
    // SAFETY: index は FindFamilyName が見つけた有効な番号。
    let font = unsafe {
        collection.GetFontFamily(index)?.GetFirstMatchingFont(
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
        )?
    };
    // SAFETY: font は有効なフォント。
    unsafe { font.CreateFontFace() }
}

fn measure(family: &str, face: IDWriteFontFace, size_pt: f32, dpi: u32) -> Font {
    let size_px = size_pt * dpi as f32 / POINTS_PER_INCH;

    let mut metrics = DWRITE_FONT_METRICS::default();
    // SAFETY: metrics は書き込める。
    unsafe { face.GetMetrics(&mut metrics) };
    let units_per_em = f32::from(metrics.designUnitsPerEm);
    let line_units =
        f32::from(metrics.ascent) + f32::from(metrics.descent) + f32::from(metrics.lineGap);

    let advance_units = reference_advance(&face).unwrap_or(metrics.designUnitsPerEm.into());
    let to_px = |units: f32| units * size_px / units_per_em;

    // DirectWrite の下線の位置はベースラインからの上端で、下が負。取り消し線の位置も上端で、上が正。
    let lines = LineMetrics {
        underline_offset: to_px(-f32::from(metrics.underlinePosition)),
        underline_thickness: to_px(f32::from(metrics.underlineThickness)),
        strikethrough_offset: to_px(f32::from(metrics.strikethroughPosition)),
        strikethrough_thickness: to_px(f32::from(metrics.strikethroughThickness)),
    };
    tracing::debug!("line metrics of {family:?}: {lines:?}");

    Font {
        family: family.to_owned(),
        size_pt,
        size_px,
        cell_width_px: to_px(advance_units as f32).ceil() as u32,
        cell_height_px: to_px(line_units).ceil() as u32,
        lines,
    }
}

/// [`CELL_WIDTH_REFERENCE`] の送り幅（フォントの設計単位）。
fn reference_advance(face: &IDWriteFontFace) -> Option<u32> {
    let codepoint = u32::from(CELL_WIDTH_REFERENCE);
    let mut glyph = 0u16;
    // SAFETY: codepoint と glyph はそれぞれ 1 要素の配列として読み書きできる。
    unsafe { face.GetGlyphIndices(&codepoint, 1, &mut glyph).ok()? };
    let mut glyph_metrics = DWRITE_GLYPH_METRICS::default();
    // SAFETY: glyph と glyph_metrics はそれぞれ 1 要素の配列として読み書きできる。
    unsafe {
        face.GetDesignGlyphMetrics(&glyph, 1, &mut glyph_metrics, false)
            .ok()?
    };
    Some(glyph_metrics.advanceWidth)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font(family: &str) -> Font {
        Font {
            family: family.to_owned(),
            size_pt: 12.0,
            size_px: 31.6,
            cell_width_px: 16,
            cell_height_px: 32,
            lines: LineMetrics {
                underline_offset: 3.0,
                underline_thickness: 1.5,
                strikethrough_offset: 9.0,
                strikethrough_thickness: 1.5,
            },
        }
    }

    #[test]
    fn logfont_uses_the_same_face_and_size() {
        let logfont = font("Bizin Gothic").logfont();
        assert_eq!(logfont.lfHeight, -32);
        let name_len = logfont
            .lfFaceName
            .iter()
            .position(|&unit| unit == 0)
            .unwrap();
        assert_eq!(
            String::from_utf16(&logfont.lfFaceName[..name_len]).unwrap(),
            "Bizin Gothic"
        );
    }

    #[test]
    fn doubling_the_dpi_doubles_the_measurements() {
        const BASE_DPI: u32 = 96;
        let factory = create_factory().unwrap();
        let base = find(&factory, &["Consolas"], 12.0, BASE_DPI).unwrap();
        let doubled = find(&factory, &["Consolas"], 12.0, BASE_DPI * 2).unwrap();
        assert_eq!(doubled.size_px, base.size_px * 2.0);
        // セルの寸法は切り上げるので、2 倍との差は 1 ピクセルまで。
        assert!(doubled.cell_width_px.abs_diff(base.cell_width_px * 2) <= 1);
        assert!(doubled.cell_height_px.abs_diff(base.cell_height_px * 2) <= 1);
        let lines = doubled.lines;
        assert!((lines.underline_offset - base.lines.underline_offset * 2.0).abs() < 1e-3);
        assert!((lines.underline_thickness - base.lines.underline_thickness * 2.0).abs() < 1e-3);
    }

    #[test]
    fn long_face_names_keep_the_terminator() {
        let logfont = font(&"x".repeat(100)).logfont();
        assert_eq!(logfont.lfFaceName.last(), Some(&0));
    }
}
