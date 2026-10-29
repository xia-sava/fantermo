//! Win32 のキーボードのメッセージを、端末のキーの情報に直す。

use fantermo_core::vt::{Key, Mods};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, MAPVK_VK_TO_CHAR, MapVirtualKeyW, VIRTUAL_KEY, VK_CAPITAL, VK_CONTROL, VK_LWIN,
    VK_MENU, VK_NUMLOCK, VK_RWIN, VK_SHIFT,
};

/// `MapVirtualKeyW(MAPVK_VK_TO_CHAR)` がデッドキーに立てる最上位のビット。
const DEAD_KEY_FLAG: u32 = 0x8000_0000;

/// WM_KEYDOWN などの `lparam` から読み取ったキーの位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPosition {
    pub scan_code: u16,
    /// 拡張キー（右の Ctrl と Alt、矢印キーの島、テンキーの Enter など）。
    pub extended: bool,
    /// 押されたままで繰り返されている。
    pub repeated: bool,
}

impl KeyPosition {
    pub fn from_lparam(lparam: isize) -> Self {
        Self {
            scan_code: ((lparam >> 16) & 0xff) as u16,
            extended: (lparam >> 24) & 1 == 1,
            repeated: (lparam >> 30) & 1 == 1,
        }
    }
}

/// スキャンコード（セット 1）を、キーボードの物理的な位置の名前に直す。
pub fn key_from_position(position: KeyPosition) -> Key {
    let KeyPosition {
        scan_code,
        extended,
        ..
    } = position;
    if extended {
        return match scan_code {
            0x1c => Key::NumpadEnter,
            0x1d => Key::ControlRight,
            0x35 => Key::NumpadDivide,
            0x37 => Key::PrintScreen,
            0x38 => Key::AltRight,
            0x45 => Key::NumLock,
            0x47 => Key::Home,
            0x48 => Key::ArrowUp,
            0x49 => Key::PageUp,
            0x4b => Key::ArrowLeft,
            0x4d => Key::ArrowRight,
            0x4f => Key::End,
            0x50 => Key::ArrowDown,
            0x51 => Key::PageDown,
            0x52 => Key::Insert,
            0x53 => Key::Delete,
            0x5b => Key::MetaLeft,
            0x5c => Key::MetaRight,
            0x5d => Key::ContextMenu,
            _ => Key::Unidentified,
        };
    }
    match scan_code {
        0x01 => Key::Escape,
        0x02 => Key::Digit1,
        0x03 => Key::Digit2,
        0x04 => Key::Digit3,
        0x05 => Key::Digit4,
        0x06 => Key::Digit5,
        0x07 => Key::Digit6,
        0x08 => Key::Digit7,
        0x09 => Key::Digit8,
        0x0a => Key::Digit9,
        0x0b => Key::Digit0,
        0x0c => Key::Minus,
        0x0d => Key::Equal,
        0x0e => Key::Backspace,
        0x0f => Key::Tab,
        0x10 => Key::Q,
        0x11 => Key::W,
        0x12 => Key::E,
        0x13 => Key::R,
        0x14 => Key::T,
        0x15 => Key::Y,
        0x16 => Key::U,
        0x17 => Key::I,
        0x18 => Key::O,
        0x19 => Key::P,
        0x1a => Key::BracketLeft,
        0x1b => Key::BracketRight,
        0x1c => Key::Enter,
        0x1d => Key::ControlLeft,
        0x1e => Key::A,
        0x1f => Key::S,
        0x20 => Key::D,
        0x21 => Key::F,
        0x22 => Key::G,
        0x23 => Key::H,
        0x24 => Key::J,
        0x25 => Key::K,
        0x26 => Key::L,
        0x27 => Key::Semicolon,
        0x28 => Key::Quote,
        0x29 => Key::Backquote,
        0x2a => Key::ShiftLeft,
        0x2b => Key::Backslash,
        0x2c => Key::Z,
        0x2d => Key::X,
        0x2e => Key::C,
        0x2f => Key::V,
        0x30 => Key::B,
        0x31 => Key::N,
        0x32 => Key::M,
        0x33 => Key::Comma,
        0x34 => Key::Period,
        0x35 => Key::Slash,
        0x36 => Key::ShiftRight,
        0x37 => Key::NumpadMultiply,
        0x38 => Key::AltLeft,
        0x39 => Key::Space,
        0x3a => Key::CapsLock,
        0x3b => Key::F1,
        0x3c => Key::F2,
        0x3d => Key::F3,
        0x3e => Key::F4,
        0x3f => Key::F5,
        0x40 => Key::F6,
        0x41 => Key::F7,
        0x42 => Key::F8,
        0x43 => Key::F9,
        0x44 => Key::F10,
        0x46 => Key::ScrollLock,
        0x47 => Key::Numpad7,
        0x48 => Key::Numpad8,
        0x49 => Key::Numpad9,
        0x4a => Key::NumpadSubtract,
        0x4b => Key::Numpad4,
        0x4c => Key::Numpad5,
        0x4d => Key::Numpad6,
        0x4e => Key::NumpadAdd,
        0x4f => Key::Numpad1,
        0x50 => Key::Numpad2,
        0x51 => Key::Numpad3,
        0x52 => Key::Numpad0,
        0x53 => Key::NumpadDecimal,
        0x56 => Key::IntlBackslash,
        0x57 => Key::F11,
        0x58 => Key::F12,
        // JIS 配列のキー。
        0x70 => Key::KanaMode,
        0x73 => Key::IntlRo,
        0x79 => Key::Convert,
        0x7b => Key::NonConvert,
        0x7d => Key::IntlYen,
        _ => Key::Unidentified,
    }
}

