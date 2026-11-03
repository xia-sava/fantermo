use super::*;

fn set(text: &str) -> WriteRequest {
    WriteRequest::Set(text.to_owned())
}

#[test]
fn osc_52_writes_are_decoded_for_every_selector_and_terminator() {
    let mut vt = new_vt();
    for data in [
        b"\x1b]52;c;aGVsbG8=\x07".as_slice(),
        b"\x1b]52;c;aGVsbG8=\x1b\\",
        b"\x1b]52;;aGVsbG8=\x07",
        b"\x1b]52;p;aGVsbG8=\x07",
        b"\x1b]52;s;aGVsbG8=\x07",
        b"\x1b]52;c;aGVsbG8\x07",
    ] {
        vt.feed(data);
        assert_eq!(vt.take_clipboard_writes(), [set("hello")], "{data:?}");
        assert!(vt.take_replies().is_empty());
    }
}

#[test]
fn osc_1337_copy_writes_like_osc_52() {
    let mut vt = new_vt();
    vt.feed(b"\x1b]1337;Copy=:aGVsbG8=\x07");
    assert_eq!(vt.take_clipboard_writes(), [set("hello")]);
}

#[test]
fn osc_5522_writes_text_and_refuses_the_rest() {
    let mut vt = new_vt();
    vt.feed(
        b"\x1b]5522;type=write:id=c1\x1b\\\
          \x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;SGVsbG8=\x1b\\\
          \x1b]5522;type=wdata\x1b\\",
    );
    assert_eq!(vt.take_clipboard_writes(), [set("Hello")]);
    assert_eq!(
        vt.take_replies(),
        b"\x1b]5522;type=write:status=DONE:id=c1\x1b\\"
    );

    vt.feed(
        b"\x1b]5522;type=write:id=c2\x1b\\\
          \x1b]5522;type=wdata:mime=aW1hZ2UvcG5n;SGVsbG8=\x1b\\\
          \x1b]5522;type=wdata\x1b\\",
    );
    assert!(vt.take_clipboard_writes().is_empty());
    assert_eq!(
        vt.take_replies(),
        b"\x1b]5522;type=write:status=ENOSYS:id=c2\x1b\\"
    );
}

#[test]
fn invalid_base64_writes_nothing() {
    let mut vt = new_vt();
    vt.feed(b"\x1b]52;c;not base64!\x07");
    assert!(vt.take_clipboard_writes().is_empty());
}

#[test]
fn an_empty_osc_52_clears_the_clipboard() {
    let mut vt = new_vt();
    vt.feed(b"\x1b]52;c;\x07");
    assert_eq!(vt.take_clipboard_writes(), [WriteRequest::Clear]);
}

#[test]
fn writes_are_kept_in_order_across_chunks() {
    let mut vt = new_vt();
    vt.feed(b"\x1b]52;c;YQ");
    vt.feed(b"==\x07\x1b]52;c;Yg==\x07");
    assert_eq!(vt.take_clipboard_writes(), [set("a"), set("b")]);
    assert!(vt.take_clipboard_writes().is_empty());
}

/// base64 で `encoded_bytes` バイトになる OSC 52 の書き込み。
fn osc_52_of_length(encoded_bytes: usize) -> Vec<u8> {
    let mut data = b"\x1b]52;c;".to_vec();
    data.extend(std::iter::repeat_n(b'A', encoded_bytes));
    data.push(0x07);
    data
}

#[test]
fn long_writes_reach_the_clipboard_up_to_the_length_of_an_escape_sequence() {
    const ENCODED_GROUP: usize = 4;
    let mut vt = new_vt();
    // libghostty-vt が手元に置く 2048 バイトを超えても届く。
    for encoded in [ENCODED_GROUP * 1024, 4 * 1024 * 1024] {
        vt.feed(&osc_52_of_length(encoded));
        let writes = vt.take_clipboard_writes();
        let [WriteRequest::Set(text)] = writes.as_slice() else {
            panic!("expected one write of {encoded} encoded bytes");
        };
        assert_eq!(text.len(), encoded / ENCODED_GROUP * 3);
    }
    // エスケープシーケンスの長さの上限（8 MiB）を超えると、端末エミュレーションが捨てる。
    vt.feed(&osc_52_of_length(9 * 1024 * 1024));
    assert!(vt.take_clipboard_writes().is_empty());
}

#[test]
fn clipboard_reads_are_queued_and_not_answered() {
    use crate::clipboard::Terminator;

    let mut vt = new_vt();
    vt.feed(b"a\x1b]52;c;?\x07b\x1b]52;;?\x1b\\");
    assert!(vt.take_replies().is_empty());
    assert_eq!(
        vt.take_clipboard_reads(),
        [
            ReadRequest {
                selector: Some(b'c'),
                terminator: Terminator::Bel,
            },
            ReadRequest {
                selector: None,
                terminator: Terminator::St,
            },
        ]
    );
    assert!(vt.take_clipboard_reads().is_empty());
    assert!(vt.take_clipboard_writes().is_empty());
    assert!(vt.plain_text().unwrap().contains("ab"));
}

#[test]
fn osc_5522_reads_are_refused() {
    let mut vt = new_vt();
    vt.feed(b"\x1b]5522;type=read:id=r1;dGV4dC9wbGFpbg==\x1b\\");
    assert_eq!(
        vt.take_replies(),
        b"\x1b]5522;type=read:status=EPERM:id=r1\x1b\\"
    );
    assert!(vt.take_clipboard_reads().is_empty());
}
