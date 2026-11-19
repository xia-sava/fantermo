//! グリフアトラス。DirectWrite で組んだ文字を Direct2D でテクスチャに描き、その場所を覚えておく。
//! タブバーの角や印のような形（[`Icon`]）も描く。
//!
//! 文字は白で、乗算済みのアルファで描く。色はシェーダで付ける。場所は棚に左から詰めていき（棚の
//! 高さはそこに置いたものの高いほう。文字は 1 セルの高さ）、いっぱいになったら全部を捨てて描き直す。
//! 斜体のようにインクがセルの左右へはみ出すグリフは、はみ出す分だけ左右に余白を持たせた場所に描く。
//! タブバーのタイトルは、Windows の UI のフォント（[`UiFont`]）で文字列を一つの絵として描く。
//!
//! [`Emoji::Color`] なら、色付きのフォント（Segoe UI Emoji など）の字形を色のまま描くため、色付きの
//! 字形を含む文字は白の文字と黒の文字で上下に並べて 2 回描く（[`GlyphAtlas::draw_glyph_text`]）。
//! 端末の文字は色付きのフォントで描くものだけ（[`ColorFonts`] で調べる）、タブバーのタイトルはすべてを
//! そう描く。[`Emoji::Monochrome`] なら、色付きのフォントも色のない字形で、1 回だけ描く。

use std::{collections::HashMap, rc::Rc};

use fantermo_core::config::{Emoji, WideGlyph};
use windows::{
    Win32::Graphics::{
        Direct2D::{
            Common::{D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT},
            D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
            D2D1_DRAW_TEXT_OPTIONS, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
            D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1DeviceContext,
            ID2D1Factory1, ID2D1SolidColorBrush,
        },
        Direct3D11::{
            D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11ShaderResourceView, ID3D11Texture2D,
        },
        DirectWrite::{
            DWRITE_FONT_STYLE, DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_GRID_FIT_MODE_DISABLED, DWRITE_LINE_METRICS,
            DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC, IDWriteFactory, IDWriteFactory2,
            IDWriteRenderingParams, IDWriteRenderingParams1, IDWriteTextFormat, IDWriteTextLayout,
        },
        Dxgi::{
            Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
            IDXGIDevice, IDXGISurface,
        },
    },
    core::Interface,
};
use windows_numerics::{Matrix3x2, Vector2};

use crate::{
    boxdraw::{self, Shape},
    color_font::ColorFonts,
    d2d_shape::{PIXEL_DPI, draw_shape},
    decoration::{self, Decoration, Placement},
    font::{self, Font},
    paint::Icon,
    text_placement::{INK_EPSILON_PX, TextPlacement, place_text, text_layout},
    ui_font::UiFont,
};

/// アトラスの一辺（ピクセル）。
const ATLAS_SIZE: u32 = 2048;

/// アトラスの一辺をテストが指定する環境変数。feature `e2e` のときだけ読む。配布物には入らない。
#[cfg(feature = "e2e")]
const SIDE_ENV: &str = "FANTERMO_E2E_ATLAS_SIDE";

/// 作るアトラスの一辺（ピクセル）。
#[cfg(not(feature = "e2e"))]
fn atlas_side() -> u32 {
    ATLAS_SIZE
}

