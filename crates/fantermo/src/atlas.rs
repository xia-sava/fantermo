//! グリフアトラス。DirectWrite で組んだ文字を Direct2D でテクスチャに描き、その場所を覚えておく。
//! タブバーの角や印のような形（[`Icon`]）も描く。
//!
//! 文字は白で、乗算済みのアルファで描く。色はシェーダで付ける。場所は棚に左から詰めていき（棚の
//! 高さはそこに置いたものの高いほう。文字は 1 セルの高さ）、いっぱいになったら全部を捨てて描き直す。
//! 斜体のようにインクがセルの左右へはみ出すグリフは、はみ出す分だけ左右に余白を持たせた場所に描く。
//! タブバーのタイトルは、Windows の UI のフォント（[`UiFont`]）で文字列を一つの絵として描く。

use std::{collections::HashMap, rc::Rc};

use fantermo_core::config::WideGlyph;
use windows::{
    Win32::Graphics::{
        Direct2D::{
            Common::{
                D2D_RECT_F, D2D_SIZE_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
                D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED,
                D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
            },
            D2D1_ANTIALIAS_MODE_ALIASED, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_SMALL,
            D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
            D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_QUADRATIC_BEZIER_SEGMENT,
            D2D1_SWEEP_DIRECTION_CLOCKWISE, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
            D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1DeviceContext,
            ID2D1Factory1, ID2D1PathGeometry1, ID2D1RenderTarget, ID2D1SolidColorBrush,
        },
        Direct3D11::{
            D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11ShaderResourceView, ID3D11Texture2D,
        },
        DirectWrite::{
            DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE, DWRITE_FONT_STYLE_ITALIC,
            DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_FONT_WEIGHT_NORMAL, DWRITE_LINE_METRICS, DWRITE_TEXT_METRICS,
            DWRITE_WORD_WRAPPING_NO_WRAP, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
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
    paint::Icon,
    ui_font::UiFont,
};

/// アトラスの一辺（ピクセル）。
const ATLAS_SIZE: u32 = 2048;

/// [`WideGlyph::Overflow`] と [`WideGlyph::OverflowCenter`] で、文字に割り当てたセルの何倍の幅まで
/// 広げて描くか。
const MAX_OVERFLOW_FACTOR: u8 = 2;

/// インクがセルの左右へはみ出す分を描く余白の上限（セルの幅の何倍か）。
const MAX_INK_MARGIN_FACTOR: f32 = 1.0;

/// インクがセルの外へ出たとみなさない、計算の誤差の分（ピクセル）。
const INK_EPSILON_PX: f32 = 1.0 / 64.0;

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
    /// 場所の左端が、文字に割り当てたセルの左端より何ピクセル左にあるか。インクが左のセルへ
    /// はみ出すグリフで正になる。
    pub left: u32,
}

/// アトラスに空きがない。
#[derive(Debug)]
pub struct AtlasFull;

/// 正方形のテクスチャを棚に分けて、場所を左から詰めて取る。棚の高さは、そこに置いたものの高いほう。
#[derive(Clone, Copy, Debug)]
struct Shelves {
    /// テクスチャの一辺。
    size: u32,
    next_x: u32,
    /// いま詰めている棚の上端。
    next_y: u32,
    /// いま詰めている棚の高さ。
    shelf_height: u32,
}

impl Shelves {
    fn new(size: u32) -> Self {
        Self {
            size,
            next_x: 0,
            next_y: 0,
            shelf_height: 0,
        }
    }

    /// 幅 `width`、高さ `height` の場所。いまの棚の右に入らなければ、次の棚の左端に置く。
    fn allocate(&mut self, width: u32, height: u32) -> Result<Slot, AtlasFull> {
        if width > self.size || height > self.size {
            return Err(AtlasFull);
        }
        if self.next_x + width > self.size {
            self.next_x = 0;
            self.next_y += self.shelf_height;
            self.shelf_height = 0;
        }
        if self.next_y + height > self.size {
            return Err(AtlasFull);
        }
        let slot = Slot {
            x: self.next_x,
            y: self.next_y,
            width,
            height,
            left: 0,
        };
        self.next_x += width;
        self.shelf_height = self.shelf_height.max(height);
        Ok(slot)
    }
}

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
    /// セルより広いグリフの描き方。
    wide_glyph: WideGlyph,
    shelves: Shelves,
    slots: HashMap<GlyphKey, Slot>,
    /// 装飾と、それを描いたセルの数ごとの場所。
    decoration_slots: HashMap<(Decoration, u8), Slot>,
    /// 形と、それを描いた幅と高さごとの場所。
    icon_slots: HashMap<(Icon, u32, u32), Slot>,
    /// タブバーのタイトルを描く書体。
    ui_font: Rc<UiFont>,
    /// タブバーのタイトルの文字列ごとの場所。
    label_slots: HashMap<String, Slot>,
}

