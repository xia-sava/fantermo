//! Unicode の既定の表示（絵文字として表示するか、文字として表示するか）を、書記素クラスタごとに決める。
//! Unicode の性質は unicode-properties、書記素クラスタの区切りは unicode-segmentation の表を使う。

use unicode_properties::{
    EmojiStatus, UnicodeEmoji,
    emoji::{is_emoji_presentation_selector, is_text_presentation_selector},
};
use unicode_segmentation::UnicodeSegmentation;

/// 書記素クラスタ `cluster` を絵文字として表示するか。
///
/// `U+FE0F`（絵文字の表示を求める異体字セレクタ）を含めば絵文字、`U+FE0E`（文字の表示を求めるもの）を
/// 含めば文字として表示する。両方を含めば、後に現れたほうに従う。どちらも含まなければ、
/// Emoji_Presentation が Yes の文字（地域指示記号を含む）を含むときだけ絵文字として表示する。
pub fn is_emoji_presentation(cluster: &str) -> bool {
    let selected = cluster.chars().rev().find_map(|c| {
        if is_emoji_presentation_selector(c) {
            Some(true)
        } else if is_text_presentation_selector(c) {
            Some(false)
        } else {
            None
        }
    });
    selected.unwrap_or_else(|| cluster.chars().any(has_emoji_presentation))
}

/// `c` の Emoji_Presentation が Yes か。
fn has_emoji_presentation(c: char) -> bool {
    matches!(
        c.emoji_status(),
        EmojiStatus::EmojiPresentation
            | EmojiStatus::EmojiPresentationAndModifierBase
            | EmojiStatus::EmojiPresentationAndEmojiComponent
            | EmojiStatus::EmojiPresentationAndModifierAndEmojiComponent
    )
}

/// 異体字セレクタの `U+FE0E` か `U+FE0F` か。
pub fn is_presentation_selector(c: char) -> bool {
    is_emoji_presentation_selector(c) || is_text_presentation_selector(c)
}

/// `text` の拡張書記素クラスタを、UTF-16 での位置と長さと一緒に先頭から返す。
pub fn utf16_clusters(text: &str) -> impl Iterator<Item = (u32, u32, &str)> {
    text.graphemes(true).scan(0u32, |position, cluster| {
        let start = *position;
        let length = cluster.encode_utf16().count() as u32;
        *position += length;
        Some((start, length, cluster))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_presentation_follows_the_selectors_and_the_default() {
        let emoji = [
            "\u{1F600}",                                   // 😀
            "\u{2733}\u{FE0F}",                            // ✳️
            "\u{263A}\u{FE0F}",                            // ☺️
            "\u{1F1EF}\u{1F1F5}",                          // 🇯🇵
            "\u{1F44D}\u{1F3FD}",                          // 👍🏽
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", // 👨‍👩‍👧
            "#\u{FE0F}\u{20E3}",                           // #️⃣
            "\u{2705}",                                    // ✅
        ];
        for cluster in emoji {
            assert!(is_emoji_presentation(cluster), "{cluster:?}");
        }
        let text = [
            "\u{2733}",          // ✳
            "\u{263A}",          // ☺
            "\u{1F600}\u{FE0E}", // 😀︎
            "#",
            "A",
            "\u{3042}", // あ
            "\u{2714}", // ✔
        ];
        for cluster in text {
            assert!(!is_emoji_presentation(cluster), "{cluster:?}");
        }
    }

    #[test]
    fn the_later_selector_wins() {
        assert!(is_emoji_presentation("\u{2733}\u{FE0E}\u{FE0F}"));
        assert!(!is_emoji_presentation("\u{2733}\u{FE0F}\u{FE0E}"));
        assert!(!is_emoji_presentation("\u{1F600}\u{FE0F}\u{FE0E}"));
    }

    #[test]
    fn clusters_are_given_with_their_utf16_ranges() {
        let clusters: Vec<_> = utf16_clusters("a\u{2733}\u{FE0F}\u{1F1EF}\u{1F1F5}b").collect();
        assert_eq!(
            clusters,
            [
                (0, 1, "a"),
                (1, 2, "\u{2733}\u{FE0F}"),
                (3, 4, "\u{1F1EF}\u{1F1F5}"),
                (7, 1, "b"),
            ]
        );
        assert_eq!(utf16_clusters("").count(), 0);
    }
}
