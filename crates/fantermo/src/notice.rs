//! 利用者への知らせ（警告とエラー）を、メッセージボックスで見せる。
//!
//! メッセージボックスは自分でメッセージを回すので、ウィンドウプロシージャの中から出すときは `App` の
//! 借用を手放してから呼ぶ（`Deferred` と `run_deferred`）。
//!
//! 警告は話題（[`Topic`]）ごとに一つだけ待たせ、積み重ねない。開いている間に届いた警告は閉じたあとに
//! まとめて一つのメッセージボックスで見せ、同じ話題の警告が続けて届けば新しいものだけを残す。開いている
//! ものと同じ文の警告は見せ直さない。

use std::cell::RefCell;

use windows::{
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            IsWindow, MB_ICONERROR, MB_ICONWARNING, MB_OK, MESSAGEBOX_STYLE, MessageBoxW,
        },
    },
    core::{HSTRING, w},
};

/// 警告の話題。同じ話題の警告は、新しいものが古いものに代わる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topic {
    /// 設定ファイルの誤り。
    Settings,
}

/// まとめて見せるときの、警告と警告の区切り。
const SEPARATOR: &str = "\n\n";

thread_local! {
    /// 窓のスレッドの警告の待ち行列。
    static QUEUE: RefCell<Queue> = RefCell::default();
}

/// `owner` の窓に、`topic` についての警告 `text` を見せる。メッセージボックスを閉じるまで戻らない。
/// すでにメッセージボックスを開いているとき（その中で回るメッセージの処理から呼ばれたとき）は、待たせて
/// すぐに戻り、開いているものを閉じたあとに見せる。
pub fn warn(owner: HWND, topic: Topic, text: String) {
    let showing = QUEUE.with_borrow_mut(|queue| {
        queue.post(topic, text);
        queue.is_showing()
    });
    if showing {
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

/// まだ見せていない `topic` の警告を取り下げる。誤りが直ったときに使う。開いているものは閉じない。
pub fn withdraw(topic: Topic) {
    QUEUE.with_borrow_mut(|queue| queue.withdraw(topic));
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

/// 警告の待ち行列。待たせているものと、いま見せているものを話題ごとに持つ。
#[derive(Default)]
struct Queue {
    /// 見せるのを待っている警告。話題ごとに一つ、届いた順。
    pending: Vec<(Topic, String)>,
    /// いま開いているメッセージボックスで見せている警告。
    showing: Vec<(Topic, String)>,
}

impl Queue {
    /// `topic` の警告 `text` を待たせる。同じ話題で待っているものと置き換える。開いているもので同じ
    /// 話題の同じ文を見せているなら、待たせない（待っていた古いものも捨てる）。
    fn post(&mut self, topic: Topic, text: String) {
        self.withdraw(topic);
        if self
            .showing
            .iter()
            .any(|(t, shown)| *t == topic && *shown == text)
        {
            return;
        }
        self.pending.push((topic, text));
    }

    fn withdraw(&mut self, topic: Topic) {
        self.pending.retain(|(t, _)| *t != topic);
    }

    fn is_showing(&self) -> bool {
        !self.showing.is_empty()
    }

    /// 待っている警告をすべて見せるものに移し、一つの文にまとめて返す。待っていなければ `None`。
    fn take(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        self.showing = std::mem::take(&mut self.pending);
        let texts: Vec<&str> = self.showing.iter().map(|(_, text)| text.as_str()).collect();
        Some(texts.join(SEPARATOR))
    }

    /// 見せていたメッセージボックスを閉じた。
    fn finish(&mut self) {
        self.showing.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 話題の違う警告が待っている状態。話題が一つしかないので、待ち行列を直接組む。
    fn queue_with(pending: &[(Topic, &str)]) -> Queue {
        Queue {
            pending: pending
                .iter()
                .map(|&(topic, text)| (topic, text.to_owned()))
                .collect(),
            showing: Vec::new(),
        }
    }

    #[test]
    fn newer_warning_of_the_same_topic_replaces_the_pending_one() {
        let mut queue = Queue::default();
        queue.post(Topic::Settings, "a".to_owned());
        queue.post(Topic::Settings, "b".to_owned());
        assert_eq!(queue.take().as_deref(), Some("b"));
        queue.finish();
        assert_eq!(queue.take(), None);
    }

    #[test]
    fn warnings_pending_together_are_shown_in_one_box() {
        let mut queue = queue_with(&[(Topic::Settings, "a"), (Topic::Settings, "b")]);
        assert_eq!(queue.take().as_deref(), Some("a\n\nb"));
        assert!(queue.is_showing());
        queue.finish();
        assert!(!queue.is_showing());
    }

    #[test]
    fn warnings_arriving_while_shown_wait_until_closed() {
        let mut queue = Queue::default();
        queue.post(Topic::Settings, "a".to_owned());
        assert_eq!(queue.take().as_deref(), Some("a"));
        // 開いている間に届いた。
        queue.post(Topic::Settings, "b".to_owned());
        queue.post(Topic::Settings, "c".to_owned());
        assert!(queue.is_showing());
        queue.finish();
        assert_eq!(queue.take().as_deref(), Some("c"));
    }

    #[test]
    fn the_same_text_as_the_open_box_is_not_shown_again() {
        let mut queue = Queue::default();
        queue.post(Topic::Settings, "a".to_owned());
        assert_eq!(queue.take().as_deref(), Some("a"));
        queue.post(Topic::Settings, "b".to_owned());
        // 待っている間に、開いているものと同じ文に戻った。
        queue.post(Topic::Settings, "a".to_owned());
        queue.finish();
        assert_eq!(queue.take(), None);
        // 閉じたあとに届けば、同じ文でも見せる。
        queue.post(Topic::Settings, "a".to_owned());
        assert_eq!(queue.take().as_deref(), Some("a"));
    }

    #[test]
    fn withdrawn_warnings_are_not_shown() {
        let mut queue = Queue::default();
        queue.post(Topic::Settings, "a".to_owned());
        queue.withdraw(Topic::Settings);
        assert_eq!(queue.take(), None);
    }
}
