//! fantermo が前提にしている、キーの変換器の振る舞い。

use libghostty_vt::{
    Terminal,
    key::{self, Key},
};

/// kitty keyboard protocol の「エスケープコードの曖昧さをなくす」（フラグ 1）でも、変換器は渡された
/// ロックの状態を修飾キーの数に入れる（Shift 1 + Num Lock 128 で 130）。`vt::Vt::encode_key` は
/// フラグ 8 がないときロックの状態を外して渡す。
#[test]
fn kitty_disambiguate_counts_lock_mods_as_given() {
    let mut terminal = Terminal::new(80, 24).unwrap();
    terminal.vt_write(b"\x1b[>1u");
    let mut encoder = key::Encoder::new().unwrap();
    let mut event = key::Event::new().unwrap();
    event
        .set_action(key::Action::Press)
        .set_key(Key::Enter)
        .set_mods(key::Mods::SHIFT | key::Mods::NUM_LOCK)
        .set_consumed_mods(key::Mods::empty())
        .set_unshifted_codepoint('\0')
        .set_utf8(None::<&str>);
    let mut out = Vec::new();
    encoder
        .set_options_from_terminal(&terminal)
        .encode_to_vec(&event, &mut out)
        .unwrap();
    assert_eq!(out, b"\x1b[13;130u");
}

fn encode_ctrl_c(text: Option<&str>) -> Vec<u8> {
    let terminal = Terminal::new(80, 24).unwrap();
    let mut encoder = key::Encoder::new().unwrap();
    let mut event = key::Event::new().unwrap();
    event
        .set_action(key::Action::Press)
        .set_key(Key::C)
        .set_mods(key::Mods::CTRL)
        .set_consumed_mods(key::Mods::empty())
        .set_unshifted_codepoint('c')
        .set_utf8(text);
    let mut out = Vec::new();
    encoder
        .set_options_from_terminal(&terminal)
        .encode_to_vec(&event, &mut out)
        .unwrap();
    out
}

/// 文字を付けない Ctrl+C は ETX（0x03）になる。
#[test]
fn ctrl_c_without_text_is_etx() {
    assert_eq!(encode_ctrl_c(None), b"\x03");
}

/// Windows は Ctrl+C を文字 U+0003 としても届ける。これを文字として付けると、変換器は
/// 普通のシェルが無視する CSI u を出す。ウィンドウの層は制御文字をキーの情報に付けないこと。
#[test]
fn ctrl_c_with_control_char_text_is_csi_u() {
    assert_eq!(encode_ctrl_c(Some("\x03")), b"\x1b[3;5u");
}
