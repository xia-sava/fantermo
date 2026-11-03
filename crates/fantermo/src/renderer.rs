//! Direct3D 11 でウィンドウに描く。

use std::{
    collections::HashMap,
    ffi::c_void,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
    rc::Rc,
};

use fantermo_core::{
    background::{self, Placement},
    config::{BackgroundFit, WideGlyph},
    frame::{Frame, ImageKey, ImageLayer, ImageView, Rgb, Underline},
    vt::Vt,
};
use windows::{
    Win32::{
        Foundation::{D2DERR_RECREATE_TARGET, HANDLE, HMODULE, HWND, INVALID_HANDLE_VALUE},
        Graphics::{
            Direct3D::{
                D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL,
                D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0,
                D3D_FEATURE_LEVEL_11_1,
            },
            Direct3D11::{
                D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
                D3D11_RESOURCE_MISC_GENERATE_MIPS, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device,
                ID3D11Device4, ID3D11DeviceContext, ID3D11RenderTargetView,
                ID3D11ShaderResourceView, ID3D11Texture2D,
            },
            DirectWrite::IDWriteFactory,
            Dxgi::{
                Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC},
                CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_ERROR_DEVICE_REMOVED,
                DXGI_ERROR_DEVICE_RESET, DXGI_ERROR_DRIVER_INTERNAL_ERROR, DXGI_MWA_NO_ALT_ENTER,
                DXGI_MWA_NO_WINDOW_CHANGES, DXGI_PRESENT, DXGI_SCALING_NONE, DXGI_SWAP_CHAIN_DESC1,
                DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_DISCARD,
                DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory2, IDXGISwapChain1,
            },
        },
        System::Threading::{
            CreateEventW, INFINITE, RegisterWaitForSingleObject, UnregisterWaitEx,
            WT_EXECUTEONLYONCE,
        },
    },
    core::{Interface, PCWSTR},
};

use crate::{
    atlas::{AtlasFull, GlyphAtlas, Slot},
    decoration::Decoration,
    font::Font,
    image_cache::ImageCache,
    image_decoder::{Decoded, Decoder, MAX_TEXTURE_SIDE},
    paint::{Area, Paint},
    pipeline::{CellPipeline, DrawList, Instance, InstanceKind},
    tab_icon::TabIcon,
    ui_font::UiFont,
};

/// スワップチェーンのバッファの数。
const SWAP_CHAIN_BUFFERS: u32 = 2;

/// 垂直同期を待って表示する。
const PRESENT_SYNC_INTERVAL: u32 = 1;

/// 端末の中の画像のテクスチャに使うバイト数の上限。これを超えると、描いていないものから捨てる。
const TERMINAL_IMAGE_TEXTURE_LIMIT: u64 = 512 * 1024 * 1024;

/// 1 ピクセルのバイト数に、ミップマップの分（全体で約 4/3 倍）を足した見積もり。
const TEXTURE_BYTES_PER_PIXEL_NUMERATOR: u64 = 16;
const TEXTURE_BYTES_PER_PIXEL_DENOMINATOR: u64 = 3;

/// 使う機能レベル（上から順に試す）。シェーダモデル 4.0 を使うので 10.0 を下限にする。
const FEATURE_LEVELS: [D3D_FEATURE_LEVEL; 4] = [
    D3D_FEATURE_LEVEL_11_1,
    D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_10_0,
];

/// 0.0〜1.0 の RGBA。
type Rgba = [f32; 4];

pub struct Renderer {
    /// デバイスを失ったことを知らせる登録。構造体のフィールドは書いた順に捨てるので、先頭に置いて
    /// デバイスやスワップチェーンより先に解除する。登録できない環境では `None`。
    _device_removed_watch: Option<DeviceRemovedWatch>,
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
    /// 端末の中の画像。テクスチャを作れなかったものは `None` で覚え、作り直さない。
    terminal_images: ImageCache<Option<ImageTexture>>,
    /// 開いているタブのアイコンのテクスチャ。
    tab_icons: Vec<TabIconTexture>,
    /// 前のフレームでアトラスに入らず飛ばした数。0 は全部入った。
    skipped_last_frame: usize,
}

