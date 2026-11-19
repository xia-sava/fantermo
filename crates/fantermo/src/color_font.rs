//! 文字が色付きのフォント（Segoe UI Emoji など）で描かれるかを、DirectWrite のシステムのフォントの
//! フォールバックで調べる。[`Emoji::Unicode`](fantermo_core::config::Emoji::Unicode) で、書記素
//! クラスタを描く書体（[`ClusterFont`]）も決める。

use windows::{
    Win32::Graphics::DirectWrite::{
        DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE, DWRITE_FONT_WEIGHT,
        DWRITE_READING_DIRECTION, DWRITE_READING_DIRECTION_LEFT_TO_RIGHT, IDWriteFactory,
        IDWriteFactory2, IDWriteFont, IDWriteFont2, IDWriteFontCollection, IDWriteFontFallback,
        IDWriteFontFamily, IDWriteNumberSubstitution, IDWriteTextAnalysisSource,
        IDWriteTextAnalysisSource_Impl,
    },
    core::{HSTRING, Interface, OutRef, implement},
};

use crate::{
    font::{self, LOCALE},
    presentation::{self, is_emoji_presentation, is_presentation_selector},
    text_placement::FamilyRun,
};

/// 絵文字として表示する書記素クラスタを組む書体。
pub const EMOJI_FAMILY: &str = "Segoe UI Emoji";

/// 文字として表示する書記素クラスタが色付きのフォントに当たるとき、その文字をすべて持っていれば組む書体。
pub const SYMBOL_FAMILY: &str = "Segoe UI Symbol";

/// [`Emoji::Unicode`](fantermo_core::config::Emoji::Unicode) で、書記素クラスタを組む書体。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClusterFont {
    /// 基準の書体（と、そのフォールバック）のまま組む。文字として表示するクラスタで、色付きのフォントに
    /// 当たらないものと、当たるが [`SYMBOL_FAMILY`] が持っていないもの。
    Base,
    /// 絵文字として表示するクラスタ。[`EMOJI_FAMILY`] で組み、色付きの字形を色のまま描く。
    Emoji,
    /// 文字として表示するクラスタで、色付きのフォントに当たり、[`SYMBOL_FAMILY`] がすべての文字を持つもの。
    /// [`SYMBOL_FAMILY`] で組む。
    Symbol,
}

impl ClusterFont {
    /// 組む書体の名前。基準の書体のままなら `None`。
    pub fn family(self) -> Option<&'static str> {
        match self {
            ClusterFont::Base => None,
            ClusterFont::Emoji => Some(EMOJI_FAMILY),
            ClusterFont::Symbol => Some(SYMBOL_FAMILY),
        }
    }
}

/// 書記素クラスタ `cluster` を、太さ `weight`、形 `style` で組む書体。絵文字として表示するかは
/// [`is_emoji_presentation`] で決め、文字として表示するものは `color_fonts` で色付きのフォントに当たるかと、
/// [`SYMBOL_FAMILY`] が持っているかを調べる。`color_fonts` がないときと、調べられなかったときは基準の書体。
pub fn cluster_font(
    color_fonts: Option<&ColorFonts>,
    cluster: &str,
    weight: DWRITE_FONT_WEIGHT,
    style: DWRITE_FONT_STYLE,
) -> ClusterFont {
    if is_emoji_presentation(cluster) {
        return ClusterFont::Emoji;
    }
    let Some(color_fonts) = color_fonts else {
        return ClusterFont::Base;
    };
    match color_fonts.contains_color(cluster, weight, style) {
        Ok(true) => {}
        Ok(false) => return ClusterFont::Base,
        Err(e) => {
            tracing::warn!("failed to find the font of {cluster:?}: {e}");
            return ClusterFont::Base;
        }
    }
    match color_fonts.symbol_has_all(cluster, weight, style) {
        Ok(true) => ClusterFont::Symbol,
        Ok(false) => ClusterFont::Base,
        Err(e) => {
            tracing::warn!("failed to look up {cluster:?} in {SYMBOL_FAMILY}: {e}");
            ClusterFont::Base
        }
    }
}

/// `text` を書記素クラスタに区切り、`font_of` が返す書体で組む範囲。基準の書体のまま組むクラスタは
/// 範囲に入れず、同じ書体のクラスタが続けば一つの範囲にまとめる。
pub fn family_runs(text: &str, mut font_of: impl FnMut(&str) -> ClusterFont) -> Vec<FamilyRun> {
    let mut runs: Vec<FamilyRun> = Vec::new();
    for (start, length, cluster) in presentation::utf16_clusters(text) {
        let Some(family) = font_of(cluster).family() else {
            continue;
        };
        match runs.last_mut() {
            Some(last) if last.family == family && last.start + last.length == start => {
                last.length += length;
            }
            _ => runs.push(FamilyRun {
                start,
                length,
                family,
            }),
        }
    }
    runs
}

