use super::*;

#[test]
fn osc_0_and_2_set_the_title() {
    let mut vt = new_vt();
    assert_eq!(vt.take_title_change().unwrap(), None);
    vt.feed("\x1b]0;日本語のタイトル\x07".as_bytes());
    assert_eq!(
        vt.take_title_change().unwrap().as_deref(),
        Some("日本語のタイトル")
    );
    // 一度取り出したら、次に設定されるまでは返さない。
    assert_eq!(vt.take_title_change().unwrap(), None);
    vt.feed(b"\x1b]2;vim\x1b\\");
    assert_eq!(vt.take_title_change().unwrap().as_deref(), Some("vim"));
}

#[test]
fn only_the_last_title_is_returned() {
    let mut vt = new_vt();
    vt.feed(b"\x1b]2;first\x07\x1b]2;second\x07");
    assert_eq!(vt.take_title_change().unwrap().as_deref(), Some("second"));
}

#[test]
fn empty_title_is_returned_as_cleared() {
    let mut vt = new_vt();
    vt.feed(b"\x1b]2;vim\x07");
    vt.take_title_change().unwrap();
    vt.feed(b"\x1b]2;\x07");
    assert_eq!(vt.take_title_change().unwrap().as_deref(), Some(""));
}

#[test]
fn title_with_invalid_utf8_is_ignored() {
    let mut vt = new_vt();
    vt.feed(b"\x1b]2;vim\x07");
    vt.take_title_change().unwrap();
    vt.feed(b"\x1b]2;caf\xe9\x07");
    assert_eq!(vt.take_title_change().unwrap(), None);
}

#[test]
fn reset_clears_the_title() {
    let mut vt = new_vt();
    vt.feed(b"\x1b]2;vim\x07");
    vt.take_title_change().unwrap();
    vt.feed(b"\x1bc");
    assert_eq!(vt.take_title_change().unwrap().as_deref(), Some(""));
}

#[test]
fn setting_the_same_title_again_is_not_a_change() {
    let mut vt = new_vt();
    vt.feed(b"\x1b]2;vim\x07");
    vt.take_title_change().unwrap();
    vt.feed(b"\x1b]2;vim\x07");
    assert_eq!(vt.take_title_change().unwrap(), None);
}

#[test]
fn bel_rings_the_bell_once() {
    let mut vt = new_vt();
    assert!(!vt.take_bell());
    vt.feed(b"a\x07b\x07");
    assert!(vt.take_bell());
    assert!(!vt.take_bell());
    // BEL は画面に何も出さない。
    assert_eq!(vt.plain_text().unwrap(), "ab");
}

#[test]
fn bel_ending_an_osc_is_not_a_bell() {
    let mut vt = new_vt();
    vt.feed(b"\x1b]2;vim\x07");
    assert!(!vt.take_bell());
}
