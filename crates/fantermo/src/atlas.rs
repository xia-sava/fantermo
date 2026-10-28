//! グリフアトラス。DirectWrite で組んだ文字を Direct2D でテクスチャに描き、その場所を覚えておく。
//!
//! 文字は白で、乗算済みのアルファで描く。色はシェーダで付ける。場所は 1 セルの高さの棚に左から
//! 詰めていき、いっぱいになったら全部を捨てて描き直す。

use std::collections::HashMap;

use windows::{
    Win32::Graphics::{
        Direct2D::{
            Common::{
                D2D_RECT_F, D2D_SIZE_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
                D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
            },
            D2D1_ANTIALIAS_MODE_ALIASED, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_SMALL,
            D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
            D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_QUADRATIC_BEZIER_SEGMENT,
            D2D1_SWEEP_DIRECTION_CLOCKWISE, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
            D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1DeviceContext,
            ID2D1Factory1, ID2D1PathGeometry1, ID2D1SolidColorBrush,
        },
        Direct3D11::{
            D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11ShaderResourceView, ID3D11Texture2D,
        },
        DirectWrite::{
            DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE, DWRITE_FONT_STYLE_ITALIC,
            DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_FONT_WEIGHT_NORMAL, DWRITE_LINE_METRICS, DWRITE_TEXT_METRICS,
            DWRITE_WORD_WRAPPING_NO_WRAP, IDWriteFactory, IDWriteTextFormat,
        },
        Dxgi::{
            Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
            IDXGIDevice, IDXGISurface,
        },
    },
    core::{HSTRING, Interface, w},
};
use windows_numerics::{Matrix3x2, Vector2};

use crate::{
    boxdraw::{self, Shape},
    decoration::{self, Decoration, Placement},
    font::Font,
};

/// アトラスの一辺（ピクセル）。
const ATLAS_SIZE: u32 = 2048;

/// Direct2D の座標をピクセルと同じにする DPI。
const PIXEL_DPI: f32 = 96.0;

/// 文字を組むときのロケール。和文の字形を選ばせる。
const LOCALE: windows::core::PCWSTR = w!("ja-jp");

/// アトラスの中の場所（ピクセル）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// アトラスに空きがない。
#[derive(Debug)]
pub struct AtlasFull;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct GlyphKey {
    text: String,
    bold: bool,
    italic: bool,
    cells: u8,
}

pub struct GlyphAtlas {
    view: ID3D11ShaderResourceView,
    factory: ID2D1Factory1,
    context: ID2D1DeviceContext,
    brush: ID2D1SolidColorBrush,
    dwrite: IDWriteFactory,
    /// 太字・斜体の組み合わせごとの書式。添字は [`format_index`]。
    formats: [IDWriteTextFormat; 4],
    cell_width: u32,
    cell_height: u32,
    /// 罫線の細線の太さ（ピクセル）。
    line_thickness: f32,
    /// 装飾の線のセルの中での位置。
    placement: Placement,
    next_x: u32,
    next_y: u32,
    slots: HashMap<GlyphKey, Slot>,
    /// 装飾と、それを描いたセルの数ごとの場所。
    decoration_slots: HashMap<(Decoration, u8), Slot>,
}

