//! 利用者への知らせ（警告とエラー）を、メッセージボックスで見せる。
//!
//! メッセージボックスは自分でメッセージを回すので、ウィンドウプロシージャの中から出すときは `App` の
//! 借用を手放してから呼ぶ（`Deferred` と `run_deferred`）。
//!
//! 警告は話題（[`Topic`]）ごとに一つだけ待たせ、積み重ねない。同じ話題の警告が続けて届けば新しいもの
//! だけを残す。開いているメッセージボックスと同じ話題で違う文の警告が届いたら、開いているものを閉じて
//! 新しい文で見せ直す。同じ文なら見せ直さない。取り下げた話題を見せていれば、それも閉じる。ほかの話題の
//! 警告は、開いているものを閉じたあとにまとめて一つのメッセージボックスで見せる。

use std::cell::RefCell;

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        System::Threading::GetCurrentThreadId,
        UI::WindowsAndMessaging::{
            EnumThreadWindows, GW_OWNER, GetClassNameW, GetWindow, IsWindow, MB_ICONERROR,
            MB_ICONWARNING, MB_OK, MESSAGEBOX_STYLE, MessageBoxW, PostMessageW, WM_CLOSE,
        },
    },
    core::{BOOL, HSTRING, w},
};

/// 警告の話題。同じ話題の警告は、新しいものが古いものに代わる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topic {
    /// 設定ファイルの誤り。
    Settings,
    /// 話題が二つ以上あるときの決まりを確かめるための話題。
    #[cfg(test)]
    Other,
}

/// まとめて見せるときの、警告と警告の区切り。
const SEPARATOR: &str = "\n\n";

/// メッセージボックスの窓のクラス名。
const MESSAGE_BOX_CLASS: &str = "#32770";

thread_local! {
    /// 窓のスレッドの警告の待ち行列。
    static QUEUE: RefCell<Queue> = RefCell::default();
}

/// `owner` の窓に、`topic` についての警告 `text` を見せる。メッセージボックスを閉じるまで戻らない。
/// すでにメッセージボックスを開いているとき（その中で回るメッセージの処理から呼ばれたとき）は、待たせて
/// すぐに戻る。開いているものが同じ話題の違う文なら閉じさせ、閉じたあとに新しい文で見せる。
pub fn warn(owner: HWND, topic: Topic, text: String) {
    let (open, close) = QUEUE.with_borrow_mut(|queue| {
        let close = queue.post(topic, text);
        (queue.is_open(), close)
    });
    if close {
        close_message_box(owner);
    }
    if open {
        return;
    }
    while let Some(text) = QUEUE.with_borrow_mut(Queue::take) {
        // 開いている間に子プロセスが終わって窓を閉じると、持ち主の窓はもうない。
        // SAFETY: 窓のハンドルを調べるだけ。
        if unsafe { IsWindow(Some(owner)) }.as_bool() {
            message_box(Some(owner), &text, MB_ICONWARNING);
        }
        QUEUE.with_borrow_mut(Queue::finish);
    }
}

/// `topic` の警告を取り下げる。誤りが直ったときに使う。待っているものは捨て、`owner` の窓に開いている
/// メッセージボックスで見せていれば閉じさせる（ほかの話題の警告も見せていれば、閉じたあとに見せ直す）。
/// メッセージボックスを開いて閉じるのを待つことはしないので、借用を持ったまま呼んでよい。
pub fn withdraw(owner: HWND, topic: Topic) {
    if QUEUE.with_borrow_mut(|queue| queue.withdraw(topic)) {
        close_message_box(owner);
    }
}

/// 持ち主の窓なしに、エラー `text` を見せる。窓を開く前や閉じたあとに使う。
pub fn error(text: &str) {
    message_box(None, text, MB_ICONERROR);
}

fn message_box(owner: Option<HWND>, text: &str, icon: MESSAGEBOX_STYLE) {
    // SAFETY: 文字列は呼び出しの間は生きている。owner は None か有効な窓で、借用は持っていない。
    let result = unsafe { MessageBoxW(owner, &HSTRING::from(text), w!("fantermo"), MB_OK | icon) };
    if result.0 == 0 {
        tracing::warn!(
            "failed to show a message box: {}",
            windows::core::Error::from_thread()
        );
    }
}