/// 作るアトラスの一辺（ピクセル）。テストが指定していれば、その値。1 以上でテクスチャの上限以下の整数でなければ警告して既定の値。
#[cfg(feature = "e2e")]
fn atlas_side() -> u32 {
    use crate::image_decoder::MAX_TEXTURE_SIDE;

    let Ok(text) = std::env::var(SIDE_ENV) else {
        return ATLAS_SIZE;
    };
    match text.parse::<u32>() {
        Ok(side) if (1..=MAX_TEXTURE_SIDE).contains(&side) => side,
        _ => {
            tracing::warn!(
                "ignoring {SIDE_ENV}={text:?}: not an integer from 1 to {MAX_TEXTURE_SIDE}"
            );
            ATLAS_SIZE
        }
    }
}

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
    /// 色付きの字形を含む。この場所に白の文字で、すぐ下の同じ大きさの場所に黒の文字で描いてある
    /// （[`GlyphAtlas::draw_glyph_text`]）。
    pub color: bool,
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

    /// 幅 `width`、高さ `height` の場所。いまの棚の右に入らなければ、次の棚の左端に置く。取れなかった
    /// ときは何も変えないので、あとで取る、いまの棚の残りに入るものは取れる。
    fn allocate(&mut self, width: u32, height: u32) -> Result<Slot, AtlasFull> {
        if width > self.size || height > self.size {
            return Err(AtlasFull);
        }
        let (mut x, mut y, mut shelf_height) = (self.next_x, self.next_y, self.shelf_height);
        if x + width > self.size {
            x = 0;
            y += shelf_height;
            shelf_height = 0;
        }
        if y + height > self.size {
            return Err(AtlasFull);
        }
        self.next_x = x + width;
        self.next_y = y;
        self.shelf_height = shelf_height.max(height);
        Ok(Slot {
            x,
            y,
            width,
            height,
            left: 0,
            color: false,
        })
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
    /// テクスチャの一辺。
    side: u32,
    view: ID3D11ShaderResourceView,
    factory: ID2D1Factory1,
    context: ID2D1DeviceContext,
    /// 白のブラシ。
    brush: ID2D1SolidColorBrush,
    /// 黒のブラシ。色付きの字形を含む文字を描くのに使う（[`GlyphAtlas::draw_glyph_text`]）。
    black_brush: ID2D1SolidColorBrush,
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
    /// 色付きのフォントの字形の描き方。
    emoji: Emoji,
    /// 端末の文字を描くフォントを引くもの。[`Emoji::Color`] のときだけ作る。作れなければ、端末の文字は
    /// どれも色付きの字形を含まないものとして描く。
    color_fonts: Option<ColorFonts>,
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
    /// タブバーのタイトルを描く設定（[`label_rendering_params`]）。作れなければ既定の描き方。
    label_rendering: Option<IDWriteRenderingParams>,
}

