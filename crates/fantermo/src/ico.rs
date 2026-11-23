//! 読んだ画像（[`Decoded`]）の並びから、ICO のファイルの中身を作る。
//!
//! 大きさごとの画像は、32 ビットの DIB（`BITMAPINFOHEADER` のあとに BGRA の色と AND マスク）で入れる。PNG の
//! エントリは使わない。DIB の高さは色と AND マスクを合わせた 2 倍の値で、行は下から上への順。色はアルファを
//! 乗算していない値に戻して入れ、AND マスクはすべて 0（アルファで透過を決める）にする。

use crate::image_decoder::Decoded;

/// ICO の先頭（`ICONDIR`）のバイト数。
const DIRECTORY_SIZE: usize = 6;

/// 画像ごとのエントリ（`ICONDIRENTRY`）のバイト数。
const ENTRY_SIZE: usize = 16;

/// `ICONDIR` の種類の値のうち、アイコン。
const ICON_TYPE: u16 = 1;

/// `BITMAPINFOHEADER` のバイト数。
const BITMAP_INFO_HEADER_SIZE: u32 = 40;

/// 1 ピクセルのビット数（BGRA）。
const BITS_PER_PIXEL: u16 = 32;

/// 1 ピクセルのバイト数（BGRA）。
const BYTES_PER_PIXEL: usize = 4;

/// 色の面の数。
const PLANES: u16 = 1;

/// 圧縮しない DIB（`BI_RGB`）。
const BI_RGB: u32 = 0;

/// ICO のエントリに書ける一辺の上限。エントリの一辺は 1 バイトで、256 は 0 と書く。
const MAX_SIDE: u32 = 256;

/// AND マスクの一行を揃える境界のバイト数。
const MASK_ROW_ALIGNMENT: usize = 4;

/// 1 バイトのビット数。
const BITS_PER_BYTE: usize = 8;

/// `images` の画像を入れた ICO のバイト列。画像の幅か高さが 0 か [`MAX_SIDE`] を超えるとき、画像が
/// `u16::MAX` 個を超えるときは `None`。
pub fn encode(images: &[Decoded]) -> Option<Vec<u8>> {
    let count = u16::try_from(images.len()).ok()?;
    if images.iter().any(|image| {
        !(1..=MAX_SIDE).contains(&image.width()) || !(1..=MAX_SIDE).contains(&image.height())
    }) {
        return None;
    }
    let bodies: Vec<Vec<u8>> = images.iter().map(dib).collect();
    let mut bytes = Vec::with_capacity(
        DIRECTORY_SIZE + ENTRY_SIZE * images.len() + bodies.iter().map(Vec::len).sum::<usize>(),
    );
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&ICON_TYPE.to_le_bytes());
    bytes.extend_from_slice(&count.to_le_bytes());
    let mut offset = DIRECTORY_SIZE + ENTRY_SIZE * images.len();
    for (image, body) in images.iter().zip(&bodies) {
        bytes.push(entry_side(image.width()));
        bytes.push(entry_side(image.height()));
        // 色の数（パレットを使わないので 0）と予約の値。
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&PLANES.to_le_bytes());
        bytes.extend_from_slice(&BITS_PER_PIXEL.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(body.len()).ok()?.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(offset).ok()?.to_le_bytes());
        offset += body.len();
    }
    for body in bodies {
        bytes.extend_from_slice(&body);
    }
    Some(bytes)
}

/// エントリに書く一辺。[`MAX_SIDE`] は 0 と書く。
fn entry_side(side: u32) -> u8 {
    if side == MAX_SIDE { 0 } else { side as u8 }
}

/// 幅 `width` の AND マスクの一行のバイト数。
fn mask_row_bytes(width: u32) -> usize {
    (width as usize)
        .div_ceil(BITS_PER_BYTE)
        .next_multiple_of(MASK_ROW_ALIGNMENT)
}

