//! タブのアイコンのピクセルを作る。画像のファイルと本物のアイコンを WIC で読み、読めなければ角を丸めた淡い色の
//! 正方形に接続方式ごとの絵と頭文字を Direct2D で描く。

use std::f32::consts::PI;

use fantermo_core::{config::BackendKind, frame::Rgb};
use windows::Win32::Graphics::{
    Direct2D::{
        Common::{
            D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
        },
        D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES,
        D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_FACTORY_TYPE_SINGLE_THREADED,
        D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES,
        D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
        D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1Factory1, ID2D1RenderTarget,
        ID2D1SolidColorBrush,
    },
    DirectWrite::{
        DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD, IDWriteFactory, IDWriteTextLayout,
    },
    Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
    Imaging::{GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnDemand},
};
use windows_numerics::{Matrix3x2, Vector2};

use super::{TabIcon, rgb};
use crate::{
    boxdraw::{Shape, arc},
    d2d_shape, font,
    image_decoder::{Decoded, Decoder},
};

/// ペンギンの腹の白。
const LIGHT_INK: Rgb = rgb(0xff, 0xff, 0xff);

/// 絵と頭文字の濃い色。
pub(super) const DARK_INK: Rgb = rgb(0x1a, 0x1c, 0x20);

/// ペンギンのくちばしの色。
const BEAK_INK: Rgb = rgb(0xf5, 0xb8, 0x1e);

/// 正方形の角の半径（一辺に対する割合）。
const CORNER_RATIO: f32 = 0.2;

/// 頭文字があるときの絵の左上の位置と一辺（アイコンの一辺に対する割合）。背景の正方形が大きく見えるように、
/// 絵を小さめに置く。
const PICTURE_WITH_INITIAL: (f32, f32) = (0.06, 0.5);

/// 頭文字がないときの絵のまわりの余白（アイコンの一辺に対する割合）。絵を大きく置き、背景の正方形は縁として
/// 見える程度にする。
const PICTURE_ALONE_MARGIN: f32 = 0.06;

/// 頭文字を収める範囲の右下の角からの一辺（アイコンの一辺に対する割合）。
const INITIAL_SIDE: f32 = 0.52;

/// 頭文字の範囲の外側の、アイコンの右下の角からの空き（アイコンの一辺に対する割合）。
const INITIAL_INSET: f32 = 0.04;

/// 頭文字のインクを範囲に合わせて広げるときの、フォントの大きさの上限（範囲の一辺に対する割合）。
/// 「.」のようにインクの小さい文字が大きくなりすぎないようにする。
const MAX_INITIAL_FONT_RATIO: f32 = 1.5;

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
    if has_initial {
        let (offset, side) = PICTURE_WITH_INITIAL;
        return Square {
            x: (size * offset).round(),
            y: (size * offset).round(),
            side: (size * side).round(),
        };
    }
    let margin = (size * PICTURE_ALONE_MARGIN).round().max(1.0);
    Square {
        x: margin,
        y: margin,
        side: size - 2.0 * margin,
    }
}

/// 一辺 `size` のアイコンの中の、絵の一辺。
fn picture_side(size: u32, has_initial: bool) -> u32 {
    picture_square(size as f32, has_initial).side as u32
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

/// 接続方式 `backend` の絵を、一辺 `side` の正方形に描く形。座標は正方形の左上から。
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
            Ink::Dark,
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
            Ink::Dark,
            Shape::rect(side * 0.3, bottom - inset, side * 0.7, bottom + inset),
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
        Ink::Dark,
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
    let polygon = |ink: Ink, points: Vec<(f32, f32)>| (ink, Shape::polygon(points));
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

/// タブのアイコンのピクセルを作るもの。画像のファイルと本物のアイコンを WIC で読み、自動で作る絵を
/// Direct2D で描く。Direct3D のデバイスは使わない。COM を初期化したスレッドで作って使う。
pub struct Generator {
    decoder: Decoder,
    d2d: ID2D1Factory1,
    dwrite: IDWriteFactory,
}

impl Generator {
    pub fn new(dwrite: &IDWriteFactory) -> windows::core::Result<Self> {
        // SAFETY: 既定の設定でファクトリを作るだけ。
        let d2d = unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        Ok(Self {
            decoder: Decoder::new()?,
            d2d,
            dwrite: dwrite.clone(),
        })
    }

    /// `icon` の一辺 `side` のピクセル。頭文字は書体 `family` で描く。画像が読めなければ、警告して自動で作る。
    /// 自動で作るときは、本物のアイコンを読めればそれを背景の正方形に載せ、読めなければ自前の絵を描く。
    pub fn pixels(
        &self,
        icon: &TabIcon,
        side: u32,
        family: &str,
    ) -> windows::core::Result<Decoded> {
        if let Some(path) = &icon.image {
            match self.decoder.decode_icon(path, side) {
                Ok(decoded) => {
                    tracing::info!("loaded the tab icon {} at {side} px", path.display());
                    return Ok(decoded);
                }
                Err(e) => {
                    tracing::warn!(
                        "failed to read the tab icon {}, generating one: {e}",
                        path.display()
                    );
                }
            }
        }
        let picture = icon.real.as_ref().and_then(|real| {
            self.decoder
                .decode_real_icon(real, picture_side(side, icon.initial.is_some()))
                .inspect_err(|e| {
                    tracing::warn!("failed to read the icon {real:?}, drawing one: {e}");
                })
                .ok()
        });
        let font = InitialFont {
            dwrite: &self.dwrite,
            family,
        };
        draw_generated(
            self.decoder.factory(),
            &self.d2d,
            &font,
            icon,
            picture.as_ref(),
            side,
        )
    }

    /// 作ったピクセル `pixels` の右下に、管理者の印の UAC の盾を重ねる。
    pub fn overlay_shield(&self, pixels: &mut Decoded) -> windows::core::Result<()> {
        self.decoder.overlay_shield(pixels)
    }
}

/// 頭文字を組む書体。
struct InitialFont<'a> {
    dwrite: &'a IDWriteFactory,
    family: &'a str,
}

