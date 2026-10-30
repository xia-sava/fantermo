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

use super::{CheckError, Comparison, Downloaded, Event, Release, current_version, wait_minutes};
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

impl Next {
    /// 更新の流れがこの先も続くか（ダウンロードに進むか、fantermo を閉じるか）。
    pub fn continues(&self) -> bool {
        !matches!(self, Self::Nothing)
    }
}

/// メッセージボックスに出す文と、ボタンとアイコン。
#[derive(Debug, PartialEq, Eq)]
struct Prompt {
    text: String,
    style: MESSAGEBOX_STYLE,
}

/// 別のスレッドの結果 `event` を、ウィンドウ `owner` の上のモーダルのメッセージボックスで見せる。
pub fn on_event(owner: HWND, event: Event) -> Next {
    match event {
        Event::Checked(Ok(comparison)) => on_checked(owner, comparison),
        Event::Checked(Err(e)) => {
            tracing::warn!("{e}");
            let prompt = check_failed_prompt(&e);
            show(owner, &prompt.text, prompt.style);
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
    let prompt = checked_prompt(&comparison, &current);
    let answer = show(owner, &prompt.text, prompt.style);
    match comparison {
        Comparison::Newer(release) if answer == IDYES => Next::Download(release),
        Comparison::Newer(_) | Comparison::UpToDate { .. } => Next::Nothing,
    }
}

/// 確かめた結果 `comparison` を、いまのバージョン `current` とあわせて見せる文。新しいバージョンがあれば、
/// 更新するかを「はい」「いいえ」で尋ねる。
fn checked_prompt(comparison: &Comparison, current: &str) -> Prompt {
    match comparison {
        Comparison::UpToDate { latest } => Prompt {
            text: format!(
                "fantermo は最新バージョンです。\n\nいまのバージョン: {current}\n配布しているバージョン: {latest}"
            ),
            style: MB_OK | MB_ICONINFORMATION,
        },
        Comparison::Newer(release) => Prompt {
            text: format!(
                "新しいバージョン {} があります（いまのバージョンは {current}）。\n\nダウンロードして更新しますか？",
                release.version
            ),
            style: MB_YESNO | MB_ICONQUESTION,
        },
    }
}

/// 確かめられなかったわけ `error` を見せる文。
fn check_failed_prompt(error: &CheckError) -> Prompt {
    Prompt {
        text: format!("更新を確認できませんでした。\n\n{}", error.message()),
        style: MB_OK | MB_ICONWARNING,
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

#[cfg(test)]
mod tests {
    use super::{super::http::HttpError, super::signature::SHA256_LEN, *};
    use crate::update::Version;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn up_to_date_is_told_with_ok_only() {
        let prompt = checked_prompt(
            &Comparison::UpToDate {
                latest: version("0.0.1"),
            },
            "0.0.1",
        );
        assert_eq!(
            prompt,
            Prompt {
                text: "fantermo は最新バージョンです。\n\nいまのバージョン: 0.0.1\n配布しているバージョン: 0.0.1"
                    .to_owned(),
                style: MB_OK | MB_ICONINFORMATION,
            }
        );
    }

    #[test]
    fn newer_version_asks_whether_to_update() {
        let release = Release {
            version: version("0.0.2"),
            msi_sha256: [0; SHA256_LEN],
        };
        let prompt = checked_prompt(&Comparison::Newer(release), "0.0.1");
        assert_eq!(
            prompt,
            Prompt {
                text: "新しいバージョン 0.0.2 があります（いまのバージョンは 0.0.1）。\n\nダウンロードして更新しますか？"
                    .to_owned(),
                style: MB_YESNO | MB_ICONQUESTION,
            }
        );
    }

    #[test]
    fn failed_check_is_told_with_ok_only() {
        let prompt = check_failed_prompt(&CheckError::Fetch(HttpError::Status(404)));
        assert_eq!(
            prompt,
            Prompt {
                text:
                    "更新を確認できませんでした。\n\n更新の情報を取得できませんでした（HTTP 404）。"
                        .to_owned(),
                style: MB_OK | MB_ICONWARNING,
            }
        );
    }

    #[test]
    fn only_download_and_quit_continue_the_update() {
        let release = Release {
            version: version("0.0.2"),
            msi_sha256: [0; SHA256_LEN],
        };
        assert!(!Next::Nothing.continues());
        assert!(Next::Download(release).continues());
        assert!(Next::Quit.continues());
    }
}