/// いま押されている修飾キーと、ロックの状態。
pub fn current_mods() -> Mods {
    let mut mods = Mods::empty();
    if is_down(VK_SHIFT) {
        mods |= Mods::SHIFT;
    }
    if is_down(VK_CONTROL) {
        mods |= Mods::CTRL;
    }
    if is_down(VK_MENU) {
        mods |= Mods::ALT;
    }
    if is_down(VK_LWIN) || is_down(VK_RWIN) {
        mods |= Mods::SUPER;
    }
    if is_toggled(VK_CAPITAL) {
        mods |= Mods::CAPS_LOCK;
    }
    if is_toggled(VK_NUMLOCK) {
        mods |= Mods::NUM_LOCK;
    }
    mods
}

/// 文字を作るのに使われた修飾キー。Shift と、AltGr（Windows では Ctrl+Alt として届く）。
pub fn consumed_mods(mods: Mods, has_text: bool) -> Mods {
    if !has_text {
        return Mods::empty();
    }
    let mut consumed = mods & Mods::SHIFT;
    if mods.contains(Mods::CTRL | Mods::ALT) {
        consumed |= Mods::CTRL | Mods::ALT;
    }
    consumed
}

/// 修飾キーなしで、このキーがいまのキーボード配列で出す文字。なければ `'\0'`。
pub fn unshifted_codepoint(virtual_key: u16) -> char {
    // SAFETY: 引数は値だけで、キーボード配列を調べるだけ。
    let mapped = unsafe { MapVirtualKeyW(u32::from(virtual_key), MAPVK_VK_TO_CHAR) };
    let code = mapped & !DEAD_KEY_FLAG;
    if code == 0 {
        return '\0';
    }
    // 英字は大文字で返るので、修飾キーなしの小文字にする。
    char::from_u32(code).map_or('\0', |c| c.to_ascii_lowercase())
}

/// WM_CHAR で 1 単位ずつ届く UTF-16 を文字に組み立てる。BMP 外の文字はサロゲートの半分ずつ 2 回で届く。
#[derive(Default)]
pub struct Utf16Assembler {
    high_surrogate: Option<u16>,
}

