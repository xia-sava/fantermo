//! fantermo 自身と、実行ファイルに含まれる第三者のソフトウェアのライセンスを見せるダイアログ。
//!
//! 文はリポジトリ直下の `LICENSE` と `THIRD-PARTY-NOTICES.txt`（`tools/licenses.sh` が作る）を
//! 実行ファイルに埋め込んだもの。

use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::IDCANCEL};

use crate::{
    font::Font,
    text_dialog::{self, Button, Dialog},
};

const TITLE: &str = "ライセンス";
const BUTTONS: [Button; 1] = [Button {
    id: IDCANCEL.0,
    label: "閉じる",
    default: true,
}];
/// `THIRD-PARTY-NOTICES.txt` の区切りの線の幅。これが折り返さずに収まる幅で開く。
const TEXT_COLUMNS: u32 = 80;

/// 見せる文。fantermo 自身のライセンスのあとに、第三者のソフトウェアのライセンスを続ける。
const TEXT: &str = concat!(
    "fantermo のライセンス\n",
    "\n",
    include_str!("../../../LICENSE"),
    "\n",
    "================================================================================\n",
    "\n",
    include_str!("../../../THIRD-PARTY-NOTICES.txt"),
);

/// ライセンスの文を読み取り専用のテキストボックスで見せ、閉じるまで待つ。
pub fn show(owner: HWND, font: &Font) {
    let dialog = Dialog {
        title: TITLE,
        text: TEXT,
        read_only: true,
        min_columns: Some(TEXT_COLUMNS),
        buttons: &BUTTONS,
    };
    text_dialog::run(owner, &dialog, font);
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
}