/// `icon` の画像を使わない形の絵を、一辺 `size` ピクセルの乗算済みアルファの BGRA に描く。`picture` は読んだ
/// 本物のアイコン（一辺は [`picture_side`]）で、なければ接続方式の絵を描く。
fn draw_generated(
    wic: &IWICImagingFactory,
    factory: &ID2D1Factory1,
    font: &InitialFont,
    icon: &TabIcon,
    picture: Option<&Decoded>,
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
        dpiX: d2d_shape::PIXEL_DPI,
        dpiY: d2d_shape::PIXEL_DPI,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };
    // SAFETY: bitmap は作ったばかりの 32bpp の乗算済み BGRA で、properties は初期化済み。
    let target = unsafe { factory.CreateWicBitmapRenderTarget(&bitmap, &properties)? };
    // SAFETY: target は有効で、色は呼び出しの間は生きている。
    let brush = unsafe { target.CreateSolidColorBrush(&d2d_color(DARK_INK), None)? };
    // SAFETY: target は有効。BeginDraw と EndDraw を対にする。
    let ended = unsafe {
        target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        target.BeginDraw();
        let drawn = draw_parts(&target, factory, font, &brush, icon, picture, size as f32);
        drawn.and(target.EndDraw(None, None))
    };
    ended?;
    drop(target);
    let mut decoded = Decoded::blank(size, size);
    // SAFETY: 描き終えたビットマップから、stride × 高さのバイトへ写す。
    unsafe { bitmap.CopyPixels(std::ptr::null(), decoded.stride(), decoded.pixels_mut())? };
    Ok(decoded)
}

/// 正方形、絵、頭文字の順に描く。
fn draw_parts(
    target: &ID2D1RenderTarget,
    factory: &ID2D1Factory1,
    font: &InitialFont,
    brush: &ID2D1SolidColorBrush,
    icon: &TabIcon,
    picture: Option<&Decoded>,
    size: f32,
) -> windows::core::Result<()> {
    let tile = Square {
        x: 0.0,
        y: 0.0,
        side: size,
    };
    fill_rounded(target, brush, tile, icon.color);
    let area = picture_square(size, icon.initial.is_some());
    match picture {
        Some(picture) => draw_picture(target, picture, area)?,
        None => draw_backend_picture(target, factory, brush, icon.backend, area)?,
    }
    if let Some(initial) = &icon.initial {
        let area = initial_square(size);
        draw_initial(target, font, brush, initial, area)?;
    }
    Ok(())
}

/// 読んだアイコン `picture` を `area` に、透過を保って描く。
fn draw_picture(
    target: &ID2D1RenderTarget,
    picture: &Decoded,
    area: Square,
) -> windows::core::Result<()> {
    let properties = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: d2d_shape::PIXEL_DPI,
        dpiY: d2d_shape::PIXEL_DPI,
    };
    let size = D2D_SIZE_U {
        width: picture.width(),
        height: picture.height(),
    };
    // SAFETY: 描画中の target で、ピクセルと properties は呼び出しの間は生きている。ピクセルは一行
    // stride バイトで height 行ある（`Decoded` が長さを保証する）。
    unsafe {
        let bitmap = target.CreateBitmap(
            size,
            Some(picture.pixels().as_ptr().cast()),
            picture.stride(),
            &properties,
        )?;
        target.DrawBitmap(
            &bitmap,
            Some(&area.rect()),
            1.0,
            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
            None,
        );
    }
    Ok(())
}

