//! グリフアトラス。DirectWrite で組んだ文字を Direct2D でテクスチャに描き、その場所を覚えておく。
//!
//! 文字は白で、乗算済みのアルファで描く。色はシェーダで付ける。場所は 1 セルの高さの棚に左から
//! 詰めていき、いっぱいになったら全部を捨てて描き直す。

use std::collections::HashMap;

use windows::{
    Win32::Graphics::{
        Direct2D::{
            Common::{D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT},
            D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
            D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_FACTORY_TYPE_SINGLE_THREADED,
            D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1DeviceContext,
            ID2D1Factory1, ID2D1SolidColorBrush,
        },
        Direct3D11::{
            D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11ShaderResourceView, ID3D11Texture2D,
        },
        DirectWrite::{
            DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE, DWRITE_FONT_STYLE_ITALIC,
            DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
            IDWriteFactory, IDWriteTextFormat,
        },
        Dxgi::{
            Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
            IDXGIDevice, IDXGISurface,
        },
    },
    core::{HSTRING, Interface, w},
};
use windows_numerics::{Matrix3x2, Vector2};

use crate::font::Font;

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
    context: ID2D1DeviceContext,
    brush: ID2D1SolidColorBrush,
    dwrite: IDWriteFactory,
    /// 太字・斜体の組み合わせごとの書式。添字は [`format_index`]。
    formats: [IDWriteTextFormat; 4],
    cell_width: u32,
    cell_height: u32,
    next_x: u32,
    next_y: u32,
    slots: HashMap<GlyphKey, Slot>,
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

        let context = create_d2d_context(device, &texture)?;
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

        let mut atlas = Self {
            view,
            context,
            brush,
            dwrite: dwrite.clone(),
            formats,
            cell_width: font.cell_width_px,
            cell_height: font.cell_height_px,
            next_x: 0,
            next_y: 0,
            slots: HashMap::new(),
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

    /// `slot` に文字を描く。割り当てた幅より広い文字は横に縮め、狭い文字は真ん中に置く。
    fn draw(&self, key: &GlyphKey, slot: Slot) -> windows::core::Result<()> {
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
            self.context.SetTransform(&transform);
            self.context.DrawTextLayout(
                Vector2 { X: 0.0, Y: 0.0 },
                &layout,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
            self.context.SetTransform(&Matrix3x2::identity());
            self.context.PopAxisAlignedClip();
            self.context.EndDraw(None, None)
        }
    }
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

/// `texture` を描画先にした Direct2D のデバイスコンテキストを作る。
fn create_d2d_context(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
) -> windows::core::Result<ID2D1DeviceContext> {
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
    Ok(context)
}
