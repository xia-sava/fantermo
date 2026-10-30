//! sixel のデコーダ。DCS の中身を少しずつ受け取り、RGBA の画像にする。

use crate::base64;

/// 画像の幅の上限（ピクセル）。
pub const MAX_IMAGE_WIDTH: u32 = 4096;
/// 画像の高さの上限（ピクセル）。
pub const MAX_IMAGE_HEIGHT: u32 = 4096;
/// 色レジスタの数。画像ごとに持つ。
pub const COLOR_REGISTERS: u32 = 1024;
/// 画像一つの DCS の中身のバイト数の上限。
const MAX_DATA_BYTES: u64 = 256 * 1024 * 1024;

/// 一つの sixel 文字が縦に占めるピクセル数。
const SIXEL_HEIGHT: u32 = 6;
/// sixel 文字の最初の文字。この文字からの差がビット列になる。
const SIXEL_FIRST: u8 = b'?';
/// sixel 文字の最後の文字。
const SIXEL_LAST: u8 = b'~';
/// 一つのコマンドの引数の数の上限（`#` の色の定義が 5 個）。
const MAX_PARAMS: usize = 5;
/// `#` の引数で色の表し方を HLS とする値。
const COLOR_SPACE_HLS: u32 = 1;
/// `#` の引数で色の表し方を RGB とする値。
const COLOR_SPACE_RGB: u32 = 2;
/// 色の成分の最大値（RGB の各成分と HLS の明度・彩度の百分率）。
const MAX_PERCENT: u32 = 100;
/// HLS の色相の最大値（度）。
const MAX_HUE: u32 = 360;
/// DEC の HLS の色相（0 度が青）を、普通の色相（0 度が赤）にするために足す角度。
const DEC_HUE_OFFSET: u32 = 240;
/// DCS の P2 で、色を置かなかった画素を透過にする値。
const BACKGROUND_TRANSPARENT: u32 = 1;
/// 背景に使う色レジスタ。
const BACKGROUND_REGISTER: usize = 0;
/// `#` で選ぶまでの色レジスタ（xterm と同じ）。
const INITIAL_REGISTER: usize = 3;
/// RGBA の 1 画素のバイト数。
const BYTES_PER_PIXEL: usize = 4;
/// 不透明のアルファ。
const OPAQUE: u8 = u8::MAX;

/// kitty graphics protocol の転送一つに載せる元のバイト数（base64 で 4096 文字、仕様の上限）。
const KITTY_CHUNK_PIXEL_BYTES: usize = 3072;
/// kitty graphics protocol の APC 一つの大きさの見込み。
const KITTY_APC_CAPACITY: usize = 4096 + 64;

/// 色レジスタの初期値（VT340 の 16 色、百分率）。残りのレジスタは黒。
const VT340_PALETTE: [[u32; 3]; 16] = [
    [0, 0, 0],
    [20, 20, 80],
    [80, 13, 13],
    [20, 80, 20],
    [80, 20, 80],
    [20, 80, 80],
    [80, 80, 20],
    [53, 53, 53],
    [26, 26, 26],
    [33, 33, 60],
    [60, 26, 26],
    [33, 60, 33],
    [60, 33, 60],
    [33, 60, 60],
    [60, 60, 33],
    [80, 80, 80],
];

/// デコードした画像。RGBA で、アルファは乗算しない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SixelImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl SixelImage {
    /// どの画素も不透明。
    pub fn is_opaque(&self) -> bool {
        let (pixels, _) = self.rgba.as_chunks::<BYTES_PER_PIXEL>();
        pixels
            .iter()
            .all(|pixel| pixel[BYTES_PER_PIXEL - 1] == OPAQUE)
    }

    /// この画像をカーソルの位置に置き、カーソルを動かさない kitty graphics protocol の転送を、APC の
    /// 一つずつに分けて順に `write` へ渡す。番号は書かず端末に任せるので、アプリの画像の番号と
    /// ぶつからず、答えも返らない。文字より下（`z=-1`）に置く。
    pub fn write_kitty_transmission(&self, write: &mut dyn FnMut(&[u8])) {
        let mut chunks = self.rgba.chunks(KITTY_CHUNK_PIXEL_BYTES).peekable();
        let mut apc = Vec::with_capacity(KITTY_APC_CAPACITY);
        let mut first = true;
        while let Some(chunk) = chunks.next() {
            apc.clear();
            apc.extend_from_slice(b"\x1b_G");
            if first {
                let control = format!("a=T,f=32,s={},v={},C=1,z=-1,", self.width, self.height);
                apc.extend_from_slice(control.as_bytes());
                first = false;
            }
            apc.extend_from_slice(b"q=2,m=");
            apc.push(if chunks.peek().is_some() { b'1' } else { b'0' });
            apc.push(b';');
            base64::push(&mut apc, chunk);
            apc.extend_from_slice(b"\x1b\\");
            write(&apc);
        }
    }
}

