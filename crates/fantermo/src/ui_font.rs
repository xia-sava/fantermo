//! タブバーのタイトルに使う、Windows の UI のフォント（メッセージボックスなどのメッセージのフォント）。
//! 端末のフォントとは別に、ウィンドウの DPI に合わせ、メッセージのフォントより少し小さくした大きさで
//! DirectWrite の書式を作り、文字列の幅を測る。

use std::{cell::RefCell, collections::HashMap};

use windows::{
    Win32::{
        Graphics::DirectWrite::{
            DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
            DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
            IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
        },
        Graphics::Gdi::LOGFONTW,
        UI::{
            HiDpi::SystemParametersInfoForDpi, WindowsAndMessaging::NONCLIENTMETRICSW,
            WindowsAndMessaging::SPI_GETNONCLIENTMETRICS,
        },
    },
    core::{HSTRING, w},
};

use crate::{
    atlas::{self, TextExtent},
    tab_bar,
};

/// メッセージのフォントを問い合わせられなかったときの書体。
const FALLBACK_FAMILY: &str = "Segoe UI";

/// メッセージのフォントを問い合わせられなかったときの大きさ（ポイント）。
const FALLBACK_SIZE_PT: f32 = 9.0;

/// メッセージのフォントの大きさに対する、タブのタイトルの大きさの割合。
const TITLE_SIZE_RATIO: f32 = 0.85;

/// 1 インチあたりのポイント数。
const POINTS_PER_INCH: f32 = 72.0;

/// 文字を組むときのロケール。和文の字形を選ばせる。
const LOCALE: windows::core::PCWSTR = w!("ja-jp");

/// 行の高さを測るときに組む文字。
const LINE_HEIGHT_REFERENCE: &str = "Mg";

/// 測った幅を覚えておく数の上限。超えたら忘れて測り直す。
const MAX_CACHED_WIDTHS: usize = 4096;

/// Windows の UI のフォント。
pub struct UiFont {
    dwrite: IDWriteFactory,
    pub family: String,
    /// フォントの大きさ（ピクセル）。
    pub size_px: f32,
    /// メッセージのフォントの太さの書式。
    format: IDWriteTextFormat,
    /// 1 行の高さ（ピクセル）。
    height_px: u32,
    /// 測った文字列の幅。
    widths: RefCell<HashMap<String, u32>>,
}

impl UiFont {
    /// DPI が `dpi` のウィンドウのメッセージのフォントの書体を、その [`TITLE_SIZE_RATIO`] 倍の大きさで。
    /// 問い合わせられなければ [`FALLBACK_FAMILY`] の [`FALLBACK_SIZE_PT`] ポイントの同じ割合。
    pub fn for_dpi(dwrite: &IDWriteFactory, dpi: u32) -> windows::core::Result<Self> {
        let (family, message_size_px, weight) = match message_font(dpi) {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!("failed to get the message font for {dpi} DPI: {e}");
                (
                    FALLBACK_FAMILY.to_owned(),
                    FALLBACK_SIZE_PT * dpi as f32 / POINTS_PER_INCH,
                    DWRITE_FONT_WEIGHT_NORMAL,
                )
            }
        };
        let size_px = message_size_px * TITLE_SIZE_RATIO;
        let format = text_format(dwrite, &family, size_px, weight)?;
        let height_px = line_height(dwrite, &format)?;
        Ok(Self {
            dwrite: dwrite.clone(),
            family,
            size_px,
            format,
            height_px,
            widths: RefCell::new(HashMap::new()),
        })
    }

    /// 1 行の高さ（ピクセル）。
    pub fn height_px(&self) -> u32 {
        self.height_px
    }

    /// `text` を 1 行に組んだものと、その送り幅とインクの範囲。
    pub fn layout(&self, text: &str) -> windows::core::Result<(IDWriteTextLayout, TextExtent)> {
        atlas::text_layout(&self.dwrite, &self.format, text, self.height_px as f32)
    }
}

