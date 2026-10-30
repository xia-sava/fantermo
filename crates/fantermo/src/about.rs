//! 「fantermo について」のダイアログ。名前、バージョン、コミット、著作権、GitHub のリンクを並べ、
//! その下の読み取り専用の入力欄に、fantermo 自身と実行ファイルに含まれる第三者のソフトウェアの
//! ライセンスを見せる。
//!
//! ライセンスの文はリポジトリ直下の `LICENSE` と `THIRD-PARTY-NOTICES.txt`（`tools/licenses.sh` が作る）を
//! 実行ファイルに埋め込んだもの。

use windows::Win32::{
    Foundation::HWND,
    UI::{
        Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus},
        WindowsAndMessaging::{GetDlgItem, IDCANCEL, MSG},
    },
};

use crate::{
    font::Font,
    text_dialog::{self, Button, Dialog, HeaderLine},
    update, version,
};

const TITLE: &str = "fantermo について";
/// 名前。
const NAME: &str = "fantermo";
/// GitHub のリポジトリ。
const REPOSITORY_URL: &str = "https://github.com/xia-sava/fantermo";
/// 「更新を確認」のボタンのコントロール ID。
const CHECK_UPDATE_ID: i32 = 101;
/// 「更新を確認」を押せないときに添える行。
const UPDATE_UNAVAILABLE: &str = "自動更新はインストーラで入れたバージョンでのみ可能です";
/// `THIRD-PARTY-NOTICES.txt` の区切りの線の幅。これが折り返さずに収まる幅で開く。
const TEXT_COLUMNS: u32 = 80;

/// fantermo 自身のライセンス。
const LICENSE: &str = include_str!("../../../LICENSE");

/// 入力欄に見せる文。fantermo 自身のライセンスのあとに、第三者のソフトウェアのライセンスを続ける。
const TEXT: &str = concat!(
    "fantermo のライセンス\n",
    "\n",
    include_str!("../../../LICENSE"),
    "\n",
    "================================================================================\n",
    "\n",
    include_str!("../../../THIRD-PARTY-NOTICES.txt"),
);

/// 「fantermo について」を開いている間の、更新の確認とのやりとり。
pub struct UpdateHooks<'a> {
    /// 開くときに、更新の確認かダウンロードを別のスレッドでしている。
    pub busy: bool,
    /// 更新の確認を別のスレッドで始める。始められたら真。
    pub start_check: &'a dyn Fn() -> bool,
    /// 呼び出し元のウィンドウ宛てのメッセージを受け取る。更新のスレッドの知らせなら、その結果をダイアログの
    /// 上に見せる。
    pub on_owner_message: &'a dyn Fn(HWND, &MSG) -> Received,
}

/// 呼び出し元のウィンドウ宛てのメッセージを受け取ったあと、ダイアログをどうするか。
pub enum Received {
    /// 更新のスレッドの知らせではない。ほかに渡す。
    Other,
    /// 結果を見せ終えた。ダイアログは開いたままにし、「更新を確認」を押せるように戻す。
    Shown,
    /// 結果を見せ、更新に進むので、ダイアログを閉じる。
    Close,
}

/// 「fantermo について」のダイアログを開き、閉じるまで待つ。
///
/// 「更新を確認」は、インストーラで入れたバージョン（[`update::can_update`]）でだけ押せる。押してもダイアログは
/// 閉じず、確かめている間は押せなくする。結果は `hooks` がダイアログの上に見せる。
pub fn show(owner: HWND, font: &Font, hooks: &UpdateHooks) {
    let can_update = update::can_update();
    let version = format!("バージョン: {}", version::display());
    let commit = format!("コミット: {}", version::COMMIT);
    let mut header = vec![
        HeaderLine::Text(NAME),
        HeaderLine::Text(&version),
        HeaderLine::Text(&commit),
        HeaderLine::Text(copyright()),
        HeaderLine::Link(REPOSITORY_URL),
    ];
    if !can_update {
        header.push(HeaderLine::Text(UPDATE_UNAVAILABLE));
    }
    let buttons = [
        Button {
            id: CHECK_UPDATE_ID,
            label: "更新を確認(&U)",
            default: false,
            enabled: check_enabled(can_update, hooks.busy),
        },
        Button {
            id: IDCANCEL.0,
            label: "閉じる",
            default: true,
            enabled: true,
        },
    ];
    let on_button = |dialog: HWND, id: i32| {
        if id != CHECK_UPDATE_ID {
            return false;
        }
        set_check_enabled(dialog, false);
        if !(hooks.start_check)() {
            set_check_enabled(dialog, check_enabled(can_update, false));
        }
        true
    };
    let on_owner_message = |dialog: HWND, msg: &MSG| match (hooks.on_owner_message)(dialog, msg) {
        Received::Other => false,
        Received::Shown => {
            set_check_enabled(dialog, check_enabled(can_update, false));
            true
        }
        Received::Close => {
            text_dialog::close(dialog);
            true
        }
    };
    let dialog = Dialog {
        title: TITLE,
        header: &header,
        text: TEXT,
        read_only: true,
        min_columns: Some(TEXT_COLUMNS),
        buttons: &buttons,
        on_button: Some(&on_button),
        on_owner_message: Some(&on_owner_message),
    };
    text_dialog::run(owner, &dialog, font);
}

/// 「更新を確認」を押せるか。インストーラで入れたバージョン（`can_update`）で、更新の確認かダウンロードを
/// していない（`busy` が偽）ときだけ押せる。
fn check_enabled(can_update: bool, busy: bool) -> bool {
    can_update && !busy
}

/// ダイアログ `dialog` の「更新を確認」を押せるか押せないかにする。押せなくするボタンに入力のフォーカスが
/// あれば、「閉じる」に移す。
fn set_check_enabled(dialog: HWND, enabled: bool) {
    // SAFETY: dialog は開いているダイアログで、ボタンはその子。
    unsafe {
        let Ok(button) = GetDlgItem(Some(dialog), CHECK_UPDATE_ID) else {
            tracing::warn!("the update button is missing");
            return;
        };
        if !enabled
            && GetFocus() == button
            && let Ok(close) = GetDlgItem(Some(dialog), IDCANCEL.0)
        {
            _ = SetFocus(Some(close));
        }
        _ = EnableWindow(button, enabled);
    }
}

/// `LICENSE` の著作権の行。
fn copyright() -> &'static str {
    LICENSE
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("Copyright"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_has_own_and_third_party_licenses() {
        let own = TEXT.find("Copyright (c) 2026 ISIGE, Akira").unwrap();
        let notices = TEXT
            .find("fantermo が使っている第三者のソフトウェアと、そのライセンス")
            .unwrap();
        assert!(own < notices);
        assert!(TEXT.contains("Ghostty (MIT)"));
    }

    #[test]
    fn update_button_is_enabled_only_for_installed_builds_while_idle() {
        assert!(check_enabled(true, false));
        assert!(!check_enabled(true, true));
        assert!(!check_enabled(false, false));
        assert!(!check_enabled(false, true));
    }

    #[test]
    fn copyright_is_the_line_of_the_license() {
        assert_eq!(copyright(), "Copyright (c) 2026 ISIGE, Akira");
    }
}
