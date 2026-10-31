//! 改行を含む文字列や大きな文字列を貼り付ける前に、内容を見せて編集させるダイアログ。
//!
//! 「貼り付け」を押せば編集した文字列を返し、「キャンセル」や Esc なら何も返さない。

use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{IDCANCEL, IDOK},
};

use crate::{
    font::Font,
    text_dialog::{self, Button, Closed, Dialog, HeaderLine},
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

/// 大きさの表示の単位（1 KB = 1024 バイト）。
const BYTES_PER_KB: u64 = 1024;
const BYTES_PER_MB: u64 = BYTES_PER_KB * BYTES_PER_KB;

/// 編集できるように `text` を見せ、「貼り付け」なら編集後の文字列を返す。改行は `\n` で受け渡す。
/// `large` なら、大きなテキストであることを入力欄の上に添える。
pub fn edit(owner: HWND, text: &str, font: &Font, large: bool) -> Option<String> {
    let notice = large.then(|| large_paste_notice(text.len()));
    let header = notice.as_deref().map(HeaderLine::Text);
    let dialog = Dialog {
        title: TITLE,
        header: header.as_slice(),
        text,
        read_only: false,
        min_columns: None,
        buttons: &BUTTONS,
        on_button: None,
        on_owner_message: None,
    };
    match text_dialog::run(owner, &dialog, font) {
        Closed::Ok(text) => Some(text),
        Closed::Button(_) | Closed::Cancel => None,
    }
}

/// `bytes` バイトの文字列を貼り付けるときに添える文。
fn large_paste_notice(bytes: usize) -> String {
    format!(
        "大きなテキストを貼り付けようとしています（{}）。",
        format_size(bytes)
    )
}

/// `bytes` バイトを読みやすく書く。1 MB 未満は整数の KB（1 KB 未満も 1 KB）、それより大きければ小数第 1 位までの
/// MB（小数部が 0 なら書かない）。どちらも四捨五入する。
fn format_size(bytes: usize) -> String {
    let bytes = bytes as u64;
    let kilobytes = (bytes + BYTES_PER_KB / 2) / BYTES_PER_KB;
    if kilobytes < BYTES_PER_KB {
        return format!("{} KB", kilobytes.max(1));
    }
    let tenths = (bytes * 10 + BYTES_PER_MB / 2) / BYTES_PER_MB;
    match tenths % 10 {
        0 => format!("{} MB", tenths / 10),
        tenth => format!("{}.{tenth} MB", tenths / 10),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KB: usize = 1024;
    const MB: usize = 1024 * 1024;

    #[test]
    fn sizes_are_written_in_kb_and_mb() {
        assert_eq!(format_size(1), "1 KB");
        assert_eq!(format_size(KB), "1 KB");
        assert_eq!(format_size(8 * KB), "8 KB");
        assert_eq!(format_size(8 * KB + KB / 2), "9 KB");
        assert_eq!(format_size(1023 * KB), "1023 KB");
        // 四捨五入で 1024 KB になるものは MB で書く。
        assert_eq!(format_size(MB - 1), "1 MB");
        assert_eq!(format_size(MB), "1 MB");
        assert_eq!(format_size(MB + MB / 5), "1.2 MB");
        assert_eq!(format_size(10 * MB), "10 MB");
        assert_eq!(format_size(1536 * MB), "1536 MB");
    }

    #[test]
    fn large_paste_notice_shows_the_size() {
        assert_eq!(
            large_paste_notice(MB + MB / 5),
            "大きなテキストを貼り付けようとしています（1.2 MB）。"
        );
    }
}
