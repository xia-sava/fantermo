//! 背景画像のファイルを WIC で読む。候補の展開と選び方、表示方法は `fantermo_core::background`。

use std::path::{Path, PathBuf};

use fantermo_core::background::{self, Files};
use windows::{
    Win32::{
        Foundation::GENERIC_READ,
        Graphics::Imaging::{
            CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource,
            IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant,
            WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnDemand,
        },
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
        UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN},
    },
    core::{HSTRING, Interface},
};

use crate::tab_icon;

/// Direct3D 11 のテクスチャの一辺の上限（`D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION`）。
pub const MAX_TEXTURE_SIDE: u32 = 16384;

/// 1 ピクセルのバイト数（乗算済みアルファの BGRA）。
const BYTES_PER_PIXEL: u32 = 4;

/// タブのアイコンの画像で、大きさを見るコマの数の上限。
const MAX_ICON_FRAMES: u32 = 32;

/// 読んだ画像。乗算済みアルファの BGRA で、行の間は詰めてある。
#[derive(Clone, Debug)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Decoded {
    /// 一行のバイト数。
    pub fn stride(&self) -> u32 {
        self.width * BYTES_PER_PIXEL
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
    /// 使い（[`tab_icon::best_frame`]）、縦横比を保って縮め、正方形の真ん中に置く。余りは透明。
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
            let chosen = tab_icon::best_frame(&sizes, side);
            let frame = decoder.GetFrame(chosen as u32)?;
            let converter = factory.CreateFormatConverter()?;
            converter.Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeMedianCut,
            )?;
            let (width, height) = sizes[chosen];
            let (fitted_width, fitted_height, x, y) =
                tab_icon::fit_in_square((width, height), side);
            let scaler = factory.CreateBitmapScaler()?;
            scaler.Initialize(
                &converter,
                fitted_width,
                fitted_height,
                WICBitmapInterpolationModeFant,
            )?;
            let mut decoded = Decoded {
                width: side,
                height: side,
                pixels: Vec::new(),
            };
            decoded.pixels = vec![0; (decoded.stride() * side) as usize];
            let stride = decoded.stride();
            let start = (y * stride + x * BYTES_PER_PIXEL) as usize;
            let length = ((fitted_height - 1) * stride + fitted_width * BYTES_PER_PIXEL) as usize;
            scaler.CopyPixels(
                std::ptr::null(),
                stride,
                &mut decoded.pixels[start..start + length],
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
            let converter = factory.CreateFormatConverter()?;
            converter.Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeMedianCut,
            )?;
            let (mut width, mut height) = (0, 0);
            converter.GetSize(&mut width, &mut height)?;
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
            let mut decoded = Decoded {
                width: stored_width,
                height: stored_height,
                pixels: Vec::new(),
            };
            decoded.pixels = vec![0; (decoded.stride() * stored_height) as usize];
            source.CopyPixels(std::ptr::null(), decoded.stride(), &mut decoded.pixels)?;
            Ok(decoded)
        }
    }
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

/// 実際のファイルシステム。
pub struct DiskFiles;

impl Files for DiskFiles {
    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn files_in(&self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
        Ok(entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect())
    }
}
