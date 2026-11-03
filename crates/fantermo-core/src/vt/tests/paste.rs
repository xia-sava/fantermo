use super::*;

#[test]
fn pasting_sends_the_text_even_if_the_application_asks_for_paste_events() {
    let mut vt = new_vt();
    vt.feed(b"\x1b[?5522h");
    vt.take_replies();
    assert_eq!(vt.paste("abc", false).unwrap(), PasteOutcome::Pasted);
    assert_eq!(vt.take_replies(), b"abc");
}

#[test]
fn pasting_one_line_sends_it_as_is() {
    let mut vt = new_vt();
    assert_eq!(vt.paste("echo hi", false).unwrap(), PasteOutcome::Pasted);
    assert_eq!(vt.take_replies(), b"echo hi");
}

#[test]
fn pasting_lines_without_bracketed_paste_needs_permission() {
    let mut vt = new_vt();
    assert_eq!(vt.paste("a\nb", false).unwrap(), PasteOutcome::Unsafe);
    assert!(vt.take_replies().is_empty());
    assert_eq!(vt.paste("a\nb", true).unwrap(), PasteOutcome::Pasted);
    // 改行は CR にして送る。
    assert_eq!(vt.take_replies(), b"a\rb");
}

#[test]
fn pasting_lines_with_bracketed_paste_is_safe() {
    let mut vt = new_vt();
    vt.feed(b"\x1b[?2004h");
    assert_eq!(vt.paste("a\nb", false).unwrap(), PasteOutcome::Pasted);
    assert_eq!(vt.take_replies(), b"\x1b[200~a\nb\x1b[201~");
}

#[test]
fn pasting_the_end_of_bracketed_paste_needs_permission() {
    let mut vt = new_vt();
    vt.feed(b"\x1b[?2004h");
    assert_eq!(
        vt.paste("a\x1b[201~b", false).unwrap(),
        PasteOutcome::Unsafe
    );
    assert!(vt.take_replies().is_empty());
    // 許しても、ESC は空白に変えて送るので、貼り付けの途中で終わらない。
    assert_eq!(vt.paste("a\x1b[201~b", true).unwrap(), PasteOutcome::Pasted);
    assert_eq!(vt.take_replies(), b"\x1b[200~a [201~b\x1b[201~");
}

#[test]
fn control_characters_are_pasted_as_spaces() {
    let mut vt = new_vt();
    assert_eq!(vt.paste("a\x1bb\x7fc", true).unwrap(), PasteOutcome::Pasted);
    assert_eq!(vt.take_replies(), b"a b c");
    // CR は改行と同じく、そのまま送る。
    assert_eq!(vt.paste("a\rb\r\n", true).unwrap(), PasteOutcome::Pasted);
    assert_eq!(vt.take_replies(), b"a\rb\r");
}

#[test]
fn crlf_in_pasted_text_is_one_line_break() {
    assert_eq!(normalize_paste("a\r\nb\r\n"), "a\nb\n");
    assert_eq!(normalize_paste("a\rb\nc"), "a\rb\nc");
    assert!(matches!(normalize_paste("ab"), Cow::Borrowed(_)));
    let mut vt = new_vt();
    assert_eq!(vt.paste("a\r\nb", true).unwrap(), PasteOutcome::Pasted);
    assert_eq!(vt.take_replies(), b"a\rb");
}

#[test]
fn safety_of_text_does_not_depend_on_the_mode() {
    assert!(is_safe_to_paste("echo hi"));
    assert!(!is_safe_to_paste("a\nb"));
    assert!(!is_safe_to_paste("a\rb"));
    assert!(!is_safe_to_paste("a\x1b[201~b"));
}
