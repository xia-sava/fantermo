//! Direct3D 11 でウィンドウに描く。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
};

use fantermo_core::{
    background::{self, Placement},
    config::{BackgroundFit, WideGlyph},
    frame::{Frame, Rgb, Underline},
};
use windows::Win32::{
    Foundation::{HMODULE, HWND},
    Graphics::{
        Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP},
        Direct3D11::{
            D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
            D3D11_RESOURCE_MISC_GENERATE_MIPS, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device,
            ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11ShaderResourceView, ID3D11Texture2D,
        },
        DirectWrite::IDWriteFactory,
        Dxgi::{
            Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC},
            CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_NONE,
            DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_DISCARD,
            DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory2, IDXGISwapChain1,
        },
    },
};

use crate::{
    atlas::{AtlasFull, GlyphAtlas, Slot},
    background::{Decoded, Decoder},
    decoration::Decoration,
    font::Font,
    paint::{Area, Paint},
    pipeline::{CellPipeline, Instance, InstanceKind},
    ui_font::UiFont,
};

/// スワップチェーンのバッファの数。
const SWAP_CHAIN_BUFFERS: u32 = 2;

/// 垂直同期を待って表示する。
const PRESENT_SYNC_INTERVAL: u32 = 1;

/// 0.0〜1.0 の RGBA。
pub type Rgba = [f32; 4];

pub struct Renderer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swap_chain: IDXGISwapChain1,
    target: Option<ID3D11RenderTargetView>,
    width: u32,
    height: u32,
    atlas: GlyphAtlas,
    pipeline: CellPipeline,
    cell_width: u32,
    cell_height: u32,
    /// 背景画像を読むもの。初めて読むときに作る。
    decoder: Option<Decoder>,
    /// 読んだ背景画像。ファイルの場所ごと。
    images: HashMap<PathBuf, ImageTexture>,
}

/// 背景画像のテクスチャ（ミップマップつき）。
struct ImageTexture {
    view: ID3D11ShaderResourceView,
    width: u32,
    height: u32,
}

/// 端末の背景に敷く画像の描き方。
pub struct Backdrop<'a> {
    /// 画像のファイル。[`Renderer::load_image`] で読んであること（なければ画像を描かない）。
    pub image: &'a Path,
    /// 画像を敷く範囲（端末の画面と余白）。
    pub region: Area,
    pub fit: BackgroundFit,
    /// 画像の不透明度（0〜1。1 で画像をそのまま見せる）。
    pub image_opacity: f32,
}