impl GlyphAtlas {
    pub fn new(
        device: &ID3D11Device,
        dwrite: &IDWriteFactory,
        font: &Font,
    ) -> windows::core::Result<Self> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: ATLAS_SIZE,
            Height: ATLAS_SIZE,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            ..Default::default()
        };
        let mut texture = None;
        // SAFETY: desc は初期化済みで、texture は書き込める。
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
        let texture: ID3D11Texture2D = texture.ok_or_else(windows::core::Error::empty)?;

        let mut view = None;
        // SAFETY: texture はシェーダから読めるテクスチャで、view は書き込める。
        unsafe { device.CreateShaderResourceView(&texture, None, Some(&mut view))? };
        let view = view.ok_or_else(windows::core::Error::empty)?;

        let (factory, context) = create_d2d_context(device, &texture)?;
        // SAFETY: context は有効な Direct2D のデバイスコンテキスト。
        let brush = unsafe {
            context.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 1.0,
                },
                None,
            )?
        };

        let formats = [
            text_format(
                dwrite,
                font,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
            )?,
            text_format(
                dwrite,
                font,
                DWRITE_FONT_WEIGHT_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
            )?,
            text_format(
                dwrite,
                font,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_ITALIC,
            )?,
            text_format(
                dwrite,
                font,
                DWRITE_FONT_WEIGHT_BOLD,
                DWRITE_FONT_STYLE_ITALIC,
            )?,
        ];
        let placement = Placement::new(
            baseline(dwrite, &formats[0], font.cell_height_px as f32)?,
            &font.lines,
            font.cell_height_px as f32,
        );

        let mut atlas = Self {
            view,
            factory,
            context,
            brush,
            dwrite: dwrite.clone(),
            formats,
            cell_width: font.cell_width_px,
            cell_height: font.cell_height_px,
            line_thickness: line_thickness(font.cell_width_px),
            placement,
            next_x: 0,
            next_y: 0,
            slots: HashMap::new(),
            decoration_slots: HashMap::new(),
        };
        atlas.clear()?;
        Ok(atlas)
    }

    pub fn view(&self) -> &ID3D11ShaderResourceView {
        &self.view
    }

    pub fn size(&self) -> (u32, u32) {
        (ATLAS_SIZE, ATLAS_SIZE)
    }

    /// 覚えている場所を捨て、テクスチャを空にする。
    pub fn clear(&mut self) -> windows::core::Result<()> {
        self.slots.clear();
        self.decoration_slots.clear();
        self.next_x = 0;
        self.next_y = 0;
        // SAFETY: context の描画先はアトラスのテクスチャ。BeginDraw と EndDraw を対にする。
        unsafe {
            self.context.BeginDraw();
            self.context.Clear(Some(&D2D1_COLOR_F::default()));
            self.context.EndDraw(None, None)
        }
    }

    /// `text` を `cells` セル分の幅で描いた場所を返す。まだ描いていなければ描く。
    pub fn slot(
        &mut self,
        text: &str,
        bold: bool,
        italic: bool,
        cells: u8,
    ) -> Result<Slot, AtlasFull> {
        let key = GlyphKey {
            text: text.to_owned(),
            bold,
            italic,
            cells,
        };
        if let Some(slot) = self.slots.get(&key) {
            return Ok(*slot);
        }
        let slot = self.allocate(self.cell_width * u32::from(cells))?;
        if let Err(e) = self.draw(&key, slot) {
            tracing::warn!("failed to rasterize {text:?}: {e}");
        }
        self.slots.insert(key, slot);
        Ok(slot)
    }

    /// `decoration` を `cells` セル分の幅で描いた場所を返す。まだ描いていなければ描く。
    pub fn decoration_slot(
        &mut self,
        decoration: Decoration,
        cells: u8,
    ) -> Result<Slot, AtlasFull> {
        if let Some(slot) = self.decoration_slots.get(&(decoration, cells)) {
            return Ok(*slot);
        }
        let slot = self.allocate(self.cell_width * u32::from(cells))?;
        let shapes = decoration::shapes(
            decoration,
            self.cell_width as f32,
            cells,
            slot.height as f32,
            &self.placement,
        );
        if let Err(e) = self.draw_shapes(&shapes, slot) {
            tracing::warn!("failed to rasterize {decoration:?}: {e}");
        }
        self.decoration_slots.insert((decoration, cells), slot);
        Ok(slot)
    }

    fn allocate(&mut self, width: u32) -> Result<Slot, AtlasFull> {
        if self.next_x + width > ATLAS_SIZE {
            self.next_x = 0;
            self.next_y += self.cell_height;
        }
        if self.next_y + self.cell_height > ATLAS_SIZE {
            return Err(AtlasFull);
        }
        let slot = Slot {
            x: self.next_x,
            y: self.next_y,
            width,
            height: self.cell_height,
        };
        self.next_x += width;
        Ok(slot)
    }

    /// `slot` に文字を描く。罫線とブロック要素は形として描き、それ以外はフォントで描く。
    fn draw(&self, key: &GlyphKey, slot: Slot) -> windows::core::Result<()> {
        let shapes = boxdraw::shapes(
            &key.text,
            slot.width as f32,
            slot.height as f32,
            self.line_thickness,
        );
        match shapes {
            Some(shapes) => self.draw_shapes(&shapes, slot),
            None => self.draw_text(key, slot),
        }
    }

    /// 罫線やブロック要素の形を `slot` に描く。
    fn draw_shapes(&self, shapes: &[Shape], slot: Slot) -> windows::core::Result<()> {
        let origin = Matrix3x2::translation(slot.x as f32, slot.y as f32);
        self.draw_in_slot(slot, &origin, |context| {
            for shape in shapes {
                self.draw_shape(context, shape)?;
            }
            Ok(())
        })
    }

    fn draw_shape(&self, context: &ID2D1DeviceContext, shape: &Shape) -> windows::core::Result<()> {
        match *shape {
            Shape::Rect {
                left,
                top,
                right,
                bottom,
                alpha,
            } => {
                let rect = D2D_RECT_F {
                    left,
                    top,
                    right,
                    bottom,
                };
                // SAFETY: 描画中の context と、同じ context が作ったブラシ。
                unsafe {
                    self.brush.SetOpacity(alpha);
                    context.FillRectangle(&rect, &self.brush);
                    self.brush.SetOpacity(1.0);
                }
            }
            Shape::Line { from, to, width } => {
                // SAFETY: 描画中の context と、同じ context が作ったブラシ。
                unsafe {
                    context.DrawLine(point(from), point(to), &self.brush, width, None);
                }
            }
            Shape::RoundedCorner {
                from,
                corner,
                to,
                radius,
                width,
            } => {
                let geometry = self.rounded_corner(from, corner, to, radius)?;
                // SAFETY: 描画中の context と、同じファクトリで作った図形。
                unsafe { context.DrawGeometry(&geometry, &self.brush, width, None) };
            }
            Shape::Wave {
                left,
                right,
                center_y,
                amplitude,
                period,
                width,
            } => {
                let geometry = self.wave(left, right, center_y, amplitude, period)?;
                // SAFETY: 描画中の context と、同じファクトリで作った図形。
                unsafe { context.DrawGeometry(&geometry, &self.brush, width, None) };
            }
        }
        Ok(())
    }

    /// 半周期ごとに二次ベジェ曲線で山と谷をつないだ波線。`left` から上へ向かって始まる。
    fn wave(
        &self,
        left: f32,
        right: f32,
        center_y: f32,
        amplitude: f32,
        period: f32,
    ) -> windows::core::Result<ID2D1PathGeometry1> {
        let half = period / 2.0;
        // 二次ベジェ曲線の頂点は、制御点までの高さの半分になる。
        let control = amplitude * 2.0;
        // SAFETY: ファクトリは有効で、sink は作った図形にだけ書き、閉じてから図形を使う。
        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(point((left, center_y)), D2D1_FIGURE_BEGIN_HOLLOW);
            let mut x = left;
            let mut upward = true;
            while x < right {
                let direction = if upward { -1.0 } else { 1.0 };
                sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT {
                    point1: point((x + half / 2.0, center_y + direction * control)),
                    point2: point((x + half, center_y)),
                });
                x += half;
                upward = !upward;
            }
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            Ok(geometry)
        }
    }

    /// `from` から `corner` へ向かい、半径 `radius` の四分円で曲がって `to` へ向かう線。
    fn rounded_corner(
        &self,
        from: (f32, f32),
        corner: (f32, f32),
        to: (f32, f32),
        radius: f32,
    ) -> windows::core::Result<ID2D1PathGeometry1> {
        let incoming = unit(from, corner);
        let outgoing = unit(corner, to);
        let arc_start = (
            corner.0 - incoming.0 * radius,
            corner.1 - incoming.1 * radius,
        );
        let arc_end = (
            corner.0 + outgoing.0 * radius,
            corner.1 + outgoing.1 * radius,
        );
        // y が下向きの座標では、向きの外積が正なら右回り。
        let clockwise = incoming.0 * outgoing.1 - incoming.1 * outgoing.0 > 0.0;

        // SAFETY: ファクトリは有効で、sink は作った図形にだけ書き、閉じてから図形を使う。
        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(point(from), D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddLine(point(arc_start));
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: point(arc_end),
                size: D2D_SIZE_F {
                    width: radius,
                    height: radius,
                },
                rotationAngle: 0.0,
                sweepDirection: if clockwise {
                    D2D1_SWEEP_DIRECTION_CLOCKWISE
                } else {
                    D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
                },
                arcSize: D2D1_ARC_SIZE_SMALL,
            });
            sink.AddLine(point(to));
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            Ok(geometry)
        }
    }

    /// `slot` を空にしてから、`transform` をかけて `body` で描く。描くのは `slot` の中だけ。
    fn draw_in_slot(
        &self,
        slot: Slot,
        transform: &Matrix3x2,
        body: impl FnOnce(&ID2D1DeviceContext) -> windows::core::Result<()>,
    ) -> windows::core::Result<()> {
        let clip = D2D_RECT_F {
            left: slot.x as f32,
            top: slot.y as f32,
            right: (slot.x + slot.width) as f32,
            bottom: (slot.y + slot.height) as f32,
        };
        // SAFETY: context の描画先はアトラスのテクスチャ。BeginDraw と EndDraw、Push と Pop を対にする。
        unsafe {
            self.context.BeginDraw();
            self.context.SetTransform(&Matrix3x2::identity());
            self.context
                .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
            self.context.Clear(Some(&D2D1_COLOR_F::default()));
            self.context.SetTransform(transform);
        }
        let drawn = body(&self.context);
        // SAFETY: 上で始めた描画と切り抜きを閉じる。
        let ended = unsafe {
            self.context.SetTransform(&Matrix3x2::identity());
            self.context.PopAxisAlignedClip();
            self.context.EndDraw(None, None)
        };
        drawn.and(ended)
    }

    /// `slot` にフォントで文字を描く。割り当てた幅より広い文字は横に縮め、狭い文字は真ん中に置く。
    fn draw_text(&self, key: &GlyphKey, slot: Slot) -> windows::core::Result<()> {
        let format = &self.formats[format_index(key.bold, key.italic)];
        let text: Vec<u16> = key.text.encode_utf16().collect();
        // SAFETY: text と format はこの呼び出しの間は生きている。
        let layout = unsafe {
            self.dwrite
                .CreateTextLayout(&text, format, f32::MAX, slot.height as f32)?
        };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        // SAFETY: metrics は書き込める。
        unsafe { layout.GetMetrics(&mut metrics)? };

        let slot_width = slot.width as f32;
        let text_width = metrics.widthIncludingTrailingWhitespace.max(1.0);
        let scale = (slot_width / text_width).min(1.0);
        let offset = (slot_width - text_width * scale) / 2.0;
        let transform = Matrix3x2 {
            M11: scale,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: slot.x as f32 + offset,
            M32: slot.y as f32,
        };
        self.draw_in_slot(slot, &transform, |context| {
            // SAFETY: 描画中の context と、同じ context が作ったブラシ。layout はこの呼び出しの間は生きている。
            unsafe {
                context.DrawTextLayout(
                    Vector2 { X: 0.0, Y: 0.0 },
                    &layout,
                    &self.brush,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
            }
            Ok(())
        })
    }
}