/// アトラスに入らないものの扱いと、飛ばした数。
struct Fit {
    /// 入らないものを飛ばして続ける。偽なら `AtlasFull` で止める。
    skip: bool,
    skipped: usize,
}

impl Fit {
    /// 入らないものがあれば `AtlasFull` で止める。
    fn stopping() -> Self {
        Self {
            skip: false,
            skipped: 0,
        }
    }

    /// 入らないものを飛ばして続ける。
    fn skipping() -> Self {
        Self {
            skip: true,
            skipped: 0,
        }
    }

    /// 場所が取れたらそれを返す。入らなかったときは、飛ばす指定なら数えて `None`、そうでなければ
    /// `AtlasFull`。
    fn place(&mut self, slot: Result<Slot, AtlasFull>) -> Result<Option<Slot>, AtlasFull> {
        match slot {
            Ok(slot) => Ok(Some(slot)),
            Err(AtlasFull) if self.skip => {
                self.skipped += 1;
                Ok(None)
            }
            Err(AtlasFull) => Err(AtlasFull),
        }
    }
}

/// 背景画像と端末の中の画像とタブのアイコンのテクスチャ（ミップマップつき）。
struct ImageTexture {
    view: ID3D11ShaderResourceView,
    width: u32,
    height: u32,
}

/// タブのアイコンの一辺 `side` のテクスチャ。作れなかったものは `None` で覚え、作り直さない。
struct TabIconTexture {
    icon: TabIcon,
    side: u32,
    texture: Option<ImageTexture>,
}

/// 端末の背景に敷く画像の描き方。
pub struct Backdrop<'a> {
    /// 画像のファイル。[`Renderer::load_image`] で読んであること（なければ画像を描かない）。
    pub image: &'a Path,
    /// 画像を敷く範囲（端末の画面、余白、スクロールバーの列）。
    pub region: Area,
    pub fit: BackgroundFit,
    /// 画像の不透明度（0〜1。1 で画像をそのまま見せる）。
    pub image_opacity: f32,
}