impl Renderer {
    pub fn new(
        hwnd: HWND,
        width: u32,
        height: u32,
        dwrite: &IDWriteFactory,
        font: &Font,
        ui_font: Rc<UiFont>,
        wide_glyph: WideGlyph,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (device, context) = create_device()?;
        // SAFETY: フラグなしでファクトリを作るだけ。
        let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width.max(1),
            Height: height.max(1),
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: SWAP_CHAIN_BUFFERS,
            Scaling: DXGI_SCALING_NONE,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ..Default::default()
        };
        // SAFETY: hwnd はこのスレッドが作った有効なウィンドウで、desc は初期化済み。
        let swap_chain =
            unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)? };

        let atlas = GlyphAtlas::new(&device, dwrite, font, ui_font, wide_glyph)?;
        let pipeline = CellPipeline::new(&device)?;
        let mut renderer = Self {
            device,
            context,
            swap_chain,
            target: None,
            width: desc.Width,
            height: desc.Height,
            atlas,
            pipeline,
            cell_width: font.cell_width_px,
            cell_height: font.cell_height_px,
            decoder: None,
            images: HashMap::new(),
        };
        renderer.create_target()?;
        Ok(renderer)
    }

    /// 背景画像 `path` を読んでテクスチャにする。読んであれば何もしない。
    pub fn load_image(&mut self, path: &Path) -> windows::core::Result<()> {
        if self.images.contains_key(path) {
            return Ok(());
        }
        let decoder = match &self.decoder {
            Some(decoder) => decoder,
            None => self.decoder.insert(Decoder::new()?),
        };
        let decoded = decoder.decode(path)?;
        let texture = self.create_image(&decoded)?;
        tracing::info!(
            "loaded the background image {} ({}x{})",
            path.display(),
            texture.width,
            texture.height
        );
        self.images.insert(path.to_owned(), texture);
        Ok(())
    }

    /// 読んだ背景画像のうち、`keep` が偽のものを捨てる。
    pub fn retain_images(&mut self, keep: impl Fn(&Path) -> bool) {
        self.images.retain(|path, _| keep(path));
    }

    fn create_image(&self, decoded: &Decoded) -> windows::core::Result<ImageTexture> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: decoded.width,
            Height: decoded.height,
            // 0 はすべてのレベルのミップマップ。縮めて描くときに粗くならないように持つ。
            MipLevels: 0,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            // ミップマップを作らせるには描画先にもできること。
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
            ..Default::default()
        };
        let mut texture = None;
        // SAFETY: desc は初期化済みで、texture は書き込める。
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))?
        };
        let texture: ID3D11Texture2D = texture.ok_or_else(windows::core::Error::empty)?;
        // SAFETY: pixels は stride × 高さのバイトを持ち、texture のレベル 0 と同じ大きさと形式。
        unsafe {
            self.context.UpdateSubresource(
                &texture,
                0,
                None,
                decoded.pixels.as_ptr().cast(),
                decoded.stride(),
                0,
            )
        };
        let mut view = None;
        // SAFETY: texture はシェーダから読めるテクスチャで、view は書き込める。
        unsafe {
            self.device
                .CreateShaderResourceView(&texture, None, Some(&mut view))?
        };
        let view = view.ok_or_else(windows::core::Error::empty)?;
        // SAFETY: view は GENERATE_MIPS を付けて作ったテクスチャのすべてのレベルのビュー。
        unsafe { self.context.GenerateMips(&view) };
        Ok(ImageTexture {
            view,
            width: decoded.width,
            height: decoded.height,
        })
    }

    /// 端末を `font` で、タブバーのタイトルを `ui_font` で描き、セルより広いグリフは `wide_glyph` の
    /// とおりに描くようにする。DPI やフォントの設定が変わったときに使う。グリフアトラスは作り直す。
    pub fn set_font(
        &mut self,
        dwrite: &IDWriteFactory,
        font: &Font,
        ui_font: Rc<UiFont>,
        wide_glyph: WideGlyph,
    ) -> windows::core::Result<()> {
        self.atlas = GlyphAtlas::new(&self.device, dwrite, font, ui_font, wide_glyph)?;
        self.cell_width = font.cell_width_px;
        self.cell_height = font.cell_height_px;
        Ok(())
    }

    /// ウィンドウのクライアント領域の大きさに合わせてバッファを作り直す。
    pub fn resize(&mut self, width: u32, height: u32) -> windows::core::Result<()> {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.width, self.height) {
            return Ok(());
        }
        // バッファを参照するビューを残したままでは作り直せない。
        self.target = None;
        // SAFETY: バッファへの参照はすべて手放した。
        unsafe {
            self.swap_chain.ResizeBuffers(
                0,
                width,
                height,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?
        };
        self.width = width;
        self.height = height;
        self.create_target()
    }

    /// 画面 `layers` を順に、それぞれ左上のセルが領域の左上に来るように描き、その上に `paints` を
    /// 順に描いて表示する。それぞれの領域はその画面の背景色で塗ってから描き、どの領域にも入らない
    /// ところは最初の画面の背景色で塗る。
    ///
    /// `backdrop` があれば、最初に画像を描き、その範囲に最初の画面の背景色を、画像が `image_opacity` の
    /// 不透明度に見える不透明度（[`background::overlay_opacity`]）で重ねる。
    /// 画面の領域は塗らず、背景色を持つセルだけを塗る。
    ///
    /// `capture` を渡すと、表示する前のバックバッファを PNG で書き出す。
    pub fn render(
        &mut self,
        layers: &[(&Frame, Area)],
        backdrop: Option<&Backdrop>,
        paints: &[Paint],
        capture: Option<&Path>,
    ) -> windows::core::Result<()> {
        let Some((first, _)) = layers.first() else {
            return Ok(());
        };
        let placed = backdrop.and_then(|backdrop| {
            let texture = self.images.get(backdrop.image)?;
            let placement = background::place(
                backdrop.fit,
                background::Rect {
                    x: backdrop.region.x as f32,
                    y: backdrop.region.y as f32,
                    width: backdrop.region.width as f32,
                    height: backdrop.region.height as f32,
                },
                (texture.width, texture.height),
            )?;
            Some((texture.view.clone(), placement, backdrop))
        });
        let mut instances = Vec::new();
        if let Some((_, placement, backdrop)) = &placed {
            instances.push(image_instance(placement));
            instances.push(translucent(
                area_rect(backdrop.region),
                first.background,
                background::overlay_opacity(backdrop.image_opacity),
            ));
        }
        let fill_areas = placed.is_none();
        let layer_instances = match self.instances(layers, paints, fill_areas) {
            Ok(instances) => instances,
            Err(AtlasFull) => {
                // 描きかけのフレームの場所は捨てたアトラスを指すので、空にしてから組み直す。
                self.atlas.clear()?;
                self.instances(layers, paints, fill_areas)
                    .unwrap_or_else(|AtlasFull| {
                        tracing::warn!("the glyph atlas cannot hold one frame");
                        Vec::new()
                    })
            }
        };
        instances.extend(layer_instances);
        let image = placed.as_ref().map(|(view, _, _)| view);
        if let Some(target) = &self.target {
            // SAFETY: target は現在のバックバッファのビュー。
            unsafe {
                self.context
                    .ClearRenderTargetView(target, &rgba(first.background))
            };
            self.pipeline.draw(
                &self.device,
                &self.context,
                target,
                (self.width, self.height),
                self.atlas.view(),
                self.atlas.size(),
                image,
                &instances,
            )?;
        }
        if let Some(path) = capture
            && let Err(e) = self.capture_png(path)
        {
            tracing::warn!("failed to capture the back buffer: {e}");
        }
        // SAFETY: スワップチェーンは有効で、フラグはない。
        unsafe {
            self.swap_chain
                .Present(PRESENT_SYNC_INTERVAL, DXGI_PRESENT(0))
                .ok()
        }
    }

    /// 画面ごとに、領域の背景（`fill_areas` のときだけ）、セルの背景、カーソル、グリフ、装飾の順に
    /// 四角形を並べ、続けて `paints` を並べる。
    fn instances(
        &mut self,
        layers: &[(&Frame, Area)],
        paints: &[Paint],
        fill_areas: bool,
    ) -> Result<Vec<Instance>, AtlasFull> {
        let mut instances = Vec::new();
        for &(frame, area) in layers {
            if fill_areas {
                instances.push(solid(area_rect(area), frame.background));
            }
            instances.append(&mut self.frame_instances(frame, area)?);
        }
        for paint in paints {
            instances.push(self.paint_instance(paint)?);
        }
        Ok(instances)
    }

    /// `paint` を描く四角形。
    fn paint_instance(&mut self, paint: &Paint) -> Result<Instance, AtlasFull> {
        Ok(match paint {
            Paint::Fill { area, color } => solid(area_rect(*area), *color),
            Paint::Icon { icon, area, color } => {
                let slot = self.atlas.icon_slot(*icon, area.width, area.height)?;
                glyph(
                    [
                        area.x as f32,
                        area.y as f32,
                        slot.width as f32,
                        slot.height as f32,
                    ],
                    slot,
                    *color,
                )
            }
            Paint::Label { x, y, text, color } => {
                let slot = self.atlas.label_slot(text)?;
                glyph(
                    [
                        *x as f32 - slot.left as f32,
                        *y as f32,
                        slot.width as f32,
                        slot.height as f32,
                    ],
                    slot,
                    *color,
                )
            }
        })
    }

    /// 背景、カーソル、グリフ、装飾の順に四角形を並べる。カーソルの上の文字は背景色で描く。
    /// 取り消し線と上線は文字と同じ色、下線は指定された色（なければ文字と同じ色）で描く。
    fn frame_instances(&mut self, frame: &Frame, area: Area) -> Result<Vec<Instance>, AtlasFull> {
        let mut backgrounds = Vec::new();
        let mut glyphs = Vec::new();
        let mut decorations = Vec::new();
        let cursor = frame.cursor;
        for cell in &frame.cells {
            let rect = self.cell_rect(area, cell.col, cell.row, cell.width);
            if let Some(bg) = cell.bg {
                backgrounds.push(solid(rect, bg));
            }
            let under_cursor =
                cursor.is_some_and(|cursor| cursor.col == cell.col && cursor.row == cell.row);
            let color = if under_cursor {
                cell.bg.unwrap_or(frame.background)
            } else {
                cell.fg
            };
            if !cell.text.is_empty() && cell.text != " " {
                let slot = self
                    .atlas
                    .slot(&cell.text, cell.bold, cell.italic, cell.width)?;
                // 左右のセルへはみ出して描くグリフは、場所がセルより広い。
                let [x, y, _, height] = rect;
                glyphs.push(glyph(
                    [x - slot.left as f32, y, slot.width as f32, height],
                    slot,
                    color,
                ));
            }
            if cell.underline != Underline::None {
                let slot = self
                    .atlas
                    .decoration_slot(Decoration::Underline(cell.underline), cell.width)?;
                decorations.push(glyph(rect, slot, cell.underline_color.unwrap_or(color)));
            }
            if cell.strikethrough {
                let slot = self
                    .atlas
                    .decoration_slot(Decoration::Strikethrough, cell.width)?;
                decorations.push(glyph(rect, slot, color));
            }
            if cell.overline {
                let slot = self
                    .atlas
                    .decoration_slot(Decoration::Overline, cell.width)?;
                decorations.push(glyph(rect, slot, color));
            }
        }
        if let Some(cursor) = cursor {
            backgrounds.push(solid(
                self.cell_rect(area, cursor.col, cursor.row, cursor.width),
                cursor.color,
            ));
        }
        backgrounds.append(&mut glyphs);
        backgrounds.append(&mut decorations);
        Ok(backgrounds)
    }

    /// `area` に描く端末の、(`col`, `row`) から `width` セル分の矩形。
    fn cell_rect(&self, area: Area, col: u16, row: u16, width: u8) -> [f32; 4] {
        [
            (area.x + u32::from(col) * self.cell_width) as f32,
            (area.y + u32::from(row) * self.cell_height) as f32,
            (u32::from(width) * self.cell_width) as f32,
            self.cell_height as f32,
        ]
    }

    fn create_target(&mut self) -> windows::core::Result<()> {
        // SAFETY: バッファ 0 はスワップチェーンが持つ有効なテクスチャ。
        let buffer: ID3D11Texture2D = unsafe { self.swap_chain.GetBuffer(0)? };
        let mut target = None;
        // SAFETY: buffer は描画先に使えるテクスチャで、target は書き込める。
        unsafe {
            self.device
                .CreateRenderTargetView(&buffer, None, Some(&mut target))?
        };
        self.target = target;
        Ok(())
    }

    /// バックバッファを CPU から読めるテクスチャに写し、PNG に書き出す。
    fn capture_png(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        // SAFETY: バッファ 0 はスワップチェーンが持つ有効なテクスチャ。
        let buffer: ID3D11Texture2D = unsafe { self.swap_chain.GetBuffer(0)? };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: desc は書き込める。
        unsafe { buffer.GetDesc(&mut desc) };
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;

        let mut staging = None;
        // SAFETY: desc は初期化済みで、staging は書き込める。
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut staging))?
        };
        let staging = staging.ok_or("CreateTexture2D returned no texture")?;
        // SAFETY: 二つのテクスチャは同じ大きさと形式。
        unsafe { self.context.CopyResource(&staging, &buffer) };

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: staging は CPU から読めるテクスチャで、mapped は書き込める。
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?
        };
        let (width, height) = (desc.Width as usize, desc.Height as usize);
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            // SAFETY: Map が成功したので、pData から RowPitch × 高さのバイトが読める。
            let row = unsafe {
                std::slice::from_raw_parts(
                    mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                    width * 4,
                )
            };
            for bgra in row.chunks_exact(4) {
                rgba.extend_from_slice(&[bgra[2], bgra[1], bgra[0], u8::MAX]);
            }
        }
        // SAFETY: 直前に Map したサブリソースを閉じる。
        unsafe { self.context.Unmap(&staging, 0) };

        write_png(path, desc.Width, desc.Height, &rgba)
    }
}