/// `image` の DIB（`BITMAPINFOHEADER`、色、AND マスク）。
fn dib(image: &Decoded) -> Vec<u8> {
    let (width, height) = (image.width(), image.height());
    let row_bytes = width as usize * BYTES_PER_PIXEL;
    let color_bytes = row_bytes * height as usize;
    let mask_bytes = mask_row_bytes(width) * height as usize;
    let mut bytes = Vec::with_capacity(BITMAP_INFO_HEADER_SIZE as usize + color_bytes + mask_bytes);
    bytes.extend_from_slice(&BITMAP_INFO_HEADER_SIZE.to_le_bytes());
    bytes.extend_from_slice(&(width as i32).to_le_bytes());
    // 色と AND マスクを合わせた高さ。
    bytes.extend_from_slice(&(2 * height as i32).to_le_bytes());
    bytes.extend_from_slice(&PLANES.to_le_bytes());
    bytes.extend_from_slice(&BITS_PER_PIXEL.to_le_bytes());
    bytes.extend_from_slice(&BI_RGB.to_le_bytes());
    bytes.extend_from_slice(&((color_bytes + mask_bytes) as u32).to_le_bytes());
    // 解像度（横と縦）、パレットの色の数、重要な色の数は使わない。
    bytes.extend_from_slice(&[0; 16]);
    for row in image.pixels().chunks_exact(row_bytes).rev() {
        for pixel in row.as_chunks::<BYTES_PER_PIXEL>().0 {
            bytes.extend_from_slice(&unpremultiplied(pixel));
        }
    }
    bytes.resize(bytes.len() + mask_bytes, 0);
    bytes
}

