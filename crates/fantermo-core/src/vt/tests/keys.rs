use super::*;

fn press(key: Key, mods: Mods, unshifted: char, text: Option<&str>) -> KeyInput<'_> {
    KeyInput {
        key,
        action: KeyAction::Press,
        mods,
        consumed_mods: Mods::empty(),
        unshifted,
        text,
    }
}

#[test]
fn keys_encode_in_legacy_mode() {
    let mut vt = new_vt();
    let mut encode = |input| vt.encode_key(&input).unwrap();
    assert_eq!(encode(press(Key::A, Mods::empty(), 'a', Some("a"))), b"a");
    assert_eq!(encode(press(Key::Enter, Mods::empty(), '\0', None)), b"\r");
    assert_eq!(
        encode(press(Key::Backspace, Mods::empty(), '\0', None)),
        b"\x7f"
    );
    assert_eq!(
        encode(press(Key::ArrowUp, Mods::empty(), '\0', None)),
        b"\x1b[A"
    );
    assert_eq!(encode(press(Key::C, Mods::CTRL, 'c', None)), b"\x03");
    assert_eq!(encode(press(Key::A, Mods::ALT, 'a', Some("a"))), b"\x1ba");
}

#[test]
fn application_cursor_mode_changes_arrow_keys() {
    let mut vt = new_vt();
    // DECCKM: カーソルキーをアプリケーションモードにする（vim などが使う）。
    vt.feed(b"\x1b[?1h");
    let encoded = vt
        .encode_key(&press(Key::ArrowUp, Mods::empty(), '\0', None))
        .unwrap();
    assert_eq!(encoded, b"\x1bOA");
}

#[test]
fn the_wheel_becomes_cursor_keys_only_on_the_alternate_screen() {
    let mut vt = new_vt();
    assert!(!vt.wheel_sends_cursor_keys().unwrap());
    // 既定で Alternate Scroll は有効。
    vt.feed(b"\x1b[?1049h");
    assert!(vt.wheel_sends_cursor_keys().unwrap());
    // アプリがマウスを求めていれば、マウスの報告を送る。
    vt.feed(b"\x1b[?1000h");
    assert!(!vt.wheel_sends_cursor_keys().unwrap());
    vt.feed(b"\x1b[?1000l");
    assert!(vt.wheel_sends_cursor_keys().unwrap());
    // アプリが Alternate Scroll を切ったら送らない。
    vt.feed(b"\x1b[?1007l");
    assert!(!vt.wheel_sends_cursor_keys().unwrap());
    vt.feed(b"\x1b[?1007h");
    assert!(vt.wheel_sends_cursor_keys().unwrap());
    vt.feed(b"\x1b[?1049l");
    assert!(!vt.wheel_sends_cursor_keys().unwrap());
}

#[test]
fn the_wheel_as_cursor_keys_follows_the_cursor_key_mode() {
    let mut vt = new_vt();
    assert_eq!(
        vt.encode_wheel_as_cursor_keys(3).unwrap(),
        b"\x1b[A\x1b[A\x1b[A"
    );
    assert_eq!(vt.encode_wheel_as_cursor_keys(-1).unwrap(), b"\x1b[B");
    vt.feed(b"\x1b[?1h");
    assert_eq!(vt.encode_wheel_as_cursor_keys(2).unwrap(), b"\x1bOA\x1bOA");
    assert_eq!(vt.encode_wheel_as_cursor_keys(-1).unwrap(), b"\x1bOB");
}

