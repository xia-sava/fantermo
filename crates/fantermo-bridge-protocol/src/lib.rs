//! fantermo とブリッジの間で、標準入出力のパイプに流す単位の形。
//!
//! 一つの単位は `[種類 1 バイト][長さ 4 バイト（リトルエンディアン）][ペイロード]`。長さはペイロードの
//! バイト数で、[`MAX_PAYLOAD_LEN`] を超えるものはエラーにする。数はすべてリトルエンディアン。
//!
//! fantermo からブリッジへ（[`ToBridge`]）:
//!
//! | 種類 | 名前 | ペイロード |
//! |---|---|---|
//! | 1 | 開始の通知 | プロトコルのバージョン（`u32`） |
//! | 2 | データ | pty へ書くバイト列 |
//! | 3 | リサイズ | 桁数・行数・ピクセルの幅・ピクセルの高さ（`u16` を 4 つ） |
//! | 4 | 終了の依頼 | なし |
//!
//! ブリッジから fantermo へ（[`FromBridge`]）:
//!
//! | 種類 | 名前 | ペイロード |
//! |---|---|---|
//! | 1 | 開始の通知 | プロトコルのバージョン（`u32`） |
//! | 2 | データ | pty から読んだバイト列 |
//! | 3 | 子プロセスの終了 | 0 なら終了コード、1 ならシグナルの番号（1 バイト）と、その値（`i32`） |
//! | 4 | エラー | UTF-8 の文 |
//!
//! 流れ: 互いに最初の単位として開始の通知を送り、相手のバージョンを [`check_version`] で確かめる。合わなければ
//! ブリッジはエラーの単位を返して終わる。fantermo は開始の通知の次にリサイズを送り、ブリッジはそれを受け取って
//! から子プロセスを起動する。子プロセスが終わると、ブリッジは子プロセスの終了の単位を送って終わる。

#![forbid(unsafe_code)]

use std::fmt;
use std::io::{self, Read, Write};

/// このプロトコルのバージョン。単位の形や意味を変えたら上げる。
pub const VERSION: u32 = 1;

/// 種類と長さの部分のバイト数。
pub const HEADER_LEN: usize = 5;

/// 一つの単位のペイロードの上限（1 MiB）。これより長いデータは、送る側が分けて送る。
pub const MAX_PAYLOAD_LEN: usize = 1 << 20;

const KIND_HELLO: u8 = 1;
const KIND_DATA: u8 = 2;
const KIND_TO_BRIDGE_RESIZE: u8 = 3;
const KIND_TO_BRIDGE_QUIT: u8 = 4;
const KIND_FROM_BRIDGE_EXIT: u8 = 3;
const KIND_FROM_BRIDGE_ERROR: u8 = 4;

const HELLO_LEN: usize = 4;
const RESIZE_LEN: usize = 8;
const EXIT_LEN: usize = 5;
const EXIT_TAG_CODE: u8 = 0;
const EXIT_TAG_SIGNAL: u8 = 1;

/// 端末の大きさ。pty の `winsize` と同じく、桁数・行数とピクセルの幅・高さを持つ。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowSize {
    pub cols: u16,
    pub rows: u16,
    pub width_px: u16,
    pub height_px: u16,
}

/// 子プロセスの終わり方。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildExit {
    /// 自分で終わった。値は終了コード。
    Code(i32),
    /// シグナルで終わった。値はシグナルの番号。
    Signal(i32),
}

/// fantermo からブリッジへ送る単位。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToBridge {
    /// 開始の通知。最初に一度だけ送る。
    Hello { version: u32 },
    /// pty へ書くバイト列。
    Data(Vec<u8>),
    /// 端末の大きさ。開始の通知の次に一度送り、そのあとは大きさが変わるたびに送る。
    Resize(WindowSize),
    /// 子プロセスを終わらせる依頼。
    Quit,
}

/// ブリッジから fantermo へ送る単位。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FromBridge {
    /// 開始の通知。最初に一度だけ送る。
    Hello { version: u32 },
    /// pty から読んだバイト列。
    Data(Vec<u8>),
    /// 子プロセスが終わった。ブリッジが送る最後の単位。
    Exit(ChildExit),
    /// ブリッジが続けられなくなった理由。
    Error(String),
}