impl GlyphAtlas {
    pub fn new(
        device: &ID3D11Device,
        dwrite: &IDWriteFactory,
        font: &Font,
        ui_font: Rc<UiFont>,
        wide_glyph: WideGlyph,
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
            wide_glyph,
            shelves: Shelves::new(ATLAS_SIZE),
            slots: HashMap::new(),
            decoration_slots: HashMap::new(),
            icon_slots: HashMap::new(),
            ui_font,
            label_slots: HashMap::new(),
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

    /// タブのアイコンを描くのに使う、Direct2D のファクトリと、DirectWrite のファクトリと、UI のフォントの
    /// 書体の名前。
    pub fn tab_icon_resources(&self) -> (&ID2D1Factory1, &IDWriteFactory, &str) {
        (&self.factory, &self.dwrite, &self.ui_font.family)
    }

    /// 覚えている場所を捨て、テクスチャを空にする。
    pub fn clear(&mut self) -> windows::core::Result<()> {
        self.slots.clear();
        self.decoration_slots.clear();
        self.icon_slots.clear();
        self.label_slots.clear();
        self.shelves = Shelves::new(ATLAS_SIZE);
        // SAFETY: context の描画先はアトラスのテクスチャ。BeginDraw と EndDraw を対にする。
        unsafe {
            self.context.BeginDraw();
            self.context.Clear(Some(&D2D1_COLOR_F::default()));
            self.context.EndDraw(None, None)
        }
    }

    /// `text` を `cells` セル分の幅で描いた場所を返す。まだ描いていなければ描く。
    /// [`WideGlyph::Overflow`] と [`WideGlyph::OverflowCenter`] で描いたグリフと、インクがセルの
    /// 左右へはみ出すグリフは、`cells` セルより広い場所になる。
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
        // 罫線・ブロック要素・点字などの記号は形として描き、それ以外はフォントで描く。
        let width = self.cell_width * u32::from(cells);
        let shapes = boxdraw::shapes(
            text,
            width as f32,
            self.cell_height as f32,
            self.line_thickness,
        );
        let slot = match shapes {
            Some(shapes) => {
                let slot = self.allocate(width)?;
                if let Err(e) = self.draw_shapes(&shapes, slot) {
                    tracing::warn!("failed to rasterize {text:?}: {e}");
                }
                slot
            }
            None => self.text_slot(&key)?,
        };
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

    /// `icon` を幅 `width`・高さ `height` で描いた場所を返す。まだ描いていなければ描く。
    pub fn icon_slot(&mut self, icon: Icon, width: u32, height: u32) -> Result<Slot, AtlasFull> {
        if let Some(slot) = self.icon_slots.get(&(icon, width, height)) {
            return Ok(*slot);
        }
        let slot = self.allocate_area(width, height)?;
        if let Err(e) = self.draw_shapes(&icon.shapes(width as f32, height as f32), slot) {
            tracing::warn!("failed to rasterize {icon:?}: {e}");
        }
        self.icon_slots.insert((icon, width, height), slot);
        Ok(slot)
    }

    /// タブバーのタイトル `text` を UI のフォントで 1 行に組んで描いた場所を返す。まだ描いていなければ
    /// 描く。場所の高さは UI のフォントの 1 行の高さで、インクが送り幅の左右へはみ出せば、その分の
    /// 余白を足す。
    pub fn label_slot(&mut self, text: &str) -> Result<Slot, AtlasFull> {
        if let Some(slot) = self.label_slots.get(text) {
            return Ok(*slot);
        }
        let height = self.ui_font.height_px();
        let (layout, extent) = match self.ui_font.layout(text) {
            Ok(measured) => measured,
            Err(e) => {
                tracing::warn!("failed to lay out the title {text:?}: {e}");
                return self.allocate_area(1, height);
            }
        };
        let margin = |outside: f32| (outside - INK_EPSILON_PX).ceil().max(0.0) as u32;
        let placement = TextPlacement {
            cells: 0,
            scale: 1.0,
            offset: 0.0,
            left_margin: margin(-extent.ink_left),
            right_margin: margin(extent.ink_right - extent.advance),
        };
        let width = placement.left_margin + extent.advance.ceil() as u32 + placement.right_margin;
        let slot = Slot {
            left: placement.left_margin,
            ..self.allocate_area(width, height)?
        };
        if let Err(e) = self.draw_text(&layout, &placement, slot) {
            tracing::warn!("failed to rasterize the title {text:?}: {e}");
        }
        self.label_slots.insert(text.to_owned(), slot);
        Ok(slot)
    }

    /// 幅 `width`、高さ 1 セルの場所。
    fn allocate(&mut self, width: u32) -> Result<Slot, AtlasFull> {
        self.allocate_area(width, self.cell_height)
    }

    /// 幅 `width`、高さ `height` の場所。
    fn allocate_area(&mut self, width: u32, height: u32) -> Result<Slot, AtlasFull> {
        self.shelves.allocate(width, height)
    }

    /// 記号や装飾の形を `slot` に描く。
    fn draw_shapes(&self, shapes: &[Shape], slot: Slot) -> windows::core::Result<()> {
        let origin = Matrix3x2::translation(slot.x as f32, slot.y as f32);
        self.draw_in_slot(slot, &origin, |context| {
            for shape in shapes {
                draw_shape(context, &self.factory, &self.brush, shape)?;
            }
            Ok(())
        })
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

    /// フォントで文字を組んで描き、その場所を返す。場所の幅と置き方は [`place_text`] で決める。
    /// 組めなければ、セルの幅の空の場所を返す。
    fn text_slot(&mut self, key: &GlyphKey) -> Result<Slot, AtlasFull> {
        let cell_width = self.cell_width as f32;
        let format = &self.formats[format_index(key.bold, key.italic)];
        let (layout, extent) =
            match text_layout(&self.dwrite, format, &key.text, self.cell_height as f32) {
                Ok(measured) => measured,
                Err(e) => {
                    tracing::warn!("failed to lay out {:?}: {e}", key.text);
                    return self.allocate(self.cell_width * u32::from(key.cells));
                }
            };
        let placement = place_text(extent, key.cells, cell_width, self.wide_glyph);
        let slot = Slot {
            left: placement.left_margin,
            ..self.allocate(placement.slot_width(self.cell_width))?
        };
        if let Err(e) = self.draw_text(&layout, &placement, slot) {
            tracing::warn!("failed to rasterize {:?}: {e}", key.text);
        }
        Ok(slot)
    }

    /// 組んだ文字を `placement` のとおりに `slot` に描く。
    fn draw_text(
        &self,
        layout: &IDWriteTextLayout,
        placement: &TextPlacement,
        slot: Slot,
    ) -> windows::core::Result<()> {
        let transform = Matrix3x2 {
            M11: placement.scale,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: (slot.x + slot.left) as f32 + placement.offset,
            M32: slot.y as f32,
        };
        self.draw_in_slot(slot, &transform, |context| {
            // SAFETY: 描画中の context と、同じ context が作ったブラシ。layout はこの呼び出しの間は生きている。
            unsafe {
                context.DrawTextLayout(
                    Vector2 { X: 0.0, Y: 0.0 },
                    layout,
                    &self.brush,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
            }
            Ok(())
        })
    }
}

/// `shape` を `brush` で `target` に描く。`factory` は `target` を作ったファクトリ、`brush` は `target` が
/// 作ったブラシ。`BeginDraw` と `EndDraw` の間で呼ぶ。
pub fn draw_shape(
    target: &ID2D1RenderTarget,
    factory: &ID2D1Factory1,
    brush: &ID2D1SolidColorBrush,
    shape: &Shape,
) -> windows::core::Result<()> {
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
            // SAFETY: 描画中の target と、同じ target が作ったブラシ。
            unsafe {
                brush.SetOpacity(alpha);
                target.FillRectangle(&rect, brush);
                brush.SetOpacity(1.0);
            }
        }
        Shape::Line { from, to, width } => {
            // SAFETY: 描画中の target と、同じ target が作ったブラシ。
            unsafe {
                target.DrawLine(point(from), point(to), brush, width, None);
            }
        }
        Shape::RoundedCorner {
            from,
            corner,
            to,
            radius,
            width,
        } => {
            let geometry = rounded_corner(factory, from, corner, to, radius)?;
            // SAFETY: 描画中の target と、同じファクトリで作った図形。
            unsafe { target.DrawGeometry(&geometry, brush, width, None) };
        }
        Shape::Wave {
            left,
            right,
            center_y,
            amplitude,
            period,
            width,
        } => {
            let geometry = wave(factory, left, right, center_y, amplitude, period)?;
            // SAFETY: 描画中の target と、同じファクトリで作った図形。
            unsafe { target.DrawGeometry(&geometry, brush, width, None) };
        }
        Shape::Polygon { ref points, alpha } => {
            let geometry = path(factory, points, true)?;
            // SAFETY: 描画中の target と、同じ target が作ったブラシと、同じファクトリで作った図形。
            unsafe {
                brush.SetOpacity(alpha);
                target.FillGeometry(&geometry, brush, None);
                brush.SetOpacity(1.0);
            }
        }
        Shape::Polyline { ref points, width } => {
            let geometry = path(factory, points, false)?;
            // SAFETY: 描画中の target と、同じファクトリで作った図形。
            unsafe { target.DrawGeometry(&geometry, brush, width, None) };
        }
    }
    Ok(())
}

/// `points` を順に結んだ図形。`closed` なら閉じて塗れる形にする。
fn path(
    factory: &ID2D1Factory1,
    points: &[(f32, f32)],
    closed: bool,
) -> windows::core::Result<ID2D1PathGeometry1> {
    let (begin, end) = if closed {
        (D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED)
    } else {
        (D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_OPEN)
    };
    let vertices: Vec<Vector2> = points.iter().map(|&p| point(p)).collect();
    // SAFETY: ファクトリは有効で、sink は作った図形にだけ書き、閉じてから図形を使う。
    unsafe {
        let geometry = factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        if let [first, rest @ ..] = &vertices[..] {
            sink.BeginFigure(*first, begin);
            sink.AddLines(rest);
            sink.EndFigure(end);
        }
        sink.Close()?;
        Ok(geometry)
    }
}

/// 半周期ごとに二次ベジェ曲線で山と谷をつないだ波線。`left` から上へ向かって始まる。
fn wave(
    factory: &ID2D1Factory1,
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
        let geometry = factory.CreatePathGeometry()?;
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
    factory: &ID2D1Factory1,
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
        let geometry = factory.CreatePathGeometry()?;
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

fn point((x, y): (f32, f32)) -> Vector2 {
    Vector2 { X: x, Y: y }
}

/// `from` から `to` へ向かう単位ベクトル。
fn unit(from: (f32, f32), to: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
    (dx / length, dy / length)
}

/// 1 行に組んだ文字の送り幅と、インクの左右の端（ピクセル）。インクの端は文字の左端からの距離で、
/// 左へはみ出していれば `ink_left` が負、右へはみ出していれば `ink_right` が `advance` より大きい。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextExtent {
    pub advance: f32,
    pub ink_left: f32,
    pub ink_right: f32,
}

/// `text` を `format` で 1 行に組んだものと、その送り幅とインクの範囲。送り幅が 0 の文字
/// （結合文字だけのものなど）は 1 ピクセルとして扱う。
pub fn text_layout(
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
    height: f32,
) -> windows::core::Result<(IDWriteTextLayout, TextExtent)> {
    let text: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: text と format はこの呼び出しの間は生きている。
    let layout = unsafe { dwrite.CreateTextLayout(&text, format, f32::MAX, height)? };
    let mut metrics = DWRITE_TEXT_METRICS::default();
    // SAFETY: metrics は書き込める。
    unsafe { layout.GetMetrics(&mut metrics)? };
    let advance = metrics.widthIncludingTrailingWhitespace.max(1.0);
    // はみ出しは組む枠からの距離なので、枠の幅を送り幅に合わせてから測る。左寄せで折り返さない
    // ので、枠の幅を変えても文字の位置は変わらない。
    // SAFETY: layout は有効。
    let overhang = unsafe {
        layout.SetMaxWidth(advance)?;
        layout.GetOverhangMetrics()?
    };
    Ok((
        layout,
        TextExtent {
            advance,
            ink_left: -overhang.left,
            ink_right: advance + overhang.right,
        },
    ))
}

/// 組んだ文字をアトラスの場所に置く方法。
#[derive(Clone, Copy, Debug, PartialEq)]
struct TextPlacement {
    /// 文字に割り当てた場所の幅（セルの数）。左右の余白は含まない。
    cells: u8,
    /// 横の倍率。
    scale: f32,
    /// セルの左端から文字の左端までの距離（ピクセル）。
    offset: f32,
    /// インクが左のセルへはみ出す分の余白（ピクセル）。
    left_margin: u32,
    /// インクが右のセルへはみ出す分の余白（ピクセル）。
    right_margin: u32,
}

impl TextPlacement {
    /// 左右の余白を含めた場所の幅（ピクセル）。
    fn slot_width(&self, cell_width: u32) -> u32 {
        self.left_margin + cell_width * u32::from(self.cells) + self.right_margin
    }
}

/// `extent` の文字を、`cells` セルの文字として置く方法を決める。縮めるか・広げるかは送り幅で決め、
/// 左右の余白はインクの範囲で決める。
///
/// 送り幅が収まる文字はセルの真ん中に置く。収まらない文字は `wide_glyph` に従い、
/// [`WideGlyph::Overflow`] なら場所を右へ広げる。[`WideGlyph::OverflowCenter`] なら場所は
/// 広げずに文字をセルの真ん中に置き、左右へはみ出した分をインクのはみ出しと同じ余白で描く。
/// どちらも [`MAX_OVERFLOW_FACTOR`] 倍の幅までで、それでも収まらなければその幅に縮める。
/// 置いた結果、インクが場所の左右へはみ出せば、その分の余白を足す（セルの幅の
/// [`MAX_INK_MARGIN_FACTOR`] 倍まで）。[`WideGlyph::Clip`] で切った文字には余白を足さない。
fn place_text(
    extent: TextExtent,
    cells: u8,
    cell_width: f32,
    wide_glyph: WideGlyph,
) -> TextPlacement {
    let width = cell_width * f32::from(cells);
    let (cells, scale, offset) = if extent.advance <= width {
        (cells, 1.0, (width - extent.advance) / 2.0)
    } else {
        match wide_glyph {
            WideGlyph::Shrink => (cells, width / extent.advance, 0.0),
            WideGlyph::Clip => {
                return TextPlacement {
                    cells,
                    scale: 1.0,
                    offset: 0.0,
                    left_margin: 0,
                    right_margin: 0,
                };
            }
            WideGlyph::Overflow => {
                let needed = (extent.advance / cell_width).ceil();
                let limit = f32::from(cells.saturating_mul(MAX_OVERFLOW_FACTOR));
                let cells = needed.min(limit) as u8;
                let scale = (cell_width * f32::from(cells) / extent.advance).min(1.0);
                (cells, scale, 0.0)
            }
            WideGlyph::OverflowCenter => {
                let limit = width * f32::from(MAX_OVERFLOW_FACTOR);
                let scale = (limit / extent.advance).min(1.0);
                (cells, scale, (width - extent.advance * scale) / 2.0)
            }
        }
    };
    let width = cell_width * f32::from(cells);
    let ink_left = offset + extent.ink_left * scale;
    let ink_right = offset + extent.ink_right * scale;
    let margin = |outside: f32| {
        ((outside - INK_EPSILON_PX).ceil()).clamp(0.0, cell_width * MAX_INK_MARGIN_FACTOR) as u32
    };
    TextPlacement {
        cells,
        scale,
        offset,
        left_margin: margin(-ink_left),
        right_margin: margin(ink_right - width),
    }
}

/// 罫線の細線の太さ（ピクセル）。セルの幅の 1/8 を、少なくとも 1 ピクセルにする。
fn line_thickness(cell_width: u32) -> f32 {
    (cell_width as f32 / 8.0).round().max(1.0)
}

/// `format` で 1 行を組んだときの、行の上端からベースラインまでの距離（ピクセル）。
/// [`text_layout`] と同じ組み方で測る。
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

#[cfg(test)]
mod tests {
    use super::*;

