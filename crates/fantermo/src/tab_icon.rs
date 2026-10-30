//! タブのタイトルの左に出す、小さな正方形のアイコン。
//!
//! プロファイルに画像を書いていなければ、角を丸めた色の正方形に、つなぎ方ごとの絵（Cygwin の C、WSL の
//! ペンギン、ConPTY の `>_`）とプロファイル名の頭文字を描いて作る。色はプロファイル名から決まるので、
//! 起動のたびに同じになる。作った絵は乗算済みアルファの BGRA（[`Decoded`]）で返す。

use std::{
    f32::consts::PI,
    path::{Path, PathBuf},
};

use fantermo_core::{
    config::{BackendKind, Profile},
    frame::Rgb,
    vt,
};
use windows::{
    Win32::Graphics::{
        Direct2D::{
            Common::{D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT},
            D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_FEATURE_LEVEL_DEFAULT,
            D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
            ID2D1Factory1, ID2D1RenderTarget, ID2D1SolidColorBrush,
        },
        DirectWrite::{
            DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_WORD_WRAPPING_NO_WRAP, IDWriteFactory, IDWriteTextLayout,
        },
        Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
        Imaging::{GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnDemand},
    },
    core::{HSTRING, w},
};
use windows_numerics::{Matrix3x2, Vector2};

use crate::{atlas, background::Decoded, boxdraw::Shape, paint::arc};

/// プロファイル名から選ぶ背景の色。白の文字と絵が読め、ライトとダークのどちらのタブの上でも沈まない
/// 明るさの色だけを並べる（テストで固定する）。
const PALETTE: [Rgb; 10] = [
    rgb(0xc6, 0x28, 0x28),
    rgb(0xc2, 0x57, 0x0c),
    rgb(0x9a, 0x67, 0x00),
    rgb(0x2e, 0x7d, 0x32),
    rgb(0x0f, 0x76, 0x6e),
    rgb(0x0e, 0x74, 0x90),
    rgb(0x1d, 0x6f, 0xd1),
    rgb(0x4f, 0x46, 0xe5),
    rgb(0x7e, 0x3f, 0xc4),
    rgb(0xbe, 0x2a, 0x78),
];

/// プロファイルなしで開いたタブの背景の色。
const NO_PROFILE_COLOR: Rgb = rgb(0x55, 0x60, 0x70);

/// 絵と頭文字の白。
const LIGHT_INK: Rgb = rgb(0xff, 0xff, 0xff);

/// ペンギンの体の色。
const DARK_INK: Rgb = rgb(0x1a, 0x1c, 0x20);

/// ペンギンのくちばしの色。
const BEAK_INK: Rgb = rgb(0xf5, 0xb8, 0x1e);

/// 正方形の角の半径（一辺に対する割合）。
const CORNER_RATIO: f32 = 0.2;

/// 頭文字があるときの絵の左上の位置と一辺（アイコンの一辺に対する割合）。
const PICTURE_WITH_INITIAL: (f32, f32) = (0.06, 0.56);

/// 頭文字がないときの絵の左上の位置と一辺（アイコンの一辺に対する割合）。
const PICTURE_ALONE: (f32, f32) = (0.18, 0.64);

/// 頭文字を収める範囲の右下の角からの一辺（アイコンの一辺に対する割合）。
const INITIAL_SIDE: f32 = 0.52;

/// 頭文字の範囲の外側の、アイコンの右下の角からの空き（アイコンの一辺に対する割合）。
const INITIAL_INSET: f32 = 0.04;

/// 頭文字のインクを範囲に合わせて広げるときの、フォントの大きさの上限（範囲の一辺に対する割合）。
/// 「.」のようにインクの小さい文字が大きくなりすぎないようにする。
const MAX_INITIAL_FONT_RATIO: f32 = 1.5;

/// Direct2D の座標をピクセルと同じにする DPI。
const PIXEL_DPI: f32 = 96.0;

/// 文字を組むときのロケール。和文の字形を選ばせる。
const LOCALE: windows::core::PCWSTR = w!("ja-jp");

const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

/// タブのアイコン。タブを開いたときに決まり、プロファイルを変えても消しても変わらない。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TabIcon {
    /// 画像のファイル。読めなければ下の値で作る。
    pub image: Option<PathBuf>,
    pub backend: BackendKind,
    /// 絵に添える頭文字。
    pub initial: Option<String>,
    /// 背景の色。
    pub color: Rgb,
}