impl tab_bar::Measure for UiFont {
    fn width(&self, text: &str) -> u32 {
        if let Some(width) = self.widths.borrow().get(text) {
            return *width;
        }
        let width = match self.layout(text) {
            Ok((_, extent)) => extent.advance.ceil() as u32,
            Err(e) => {
                tracing::warn!("failed to measure {text:?}: {e}");
                0
            }
        };
        let mut widths = self.widths.borrow_mut();
        if widths.len() >= MAX_CACHED_WIDTHS {
            widths.clear();
        }
        widths.insert(text.to_owned(), width);
        width
    }

    fn height(&self) -> u32 {
        self.height_px
    }
}

/// DPI が `dpi` のときのメッセージのフォント（Windows の設定）。ダイアログのコントロールにも使う。
pub fn message_logfont(dpi: u32) -> windows::core::Result<LOGFONTW> {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    // SAFETY: metrics は cbSize を入れた書き込める NONCLIENTMETRICSW。
    unsafe {
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some((&raw mut metrics).cast()),
            0,
            dpi,
        )?
    };
    Ok(metrics.lfMessageFont)
}

/// DPI が `dpi` のときのメッセージのフォントの書体、大きさ（ピクセル）、太さ。
fn message_font(dpi: u32) -> windows::core::Result<(String, f32, DWRITE_FONT_WEIGHT)> {
    let font = message_logfont(dpi)?;
    let name_len = font
        .lfFaceName
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(font.lfFaceName.len());
    let family = String::from_utf16_lossy(&font.lfFaceName[..name_len]);
    // 負の値は行の間隔を含まない文字の高さ（ピクセル）、正の値は含む高さ。どちらも大きさとして使う。
    let size_px = font.lfHeight.unsigned_abs() as f32;
    if family.is_empty() || size_px == 0.0 {
        return Err(windows::core::Error::new(
            windows::Win32::Foundation::E_UNEXPECTED,
            format!("the message font is empty: {family:?} {size_px}px"),
        ));
    }
    let weight = if font.lfWeight > 0 {
        DWRITE_FONT_WEIGHT(font.lfWeight)
    } else {
        DWRITE_FONT_WEIGHT_NORMAL
    };
    Ok((family, size_px, weight))
}

fn text_format(
    dwrite: &IDWriteFactory,
    family: &str,
    size_px: f32,
    weight: DWRITE_FONT_WEIGHT,
) -> windows::core::Result<IDWriteTextFormat> {
    // SAFETY: 文字列はこの呼び出しの間は生きている。
    let format = unsafe {
        dwrite.CreateTextFormat(
            &HSTRING::from(family),
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size_px,
            LOCALE,
        )?
    };
    // SAFETY: format は有効な書式。
    unsafe { format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)? };
    Ok(format)
}

/// `format` で 1 行を組んだときの高さ（ピクセル、切り上げ）。
fn line_height(dwrite: &IDWriteFactory, format: &IDWriteTextFormat) -> windows::core::Result<u32> {
    let text: Vec<u16> = LINE_HEIGHT_REFERENCE.encode_utf16().collect();
    // SAFETY: text と format はこの呼び出しの間は生きている。
    let layout = unsafe { dwrite.CreateTextLayout(&text, format, f32::MAX, f32::MAX)? };
    let mut metrics = DWRITE_TEXT_METRICS::default();
    // SAFETY: metrics は書き込める。
    unsafe { layout.GetMetrics(&mut metrics)? };
    Ok((metrics.height.ceil() as u32).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{font, tab_bar::Measure};

    #[test]
    fn the_message_font_follows_the_dpi() {
        let dwrite = font::create_factory().unwrap();
        let base = UiFont::for_dpi(&dwrite, 96).unwrap();
        let doubled = UiFont::for_dpi(&dwrite, 192).unwrap();
        assert!(!base.family.is_empty());
        assert_eq!(base.family, doubled.family);
        assert!((doubled.size_px - base.size_px * 2.0).abs() <= 1.0);
        assert!(doubled.height() >= base.height() * 2 - 1);
        // 等幅でなくてよい。
        let narrow = base.width("iiii");
        let wide = base.width("MMMM");
        assert!(0 < narrow && narrow < wide);
        // 覚えた幅を返す。
        assert_eq!(base.width("MMMM"), wide);
    }
}