fn point((x, y): (f32, f32)) -> Vector2 {
    Vector2 { X: x, Y: y }
}

/// `from` から `to` へ向かう単位ベクトル。
fn unit(from: (f32, f32), to: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
    (dx / length, dy / length)
}

/// 罫線の細線の太さ（ピクセル）。セルの幅の 1/8 を、少なくとも 1 ピクセルにする。
fn line_thickness(cell_width: u32) -> f32 {
    (cell_width as f32 / 8.0).round().max(1.0)
}

/// `format` で 1 行を組んだときの、行の上端からベースラインまでの距離（ピクセル）。
/// [`GlyphAtlas::draw_text`] と同じ組み方で測る。
fn baseline(
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    height: f32,
) -> windows::core::Result<f32> {
    let text: Vec<u16> = "M".encode_utf16().collect();
    // SAFETY: text と format はこの呼び出しの間は生きている。
    let layout = unsafe { dwrite.CreateTextLayout(&text, format, f32::MAX, height)? };
    let mut lines = [DWRITE_LINE_METRICS::default()];
    let mut count = 0u32;
    // SAFETY: lines は 1 行分の書き込める配列で、count は書き込める。
    unsafe { layout.GetLineMetrics(Some(&mut lines), &mut count)? };
    Ok(lines[0].baseline)
}