impl TabIcon {
    /// プロファイルなしで開いたタブ（fantermo の引数のコマンドライン）のアイコン。
    pub const fn without_profile() -> Self {
        Self {
            image: None,
            backend: BackendKind::Conpty,
            initial: None,
            color: NO_PROFILE_COLOR,
        }
    }

    /// プロファイル `profile` のアイコン。相対パスの画像は `config_dir` から辿る。
    pub fn for_profile(profile: &Profile, config_dir: Option<&Path>) -> Self {
        let image = profile.icon.as_deref().map(|path| match config_dir {
            Some(folder) => folder.join(path),
            None => PathBuf::from(path),
        });
        Self {
            image,
            backend: profile.backend,
            initial: initial(&profile.name),
            color: color_of(&profile.name),
        }
    }
}

/// プロファイル名 `name` の頭文字。空白を除いた最初の書記素クラスタ（結合文字やサロゲートペアを分けない）で、
/// 大文字があれば大文字にする。幅のあるものがなければ `None`。
pub fn initial(name: &str) -> Option<String> {
    let grapheme = vt::split_graphemes(name)
        .into_iter()
        .find(|grapheme| grapheme.width > 0 && !grapheme.text.trim().is_empty())?;
    let upper = grapheme.text.to_uppercase();
    // 「ß」のように文字の数が変わるものは、一文字に見えなくなるのでそのまま。
    Some(if upper.chars().count() == grapheme.text.chars().count() {
        upper
    } else {
        grapheme.text
    })
}

/// プロファイル名 `name` の背景の色。
pub fn color_of(name: &str) -> Rgb {
    PALETTE[hash(name) as usize % PALETTE.len()]
}

/// FNV-1a。実行のたびにも Rust の版でも変わらないようにする。
fn hash(text: &str) -> u32 {
    const OFFSET_BASIS: u32 = 0x811c_9dc5;
    const PRIME: u32 = 0x0100_0193;
    text.bytes().fold(OFFSET_BASIS, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(PRIME)
    })
}

/// 正方形の範囲。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Square {
    x: f32,
    y: f32,
    side: f32,
}

impl Square {
    fn rect(self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.x,
            top: self.y,
            right: self.x + self.side,
            bottom: self.y + self.side,
        }
    }
}

/// 一辺 `size` のアイコンの中の、絵の範囲。ピクセルの境目に合わせる。
fn picture_square(size: f32, has_initial: bool) -> Square {
    let (offset, side) = if has_initial {
        PICTURE_WITH_INITIAL
    } else {
        PICTURE_ALONE
    };
    Square {
        x: (size * offset).round(),
        y: (size * offset).round(),
        side: (size * side).round(),
    }
}

/// 一辺 `size` のアイコンの中の、頭文字の範囲。右下の角に寄せ、ピクセルの境目に合わせる。
fn initial_square(size: f32) -> Square {
    let side = (size * INITIAL_SIDE).round();
    let corner = size - (size * INITIAL_INSET).round();
    Square {
        x: corner - side,
        y: corner - side,
        side,
    }
}

/// 絵を描く色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ink {
    Light,
    Dark,
    Beak,
}

impl Ink {
    fn color(self) -> Rgb {
        match self {
            Ink::Light => LIGHT_INK,
            Ink::Dark => DARK_INK,
            Ink::Beak => BEAK_INK,
        }
    }
}

/// つなぎ方 `backend` の絵を、一辺 `side` の正方形に描く形。座標は正方形の左上から。
fn picture_shapes(backend: BackendKind, side: f32) -> Vec<(Ink, Shape)> {
    match backend {
        BackendKind::Conpty => prompt_shapes(side),
        BackendKind::Cygwin => letter_c_shapes(side),
        BackendKind::Wsl => penguin_shapes(side),
    }
}

/// 線の太さ。小さくても 1 ピクセル半は残して見分けられるようにする。
fn stroke(side: f32) -> f32 {
    (side * 0.2).max(1.5)
}

/// `>_`。
fn prompt_shapes(side: f32) -> Vec<(Ink, Shape)> {
    let width = stroke(side);
    let inset = width / 2.0;
    let bottom = side - inset;
    vec![
        (
            Ink::Light,
            Shape::Polyline {
                points: vec![
                    (inset, inset),
                    (side * 0.5, side * 0.34),
                    (inset, side * 0.68 - inset),
                ],
                width,
            },
        ),
        (
            Ink::Light,
            Shape::Rect {
                left: side * 0.3,
                top: bottom - inset,
                right: side * 0.7,
                bottom: bottom + inset,
                alpha: 1.0,
            },
        ),
    ]
}

