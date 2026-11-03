//! 貼り付けとコピーと選択、アプリからのクリップボードの読み書きの依頼と、読み取りの確認。

use fantermo_core::{
    clipboard::{ReadDecision, ReadRequest, WriteRequest, read_decision},
    config::{ClipboardRead, PasteConfig, PasteConfirm},
    vt::{self, PasteOutcome},
};
use windows::Win32::{Foundation::WPARAM, UI::Input::KeyboardAndMouse::IsWindowEnabled};

use super::{
    App, WM_APP_CLIPBOARD_PROMPT, post_or_warn,
    proc::{Deferred, in_menu},
};
use crate::{
    clipboard,
    clipboard_prompt::{self, Choice},
    session::SessionId,
};

impl App {
    /// クリップボードの文字列を貼り付ける。設定の `paste.confirm` と `paste.confirm_size` に従って確認が
    /// 要るときは、編集のダイアログを開くよう返す。
    pub(super) fn paste_clipboard(&mut self) -> Option<Deferred> {
        let text = match clipboard::text(self.hwnd) {
            Ok(Some(text)) => vt::normalize_paste(&text).into_owned(),
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!("failed to read the clipboard: {e}");
                return None;
            }
        };
        self.paste_text(text)
    }

    /// 改行が LF の `text` を貼り付ける。確認が要るときは、編集のダイアログを開くよう返す。
    pub(super) fn paste_text(&mut self, text: String) -> Option<Deferred> {
        let (needs_confirmation, large) = match paste_action(&self.paste_config, &text) {
            PasteAction::Confirm { large } => (true, large),
            PasteAction::Paste { allow_unsafe } => (
                self.paste(&text, allow_unsafe) == Some(PasteOutcome::Unsafe),
                false,
            ),
        };
        needs_confirmation.then(|| Deferred::EditPaste {
            text,
            font: self.font.clone(),
            large,
        })
    }

    /// `text` を貼り付ける。`allow_unsafe` でなければ、コマンドとして実行される可能性がある文字列は送らない。
    pub(super) fn paste(&mut self, text: &str, allow_unsafe: bool) -> Option<PasteOutcome> {
        let outcome = match self.session_mut().vt.paste(text, allow_unsafe) {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::warn!("failed to paste: {e}");
                return None;
            }
        };
        let encoded = self.session_mut().vt.take_replies();
        self.send(&encoded);
        Some(outcome)
    }

    /// 選択範囲をクリップボードにコピーし、選択を外す。選んでいなければ何もせず `false`。
    pub(super) fn copy_selection(&mut self) -> bool {
        let text = match self.session().vt.selected_text() {
            Ok(Some(text)) => text,
            Ok(None) => return false,
            Err(e) => {
                tracing::warn!("failed to read the selection: {e}");
                return false;
            }
        };
        if let Err(e) = clipboard::set_text(self.hwnd, &text) {
            tracing::warn!("failed to copy to the clipboard: {e}");
        }
        self.clear_selection();
        true
    }

    pub(super) fn clear_selection(&mut self) {
        if let Err(e) = self.session_mut().vt.clear_selection() {
            tracing::warn!("failed to clear the selection: {e}");
        }
        self.invalidate();
    }

    /// `sessions` の `index` の端末のアプリからの、クリップボードの書き込みと読み取りの依頼を処理する。
    pub(super) fn on_clipboard_requests(&mut self, index: usize) {
        for write in self.sessions[index].vt.take_clipboard_writes() {
            let result = match write {
                WriteRequest::Set(text) => clipboard::set_text(self.hwnd, &text),
                WriteRequest::Clear => clipboard::clear(self.hwnd),
            };
            if let Err(e) = result {
                tracing::warn!("failed to write to the clipboard for an application: {e}");
            }
        }
        let id = self.sessions[index].id();
        for request in self.sessions[index].vt.take_clipboard_reads() {
            let allowed = self.sessions[index].clipboard_read_allowed;
            match read_decision(self.clipboard_read, allowed) {
                ReadDecision::Answer => self.answer_clipboard_read(id, request),
                ReadDecision::Refuse => tracing::debug!("refused a clipboard read from {id:?}"),
                ReadDecision::Ask => {
                    tracing::debug!("a clipboard read from {id:?} waits to be asked about");
                    self.clipboard_prompts.push(id, request);
                    self.post_clipboard_prompt();
                }
            }
        }
    }

    /// 確認を待っている依頼を見るよう、ウィンドウに知らせる。
    fn post_clipboard_prompt(&self) {
        post_or_warn(self.hwnd, WM_APP_CLIPBOARD_PROMPT, WPARAM(0));
    }

    /// いまのクリップボードの文字列を、端末 `id` のアプリへ答える。文字列がなければ空で答える。
    fn answer_clipboard_read(&mut self, id: SessionId, request: ReadRequest) {
        let text = match clipboard::text(self.hwnd) {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!("failed to read the clipboard for an application: {e}");
                None
            }
        };
        let reply = request.reply(text.as_deref().unwrap_or_default());
        match self.sessions.iter_mut().find(|session| session.id() == id) {
            Some(session) => session.write(&reply),
            None => tracing::debug!("dropping a clipboard answer for a closed terminal {id:?}"),
        }
    }

    /// 次に確認する依頼があれば、ダイアログを開くよう返す。ほかのダイアログを開いている間（ウィンドウが無効の
    /// 間）とメニューを開いている間は開かず、有効に戻ったときとメニューを閉じたときに見直す。待っている間に設定や
    /// タブの許可が変わったら、それに従って片づける。
    pub(super) fn next_clipboard_prompt(&mut self) -> Option<Deferred> {
        // SAFETY: ウィンドウのハンドルを調べるだけ。
        if !unsafe { IsWindowEnabled(self.hwnd) }.as_bool() {
            return None;
        }
        if in_menu() {
            if self.clipboard_prompts.has_waiting() {
                tracing::debug!("a clipboard read waits for the menu to close");
            }
            return None;
        }
        while let Some((id, request)) = self.clipboard_prompts.start_next() {
            let index = self.sessions.iter().position(|session| session.id() == id);
            let allowed = index.is_some_and(|index| self.sessions[index].clipboard_read_allowed);
            match read_decision(self.clipboard_read, allowed) {
                ReadDecision::Answer => self.answer_clipboard_read(id, request),
                ReadDecision::Refuse => tracing::debug!("refused a clipboard read from {id:?}"),
                ReadDecision::Ask => {
                    if let Some(index) = index {
                        tracing::debug!("asking about a clipboard read from {id:?}");
                        let session = &self.sessions[index];
                        return Some(Deferred::AskClipboardRead {
                            text: clipboard_prompt::request_text(
                                index + 1,
                                session.profile_name.as_deref(),
                                index == self.active,
                            ),
                        });
                    }
                }
            }
            self.clipboard_prompts.finish();
        }
        None
    }

    /// 確認のダイアログの答え `choice` に従う。ダイアログの間にタブを閉じていれば、何もしない。
    pub(super) fn finish_clipboard_prompt(&mut self, choice: Choice) {
        if let Some((id, request)) = self.clipboard_prompts.finish() {
            tracing::debug!("the user answered a clipboard read from {id:?}: {choice:?}");
            let allows = choice != Choice::Deny && self.clipboard_read != ClipboardRead::Deny;
            if allows {
                if choice == Choice::AllowForTab
                    && let Some(session) =
                        self.sessions.iter_mut().find(|session| session.id() == id)
                {
                    session.clipboard_read_allowed = true;
                }
                self.answer_clipboard_read(id, request);
            }
        }
        if self.clipboard_prompts.has_waiting() {
            self.post_clipboard_prompt();
        }
    }

    pub(super) fn has_selection(&self) -> bool {
        self.session()
            .vt
            .selected_text()
            .is_ok_and(|text| text.is_some())
    }

    pub(super) fn select_all(&mut self) {
        if let Err(e) = self.session_mut().vt.select_all() {
            tracing::warn!("failed to select all: {e}");
        }
        self.invalidate();
    }
}

