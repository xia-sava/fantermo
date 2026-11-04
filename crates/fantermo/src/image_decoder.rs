//! 画像のファイルと実行ファイルのアイコンと Windows のストックアイコンを WIC で読む（背景画像、タブのアイコン、
//! 管理者の印の盾）。
//! 背景画像の候補の展開と選び方、表示方法は `fantermo_core::background`。

use std::path::Path;

use fantermo_core::background;
use windows::{
    Win32::{
        Foundation::{E_FAIL, GENERIC_READ},
        Graphics::Direct3D11::D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION,
        Graphics::Imaging::{
            CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource,
            IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant,
            WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnDemand,
        },
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
        UI::{
            Shell::{
                SHDefExtractIconW, SHGSI_ICONLOCATION, SHGetStockIconInfo, SHSTOCKICONID,
                SHSTOCKICONINFO, SIID_SHIELD,
            },
            WindowsAndMessaging::{
                DestroyIcon, GetSystemMetrics, HICON, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
            },
        },
    },
    core::{HSTRING, Interface},
};

use crate::tab_icon::RealIcon;

/// Direct3D 11 のテクスチャの一辺の上限（`D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION`）。
pub const MAX_TEXTURE_SIDE: u32 = D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION;

/// 1 ピクセルのバイト数（乗算済みアルファの BGRA）。
const BYTES_PER_PIXEL: u32 = 4;

/// タブのアイコンの画像で、大きさを見るコマの数の上限。
const MAX_ICON_FRAMES: u32 = 32;

/// 画像の元の大きさ（幅 × 高さ）の上限。展開する前に比べて、超える画像は読まない。
const MAX_SOURCE_PIXELS: u64 = 100_000_000;

/// 元の大きさが幅 `width`、高さ `height` の画像が、[`MAX_SOURCE_PIXELS`] を超えるか。
fn exceeds_source_pixel_limit(width: u32, height: u32) -> bool {
    u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS
}

/// 元の大きさが [`MAX_SOURCE_PIXELS`] を超える画像を読まないときの誤り。呼び出す側が、この誤りを
/// ファイル名つきで warn に残す。
fn source_too_large(width: u32, height: u32) -> windows::core::Error {
    windows::core::Error::new(
        E_FAIL,
        format!("the image is {width}x{height}, over {MAX_SOURCE_PIXELS} pixels"),
    )
}