/// 一つの書体（端末のフォントや UI のフォント）を基準にして、文字を描くフォントを引くもの。
pub struct ColorFonts {
    fallback: IDWriteFontFallback,
    collection: IDWriteFontCollection,
    /// 基準の書体の名前。
    family: HSTRING,
    /// [`SYMBOL_FAMILY`] の書体。システムになければ `None`。
    symbol: Option<IDWriteFontFamily>,
}

impl ColorFonts {
    /// 書体 `family` を基準にして引くもの。
    pub fn new(dwrite: &IDWriteFactory, family: &str) -> windows::core::Result<Self> {
        let factory: IDWriteFactory2 = dwrite.cast()?;
        // SAFETY: factory は有効な DirectWrite のファクトリ。
        let fallback = unsafe { factory.GetSystemFontFallback()? };
        let collection = font::system_collection(dwrite)?;
        let symbol = font::find_family(&collection, SYMBOL_FAMILY)
            // SAFETY: index は find_family が見つけた有効な番号。
            .and_then(|index| unsafe { collection.GetFontFamily(index) })
            .inspect_err(|e| tracing::warn!("{SYMBOL_FAMILY} is not available: {e}"))
            .ok();
        Ok(Self {
            fallback,
            collection,
            family: HSTRING::from(family),
            symbol,
        })
    }

    /// [`SYMBOL_FAMILY`] の太さ `weight`、形 `style` のフォントが、`text` の文字をすべて持っているか。
    /// 異体字セレクタ（`U+FE0E` と `U+FE0F`）は見ない。
    fn symbol_has_all(
        &self,
        text: &str,
        weight: DWRITE_FONT_WEIGHT,
        style: DWRITE_FONT_STYLE,
    ) -> windows::core::Result<bool> {
        let Some(family) = &self.symbol else {
            return Ok(false);
        };
        // SAFETY: family は有効な書体。
        let font =
            unsafe { family.GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, style)? };
        for c in text.chars().filter(|&c| !is_presentation_selector(c)) {
            // SAFETY: font は有効なフォント。
            if !unsafe { font.HasCharacter(u32::from(c))? }.as_bool() {
                return Ok(false);
            }
        }
        Ok(true)
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
        let index = font::find_family(&self.collection, &self.family.to_string())?;
        // SAFETY: index は find_family が見つけた有効な番号。
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

    #[test]
    fn clusters_choose_the_emoji_font_the_symbol_font_or_the_base() {
        let factory = font::create_factory().unwrap();
        let fonts = ColorFonts::new(&factory, "Consolas").unwrap();
        let font_of = |cluster: &str| {
            cluster_font(
                Some(&fonts),
                cluster,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
            )
        };
        assert_eq!(font_of("\u{1F600}"), ClusterFont::Emoji);
        assert_eq!(font_of("\u{2733}\u{FE0F}"), ClusterFont::Emoji);
        // Consolas になく色付きのフォントに当たる ✳ は、Segoe UI Symbol が持っている。
        assert_eq!(font_of("\u{2733}"), ClusterFont::Symbol);
        assert_eq!(font_of("A"), ClusterFont::Base);
        // 基準の書体を調べられなくても、絵文字として表示するものは決まる。
        assert_eq!(
            cluster_font(
                None,
                "\u{1F600}",
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL
            ),
            ClusterFont::Emoji
        );
        assert_eq!(
            cluster_font(
                None,
                "\u{2733}",
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL
            ),
            ClusterFont::Base
        );
    }

    #[test]
    fn runs_cover_the_clusters_that_are_not_in_the_base_font() {
        let font_of = |cluster: &str| match cluster {
            "e" | "\u{1F1EF}\u{1F1F5}" => ClusterFont::Emoji,
            "s" => ClusterFont::Symbol,
            _ => ClusterFont::Base,
        };
        let run = |start, length, family| FamilyRun {
            start,
            length,
            family,
        };
        assert_eq!(
            family_runs("ae\u{1F1EF}\u{1F1F5}sbee", font_of),
            [
                run(1, 5, EMOJI_FAMILY),
                run(6, 1, SYMBOL_FAMILY),
                run(8, 2, EMOJI_FAMILY),
            ]
        );
        assert!(family_runs("abc", font_of).is_empty());
    }
}