/// 読んでいる途中のコマンド。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    None,
    /// `!`: 次の sixel 文字の繰り返し。
    Repeat,
    /// `#`: 色の定義と選択。
    Color,
    /// `"`: ラスター属性。
    Raster,
}

/// 画像を捨てる理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Overflow {
    Size,
    Bytes,
}

/// sixel のデコーダ。[`SixelDecoder::feed`] で中身を渡し、[`SixelDecoder::finish`] で画像を受け取る。
pub struct SixelDecoder {
    transparent_background: bool,
    /// 色レジスタ（RGB）。
    palette: Vec<[u8; 3]>,
    current_register: usize,
    /// 次に置く位置（ピクセル）。
    x: u32,
    y: u32,
    /// 次の sixel 文字の繰り返しの数。
    repeat: u32,
    command: Command,
    params: [u32; MAX_PARAMS],
    /// 読んでいる引数の番号。
    param_index: usize,
    /// 色を置いたところまでの大きさ。
    drawn_width: u32,
    drawn_height: u32,
    /// ラスター属性で宣言された大きさ。
    declared_width: u32,
    declared_height: u32,
    /// 確保した大きさの画素。行の間隔は `capacity_width`。色を置いていない画素のアルファは 0。
    pixels: Vec<u8>,
    capacity_width: u32,
    capacity_height: u32,
    data_bytes: u64,
    overflow: Option<Overflow>,
    register_overflow_logged: bool,
}

impl SixelDecoder {
    /// `background_mode` は DCS の P2。
    pub fn new(background_mode: u32) -> Self {
        let mut palette = vec![[0; 3]; COLOR_REGISTERS as usize];
        for (register, color) in palette.iter_mut().zip(VT340_PALETTE) {
            *register = color.map(percent_to_byte);
        }
        Self {
            transparent_background: background_mode == BACKGROUND_TRANSPARENT,
            palette,
            current_register: INITIAL_REGISTER,
            x: 0,
            y: 0,
            repeat: 1,
            command: Command::None,
            params: [0; MAX_PARAMS],
            param_index: 0,
            drawn_width: 0,
            drawn_height: 0,
            declared_width: 0,
            declared_height: 0,
            pixels: Vec::new(),
            capacity_width: 0,
            capacity_height: 0,
            data_bytes: 0,
            overflow: None,
            register_overflow_logged: false,
        }
    }

    /// DCS の中身の続きを読む。
    pub fn feed(&mut self, data: &[u8]) {
        self.data_bytes = self.data_bytes.saturating_add(data.len() as u64);
        if self.data_bytes > MAX_DATA_BYTES && self.overflow.is_none() {
            self.discard(Overflow::Bytes);
        }
        for &byte in data {
            if self.overflow.is_some() {
                return;
            }
            self.feed_byte(byte);
        }
    }

    /// 読み終えた画像。何も描いていない、または上限を超えて捨てたなら `None`。
    pub fn finish(mut self) -> Option<SixelImage> {
        self.finish_command();
        if self.overflow.is_some() {
            return None;
        }
        let width = self.drawn_width.max(self.declared_width);
        let height = self.drawn_height.max(self.declared_height);
        if width == 0 || height == 0 {
            return None;
        }
        self.reserve(width, height);
        let rgba = self.into_rows(width, height);
        Some(SixelImage {
            width,
            height,
            rgba,
        })
    }

    /// 画素を `width` × `height` に詰め、背景を透過にしない画像なら色を置いていない画素を埋める。
    fn into_rows(mut self, width: u32, height: u32) -> Vec<u8> {
        let row_bytes = width as usize * BYTES_PER_PIXEL;
        let stride = self.capacity_width as usize * BYTES_PER_PIXEL;
        for row in 1..height as usize {
            self.pixels
                .copy_within(row * stride..row * stride + row_bytes, row * row_bytes);
        }
        self.pixels.truncate(row_bytes * height as usize);
        self.pixels.shrink_to_fit();
        if !self.transparent_background {
            let [r, g, b] = self.palette[BACKGROUND_REGISTER];
            let (pixels, _) = self.pixels.as_chunks_mut::<BYTES_PER_PIXEL>();
            for pixel in pixels {
                if pixel[BYTES_PER_PIXEL - 1] == 0 {
                    *pixel = [r, g, b, OPAQUE];
                }
            }
        }
        self.pixels
    }