/// 右が開いた円弧の C。
fn letter_c_shapes(side: f32) -> Vec<(Ink, Shape)> {
    /// 開いている部分の半分の角度（ラジアン）。
    const OPENING: f32 = PI / 4.0;
    let width = stroke(side);
    let center = (side / 2.0, side / 2.0);
    let radius = (side - width) / 2.0;
    vec![(
        Ink::Light,
        Shape::Polyline {
            points: arc(center, radius, OPENING, 2.0 * PI - OPENING),
            width,
        },
    )]
}

/// 正面を向いたペンギン。暗い体に白い腹、黄色いくちばし。
fn penguin_shapes(side: f32) -> Vec<(Ink, Shape)> {
    let ellipse = |center: (f32, f32), radius: (f32, f32)| -> Vec<(f32, f32)> {
        arc((0.0, 0.0), 1.0, 0.0, 2.0 * PI)
            .into_iter()
            .map(|(x, y)| (center.0 + x * radius.0, center.1 + y * radius.1))
            .collect()
    };
    let polygon = |ink: Ink, points: Vec<(f32, f32)>| (ink, Shape::Polygon { points, alpha: 1.0 });
    let at = |x: f32, y: f32| (side * x, side * y);
    vec![
        polygon(
            Ink::Dark,
            ellipse(at(0.5, 0.26), (side * 0.26, side * 0.26)),
        ),
        polygon(
            Ink::Dark,
            ellipse(at(0.5, 0.64), (side * 0.40, side * 0.36)),
        ),
        polygon(
            Ink::Light,
            ellipse(at(0.5, 0.68), (side * 0.24, side * 0.27)),
        ),
        polygon(
            Ink::Beak,
            vec![at(0.32, 0.26), at(0.68, 0.26), at(0.5, 0.46)],
        ),
    ]
}

/// `icon` の画像を使わない形の絵を、一辺 `size` ピクセルの乗算済みアルファの BGRA に描く。
pub fn draw_generated(
    wic: &IWICImagingFactory,
    factory: &ID2D1Factory1,
    dwrite: &IDWriteFactory,
    family: &str,
    icon: &TabIcon,
    size: u32,
) -> windows::core::Result<Decoded> {
    // SAFETY: 作ったばかりの WIC のビットマップを描画先にする。出力先は呼び出しの間は生きている。
    let bitmap = unsafe {
        wic.CreateBitmap(
            size,
            size,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapCacheOnDemand,
        )?
    };
    let properties = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: PIXEL_DPI,
        dpiY: PIXEL_DPI,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };
    // SAFETY: bitmap は作ったばかりの 32bpp の乗算済み BGRA で、properties は初期化済み。
    let target = unsafe { factory.CreateWicBitmapRenderTarget(&bitmap, &properties)? };
    // SAFETY: target は有効で、色は呼び出しの間は生きている。
    let brush = unsafe { target.CreateSolidColorBrush(&d2d_color(LIGHT_INK), None)? };
    // SAFETY: target は有効。BeginDraw と EndDraw を対にする。
    let ended = unsafe {
        target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        target.BeginDraw();
        let drawn = draw_parts(&target, factory, dwrite, family, &brush, icon, size as f32);
        drawn.and(target.EndDraw(None, None))
    };
    ended?;
    drop(target);
    let mut decoded = Decoded {
        width: size,
        height: size,
        pixels: Vec::new(),
    };
    decoded.pixels = vec![0; (decoded.stride() * size) as usize];
    // SAFETY: 描き終えたビットマップから、stride × 高さのバイトへ写す。
    unsafe { bitmap.CopyPixels(std::ptr::null(), decoded.stride(), &mut decoded.pixels)? };
    Ok(decoded)
}