/// `area` の左上の x, y と幅, 高さ。
fn area_rect(area: Area) -> [f32; 4] {
    [
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    ]
}

fn rgba(color: Rgb) -> Rgba {
    [
        f32::from(color.r) / 255.0,
        f32::from(color.g) / 255.0,
        f32::from(color.b) / 255.0,
        1.0,
    ]
}

/// アトラスの `slot` の形を、`color` で `rect` に描く。
fn glyph(rect: [f32; 4], slot: Slot, color: Rgb) -> Instance {
    Instance {
        rect,
        uv: [
            slot.x as f32,
            slot.y as f32,
            slot.width as f32,
            slot.height as f32,
        ],
        color: rgba(color),
        kind: InstanceKind::Glyph,
        _padding: [0; 3],
    }
}

fn solid(rect: [f32; 4], color: Rgb) -> Instance {
    translucent(rect, color, 1.0)
}

/// `rect` を `color` で、不透明度 `alpha`（0〜1）で塗る。
fn translucent(rect: [f32; 4], color: Rgb, alpha: f32) -> Instance {
    let [r, g, b, _] = rgba(color);
    Instance {
        rect,
        uv: [0.0; 4],
        color: [r, g, b, alpha.clamp(0.0, 1.0)],
        kind: InstanceKind::Solid,
        _padding: [0; 3],
    }
}

