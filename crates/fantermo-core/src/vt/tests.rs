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

/// テストの入力に使う base64。
fn base64(data: &[u8]) -> String {
    let mut encoded = Vec::new();
    crate::base64::push(&mut encoded, data);
    String::from_utf8(encoded).unwrap()
}

fn graphics(vt: &mut Vt, control: &str, payload: &str) {
    vt.feed(format!("\x1b_G{control};{payload}\x1b\\").as_bytes());
}