/// 正方形、絵、頭文字の順に描く。
fn draw_parts(
    target: &ID2D1RenderTarget,
    factory: &ID2D1Factory1,
    dwrite: &IDWriteFactory,
    family: &str,
    brush: &ID2D1SolidColorBrush,
    icon: &TabIcon,
    size: f32,
) -> windows::core::Result<()> {
    let tile = Square {
        x: 0.0,
        y: 0.0,
        side: size,
    };
    fill_rounded(target, brush, tile, icon.color, 1.0);
    let picture = picture_square(size, icon.initial.is_some());
    // SAFETY: 描画中の target に変換を設定するだけ。
    unsafe { target.SetTransform(&Matrix3x2::translation(picture.x, picture.y)) };
    for (ink, shape) in picture_shapes(icon.backend, picture.side) {
        // SAFETY: brush は target が作ったブラシで、色は呼び出しの間は生きている。
        unsafe { brush.SetColor(&d2d_color(ink.color())) };
        atlas::draw_shape(target, factory, brush, &shape)?;
    }
    // SAFETY: 描画中の target の変換を戻すだけ。
    unsafe { target.SetTransform(&Matrix3x2::identity()) };
    if let Some(initial) = &icon.initial {
        let area = initial_square(size);
        draw_initial(target, dwrite, family, brush, initial, area)?;
    }
    Ok(())
}

fn fill_rounded(
    target: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    area: Square,
    color: Rgb,
    opacity: f32,
) {
    let radius = area.side * CORNER_RATIO;
    // SAFETY: 描画中の target と、同じ target が作ったブラシ。値は呼び出しの間は生きている。
    unsafe {
        brush.SetColor(&d2d_color(color));
        brush.SetOpacity(opacity);
        target.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: area.rect(),
                radiusX: radius,
                radiusY: radius,
            },
            brush,
        );
        brush.SetOpacity(1.0);
    }
}

/// `text` を太字で組んだもののインクの範囲（左・上・右・下）。組む範囲の左上が原点。
fn ink_bounds(layout: &IDWriteTextLayout) -> windows::core::Result<[f32; 4]> {
    // SAFETY: layout は有効。
    let (overhang, width, height) = unsafe {
        (
            layout.GetOverhangMetrics()?,
            layout.GetMaxWidth(),
            layout.GetMaxHeight(),
        )
    };
    Ok([
        -overhang.left,
        -overhang.top,
        width + overhang.right,
        height + overhang.bottom,
    ])
}

/// 太字の `text` を、大きさ `font_size` ピクセルの書体で 1 行に組む。
fn initial_layout(
    dwrite: &IDWriteFactory,
    family: &str,
    text: &str,
    font_size: f32,
) -> windows::core::Result<IDWriteTextLayout> {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: 文字列はこの呼び出しの間は生きている。
    unsafe {
        let format = dwrite.CreateTextFormat(
            &HSTRING::from(family),
            None,
            DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            font_size,
            LOCALE,
        )?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        dwrite.CreateTextLayout(&utf16, &format, font_size, font_size)
    }
}

/// インクの幅と高さが `ink` のとき、一辺 `side` に収めるフォントの大きさ。`font_size` で測った値。
fn fitted_font_size(font_size: f32, ink: [f32; 4], side: f32) -> f32 {
    let [left, top, right, bottom] = ink;
    let larger = (right - left).max(bottom - top);
    if larger <= 0.0 {
        return font_size;
    }
    (font_size * side / larger).min(side * MAX_INITIAL_FONT_RATIO)
}

/// `text` を、インクが `area` の真ん中に来るように、`area` に収まる大きさで描く。
fn draw_initial(
    target: &ID2D1RenderTarget,
    dwrite: &IDWriteFactory,
    family: &str,
    brush: &ID2D1SolidColorBrush,
    text: &str,
    area: Square,
) -> windows::core::Result<()> {
    let trial = initial_layout(dwrite, family, text, area.side)?;
    let font_size = fitted_font_size(area.side, ink_bounds(&trial)?, area.side);
    let layout = initial_layout(dwrite, family, text, font_size)?;
    let [left, top, right, bottom] = ink_bounds(&layout)?;
    let origin = Vector2 {
        X: (area.x + (area.side - (right - left)) / 2.0 - left).round(),
        Y: (area.y + (area.side - (bottom - top)) / 2.0 - top).round(),
    };
    // SAFETY: 描画中の target と、同じ target が作ったブラシ。layout はこの呼び出しの間は生きている。
    unsafe {
        brush.SetColor(&d2d_color(LIGHT_INK));
        target.DrawTextLayout(
            origin,
            &layout,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
        );
    }
    Ok(())
}

fn d2d_color(color: Rgb) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: 1.0,
    }
}