    use windows::{
        Win32::Graphics::DirectWrite::{DWRITE_GLYPH_METRICS, IDWriteFontFace},
        core::BOOL,
    };

    use crate::font;

    const CELL_WIDTH: f32 = 16.0;

    fn placement(cells: u8, scale: f32, offset: f32) -> TextPlacement {
        TextPlacement {
            cells,
            scale,
            offset,
            left_margin: 0,
            right_margin: 0,
        }
    }

    /// インクが送り幅にちょうど収まる文字。
    fn upright(advance: f32) -> TextExtent {
        TextExtent {
            advance,
            ink_left: 0.0,
            ink_right: advance,
        }
    }

    #[test]
    fn shelves_grow_to_their_tallest_slot() {
        let mut shelves = Shelves::new(100);
        let at = |slot: Slot| (slot.x, slot.y, slot.width, slot.height);
        assert_eq!(at(shelves.allocate(40, 10).unwrap()), (0, 0, 40, 10));
        // セルより高いものも同じ棚に置き、棚はその高さになる。
        assert_eq!(at(shelves.allocate(40, 30).unwrap()), (40, 0, 40, 30));
        // 右に入らなければ、いちばん高いものの下に次の棚を始める。
        assert_eq!(at(shelves.allocate(30, 10).unwrap()), (0, 30, 30, 10));
        assert_eq!(at(shelves.allocate(70, 20).unwrap()), (30, 30, 70, 20));
        assert_eq!(at(shelves.allocate(10, 50).unwrap()), (0, 50, 10, 50));
        // 下に入らない、テクスチャより大きい。
        assert!(shelves.allocate(100, 1).is_err());
        assert!(Shelves::new(100).allocate(101, 1).is_err());
        assert!(Shelves::new(100).allocate(1, 101).is_err());
    }