/// このスレッドで `owner` の窓を持ち主として開いているメッセージボックスに、閉じる求めを積む。閉じると
/// [`warn`] の中の `MessageBoxW` が戻り、待っている警告があれば続けて見せる。
fn close_message_box(owner: HWND) {
    let mut search = BoxSearch { owner, closed: 0 };
    // SAFETY: search はこの呼び出しの間は生きていて、コールバックはこの呼び出しの中でだけ呼ばれる。
    unsafe {
        _ = EnumThreadWindows(
            GetCurrentThreadId(),
            Some(close_if_message_box),
            LPARAM(std::ptr::from_mut(&mut search) as isize),
        );
    }
    if search.closed == 0 {
        tracing::warn!("found no message box to close");
    }
}

/// [`close_message_box`] がコールバックに渡すもの。
struct BoxSearch {
    owner: HWND,
    /// 閉じる求めを積んだメッセージボックスの数。
    closed: usize,
}

extern "system" fn close_if_message_box(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam は close_message_box が渡した BoxSearch を指し、列挙の間は生きている。
    let search = unsafe { &mut *(lparam.0 as *mut BoxSearch) };
    let mut class = [0u16; 16];
    // SAFETY: hwnd は列挙された窓で、class は書き込める。
    let length = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
    if String::from_utf16_lossy(&class[..length]) != MESSAGE_BOX_CLASS {
        return true.into();
    }
    // SAFETY: hwnd は列挙された窓。
    if unsafe { GetWindow(hwnd, GW_OWNER) }.ok() != Some(search.owner) {
        return true.into();
    }
    // OK だけのメッセージボックスは、WM_CLOSE で OK を押したのと同じに閉じる。
    // SAFETY: hwnd は有効な窓。積むだけで、この場では処理されない。
    match unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) } {
        Ok(()) => search.closed += 1,
        Err(e) => tracing::warn!("failed to close a message box: {e}"),
    }
    true.into()
}

/// 警告の待ち行列。待たせているものと、いま見せているものを話題ごとに持つ。
#[derive(Default)]
struct Queue {
    /// 見せるのを待っている警告。話題ごとに一つ、届いた順。
    pending: Vec<(Topic, String)>,
    /// いま開いているメッセージボックスで見せている警告。閉じさせたら空にする。
    showing: Vec<(Topic, String)>,
    /// メッセージボックスを開いている（閉じさせていても、閉じるまでは開いている）。
    open: bool,
}

impl Queue {
    /// `topic` の警告 `text` を待たせる。同じ話題で待っているものと置き換える。開いているもので同じ
    /// 話題の同じ文を見せているなら、待たせない（待っていた古いものも捨てる）。違う文を見せているなら
    /// 待たせ、開いているものを閉じさせるために `true` を返す。
    fn post(&mut self, topic: Topic, text: String) -> bool {
        self.pending.retain(|(t, _)| *t != topic);
        let Some((_, shown)) = self.showing.iter().find(|(t, _)| *t == topic) else {
            self.pending.push((topic, text));
            return false;
        };
        if *shown == text {
            return false;
        }
        self.stop_showing(topic);
        self.pending.push((topic, text));
        true
    }

    /// `topic` の警告を取り下げる。開いているもので見せていれば、閉じさせるために `true` を返す。
    fn withdraw(&mut self, topic: Topic) -> bool {
        self.pending.retain(|(t, _)| *t != topic);
        if !self.showing.iter().any(|(t, _)| *t == topic) {
            return false;
        }
        self.stop_showing(topic);
        true
    }

    /// 開いているものを閉じさせる。一緒に見せていたほかの話題の警告は、新しいものが待っていなければ、
    /// 閉じたあとに見せ直すよう待たせる。
    fn stop_showing(&mut self, topic: Topic) {
        let shown = std::mem::take(&mut self.showing);
        let again: Vec<(Topic, String)> = shown
            .into_iter()
            .filter(|(t, _)| *t != topic && !self.pending.iter().any(|(p, _)| p == t))
            .collect();
        self.pending.splice(0..0, again);
    }

    fn is_open(&self) -> bool {
        self.open
    }

    /// 待っている警告をすべて見せるものに移し、一つの文にまとめて返す。待っていなければ `None`。
    fn take(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        self.open = true;
        self.showing = std::mem::take(&mut self.pending);
        let texts: Vec<&str> = self.showing.iter().map(|(_, text)| text.as_str()).collect();
        Some(texts.join(SEPARATOR))
    }