/// `sizes` の大きさのコマのうち、一辺 `side` のアイコンに使うものの位置。`side` を覆える最も小さいコマ、
/// なければ最も大きいコマ。同じ大きさなら前のもの。
pub fn best_frame(sizes: &[(u32, u32)], side: u32) -> usize {
    let shorter = |size: &(u32, u32)| size.0.min(size.1);
    let covering = sizes
        .iter()
        .enumerate()
        .filter(|(_, size)| shorter(size) >= side)
        .min_by_key(|(_, size)| shorter(size));
    // `max_by_key` は同じ値なら後ろを返すので、位置の逆順で前のものにする。
    let largest = sizes
        .iter()
        .enumerate()
        .rev()
        .max_by_key(|(_, size)| shorter(size));
    covering.or(largest).map_or(0, |(index, _)| index)
}

/// 大きさ `image` の画像を、縦横比を保って一辺 `side` の正方形に収めたときの幅と高さ（少なくとも 1）と、
/// 真ん中に置くときの左上。
pub fn fit_in_square(image: (u32, u32), side: u32) -> (u32, u32, u32, u32) {
    let (width, height) = image;
    let (fitted_width, fitted_height) = if width >= height {
        (side, scaled(height, side, width))
    } else {
        (scaled(width, side, height), side)
    };
    (
        fitted_width,
        fitted_height,
        (side - fitted_width) / 2,
        (side - fitted_height) / 2,
    )
}

