//! アプリからのクリップボードの読み書き（OSC 52 など）の決まり。Windows のクリップボードそのものは
//! 触らない。
//!
//! 書き込みは常に受ける。読み出しは `clipboard.read` とタブごとの許可で決め、確かめが要る依頼は
//! [`ReadPrompts`] で待たせて、一つずつ確かめる。

use crate::{base64, config::ClipboardRead};

/// アプリから書き込める文字列のバイト数の上限（デコードしたあと）。libghostty-vt がエスケープシーケンスの
/// 長さに持つ上限（OSC 52 は base64 なので、デコードすると約 6 MiB）と同じ値で、OSC 5522 にも同じ値を使う。
pub const MAX_WRITE_BYTES: usize = 8 * 1024 * 1024;

/// アプリへ渡す文字列のバイト数の上限（base64 にする前）。
pub const MAX_READ_BYTES: usize = 1024 * 1024;

/// アプリからの書き込み一つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteRequest {
    /// クリップボードをこの文字列で置き換える。UTF-8 でないバイトは U+FFFD に置き換えてある。
    Set(String),
    /// クリップボードを空にする。
    Clear,
}

/// アプリが書き込んだ `data` を、クリップボードへの書き込みにする。上限を超えていれば `None`。
pub fn write_request(data: &[u8]) -> Option<WriteRequest> {
    (data.len() <= MAX_WRITE_BYTES)
        .then(|| WriteRequest::Set(String::from_utf8_lossy(data).into_owned()))
}

/// 読み出しの依頼の終端。答えも同じ終端で返す。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terminator {
    /// `BEL`
    Bel,
    /// `ESC \`（8 ビットの ST で届いたものも、答えはこの形で返す）
    St,
}

impl Terminator {
    fn bytes(self) -> &'static [u8] {
        match self {
            Terminator::Bel => b"\x07",
            Terminator::St => b"\x1b\\",
        }
    }
}

/// アプリからの読み出しの依頼一つ（`OSC 52 ; Pc ; ? ST`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadRequest {
    /// `Pc`。書いていなければ `None`。
    pub selector: Option<u8>,
    pub terminator: Terminator,
}

impl ReadRequest {
    /// 依頼への答え（`OSC 52 ; Pc ; <base64> ST`）。`Pc` は依頼のもの（書いていなければ `c`）。
    /// `text` が上限を超えていれば、空のクリップボードとして答える（アプリを待たせない）。
    pub fn reply(&self, text: &str) -> Vec<u8> {
        let text = if text.len() <= MAX_READ_BYTES {
            text
        } else {
            tracing::warn!(
                "answering a clipboard read with nothing: {} bytes is over the limit of {MAX_READ_BYTES}",
                text.len()
            );
            ""
        };
        let mut reply = b"\x1b]52;".to_vec();
        reply.push(self.selector.unwrap_or(b'c'));
        reply.push(b';');
        base64::push(&mut reply, text.as_bytes());
        reply.extend_from_slice(self.terminator.bytes());
        reply
    }
}

/// 読み出しの依頼をどうするか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadDecision {
    /// 確かめずに答える。
    Answer,
    /// 答えない。
    Refuse,
    /// 利用者に確かめる。
    Ask,
}

/// 設定 `policy` と、そのタブで今後も許可すると決めたか `allowed_for_tab` から、読み出しの依頼を
/// どうするかを決める。`deny` はタブの許可より強い。
pub fn read_decision(policy: ClipboardRead, allowed_for_tab: bool) -> ReadDecision {
    match policy {
        ClipboardRead::Deny => ReadDecision::Refuse,
        ClipboardRead::Allow => ReadDecision::Answer,
        ClipboardRead::Ask if allowed_for_tab => ReadDecision::Answer,
        ClipboardRead::Ask => ReadDecision::Ask,
    }
}

/// いま確かめている依頼。
#[derive(Debug)]
struct Showing<K> {
    tab: K,
    /// タブを閉じて取り下げたら `None`。ダイアログが閉じるまでは、確かめている状態のまま。
    request: Option<ReadRequest>,
}

/// 確かめを待っている読み出しの依頼。確かめるのは一度に一つで、`K` はタブを見分けるもの。
///
/// タブごとに待たせるのは一つだけにする（読み続けるアプリがいても増えない。答えは待っていた数によらず
/// 一度で足りる）。
#[derive(Debug)]
pub struct ReadPrompts<K> {
    waiting: Vec<(K, ReadRequest)>,
    showing: Option<Showing<K>>,
}

impl<K> Default for ReadPrompts<K> {
    fn default() -> Self {
        Self {
            waiting: Vec::new(),
            showing: None,
        }
    }
}

impl<K: Copy + PartialEq> ReadPrompts<K> {
    /// `tab` の依頼を待たせる。そのタブの依頼がすでに待っていれば、増やさない。
    pub fn push(&mut self, tab: K, request: ReadRequest) {
        if !self.waiting.iter().any(|(waiting, _)| *waiting == tab) {
            self.waiting.push((tab, request));
        }
    }

    /// 次に確かめる依頼を、確かめているものにして返す。確かめている途中か、待っているものがなければ `None`。
    pub fn start_next(&mut self) -> Option<(K, ReadRequest)> {
        if self.showing.is_some() || self.waiting.is_empty() {
            return None;
        }
        let (tab, request) = self.waiting.remove(0);
        self.showing = Some(Showing {
            tab,
            request: Some(request),
        });
        Some((tab, request))
    }

    /// 確かめ終えた。取り下げていなければ、その依頼を返す。
    pub fn finish(&mut self) -> Option<(K, ReadRequest)> {
        let showing = self.showing.take()?;
        Some((showing.tab, showing.request?))
    }