/// 乗算済みアルファの BGRA の 1 ピクセル `pixel` の、アルファを乗算していない値。透明なら 0。
fn unpremultiplied(pixel: &[u8]) -> [u8; 4] {
    let alpha = u32::from(pixel[3]);
    if alpha == 0 {
        return [0; 4];
    }
    let channel = |value: u8| ((u32::from(value) * 255 + alpha / 2) / alpha).min(255) as u8;
    [
        channel(pixel[0]),
        channel(pixel[1]),
        channel(pixel[2]),
        pixel[3],
    ]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

    use super::*;
    use crate::image_decoder::Decoder;

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([bytes[at], bytes[at + 1]])
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    /// 一辺 `side` の、位置ごとに色とアルファの違う乗算済みアルファの画像。透明、半透明、不透明を含む。
    fn sample(side: u32) -> Decoded {
        let mut pixels = Vec::new();
        for y in 0..side {
            for x in 0..side {
                let alpha = ((x * 255) / (side - 1)) as u8;
                let straight = [(y * 7) as u8, (x * 11) as u8, ((x + y) * 5) as u8];
                let premultiplied =
                    straight.map(|value| ((u32::from(value) * u32::from(alpha) + 127) / 255) as u8);
                pixels.extend_from_slice(&premultiplied);
                pixels.push(alpha);
            }
        }
        Decoded::new(side, side, pixels).unwrap()
    }

    #[test]
    fn the_header_and_the_entries_describe_each_image() {
        let images = [sample(16), sample(20), sample(40), Decoded::blank(256, 256)];
        let bytes = encode(&images).unwrap();
        assert_eq!(u16_at(&bytes, 0), 0);
        assert_eq!(u16_at(&bytes, 2), ICON_TYPE);
        assert_eq!(u16_at(&bytes, 4), 4);
        // 一辺、色の数、予約、面、ビット数、バイト数、オフセット。
        let mut expected_offset = (DIRECTORY_SIZE + ENTRY_SIZE * images.len()) as u32;
        for (index, (side, mask_row)) in [(16, 4), (20, 4), (40, 8), (256, 32)]
            .into_iter()
            .enumerate()
        {
            let entry = &bytes[DIRECTORY_SIZE + ENTRY_SIZE * index..][..ENTRY_SIZE];
            let written_side = if side == 256 { 0 } else { side as u8 };
            assert_eq!(&entry[..4], [written_side, written_side, 0, 0], "{side}");
            assert_eq!(u16_at(entry, 4), PLANES);
            assert_eq!(u16_at(entry, 6), BITS_PER_PIXEL);
            let size = BITMAP_INFO_HEADER_SIZE + side * side * 4 + mask_row * side;
            assert_eq!(u32_at(entry, 8), size, "{side}");
            assert_eq!(u32_at(entry, 12), expected_offset, "{side}");
            expected_offset += size;
        }
        assert_eq!(bytes.len(), expected_offset as usize);
    }

    #[test]
    fn each_image_is_a_bottom_up_dib_with_an_empty_mask() {
        const SIDE: u32 = 20;
        let image = sample(SIDE);
        let bytes = encode(std::slice::from_ref(&image)).unwrap();
        let body = &bytes[u32_at(&bytes, DIRECTORY_SIZE + 12) as usize..];
        assert_eq!(u32_at(body, 0), BITMAP_INFO_HEADER_SIZE);
        assert_eq!(u32_at(body, 4), SIDE);
        assert_eq!(u32_at(body, 8), 2 * SIDE);
        assert_eq!(u16_at(body, 12), PLANES);
        assert_eq!(u16_at(body, 14), BITS_PER_PIXEL);
        assert_eq!(u32_at(body, 16), BI_RGB);
        let colors = &body[BITMAP_INFO_HEADER_SIZE as usize..];
        let row = SIDE as usize * 4;
        // DIB の最初の行は画像の最後の行。色はアルファを乗算していない値。
        let last_row = &image.pixels()[row * (SIDE as usize - 1)..][..row];
        for (written, pixel) in colors[..row].chunks(4).zip(last_row.chunks(4)) {
            assert_eq!(written, unpremultiplied(pixel));
        }
        // AND マスクは 4 バイトの行がすべて 0。
        let mask = &colors[row * SIDE as usize..];
        assert_eq!(mask.len(), 4 * SIDE as usize);
        assert!(mask.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn premultiplied_colors_are_turned_back() {
        assert_eq!(
            unpremultiplied(&[0x10, 0x20, 0x30, 0xff]),
            [0x10, 0x20, 0x30, 0xff]
        );
        assert_eq!(
            unpremultiplied(&[0x00, 0x40, 0x80, 0x80]),
            [0x00, 0x80, 0xff, 0x80]
        );
        // 透明なピクセルは色を持たない。
        assert_eq!(unpremultiplied(&[0x05, 0x06, 0x07, 0x00]), [0; 4]);
        // アルファより大きい色（乗算済みとして正しくない値）も 255 で止める。
        assert_eq!(
            unpremultiplied(&[0xff, 0x00, 0x00, 0x10]),
            [0xff, 0x00, 0x00, 0x10]
        );
    }

    #[test]
    fn images_that_do_not_fit_in_an_entry_are_refused() {
        assert!(encode(&[Decoded::blank(257, 257)]).is_none());
        assert!(encode(&[Decoded::blank(0, 0)]).is_none());
        assert!(encode(&[sample(16), Decoded::blank(16, 300)]).is_none());
        // 画像がなければ、エントリのない ICO。
        assert_eq!(encode(&[]).unwrap(), [0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn wic_reads_back_every_size() {
        const SIDES: [u32; 6] = [16, 20, 24, 32, 40, 48];
        /// 乗算済みのアルファとの往復で許す差。
        const TOLERANCE: u8 = 2;
        let images: Vec<Decoded> = SIDES.iter().map(|&side| sample(side)).collect();
        let bytes = encode(&images).unwrap();
        let dir = std::env::temp_dir().join(format!("fantermo-ico-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path: PathBuf = dir.join("sample.ico");
        std::fs::write(&path, &bytes).unwrap();
        // SAFETY: テストのスレッドで初期化する。二度目の初期化は S_FALSE が返るだけ。
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .unwrap();
        let decoder = Decoder::new().unwrap();
        let read: Vec<Decoded> = SIDES
            .iter()
            .map(|&side| decoder.decode_icon(&path, side).unwrap())
            .collect();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        for (image, read) in images.iter().zip(&read) {
            assert_eq!(
                (read.width(), read.height()),
                (image.width(), image.height())
            );
            for (index, (read, original)) in read.pixels().iter().zip(image.pixels()).enumerate() {
                assert!(
                    read.abs_diff(*original) <= TOLERANCE,
                    "{} px, byte {index}: {read} and {original}",
                    image.width()
                );
            }
        }
    }
}
