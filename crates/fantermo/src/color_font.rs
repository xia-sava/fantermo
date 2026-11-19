//! 文字が色付きのフォント（Segoe UI Emoji など）で描かれるかを、DirectWrite のシステムのフォントの
//! フォールバックで調べる。

use windows::{
    Win32::Graphics::DirectWrite::{
        DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE, DWRITE_FONT_WEIGHT,
        DWRITE_READING_DIRECTION, DWRITE_READING_DIRECTION_LEFT_TO_RIGHT, IDWriteFactory,
        IDWriteFactory2, IDWriteFont, IDWriteFont2, IDWriteFontCollection, IDWriteFontFallback,
        IDWriteNumberSubstitution, IDWriteTextAnalysisSource, IDWriteTextAnalysisSource_Impl,
    },
    core::{BOOL, HSTRING, Interface, OutRef, implement},
};

use crate::font::{self, LOCALE};

/// 端末のフォントを基準の書体にして、文字を描くフォントを引くもの。
pub struct ColorFonts {
    fallback: IDWriteFontFallback,
    collection: IDWriteFontCollection,
    /// 基準の書体の名前。
    family: HSTRING,
}

impl ColorFonts {
    /// 書体 `family` を基準にして引くもの。
    pub fn new(dwrite: &IDWriteFactory, family: &str) -> windows::core::Result<Self> {
        let factory: IDWriteFactory2 = dwrite.cast()?;
        // SAFETY: factory は有効な DirectWrite のファクトリ。
        let fallback = unsafe { factory.GetSystemFontFallback()? };
        Ok(Self {
            fallback,
            collection: font::system_collection(dwrite)?,
            family: HSTRING::from(family),
        })
    }

    /// `text` を太さ `weight`、形 `style` で組んだとき、色付きのフォントで描く文字を含むか。
    /// 文字列の途中で別のフォントに切り替わるなら、切り替わった先のフォントも見る。
    pub fn contains_color(
        &self,
        text: &str,
        weight: DWRITE_FONT_WEIGHT,
        style: DWRITE_FONT_STYLE,
    ) -> windows::core::Result<bool> {
        let units: Vec<u16> = text.encode_utf16().collect();
        let length = units.len() as u32;
        let source: IDWriteTextAnalysisSource = AnalysisSource { text: units }.into();
        let mut position = 0;
        while position < length {
            let mut mapped_length = 0u32;
            let mut mapped_font = None;
            let mut scale = 0.0f32;
            // SAFETY: source、collection、family はこの呼び出しの間は生きていて、受け取る値は書き込める。
            unsafe {
                self.fallback.MapCharacters(
                    &source,
                    position,
                    length - position,
                    &self.collection,
                    &self.family,
                    weight,
                    style,
                    DWRITE_FONT_STRETCH_NORMAL,
                    &mut mapped_length,
                    &mut mapped_font,
                    &mut scale,
                )?
            };
            // 当たるフォントがなければ、基準の書体で描く。
            let font = match mapped_font {
                Some(font) => font,
                None => self.base_font(weight, style)?,
            };
            if is_color_font(&font)? {
                return Ok(true);
            }
            if mapped_length == 0 {
                break;
            }
            position += mapped_length;
        }
        Ok(false)
    }

    /// 基準の書体の、太さ `weight`、形 `style` のフォント。
    fn base_font(
        &self,
        weight: DWRITE_FONT_WEIGHT,
        style: DWRITE_FONT_STYLE,
    ) -> windows::core::Result<IDWriteFont> {
        let mut index = 0u32;
        let mut exists = BOOL::default();
        // SAFETY: family はこの呼び出しの間は生きていて、index と exists は書き込める。
        unsafe {
            self.collection
                .FindFamilyName(&self.family, &mut index, &mut exists)?
        };
        if !exists.as_bool() {
            return Err(windows::core::Error::empty());
        }
        // SAFETY: index は FindFamilyName が見つけた有効な番号。
        unsafe {
            self.collection.GetFontFamily(index)?.GetFirstMatchingFont(
                weight,
                DWRITE_FONT_STRETCH_NORMAL,
                style,
            )
        }
    }
}