impl Renderer {
    /// `hwnd` に描く `Renderer` を作る。デバイスを失うと、`on_device_removed` をスレッドプールのスレッドから
    /// 一度だけ呼ぶ（`ID3D11Device4` を使えない環境では呼ばない）。`on_device_removed` は、`Renderer` を
    /// 捨てるときにウィンドウのスレッドがその終わりを待つので、すぐに戻るもの（`PostMessageW` など）にする。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hwnd: HWND,
        width: u32,
        height: u32,
        dwrite: &IDWriteFactory,
        font: &Font,
        ui_font: Rc<UiFont>,
        wide_glyph: WideGlyph,
        on_device_removed: DeviceRemovedNotify,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (device, context) = create_device()?;
        let device_removed_watch = DeviceRemovedWatch::register(&device, on_device_removed);
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
        // DXGI は既定でウィンドウのメッセージを見て Alt+Enter で全画面に切り替えようとする。
        // Alt+Enter は端末へ届けるものなので、DXGI の監視を外す。
        // SAFETY: hwnd は有効なウィンドウで、factory はそのスワップチェーンを作ったもの。
        if let Err(e) = unsafe {
            factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER | DXGI_MWA_NO_WINDOW_CHANGES)
        } {
            tracing::warn!("failed to stop DXGI from handling Alt+Enter: {e}");
        }

        let atlas = GlyphAtlas::new(&device, dwrite, font, ui_font, wide_glyph)?;
        let pipeline = CellPipeline::new(&device)?;
        let mut renderer = Self {
            _device_removed_watch: device_removed_watch,
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
            terminal_images: ImageCache::new(TERMINAL_IMAGE_TEXTURE_LIMIT),
            tab_icons: Vec::new(),
            skipped_last_frame: 0,
        };
        renderer.create_target()?;
        Ok(renderer)
    }

    /// デバイスを失ったか（`GetDeviceRemovedReason`）。失っていれば、その理由をエラーで返す。
    pub fn device_removed(&self) -> windows::core::Result<()> {
        // SAFETY: 引数のない問い合わせ。
        unsafe { self.device.GetDeviceRemovedReason() }
    }

    /// デバイスを失った理由（`GetDeviceRemovedReason`）。失っていなければ `S_OK`。
    pub fn device_removed_reason(&self) -> String {
        match self.device_removed() {
            Ok(()) => "S_OK".to_owned(),
            Err(e) => e.to_string(),
        }
    }

    /// すべての資源を手放す。同じウィンドウに新しい `Renderer` を作る前に使う。
    ///
    /// 一つのウィンドウに flip model のスワップチェーンは一つしか持てず、Direct3D 11 はオブジェクトの解放を
    /// 遅らせるので、スワップチェーンを手放したあとに `ClearState` と `Flush` で解放を済ませる。デバイスを
    /// 失ったことを知らせる登録は、ほかの資源より先に解除する。
    pub fn release(self) {
        let context = self.context.clone();
        drop(self);
        // SAFETY: 引数のない呼び出し。デバイスを失っていても呼べる。
        unsafe {
            context.ClearState();
            context.Flush();
        }
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

    /// 端末 `owner` の `vt` の画面に出す `images` のテクスチャを揃える。`vt` の保管から消えたものや
    /// ピクセルが変わったものは捨てる。[`Renderer::render`] の前に呼ぶ。
    pub fn prepare_terminal_images(&mut self, owner: usize, images: &[ImageView], vt: &Vt) {
        let removed =
            self.terminal_images
                .retain_owner(owner, |key| match vt.image_is_stored(key) {
                    Ok(stored) => stored,
                    Err(e) => {
                        tracing::warn!("failed to look up the terminal image {}: {e}", key.id);
                        false
                    }
                });
        if removed > 0 {
            tracing::debug!("dropped {removed} terminal image textures no longer stored");
        }
        for view in images {
            if self.terminal_images.touch(&view.key) {
                continue;
            }
            match vt.image_pixels(&view.key) {
                Ok(Some(pixels)) => {
                    // `Decoded::new` が警告する。作れなかったものは `None` で覚える。
                    match Decoded::new(pixels.width, pixels.height, pixels.to_premultiplied_bgra())
                    {
                        Some(decoded) => self.add_terminal_image(owner, view.key, &decoded),
                        None => self.terminal_images.insert(owner, view.key, None, 0),
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("failed to read the terminal image {}: {e}", view.key.id);
                }
            }
        }
        let evicted = self.terminal_images.finish_frame();
        if evicted > 0 {
            tracing::debug!("dropped {evicted} terminal image textures over the limit");
        }
    }

    /// 端末 `owner` の画像のテクスチャをすべて捨てる。端末を閉じたときに使う。
    pub fn drop_terminal_images(&mut self, owner: usize) {
        let removed = self.terminal_images.remove_owner(owner);
        tracing::debug!("dropped {removed} terminal image textures of a closed terminal");
    }

    fn add_terminal_image(&mut self, owner: usize, key: ImageKey, decoded: &Decoded) {
        // Direct3D のテクスチャに収まらない画像は作らない。端末エミュレーションの上限とは別に見る。
        let texture = if decoded.width() > MAX_TEXTURE_SIDE || decoded.height() > MAX_TEXTURE_SIDE {
            tracing::warn!(
                "the terminal image {} ({}x{}) is larger than the texture limit {MAX_TEXTURE_SIDE}",
                key.id,
                decoded.width(),
                decoded.height()
            );
            None
        } else {
            self.create_terminal_image_texture(&key, decoded)
        };
        let pixels = u64::from(decoded.width()) * u64::from(decoded.height());
        let bytes = texture.as_ref().map_or(0, |_| {
            pixels * TEXTURE_BYTES_PER_PIXEL_NUMERATOR / TEXTURE_BYTES_PER_PIXEL_DENOMINATOR
        });
        self.terminal_images.insert(owner, key, texture, bytes);
    }

    fn create_terminal_image_texture(
        &self,
        key: &ImageKey,
        decoded: &Decoded,
    ) -> Option<ImageTexture> {
        match self.create_image(decoded) {
            Ok(texture) => {
                tracing::debug!(
                    "created the terminal image texture {} ({}x{})",
                    key.id,
                    decoded.width(),
                    decoded.height()
                );
                Some(texture)
            }
            Err(e) => {
                tracing::warn!(
                    "failed to create the terminal image texture {} ({}x{}): {e}",
                    key.id,
                    decoded.width(),
                    decoded.height()
                );
                None
            }
        }
    }

    fn create_image(&self, decoded: &Decoded) -> windows::core::Result<ImageTexture> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: decoded.width(),
            Height: decoded.height(),
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
        // SAFETY: pixels は stride × 高さのバイトを持ち（`Decoded` が長さを保証する）、texture のレベル 0 と同じ大きさと形式。
        unsafe {
            self.context.UpdateSubresource(
                &texture,
                0,
                None,
                decoded.pixels().as_ptr().cast(),
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
            width: decoded.width(),
            height: decoded.height(),
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
        // 前に `ResizeBuffers` か描画先の作成に失敗したときは描画先がないので、大きさが同じでも作り直す。
        if (width, height) == (self.width, self.height) && self.target.is_some() {
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
    /// 画面の中の画像は [`Renderer::prepare_terminal_images`] で揃えたものを描く。
    ///
    /// 1 フレームの文字がアトラスに入りきらないときは、アトラスを空にして組み直し、それでも入らない
    /// ものだけを飛ばして、入ったものを描く。
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
        let backdrop_list = || {
            let mut list = DrawList::default();
            if let Some((view, placement, backdrop)) = &placed {
                list.push_image(image_instance(placement), view);
                list.push(translucent(
                    area_rect(backdrop.region),
                    first.background,
                    background::overlay_opacity(backdrop.image_opacity),
                ));
            }
            list
        };
        let fill_areas = placed.is_none();
        let mut list = backdrop_list();
        let mut fit = Fit::stopping();
        if let Err(AtlasFull) = self.push_layers(&mut list, layers, paints, fill_areas, &mut fit) {
            // 描きかけのフレームの場所は捨てたアトラスを指すので、空にしてから組み直す。
            // 組み直しても入らない分は飛ばして、入った分だけ描く。
            self.atlas.clear()?;
            list = backdrop_list();
            fit = Fit::skipping();
            let rebuilt = self.push_layers(&mut list, layers, paints, fill_areas, &mut fit);
            debug_assert!(rebuilt.is_ok(), "skipping never stops at AtlasFull");
        }
        self.warn_if_skipped(fit.skipped);
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
                &self.atlas,
                &list,
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

    /// アトラスに入らず飛ばした数が前のフレームと変わったときだけ、警告を残す。入らないフレームが続く間、
    /// 毎フレーム同じ警告を出さないためで、全部入ったフレームで数を 0 に戻す。
    fn warn_if_skipped(&mut self, skipped: usize) {
        if skipped != 0 && skipped != self.skipped_last_frame {
            tracing::warn!("the glyph atlas cannot hold one frame, skipped {skipped} items");
        }
        self.skipped_last_frame = skipped;
    }

    /// 画面ごとに、領域の背景（`fill_areas` のときだけ）と画面の中身を `list` に並べ、続けて `paints` を
    /// 並べる。アトラスに入らないものは、`fit` の指定に従って、止めるか飛ばす。
    fn push_layers(
        &mut self,
        list: &mut DrawList,
        layers: &[(&Frame, Area)],
        paints: &[Paint],
        fill_areas: bool,
        fit: &mut Fit,
    ) -> Result<(), AtlasFull> {
        for &(frame, area) in layers {
            if fill_areas {
                list.push(solid(area_rect(area), frame.background));
            }
            self.push_frame(list, frame, area, fit)?;
        }
        for paint in paints {
            self.push_paint(list, paint, fit)?;
        }
        Ok(())
    }

    /// タブのアイコン `icon` を `area` に描く。一辺が `area` の幅のテクスチャを
    /// [`Renderer::prepare_tab_icons`] で作ってあれば描く。
    fn push_tab_icon(&self, list: &mut DrawList, icon: &TabIcon, area: Area) {
        let Some(texture) = self.tab_icon_texture(icon, area.width) else {
            return;
        };
        let whole = background::Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };
        let placement = Placement {
            rect: background::Rect {
                x: area.x as f32,
                y: area.y as f32,
                width: area.width as f32,
                height: area.height as f32,
            },
            uv: whole,
            repeat: false,
        };
        list.push_image(image_instance(&placement), &texture.view);
    }

    /// タブのアイコン `icon` の一辺 `side` のテクスチャ。作っていないか、作れなかったときは `None`。
    fn tab_icon_texture(&self, icon: &TabIcon, side: u32) -> Option<&ImageTexture> {
        self.tab_icons
            .iter()
            .find(|made| made.icon == *icon && made.side == side)
            .and_then(|made| made.texture.as_ref())
    }

    /// タブのアイコンとそのピクセル `icons` のテクスチャを揃える。一辺はピクセルの幅。`icons` にないものは捨てる。
    /// 作れなかったものは警告して `None` で覚え、作り直さない。[`Renderer::render`] の前に呼ぶ。
    pub fn prepare_tab_icons<'a>(
        &mut self,
        icons: impl Iterator<Item = (&'a TabIcon, &'a Decoded)>,
    ) {
        let icons: Vec<(&TabIcon, &Decoded)> = icons.collect();
        self.tab_icons.retain(|made| {
            icons
                .iter()
                .any(|(icon, pixels)| **icon == made.icon && pixels.width() == made.side)
        });
        for (icon, pixels) in icons {
            let side = pixels.width();
            if self
                .tab_icons
                .iter()
                .any(|made| made.icon == *icon && made.side == side)
            {
                continue;
            }
            let texture = self
                .create_image(pixels)
                .inspect_err(|e| tracing::warn!("failed to create a tab icon: {e}"))
                .ok();
            self.tab_icons.push(TabIconTexture {
                icon: icon.clone(),
                side,
                texture,
            });
        }
    }

    /// `frame` の画像のうち `layer` の層のものを、下から順に `list` に並べる。画面の外へはみ出す分は
    /// 切り抜く。
    fn push_terminal_images(
        &self,
        list: &mut DrawList,
        frame: &Frame,
        area: Area,
        layer: ImageLayer,
    ) {
        let grid = background::Rect {
            x: area.x as f32,
            y: area.y as f32,
            width: area.width.min(u32::from(frame.cols) * self.cell_width) as f32,
            height: area.height.min(u32::from(frame.rows) * self.cell_height) as f32,
        };
        let cell = (self.cell_width, self.cell_height);
        for view in frame.images.iter().filter(|view| view.layer == layer) {
            let Some(Some(texture)) = self.terminal_images.get(&view.key) else {
                continue;
            };
            let image = (texture.width, texture.height);
            if let Some(placement) = view.placement(grid, cell, image) {
                list.push_image(image_instance(&placement), &texture.view);
            }
        }
    }

    /// `paint` を描く四角形を `list` に並べる。
    fn push_paint(
        &mut self,
        list: &mut DrawList,
        paint: &Paint,
        fit: &mut Fit,
    ) -> Result<(), AtlasFull> {
        match paint {
            Paint::Fill { area, color } => list.push(solid(area_rect(*area), *color)),
            Paint::Translucent {
                area,
                color,
                opacity,
            } => list.push(translucent(area_rect(*area), *color, *opacity)),
            Paint::Icon { icon, area, color } => {
                let Some(slot) = fit.place(self.atlas.icon_slot(*icon, area.width, area.height))?
                else {
                    return Ok(());
                };
                list.push(glyph(
                    [
                        area.x as f32,
                        area.y as f32,
                        slot.width as f32,
                        slot.height as f32,
                    ],
                    slot,
                    *color,
                ));
            }
            Paint::Label { x, y, text, color } => {
                let Some(slot) = fit.place(self.atlas.label_slot(text))? else {
                    return Ok(());
                };
                list.push(glyph(
                    [
                        *x as f32 - slot.left as f32,
                        *y as f32,
                        slot.width as f32,
                        slot.height as f32,
                    ],
                    slot,
                    *color,
                ));
            }
            Paint::TabIcon { icon, area } => self.push_tab_icon(list, icon, *area),
        }
        Ok(())
    }

    /// 背景より下の画像、背景、文字より下の画像、カーソル、グリフ、装飾、文字より上の画像の順に
    /// 四角形を並べる。カーソルの上の文字は背景色で描く。
    /// 取り消し線と上線は文字と同じ色、下線は指定された色（なければ文字と同じ色）で描く。
    /// アトラスに入らない文字と装飾は、`fit` の指定に従って、止めるか飛ばす。
    fn push_frame(
        &mut self,
        list: &mut DrawList,
        frame: &Frame,
        area: Area,
        fit: &mut Fit,
    ) -> Result<(), AtlasFull> {
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
                let placed = self
                    .atlas
                    .slot(&cell.text, cell.bold, cell.italic, cell.width);
                if let Some(slot) = fit.place(placed)? {
                    // 左右のセルへはみ出して描くグリフは、場所がセルより広い。
                    let [x, y, _, height] = rect;
                    glyphs.push(glyph(
                        [x - slot.left as f32, y, slot.width as f32, height],
                        slot,
                        color,
                    ));
                }
            }
            if cell.underline != Underline::None {
                let placed = self
                    .atlas
                    .decoration_slot(Decoration::Underline(cell.underline), cell.width);
                if let Some(slot) = fit.place(placed)? {
                    decorations.push(glyph(rect, slot, cell.underline_color.unwrap_or(color)));
                }
            }
            if cell.strikethrough {
                let placed = self
                    .atlas
                    .decoration_slot(Decoration::Strikethrough, cell.width);
                if let Some(slot) = fit.place(placed)? {
                    decorations.push(glyph(rect, slot, color));
                }
            }
            if cell.overline {
                let placed = self.atlas.decoration_slot(Decoration::Overline, cell.width);
                if let Some(slot) = fit.place(placed)? {
                    decorations.push(glyph(rect, slot, color));
                }
            }
        }
        self.push_terminal_images(list, frame, area, ImageLayer::BelowBackground);
        backgrounds.into_iter().for_each(|cell| list.push(cell));
        self.push_terminal_images(list, frame, area, ImageLayer::BelowText);
        if let Some(cursor) = cursor {
            list.push(solid(
                self.cell_rect(area, cursor.col, cursor.row, cursor.width),
                cursor.color,
            ));
        }
        glyphs.into_iter().for_each(|cell| list.push(cell));
        decorations.into_iter().for_each(|cell| list.push(cell));
        self.push_terminal_images(list, frame, area, ImageLayer::AboveText);
        Ok(())
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
            for bgra in row.as_chunks::<4>().0 {
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

/// デバイスを失ったときに [`Renderer`] が呼ぶもの。スレッドプールのスレッドから呼ぶ。
pub type DeviceRemovedNotify = Box<dyn Fn() + Send + Sync + 'static>;

/// デバイスを失ったときにイベントをシグナルさせ（`ID3D11Device4::RegisterDeviceRemovedEvent`）、
/// スレッドプールでイベントを待って、シグナルされたら知らせを一度だけ呼ぶ。捨てるときに、待ちを解除して
/// 呼び出しの終わりを待ち、デバイスへの登録を解除してから、イベントと知らせを手放す。
struct DeviceRemovedWatch {
    device: ID3D11Device4,
    cookie: u32,
    wait: HANDLE,
    /// デバイスがシグナルさせるイベント。`Drop` で登録を解除したあと、フィールドを捨てるときに閉じる。
    _event: OwnedHandle,
    /// 待ちのコールバックに渡したポインタの指す先。`Drop` で待ちを解除したあと、フィールドを捨てるときに
    /// 手放す。
    _notify: Box<DeviceRemovedNotify>,
}

impl DeviceRemovedWatch {
    /// `device` を失ったら `notify` を呼ぶように登録する。`ID3D11Device4` を使えない環境と、登録に失敗した
    /// ときは `None`（描くときに失ったことに気づく）。
    fn register(device: &ID3D11Device, notify: DeviceRemovedNotify) -> Option<Self> {
        let device: ID3D11Device4 = match device.cast() {
            Ok(device) => device,
            Err(e) => {
                tracing::warn!("no ID3D11Device4; noticing a lost device only when drawing: {e}");
                return None;
            }
        };
        // SAFETY: 名前のない、手動でリセットするイベントを作るだけ。
        let event = match unsafe { CreateEventW(None, true, false, PCWSTR::null()) } {
            Ok(event) => event,
            Err(e) => {
                tracing::warn!("failed to create the event for a lost Direct3D device: {e}");
                return None;
            }
        };
        // SAFETY: event はいま作った有効なハンドルで、ほかに持ち主はいない。
        let event = unsafe { OwnedHandle::from_raw_handle(event.0) };
        let notify = Box::new(notify);
        let context: *const DeviceRemovedNotify = &*notify;
        let mut wait = HANDLE::default();
        // SAFETY: wait は書き込める。event は有効なイベント。context の指す先は Box の中にあって動かず、
        // Drop が待ちを解除してコールバックの終わりを待ってから手放す。失敗したときは notify をこの場で
        // 手放すが、登録されていないのでコールバックは呼ばれない。WT_EXECUTEONLYONCE で、シグナルされた
        // ままのイベントでもコールバックは一度だけ呼ばれる。
        if let Err(e) = unsafe {
            RegisterWaitForSingleObject(
                &mut wait,
                HANDLE(event.as_raw_handle()),
                Some(on_device_removed),
                Some(context.cast()),
                INFINITE,
                WT_EXECUTEONLYONCE,
            )
        } {
            tracing::warn!("failed to wait for the event for a lost Direct3D device: {e}");
            return None;
        }
        // SAFETY: event は有効なイベントで、登録を解除するまで閉じない。
        let cookie =
            match unsafe { device.RegisterDeviceRemovedEvent(HANDLE(event.as_raw_handle())) } {
                Ok(cookie) => cookie,
                Err(e) => {
                    tracing::warn!("failed to register the event for a lost Direct3D device: {e}");
                    unregister_wait(wait);
                    return None;
                }
            };
        tracing::debug!("registered the event for a lost Direct3D device");
        Some(Self {
            device,
            cookie,
            wait,
            _event: event,
            _notify: notify,
        })
    }
}

impl Drop for DeviceRemovedWatch {
    fn drop(&mut self) {
        unregister_wait(self.wait);
        // SAFETY: cookie は self.device の RegisterDeviceRemovedEvent が返したもので、まだ解除していない。
        // デバイスを失っていても呼べる。
        unsafe { self.device.UnregisterDeviceRemoved(self.cookie) };
    }
}

/// `RegisterWaitForSingleObject` の待ちを解除し、走っているコールバックの終わりまで待つ。
fn unregister_wait(wait: HANDLE) {
    // SAFETY: wait は RegisterWaitForSingleObject が返したもので、まだ解除していない。コールバック
    // （`on_device_removed`）の中からは呼ばないので、終わりを待っても止まったままにならない。
    if let Err(e) = unsafe { UnregisterWaitEx(wait, Some(INVALID_HANDLE_VALUE)) } {
        tracing::warn!("failed to stop waiting for the event for a lost Direct3D device: {e}");
    }
}

/// デバイスを失ったことを知らせるイベントがシグナルされた。スレッドプールのスレッドで呼ばれる。
unsafe extern "system" fn on_device_removed(context: *mut c_void, _timed_out: bool) {
    // SAFETY: context は DeviceRemovedWatch::register が渡した Box の中の DeviceRemovedNotify を指し、
    // DeviceRemovedWatch の Drop がこの呼び出しの終わりを待ってから手放す。
    let notify = unsafe { &*context.cast::<DeviceRemovedNotify>() };
    notify();
}

/// `error` が、デバイスを失った（ドライバの更新やリセット、ドライバの内部のエラーなど）ことによるもの。
/// グリフアトラスの Direct2D が描画先の作り直しを求めるのも、同じデバイスを失ったときなので含める。
/// `Renderer` を作り直せば描き続けられる。
pub fn is_device_lost(error: &windows::core::Error) -> bool {
    let code = error.code();
    code == DXGI_ERROR_DEVICE_REMOVED
        || code == DXGI_ERROR_DEVICE_RESET
        || code == DXGI_ERROR_DRIVER_INTERNAL_ERROR
        || code == D2DERR_RECREATE_TARGET
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
    let mut level = D3D_FEATURE_LEVEL::default();
    // SAFETY: device・level・context は書き込める。BGRA に対応させるのは、グリフを Direct2D で描くため。
    // 機能レベルの並びは作れるデバイスの下限を決める。満たさないハードウェアは失敗させて WARP に
    // 切り替えさせる。
    unsafe {
        D3D11CreateDevice(
            None,
            driver_type,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&FEATURE_LEVELS),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut level),
            Some(&mut context),
        )?
    };
    let device = device.ok_or_else(windows::core::Error::empty)?;
    let context = context.ok_or_else(windows::core::Error::empty)?;
    tracing::info!(
        "created a Direct3D 11 device ({driver_type:?}) with the feature level {:#x}",
        level.0
    );
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

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::{E_FAIL, E_OUTOFMEMORY};

    use super::*;

    #[test]
    fn only_errors_from_losing_the_device_count_as_losing_it() {
        assert!(is_device_lost(&DXGI_ERROR_DEVICE_REMOVED.into()));
        assert!(is_device_lost(&DXGI_ERROR_DEVICE_RESET.into()));
        assert!(is_device_lost(&DXGI_ERROR_DRIVER_INTERNAL_ERROR.into()));
        assert!(is_device_lost(&D2DERR_RECREATE_TARGET.into()));
        assert!(!is_device_lost(&E_FAIL.into()));
        assert!(!is_device_lost(&E_OUTOFMEMORY.into()));
    }

    #[test]
    fn a_warp_device_registers_for_its_removal_and_unregisters_on_drop() {
        let (device, _context) = create_device_with(D3D_DRIVER_TYPE_WARP).unwrap();
        let watch = DeviceRemovedWatch::register(&device, Box::new(|| {}));
        assert!(watch.is_some());
        drop(watch);
    }
}