/// 接続方式 `backend` の絵を `area` に描く。
fn draw_backend_picture(
    target: &ID2D1RenderTarget,
    factory: &ID2D1Factory1,
    brush: &ID2D1SolidColorBrush,
    backend: BackendKind,
    area: Square,
) -> windows::core::Result<()> {
    // SAFETY: 描画中の target に変換を設定するだけ。
    unsafe { target.SetTransform(&Matrix3x2::translation(area.x, area.y)) };
    for (ink, shape) in picture_shapes(backend, area.side) {
        // SAFETY: brush は target が作ったブラシで、色は呼び出しの間は生きている。
        unsafe { brush.SetColor(&d2d_color(ink.color())) };
        d2d_shape::draw_shape(target, factory, brush, &shape)?;
    }
    // SAFETY: 描画中の target の変換を戻すだけ。
    unsafe { target.SetTransform(&Matrix3x2::identity()) };
    Ok(())
}

fn fill_rounded(
    target: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    area: Square,
    color: Rgb,
) {
    let radius = area.side * CORNER_RATIO;
    // SAFETY: 描画中の target と、同じ target が作ったブラシ。値は呼び出しの間は生きている。
    unsafe {
        brush.SetColor(&d2d_color(color));
        target.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: area.rect(),
                radiusX: radius,
                radiusY: radius,
            },
            brush,
        );
    }
}

/// 太字で組んだもののインクの範囲（左・上・右・下）。組む範囲の左上が原点。
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
    let format = font::text_format(
        dwrite,
        family,
        font_size,
        DWRITE_FONT_WEIGHT_BOLD,
        DWRITE_FONT_STYLE_NORMAL,
    )?;
    // SAFETY: utf16 と format はこの呼び出しの間は生きている。
    unsafe { dwrite.CreateTextLayout(&utf16, &format, font_size, font_size) }
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
    font: &InitialFont,
    brush: &ID2D1SolidColorBrush,
    text: &str,
    area: Square,
) -> windows::core::Result<()> {
    let trial = initial_layout(font.dwrite, font.family, text, area.side)?;
    let font_size = fitted_font_size(area.side, ink_bounds(&trial)?, area.side);
    let layout = initial_layout(font.dwrite, font.family, text, font_size)?;
    let [left, top, right, bottom] = ink_bounds(&layout)?;
    let origin = Vector2 {
        X: (area.x + (area.side - (right - left)) / 2.0 - left).round(),
        Y: (area.y + (area.side - (bottom - top)) / 2.0 - top).round(),
    };
    // SAFETY: 描画中の target と、同じ target が作ったブラシ。layout はこの呼び出しの間は生きている。
    unsafe {
        brush.SetColor(&d2d_color(DARK_INK));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 形の点が一辺 `side` の正方形の中にある。線の太さの分だけはみ出すものは含めて確認する。
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
        // Cygwin は線一本の C。WSL は濃い色と白とくちばしの色を使う。ConPTY は線と下線。
        assert_eq!(shapes(BackendKind::Cygwin).len(), 1);
        let inks = |backend| -> Vec<Ink> { shapes(backend).iter().map(|(ink, _)| *ink).collect() };
        assert_eq!(
            inks(BackendKind::Wsl),
            [Ink::Dark, Ink::Dark, Ink::Light, Ink::Beak]
        );
        assert_eq!(inks(BackendKind::Conpty), [Ink::Dark, Ink::Dark]);
        assert_eq!(inks(BackendKind::Cygwin), [Ink::Dark]);
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
        for size in [18.0, 27.0, 36.0] {
            // 頭文字がなければ、絵は真ん中に大きく、背景の正方形は縁として残る。
            let alone = picture_square(size, false);
            assert_eq!(alone.x * 2.0 + alone.side, size);
            assert!(alone.x >= 1.0, "{size}");
            assert!(alone.side / size >= 0.8, "{size}");
            // 頭文字があれば、絵を小さく置いて、背景の正方形を大きく見せる。
            let with_initial = picture_square(size, true);
            assert!(with_initial.side <= size * 0.5 + 0.5, "{size}");
            assert!(alone.side > with_initial.side, "{size}");
        }
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
}