fn format_index(bold: bool, italic: bool) -> usize {
    usize::from(bold) | (usize::from(italic) << 1)
}

fn text_format(
    dwrite: &IDWriteFactory,
    font: &Font,
    weight: DWRITE_FONT_WEIGHT,
    style: DWRITE_FONT_STYLE,
) -> windows::core::Result<IDWriteTextFormat> {
    // SAFETY: 文字列はこの呼び出しの間は生きている。
    let format = unsafe {
        dwrite.CreateTextFormat(
            &HSTRING::from(font.family.as_str()),
            None,
            weight,
            style,
            DWRITE_FONT_STRETCH_NORMAL,
            font.size_px,
            LOCALE,
        )?
    };
    // SAFETY: format は有効な書式。
    unsafe { format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)? };
    Ok(format)
}

/// `texture` を描画先にした Direct2D のデバイスコンテキストと、それを作ったファクトリ。
fn create_d2d_context(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
) -> windows::core::Result<(ID2D1Factory1, ID2D1DeviceContext)> {
    // SAFETY: 単一スレッド用のファクトリを作るだけ。
    let factory: ID2D1Factory1 =
        unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
    let dxgi_device: IDXGIDevice = device.cast()?;
    // SAFETY: dxgi_device は BGRA に対応した Direct3D 11 のデバイス。
    let d2d_device = unsafe { factory.CreateDevice(&dxgi_device)? };
    // SAFETY: d2d_device は有効。
    let context = unsafe { d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)? };

    let surface: IDXGISurface = texture.cast()?;
    let properties = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: PIXEL_DPI,
        dpiY: PIXEL_DPI,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        ..Default::default()
    };
    // SAFETY: surface はアトラスのテクスチャの面で、properties は初期化済み。
    let bitmap = unsafe { context.CreateBitmapFromDxgiSurface(&surface, Some(&properties))? };
    // SAFETY: bitmap は context が作った描画先。
    unsafe {
        context.SetTarget(&bitmap);
        context.SetDpi(PIXEL_DPI, PIXEL_DPI);
        context.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
    }
    Ok((factory, context))
}