    #[test]
    fn narrow_glyphs_are_centered_in_every_mode() {
        for wide_glyph in WideGlyph::ALL {
            assert_eq!(
                place_text(upright(10.0), 1, CELL_WIDTH, wide_glyph),
                placement(1, 1.0, 3.0)
            );
            assert_eq!(
                place_text(upright(32.0), 2, CELL_WIDTH, wide_glyph),
                placement(2, 1.0, 0.0)
            );
        }
    }

    #[test]
    fn wide_glyphs_shrink_into_their_cells() {
        assert_eq!(
            place_text(upright(32.0), 1, CELL_WIDTH, WideGlyph::Shrink),
            placement(1, 0.5, 0.0)
        );
    }

    #[test]
    fn clipped_glyphs_keep_their_size_at_the_left() {
        assert_eq!(
            place_text(upright(32.0), 1, CELL_WIDTH, WideGlyph::Clip),
            placement(1, 1.0, 0.0)
        );
        // 切る文字は、インクがはみ出しても余白を足さない。
        let slanted = TextExtent {
            advance: 32.0,
            ink_left: -2.0,
            ink_right: 35.0,
        };
        assert_eq!(
            place_text(slanted, 1, CELL_WIDTH, WideGlyph::Clip),
            placement(1, 1.0, 0.0)
        );
    }

