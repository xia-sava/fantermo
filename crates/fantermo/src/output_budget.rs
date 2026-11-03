//! バックエンドの読むためのスレッドが、ウィンドウのスレッドへ渡してまだ受け取られていない出力のバイト数を
//! 数える。上限に達したら、読むためのスレッドは受け取られるまで次を渡すのを待つ。その間は読まないので、
//! pty の側でアプリの出力が止まる。ConPTY とブリッジの両方が使う。
//!
//! 渡したことの知らせ（`Wake`）もここでまとめる。知らせたあと、ウィンドウのスレッドが受け取り始める
//! （[`OutputBudget::receiving`]）までは知らせ直さないので、まだ処理されていない知らせは端末ごとに高々一つになる。
//! 知らせを積みすぎると、ウィンドウのスレッドがそれを処理し続けて、キー入力と描画が後回しになるため。

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

/// ウィンドウのスレッドがまだ受け取っていない出力のバイト数の上限。ブリッジが書き終えていない入力に置く
/// 上限と同じ大きさ。ConPTY の一度の読み取り（64 KiB）なら 1024 回分。
pub const MAX_PENDING_OUTPUT: usize = 64 * 1024 * 1024;

/// まだ受け取られていない出力のバイト数と、上限で待っている読むためのスレッドを起こすもの。
pub struct OutputBudget {
    limit: usize,
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    /// 渡して、まだ受け取られていないバイト数。
    pending: usize,
    /// 破棄した。このあとは待たない。
    closed: bool,
    /// 知らせたあと、ウィンドウのスレッドがまだ受け取り始めていない。
    notified: bool,
}

impl OutputBudget {
    /// 受け取られていないバイト数の上限が `limit` のもの。
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            state: Mutex::new(State {
                pending: 0,
                closed: false,
                notified: false,
            }),
            changed: Condvar::new(),
        }
    }

    /// `len` バイトを渡してよくなるまで待ち、数に入れる。足すと上限を超えるなら、受け取られて収まるまで待つ。
    /// 受け取られていないものがなければ、一つで上限を超える大きさでも待たずに数に入れる。[`close`](Self::close)
    /// したら待つのをやめて `false` を返す。そのときは渡さずに読むのをやめる。
    pub fn push(&self, len: usize) -> bool {
        let mut state = self.lock();
        while !state.closed && state.pending > 0 && state.pending.saturating_add(len) > self.limit {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|e| poisoned(e, "wait"));
        }
        if state.closed {
            return false;
        }
        state.pending = state.pending.saturating_add(len);
        true
    }

    /// 読むためのスレッドが、[`push`](Self::push) した分をチャネルに入れ終えた。まだ知らせていなければ `wake` で
    /// 知らせる。`wake` は積めたら `true` を返す。積めなければ、次の `sent` でまた知らせる。知らせたあとは、
    /// ウィンドウのスレッドが受け取り始めるまで知らせない。
    pub fn sent(&self, wake: impl FnOnce() -> bool) {
        let mut state = self.lock();
        if !state.notified {
            // 知らせを積むだけで待たないので、ロックを持ったまま呼んでよい。
            state.notified = wake();
        }
    }

    /// ウィンドウのスレッドがチャネルから受け取り始める。チャネルを読む前に呼ぶ。このあとに入れたものは
    /// [`sent`](Self::sent) がもう一度知らせる。前に入れたものは、この呼び出しのあとに読むチャネルの中にある。
    pub fn receiving(&self) {
        self.lock().notified = false;
    }

    /// `len` バイトを受け取った。上限で待っているスレッドを起こす。
    pub fn drained(&self, len: usize) {
        let mut state = self.lock();
        state.pending = state.pending.saturating_sub(len);
        self.changed.notify_all();
    }

    /// 破棄する。上限で待っているスレッドを起こし、このあとの [`push`](Self::push) は待たずに `false` を返す。
    pub fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| poisoned(e, "lock"))
    }
}

/// 値を変える途中で止まることはない（足し引きと代入だけ）ので、ほかのスレッドが panic したあとも中身はそのまま使う。
fn poisoned<T>(error: PoisonError<T>, operation: &str) -> T {
    tracing::warn!("the output budget is poisoned ({operation}); using it as it is");
    error.into_inner()
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };

    use super::*;

    const LIMIT: usize = 100;

    #[test]
    fn pushes_up_to_the_limit_do_not_wait() {
        let budget = OutputBudget::new(LIMIT);
        assert!(budget.push(60));
        assert!(budget.push(40));
        budget.drained(100);
        // 受け取られていないものがなければ、一つで上限を超えても通す。
        assert!(budget.push(LIMIT * 3));
    }

    /// `sent` が知らせたかどうかを返す。知らせは `posted` の値で積めたことにする。
    fn sent_with(budget: &OutputBudget, posted: bool) -> bool {
        let mut woken = false;
        budget.sent(|| {
            woken = true;
            posted
        });
        woken
    }

    #[test]
    fn a_wake_is_asked_for_once_until_receiving_starts() {
        let budget = OutputBudget::new(LIMIT);
        assert!(budget.push(1));
        assert!(sent_with(&budget, true));
        assert!(budget.push(1));
        assert!(!sent_with(&budget, true));
        budget.receiving();
        // 受け取り始めたあとに入れたものは、また知らせる。
        assert!(budget.push(1));
        assert!(sent_with(&budget, true));
        assert!(!sent_with(&budget, true));
    }

    #[test]
    fn a_wake_that_could_not_be_posted_is_asked_for_again() {
        let budget = OutputBudget::new(LIMIT);
        assert!(budget.push(1));
        assert!(sent_with(&budget, false));
        assert!(budget.push(1));
        assert!(sent_with(&budget, true));
        assert!(!sent_with(&budget, true));
    }

    #[test]
    fn a_push_over_the_limit_waits_until_drained() {
        let budget = Arc::new(OutputBudget::new(LIMIT));
        let drained = Arc::new(AtomicBool::new(false));
        assert!(budget.push(LIMIT));
        let pusher = thread::spawn({
            let budget = Arc::clone(&budget);
            let drained = Arc::clone(&drained);
            move || {
                let pushed = budget.push(1);
                // drained より前には戻らない。
                (pushed, drained.load(Ordering::SeqCst))
            }
        });
        drained.store(true, Ordering::SeqCst);
        budget.drained(1);
        assert_eq!(pusher.join().unwrap(), (true, true));
    }

    #[test]
    fn closing_releases_a_waiting_push() {
        let budget = Arc::new(OutputBudget::new(LIMIT));
        let closed = Arc::new(AtomicBool::new(false));
        assert!(budget.push(LIMIT));
        let pusher = thread::spawn({
            let budget = Arc::clone(&budget);
            let closed = Arc::clone(&closed);
            move || {
                let pushed = budget.push(1);
                (pushed, closed.load(Ordering::SeqCst))
            }
        });
        closed.store(true, Ordering::SeqCst);
        budget.close();
        assert_eq!(pusher.join().unwrap(), (false, true));
        // 閉じたあとは、上限に収まっても渡さない。
        budget.drained(LIMIT);
        assert!(!budget.push(1));
    }
}
