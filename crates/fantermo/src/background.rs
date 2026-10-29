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

/// Direct3D 11 のテクスチャの一辺の上限（`D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION`）。
pub const MAX_TEXTURE_SIDE: u32 = 16384;

/// 1 ピクセルのバイト数（乗算済みアルファの BGRA）。
const BYTES_PER_PIXEL: u32 = 4;

/// 読んだ画像。乗算済みアルファの BGRA で、行の間は詰めてある。
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
