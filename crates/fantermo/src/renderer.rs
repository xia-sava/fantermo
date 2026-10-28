//! Direct3D 11 で窓に描く。

use std::path::Path;

use fantermo_core::{
    config::WideGlyph,
    frame::{Frame, Rgb, Underline},
};
use windows::Win32::{
    Foundation::{HMODULE, HWND},
    Graphics::{
        Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP},
        Direct3D11::{
            D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
            D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
            D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView,
            ID3D11Texture2D,
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
    decoration::Decoration,
    font::Font,
    pipeline::{CellPipeline, Instance, InstanceKind},
};

/// スワップチェーンのバッファの数。
const SWAP_CHAIN_BUFFERS: u32 = 2;

/// 垂直同期を待って表示する。
const PRESENT_SYNC_INTERVAL: u32 = 1;

/// 0.0〜1.0 の RGBA。
pub type Rgba = [f32; 4];

/// バックバッファ（窓のクライアント領域）の中の矩形。単位はピクセル。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

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
}

impl Renderer {
    pub fn new(
        hwnd: HWND,
        width: u32,
        height: u32,
        dwrite: &IDWriteFactory,
        font: &Font,
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
        // SAFETY: hwnd はこのスレッドが作った有効な窓で、desc は初期化済み。
        let swap_chain =
            unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)? };

        let atlas = GlyphAtlas::new(&device, dwrite, font, wide_glyph)?;
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
        };
        renderer.create_target()?;
        Ok(renderer)
    }

    /// `font` で、セルより広いグリフは `wide_glyph` のとおりに描くようにする。DPI やフォントの設定が
    /// 変わったときに使う。グリフアトラスは作り直す。
    pub fn set_font(
        &mut self,
        dwrite: &IDWriteFactory,
        font: &Font,
        wide_glyph: WideGlyph,
    ) -> windows::core::Result<()> {
        self.atlas = GlyphAtlas::new(&self.device, dwrite, font, wide_glyph)?;
        self.cell_width = font.cell_width_px;
        self.cell_height = font.cell_height_px;
        Ok(())
    }

    /// 窓のクライアント領域の大きさに合わせてバッファを作り直す。
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

    /// 画面 `layers` を順に、それぞれ左上のセルが領域の左上に来るように描いて表示する。それぞれの
    /// 領域はその画面の背景色で塗ってから描き、どの領域にも入らないところは最初の画面の背景色で塗る。
    /// `capture` を渡すと、表示する前のバックバッファを PNG で書き出す。
    pub fn render(
        &mut self,
        layers: &[(&Frame, Area)],
        capture: Option<&Path>,
    ) -> windows::core::Result<()> {
        let Some((first, _)) = layers.first() else {
            return Ok(());
        };
        let instances = match self.instances(layers) {
            Ok(instances) => instances,
            Err(AtlasFull) => {
                // 描きかけのフレームの場所は捨てたアトラスを指すので、空にしてから組み直す。
                self.atlas.clear()?;
                self.instances(layers).unwrap_or_else(|AtlasFull| {
                    tracing::warn!("the glyph atlas cannot hold one frame");
                    Vec::new()
                })
            }
        };
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

    /// 画面ごとに、領域の背景、セルの背景、カーソル、グリフ、装飾の順に四角形を並べる。
    fn instances(&mut self, layers: &[(&Frame, Area)]) -> Result<Vec<Instance>, AtlasFull> {
        let mut instances = Vec::new();
        for &(frame, area) in layers {
            instances.push(solid(
                [
                    area.x as f32,
                    area.y as f32,
                    area.width as f32,
                    area.height as f32,
                ],
                frame.background,
            ));
            instances.append(&mut self.frame_instances(frame, area)?);
        }
        Ok(instances)
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
                // 右のセルへはみ出して描くグリフは、場所がセルより広い。
                let [x, y, _, height] = rect;
                glyphs.push(glyph([x, y, slot.width as f32, height], slot, color));
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
    Instance {
        rect,
        uv: [0.0; 4],
        color: rgba(color),
        kind: InstanceKind::Solid,
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
