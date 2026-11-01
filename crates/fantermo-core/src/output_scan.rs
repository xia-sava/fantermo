//! 子プロセスの出力を端末エミュレーションへ流す前に見て、libghostty-vt が扱わないもの（sixel の DCS と
//! 関係するモード）と、あとで答えるもの（OSC 52 の読み取りの依頼）を取り出す。それ以外のバイトは順番を
//! 変えずに渡す。チャンクの切れ目はどこにあってもよい。
//!
//! OSC 52 の読み取りの依頼を libghostty-vt に渡さないのは、そのコールバックの中で答えなければ空の答えを
//! 書き込むので、ダイアログで確認してから答えることも、拒否して何も返さないこともできないため。

use crate::{
    clipboard::{ReadRequest, Terminator},
    sixel::{COLOR_REGISTERS, MAX_IMAGE_HEIGHT, MAX_IMAGE_WIDTH, SixelDecoder, SixelImage},
};

const ESC: u8 = 0x1b;
/// OSC を終える BEL。
const BEL: u8 = 0x07;
/// OSC の始まりの `ESC ]` のバイト数。
const OSC_INTRODUCER_LEN: usize = 2;
/// `ESC ]` に続く OSC 52 の始まり。
const OSC_52_PREFIX: &[u8] = b"52;";
/// 8 ビットの DCS の始まり。
const C1_DCS: u8 = 0x90;
/// 8 ビットの ST。
const C1_ST: u8 = 0x9c;
/// DCS を取り消す。
const CAN: u8 = 0x18;
/// DCS を取り消す。
const SUB: u8 = 0x1a;
/// 見分けるために手元に置くバイト数の上限（`ESC [ ?` や `ESC P` とそのパラメータ）。超えたものは見分けずに流す。
const MAX_HELD_BYTES: usize = 64;
/// sixel を置いてもカーソルを動かさず、スクロールもしないモード（DECSDM）の番号。
const MODE_SIXEL_DISPLAY: &str = "80";
/// 色を置かなかった画素の扱いを決める、DCS のパラメータの位置。
const BACKGROUND_PARAM_INDEX: usize = 1;
/// DECRQM の答えの、モードが有効。
const DECRQM_SET: u32 = 1;
/// DECRQM の答えの、モードが無効。
const DECRQM_RESET: u32 = 2;
/// XTSMGRAPHICS の項目: 色レジスタの数。
const GRAPHICS_ITEM_COLORS: u32 = 1;
/// XTSMGRAPHICS の項目: sixel の大きさ。
const GRAPHICS_ITEM_SIXEL_SIZE: u32 = 2;
/// XTSMGRAPHICS の操作: 読む。
const GRAPHICS_READ: u32 = 1;
/// XTSMGRAPHICS の操作: 既定に戻す。
const GRAPHICS_RESET: u32 = 2;
/// XTSMGRAPHICS の操作: 変える。
const GRAPHICS_SET: u32 = 3;
/// XTSMGRAPHICS の操作: 上限を読む。
const GRAPHICS_READ_MAX: u32 = 4;
/// XTSMGRAPHICS の答え: 成功。
const GRAPHICS_SUCCESS: u32 = 0;
/// XTSMGRAPHICS の答え: 項目が正しくない。
const GRAPHICS_ERROR_ITEM: u32 = 1;
/// XTSMGRAPHICS の答え: 操作が正しくない。
const GRAPHICS_ERROR_ACTION: u32 = 2;
/// XTSMGRAPHICS の答え: 失敗。
const GRAPHICS_FAILURE: u32 = 3;