    fn feed_byte(&mut self, byte: u8) {
        if self.command != Command::None {
            match byte {
                b'0'..=b'9' => {
                    let param = &mut self.params[self.param_index];
                    *param = param
                        .saturating_mul(10)
                        .saturating_add(u32::from(byte - b'0'));
                    return;
                }
                b';' => {
                    self.param_index = (self.param_index + 1).min(MAX_PARAMS - 1);
                    return;
                }
                _ => self.finish_command(),
            }
        }
        match byte {
            SIXEL_FIRST..=SIXEL_LAST => self.draw(byte - SIXEL_FIRST),
            b'!' => self.begin_command(Command::Repeat),
            b'#' => self.begin_command(Command::Color),
            b'"' => self.begin_command(Command::Raster),
            b'$' => self.x = 0,
            b'-' => {
                self.x = 0;
                self.y = self.y.saturating_add(SIXEL_HEIGHT);
            }
            // 改行など、sixel の中で意味を持たない文字は読み飛ばす。
            _ => {}
        }
    }

    fn begin_command(&mut self, command: Command) {
        self.command = command;
        self.params = [0; MAX_PARAMS];
        self.param_index = 0;
    }

    fn finish_command(&mut self) {
        let params = self.params;
        match self.command {
            Command::None => {}
            Command::Repeat => self.repeat = params[0].max(1),
            Command::Color => self.select_color(params),
            Command::Raster => self.declare_size(params[2], params[3]),
        }
        self.command = Command::None;
    }

    /// `#Pc` で選び、`#Pc;Pu;Px;Py;Pz` で定義して選ぶ。
    fn select_color(&mut self, [register, space, x, y, z]: [u32; MAX_PARAMS]) {
        if register >= COLOR_REGISTERS {
            if !self.register_overflow_logged {
                self.register_overflow_logged = true;
                tracing::warn!("sixel: ignored color register {register} and later ones");
            }
            return;
        }
        let register = register as usize;
        self.current_register = register;
        if self.param_index == 0 {
            return;
        }
        let color = match space {
            COLOR_SPACE_HLS => hls_to_rgb(x, y, z),
            COLOR_SPACE_RGB if [x, y, z].iter().all(|&value| value <= MAX_PERCENT) => {
                Some([x, y, z].map(percent_to_byte))
            }
            _ => None,
        };
        if let Some(color) = color {
            self.palette[register] = color;
        }
    }

    /// `"Pan;Pad;Ph;Pv` の大きさ。画像はこの大きさより小さくならない。縦横比は 1:1 のまま描く。
    fn declare_size(&mut self, width: u32, height: u32) {
        if width > MAX_IMAGE_WIDTH || height > MAX_IMAGE_HEIGHT {
            self.discard(Overflow::Size);
            return;
        }
        self.declared_width = self.declared_width.max(width);
        self.declared_height = self.declared_height.max(height);
        self.reserve(self.declared_width, self.declared_height);
    }

    fn draw(&mut self, bits: u8) {
        let count = std::mem::replace(&mut self.repeat, 1);
        let end_x = self.x.saturating_add(count);
        if bits != 0 {
            let top_bit = 7 - bits.leading_zeros();
            let end_y = self.y.saturating_add(top_bit + 1);
            if end_x > MAX_IMAGE_WIDTH || end_y > MAX_IMAGE_HEIGHT {
                self.discard(Overflow::Size);
                return;
            }
            self.reserve(end_x, end_y);
            self.fill(bits, end_x);
            self.drawn_width = self.drawn_width.max(end_x);
            self.drawn_height = self.drawn_height.max(end_y);
        }
        self.x = end_x;
    }