fn is_color_font(font: &IDWriteFont) -> windows::core::Result<bool> {
    let font: IDWriteFont2 = font.cast()?;
    // SAFETY: font は有効なフォント。
    Ok(unsafe { font.IsColorFont() }.as_bool())
}

/// [`IDWriteFontFallback::MapCharacters`] に渡す文字列。左から右へ書く 1 段落で、ロケールは
/// [`LOCALE`]、数字の置き換えはない。
#[implement(IDWriteTextAnalysisSource)]
struct AnalysisSource {
    /// UTF-16 の文字列。
    text: Vec<u16>,
}

impl AnalysisSource {
    /// 位置 `position` から末尾までの長さ。
    fn rest_length(&self, position: u32) -> u32 {
        (self.text.len() as u32).saturating_sub(position)
    }
}

impl IDWriteTextAnalysisSource_Impl for AnalysisSource_Impl {
    fn GetTextAtPosition(
        &self,
        textposition: u32,
        textstring: *mut *mut u16,
        textlength: *mut u32,
    ) -> windows::core::Result<()> {
        let rest = self.text.get(textposition as usize..).unwrap_or_default();
        // SAFETY: DirectWrite が書き込める場所を渡す。文字列は self が持ち、self が生きている間は動かない。
        unsafe {
            *textstring = if rest.is_empty() {
                std::ptr::null_mut()
            } else {
                rest.as_ptr().cast_mut()
            };
            *textlength = rest.len() as u32;
        }
        Ok(())
    }

    fn GetTextBeforePosition(
        &self,
        textposition: u32,
        textstring: *mut *mut u16,
        textlength: *mut u32,
    ) -> windows::core::Result<()> {
        // 先頭と、末尾より後ろの位置では、前の文字はない。
        let before = self.text.get(..textposition as usize).unwrap_or_default();
        // SAFETY: DirectWrite が書き込める場所を渡す。文字列は self が持ち、self が生きている間は動かない。
        unsafe {
            *textstring = if before.is_empty() {
                std::ptr::null_mut()
            } else {
                before.as_ptr().cast_mut()
            };
            *textlength = before.len() as u32;
        }
        Ok(())
    }

    fn GetParagraphReadingDirection(&self) -> DWRITE_READING_DIRECTION {
        DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
    }

    fn GetLocaleName(
        &self,
        textposition: u32,
        textlength: *mut u32,
        localename: *mut *mut u16,
    ) -> windows::core::Result<()> {
        // SAFETY: DirectWrite が書き込める場所を渡す。ロケールの名前は静的な文字列。
        unsafe {
            *textlength = self.rest_length(textposition);
            *localename = LOCALE.as_ptr().cast_mut();
        }
        Ok(())
    }

    fn GetNumberSubstitution(
        &self,
        textposition: u32,
        textlength: *mut u32,
        numbersubstitution: OutRef<IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        // SAFETY: DirectWrite が書き込める場所を渡す。
        unsafe { *textlength = self.rest_length(textposition) };
        numbersubstitution.write(None)
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD,
        DWRITE_FONT_WEIGHT_NORMAL,
    };

    use super::*;

    #[test]
    fn emoji_are_drawn_with_a_color_font() {
        let factory = font::create_factory().unwrap();
        let fonts = ColorFonts::new(&factory, "Consolas").unwrap();
        let is_color = |text: &str| {
            fonts
                .contains_color(text, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL)
                .unwrap()
        };
        assert!(!is_color("A"));
        assert!(!is_color("あ"));
        assert!(is_color("\u{1F600}"));
        assert!(is_color("\u{2705}"));
        // 途中で色付きのフォントに切り替わる文字列も、色付きのフォントで描く文字を含む。
        assert!(is_color("A\u{1F600}"));
        assert!(!is_color("Aあ"));
        assert!(
            fonts
                .contains_color(
                    "\u{1F600}",
                    DWRITE_FONT_WEIGHT_BOLD,
                    DWRITE_FONT_STYLE_ITALIC
                )
                .unwrap()
        );
    }
}
