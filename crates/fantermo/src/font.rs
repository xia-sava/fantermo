//! DirectWrite でフォントを探し、セルの寸法を測る。

use windows::{
    Win32::Graphics::{
        DirectWrite::{
            DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_METRICS, DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_GLYPH_METRICS,
            DWriteCreateFactory, IDWriteFactory, IDWriteFont1, IDWriteFontCollection,
            IDWriteFontFace, IDWriteFontFamily,
        },
        Gdi::{DEFAULT_CHARSET, LOGFONTW},
    },
    core::{BOOL, HSTRING, Interface},
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

/// システムのフォントの書体の一つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Family {
    /// システムのフォントの中の番号。
    pub index: u32,
    /// 見せる名前。日本語の名前があればそれ、なければ英語の名前。
    pub name: String,
}

/// 見せる名前を選ぶときに探すロケールの順。
const NAME_LOCALES: [&str; 2] = ["ja-jp", "en-us"];

/// システムのフォントのうち、等幅の書体（標準の太さ・字形のフォントが等幅のもの）。見せる名前の順。
pub fn monospaced_families(factory: &IDWriteFactory) -> windows::core::Result<Vec<Family>> {
    let collection = system_collection(factory)?;
    // SAFETY: collection は有効なフォントの集まり。
    let count = unsafe { collection.GetFontFamilyCount() };
    let mut families = Vec::new();
    for index in 0..count {
        // SAFETY: index は書体の数より小さい。
        let family = match unsafe { collection.GetFontFamily(index) } {
            Ok(family) => family,
            Err(e) => {
                tracing::warn!("failed to read the font family {index}: {e}");
                continue;
            }
        };
        match is_monospaced(&family) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                tracing::debug!("cannot tell whether the font family {index} is monospaced: {e}");
                continue;
            }
        }
        match display_name(&family) {
            Ok(name) => families.push(Family { index, name }),
            Err(e) => tracing::warn!("failed to read the name of the font family {index}: {e}"),
        }
    }
    families.sort_by_key(|family| family.name.to_lowercase());
    Ok(families)
}

/// 名前 `name`（どの言語の名前でもよい）の書体の、システムのフォントの中の番号。
pub fn family_index(factory: &IDWriteFactory, name: &str) -> Option<u32> {
    let collection = system_collection(factory).ok()?;
    find_family(&collection, name).ok()
}

fn is_monospaced(family: &IDWriteFontFamily) -> windows::core::Result<bool> {
    // SAFETY: family は有効な書体。
    let font = unsafe {
        family.GetFirstMatchingFont(
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
        )?
    };
    let font: IDWriteFont1 = font.cast()?;
    // SAFETY: font は有効なフォント。
    Ok(unsafe { font.IsMonospacedFont() }.as_bool())
}

/// 書体の見せる名前。[`NAME_LOCALES`] の順に探し、どれもなければ最初の名前。
fn display_name(family: &IDWriteFontFamily) -> windows::core::Result<String> {
    // SAFETY: family は有効な書体。
    let names = unsafe { family.GetFamilyNames()? };
    let mut index = 0u32;
    for locale in NAME_LOCALES {
        let mut exists = BOOL::default();
        // SAFETY: index と exists は書き込める。
        unsafe { names.FindLocaleName(&HSTRING::from(locale), &mut index, &mut exists)? };
        if exists.as_bool() {
            break;
        }
        index = 0;
    }
    // SAFETY: index は名前の数より小さい（見つからなければ 0。書体は少なくとも一つの名前を持つ）。
    let length = unsafe { names.GetStringLength(index)? } as usize;
    let mut buffer = vec![0u16; length + 1];
    // SAFETY: buffer は終端を含めて length + 1 個の u16 を書き込める。
    unsafe { names.GetString(index, &mut buffer)? };
    Ok(String::from_utf16_lossy(&buffer[..length]))
}

fn system_collection(factory: &IDWriteFactory) -> windows::core::Result<IDWriteFontCollection> {
    let mut collection = None;
    // SAFETY: collection は書き込める Option を指す。
    unsafe { factory.GetSystemFontCollection(&mut collection, false)? };
    collection.ok_or_else(windows::core::Error::empty)
}

fn find_family(collection: &IDWriteFontCollection, family: &str) -> windows::core::Result<u32> {
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
    Ok(index)
}

fn font_face(factory: &IDWriteFactory, family: &str) -> windows::core::Result<IDWriteFontFace> {
    let collection = system_collection(factory)?;
    let index = find_family(&collection, family)?;
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
    fn monospaced_families_include_consolas() {
        let factory = create_factory().unwrap();
        let families = monospaced_families(&factory).unwrap();
        let consolas = family_index(&factory, "Consolas").unwrap();
        assert!(
            families
                .iter()
                .any(|family| family.index == consolas && family.name == "Consolas"),
            "{families:?}"
        );
        assert!(
            !families
                .iter()
                .any(|family| Some(family.index) == family_index(&factory, "Segoe UI")),
        );
        // 日本語の名前を持つ書体は、日本語の名前で見せる。ＭＳ ゴシックの無い環境では確認しない。
        let Some(ms_gothic) = family_index(&factory, "MS Gothic") else {
            return;
        };
        assert!(
            families
                .iter()
                .any(|family| family.index == ms_gothic && family.name == "ＭＳ ゴシック"),
            "{families:?}"
        );
    }

    #[test]
    fn long_face_names_keep_the_terminator() {
        let logfont = font(&"x".repeat(100)).logfont();
        assert_eq!(logfont.lfFaceName.last(), Some(&0));
    }
}