    /// `bits` の立っている行の、`self.x` から `end_x` の手前までを、選んでいる色にする。
    fn fill(&mut self, bits: u8, end_x: u32) {
        let [r, g, b] = self.palette[self.current_register];
        let color = [r, g, b, OPAQUE];
        let stride = self.capacity_width as usize * BYTES_PER_PIXEL;
        let left = self.x as usize * BYTES_PER_PIXEL;
        let right = end_x as usize * BYTES_PER_PIXEL;
        for bit in 0..SIXEL_HEIGHT {
            if bits & (1 << bit) == 0 {
                continue;
            }
            let row = (self.y + bit) as usize * stride;
            let (pixels, _) =
                self.pixels[row + left..row + right].as_chunks_mut::<BYTES_PER_PIXEL>();
            pixels.fill(color);
        }
    }

    /// 少なくとも `width` × `height` の画素を置けるようにする。
    fn reserve(&mut self, width: u32, height: u32) {
        if width <= self.capacity_width && height <= self.capacity_height {
            return;
        }
        let new_width = grown_capacity(self.capacity_width, width, MAX_IMAGE_WIDTH);
        let new_height = grown_capacity(self.capacity_height, height, MAX_IMAGE_HEIGHT);
        let old_stride = self.capacity_width as usize * BYTES_PER_PIXEL;
        let new_stride = new_width as usize * BYTES_PER_PIXEL;
        self.pixels.resize(new_stride * new_height as usize, 0);
        if new_stride != old_stride {
            // 行の間隔が広がるので、後ろの行から動かす。
            for row in (1..self.capacity_height as usize).rev() {
                self.pixels
                    .copy_within(row * old_stride..(row + 1) * old_stride, row * new_stride);
                self.pixels[row * old_stride..row * new_stride].fill(0);
            }
        }
        self.capacity_width = new_width;
        self.capacity_height = new_height;
    }

    fn discard(&mut self, overflow: Overflow) {
        tracing::warn!("sixel: discarded an image over the limit: {overflow:?}");
        self.overflow = Some(overflow);
        self.pixels = Vec::new();
    }
}

/// 使い切った容量 `current` を、`required` 以上で `limit` までの大きさに倍々で増やす。
fn grown_capacity(current: u32, required: u32, limit: u32) -> u32 {
    if required <= current {
        return current;
    }
    required.max(current.saturating_mul(2)).min(limit)
}

/// 百分率の色の成分を 0〜255 にする。
fn percent_to_byte(percent: u32) -> u8 {
    ((percent * u32::from(u8::MAX) + MAX_PERCENT / 2) / MAX_PERCENT) as u8
}