/// 単位の種類とペイロードの読み書き。[`ToBridge`] と [`FromBridge`] が実装する。
pub trait Message: Sized {
    /// 種類の値。
    fn kind(&self) -> u8;
    /// ペイロードを `out` の後ろに足す。
    fn encode_payload(&self, out: &mut Vec<u8>);
    /// 種類とペイロードから単位を作る。
    fn decode(kind: u8, payload: &[u8]) -> Result<Self, DecodeError>;
}

/// 受け取ったバイト列を単位として読めなかった。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// 長さが [`MAX_PAYLOAD_LEN`] を超えている。
    TooLong { kind: u8, len: u32 },
    /// 知らない種類。
    UnknownKind(u8),
    /// 長さの決まった種類で、ペイロードの長さが違う。
    BadLength { kind: u8, len: usize },
    /// 子プロセスの終了の単位で、終わり方の値が 0 でも 1 でもない。
    BadExitTag(u8),
    /// エラーの単位の文が UTF-8 でない。
    InvalidText,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { kind, len } => write!(
                f,
                "データが長すぎます（種類 {kind}、長さ {len}、上限 {MAX_PAYLOAD_LEN}）"
            ),
            Self::UnknownKind(kind) => write!(f, "不明な種類のデータです（種類 {kind}）"),
            Self::BadLength { kind, len } => {
                write!(
                    f,
                    "データの長さが正しくありません（種類 {kind}、長さ {len}）"
                )
            }
            Self::BadExitTag(tag) => write!(
                f,
                "子プロセスの終了の種類を示す値が正しくありません（{tag}）"
            ),
            Self::InvalidText => write!(f, "エラーの文が UTF-8 ではありません"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// ペイロードが [`MAX_PAYLOAD_LEN`] を超えていて、一つの単位にできない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooLongError {
    pub len: usize,
}

impl fmt::Display for TooLongError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "単位が長すぎます（長さ {}、上限 {MAX_PAYLOAD_LEN}）",
            self.len
        )
    }
}

impl std::error::Error for TooLongError {}

/// 相手のプロトコルのバージョンが [`VERSION`] と違う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionMismatch {
    pub ours: u32,
    pub theirs: u32,
}

impl fmt::Display for VersionMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ブリッジのプロトコルのバージョンが一致しません（こちら {}、相手 {}）。fantermo を入れ直してください。",
            self.ours, self.theirs
        )
    }
}

impl std::error::Error for VersionMismatch {}

/// 相手が開始の通知で伝えたバージョンが [`VERSION`] と同じかを確かめる。
pub fn check_version(theirs: u32) -> Result<(), VersionMismatch> {
    if theirs == VERSION {
        Ok(())
    } else {
        Err(VersionMismatch {
            ours: VERSION,
            theirs,
        })
    }
}

impl Message for ToBridge {
    fn kind(&self) -> u8 {
        match self {
            Self::Hello { .. } => KIND_HELLO,
            Self::Data(_) => KIND_DATA,
            Self::Resize(_) => KIND_TO_BRIDGE_RESIZE,
            Self::Quit => KIND_TO_BRIDGE_QUIT,
        }
    }