    /// 見せていたメッセージボックスを閉じた。
    fn finish(&mut self) {
        self.open = false;
        self.showing.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 警告が待っている状態。
    fn queue_with(pending: &[(Topic, &str)]) -> Queue {
        Queue {
            pending: pending
                .iter()
                .map(|&(topic, text)| (topic, text.to_owned()))
                .collect(),
            ..Queue::default()
        }
    }

    /// `text` を一つ見せている状態。
    fn showing(topic: Topic, text: &str) -> Queue {
        let mut queue = queue_with(&[(topic, text)]);
        assert_eq!(queue.take().as_deref(), Some(text));
        queue
    }

    #[test]
    fn newer_warning_of_the_same_topic_replaces_the_pending_one() {
        let mut queue = Queue::default();
        assert!(!queue.post(Topic::Settings, "a".to_owned()));
        assert!(!queue.post(Topic::Settings, "b".to_owned()));
        assert_eq!(queue.take().as_deref(), Some("b"));
        queue.finish();
        assert_eq!(queue.take(), None);
    }

    #[test]
    fn warnings_pending_together_are_shown_in_one_box() {
        let mut queue = queue_with(&[(Topic::Settings, "a"), (Topic::Other, "b")]);
        assert_eq!(queue.take().as_deref(), Some("a\n\nb"));
        assert!(queue.is_open());
        queue.finish();
        assert!(!queue.is_open());
    }

    #[test]
    fn a_different_text_closes_the_open_box_and_is_shown_next() {
        let mut queue = showing(Topic::Settings, "a");
        assert!(queue.post(Topic::Settings, "b".to_owned()));
        // 閉じるまでは開いているので、届いた警告は待たせる。
        assert!(queue.is_open());
        // 閉じるのを待つ間に届いたものは、閉じさせ直さずに置き換える。
        assert!(!queue.post(Topic::Settings, "c".to_owned()));
        queue.finish();
        assert_eq!(queue.take().as_deref(), Some("c"));
    }

    #[test]
    fn the_same_text_as_the_open_box_is_not_shown_again() {
        let mut queue = showing(Topic::Settings, "a");
        assert!(!queue.post(Topic::Settings, "a".to_owned()));
        queue.finish();
        assert_eq!(queue.take(), None);
        // 閉じたあとに届けば、同じ文でも見せる。
        assert!(!queue.post(Topic::Settings, "a".to_owned()));
        assert_eq!(queue.take().as_deref(), Some("a"));
    }

    #[test]
    fn returning_to_the_shown_text_after_closing_it_shows_it_again() {
        let mut queue = showing(Topic::Settings, "a");
        assert!(queue.post(Topic::Settings, "b".to_owned()));
        // 閉じさせたあとなので、開いているものと同じ文でも見せ直す。
        assert!(!queue.post(Topic::Settings, "a".to_owned()));
        queue.finish();
        assert_eq!(queue.take().as_deref(), Some("a"));
    }

    #[test]
    fn withdrawn_warnings_are_not_shown() {
        let mut queue = Queue::default();
        queue.post(Topic::Settings, "a".to_owned());
        assert!(!queue.withdraw(Topic::Settings));
        assert_eq!(queue.take(), None);
    }

    #[test]
    fn withdrawing_the_shown_topic_closes_the_open_box() {
        let mut queue = showing(Topic::Settings, "a");
        assert!(queue.withdraw(Topic::Settings));
        // 二度目は閉じさせない。
        assert!(!queue.withdraw(Topic::Settings));
        queue.finish();
        assert_eq!(queue.take(), None);
    }

    #[test]
    fn withdrawing_a_topic_not_shown_keeps_the_open_box() {
        let mut queue = showing(Topic::Other, "x");
        assert!(!queue.post(Topic::Settings, "a".to_owned()));
        assert!(!queue.withdraw(Topic::Settings));
        queue.finish();
        assert_eq!(queue.take(), None);
    }

    #[test]
    fn other_topics_shown_together_are_shown_again_after_closing() {
        let mut queue = queue_with(&[(Topic::Other, "x"), (Topic::Settings, "a")]);
        assert_eq!(queue.take().as_deref(), Some("x\n\na"));
        assert!(queue.withdraw(Topic::Settings));
        queue.finish();
        assert_eq!(queue.take().as_deref(), Some("x"));
    }

    #[test]
    fn each_topic_shows_its_newest_text_after_closing() {
        let mut queue = queue_with(&[(Topic::Other, "x"), (Topic::Settings, "a")]);
        assert_eq!(queue.take().as_deref(), Some("x\n\na"));
        assert!(queue.post(Topic::Other, "y".to_owned()));
        assert!(!queue.post(Topic::Settings, "b".to_owned()));
        queue.finish();
        assert_eq!(queue.take().as_deref(), Some("y\n\nb"));
    }
}