/// 出力を見た結果。来た順に渡す。
#[derive(Debug, PartialEq, Eq)]
pub enum OutputEvent<'a> {
    /// 端末エミュレーションへそのまま流すバイト列。
    Bytes(&'a [u8]),
    /// 子プロセスへ返す応答。
    Reply(Vec<u8>),
    /// 読み終えた sixel の画像。
    Sixel {
        image: SixelImage,
        /// `true` ならカーソルの位置に置いてカーソルを進める。`false`（DECSDM）なら画面の左上に置いて
        /// カーソルを動かさない。
        scrolling: bool,
    },
    /// アプリからのクリップボードの読み取りの依頼。
    ClipboardRead(ReadRequest),
}

/// どこまで読んだか。
enum State {
    Ground,
    /// `ESC` を読んだ。
    Escape,
    /// `ESC P` か 8 ビットの DCS と、そのパラメータを読んでいる。
    DcsParams,
    /// `ESC [` を読んだ。
    Csi,
    /// `ESC [ ?` とそのパラメータを読んでいる。
    CsiPrivate,
    /// `ESC [ ?` とそのパラメータの後ろで `$` を読んだ（`$` は手元に置かない）。
    CsiPrivateDollar,
    /// `ESC ]` と、そのあとの `52;` の途中までを読んでいる。
    OscPrefix,
    /// `ESC ] 52 ;` を読んだ。`Pc` が来るところ。
    Osc52Selector,
    /// `ESC ] 52 ; Pc` を読んだ。`;` が来るところ。
    Osc52SelectorEnd {
        selector: Option<u8>,
    },
    /// `ESC ] 52 ; Pc ;` を読んだ。`?` が来るところ。
    Osc52Payload {
        selector: Option<u8>,
    },
    /// `ESC ] 52 ; Pc ; ?` を読んだ。終端が来るところ。
    Osc52Terminator {
        selector: Option<u8>,
    },
    /// `ESC ] 52 ; Pc ; ?` の後ろで `ESC` を読んだ。`\` が来るところ。
    Osc52TerminatorEscape {
        selector: Option<u8>,
    },
    /// sixel の中身を読んでいる。
    Sixel(Box<SixelDecoder>),
    /// sixel の中身の後ろで `ESC` を読んだ。
    SixelEscape(Box<SixelDecoder>),
}

/// 出力を見る状態。
pub struct OutputScanner {
    state: State,
    /// まだ流すと決めていない、シーケンスの始まりのバイト。
    held: Vec<u8>,
    /// 読んでいる UTF-8 の文字の、残りの継続バイトの数。この間の 0x90 は C1 ではない。
    utf8_continuation: u8,
    /// DECSDM が有効。
    sixel_display_mode: bool,
}

