//! 読んだ画像（[`Decoded`]）から GDI のビットマップとアイコンを作る。

use windows::Win32::{
    Foundation::E_INVALIDARG,
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
        DeleteObject, HBITMAP,
    },
    UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO},
};

use crate::image_decoder::Decoded;

/// マスクのビットマップの一行のバイト数の単位（`CreateBitmap` は行を 16 ビット境界に揃える）。
const MASK_ROW_ALIGNMENT: usize = 2;

/// `image` のアイコン。透過は色のアルファで表し、マスクはすべて 0 にする。要らなくなったら呼び出し側が
/// `DestroyIcon` で捨てる。
pub fn icon(image: &Decoded) -> windows::core::Result<HICON> {
    // アイコンの色はアルファを掛けていない BGRA で持つ。
    let straight = Decoded::new(
        image.width(),
        image.height(),
        unpremultiplied(image.pixels()),
    )
    .ok_or_else(|| windows::core::Error::from(E_INVALIDARG))?;
    let color = bitmap(&straight)?;
    let row_bytes = (image.width() as usize)
        .div_ceil(8)
        .next_multiple_of(MASK_ROW_ALIGNMENT);
    let mask_bits = vec![0u8; row_bytes * image.height() as usize];
    // 大きさは bitmap が i32 に収まることを確認ずみ。
    let (width, height) = (image.width() as i32, image.height() as i32);
    // SAFETY: mask_bits は 1 ビットの高さ × 行のバイト数の大きさで、呼び出しの間は生きている。
    let mask = unsafe { CreateBitmap(width, height, 1, 1, Some(mask_bits.as_ptr().cast())) };
    let result = if mask.is_invalid() {
        Err(windows::core::Error::from_thread())
    } else {
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        // SAFETY: info の二つのビットマップは有効。CreateIconIndirect はビットマップを写すので、このあと
        // 消してよい。
        unsafe { CreateIconIndirect(&info) }
    };
    for bitmap in [color, mask] {
        if bitmap.is_invalid() {
            continue;
        }
        // SAFETY: ここで作ったビットマップで、ほかからは使っていない。
        if !unsafe { DeleteObject(bitmap.into()) }.as_bool() {
            tracing::warn!("failed to delete an icon bitmap");
        }
    }
    result
}

/// 乗算済みアルファの BGRA `pixels` を、アルファを掛けていない BGRA にする。透明なピクセルは 0。
fn unpremultiplied(pixels: &[u8]) -> Vec<u8> {
    let mut straight = pixels.to_vec();
    for pixel in straight.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = (u32::from(*channel) * 255 + alpha / 2)
                .checked_div(alpha)
                .map_or(0, |value| value.min(255) as u8);
        }
    }
    straight
}

/// `image`（乗算済みアルファの BGRA）の 32 ビットの DIB。要らなくなったら呼び出し側が `DeleteObject`
/// で消す。
pub fn bitmap(image: &Decoded) -> windows::core::Result<HBITMAP> {
    let width =
        i32::try_from(image.width()).map_err(|_| windows::core::Error::from(E_INVALIDARG))?;
    let height =
        i32::try_from(image.height()).map_err(|_| windows::core::Error::from(E_INVALIDARG))?;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // 負の高さは、上の行から並べることを表す。
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: info は初期化済みで、bits は書き込める。
    let bitmap = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)? };
    let pixels = image.pixels();
    // SAFETY: bits は高さ × 幅 × 4 バイトの、行の間を詰めた領域で、pixels も同じ大きさと並び
    // （`Decoded::new` が長さを保証する）。
    unsafe {
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
    }
    Ok(bitmap)
}

#[cfg(test)]
mod tests {
    use windows::Win32::{
        Graphics::Gdi::{BITMAP, DIBSECTION, GetObjectW},
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
        UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo},
    };

    use super::*;
    use crate::image_decoder::Decoder;

    #[test]
    fn premultiplied_pixels_are_unpremultiplied() {
        assert_eq!(
            unpremultiplied(&[
                0x10, 0x20, 0x30, 0xff, 0x00, 0x00, 0x80, 0x80, 0x12, 0x34, 0x56, 0x00
            ]),
            [
                0x10, 0x20, 0x30, 0xff, 0x00, 0x00, 0xff, 0x80, 0x00, 0x00, 0x00, 0x00
            ]
        );
    }

    #[test]
    fn icons_have_the_size_and_pixels_of_the_image() {
        const SIDE: u32 = 3;
        const OPAQUE: [u8; 4] = [0x10, 0x20, 0xc0, 0xff];
        const HALF: [u8; 4] = [0x00, 0x00, 0x80, 0x80];
        let pixels = [OPAQUE, HALF, [0; 4]].concat().repeat(SIDE as usize);
        let image = Decoded::new(SIDE, SIDE, pixels).unwrap();
        let icon = icon(&image).unwrap();
        let mut info = ICONINFO::default();
        // SAFETY: icon は有効なアイコンで、info は書き込める。GetIconInfo が作ったビットマップは下で消す。
        unsafe { GetIconInfo(icon, &mut info) }.unwrap();
        for bitmap in [info.hbmColor, info.hbmMask] {
            let mut header = BITMAP::default();
            // SAFETY: bitmap は GetIconInfo が作ったもので、header は書き込める。
            let written = unsafe {
                GetObjectW(
                    bitmap.into(),
                    size_of::<BITMAP>() as i32,
                    Some((&raw mut header).cast()),
                )
            };
            assert_eq!(written as usize, size_of::<BITMAP>());
            assert_eq!((header.bmWidth, header.bmHeight), (3, 3));
            // SAFETY: GetIconInfo が作ったビットマップで、ほかからは使っていない。
            assert!(unsafe { DeleteObject(bitmap.into()) }.as_bool());
        }
        // SAFETY: テストのスレッドで初期化する。二度目の初期化は S_FALSE が返るだけ。
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .unwrap();
        let decoded = Decoder::new()
            .unwrap()
            .decode_icon_handle(icon, SIDE)
            .unwrap();
        assert_eq!(decoded.pixels(), image.pixels());
        // SAFETY: icon は作ったばかりで、ほかからは使っていない。
        unsafe { DestroyIcon(icon) }.unwrap();
    }

    #[test]
    fn bitmaps_have_the_size_and_pixels_of_the_image() {
        // 2×1 の、左が青、右が半透明の赤。
        let image = Decoded::new(2, 1, vec![0xff, 0, 0, 0xff, 0, 0, 0x80, 0x80]).unwrap();
        let bitmap = bitmap(&image).unwrap();
        let mut section = DIBSECTION::default();
        // SAFETY: bitmap は有効なビットマップで、section は書き込める。
        let written = unsafe {
            GetObjectW(
                bitmap.into(),
                size_of::<DIBSECTION>() as i32,
                Some((&raw mut section).cast()),
            )
        };
        assert_eq!(written as usize, size_of::<DIBSECTION>());
        assert_eq!((section.dsBm.bmWidth, section.dsBm.bmHeight), (2, 1));
        assert_eq!(section.dsBm.bmBitsPixel, 32);
        // SAFETY: bmBits は 2×4 バイトの有効な領域。
        let bits = unsafe { std::slice::from_raw_parts(section.dsBm.bmBits.cast::<u8>(), 8) };
        assert_eq!(bits, image.pixels());
        // SAFETY: bitmap は作ったばかりで、ほかからは使っていない。
        assert!(unsafe { DeleteObject(bitmap.into()) }.as_bool());
    }
}