/// kitty keyboard protocol では、本物のカーソルキーと同じ形にする。離したのを求めるアプリには離した分も送る。
#[test]
fn the_wheel_as_cursor_keys_follows_the_kitty_keyboard_protocol() {
    let cases: [(&[u8], &[u8]); 3] = [
        // エスケープコードの曖昧さをなくす。
        (b"\x1b[>1u", b"\x1b[A"),
        // 押す・離すを報告する。
        (b"\x1b[>3u", b"\x1b[1;1:1A\x1b[1;1:3A"),
        // すべてのキーをエスケープコードで報告する。
        (b"\x1b[>11u", b"\x1b[1;1:1A\x1b[1;1:3A"),
    ];
    for (flags, expected) in cases {
        let mut vt = new_vt();
        vt.feed(flags);
        assert_eq!(
            vt.encode_wheel_as_cursor_keys(1).unwrap(),
            expected,
            "{flags:?}"
        );
    }
}

/// kitty keyboard protocol の「エスケープコードの曖昧さをなくす」（フラグ 1）では、ロックの状態を
/// 修飾キーの数に入れない。
#[test]
fn kitty_disambiguate_leaves_out_lock_mods() {
    let mut vt = new_vt();
    vt.feed(b"\x1b[>1u");
    assert_eq!(
        vt.terminal.kitty_keyboard_flags().unwrap(),
        key::KittyKeyFlags::DISAMBIGUATE
    );
    let mut encode = |input| vt.encode_key(&input).unwrap();
    let num_lock = Mods::NUM_LOCK;
    let caps_lock = Mods::CAPS_LOCK;
    assert_eq!(
        encode(press(Key::Enter, Mods::SHIFT | num_lock, '\0', None)),
        b"\x1b[13;2u"
    );
    assert_eq!(
        encode(press(Key::Enter, Mods::CTRL | num_lock, '\0', None)),
        b"\x1b[13;5u"
    );
    assert_eq!(
        encode(press(Key::I, Mods::CTRL | num_lock, 'i', None)),
        b"\x1b[105;5u"
    );
    assert_eq!(
        encode(press(Key::Enter, Mods::SHIFT | caps_lock, '\0', None)),
        b"\x1b[13;2u"
    );
    assert_eq!(
        encode(press(Key::I, Mods::CTRL | caps_lock | num_lock, 'i', None)),
        b"\x1b[105;5u"
    );
    assert_eq!(
        encode(press(Key::Enter, num_lock | caps_lock, '\0', None)),
        b"\r"
    );
    assert_eq!(encode(press(Key::A, caps_lock, 'a', Some("A"))), b"A");
}

/// 「すべてのキーをエスケープコードで報告する」（フラグ 8）では、ロックの状態も修飾キーの数に入れる。
#[test]
fn kitty_report_all_includes_lock_mods() {
    let mut vt = new_vt();
    vt.feed(b"\x1b[>8u");
    let mut encode = |input| vt.encode_key(&input).unwrap();
    assert_eq!(
        encode(press(Key::Enter, Mods::SHIFT | Mods::NUM_LOCK, '\0', None)),
        b"\x1b[13;130u"
    );
    assert_eq!(
        encode(press(Key::Enter, Mods::NUM_LOCK, '\0', None)),
        b"\x1b[13;129u"
    );
    assert_eq!(
        encode(press(Key::A, Mods::CAPS_LOCK, 'a', Some("A"))),
        b"\x1b[97;65u"
    );
}

/// kitty keyboard protocol をやめたあとは、ロックの状態をこれまでどおり渡す。
#[test]
fn lock_mods_pass_through_after_kitty_is_popped() {
    let mut vt = new_vt();
    vt.feed(b"\x1b[>1u\x1b[<u");
    assert!(vt.terminal.kitty_keyboard_flags().unwrap().is_empty());
    assert_eq!(
        vt.mods_to_report(Mods::SHIFT | Mods::NUM_LOCK).unwrap(),
        Mods::SHIFT | Mods::NUM_LOCK
    );
}

#[test]
fn key_release_sends_nothing_in_legacy_mode() {
    let mut vt = new_vt();
    let release = KeyInput {
        action: KeyAction::Release,
        ..press(Key::A, Mods::empty(), 'a', None)
    };
    assert!(vt.encode_key(&release).unwrap().is_empty());
}