/// 読んだ画像。乗算済みアルファの BGRA で、行の間は詰めてある。ピクセルのバイト数が
/// 幅 × 高さ × 4 であることは [`Decoded::new`] が保証する。
#[derive(Clone, Debug)]
pub struct Decoded {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Decoded {
    /// 幅 `width`、高さ `height` の画像。`pixels` のバイト数が幅 × 高さ × 4 でなければ、警告して
    /// `None` を返す。
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        let expected = u128::from(width) * u128::from(height) * u128::from(BYTES_PER_PIXEL);
        if pixels.len() as u128 != expected {
            tracing::warn!(
                "the image {width}x{height} needs {expected} bytes of pixels, but has {}",
                pixels.len()
            );
            return None;
        }
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    /// 幅 `width`、高さ `height` の、すべてのバイトが 0（透明）の画像。
    pub fn blank(width: u32, height: u32) -> Self {
        let length = width as usize * height as usize * BYTES_PER_PIXEL as usize;
        Self {
            width,
            height,
            pixels: vec![0; length],
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// 乗算済みアルファの BGRA のバイト列。
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// 書き込める乗算済みアルファの BGRA のバイト列。長さは変えられない。
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    /// 一行のバイト数。
    pub fn stride(&self) -> u32 {
        self.width * BYTES_PER_PIXEL
    }

    /// `badge` を、左上が (`x`, `y`) になる位置に重ねる（乗算済みアルファの over）。この画像からはみ出す分は
    /// 切る。
    pub fn overlay(&mut self, badge: &Decoded, x: u32, y: u32) {
        let columns = badge.width.min(self.width.saturating_sub(x)) as usize;
        let rows = badge.height.min(self.height.saturating_sub(y));
        if columns == 0 || rows == 0 {
            return;
        }
        let bytes = BYTES_PER_PIXEL as usize;
        for row in 0..rows {
            let source_start = (row * badge.stride()) as usize;
            let target_start = ((y + row) * self.stride() + x * BYTES_PER_PIXEL) as usize;
            let source = &badge.pixels[source_start..source_start + columns * bytes];
            let target = &mut self.pixels[target_start..target_start + columns * bytes];
            for (target, source) in target
                .chunks_exact_mut(bytes)
                .zip(source.chunks_exact(bytes))
            {
                let transparency = 255 - u32::from(source[3]);
                for (target, source) in target.iter_mut().zip(source) {
                    let below = (u32::from(*target) * transparency + 127) / 255;
                    *target = (u32::from(*source) + below).min(255) as u8;
                }
            }
        }
    }
}

/// WIC で画像のファイルを読むもの。COM を初期化したスレッドで作って使う。
pub struct Decoder {
    factory: IWICImagingFactory,
}

impl Decoder {
    pub fn new() -> windows::core::Result<Self> {
        // SAFETY: このスレッドは COM を初期化している（main）。
        let factory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
        Ok(Self { factory })
    }

    /// WIC のファクトリ。
    pub fn factory(&self) -> &IWICImagingFactory {
        &self.factory
    }

    /// `path` の画像を、一辺 `side` のタブのアイコンとして読む。コマが複数あれば（ICO）大きさの合うものを
    /// 使い（[`best_frame`]）、縦横比を保って縮め、正方形の真ん中に置く。余りは透明。
    /// 選んだコマの元の大きさが [`MAX_SOURCE_PIXELS`] を超えればエラー。
    pub fn decode_icon(&self, path: &Path, side: u32) -> windows::core::Result<Decoded> {
        let factory = &self.factory;
        // SAFETY: どの呼び出しも、作ったばかりの WIC のオブジェクトに対するもので、文字列と出力先は
        // 呼び出しの間は生きている。
        unsafe {
            let decoder = factory.CreateDecoderFromFilename(
                &HSTRING::from(path.as_os_str()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )?;
            let frames = decoder.GetFrameCount()?.min(MAX_ICON_FRAMES);
            let mut sizes = Vec::new();
            for index in 0..frames {
                let (mut width, mut height) = (0, 0);
                decoder.GetFrame(index)?.GetSize(&mut width, &mut height)?;
                sizes.push((width, height));
            }
            let chosen = best_frame(&sizes, side);
            let (chosen_width, chosen_height) = sizes[chosen];
            if exceeds_source_pixel_limit(chosen_width, chosen_height) {
                return Err(source_too_large(chosen_width, chosen_height));
            }
            let frame = decoder.GetFrame(chosen as u32)?;
            self.fit_in_square(&frame.cast()?, sizes[chosen], side)
        }
    }

    /// プロファイルの本物のアイコン `real` を、一辺 `side` で読む。
    pub fn decode_real_icon(&self, real: &RealIcon, side: u32) -> windows::core::Result<Decoded> {
        match real {
            RealIcon::File(path) => self.decode_icon(path, side),
            RealIcon::Executable(path) => self.decode_executable_icon(path, side),
        }
    }

    /// 実行ファイル `path` の最初のアイコンを、一辺 `side` で読む。アイコンがなければエラー。
    fn decode_executable_icon(&self, path: &Path, side: u32) -> windows::core::Result<Decoded> {
        self.decode_icon_resource(&HSTRING::from(path.as_os_str()), 0, side)
    }

    /// Windows のストックアイコン `id` を、一辺 `side` で読む。
    pub fn decode_stock_icon(
        &self,
        id: SHSTOCKICONID,
        side: u32,
    ) -> windows::core::Result<Decoded> {
        let mut info = SHSTOCKICONINFO {
            cbSize: size_of::<SHSTOCKICONINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: info は cbSize を入れて初期化済みで、呼び出しの間は生きている。場所だけを問い合わせる
        // ので、アイコンのハンドルは作られない。
        unsafe { SHGetStockIconInfo(id, SHGSI_ICONLOCATION, &mut info)? };
        if !info.hIcon.is_invalid() {
            // SAFETY: SHGetStockIconInfo が作ったアイコンで、ほかからは使っていない。
            if let Err(e) = unsafe { DestroyIcon(info.hIcon) } {
                tracing::warn!("failed to destroy a stock icon: {e}");
            }
        }
        let length = info
            .szPath
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(info.szPath.len());
        let path = HSTRING::from_wide(&info.szPath[..length]);
        self.decode_icon_resource(&path, info.iIcon, side)
    }

    /// `image` の右下に、UAC の盾（管理者の印）を重ねる。盾の一辺は [`shield_side`]。
    pub fn overlay_shield(&self, image: &mut Decoded) -> windows::core::Result<()> {
        let side = image.width().min(image.height());
        let shield = self.decode_stock_icon(SIID_SHIELD, shield_side(side))?;
        image.overlay(
            &shield,
            image.width() - shield.width(),
            image.height() - shield.height(),
        );
        Ok(())
    }

    /// ファイル `path` の位置 `index`（負ならリソースの番号）のアイコンを、一辺 `side` で読む。アイコンが
    /// なければエラー。
    fn decode_icon_resource(
        &self,
        path: &HSTRING,
        index: i32,
        side: u32,
    ) -> windows::core::Result<Decoded> {
        let mut icon = HICON::default();
        // SAFETY: 文字列と出力先は呼び出しの間は生きている。
        let result = unsafe { SHDefExtractIconW(path, index, 0, Some(&mut icon), None, side) };
        result.ok()?;
        // アイコンがないときは成功（S_FALSE）で、ハンドルは空のまま。
        if icon.is_invalid() {
            return Err(E_FAIL.into());
        }
        let decoded = self.decode_icon_handle(icon, side);
        // SAFETY: SHDefExtractIconW が作ったアイコンで、ほかからは使っていない。
        if let Err(e) = unsafe { DestroyIcon(icon) } {
            tracing::warn!("failed to destroy an icon: {e}");
        }
        decoded
    }

    /// アイコンのハンドル `icon` を、透過を保って一辺 `side` で読む。`icon` は呼び出す側が持ったまま。
    pub fn decode_icon_handle(&self, icon: HICON, side: u32) -> windows::core::Result<Decoded> {
        // SAFETY: icon は呼び出す側が持っている有効なハンドル。出力先は呼び出しの間は生きている。
        unsafe {
            let bitmap = self.factory.CreateBitmapFromHICON(icon)?;
            let (mut width, mut height) = (0, 0);
            bitmap.GetSize(&mut width, &mut height)?;
            self.fit_in_square(&bitmap.cast()?, (width, height), side)
        }
    }

    /// 大きさ `size` の画像 `source` を縦横比を保って縮め、一辺 `side` の正方形の真ん中に置く。余りは透明。
    fn fit_in_square(
        &self,
        source: &IWICBitmapSource,
        size: (u32, u32),
        side: u32,
    ) -> windows::core::Result<Decoded> {
        let factory = &self.factory;
        // SAFETY: どの呼び出しも、作ったばかりの WIC のオブジェクトに対するもので、出力先は呼び出しの間は
        // 生きている。
        unsafe {
            let converter = factory.CreateFormatConverter()?;
            converter.Initialize(
                source,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeMedianCut,
            )?;
            let (fitted_width, fitted_height, x, y) = fit_in_square(size, side);
            let scaler = factory.CreateBitmapScaler()?;
            scaler.Initialize(
                &converter,
                fitted_width,
                fitted_height,
                WICBitmapInterpolationModeFant,
            )?;
            let mut decoded = Decoded::blank(side, side);
            let stride = decoded.stride();
            let start = (y * stride + x * BYTES_PER_PIXEL) as usize;
            let length = ((fitted_height - 1) * stride + fitted_width * BYTES_PER_PIXEL) as usize;
            scaler.CopyPixels(
                std::ptr::null(),
                stride,
                &mut decoded.pixels_mut()[start..start + length],
            )?;
            Ok(decoded)
        }
    }

    /// `path` の画像の最初のコマを読む。画面（すべてのモニタを合わせた範囲）を縦横比を保って覆える
    /// 大きさより大きければ、そこまで縮める（[`background::stored_size`]）。
    pub fn decode(&self, path: &Path) -> windows::core::Result<Decoded> {
        let factory = &self.factory;
        // SAFETY: どの呼び出しも、作ったばかりの WIC のオブジェクトに対するもので、文字列と出力先は
        // 呼び出しの間は生きている。
        unsafe {
            let decoder = factory.CreateDecoderFromFilename(
                &HSTRING::from(path.as_os_str()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )?;
            let frame = decoder.GetFrame(0)?;
            let (mut width, mut height) = (0, 0);
            frame.GetSize(&mut width, &mut height)?;
            if exceeds_source_pixel_limit(width, height) {
                return Err(source_too_large(width, height));
            }
            let converter = factory.CreateFormatConverter()?;
            converter.Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeMedianCut,
            )?;
            let (stored_width, stored_height) =
                background::stored_size((width, height), screen_size(), MAX_TEXTURE_SIDE);
            let source: IWICBitmapSource = if (stored_width, stored_height) == (width, height) {
                converter.cast()?
            } else {
                let scaler = factory.CreateBitmapScaler()?;
                scaler.Initialize(
                    &converter,
                    stored_width,
                    stored_height,
                    WICBitmapInterpolationModeFant,
                )?;
                scaler.cast()?
            };
            let mut decoded = Decoded::blank(stored_width, stored_height);
            source.CopyPixels(std::ptr::null(), decoded.stride(), decoded.pixels_mut())?;
            Ok(decoded)
        }
    }
}

/// 管理者の印の盾の一辺の、少なくともこれだけ。小さいアイコンでも盾の形が分かるようにする。
const MIN_SHIELD_SIDE: u32 = 10;

/// 一辺 `side` のアイコンに重ねる盾の一辺。アイコンの半分で、少なくとも [`MIN_SHIELD_SIDE`]、多くとも `side`。
fn shield_side(side: u32) -> u32 {
    (side / 2).max(MIN_SHIELD_SIDE).min(side)
}

/// すべてのモニタを合わせた範囲の大きさ。
fn screen_size() -> (u32, u32) {
    // SAFETY: 決まった番号の値を問い合わせるだけ。
    let (width, height) = unsafe {
        (
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    (width.max(0) as u32, height.max(0) as u32)
}

/// `sizes` の大きさのコマのうち、一辺 `side` のアイコンに使うものの位置。`side` を覆える最も小さいコマ、
/// なければ最も大きいコマ。同じ大きさなら前のもの。
fn best_frame(sizes: &[(u32, u32)], side: u32) -> usize {
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
fn fit_in_square(image: (u32, u32), side: u32) -> (u32, u32, u32, u32) {
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
    use std::path::PathBuf;

    use windows::Win32::{
        Graphics::Gdi::{CreateBitmap, DeleteObject},
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
        UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO},
    };

    use super::*;

    /// 一辺のピクセル数。
    const SIDE: i32 = 2;

    #[test]
    fn source_pixel_limit_is_inclusive() {
        assert!(!exceeds_source_pixel_limit(10_000, 10_000));
        assert!(exceeds_source_pixel_limit(10_000, 10_001));
        // 掛け算が u32 に収まらない大きさでも、桁あふれせずに超えると判る。
        assert!(exceeds_source_pixel_limit(u32::MAX, u32::MAX));
        assert!(!exceeds_source_pixel_limit(0, u32::MAX));
    }

    /// COM を初期化したスレッドで作った [`Decoder`]。
    fn decoder() -> Decoder {
        // SAFETY: テストのスレッドで初期化する。二度目の初期化は S_FALSE が返るだけ。
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .expect("COM");
        Decoder::new().expect("WIC")
    }

    /// 上の行が `first`、下の行が `second`（どちらも BGRA）の色で、`mask`（1 ビット、行は 16 ビット単位）の
    /// アイコンを作る。
    fn icon(first: [u8; 4], second: [u8; 4], mask: [u8; 4]) -> HICON {
        let color = [first, first, second, second].concat();
        // SAFETY: ピクセルは呼び出しの間は生きていて、大きさに足りる。作ったビットマップは
        // CreateIconIndirect が写すので、そのあとに消してよい。
        unsafe {
            let color = CreateBitmap(SIDE, SIDE, 1, 32, Some(color.as_ptr().cast()));
            let mask = CreateBitmap(SIDE, SIDE, 1, 1, Some(mask.as_ptr().cast()));
            let info = ICONINFO {
                fIcon: true.into(),
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: mask,
                hbmColor: color,
            };
            let icon = CreateIconIndirect(&info).expect("icon");
            assert!(DeleteObject(color.into()).as_bool());
            assert!(DeleteObject(mask.into()).as_bool());
            icon
        }
    }

    fn decode(icon: HICON) -> Decoded {
        let decoded = decoder()
            .decode_icon_handle(icon, SIDE as u32)
            .expect("decode");
        // SAFETY: icon は作ったばかりで、ほかからは使っていない。
        unsafe { DestroyIcon(icon) }.expect("destroy");
        decoded
    }

    #[test]
    fn icons_with_an_alpha_channel_keep_it_premultiplied() {
        const OPAQUE: [u8; 4] = [0x10, 0x20, 0xc0, 0xff];
        const HALF: [u8; 4] = [0x00, 0x00, 0xff, 0x80];
        let decoded = decode(icon(OPAQUE, HALF, [0; 4]));
        assert_eq!(&decoded.pixels()[..4], OPAQUE);
        // 半透明の赤は、色にアルファを掛けた値になる。
        assert_eq!(&decoded.pixels()[8..12], [0x00, 0x00, 0x80, 0x80]);
    }

    #[test]
    fn icons_with_only_a_mask_keep_their_transparent_part() {
        // アルファのない色で、マスクは上の行が不透明、下の行が透明。
        const RED: [u8; 4] = [0x00, 0x00, 0xff, 0x00];
        const TRANSPARENT: [u8; 4] = [0x00; 4];
        let decoded = decode(icon(RED, TRANSPARENT, [0x00, 0x00, 0xc0, 0x00]));
        assert_eq!(&decoded.pixels()[..4], [0x00, 0x00, 0xff, 0xff]);
        assert_eq!(&decoded.pixels()[8..12], TRANSPARENT);
    }

    #[test]
    fn real_icons_are_read_at_the_asked_size() {
        const SIDE: u32 = 23;
        let cmd = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .expect("SystemRoot")
            .join(r"System32\cmd.exe");
        let decoded = decoder()
            .decode_real_icon(&RealIcon::Executable(cmd), SIDE)
            .expect("the icon of cmd.exe");
        assert_eq!((decoded.width(), decoded.height()), (SIDE, SIDE));
        assert_eq!(decoded.pixels().len(), (SIDE * SIDE * 4) as usize);
        assert!(decoded.pixels().chunks(4).any(|pixel| pixel[3] > 0));
    }

    /// 1 ビットのグレースケールで、すべて 0 の `width` × `height` の PNG を一時フォルダに書き、そのパスを返す。
    fn write_png(name: &str, width: u32, height: u32) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fantermo-decoder-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the folder");
        let path = dir.join(name);
        let file = std::fs::File::create(&path).expect("create the PNG");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::One);
        let mut writer = encoder.write_header().expect("write the PNG header");
        let row_bytes = (width as usize).div_ceil(8);
        writer
            .write_image_data(&vec![0; row_bytes * height as usize])
            .expect("write the PNG pixels");
        writer.finish().expect("finish the PNG");
        path
    }

    #[test]
    fn icon_files_over_the_source_pixel_limit_are_errors() {
        let decoder = decoder();
        let small = write_png("icon-small.png", 4, 4);
        let decoded = decoder.decode_icon(&small, 8).expect("a small icon");
        assert_eq!((decoded.width(), decoded.height()), (8, 8));
        let large = write_png("icon-large.png", 10_001, 10_001);
        let error = decoder.decode_icon(&large, 16).expect_err("a huge icon");
        assert!(error.message().contains("over"), "{error}");
        for path in [small, large] {
            std::fs::remove_file(&path).expect("remove the PNG");
        }
    }

    #[test]
    fn the_shield_is_read_at_the_asked_size() {
        const SIDE: u32 = 16;
        let shield = decoder()
            .decode_stock_icon(SIID_SHIELD, SIDE)
            .expect("the shield");
        assert_eq!((shield.width(), shield.height()), (SIDE, SIDE));
        let alphas: Vec<u8> = shield.pixels().chunks(4).map(|pixel| pixel[3]).collect();
        assert!(alphas.contains(&0xff));
        // 盾の形の外は透明。
        assert!(alphas.contains(&0));
    }

    #[test]
    fn the_shield_sits_in_the_bottom_right_corner() {
        const SIDE: u32 = 18;
        let mut image = Decoded::blank(SIDE, SIDE);
        decoder().overlay_shield(&mut image).expect("the shield");
        let shield = shield_side(SIDE);
        let alpha = |x: u32, y: u32| image.pixels()[((y * SIDE + x) * 4 + 3) as usize];
        let opaque_inside = (0..SIDE)
            .flat_map(|y| (0..SIDE).map(move |x| (x, y)))
            .filter(|&(x, y)| alpha(x, y) > 0)
            .all(|(x, y)| x >= SIDE - shield && y >= SIDE - shield);
        assert!(opaque_inside);
        assert!((SIDE - shield..SIDE).any(|x| alpha(x, SIDE - shield / 2) == 0xff));
    }

    #[test]
    fn shields_are_half_the_icon_but_not_too_small() {
        assert_eq!(shield_side(32), 16);
        assert_eq!(shield_side(36), 18);
        assert_eq!(shield_side(18), MIN_SHIELD_SIDE);
        assert_eq!(shield_side(16), MIN_SHIELD_SIDE);
        assert_eq!(shield_side(4), 4);
    }

    /// 1×1 の画像。
    fn pixel(bgra: [u8; 4]) -> Decoded {
        Decoded::new(1, 1, bgra.to_vec()).unwrap()
    }

    #[test]
    fn overlays_blend_premultiplied_pixels() {
        const BLUE: [u8; 4] = [0xff, 0, 0, 0xff];
        // 不透明なものは下を隠す。
        let mut image = pixel(BLUE);
        image.overlay(&pixel([0, 0, 0xff, 0xff]), 0, 0);
        assert_eq!(image.pixels(), [0, 0, 0xff, 0xff]);
        // 半分の透明の赤（乗算済み）は、下の青を半分残す。
        let mut image = pixel(BLUE);
        image.overlay(&pixel([0, 0, 0x80, 0x80]), 0, 0);
        assert_eq!(image.pixels(), [0x7f, 0, 0x80, 0xff]);
        // 透明なものは何も変えない。
        let mut image = pixel(BLUE);
        image.overlay(&pixel([0; 4]), 0, 0);
        assert_eq!(image.pixels(), BLUE);
        // 透明の上に重ねると、重ねたものになる。
        let mut image = pixel([0; 4]);
        image.overlay(&pixel([0, 0, 0x80, 0x80]), 0, 0);
        assert_eq!(image.pixels(), [0, 0, 0x80, 0x80]);
    }

    #[test]
    fn overlays_are_clipped_to_the_image() {
        const RED: [u8; 4] = [0, 0, 0xff, 0xff];
        let badge = Decoded::new(2, 2, RED.repeat(4)).unwrap();
        let mut image = Decoded::blank(3, 3);
        image.overlay(&badge, 2, 2);
        let painted: Vec<usize> = image
            .pixels()
            .chunks(4)
            .enumerate()
            .filter(|(_, pixel)| pixel[3] > 0)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(painted, [8]);
        // 外に置いたものは何も描かない。
        let mut image = Decoded::blank(3, 3);
        image.overlay(&badge, 3, 0);
        image.overlay(&badge, 0, 5);
        image.overlay(&badge, 100, 100);
        assert!(image.pixels().iter().all(|&byte| byte == 0));
    }

    #[test]
    fn blank_images_are_transparent_and_have_the_right_length() {
        let mut image = Decoded::blank(3, 2);
        assert_eq!((image.width(), image.height(), image.stride()), (3, 2, 12));
        assert_eq!(image.pixels(), [0; 24]);
        image.pixels_mut()[5] = 7;
        assert_eq!(image.pixels().len(), 24);
        assert_eq!(image.pixels()[5], 7);
        assert!(Decoded::blank(0, 4).pixels().is_empty());
    }

    #[test]
    fn decoded_images_need_pixels_of_width_times_height_times_four_bytes() {
        let image = Decoded::new(3, 2, vec![0; 24]).expect("the exact length");
        assert_eq!((image.width(), image.height(), image.stride()), (3, 2, 12));
        assert_eq!(image.pixels().len(), 24);
        assert!(Decoded::new(3, 2, vec![0; 23]).is_none());
        assert!(Decoded::new(3, 2, vec![0; 25]).is_none());
        assert!(Decoded::new(3, 2, Vec::new()).is_none());
        assert!(Decoded::new(0, 5, Vec::new()).is_some());
        // 幅 × 高さ × 4 が u32 に収まらなくても、数え間違えて通さない。
        assert!(Decoded::new(u32::MAX, u32::MAX, vec![0; 16]).is_none());
    }

    #[test]
    fn missing_real_icons_are_errors() {
        const MISSING: &str = r"C:\no-such-folder\no-such.exe";
        let decoder = decoder();
        for real in [
            RealIcon::Executable(PathBuf::from(MISSING)),
            RealIcon::File(PathBuf::from(MISSING)),
        ] {
            assert!(decoder.decode_real_icon(&real, 16).is_err(), "{real:?}");
        }
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
