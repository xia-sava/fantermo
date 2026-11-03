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
    // 同じタイトルをもう一度設定しても変更にならない。
    vt.feed(b"\x1b]2;vim\x07");
    assert_eq!(vt.take_title_change().unwrap(), None);
    // 取り出す前に続けて設定したときは、最後のものだけを返す。
    vt.feed(b"\x1b]2;first\x07\x1b]2;second\x07");
    assert_eq!(vt.take_title_change().unwrap().as_deref(), Some("second"));
}

#[test]
fn the_title_is_cleared_by_an_empty_title_or_a_reset_and_kept_by_invalid_utf8() {
    let mut vt = new_vt();
    vt.feed(b"\x1b]2;vim\x07");
    vt.take_title_change().unwrap();
    // UTF-8 として読めないタイトルは捨てる。
    vt.feed(b"\x1b]2;caf\xe9\x07");
    assert_eq!(vt.take_title_change().unwrap(), None);
    vt.feed(b"\x1b]2;\x07");
    assert_eq!(vt.take_title_change().unwrap().as_deref(), Some(""));
    vt.feed(b"\x1b]2;vim\x07");
    vt.take_title_change().unwrap();
    vt.feed(b"\x1bc");
    assert_eq!(vt.take_title_change().unwrap().as_deref(), Some(""));
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