impl Default for OutputScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputScanner {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            held: Vec::with_capacity(MAX_HELD_BYTES),
            utf8_continuation: 0,
            sixel_display_mode: false,
        }
    }

    /// `data` を読み、結果を順に `sink` へ渡す。
    pub fn scan(&mut self, data: &[u8], sink: &mut dyn FnMut(OutputEvent)) {
        let mut index = 0;
        // Ground で読み進めた、まだ渡していないところの始まり。
        let mut span_start = 0;
        while index < data.len() {
            let byte = data[index];
            match &mut self.state {
                State::Ground => {
                    if self.starts_sequence(byte) {
                        if span_start < index {
                            sink(OutputEvent::Bytes(&data[span_start..index]));
                        }
                        self.held.push(byte);
                        self.state = if byte == ESC {
                            State::Escape
                        } else {
                            State::DcsParams
                        };
                        span_start = index + 1;
                    }
                    index += 1;
                }
                State::Escape => {
                    match byte {
                        b'P' => {
                            self.held.push(byte);
                            self.state = State::DcsParams;
                            index += 1;
                        }
                        b'[' => {
                            self.held.push(byte);
                            self.state = State::Csi;
                            index += 1;
                        }
                        b']' => {
                            self.held.push(byte);
                            self.state = State::OscPrefix;
                            index += 1;
                        }
                        _ => {
                            // RIS は DECSDM も戻す。
                            if byte == b'c' {
                                self.sixel_display_mode = false;
                            }
                            self.release_held(sink);
                        }
                    }
                    span_start = index;
                }
                State::DcsParams => {
                    match byte {
                        b'0'..=b'9' | b';' if self.held.len() < MAX_HELD_BYTES => {
                            self.held.push(byte);
                            index += 1;
                        }
                        b'q' => {
                            let decoder = SixelDecoder::new(background_mode(&self.held));
                            self.held.clear();
                            self.state = State::Sixel(Box::new(decoder));
                            index += 1;
                        }
                        // sixel でない DCS（DECRQSS、XTGETTCAP など）。
                        _ => self.release_held(sink),
                    }
                    span_start = index;
                }
                State::Csi => {
                    if byte == b'?' {
                        self.held.push(byte);
                        self.state = State::CsiPrivate;
                        index += 1;
                    } else {
                        self.release_held(sink);
                    }
                    span_start = index;
                }
                State::CsiPrivate => {
                    match byte {
                        b'0'..=b'9' | b';' if self.held.len() < MAX_HELD_BYTES => {
                            self.held.push(byte);
                            index += 1;
                        }
                        b'h' | b'l' => {
                            if private_params(&self.held)
                                .split(';')
                                .any(|mode| mode == MODE_SIXEL_DISPLAY)
                            {
                                self.sixel_display_mode = byte == b'h';
                            }
                            self.release_held(sink);
                        }
                        b'S' => {
                            sink(OutputEvent::Reply(graphics_attribute_reply(
                                private_params(&self.held),
                            )));
                            self.held.clear();
                            self.state = State::Ground;
                            index += 1;
                        }
                        b'$' => {
                            self.state = State::CsiPrivateDollar;
                            index += 1;
                        }
                        _ => self.release_held(sink),
                    }
                    span_start = index;
                }
                State::CsiPrivateDollar => {
                    if byte == b'p' && private_params(&self.held) == MODE_SIXEL_DISPLAY {
                        let value = if self.sixel_display_mode {
                            DECRQM_SET
                        } else {
                            DECRQM_RESET
                        };
                        let reply = format!("\x1b[?{MODE_SIXEL_DISPLAY};{value}$y");
                        sink(OutputEvent::Reply(reply.into_bytes()));
                        self.held.clear();
                        self.state = State::Ground;
                        index += 1;
                    } else {
                        self.held.push(b'$');
                        self.release_held(sink);
                    }
                    span_start = index;
                }
                State::OscPrefix => {
                    // `held` は `ESC ]` と、そのあとに読んだ `52;` の始まり。
                    let read = self.held.len() - OSC_INTRODUCER_LEN;
                    if OSC_52_PREFIX.get(read) == Some(&byte) {
                        self.held.push(byte);
                        index += 1;
                        if read + 1 == OSC_52_PREFIX.len() {
                            self.state = State::Osc52Selector;
                        }
                    } else {
                        self.release_held(sink);
                    }
                    span_start = index;
                }
                State::Osc52Selector => {
                    match byte {
                        b';' => {
                            self.held.push(byte);
                            self.state = State::Osc52Payload { selector: None };
                            index += 1;
                        }
                        _ if is_selector(byte) => {
                            self.held.push(byte);
                            self.state = State::Osc52SelectorEnd {
                                selector: Some(byte),
                            };
                            index += 1;
                        }
                        _ => self.release_held(sink),
                    }
                    span_start = index;
                }
                &mut State::Osc52SelectorEnd { selector } => {
                    if byte == b';' {
                        self.held.push(byte);
                        self.state = State::Osc52Payload { selector };
                        index += 1;
                    } else {
                        self.release_held(sink);
                    }
                    span_start = index;
                }
                &mut State::Osc52Payload { selector } => {
                    if byte == b'?' {
                        self.held.push(byte);
                        self.state = State::Osc52Terminator { selector };
                        index += 1;
                    } else {
                        self.release_held(sink);
                    }
                    span_start = index;
                }
                &mut State::Osc52Terminator { selector } => {
                    match byte {
                        BEL | C1_ST => {
                            let terminator = if byte == BEL {
                                Terminator::Bel
                            } else {
                                Terminator::St
                            };
                            self.finish_clipboard_read(selector, terminator, sink);
                            index += 1;
                        }
                        ESC => {
                            self.held.push(byte);
                            self.state = State::Osc52TerminatorEscape { selector };
                            index += 1;
                        }
                        _ => self.release_held(sink),
                    }
                    span_start = index;
                }
                &mut State::Osc52TerminatorEscape { selector } => {
                    if byte == b'\\' {
                        self.finish_clipboard_read(selector, Terminator::St, sink);
                        index += 1;
                    } else {
                        self.release_held(sink);
                    }
                    span_start = index;
                }
                State::Sixel(decoder) => {
                    let rest = &data[index..];
                    let end = rest
                        .iter()
                        .position(|&byte| matches!(byte, ESC | C1_ST | CAN | SUB))
                        .unwrap_or(rest.len());
                    decoder.feed(&rest[..end]);
                    index += end;
                    if let Some(&terminator) = data.get(index) {
                        index += 1;
                        let State::Sixel(decoder) =
                            std::mem::replace(&mut self.state, State::Ground)
                        else {
                            unreachable!("the state is Sixel here");
                        };
                        match terminator {
                            ESC => self.state = State::SixelEscape(decoder),
                            C1_ST => self.finish_sixel(*decoder, sink),
                            _ => tracing::debug!("sixel: cancelled by CAN or SUB"),
                        }
                    }
                    span_start = index;
                }
                State::SixelEscape(_) => {
                    let State::SixelEscape(decoder) =
                        std::mem::replace(&mut self.state, State::Ground)
                    else {
                        unreachable!("the state is SixelEscape here");
                    };
                    self.finish_sixel(*decoder, sink);
                    if byte == b'\\' {
                        index += 1;
                    } else {
                        // ST でない ESC は、画像を終えて次のシーケンスを始める。
                        self.held.push(ESC);
                        self.state = State::Escape;
                    }
                    span_start = index;
                }
            }
        }
        if matches!(self.state, State::Ground) && span_start < data.len() {
            sink(OutputEvent::Bytes(&data[span_start..]));
        }
    }

    /// `byte` がシーケンスの始まり（`ESC` か 8 ビットの DCS）。UTF-8 の文字の途中のバイトは含めない。
    fn starts_sequence(&mut self, byte: u8) -> bool {
        match byte {
            ESC => {
                self.utf8_continuation = 0;
                true
            }
            0x00..=0x7f => {
                self.utf8_continuation = 0;
                false
            }
            0x80..=0xbf => {
                if self.utf8_continuation > 0 {
                    self.utf8_continuation -= 1;
                    return false;
                }
                byte == C1_DCS
            }
            0xc2..=0xdf => {
                self.utf8_continuation = 1;
                false
            }
            0xe0..=0xef => {
                self.utf8_continuation = 2;
                false
            }
            0xf0..=0xf4 => {
                self.utf8_continuation = 3;
                false
            }
            _ => {
                self.utf8_continuation = 0;
                false
            }
        }
    }

    /// 手元に置いていたバイトを流し、Ground に戻る。読んでいるバイトは Ground で読み直す。
    fn release_held(&mut self, sink: &mut dyn FnMut(OutputEvent)) {
        sink(OutputEvent::Bytes(&self.held));
        self.held.clear();
        self.state = State::Ground;
    }

    /// 読み終えた読み取りの依頼を渡し、Ground に戻る。
    fn finish_clipboard_read(
        &mut self,
        selector: Option<u8>,
        terminator: Terminator,
        sink: &mut dyn FnMut(OutputEvent),
    ) {
        sink(OutputEvent::ClipboardRead(ReadRequest {
            selector,
            terminator,
        }));
        self.held.clear();
        self.state = State::Ground;
    }

    fn finish_sixel(&mut self, decoder: SixelDecoder, sink: &mut dyn FnMut(OutputEvent)) {
        if let Some(image) = decoder.finish() {
            sink(OutputEvent::Sixel {
                image,
                scrolling: !self.sixel_display_mode,
            });
        }
    }
}

