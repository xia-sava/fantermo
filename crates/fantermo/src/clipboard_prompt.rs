//! アプリがクリップボードの内容を読み出そうとしたときの、確認のダイアログ。
//!
//! ダイアログは自分でメッセージを回すので、ウィンドウプロシージャの中から開くときは `App` の借用を手放して
//! から呼ぶ（`Deferred` と `run_deferred`）。

use std::cell::Cell;

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, S_OK, WPARAM},
        UI::{
            Controls::{
                TASKDIALOG_BUTTON, TASKDIALOG_NOTIFICATIONS, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0,
                TD_WARNING_ICON, TDF_ALLOW_DIALOG_CANCELLATION, TDF_POSITION_RELATIVE_TO_WINDOW,
                TDN_CREATED, TDN_DESTROYED, TaskDialogIndirect,
            },
            WindowsAndMessaging::{IDCANCEL, PostMessageW, WM_CLOSE},
        },
    },
    core::{HRESULT, HSTRING, PCWSTR, w},
};

/// 利用者の答え。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// この依頼だけ許可する。
    Allow,
    /// この依頼を許可し、そのタブの以後の依頼も確かめずに許可する。
    AllowForTab,
    /// 拒否する（Esc や閉じるボタンで閉じたときも）。
    Deny,
}

/// 「許可」のボタンの ID。
const ID_ALLOW: i32 = 1001;
/// 「拒否」のボタンの ID。
const ID_DENY: i32 = 1002;
/// 「このタブでは今後も許可」のボタンの ID。
const ID_ALLOW_FOR_TAB: i32 = 1003;

thread_local! {
    /// 開いているダイアログ。タブを閉じたときに閉じさせる。
    static OPEN_DIALOG: Cell<Option<HWND>> = const { Cell::new(None) };
}

/// どのタブからの依頼かを伝える文。タブのタイトルはアプリが決められるので、文に入れない。
///
/// `tab_number` は左から数えたタブの番号（1 から）、`profile` はタブを開いたときのプロファイルの名前。
pub fn request_text(tab_number: usize, profile: Option<&str>, shown: bool) -> String {
    let shown = if shown {
        "表示しているタブ"
    } else {
        "表示していないタブ"
    };
    let place = match profile {
        Some(profile) => format!("タブ {tab_number}（プロファイル「{profile}」、{shown}）"),
        None => format!("タブ {tab_number}（{shown}）"),
    };
    format!(
        "{place} のアプリが、クリップボードの内容を読み出そうとしています。\n\n許可すると、いまクリップボードにある文字列がそのアプリへ渡ります。"
    )
}

/// ダイアログのボタンの ID を答えにする。
fn choice_of_button(button: i32) -> Choice {
    match button {
        ID_ALLOW => Choice::Allow,
        ID_ALLOW_FOR_TAB => Choice::AllowForTab,
        _ => Choice::Deny,
    }
}

/// `owner` のウィンドウに確認のダイアログ（本文 `text`）を出し、答えを返す。閉じるまで戻らない。出せなければ
/// 拒否とする。
pub fn ask(owner: HWND, text: &str) -> Choice {
    let content = HSTRING::from(text);
    let buttons = [
        TASKDIALOG_BUTTON {
            nButtonID: ID_ALLOW,
            pszButtonText: w!("許可(&A)"),
        },
        TASKDIALOG_BUTTON {
            nButtonID: ID_DENY,
            pszButtonText: w!("拒否(&D)"),
        },
        TASKDIALOG_BUTTON {
            nButtonID: ID_ALLOW_FOR_TAB,
            pszButtonText: w!("このタブでは今後も許可(&T)"),
        },
    ];
    let config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW,
        pszWindowTitle: w!("fantermo"),
        Anonymous1: TASKDIALOGCONFIG_0 {
            pszMainIcon: TD_WARNING_ICON,
        },
        pszMainInstruction: w!("クリップボードの読み出しの依頼"),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: ID_DENY,
        pfCallback: Some(on_notification),
        ..Default::default()
    };
    let mut button = IDCANCEL.0;
    // SAFETY: config が指す文字列とボタンは、この呼び出しの間は生きている。owner は有効なウィンドウで、
    // 呼び出し元は借用を持っていない。
    let shown = unsafe { TaskDialogIndirect(&raw const config, Some(&raw mut button), None, None) };
    if let Err(e) = shown {
        tracing::warn!("failed to ask about a clipboard read: {e}");
        return Choice::Deny;
    }
    choice_of_button(button)
}

/// 開いているダイアログがあれば閉じさせる（拒否と同じに閉じる）。依頼を出したタブを閉じたときに使う。
/// 閉じるのを待たないので、借用を持ったまま呼んでよい。
pub fn close() {
    let Some(dialog) = OPEN_DIALOG.get() else {
        return;
    };
    // SAFETY: dialog は開いているダイアログで、閉じたら OPEN_DIALOG から外す。積むだけで、この場では処理されない。
    if let Err(e) = unsafe { PostMessageW(Some(dialog), WM_CLOSE, WPARAM(0), LPARAM(0)) } {
        tracing::warn!("failed to close the clipboard read dialog: {e}");
    }
}

/// ダイアログの知らせ。開いているダイアログのハンドルを覚える。
unsafe extern "system" fn on_notification(
    hwnd: HWND,
    notification: TASKDIALOG_NOTIFICATIONS,
    _wparam: WPARAM,
    _lparam: LPARAM,
    _data: isize,
) -> HRESULT {
    match notification {
        TDN_CREATED => OPEN_DIALOG.set(Some(hwnd)),
        TDN_DESTROYED => OPEN_DIALOG.set(None),
        _ => {}
    }
    S_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_names_the_tab_and_the_profile_and_says_it_is_a_read() {
        let text = request_text(2, Some("Cygwin"), false);
        assert!(
            text.starts_with("タブ 2（プロファイル「Cygwin」、表示していないタブ） のアプリが、"),
            "{text}"
        );
        assert!(text.contains("読み出そうとしています"), "{text}");
        let text = request_text(1, None, true);
        assert!(
            text.starts_with("タブ 1（表示しているタブ） のアプリが、"),
            "{text}"
        );
    }

    #[test]
    fn only_the_allowing_buttons_allow() {
        assert_eq!(choice_of_button(ID_ALLOW), Choice::Allow);
        assert_eq!(choice_of_button(ID_ALLOW_FOR_TAB), Choice::AllowForTab);
        assert_eq!(choice_of_button(ID_DENY), Choice::Deny);
        assert_eq!(choice_of_button(IDCANCEL.0), Choice::Deny);
    }
}
