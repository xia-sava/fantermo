//! 読んだ画像（[`Decoded`]）から GDI のビットマップを作る。

use windows::Win32::{
    Foundation::E_INVALIDARG,
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, HBITMAP,
    },
};

use crate::image_decoder::Decoded;

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
    use windows::Win32::Graphics::Gdi::{DIBSECTION, DeleteObject, GetObjectW};

    use super::*;

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