    #[test]
    fn overflowing_glyphs_widen_the_slot_up_to_a_limit() {
        assert_eq!(
            place_text(upright(32.0), 1, CELL_WIDTH, WideGlyph::Overflow),
            placement(2, 1.0, 0.0)
        );
        // 少しでもはみ出せば、次のセルまで広げる。
        assert_eq!(
            place_text(upright(17.0), 1, CELL_WIDTH, WideGlyph::Overflow),
            placement(2, 1.0, 0.0)
        );
        // 2 倍を超える分は縮める。
        assert_eq!(
            place_text(upright(64.0), 1, CELL_WIDTH, WideGlyph::Overflow),
            placement(2, 0.5, 0.0)
        );
        assert_eq!(
            place_text(upright(48.0), 2, CELL_WIDTH, WideGlyph::Overflow),
            placement(3, 1.0, 0.0)
        );
    }

    /// `left` と `right` の余白を持つ置き方。
    fn with_margins(placement: TextPlacement, left: u32, right: u32) -> TextPlacement {
        TextPlacement {
            left_margin: left,
            right_margin: right,
            ..placement
        }
    }

    #[test]
    fn centered_overflowing_glyphs_spill_to_both_sides_up_to_a_limit() {
        let placed = place_text(upright(32.0), 1, CELL_WIDTH, WideGlyph::OverflowCenter);
        assert_eq!(placed, with_margins(placement(1, 1.0, -8.0), 8, 8));
        assert_eq!(placed.slot_width(CELL_WIDTH as u32), 32);
        // はみ出した分だけ余白を足す。半端なピクセルは切り上げる。
        assert_eq!(
            place_text(upright(17.0), 1, CELL_WIDTH, WideGlyph::OverflowCenter),
            with_margins(placement(1, 1.0, -0.5), 1, 1)
        );
        // 2 倍を超える分は縮める。
        assert_eq!(
            place_text(upright(64.0), 1, CELL_WIDTH, WideGlyph::OverflowCenter),
            with_margins(placement(1, 0.5, -8.0), 8, 8)
        );
        assert_eq!(
            place_text(upright(48.0), 2, CELL_WIDTH, WideGlyph::OverflowCenter),
            with_margins(placement(2, 1.0, -8.0), 8, 8)
        );
        // インクが送り幅の外へ出ていれば、その分も余白にする。
        let slanted = TextExtent {
            advance: 32.0,
            ink_left: -1.0,
            ink_right: 35.0,
        };
        assert_eq!(
            place_text(slanted, 1, CELL_WIDTH, WideGlyph::OverflowCenter),
            with_margins(placement(1, 1.0, -8.0), 9, 11)
        );
        // インクが送り幅より狭ければ、余白はインクの分だけ。
        let round = TextExtent {
            advance: 32.0,
            ink_left: 4.0,
            ink_right: 28.0,
        };
        assert_eq!(
            place_text(round, 1, CELL_WIDTH, WideGlyph::OverflowCenter),
            with_margins(placement(1, 1.0, -8.0), 4, 4)
        );
    }