/// 背景画像を `placement` のとおりに描く。
fn image_instance(placement: &Placement) -> Instance {
    let Placement { rect, uv, repeat } = *placement;
    Instance {
        rect: [rect.x, rect.y, rect.width, rect.height],
        uv: [uv.x, uv.y, uv.width, uv.height],
        color: [1.0; 4],
        kind: if repeat {
            InstanceKind::ImageRepeat
        } else {
            InstanceKind::Image
        },
        _padding: [0; 3],
    }
}

fn create_device() -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    match create_device_with(D3D_DRIVER_TYPE_HARDWARE) {
        Ok(created) => Ok(created),
        Err(e) => {
            tracing::warn!("no hardware Direct3D 11 device, falling back to WARP: {e}");
            create_device_with(D3D_DRIVER_TYPE_WARP)
        }
    }
}

fn create_device_with(
    driver_type: D3D_DRIVER_TYPE,
) -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    // SAFETY: device と context は書き込める。BGRA に対応させるのは、グリフを Direct2D で描くため。
    unsafe {
        D3D11CreateDevice(
            None,
            driver_type,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?
    };
    let device = device.ok_or_else(windows::core::Error::empty)?;
    let context = context.ok_or_else(windows::core::Error::empty)?;
    Ok((device, context))
}

fn write_png(
    path: &Path,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(())
}