/// 貼り付けるときにすること。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PasteAction {
    /// 送らずに、編集のダイアログで確認する。`large` なら、大きさで確認する。
    Confirm { large: bool },
    /// 端末に渡す。`allow_unsafe` でなければ、コマンドとして実行される可能性がある文字列は端末が送らずに返す。
    Paste { allow_unsafe: bool },
}

/// 貼り付けの設定が `config` のときに、`text` を貼り付けるためにすること。大きさは改行を LF にしたあとの
/// UTF-8 のバイト数で見る。
fn paste_action(config: &PasteConfig, text: &str) -> PasteAction {
    if config.is_large(text.len()) {
        return PasteAction::Confirm { large: true };
    }
    match config.confirm {
        PasteConfirm::Unsafe => PasteAction::Paste {
            allow_unsafe: false,
        },
        PasteConfirm::Multiline if !vt::is_safe_to_paste(text) => {
            PasteAction::Confirm { large: false }
        }
        PasteConfirm::Multiline => PasteAction::Paste {
            allow_unsafe: false,
        },
        PasteConfirm::Never => PasteAction::Paste { allow_unsafe: true },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_confirmation_follows_the_setting() {
        let as_is = PasteAction::Paste {
            allow_unsafe: false,
        };
        let unchecked = PasteAction::Paste { allow_unsafe: true };
        let confirm = PasteAction::Confirm { large: false };
        let config = |confirm| PasteConfig {
            confirm,
            ..PasteConfig::default()
        };
        // 既定は、端末が bracketed paste のモードを見て断ったときだけ確認する。
        assert_eq!(paste_action(&config(PasteConfirm::Unsafe), "a\nb"), as_is);
        assert_eq!(
            paste_action(&config(PasteConfirm::Multiline), "a\nb"),
            confirm
        );
        assert_eq!(
            paste_action(&config(PasteConfirm::Multiline), "a\rb"),
            confirm
        );
        assert_eq!(
            paste_action(&config(PasteConfirm::Multiline), "echo hi"),
            as_is
        );
        assert_eq!(
            paste_action(&config(PasteConfirm::Never), "a\nb"),
            unchecked
        );
        assert_eq!(
            paste_action(&config(PasteConfirm::Never), "echo hi"),
            unchecked
        );
    }

    #[test]
    fn large_paste_is_confirmed_whatever_the_other_settings_say() {
        const LIMIT: usize = 8 * 1024;
        let large = PasteAction::Confirm { large: true };
        let lines = "line\n".repeat(LIMIT / 5 + 1);
        for confirm in PasteConfirm::ALL {
            let config = PasteConfig {
                confirm,
                confirm_size: 8,
            };
            assert_eq!(paste_action(&config, &"x".repeat(LIMIT)), large);
            assert_eq!(paste_action(&config, &lines), large);
            assert_ne!(paste_action(&config, &"x".repeat(LIMIT - 1)), large);
            let disabled = PasteConfig {
                confirm,
                confirm_size: 0,
            };
            assert_ne!(paste_action(&disabled, &"x".repeat(LIMIT)), large);
        }
        // 大きさは文字数ではなく UTF-8 のバイト数で見る。
        let config = PasteConfig {
            confirm: PasteConfirm::Never,
            confirm_size: 8,
        };
        assert_eq!(paste_action(&config, &"あ".repeat(LIMIT / 3 + 1)), large);
    }
}