    #[test]
    fn ink_outside_the_cell_gets_margins_without_shrinking() {
        // 送り幅はセルと同じで、インクが左へ 1.5、右へ 2.25 ピクセルはみ出す。
        let slanted = TextExtent {
            advance: 16.0,
            ink_left: -1.5,
            ink_right: 18.25,
        };
        for wide_glyph in WideGlyph::ALL {
            let placed = place_text(slanted, 1, CELL_WIDTH, wide_glyph);
            assert_eq!(
                placed,
                TextPlacement {
                    left_margin: 2,
                    right_margin: 3,
                    ..placement(1, 1.0, 0.0)
                }
            );
            assert_eq!(placed.slot_width(CELL_WIDTH as u32), 21);
        }
    }

    #[test]
    fn ink_margins_are_measured_after_shrinking_and_limited() {
        // 縮めたあとのインクで余白を決める。
        let slanted = TextExtent {
            advance: 32.0,
            ink_left: 0.0,
            ink_right: 36.0,
        };
        assert_eq!(
            place_text(slanted, 1, CELL_WIDTH, WideGlyph::Shrink),
            TextPlacement {
                right_margin: 2,
                ..placement(1, 0.5, 0.0)
            }
        );
        // 余白はセルの幅まで。
        let far = TextExtent {
            advance: 0.0,
            ink_left: -40.0,
            ink_right: 0.0,
        };
        assert_eq!(
            place_text(far, 1, CELL_WIDTH, WideGlyph::Shrink).left_margin,
            16
        );
        // 計算の誤差ほどのはみ出しには余白を足さない。
        let touching = TextExtent {
            advance: 16.0,
            ink_left: -0.001,
            ink_right: 16.001,
        };
        assert_eq!(
            place_text(touching, 1, CELL_WIDTH, WideGlyph::Shrink),
            placement(1, 1.0, 0.0)
        );
    }

