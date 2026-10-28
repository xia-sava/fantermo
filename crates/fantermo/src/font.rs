//! DirectWrite でフォントを探し、セルの寸法を測る。

use windows::{
    Win32::Graphics::DirectWrite::{
        DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_METRICS, DWRITE_FONT_STRETCH_NORMAL,
        DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_GLYPH_METRICS,
        DWriteCreateFactory, IDWriteFactory, IDWriteFontFace,
    },
    core::{BOOL, HSTRING},
};

/// 1 インチあたりのポイント数。
const POINTS_PER_INCH: f32 = 72.0;

/// セルの幅を決めるときに測る文字。
const CELL_WIDTH_REFERENCE: char = 'M';

/// 見つかったフォントと、その大きさで描くときのセルの寸法。
pub struct Font {
    pub family: String,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
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

    Font {
        family: family.to_owned(),
        cell_width_px: (advance_units as f32 * size_px / units_per_em).ceil() as u32,
        cell_height_px: (line_units * size_px / units_per_em).ceil() as u32,
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