impl Utf16Assembler {
    /// 文字ができたら返す。サロゲートの前半だけなら覚えておいて `None`。
    pub fn push(&mut self, unit: u16) -> Option<char> {
        if let Some(high) = self.high_surrogate.take()
            && let Some(Ok(c)) = char::decode_utf16([high, unit]).next()
        {
            return Some(c);
        }
        if (0xd800..0xdc00).contains(&unit) {
            self.high_surrogate = Some(unit);
            return None;
        }
        char::from_u32(u32::from(unit))
    }
}

/// 入力された文字のうち、キーの情報に付けてよいもの。制御文字は付けない。
///
/// Windows は Ctrl+C を U+0003 としても届ける。これを付けると、変換器は普通のシェルが無視する
/// CSI u を出す（`crates/fantermo-core/tests/key_encoding.rs`）。
pub fn printable_text(chars: &[char]) -> Option<String> {
    let text: String = chars.iter().filter(|c| !c.is_control()).collect();
    (!text.is_empty()).then_some(text)
}

fn is_down(key: VIRTUAL_KEY) -> bool {
    key_state(key) < 0
}

fn is_toggled(key: VIRTUAL_KEY) -> bool {
    key_state(key) & 1 == 1
}

/// 最上位のビットが押されているか、最下位のビットがロックの状態。
fn key_state(key: VIRTUAL_KEY) -> i16 {
    // SAFETY: 値を渡して、このスレッドのキーボードの状態を読むだけ。
    unsafe { GetKeyState(i32::from(key.0)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(scan_code: u16, extended: bool) -> KeyPosition {
        KeyPosition {
            scan_code,
            extended,
            repeated: false,
        }
    }

    #[test]
    fn lparam_is_decoded() {
        // スキャンコード 0x48、拡張キー、繰り返し。
        let lparam = (0x48 << 16) | (1 << 24) | (1 << 30);
        assert_eq!(
            KeyPosition::from_lparam(lparam),
            KeyPosition {
                scan_code: 0x48,
                extended: true,
                repeated: true
            }
        );
    }

    #[test]
    fn extended_flag_separates_keys_sharing_a_scan_code() {
        assert_eq!(key_from_position(position(0x48, false)), Key::Numpad8);
        assert_eq!(key_from_position(position(0x48, true)), Key::ArrowUp);
        assert_eq!(key_from_position(position(0x1c, false)), Key::Enter);
        assert_eq!(key_from_position(position(0x1c, true)), Key::NumpadEnter);
    }

    #[test]
    fn jis_keys_have_names() {
        assert_eq!(key_from_position(position(0x7d, false)), Key::IntlYen);
        assert_eq!(key_from_position(position(0x73, false)), Key::IntlRo);
    }

    #[test]
    fn surrogate_halves_are_joined() {
        let mut assembler = Utf16Assembler::default();
        let mut units = [0u16; 2];
        '😀'.encode_utf16(&mut units);
        assert_eq!(assembler.push(units[0]), None);
        assert_eq!(assembler.push(units[1]), Some('😀'));
        assert_eq!(assembler.push(u16::from(b'a')), Some('a'));
    }

    #[test]
    fn control_characters_are_not_attached() {
        assert_eq!(printable_text(&['\u{3}']), None);
        assert_eq!(printable_text(&['A']), Some("A".to_owned()));
    }

    #[test]
    fn shift_and_altgr_are_consumed_only_with_text() {
        assert_eq!(consumed_mods(Mods::SHIFT | Mods::CTRL, true), Mods::SHIFT);
        assert_eq!(
            consumed_mods(Mods::CTRL | Mods::ALT, true),
            Mods::CTRL | Mods::ALT
        );
        assert_eq!(consumed_mods(Mods::SHIFT, false), Mods::empty());
    }
}