/// `value * numerator / denominator` を四捨五入して、少なくとも 1。
fn scaled(value: u32, numerator: u32, denominator: u32) -> u32 {
    let numerator = u64::from(numerator);
    let denominator = u64::from(denominator).max(1);
    let result = (u64::from(value) * numerator + denominator / 2) / denominator;
    result.clamp(1, numerator) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 相対輝度（WCAG）。
    fn luminance(color: Rgb) -> f32 {
        let channel = |value: u8| {
            let value = f32::from(value) / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
    }

    /// コントラスト比（WCAG）。
    fn contrast(a: Rgb, b: Rgb) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn every_color_reads_on_light_and_dark_tabs() {
        // タブの色: 既定の表示しているタブ、既定のタブバー、ダークの表示しているタブ、ダークのタブバー。
        const LIGHT_TABS: [Rgb; 2] = [rgb(0xff, 0xff, 0xff), rgb(0xcc, 0xcc, 0xcc)];
        const DARK_TABS: [Rgb; 2] = [rgb(0x1e, 0x1e, 0x1e), rgb(0x30, 0x30, 0x30)];
        for color in PALETTE.iter().chain([&NO_PROFILE_COLOR]) {
            assert!(contrast(*color, LIGHT_INK) >= 3.5, "{color:?} and its ink");
            assert!(
                contrast(*color, DARK_INK) >= 2.0,
                "{color:?} and the penguin"
            );
            for tab in LIGHT_TABS {
                assert!(contrast(*color, tab) >= 2.0, "{color:?} on {tab:?}");
            }
            for tab in DARK_TABS {
                assert!(contrast(*color, tab) >= 1.5, "{color:?} on {tab:?}");
            }
        }
    }

    #[test]
    fn the_palette_has_distinct_colors() {
        for (index, color) in PALETTE.iter().enumerate() {
            assert!(!PALETTE[..index].contains(color));
        }
    }

    #[test]
    fn a_name_always_gets_the_same_color() {
        // FNV-1a の値を固定する。変わると、利用者のタブの色が変わる。
        assert_eq!(hash(""), 0x811c_9dc5);
        assert_eq!(hash("a"), 0xe40c_292c);
        assert_eq!(hash("foobar"), 0xbf9c_f968);
        assert_eq!(color_of("foobar"), PALETTE[0xbf9c_f968_usize % 10]);
        assert_eq!(color_of("zsh"), color_of("zsh"));
    }

    #[test]
    fn names_spread_over_the_palette() {
        let names = [
            "PowerShell",
            "zsh",
            "Ubuntu",
            "Cygwin",
            "cmd",
            "Debian",
            "pwsh",
            "bash",
        ];
        let mut colors: Vec<Rgb> = names.iter().map(|name| color_of(name)).collect();
        colors.sort_by_key(|color| (color.r, color.g, color.b));
        colors.dedup();
        assert!(colors.len() >= 4, "{colors:?}");
    }

    #[test]
    fn the_initial_is_the_first_grapheme_cluster_in_upper_case() {
        assert_eq!(initial("zsh").as_deref(), Some("Z"));
        assert_eq!(initial("Ubuntu").as_deref(), Some("U"));
        assert_eq!(initial("  bash").as_deref(), Some("B"));
        // 全角の空白も飛ばす。
        assert_eq!(initial("\u{3000}端末").as_deref(), Some("端"));
        // 結合文字は前の文字から離さない。
        assert_eq!(initial("e\u{301}x").as_deref(), Some("E\u{301}"));
        // サロゲートペアになる文字を半分にしない。
        assert_eq!(initial("𠮷野家").as_deref(), Some("𠮷"));
        assert_eq!(initial("😀 emoji").as_deref(), Some("😀"));
        // 肌の色の修飾子と ZWJ の並びも一つ。
        assert_eq!(initial("👍🏽ok").as_deref(), Some("👍🏽"));
        // 大文字にすると文字の数が変わるものはそのまま。
        assert_eq!(initial("ßeta").as_deref(), Some("ß"));
        assert_eq!(initial(""), None);
        assert_eq!(initial("   "), None);
        // 幅のない文字だけなら頭文字はない。
        assert_eq!(initial("\u{7}"), None);
        assert_eq!(initial("\u{7}a").as_deref(), Some("A"));
    }

    #[test]
    fn profiles_decide_the_icon() {
        let profile = Profile {
            icon: Some(r"icons\zsh.png".to_owned()),
            ..Profile::new("zsh", "zsh -l", BackendKind::Cygwin)
        };
        let icon = TabIcon::for_profile(&profile, Some(Path::new(r"C:\conf")));
        assert_eq!(icon.image, Some(PathBuf::from(r"C:\conf\icons\zsh.png")));
        assert_eq!(icon.backend, BackendKind::Cygwin);
        assert_eq!(icon.initial.as_deref(), Some("Z"));
        assert_eq!(icon.color, color_of("zsh"));
        // 絶対パスは設定ファイルのフォルダによらない。
        let absolute = Profile {
            icon: Some(r"D:\pics\a.png".to_owned()),
            ..profile.clone()
        };
        assert_eq!(
            TabIcon::for_profile(&absolute, Some(Path::new(r"C:\conf"))).image,
            Some(PathBuf::from(r"D:\pics\a.png"))
        );
        assert_eq!(
            TabIcon::for_profile(&profile, None).image,
            Some(PathBuf::from(r"icons\zsh.png"))
        );
        let plain = TabIcon::for_profile(&Profile::builtin(), None);
        assert_eq!(plain.image, None);
        assert_eq!(plain.backend, BackendKind::Conpty);
    }

    #[test]
    fn tabs_without_a_profile_have_no_initial() {
        let icon = TabIcon::without_profile();
        assert_eq!(icon.backend, BackendKind::Conpty);
        assert_eq!(icon.initial, None);
        assert_eq!(icon.image, None);
        assert!(!PALETTE.contains(&icon.color));
    }

    /// 形の点が一辺 `side` の正方形の中にある。線の太さの分だけはみ出すものは含めて確かめる。
    fn assert_inside(shape: &Shape, side: f32) {
        const EPSILON: f32 = 1e-3;
        let check = |points: &[(f32, f32)], margin: f32| {
            for &(x, y) in points {
                assert!(
                    x - margin >= -EPSILON && x + margin <= side + EPSILON,
                    "{x}"
                );
                assert!(
                    y - margin >= -EPSILON && y + margin <= side + EPSILON,
                    "{y}"
                );
            }
        };
        match shape {
            Shape::Polygon { points, .. } => check(points, 0.0),
            Shape::Polyline { points, width } => check(points, width / 2.0),
            Shape::Rect {
                left,
                top,
                right,
                bottom,
                ..
            } => check(&[(*left, *top), (*right, *bottom)], 0.0),
            other => panic!("unexpected shape: {other:?}"),
        }
    }

    #[test]
    fn pictures_stay_inside_their_squares() {
        for backend in BackendKind::ALL {
            for side in [8.0, 11.0, 16.0, 32.0] {
                for (_, shape) in picture_shapes(backend, side) {
                    assert_inside(&shape, side);
                }
            }
        }
    }

    #[test]
    fn pictures_differ_by_backend() {
        let shapes = |backend| picture_shapes(backend, 16.0);
        // Cygwin は線一本の C。WSL は暗い色と白とくちばしの色を使う。ConPTY は線と下線。
        assert_eq!(shapes(BackendKind::Cygwin).len(), 1);
        let inks = |backend| -> Vec<Ink> { shapes(backend).iter().map(|(ink, _)| *ink).collect() };
        assert_eq!(
            inks(BackendKind::Wsl),
            [Ink::Dark, Ink::Dark, Ink::Light, Ink::Beak]
        );
        assert_eq!(inks(BackendKind::Conpty), [Ink::Light, Ink::Light]);
    }

    #[test]
    fn the_c_opens_to_the_right() {
        let shapes = picture_shapes(BackendKind::Cygwin, 16.0);
        let (_, Shape::Polyline { points, .. }) = &shapes[0] else {
            panic!("not a line: {shapes:?}");
        };
        // 右の端の真ん中（角度 0）を通らない。
        assert!(
            points
                .iter()
                .all(|&(x, y)| !(x > 13.0 && (y - 8.0).abs() < 2.0))
        );
    }

    #[test]
    fn the_picture_and_the_initial_stay_in_the_icon() {
        for size in [12.0, 18.0, 23.0, 27.0, 36.0, 72.0] {
            for has_initial in [false, true] {
                let picture = picture_square(size, has_initial);
                assert!(
                    picture.x >= 0.0 && picture.x + picture.side <= size,
                    "{size}"
                );
                assert_eq!(picture.x.fract(), 0.0);
                assert_eq!(picture.side.fract(), 0.0);
            }
            let area = initial_square(size);
            assert!(area.x >= 0.0 && area.x + area.side <= size, "{size}");
            assert_eq!((area.x.fract(), area.side.fract()), (0.0, 0.0));
        }
        // 頭文字がなければ、絵は真ん中に大きく。
        let alone = picture_square(18.0, false);
        assert_eq!(alone.x * 2.0 + alone.side, 18.0);
        assert!(alone.side > picture_square(18.0, true).side);
    }

    #[test]
    fn the_initial_is_scaled_to_fit() {
        // 10 の範囲に、大きさ 10 で測ってインクが 5×8 の文字。高いほうを 10 にする。
        assert_eq!(fitted_font_size(10.0, [1.0, 1.0, 6.0, 9.0], 10.0), 12.5);
        // インクの小さい文字は上限で止める。
        assert_eq!(fitted_font_size(10.0, [0.0, 0.0, 1.0, 1.0], 10.0), 15.0);
        // 大きい文字は縮める。
        assert_eq!(fitted_font_size(10.0, [0.0, 0.0, 20.0, 10.0], 10.0), 5.0);
        // インクがなければそのまま。
        assert_eq!(fitted_font_size(10.0, [0.0, 0.0, 0.0, 0.0], 10.0), 10.0);
    }

    #[test]
    fn the_frame_nearest_above_the_size_is_chosen() {
        let ico = [(16, 16), (32, 32), (48, 48), (256, 256)];
        assert_eq!(best_frame(&ico, 18), 1);
        assert_eq!(best_frame(&ico, 16), 0);
        assert_eq!(best_frame(&ico, 27), 1);
        assert_eq!(best_frame(&ico, 33), 2);
        // 覆えるものがなければ最も大きいもの。
        assert_eq!(best_frame(&[(16, 16), (24, 24), (8, 8)], 36), 1);
        // 同じ大きさのコマ（アニメーションの GIF など）は最初のもの。
        assert_eq!(best_frame(&[(64, 64), (64, 64)], 18), 0);
        assert_eq!(best_frame(&[(8, 8), (8, 8)], 18), 0);
        assert_eq!(best_frame(&[], 18), 0);
    }

    #[test]
    fn images_keep_their_aspect_in_the_square() {
        assert_eq!(fit_in_square((64, 64), 18), (18, 18, 0, 0));
        assert_eq!(fit_in_square((64, 32), 18), (18, 9, 0, 4));
        assert_eq!(fit_in_square((32, 64), 18), (9, 18, 4, 0));
        // 小さい画像も広げる。
        assert_eq!(fit_in_square((16, 16), 27), (27, 27, 0, 0));
        // 細長い画像も消えない。
        assert_eq!(fit_in_square((1000, 1), 18), (18, 1, 0, 8));
    }
}