    /// Consolas の斜体（太字なら太字斜体）の書体。
    fn italic_face(factory: &IDWriteFactory, weight: DWRITE_FONT_WEIGHT) -> IDWriteFontFace {
        let mut collection = None;
        let mut index = 0u32;
        let mut exists = BOOL::default();
        // SAFETY: 引数はどれも書き込める。FindFamilyName が見つけた番号で書体を取り出す。
        unsafe {
            factory
                .GetSystemFontCollection(&mut collection, false)
                .unwrap();
            let collection = collection.unwrap();
            collection
                .FindFamilyName(w!("Consolas"), &mut index, &mut exists)
                .unwrap();
            assert!(exists.as_bool());
            collection
                .GetFontFamily(index)
                .unwrap()
                .GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC)
                .unwrap()
                .CreateFontFace()
                .unwrap()
        }
    }

    /// `face` の `c` のインクの左右の端（ピクセル）。フォントの設計上の左右のサイドベアリングから求める。
    fn designed_ink(face: &IDWriteFontFace, c: char, size_px: f32) -> (f32, f32) {
        let codepoint = u32::from(c);
        let mut glyph = 0u16;
        let mut glyph_metrics = DWRITE_GLYPH_METRICS::default();
        let mut font_metrics = Default::default();
        // SAFETY: 引数はどれも 1 要素の配列として読み書きできる。
        unsafe {
            face.GetGlyphIndices(&codepoint, 1, &mut glyph).unwrap();
            face.GetDesignGlyphMetrics(&glyph, 1, &mut glyph_metrics, false)
                .unwrap();
            face.GetMetrics(&mut font_metrics);
        }
        let to_px = |units: f32| units * size_px / f32::from(font_metrics.designUnitsPerEm);
        let advance = glyph_metrics.advanceWidth as f32;
        (
            to_px(glyph_metrics.leftSideBearing as f32),
            to_px(advance - glyph_metrics.rightSideBearing as f32),
        )
    }