/// `byte` が OSC 52 の `Pc` になれる。libghostty-vt と同じく 1 文字だけを見る。
fn is_selector(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

/// XTSMGRAPHICS（`CSI ? Pi ; Pa ; Pv S`）への答え。色レジスタの数と sixel の大きさは決まった値で、変える
/// 依頼には失敗と答える。
fn graphics_attribute_reply(params: &str) -> Vec<u8> {
    let mut params = params.split(';').map(|value| value.parse::<u32>().ok());
    let item = params.next().flatten();
    let action = params.next().flatten();
    let values = match item {
        Some(GRAPHICS_ITEM_COLORS) => COLOR_REGISTERS.to_string(),
        Some(GRAPHICS_ITEM_SIXEL_SIZE) => format!("{MAX_IMAGE_WIDTH};{MAX_IMAGE_HEIGHT}"),
        _ => {
            let item = item.unwrap_or(0);
            return format!("\x1b[?{item};{GRAPHICS_ERROR_ITEM};0S").into_bytes();
        }
    };
    let item = item.unwrap_or(0);
    match action {
        Some(GRAPHICS_READ | GRAPHICS_RESET | GRAPHICS_READ_MAX) => {
            format!("\x1b[?{item};{GRAPHICS_SUCCESS};{values}S").into_bytes()
        }
        Some(GRAPHICS_SET) => format!("\x1b[?{item};{GRAPHICS_FAILURE};{values}S").into_bytes(),
        _ => format!("\x1b[?{item};{GRAPHICS_ERROR_ACTION};{values}S").into_bytes(),
    }
}

/// `ESC P` か 8 ビットの DCS で始まる `held` の、P2 の値。
fn background_mode(held: &[u8]) -> u32 {
    let start = if held.first() == Some(&C1_DCS) { 1 } else { 2 };
    let params = String::from_utf8_lossy(&held[start..]);
    params
        .split(';')
        .nth(BACKGROUND_PARAM_INDEX)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

/// `ESC [ ?` で始まる `held` のパラメータ。
fn private_params(held: &[u8]) -> &str {
    std::str::from_utf8(&held[3..]).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テストで比べやすい形の結果。
    #[derive(Debug, PartialEq, Eq)]
    enum Owned {
        Bytes(Vec<u8>),
        Reply(Vec<u8>),
        Sixel(u32, u32, bool),
        ClipboardRead(ReadRequest),
    }

    /// `chunks` を順に読ませ、続いたバイト列を一つにまとめて返す。
    fn scan_chunks(scanner: &mut OutputScanner, chunks: &[&[u8]]) -> Vec<Owned> {
        let mut events: Vec<Owned> = Vec::new();
        for chunk in chunks {
            scanner.scan(chunk, &mut |event| match event {
                OutputEvent::Bytes(bytes) => {
                    if let Some(Owned::Bytes(last)) = events.last_mut() {
                        last.extend_from_slice(bytes);
                    } else {
                        events.push(Owned::Bytes(bytes.to_vec()));
                    }
                }
                OutputEvent::Reply(reply) => events.push(Owned::Reply(reply)),
                OutputEvent::Sixel { image, scrolling } => {
                    events.push(Owned::Sixel(image.width, image.height, scrolling));
                }
                OutputEvent::ClipboardRead(request) => {
                    events.push(Owned::ClipboardRead(request));
                }
            });
        }
        events
    }

    fn scan(data: &[u8]) -> Vec<Owned> {
        scan_chunks(&mut OutputScanner::new(), &[data])
    }

    /// 1 バイトずつ読ませても同じ結果になることも確認する。
    fn scan_checked(data: &[u8]) -> Vec<Owned> {
        let chunks: Vec<&[u8]> = data.chunks(1).collect();
        let by_byte = scan_chunks(&mut OutputScanner::new(), &chunks);
        let whole = scan(data);
        assert_eq!(whole, by_byte);
        whole
    }

    fn bytes(data: &[u8]) -> Owned {
        Owned::Bytes(data.to_vec())
    }

    #[test]
    fn ordinary_output_passes_unchanged() {
        for data in [
            b"hello\r\n".as_slice(),
            b"\x1b[31mred\x1b[0m",
            b"\x1b]0;title\x07",
            b"\x1b]0;title\x1b\\",
            b"\x1b_Ga=q\x1b\\",
            b"\x1b[?25l\x1b[?1049h",
            b"\x1bcabc",
            b"\x1b\x1b[0m",
            "日本語".as_bytes(),
        ] {
            assert_eq!(scan_checked(data), [bytes(data)], "{data:?}");
        }
    }

    #[test]
    fn a_sixel_is_cut_out_in_the_order_of_the_stream() {
        assert_eq!(
            scan_checked(b"ab\x1bPq@\x1b\\cd"),
            [bytes(b"ab"), Owned::Sixel(1, 1, true), bytes(b"cd"),]
        );
    }

    #[test]
    fn sixel_parameters_are_read() {
        // P2 の 1 は色を置かなかった画素を透過にする。
        let mut scanner = OutputScanner::new();
        let mut alpha = None;
        scanner.scan(b"\x1bP0;1;8q@A\x1b\\", &mut |event| {
            if let OutputEvent::Sixel { image, .. } = event {
                alpha = Some(image.rgba[7]);
            }
        });
        assert_eq!(alpha, Some(0));
        let mut scanner = OutputScanner::new();
        scanner.scan(b"\x1bP0;0;8q@A\x1b\\", &mut |event| {
            if let OutputEvent::Sixel { image, .. } = event {
                alpha = Some(image.rgba[7]);
            }
        });
        assert_eq!(alpha, Some(u8::MAX));
    }

    #[test]
    fn other_dcs_pass_unchanged() {
        for data in [
            b"\x1bP$qm\x1b\\".as_slice(),
            b"\x1bP+q544e\x1b\\",
            b"\x1bP1$r0m\x1b\\",
            b"\x1bPtmux;x\x1b\\",
            b"\x1bP\x1b\\",
        ] {
            assert_eq!(scan_checked(data), [bytes(data)], "{data:?}");
        }
    }

    #[test]
    fn eight_bit_controls_start_and_end_a_sixel() {
        assert_eq!(
            scan_checked(b"a\x90q@\x9cb"),
            [bytes(b"a"), Owned::Sixel(1, 1, true), bytes(b"b")]
        );
        // ST だけが 8 ビットでも、DCS だけが 8 ビットでもよい。
        assert_eq!(scan_checked(b"\x1bPq@\x9c"), [Owned::Sixel(1, 1, true)]);
        assert_eq!(scan_checked(b"\x90q@\x1b\\"), [Owned::Sixel(1, 1, true)]);
    }

    #[test]
    fn continuation_bytes_of_utf8_are_not_controls() {
        // 「ぐ」は E3 81 90、U+0090 は C2 90。
        for text in ["ぐq@", "\u{90}q@", "𐐐q@"] {
            assert_eq!(scan_checked(text.as_bytes()), [bytes(text.as_bytes())]);
        }
        assert_eq!(
            text_after_sixel(),
            [Owned::Sixel(1, 1, true), bytes("ぐ".as_bytes())]
        );
    }

    fn text_after_sixel() -> Vec<Owned> {
        let mut data = b"\x1bPq@\x1b\\".to_vec();
        data.extend_from_slice("ぐ".as_bytes());
        scan_checked(&data)
    }

    #[test]
    fn an_eight_bit_dcs_without_q_passes_unchanged() {
        assert_eq!(scan_checked(b"\x90$qm\x9c"), [bytes(b"\x90$qm\x9c")]);
    }

    #[test]
    fn cancelling_drops_the_image() {
        assert_eq!(scan_checked(b"\x1bPq@\x18a"), [bytes(b"a")]);
        assert_eq!(scan_checked(b"\x1bPq@\x1aa"), [bytes(b"a")]);
    }

    #[test]
    fn an_escape_sequence_ends_the_image_and_is_kept() {
        assert_eq!(
            scan_checked(b"\x1bPq@\x1b[0m"),
            [Owned::Sixel(1, 1, true), bytes(b"\x1b[0m")]
        );
        assert_eq!(
            scan_checked(b"\x1bPq@\x1bPq@@\x1b\\"),
            [Owned::Sixel(1, 1, true), Owned::Sixel(2, 1, true)]
        );
    }

    #[test]
    fn images_without_pixels_are_not_reported() {
        assert_eq!(scan_checked(b"a\x1bPq\x1b\\b"), [bytes(b"ab")]);
    }

    #[test]
    fn the_sixel_display_mode_is_followed_and_passed_on() {
        let data = b"\x1b[?80h\x1bPq@\x1b\\\x1b[?1;80l\x1bPq@\x1b\\\x1b[?80h\x1bc\x1bPq@\x1b\\";
        assert_eq!(
            scan_checked(data),
            [
                bytes(b"\x1b[?80h"),
                Owned::Sixel(1, 1, false),
                bytes(b"\x1b[?1;80l"),
                Owned::Sixel(1, 1, true),
                bytes(b"\x1b[?80h\x1bc"),
                Owned::Sixel(1, 1, true),
            ]
        );
    }

    #[test]
    fn other_private_modes_do_not_change_the_sixel_display_mode() {
        let mut scanner = OutputScanner::new();
        scan_chunks(&mut scanner, &[b"\x1b[?180h\x1b[?801h"]);
        assert!(!scanner.sixel_display_mode);
    }

    #[test]
    fn graphics_attributes_are_answered_in_order() {
        let reply = |text: &str| Owned::Reply(text.as_bytes().to_vec());
        assert_eq!(
            scan_checked(b"a\x1b[?1;1;0Sb\x1b[?2;1;0Sc"),
            [
                bytes(b"a"),
                reply("\x1b[?1;0;1024S"),
                bytes(b"b"),
                reply("\x1b[?2;0;4096;4096S"),
                bytes(b"c"),
            ]
        );
        assert_eq!(scan(b"\x1b[?1;4S"), [reply("\x1b[?1;0;1024S")]);
        assert_eq!(scan(b"\x1b[?2;2S"), [reply("\x1b[?2;0;4096;4096S")]);
        assert_eq!(scan(b"\x1b[?1;3;16S"), [reply("\x1b[?1;3;1024S")]);
        assert_eq!(scan(b"\x1b[?1;9S"), [reply("\x1b[?1;2;1024S")]);
        // ReGIS は持たない。
        assert_eq!(scan(b"\x1b[?3;1;0S"), [reply("\x1b[?3;1;0S")]);
        assert_eq!(scan(b"\x1b[?S"), [reply("\x1b[?0;1;0S")]);
    }

    #[test]
    fn scroll_up_without_the_question_mark_passes() {
        assert_eq!(scan_checked(b"\x1b[3S"), [bytes(b"\x1b[3S")]);
    }

    #[test]
    fn the_sixel_display_mode_is_reported() {
        let reply = |text: &str| Owned::Reply(text.as_bytes().to_vec());
        assert_eq!(
            scan_checked(b"\x1b[?80$p\x1b[?80h\x1b[?80$p"),
            [
                reply("\x1b[?80;2$y"),
                bytes(b"\x1b[?80h"),
                reply("\x1b[?80;1$y"),
            ]
        );
        // ほかのモードの問い合わせは端末エミュレーションが答える。
        assert_eq!(scan_checked(b"\x1b[?25$p"), [bytes(b"\x1b[?25$p")]);
        assert_eq!(scan_checked(b"\x1b[?80$x"), [bytes(b"\x1b[?80$x")]);
    }

    fn clipboard_read(selector: Option<u8>, terminator: Terminator) -> Owned {
        Owned::ClipboardRead(ReadRequest {
            selector,
            terminator,
        })
    }

    #[test]
    fn clipboard_reads_are_cut_out_in_the_order_of_the_stream() {
        assert_eq!(
            scan_checked(b"ab\x1b]52;c;?\x07cd"),
            [
                bytes(b"ab"),
                clipboard_read(Some(b'c'), Terminator::Bel),
                bytes(b"cd"),
            ]
        );
    }

    #[test]
    fn clipboard_reads_take_any_selector_and_terminator() {
        for (data, selector, terminator) in [
            (b"\x1b]52;c;?\x1b\\".as_slice(), Some(b'c'), Terminator::St),
            (b"\x1b]52;p;?\x9c", Some(b'p'), Terminator::St),
            (b"\x1b]52;s;?\x07", Some(b's'), Terminator::Bel),
            (b"\x1b]52;0;?\x07", Some(b'0'), Terminator::Bel),
            (b"\x1b]52;;?\x07", None, Terminator::Bel),
        ] {
            assert_eq!(
                scan_checked(data),
                [clipboard_read(selector, terminator)],
                "{data:?}"
            );
        }
    }

    #[test]
    fn other_oscs_pass_unchanged() {
        for data in [
            // 書き込み。
            b"\x1b]52;c;aGVsbG8=\x07".as_slice(),
            b"\x1b]52;;aGVsbG8=\x1b\\",
            // 空にする。
            b"\x1b]52;c;\x07",
            // `?` で終わらないもの。
            b"\x1b]52;c;?x\x07",
            // `Pc` が 2 文字以上のものは、libghostty-vt も読まない。
            b"\x1b]52;pc;?\x07",
            b"\x1b]52\x07",
            b"\x1b]521;c;?\x07",
            b"\x1b]5;c;?\x07",
            b"\x1b]7;file:///c/\x07",
            b"\x1b]\x07",
        ] {
            assert_eq!(scan_checked(data), [bytes(data)], "{data:?}");
        }
    }

    #[test]
    fn an_escape_after_the_question_mark_starts_the_next_sequence() {
        let data = b"\x1b]52;c;?\x1b[0m";
        assert_eq!(scan_checked(data), [bytes(data)]);
        // 手元に置いていたものを流したあとの `ESC` から、読み取りの依頼を読み直す。
        assert_eq!(
            scan_checked(b"\x1b]52;c;\x1b]52;c;?\x07"),
            [
                bytes(b"\x1b]52;c;"),
                clipboard_read(Some(b'c'), Terminator::Bel),
            ]
        );
    }

    #[test]
    fn long_clipboard_writes_are_not_held() {
        let mut data = b"\x1b]52;c;".to_vec();
        data.extend(std::iter::repeat_n(b'A', MAX_HELD_BYTES * 4));
        data.push(0x07);
        assert_eq!(scan_checked(&data), [bytes(&data)]);
    }

    #[test]
    fn long_parameters_are_not_held_forever() {
        let mut data = b"\x1bP".to_vec();
        data.extend(std::iter::repeat_n(b'1', MAX_HELD_BYTES * 2));
        data.push(b'q');
        assert_eq!(scan_checked(&data), [bytes(&data)]);
    }
}
