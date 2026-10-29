//! 「fantermo について」のダイアログ。名前、バージョン、コミット、著作権、GitHub のリンクを並べ、
//! その下の読み取り専用の入力欄に、fantermo 自身と実行ファイルに含まれる第三者のソフトウェアの
//! ライセンスを見せる。
//!
//! ライセンスの文はリポジトリ直下の `LICENSE` と `THIRD-PARTY-NOTICES.txt`（`tools/licenses.sh` が作る）を
//! 実行ファイルに埋め込んだもの。

use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::IDCANCEL};

use crate::{
    font::Font,
    text_dialog::{self, Button, Closed, Dialog, HeaderLine},
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

/// 「fantermo について」のダイアログを開き、閉じるまで待つ。「更新を確認」を押して閉じたら真を返す。
///
/// 「更新を確認」は、インストーラで入れたバージョン（[`update::can_update`]）でだけ押せる。
pub fn show(owner: HWND, font: &Font) -> bool {
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
            enabled: can_update,
        },
        Button {
            id: IDCANCEL.0,
            label: "閉じる",
            default: true,
            enabled: true,
        },
    ];
    let dialog = Dialog {
        title: TITLE,
        header: &header,
        text: TEXT,
        read_only: true,
        min_columns: Some(TEXT_COLUMNS),
        buttons: &buttons,
    };
    text_dialog::run(owner, &dialog, font) == Closed::Button(CHECK_UPDATE_ID)
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
    fn copyright_is_the_line_of_the_license() {
        assert_eq!(copyright(), "Copyright (c) 2026 ISIGE, Akira");
    }
}
