//! 改行を含む文字列を貼り付ける前に、内容を見せて編集させるダイアログ。
//!
//! 「貼り付け」を押せば編集した文字列を返し、「キャンセル」や Esc なら何も返さない。

use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{IDCANCEL, IDOK},
};

use crate::{
    font::Font,
    text_dialog::{self, Button, Dialog},
};

const TITLE: &str = "貼り付ける内容の確認";
const BUTTONS: [Button; 2] = [
    Button {
        id: IDOK.0,
        label: "貼り付け(&P)",
        default: true,
        enabled: true,
    },
    Button {
        id: IDCANCEL.0,
        label: "キャンセル",
        default: false,
        enabled: true,
    },
];

/// 編集できるように `text` を見せ、「貼り付け」なら編集後の文字列を返す。改行は `\n` で受け渡す。
pub fn edit(owner: HWND, text: &str, font: &Font) -> Option<String> {
    let dialog = Dialog {
        title: TITLE,
        header: &[],
        text,
        read_only: false,
        min_columns: None,
        buttons: &BUTTONS,
    };
    text_dialog::run(owner, &dialog, font)
}