/// DEC の HLS（色相は青が 0 度・赤が 120 度・緑が 240 度、明度と彩度は百分率）を RGB にする。範囲の
/// 外なら `None`。
fn hls_to_rgb(hue: u32, lightness: u32, saturation: u32) -> Option<[u8; 3]> {
    if hue > MAX_HUE || lightness > MAX_PERCENT || saturation > MAX_PERCENT {
        return None;
    }
    let hue = f64::from((hue + DEC_HUE_OFFSET) % MAX_HUE) / 60.0;
    let lightness = f64::from(lightness) / f64::from(MAX_PERCENT);
    let saturation = f64::from(saturation) / f64::from(MAX_PERCENT);
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let second = chroma * (1.0 - (hue % 2.0 - 1.0).abs());
    let (r, g, b) = match hue as u32 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let offset = lightness - chroma / 2.0;
    Some([r, g, b].map(|value| ((value + offset) * f64::from(u8::MAX)).round() as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const HOLE: [u8; 4] = [0, 0, 0, 0];
    const BLACK: [u8; 4] = [0, 0, 0, 255];

    fn decode(background_mode: u32, data: &[u8]) -> Option<SixelImage> {
        let mut decoder = SixelDecoder::new(background_mode);
        decoder.feed(data);
        decoder.finish()
    }

    fn pixel(image: &SixelImage, x: u32, y: u32) -> [u8; 4] {
        let start = (y * image.width + x) as usize * BYTES_PER_PIXEL;
        image.rgba[start..start + BYTES_PER_PIXEL]
            .try_into()
            .unwrap()
    }

    #[test]
    fn bits_go_from_the_top_with_the_selected_color() {
        // '@' は一番上の行だけ、'A' は上から 2 行目だけ、'~' は 6 行すべて。
        let image = decode(1, b"#1;2;100;0;0#1@A~").unwrap();
        assert_eq!((image.width, image.height), (3, 6));
        assert_eq!(pixel(&image, 0, 0), RED);
        assert_eq!(pixel(&image, 0, 1), HOLE);
        assert_eq!(pixel(&image, 1, 0), HOLE);
        assert_eq!(pixel(&image, 1, 1), RED);
        for y in 0..6 {
            assert_eq!(pixel(&image, 2, y), RED);
        }
    }

    #[test]
    fn unset_pixels_are_filled_with_register_0_unless_the_background_is_transparent() {
        let image = decode(0, b"#1;2;100;0;0#1@").unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        let image = decode(0, b"#1;2;100;0;0#1@A").unwrap();
        assert_eq!(pixel(&image, 1, 0), BLACK);
        let image = decode(2, b"#0;2;0;0;100#1;2;100;0;0#1@A").unwrap();
        assert_eq!(pixel(&image, 1, 0), BLUE);
        let image = decode(1, b"#1;2;100;0;0#1@A").unwrap();
        assert_eq!(pixel(&image, 1, 0), HOLE);
    }

    #[test]
    fn repeat_applies_only_to_the_next_character() {
        let image = decode(1, b"#1;2;100;0;0#1!3@A").unwrap();
        assert_eq!(image.width, 4);
        assert_eq!(pixel(&image, 2, 0), RED);
        assert_eq!(pixel(&image, 3, 0), HOLE);
        assert_eq!(pixel(&image, 3, 1), RED);
        // 0 は 1 回と同じ。
        assert_eq!(decode(1, b"!0@").unwrap().width, 1);
    }

    #[test]
    fn dollar_returns_to_the_left_and_minus_goes_to_the_next_band() {
        let image = decode(1, b"#1;2;100;0;0#2;2;0;0;100#1@@$#2?A-#1@").unwrap();
        assert_eq!((image.width, image.height), (2, 7));
        assert_eq!(pixel(&image, 0, 0), RED);
        assert_eq!(pixel(&image, 1, 0), RED);
        assert_eq!(pixel(&image, 0, 1), HOLE);
        assert_eq!(pixel(&image, 1, 1), BLUE);
        assert_eq!(pixel(&image, 0, 6), RED);
        assert_eq!(pixel(&image, 1, 6), HOLE);
    }

    #[test]
    fn the_size_comes_from_the_set_bits_and_a_trailing_newline_adds_nothing() {
        let image = decode(1, b"@??-").unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(decode(1, b"??-?"), None);
    }

    #[test]
    fn raster_attributes_only_grow_the_image() {
        let image = decode(1, b"\"1;1;4;8@").unwrap();
        assert_eq!((image.width, image.height), (4, 8));
        assert_eq!(pixel(&image, 3, 7), HOLE);
        let image = decode(1, b"\"1;1;1;1!4~").unwrap();
        assert_eq!((image.width, image.height), (4, 6));
        // ラスター属性の大きさは背景で埋まる。
        let image = decode(0, b"\"1;1;2;2").unwrap();
        assert_eq!(pixel(&image, 1, 1), BLACK);
    }

    #[test]
    fn hls_uses_the_dec_hues() {
        assert_eq!(hls_to_rgb(0, 50, 100), Some([0, 0, 255]));
        assert_eq!(hls_to_rgb(120, 50, 100), Some([255, 0, 0]));
        assert_eq!(hls_to_rgb(240, 50, 100), Some([0, 255, 0]));
        assert_eq!(hls_to_rgb(0, 100, 0), Some([255, 255, 255]));
        assert_eq!(hls_to_rgb(361, 50, 100), None);
        let image = decode(1, b"#5;1;120;50;100#5@").unwrap();
        assert_eq!(pixel(&image, 0, 0), RED);
    }

    #[test]
    fn the_initial_color_and_palette_are_those_of_xterm() {
        let image = decode(1, b"@").unwrap();
        assert_eq!(pixel(&image, 0, 0), [51, 204, 51, 255]);
        let image = decode(1, b"#7@").unwrap();
        assert_eq!(pixel(&image, 0, 0), [135, 135, 135, 255]);
    }

    #[test]
    fn color_registers_keep_the_color_at_the_time_of_drawing() {
        let image = decode(1, b"#1;2;100;0;0#1@#1;2;0;0;100@").unwrap();
        assert_eq!(pixel(&image, 0, 0), RED);
        assert_eq!(pixel(&image, 1, 0), BLUE);
    }

    #[test]
    fn registers_and_colors_out_of_range_are_ignored() {
        let image = decode(1, b"#1;2;100;0;0#1#5000;2;0;0;100@").unwrap();
        assert_eq!(pixel(&image, 0, 0), RED);
        let image = decode(1, b"#1;2;100;0;0#1;2;101;0;0@").unwrap();
        assert_eq!(pixel(&image, 0, 0), RED);
        let image = decode(1, b"#1;2;100;0;0#1;9;0;0;0@").unwrap();
        assert_eq!(pixel(&image, 0, 0), RED);
    }

    #[test]
    fn broken_input_is_read_without_panicking() {
        assert_eq!(decode(1, b""), None);
        assert_eq!(decode(1, b"!"), None);
        assert_eq!(decode(1, b"#;;;;;;;;;;"), None);
        assert_eq!(decode(1, b"#99999999999999999999;2"), None);
        assert_eq!(decode(1, b"\"\"\""), None);
        assert_eq!(decode(1, b"\r\n\0\x7f\xff"), None);
        // 引数の後ろの意味のない文字はコマンドを終えるだけで、繰り返しは次の sixel 文字に効く。
        assert_eq!(decode(1, b"!2\n@").unwrap().width, 2);
        assert_eq!(decode(1, b"-".repeat(10_000).as_slice()), None);
    }

    #[test]
    fn images_over_the_limit_are_discarded() {
        let wide = format!("!{}@", MAX_IMAGE_WIDTH + 1);
        assert_eq!(decode(1, wide.as_bytes()), None);
        let tall = "-".repeat(MAX_IMAGE_HEIGHT as usize / 6 + 1) + "@";
        assert_eq!(decode(1, tall.as_bytes()), None);
        let declared = format!("\"1;1;{};1", MAX_IMAGE_WIDTH + 1);
        assert_eq!(decode(1, declared.as_bytes()), None);
        // 上限ちょうどは通る。
        let limit = format!("!{}@", MAX_IMAGE_WIDTH);
        assert_eq!(decode(1, limit.as_bytes()).unwrap().width, MAX_IMAGE_WIDTH);
    }

    #[test]
    fn the_byte_limit_discards_the_image() {
        let mut decoder = SixelDecoder::new(1);
        decoder.feed(b"@");
        decoder.data_bytes = MAX_DATA_BYTES;
        decoder.feed(b"@");
        assert_eq!(decoder.finish(), None);
    }

    #[test]
    fn growing_keeps_the_pixels_in_place() {
        // 幅と高さを何度か広げても、先に置いた画素が動かない。
        let mut data = b"#1;2;100;0;0#1".to_vec();
        for _ in 0..20 {
            data.extend_from_slice(b"~-");
        }
        data.extend_from_slice(b"$!100@");
        let image = decode(1, &data).unwrap();
        assert_eq!((image.width, image.height), (100, 121));
        for y in 0..120 {
            assert_eq!(pixel(&image, 0, y), RED, "{y}");
            assert_eq!(pixel(&image, 1, y), HOLE, "{y}");
        }
        assert_eq!(pixel(&image, 99, 120), RED);
    }

    #[test]
    fn a_kitty_transmission_is_split_and_only_the_first_part_has_the_size() {
        let image = SixelImage {
            width: 1000,
            height: 1,
            rgba: vec![7; 4000],
        };
        let mut apcs = Vec::new();
        image.write_kitty_transmission(&mut |apc| {
            apcs.push(String::from_utf8(apc.to_vec()).unwrap());
        });
        assert_eq!(apcs.len(), 2);
        assert!(
            apcs[0].starts_with("\x1b_Ga=T,f=32,s=1000,v=1,C=1,z=-1,q=2,m=1;"),
            "{}",
            &apcs[0][..50]
        );
        assert!(apcs[1].starts_with("\x1b_Gq=2,m=0;"));
        assert!(apcs.iter().all(|apc| apc.ends_with("\x1b\\")));
    }

    #[test]
    fn opaque_images_are_told_apart() {
        assert!(decode(0, b"@A").unwrap().is_opaque());
        assert!(!decode(1, b"@A").unwrap().is_opaque());
    }

    #[test]
    fn data_can_arrive_in_pieces() {
        let data = b"\"1;1;3;6#12;2;100;0;0#12!3~";
        let whole = decode(1, data);
        let mut decoder = SixelDecoder::new(1);
        for byte in data {
            decoder.feed(std::slice::from_ref(byte));
        }
        assert_eq!(decoder.finish(), whole);
    }
}
