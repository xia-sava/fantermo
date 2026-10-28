//! Direct3D 11 で窓に描く。

use std::path::Path;

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
        Dxgi::{
            Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC},
            CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_NONE,
            DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_DISCARD,
            DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory2, IDXGISwapChain1,
        },
    },
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
}

impl Renderer {
    pub fn new(hwnd: HWND, width: u32, height: u32) -> windows::core::Result<Self> {
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

        let mut renderer = Self {
            device,
            context,
            swap_chain,
            target: None,
            width: desc.Width,
            height: desc.Height,
        };
        renderer.create_target()?;
        Ok(renderer)
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

    /// 1 フレームを描いて表示する。`capture` を渡すと、表示する前のバックバッファを PNG で書き出す。
    pub fn render(
        &mut self,
        background: Rgba,
        capture: Option<&Path>,
    ) -> windows::core::Result<()> {
        if let Some(target) = &self.target {
            // SAFETY: target は現在のバックバッファのビュー。
            unsafe { self.context.ClearRenderTargetView(target, &background) };
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
