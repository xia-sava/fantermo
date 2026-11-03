use libghostty_vt::kitty::graphics::ImageFormat;

use super::{images::rgba_pixels, links::URI_BUFFER_LEN, *};
use crate::frame::ImageLayer;

mod clipboard;
mod colors;
mod frame;
mod images;
mod keys;
mod links;
mod paste;
mod pointer;
mod screen;
mod scroll;
mod sixel;
mod title;

const SIZE: GridSize = GridSize {
    cols: 20,
    rows: 5,
    cell_width_px: 8,
    cell_height_px: 16,
};

fn new_vt() -> Vt {
    Vt::new(SIZE, "xterm-256color").unwrap()
}

/// 赤い 1×1 の PNG の base64。
const RED_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAA\
                       DUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

/// 3 バイトごとに 4 文字にする base64。テストの入力に使う。
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in data.chunks(3) {
        let mut bytes = [0u8; 3];
        bytes[..chunk.len()].copy_from_slice(chunk);
        let digits = [
            bytes[0] >> 2,
            (bytes[0] & 0x03) << 4 | bytes[1] >> 4,
            (bytes[1] & 0x0f) << 2 | bytes[2] >> 6,
            bytes[2] & 0x3f,
        ];
        for (index, digit) in digits.into_iter().enumerate() {
            encoded.push(if index <= chunk.len() {
                char::from(ALPHABET[usize::from(digit)])
            } else {
                '='
            });
        }
    }
    encoded
}

fn graphics(vt: &mut Vt, control: &str, payload: &str) {
    vt.feed(format!("\x1b_G{control};{payload}\x1b\\").as_bytes());
}
