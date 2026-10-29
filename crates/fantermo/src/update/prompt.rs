//! 更新の確認とダウンロードの結果を、メッセージボックスで見せて次にすることを決める。

use windows::{
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            IDOK, IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_ICONWARNING, MB_OK,
            MB_OKCANCEL, MB_YESNO, MESSAGEBOX_RESULT, MESSAGEBOX_STYLE, MessageBoxW,
        },
    },
    core::HSTRING,
};

use super::{Comparison, Downloaded, Event, Release, current_version, wait_minutes};
use crate::logging;

/// メッセージボックスのタイトル。
const TITLE: &str = "fantermo の更新";

/// 結果を見せたあとにすること。
pub enum Next {
    /// 何もしない。更新の仕事は終わった。
    Nothing,
    /// 新しいバージョンをダウンロードする。
    Download(Release),
    /// 更新を適用するスクリプトに引き渡したので、fantermo を閉じる。
    Quit,
}

/// 別のスレッドの結果 `event` を、ウィンドウ `owner` の上のメッセージボックスで見せる。
pub fn on_event(owner: HWND, event: Event) -> Next {
    match event {
        Event::Checked(Ok(comparison)) => on_checked(owner, comparison),
        Event::Checked(Err(e)) => {
            tracing::warn!("{e}");
            show(
                owner,
                &format!("更新を確認できませんでした。\n\n{}", e.message()),
                MB_OK | MB_ICONWARNING,
            );
            Next::Nothing
        }
        Event::Downloaded(Ok(downloaded)) => on_downloaded(owner, downloaded),
        Event::Downloaded(Err(e)) => {
            tracing::warn!("{e}");
            show(
                owner,
                &format!("更新できませんでした。\n\n{}", e.message()),
                MB_OK | MB_ICONWARNING,
            );
            Next::Nothing
        }
    }
}

fn on_checked(owner: HWND, comparison: Comparison) -> Next {
    let current = current_version().map_or_else(String::new, |version| version.to_string());
    match comparison {
        Comparison::UpToDate { latest } => {
            show(
                owner,
                &format!(
                    "fantermo は最新バージョンです。\n\nいまのバージョン: {current}\n配布しているバージョン: {latest}"
                ),
                MB_OK | MB_ICONINFORMATION,
            );
            Next::Nothing
        }
        Comparison::Newer(release) => {
            let text = format!(
                "新しいバージョン {} があります（いまのバージョンは {current}）。\n\nダウンロードして更新しますか？",
                release.version
            );
            if show(owner, &text, MB_YESNO | MB_ICONQUESTION) == IDYES {
                Next::Download(release)
            } else {
                Next::Nothing
            }
        }
    }
}

/// ほかの fantermo が動いていれば知らせ、全部のタブを閉じることを確かめてから引き渡す。取りやめたら
/// ダウンロードした MSI を消す。
fn on_downloaded(owner: HWND, downloaded: Downloaded) -> Next {
    let others = downloaded.other_instances();
    if !others.is_empty() {
        let text = format!(
            "ほかに fantermo が {} 個動いています。\n\n更新は、それらがすべて閉じてから入れます\
             （{} 分たっても閉じなければ、待たずに入れます）。続けますか？",
            others.len(),
            wait_minutes()
        );
        if show(owner, &text, MB_YESNO | MB_ICONWARNING) != IDYES {
            return Next::Nothing;
        }
    }
    let text = format!(
        "全部のタブを閉じて、バージョン {} に更新します。\n\n更新を入れ終えたら、fantermo を起動し直します。",
        downloaded.version
    );
    if show(owner, &text, MB_OKCANCEL | MB_ICONINFORMATION) != IDOK {
        return Next::Nothing;
    }
    match downloaded.hand_off(&others) {
        Ok(()) => Next::Quit,
        Err(e) => {
            tracing::warn!("failed to hand the update off: {e}");
            let mut text = "更新を始められませんでした。".to_owned();
            if let Some(dir) = logging::log_dir() {
                text.push_str(&format!("\n\nログ: {}", dir.display()));
            }
            show(owner, &text, MB_OK | MB_ICONERROR);
            Next::Nothing
        }
    }
}

fn show(owner: HWND, text: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    // SAFETY: owner は有効なウィンドウで、文字列はこの呼び出しの間は生きている。ウィンドウの借用は
    // 持っていない。
    unsafe {
        MessageBoxW(
            Some(owner),
            &HSTRING::from(text),
            &HSTRING::from(TITLE),
            style,
        )
    }
}