    #[test]
    fn italic_slots_contain_the_ink() {
        const DPI: u32 = 192;
        let factory = font::create_factory().unwrap();
        let font = font::find(&factory, &["Consolas"], 12.0, DPI).unwrap();
        let cell_width = font.cell_width_px as f32;
        let mut overhangs = (false, false);
        for weight in [DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_BOLD] {
            let format = text_format(&factory, &font, weight, DWRITE_FONT_STYLE_ITALIC).unwrap();
            let face = italic_face(&factory, weight);
            for c in ['d', 'f', 'W'] {
                let (_, extent) = text_layout(
                    &factory,
                    &format,
                    &c.to_string(),
                    font.cell_height_px as f32,
                )
                .unwrap();
                let placed = place_text(extent, 1, cell_width, WideGlyph::Shrink);
                assert_eq!(placed.scale, 1.0, "{c:?} is not shrunk");
                // 場所の左端と右端を、セルの左端からの距離で表す。
                let slot_left = -(placed.left_margin as f32);
                let slot_right = cell_width + placed.right_margin as f32;
                let (ink_left, ink_right) = designed_ink(&face, c, font.size_px);
                let (ink_left, ink_right) = (placed.offset + ink_left, placed.offset + ink_right);
                assert!(
                    slot_left <= ink_left && ink_right <= slot_right,
                    "{c:?} (weight {}): ink {ink_left}..{ink_right} is outside {slot_left}..{slot_right}",
                    weight.0
                );
                overhangs.0 |= ink_left < 0.0;
                overhangs.1 |= ink_right > cell_width;
            }
        }
        // 確認した文字のインクには、セルの左へも右へもはみ出すものがある。
        assert_eq!(overhangs, (true, true));
    }
}