    fn encode_payload(&self, out: &mut Vec<u8>) {
        match self {
            Self::Hello { version } => out.extend_from_slice(&version.to_le_bytes()),
            Self::Data(bytes) => out.extend_from_slice(bytes),
            Self::Resize(size) => {
                for value in [size.cols, size.rows, size.width_px, size.height_px] {
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
            Self::Quit => {}
        }
    }

    fn decode(kind: u8, payload: &[u8]) -> Result<Self, DecodeError> {
        match kind {
            KIND_HELLO => decode_hello(kind, payload).map(|version| Self::Hello { version }),
            KIND_DATA => Ok(Self::Data(payload.to_vec())),
            KIND_TO_BRIDGE_RESIZE => {
                let bytes: &[u8; RESIZE_LEN] = fixed(kind, payload)?;
                let value = |i: usize| u16::from_le_bytes([bytes[i * 2], bytes[i * 2 + 1]]);
                Ok(Self::Resize(WindowSize {
                    cols: value(0),
                    rows: value(1),
                    width_px: value(2),
                    height_px: value(3),
                }))
            }
            KIND_TO_BRIDGE_QUIT => {
                let _: &[u8; 0] = fixed(kind, payload)?;
                Ok(Self::Quit)
            }
            _ => Err(DecodeError::UnknownKind(kind)),
        }
    }
}

impl Message for FromBridge {
    fn kind(&self) -> u8 {
        match self {
            Self::Hello { .. } => KIND_HELLO,
            Self::Data(_) => KIND_DATA,
            Self::Exit(_) => KIND_FROM_BRIDGE_EXIT,
            Self::Error(_) => KIND_FROM_BRIDGE_ERROR,
        }
    }

    fn encode_payload(&self, out: &mut Vec<u8>) {
        match self {
            Self::Hello { version } => out.extend_from_slice(&version.to_le_bytes()),
            Self::Data(bytes) => out.extend_from_slice(bytes),
            Self::Exit(exit) => {
                let (tag, value) = match exit {
                    ChildExit::Code(code) => (EXIT_TAG_CODE, code),
                    ChildExit::Signal(signal) => (EXIT_TAG_SIGNAL, signal),
                };
                out.push(tag);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Self::Error(text) => out.extend_from_slice(text.as_bytes()),
        }
    }

    fn decode(kind: u8, payload: &[u8]) -> Result<Self, DecodeError> {
        match kind {
            KIND_HELLO => decode_hello(kind, payload).map(|version| Self::Hello { version }),
            KIND_DATA => Ok(Self::Data(payload.to_vec())),
            KIND_FROM_BRIDGE_EXIT => {
                let bytes: &[u8; EXIT_LEN] = fixed(kind, payload)?;
                let value = i32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
                match bytes[0] {
                    EXIT_TAG_CODE => Ok(Self::Exit(ChildExit::Code(value))),
                    EXIT_TAG_SIGNAL => Ok(Self::Exit(ChildExit::Signal(value))),
                    tag => Err(DecodeError::BadExitTag(tag)),
                }
            }
            KIND_FROM_BRIDGE_ERROR => String::from_utf8(payload.to_vec())
                .map(Self::Error)
                .map_err(|_| DecodeError::InvalidText),
            _ => Err(DecodeError::UnknownKind(kind)),
        }
    }
}

fn decode_hello(kind: u8, payload: &[u8]) -> Result<u32, DecodeError> {
    let bytes: &[u8; HELLO_LEN] = fixed(kind, payload)?;
    Ok(u32::from_le_bytes(*bytes))
}

/// 長さの決まった種類のペイロードを、その長さの配列として見る。
fn fixed<const N: usize>(kind: u8, payload: &[u8]) -> Result<&[u8; N], DecodeError> {
    payload.try_into().map_err(|_| DecodeError::BadLength {
        kind,
        len: payload.len(),
    })
}

/// 単位を種類・長さ・ペイロードのバイト列にして `out` の後ろに足す。
pub fn encode_into<M: Message>(message: &M, out: &mut Vec<u8>) -> Result<(), TooLongError> {
    let start = out.len();
    out.push(message.kind());
    out.extend_from_slice(&[0; 4]);
    message.encode_payload(out);
    let len = out.len() - start - HEADER_LEN;
    if len > MAX_PAYLOAD_LEN {
        out.truncate(start);
        return Err(TooLongError { len });
    }
    let len = u32::try_from(len).expect("MAX_PAYLOAD_LEN は u32 に収まる");
    out[start + 1..start + HEADER_LEN].copy_from_slice(&len.to_le_bytes());
    Ok(())
}

/// 単位をバイト列にする。
pub fn encode<M: Message>(message: &M) -> Result<Vec<u8>, TooLongError> {
    let mut out = Vec::new();
    encode_into(message, &mut out)?;
    Ok(out)
}

/// 単位を `writer` へ書く。長すぎる単位は `InvalidInput` のエラーにする。
pub fn write_message<M: Message>(writer: &mut impl Write, message: &M) -> io::Result<()> {
    let bytes =
        encode(message).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    writer.write_all(&bytes)
}

/// `reader` から単位を一つ読む。単位の切れ目で終わりに来たら `None`。単位の途中で終わったら
/// `UnexpectedEof`、単位として読めなければ `InvalidData`（中身は [`DecodeError`]）のエラーにする。
pub fn read_message<M: Message>(reader: &mut impl Read) -> io::Result<Option<M>> {
    let mut header = [0; HEADER_LEN];
    let mut filled = 0;
    while filled < HEADER_LEN {
        match reader.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let (kind, len) = parse_header(&header).map_err(invalid_data)?;
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload)?;
    M::decode(kind, &payload).map(Some).map_err(invalid_data)
}

fn invalid_data(error: DecodeError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

/// 種類と長さを読み、長さが上限を超えていればエラーにする。
fn parse_header(header: &[u8; HEADER_LEN]) -> Result<(u8, usize), DecodeError> {
    let kind = header[0];
    let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]);
    match usize::try_from(len) {
        Ok(len) if len <= MAX_PAYLOAD_LEN => Ok((kind, len)),
        _ => Err(DecodeError::TooLong { kind, len }),
    }
}

/// 途中で切れて届くバイト列を溜め、揃った単位から順に取り出す。
#[derive(Debug, Default)]
pub struct Decoder {
    buf: Vec<u8>,
    /// `buf` のうち、取り出し終えた部分の長さ。
    start: usize,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 届いたバイト列を溜める。
    pub fn push(&mut self, bytes: &[u8]) {
        if self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        } else if self.start > self.buf.len() / 2 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// 溜めたうち、まだ取り出していないバイト数。
    pub fn pending_len(&self) -> usize {
        self.buf.len() - self.start
    }

    /// 揃った単位を一つ取り出す。揃っていなければ `None`。エラーのあとは同じエラーを返し続ける
    /// （どこが次の単位の始まりか分からないため）。
    pub fn next_message<M: Message>(&mut self) -> Result<Option<M>, DecodeError> {
        let rest = &self.buf[self.start..];
        let Some(header) = rest.first_chunk::<HEADER_LEN>() else {
            return Ok(None);
        };
        let (kind, len) = parse_header(header)?;
        let Some(payload) = rest[HEADER_LEN..].get(..len) else {
            return Ok(None);
        };
        let message = M::decode(kind, payload)?;
        self.start += HEADER_LEN + len;
        Ok(Some(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_bridge_samples() -> Vec<ToBridge> {
        vec![
            ToBridge::Hello { version: VERSION },
            ToBridge::Data(b"echo hi\r".to_vec()),
            ToBridge::Data(Vec::new()),
            ToBridge::Resize(WindowSize {
                cols: 132,
                rows: 50,
                width_px: 1188,
                height_px: 1000,
            }),
            ToBridge::Quit,
        ]
    }

    fn from_bridge_samples() -> Vec<FromBridge> {
        vec![
            FromBridge::Hello { version: VERSION },
            FromBridge::Data("こんにちは\r\n".as_bytes().to_vec()),
            FromBridge::Exit(ChildExit::Code(7)),
            FromBridge::Exit(ChildExit::Code(-1)),
            FromBridge::Exit(ChildExit::Signal(9)),
            FromBridge::Error("起動できません".to_string()),
        ]
    }

    fn round_trip<M: Message + Clone + PartialEq + fmt::Debug>(samples: &[M]) {
        let mut bytes = Vec::new();
        for message in samples {
            encode_into(message, &mut bytes).unwrap();
        }
        let mut decoder = Decoder::new();
        decoder.push(&bytes);
        for message in samples {
            assert_eq!(decoder.next_message::<M>().unwrap().as_ref(), Some(message));
        }
        assert_eq!(decoder.next_message::<M>().unwrap(), None);
        assert_eq!(decoder.pending_len(), 0);

        let mut reader = bytes.as_slice();
        for message in samples {
            assert_eq!(
                read_message::<M>(&mut reader).unwrap().as_ref(),
                Some(message)
            );
        }
        assert_eq!(read_message::<M>(&mut reader).unwrap(), None);
    }

    #[test]
    fn round_trips_both_directions() {
        round_trip(&to_bridge_samples());
        round_trip(&from_bridge_samples());
    }

    #[test]
    fn layout_is_kind_then_little_endian_length() {
        let bytes = encode(&ToBridge::Resize(WindowSize {
            cols: 0x0102,
            rows: 3,
            width_px: 4,
            height_px: 5,
        }))
        .unwrap();
        assert_eq!(bytes, [3, 8, 0, 0, 0, 2, 1, 3, 0, 4, 0, 5, 0]);
        let bytes = encode(&FromBridge::Exit(ChildExit::Signal(15))).unwrap();
        assert_eq!(bytes, [3, 5, 0, 0, 0, 1, 15, 0, 0, 0]);
        assert_eq!(encode(&ToBridge::Quit).unwrap(), [4, 0, 0, 0, 0]);
        assert_eq!(
            encode(&FromBridge::Hello { version: 1 }).unwrap(),
            [1, 4, 0, 0, 0, 1, 0, 0, 0]
        );
    }

    #[test]
    fn decoder_takes_bytes_one_at_a_time() {
        let samples = from_bridge_samples();
        let mut bytes = Vec::new();
        for message in &samples {
            encode_into(message, &mut bytes).unwrap();
        }
        let mut decoder = Decoder::new();
        let mut got = Vec::new();
        for byte in bytes {
            decoder.push(&[byte]);
            while let Some(message) = decoder.next_message::<FromBridge>().unwrap() {
                got.push(message);
            }
        }
        assert_eq!(got, samples);
        assert_eq!(decoder.pending_len(), 0);
    }

    #[test]
    fn decoder_keeps_partial_message() {
        let bytes = encode(&ToBridge::Data(b"abcdef".to_vec())).unwrap();
        let mut decoder = Decoder::new();
        decoder.push(&bytes[..3]);
        assert_eq!(decoder.next_message::<ToBridge>().unwrap(), None);
        decoder.push(&bytes[3..8]);
        assert_eq!(decoder.next_message::<ToBridge>().unwrap(), None);
        assert_eq!(decoder.pending_len(), 8);
        decoder.push(&bytes[8..]);
        assert_eq!(
            decoder.next_message::<ToBridge>().unwrap(),
            Some(ToBridge::Data(b"abcdef".to_vec()))
        );
    }

    /// 1 バイトずつしか返さない `Read`。
    struct Trickle<'a>(&'a [u8]);

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let Some((&first, rest)) = self.0.split_first() else {
                return Ok(0);
            };
            if buf.is_empty() {
                return Ok(0);
            }
            buf[0] = first;
            self.0 = rest;
            Ok(1)
        }
    }

    #[test]
    fn read_message_handles_short_reads() {
        let samples = to_bridge_samples();
        let mut bytes = Vec::new();
        for message in &samples {
            encode_into(message, &mut bytes).unwrap();
        }
        let mut reader = Trickle(&bytes);
        for message in &samples {
            assert_eq!(
                read_message::<ToBridge>(&mut reader).unwrap().as_ref(),
                Some(message)
            );
        }
        assert_eq!(read_message::<ToBridge>(&mut reader).unwrap(), None);
    }

    #[test]
    fn read_message_reports_eof_inside_message() {
        let bytes = encode(&ToBridge::Data(b"abcdef".to_vec())).unwrap();
        for cut in [1, HEADER_LEN, bytes.len() - 1] {
            let error = read_message::<ToBridge>(&mut &bytes[..cut]).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof, "cut={cut}");
        }
    }

    #[test]
    fn payload_length_limit() {
        let largest = ToBridge::Data(vec![0; MAX_PAYLOAD_LEN]);
        let bytes = encode(&largest).unwrap();
        let mut decoder = Decoder::new();
        decoder.push(&bytes);
        assert_eq!(decoder.next_message::<ToBridge>().unwrap(), Some(largest));

        let too_long = ToBridge::Data(vec![0; MAX_PAYLOAD_LEN + 1]);
        assert_eq!(
            encode(&too_long),
            Err(TooLongError {
                len: MAX_PAYLOAD_LEN + 1
            })
        );
        let mut out = vec![9];
        assert!(encode_into(&too_long, &mut out).is_err());
        assert_eq!(out, [9]);
        let error = write_message(&mut Vec::new(), &too_long).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

        // 長さだけ届いた時点で、ペイロードを待たずにエラーにする。
        let len = u32::try_from(MAX_PAYLOAD_LEN + 1).unwrap();
        let mut header = vec![KIND_DATA];
        header.extend_from_slice(&len.to_le_bytes());
        let mut decoder = Decoder::new();
        decoder.push(&header);
        let expected = DecodeError::TooLong {
            kind: KIND_DATA,
            len,
        };
        assert_eq!(decoder.next_message::<ToBridge>(), Err(expected.clone()));
        assert_eq!(decoder.next_message::<ToBridge>(), Err(expected));
        let error = read_message::<ToBridge>(&mut header.as_slice()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        let mut header = vec![KIND_DATA];
        header.extend_from_slice(&u32::MAX.to_le_bytes());
        let mut decoder = Decoder::new();
        decoder.push(&header);
        assert!(matches!(
            decoder.next_message::<FromBridge>(),
            Err(DecodeError::TooLong { .. })
        ));
    }

    #[test]
    fn unknown_kind() {
        for kind in [0, 5, 255] {
            let bytes = [kind, 0, 0, 0, 0];
            let mut decoder = Decoder::new();
            decoder.push(&bytes);
            assert_eq!(
                decoder.next_message::<ToBridge>(),
                Err(DecodeError::UnknownKind(kind))
            );
            assert_eq!(
                decoder.next_message::<FromBridge>(),
                Err(DecodeError::UnknownKind(kind))
            );
            let error = read_message::<FromBridge>(&mut bytes.as_slice()).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            let inner = error
                .into_inner()
                .unwrap()
                .downcast::<DecodeError>()
                .unwrap();
            assert_eq!(*inner, DecodeError::UnknownKind(kind));
        }
    }

    #[test]
    fn wrong_length_for_fixed_kinds() {
        assert_eq!(
            ToBridge::decode(KIND_HELLO, &[1, 0, 0]),
            Err(DecodeError::BadLength {
                kind: KIND_HELLO,
                len: 3
            })
        );
        assert_eq!(
            ToBridge::decode(KIND_TO_BRIDGE_RESIZE, &[0; 9]),
            Err(DecodeError::BadLength {
                kind: KIND_TO_BRIDGE_RESIZE,
                len: 9
            })
        );
        assert_eq!(
            ToBridge::decode(KIND_TO_BRIDGE_QUIT, &[0]),
            Err(DecodeError::BadLength {
                kind: KIND_TO_BRIDGE_QUIT,
                len: 1
            })
        );
        assert_eq!(
            FromBridge::decode(KIND_FROM_BRIDGE_EXIT, &[0; 4]),
            Err(DecodeError::BadLength {
                kind: KIND_FROM_BRIDGE_EXIT,
                len: 4
            })
        );
        assert_eq!(
            FromBridge::decode(KIND_FROM_BRIDGE_EXIT, &[2, 0, 0, 0, 0]),
            Err(DecodeError::BadExitTag(2))
        );
        assert_eq!(
            FromBridge::decode(KIND_FROM_BRIDGE_ERROR, &[0xff, 0xfe]),
            Err(DecodeError::InvalidText)
        );
    }

    #[test]
    fn version_check() {
        assert_eq!(check_version(VERSION), Ok(()));
        let mismatch = check_version(VERSION + 1).unwrap_err();
        assert_eq!(
            mismatch,
            VersionMismatch {
                ours: VERSION,
                theirs: VERSION + 1
            }
        );
        // ブリッジは合わないことをエラーの単位で返す。
        let reply = FromBridge::Error(mismatch.to_string());
        let mut decoder = Decoder::new();
        decoder.push(&encode(&reply).unwrap());
        assert_eq!(decoder.next_message::<FromBridge>().unwrap(), Some(reply));
    }

    #[test]
    fn decoder_compacts_consumed_bytes() {
        let message = ToBridge::Data(vec![7; 100]);
        let bytes = encode(&message).unwrap();
        let mut decoder = Decoder::new();
        for _ in 0..1000 {
            decoder.push(&bytes);
            decoder.push(&bytes[..10]);
            assert_eq!(
                decoder.next_message::<ToBridge>().unwrap().as_ref(),
                Some(&message)
            );
            decoder.push(&bytes[10..]);
            assert_eq!(
                decoder.next_message::<ToBridge>().unwrap().as_ref(),
                Some(&message)
            );
            assert_eq!(decoder.next_message::<ToBridge>().unwrap(), None);
        }
        assert!(decoder.buf.len() <= bytes.len() * 3);
    }
}