impl GlyphAtlas {
    pub fn new(
        device: &ID3D11Device,
        dwrite: &IDWriteFactory,
        font: &Font,
        ui_font: Rc<UiFont>,
        wide_glyph: WideGlyph,
        emoji: Emoji,
    ) -> windows::core::Result<Self> {
        let side = atlas_side();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: side,
            Height: side,
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
        // SAFETY: context は有効な Direct2D のデバイスコンテキスト。
        let black_brush = unsafe {
            context.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                },
                None,
            )?
        };

        let format = |bold, italic| {
            let (weight, style) = font_variant(bold, italic);
            text_format(dwrite, font, weight, style)
        };
        // 添字は format_index の順。
        let formats = [
            format(false, false)?,
            format(true, false)?,
            format(false, true)?,
            format(true, true)?,
        ];
        let placement = Placement::new(
            baseline(dwrite, &formats[0], font.cell_height_px as f32)?,
            &font.lines,
            font.cell_height_px as f32,
        );
        let color_fonts = match emoji {
            Emoji::Color => ColorFonts::new(dwrite, &font.family)
                .inspect_err(|e| {
                    tracing::warn!("failed to prepare finding the color fonts: {e}");
                })
                .ok(),
            Emoji::Monochrome => None,
        };

        let mut atlas = Self {
            side,
            view,
            factory,
            context,
            brush,
            black_brush,
            dwrite: dwrite.clone(),
            formats,
            cell_width: font.cell_width_px,
            cell_height: font.cell_height_px,
            line_thickness: line_thickness(font.cell_width_px),
            placement,
            wide_glyph,
            emoji,
            color_fonts,
            shelves: Shelves::new(side),
            slots: HashMap::new(),
            decoration_slots: HashMap::new(),
            icon_slots: HashMap::new(),
            ui_font,
            label_slots: HashMap::new(),
            label_rendering: label_rendering_params(dwrite)
                .inspect_err(|e| tracing::warn!("failed to create the title rendering params: {e}"))
                .ok(),
        };
        atlas.clear()?;
        Ok(atlas)
    }

    pub fn view(&self) -> &ID3D11ShaderResourceView {
        &self.view
    }

    pub fn size(&self) -> (u32, u32) {
        (self.side, self.side)
    }

    /// 覚えている場所を捨て、テクスチャを空にする。
    pub fn clear(&mut self) -> windows::core::Result<()> {
        self.slots.clear();
        self.decoration_slots.clear();
        self.icon_slots.clear();
        self.label_slots.clear();
        self.shelves = Shelves::new(self.side);
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
    ///
    /// [`Emoji::Color`] なら、色付きの絵文字を色のまま描けるよう、タイトルはどれも色付きの字形を含む
    /// ものとして描く（[`GlyphAtlas::draw_glyph_text`]）。
    pub fn label_slot(&mut self, text: &str) -> Result<Slot, AtlasFull> {
        if let Some(slot) = self.label_slots.get(text) {
            return Ok(*slot);
        }
        let height = self.ui_font.height_px();
        let (layout, extent) = match self.ui_font.layout(text) {
            Ok(measured) => measured,
            Err(e) => {
                // 空の場所も覚えて、次からはそれを返す。
                tracing::warn!("failed to lay out the title {text:?}: {e}");
                let slot = self.allocate_text(1, height, false)?;
                self.label_slots.insert(text.to_owned(), slot);
                return Ok(slot);
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
        let color = self.emoji == Emoji::Color;
        let slot = Slot {
            left: placement.left_margin,
            ..self.allocate_text(width, height, color)?
        };
        // SAFETY: context は有効で、描画の設定は self が持っている。
        unsafe {
            self.context
                .SetTextRenderingParams(self.label_rendering.as_ref())
        };
        if let Err(e) = self.draw_glyph_text(&layout, &placement, slot) {
            tracing::warn!("failed to rasterize the title {text:?}: {e}");
        }
        // SAFETY: context は有効。端末の文字は既定の描き方に戻す。
        unsafe { self.context.SetTextRenderingParams(None) };
        self.label_slots.insert(text.to_owned(), slot);
        Ok(slot)
    }

    /// 文字を描く、幅 `width`、高さ `height` の場所。`color` なら色付きの字形を含む文字の場所で、
    /// すぐ下に同じ大きさの場所も取る（[`GlyphAtlas::draw_glyph_text`]）。
    fn allocate_text(&mut self, width: u32, height: u32, color: bool) -> Result<Slot, AtlasFull> {
        let rows = if color { 2 } else { 1 };
        Ok(Slot {
            height,
            color,
            ..self.allocate_area(width, height * rows)?
        })
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
        let color = self.contains_color(key);
        let slot = Slot {
            left: placement.left_margin,
            ..self.allocate_text(
                placement.slot_width(self.cell_width),
                self.cell_height,
                color,
            )?
        };
        if let Err(e) = self.draw_glyph_text(&layout, &placement, slot) {
            tracing::warn!("failed to rasterize {:?}: {e}", key.text);
        }
        Ok(slot)
    }

    /// 端末の文字 `key` が色付きのフォントで描く文字を含むか。調べられなければ含まないとする。
    fn contains_color(&self, key: &GlyphKey) -> bool {
        let Some(color_fonts) = &self.color_fonts else {
            return false;
        };
        let (weight, style) = font_variant(key.bold, key.italic);
        color_fonts
            .contains_color(&key.text, weight, style)
            .unwrap_or_else(|e| {
                tracing::warn!("failed to find the font of {:?}: {e}", key.text);
                false
            })
    }

    /// 組んだ文字を `placement` のとおりに `slot` に白の文字で描く。
    ///
    /// 色付きの字形を含む場所（[`Slot::color`]）なら、そのすぐ下の同じ大きさの場所にも黒の文字で描く。
    /// 色付きの字形は文字の色によらず同じ色に描かれるので、二つの差が文字の色で塗る部分になる
    /// （`cells.hlsl` の `KIND_COLOR_GLYPH`）。
    fn draw_glyph_text(
        &self,
        layout: &IDWriteTextLayout,
        placement: &TextPlacement,
        slot: Slot,
    ) -> windows::core::Result<()> {
        self.draw_text(layout, placement, slot, &self.brush)?;
        if !slot.color {
            return Ok(());
        }
        let below = Slot {
            y: slot.y + slot.height,
            ..slot
        };
        self.draw_text(layout, placement, below, &self.black_brush)
    }

    /// 組んだ文字を `placement` のとおりに `slot` に `brush` で描く。
    fn draw_text(
        &self,
        layout: &IDWriteTextLayout,
        placement: &TextPlacement,
        slot: Slot,
        brush: &ID2D1SolidColorBrush,
    ) -> windows::core::Result<()> {
        let transform = Matrix3x2 {
            M11: placement.scale,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: (slot.x + slot.left) as f32 + placement.offset,
            M32: slot.y as f32,
        };
        let options = draw_text_options(self.emoji);
        self.draw_in_slot(slot, &transform, |context| {
            // SAFETY: 描画中の context と、同じ context が作ったブラシ。layout はこの呼び出しの間は生きている。
            unsafe {
                context.DrawTextLayout(Vector2 { X: 0.0, Y: 0.0 }, layout, brush, options);
            }
            Ok(())
        })
    }
}

/// 文字を描くときの指定。[`Emoji::Monochrome`] なら、色付きのフォントも色のない字形で描く。
fn draw_text_options(emoji: Emoji) -> D2D1_DRAW_TEXT_OPTIONS {
    match emoji {
        Emoji::Color => D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
        Emoji::Monochrome => D2D1_DRAW_TEXT_OPTIONS_NONE,
    }
}

/// タブバーのタイトルを描く設定。縦横どちらにもアンチエイリアスをかけ、ヒンティングでピクセルの格子に
/// 合わせない。ガンマやコントラストはシステムの既定のまま。
///
/// 既定の描き方では、UI のフォント（Yu Gothic UI など）の漢字やかなはヒンティングで格子に強く合わせられ、
/// 斜めの線と曲線が階段状になる。端末の文字の描き方は変えず、この設定はタイトルにだけ使う。
fn label_rendering_params(
    dwrite: &IDWriteFactory,
) -> windows::core::Result<IDWriteRenderingParams> {
    let factory: IDWriteFactory2 = dwrite.cast()?;
    // SAFETY: factory と既定の設定はこの呼び出しの間は生きている。
    unsafe {
        let base: IDWriteRenderingParams1 = dwrite.CreateRenderingParams()?.cast()?;
        factory
            .CreateCustomRenderingParams(
                base.GetGamma(),
                base.GetEnhancedContrast(),
                base.GetGrayscaleEnhancedContrast(),
                base.GetClearTypeLevel(),
                base.GetPixelGeometry(),
                DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
                DWRITE_GRID_FIT_MODE_DISABLED,
            )?
            .cast()
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

/// 太字・斜体の組み合わせの、フォントの太さと形。
fn font_variant(bold: bool, italic: bool) -> (DWRITE_FONT_WEIGHT, DWRITE_FONT_STYLE) {
    let weight = if bold {
        DWRITE_FONT_WEIGHT_BOLD
    } else {
        DWRITE_FONT_WEIGHT_NORMAL
    };
    let style = if italic {
        DWRITE_FONT_STYLE_ITALIC
    } else {
        DWRITE_FONT_STYLE_NORMAL
    };
    (weight, style)
}

fn text_format(
    dwrite: &IDWriteFactory,
    font: &Font,
    weight: DWRITE_FONT_WEIGHT,
    style: DWRITE_FONT_STYLE,
) -> windows::core::Result<IDWriteTextFormat> {
    font::text_format(dwrite, &font.family, font.size_px, weight, style)
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
    fn a_slot_that_does_not_fit_leaves_the_shelf_as_it_was() {
        let mut shelves = Shelves::new(100);
        let at = |slot: Slot| (slot.x, slot.y);
        assert_eq!(at(shelves.allocate(60, 60).unwrap()), (0, 0));
        // 右に入らず、次の棚の下にも入らない。失敗しても、いまの棚は進まない。
        assert!(shelves.allocate(50, 50).is_err());
        assert!(shelves.allocate(50, 50).is_err());
        // いまの棚の残りに入るものは入る。
        assert_eq!(at(shelves.allocate(40, 40).unwrap()), (60, 0));
        assert_eq!(at(shelves.allocate(30, 30).unwrap()), (0, 60));
    }
}