    /// `tab` の依頼を捨てる。いま確かめている依頼も捨てたら `true` を返す（ダイアログを閉じさせる）。
    pub fn discard_tab(&mut self, tab: K) -> bool {
        self.waiting.retain(|(waiting, _)| *waiting != tab);
        let Some(showing) = &mut self.showing else {
            return false;
        };
        showing.tab == tab && showing.request.take().is_some()
    }

    /// 待っている依頼がある。
    pub fn has_waiting(&self) -> bool {
        !self.waiting.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST: ReadRequest = ReadRequest {
        selector: Some(b'c'),
        terminator: Terminator::Bel,
    };

    #[test]
    fn writes_up_to_the_limit_are_accepted() {
        assert_eq!(
            write_request(b"hello"),
            Some(WriteRequest::Set("hello".to_owned()))
        );
        let largest = vec![b'a'; MAX_WRITE_BYTES];
        assert!(write_request(&largest).is_some());
        let too_large = vec![b'a'; MAX_WRITE_BYTES + 1];
        assert_eq!(write_request(&too_large), None);
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        assert_eq!(
            write_request(b"a\xffb"),
            Some(WriteRequest::Set("a\u{fffd}b".to_owned()))
        );
    }

    #[test]
    fn replies_keep_the_selector_and_the_terminator() {
        assert_eq!(REQUEST.reply("hello"), b"\x1b]52;c;aGVsbG8=\x07");
        let request = ReadRequest {
            selector: Some(b'p'),
            terminator: Terminator::St,
        };
        assert_eq!(request.reply("hello"), b"\x1b]52;p;aGVsbG8=\x1b\\");
    }

    #[test]
    fn a_reply_without_a_selector_names_the_clipboard() {
        let request = ReadRequest {
            selector: None,
            terminator: Terminator::Bel,
        };
        assert_eq!(request.reply(""), b"\x1b]52;c;\x07");
    }

    #[test]
    fn text_over_the_limit_is_answered_with_nothing() {
        let largest = "a".repeat(MAX_READ_BYTES);
        assert!(REQUEST.reply(&largest).len() > MAX_READ_BYTES);
        let too_large = "a".repeat(MAX_READ_BYTES + 1);
        assert_eq!(REQUEST.reply(&too_large), b"\x1b]52;c;\x07");
    }

    #[test]
    fn the_setting_and_the_tab_decide_a_read() {
        for allowed in [false, true] {
            assert_eq!(
                read_decision(ClipboardRead::Allow, allowed),
                ReadDecision::Answer
            );
            assert_eq!(
                read_decision(ClipboardRead::Deny, allowed),
                ReadDecision::Refuse
            );
        }
        assert_eq!(read_decision(ClipboardRead::Ask, false), ReadDecision::Ask);
        assert_eq!(
            read_decision(ClipboardRead::Ask, true),
            ReadDecision::Answer
        );
    }

    #[test]
    fn prompts_are_shown_one_at_a_time_in_order() {
        let mut prompts = ReadPrompts::default();
        prompts.push(1, REQUEST);
        prompts.push(2, REQUEST);
        assert_eq!(prompts.start_next(), Some((1, REQUEST)));
        assert_eq!(prompts.start_next(), None);
        assert_eq!(prompts.finish(), Some((1, REQUEST)));
        assert_eq!(prompts.start_next(), Some((2, REQUEST)));
        assert_eq!(prompts.finish(), Some((2, REQUEST)));
        assert_eq!(prompts.start_next(), None);
        assert_eq!(prompts.finish(), None);
    }

    #[test]
    fn a_tab_waits_with_one_request_only() {
        let mut prompts = ReadPrompts::default();
        prompts.push(1, REQUEST);
        prompts.push(1, REQUEST);
        assert_eq!(prompts.start_next(), Some((1, REQUEST)));
        // 確かめている間に届いたものは、一つ待たせる。
        prompts.push(1, REQUEST);
        prompts.push(1, REQUEST);
        assert!(prompts.has_waiting());
        assert_eq!(prompts.finish(), Some((1, REQUEST)));
        assert_eq!(prompts.start_next(), Some((1, REQUEST)));
        assert!(!prompts.has_waiting());
    }

    #[test]
    fn closing_a_tab_discards_its_waiting_request() {
        let mut prompts = ReadPrompts::default();
        prompts.push(1, REQUEST);
        prompts.push(2, REQUEST);
        assert!(!prompts.discard_tab(1));
        assert_eq!(prompts.start_next(), Some((2, REQUEST)));
    }

    #[test]
    fn closing_the_tab_being_asked_about_closes_the_dialog_once() {
        let mut prompts = ReadPrompts::default();
        prompts.push(1, REQUEST);
        prompts.push(2, REQUEST);
        assert_eq!(prompts.start_next(), Some((1, REQUEST)));
        assert!(prompts.discard_tab(1));
        assert!(!prompts.discard_tab(1));
        // ダイアログが閉じるまでは、次を出さない。
        assert_eq!(prompts.start_next(), None);
        assert_eq!(prompts.finish(), None);
        assert_eq!(prompts.start_next(), Some((2, REQUEST)));
    }

    #[test]
    fn closing_another_tab_keeps_the_dialog() {
        let mut prompts = ReadPrompts::default();
        prompts.push(1, REQUEST);
        assert_eq!(prompts.start_next(), Some((1, REQUEST)));
        assert!(!prompts.discard_tab(2));
        assert_eq!(prompts.finish(), Some((1, REQUEST)));
    }
}
