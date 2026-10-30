//! 背景画像のファイルを WIC で読む。候補の展開と選び方、表示方法は `fantermo_core::background`。

use std::path::{Path, PathBuf};

use fantermo_core::background::{self, Files};
use windows::{
    Win32::{
        Foundation::{E_FAIL, GENERIC_READ},
        Graphics::Imaging::{
            CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource,
            IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant,
            WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnDemand,
        },
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
        UI::{
            Shell::SHDefExtractIconW,
            WindowsAndMessaging::{
                DestroyIcon, GetSystemMetrics, HICON, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
            },
        },
    },
    core::{HSTRING, Interface},
};

use crate::tab_icon::{self, RealIcon};

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
        let mut icon = HICON::default();
        // SAFETY: 文字列と出力先は呼び出しの間は生きている。
        let result = unsafe {
            SHDefExtractIconW(
                &HSTRING::from(path.as_os_str()),
                0,
                0,
                Some(&mut icon),
                None,
                side,
            )
        };
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

    /// アイコンのハンドル `icon` を、透過を保って一辺 `side` で読む。
    fn decode_icon_handle(&self, icon: HICON, side: u32) -> windows::core::Result<Decoded> {
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
            let (fitted_width, fitted_height, x, y) = tab_icon::fit_in_square(size, side);
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

#[cfg(test)]
mod tests {
    use windows::Win32::{
        Graphics::Gdi::{CreateBitmap, DeleteObject},
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
        UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO},
    };

    use super::*;

    /// 一辺のピクセル数。
    const SIDE: i32 = 2;

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
        assert_eq!(&decoded.pixels[..4], OPAQUE);
        // 半透明の赤は、色にアルファを掛けた値になる。
        assert_eq!(&decoded.pixels[8..12], [0x00, 0x00, 0x80, 0x80]);
    }

    #[test]
    fn icons_with_only_a_mask_keep_their_transparent_part() {
        // アルファのない色で、マスクは上の行が不透明、下の行が透明。
        const RED: [u8; 4] = [0x00, 0x00, 0xff, 0x00];
        const TRANSPARENT: [u8; 4] = [0x00; 4];
        let decoded = decode(icon(RED, TRANSPARENT, [0x00, 0x00, 0xc0, 0x00]));
        assert_eq!(&decoded.pixels[..4], [0x00, 0x00, 0xff, 0xff]);
        assert_eq!(&decoded.pixels[8..12], TRANSPARENT);
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
        assert_eq!((decoded.width, decoded.height), (SIDE, SIDE));
        assert_eq!(decoded.pixels.len(), (SIDE * SIDE * 4) as usize);
        assert!(decoded.pixels.chunks(4).any(|pixel| pixel[3] > 0));
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
}
