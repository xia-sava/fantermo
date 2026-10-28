//! 画面のリンク。OSC 8 のハイパーリンクと、画面の文字から見つけた URL を扱う。
//!
//! 文字から URL を見つける処理、リンクの範囲をセルの並びに直す処理、開いてよいリンクかを決める処理を
//! 置く。端末の状態からセルを取り出すのは `vt`、実際に開くのは fantermo の側。

use std::ops::Range;

/// 画面の文字から見つける URL の始まり。小文字で比べる。
const URL_PREFIXES: [&str; 4] = ["http://", "https://", "file://", "mailto:"];

/// URL の終わりに付いていても、URL に含めない文字。文の句読点や、URL を囲む引用符。
const TRAILING_PUNCTUATION: &[char] = &['.', ',', ':', ';', '!', '?', '\'', '"', '*'];

/// URL の終わりに付いていて、対になる開き括弧が URL の中になければ含めない閉じ括弧。
const CLOSING_BRACKETS: [(char, char); 3] = [(')', '('), (']', '['), ('}', '{')];

/// 開いてよい scheme。小文字で比べる。
const ALLOWED_SCHEMES: [&str; 4] = ["http", "https", "mailto", "file"];

/// 表示している範囲の一行の中で、リンクが占める桁の範囲。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSpan {
    pub row: u16,
    pub cols: Range<u16>,
}

/// 画面のリンク一つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub uri: String,
    /// 表示している範囲に入っている部分。折り返しをまたぐリンクは、行ごとに一つずつ持つ。
    pub spans: Vec<LinkSpan>,
}

/// 折り返しでつながった一続きの行（論理行）の、セル一つ。全角の文字の右半分は含めない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LineCell {
    /// スクロールバックの先頭から数えた行。
    pub screen_row: u32,
    pub col: u16,
    pub width: u8,
    /// セルの文字（書記素クラスタ）。空のセルは空。
    pub text: String,
    /// OSC 8 のハイパーリンクの URI。
    pub uri: Option<String>,
}

/// 論理行の `index` 番目のセルにあるリンクの、URI とセルの範囲。OSC 8 のハイパーリンクがあれば
/// そちらを、なければ文字から見つけた URL を返す。
pub(crate) fn link_in_line(cells: &[LineCell], index: usize) -> Option<(String, Range<usize>)> {
    let cell = cells.get(index)?;
    if let Some(uri) = &cell.uri {
        let same = |other: &LineCell| other.uri.as_ref() == Some(uri);
        let start = cells[..index]
            .iter()
            .rposition(|other| !same(other))
            .map_or(0, |before| before + 1);
        let end = cells[index..]
            .iter()
            .position(|other| !same(other))
            .map_or(cells.len(), |after| index + after);
        return Some((uri.clone(), start..end));
    }
    let texts: Vec<&str> = cells.iter().map(|cell| cell.text.as_str()).collect();
    let range = find_url(&texts, index)?;
    let url = texts[range.clone()].concat();
    Some((url, range))
}

/// 論理行のセルの範囲を、表示している範囲の行ごとの桁の範囲に直す。`viewport_top` は表示している
/// 範囲の先頭の行（スクロールバックの先頭から数えた行）で、`rows` はその行数。範囲の外の行は除く。
pub(crate) fn spans_of(
    cells: &[LineCell],
    range: Range<usize>,
    viewport_top: u32,
    rows: u16,
) -> Vec<LinkSpan> {
    let mut spans: Vec<LinkSpan> = Vec::new();
    for cell in &cells[range] {
        let Some(row) = cell
            .screen_row
            .checked_sub(viewport_top)
            .filter(|&row| row < u32::from(rows))
        else {
            continue;
        };
        let row = row as u16;
        let end = cell.col + u16::from(cell.width);
        match spans.last_mut() {
            Some(span) if span.row == row => span.cols.end = end,
            _ => spans.push(LinkSpan {
                row,
                cols: cell.col..end,
            }),
        }
    }
    spans
}

/// セルの文字を並べた一行から、`index` 番目のセルを含む URL を探し、そのセルの範囲を返す。
/// 一つのセルが ASCII の一文字でなければ、そこで URL が切れる（全角の文字が続く日本語の文など）。
pub fn find_url(cells: &[&str], index: usize) -> Option<Range<usize>> {
    let chars: Vec<char> = cells.iter().map(|text| ascii_char(text)).collect();
    let mut start = 0;
    while start < chars.len() {
        let Some(prefix) = url_prefix_at(&chars, start) else {
            start += 1;
            continue;
        };
        let end = url_end(&chars, start, prefix.len());
        if end <= start + prefix.len() {
            start += prefix.len();
            continue;
        }
        if (start..end).contains(&index) {
            return Some(start..end);
        }
        start = end;
    }
    None
}

/// セルの文字が ASCII の一文字ならその文字、そうでなければ URL に入らない印（`'\0'`）。
fn ascii_char(text: &str) -> char {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii() => c,
        _ => '\0',
    }
}

/// `start` から URL が始まっていれば、その始まりの文字列。前の文字が scheme の一部になりうる
/// 文字（英数字など）なら、始まりとはみなさない。
fn url_prefix_at(chars: &[char], start: usize) -> Option<&'static str> {
    if start > 0 && is_scheme_char(chars[start - 1]) {
        return None;
    }
    URL_PREFIXES.into_iter().find(|prefix| {
        let candidate = &chars[start..];
        candidate.len() >= prefix.len()
            && prefix
                .chars()
                .zip(candidate)
                .all(|(expected, actual)| actual.to_ascii_lowercase() == expected)
    })
}

/// `start` から始まる URL の終わり（含まない）。URL に使える文字が続くところまで進め、終わりの
/// 句読点と、対になっていない閉じ括弧を除く。
fn url_end(chars: &[char], start: usize, prefix_len: usize) -> usize {
    let mut end = start + prefix_len;
    while end < chars.len() && is_url_char(chars[end]) {
        end += 1;
    }
    let body = start + prefix_len;
    while end > body {
        let last = chars[end - 1];
        let unbalanced_bracket = CLOSING_BRACKETS
            .iter()
            .find(|(close, _)| *close == last)
            .is_some_and(|&(close, open)| {
                let url = &chars[start..end];
                url.iter().filter(|&&c| c == close).count()
                    > url.iter().filter(|&&c| c == open).count()
            });
        if !TRAILING_PUNCTUATION.contains(&last) && !unbalanced_bracket {
            break;
        }
        end -= 1;
    }
    end
}

fn is_scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')
}

/// URL に使える文字（RFC 3986 の unreserved、reserved と `%`）。
fn is_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(c)
}

/// リンクを開く先。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenTarget {
    /// 既定のアプリ（ブラウザ、メールのアプリ）に渡す URI。ASCII だけで、それ以外の文字は
    /// パーセントエンコードしてある。
    Uri(String),
    /// `file:` の URI が指す、Windows のパス。開くのではなく、エクスプローラーで見せる。
    Path(String),
}

/// リンクを開かない理由。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefusedLink {
    /// 開いてよい scheme ではない。
    Scheme(String),
    /// scheme がない。
    NoScheme,
    /// 空白、制御文字、引用符など、URI に使えない文字を含む。
    InvalidCharacter,
    /// scheme のあとが空か、`http` で接続先がない。
    Empty,
    /// `file:` の URI が、ドライブ名も接続先もないパス（Unix のパスなど）を指す。
    NotWindowsPath,
}

impl std::fmt::Display for RefusedLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scheme(scheme) => write!(f, "the scheme {scheme:?} is not allowed"),
            Self::NoScheme => write!(f, "no scheme"),
            Self::InvalidCharacter => write!(f, "invalid character"),
            Self::Empty => write!(f, "nothing after the scheme"),
            Self::NotWindowsPath => write!(f, "not a Windows path"),
        }
    }
}

/// `uri` を開いてよければ、開く先を返す。開くのは `http`、`https`、`mailto`、`file` だけ。
/// URI はアプリが出力したもの（外の入力）なので、任意のコマンドやファイルを開かせない。
pub fn open_target(uri: &str) -> Result<OpenTarget, RefusedLink> {
    if uri
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || "\"<>\\^`{|}".contains(c))
    {
        return Err(RefusedLink::InvalidCharacter);
    }
    let (scheme, rest) = uri.split_once(':').ok_or(RefusedLink::NoScheme)?;
    let scheme = scheme.to_ascii_lowercase();
    if !ALLOWED_SCHEMES.contains(&scheme.as_str()) {
        let shown = if scheme.chars().all(is_scheme_char) {
            scheme
        } else {
            String::new()
        };
        return Err(RefusedLink::Scheme(shown));
    }
    match scheme.as_str() {
        "file" => file_path(rest).map(OpenTarget::Path),
        "mailto" if rest.is_empty() => Err(RefusedLink::Empty),
        "http" | "https" if !has_host(rest) => Err(RefusedLink::Empty),
        _ => Ok(OpenTarget::Uri(percent_encode_non_ascii(&format!(
            "{scheme}:{rest}"
        )))),
    }
}

/// `http:` と `https:` の scheme のあとが、`//` と空でない接続先で始まる。
fn has_host(rest: &str) -> bool {
    rest.strip_prefix("//")
        .and_then(|authority| authority.split(['/', '?', '#']).next())
        .is_some_and(|host| !host.is_empty())
}

/// ASCII でない文字を、UTF-8 のバイトごとにパーセントエンコードする。
fn percent_encode_non_ascii(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            encoded.push(c);
            continue;
        }
        let mut buf = [0u8; 4];
        for byte in c.encode_utf8(&mut buf).bytes() {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `file:` の scheme のあと（`//接続先/パス` か `/パス`）を、Windows のパスにする。
/// 接続先が空か `localhost` ならドライブ名で始まるパス、それ以外なら UNC のパス。
fn file_path(rest: &str) -> Result<String, RefusedLink> {
    let (host, path) = match rest.strip_prefix("//") {
        Some(authority_and_path) => match authority_and_path.find('/') {
            Some(slash) => authority_and_path.split_at(slash),
            None => (authority_and_path, ""),
        },
        None => ("", rest),
    };
    let path = percent_decode(path).ok_or(RefusedLink::InvalidCharacter)?;
    if path.chars().any(|c| c.is_control() || c == '"') {
        return Err(RefusedLink::InvalidCharacter);
    }
    let host = percent_decode(host).ok_or(RefusedLink::InvalidCharacter)?;
    // `file://C:/x` のように、ドライブ名を接続先の位置に書くものもある。
    if starts_with_drive(&host) {
        return Ok(format!("{host}{path}").replace('/', "\\"));
    }
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        let local = path.strip_prefix('/').unwrap_or(&path);
        if !starts_with_drive(local) {
            return Err(RefusedLink::NotWindowsPath);
        }
        return Ok(local.replace('/', "\\"));
    }
    if host.chars().any(|c| c.is_control() || c == '"') {
        return Err(RefusedLink::InvalidCharacter);
    }
    if path.trim_matches('/').is_empty() {
        return Err(RefusedLink::Empty);
    }
    Ok(format!("\\\\{host}{}", path.replace('/', "\\")))
}

/// `C:` のようなドライブ名で始まり、そのあとが空か区切りの文字。
fn starts_with_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes.get(2).is_none_or(|&c| c == b'/' || c == b'\\')
}

/// パーセントエンコードを戻す。`%` のあとが 16 進の 2 桁でないか、戻したものが UTF-8 でなければ `None`。
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            decoded.push(bytes[i]);
            i += 1;
            continue;
        }
        let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
        decoded.push(u8::from_str_radix(hex, 16).ok()?);
        i += 3;
    }
    String::from_utf8(decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `line` を 1 文字 1 セルにして、`index` 番目の文字を含む URL を返す。
    fn url_at(line: &str, index: usize) -> Option<String> {
        let cells: Vec<String> = line.chars().map(String::from).collect();
        let cells: Vec<&str> = cells.iter().map(String::as_str).collect();
        find_url(&cells, index).map(|range| cells[range].concat())
    }

    /// `line` の中の URL のうち、最初の文字を含むもの。
    fn url_in(line: &str) -> Option<String> {
        let index = line.find("://").or_else(|| line.find(':'))?;
        url_at(line, line[..index].chars().count())
    }

    #[test]
    fn finds_urls_of_each_scheme() {
        assert_eq!(
            url_in("see https://example.com/a?b=c#d now").as_deref(),
            Some("https://example.com/a?b=c#d")
        );
        assert_eq!(
            url_in("http://localhost:8080/").as_deref(),
            Some("http://localhost:8080/")
        );
        assert_eq!(
            url_in("open file:///C:/Users/xia/a.txt").as_deref(),
            Some("file:///C:/Users/xia/a.txt")
        );
        assert_eq!(
            url_in("mail mailto:a@example.com.").as_deref(),
            Some("mailto:a@example.com")
        );
        assert_eq!(
            url_in("HTTPS://EXAMPLE.COM").as_deref(),
            Some("HTTPS://EXAMPLE.COM")
        );
    }

    #[test]
    fn the_url_contains_the_pointed_cell() {
        let line = "a https://x.example b";
        assert_eq!(url_at(line, 0), None);
        assert_eq!(url_at(line, 1), None);
        assert_eq!(url_at(line, 2).as_deref(), Some("https://x.example"));
        assert_eq!(url_at(line, 18).as_deref(), Some("https://x.example"));
        assert_eq!(url_at(line, 19), None);
        // 二つ目の URL も見つける。
        let line = "https://a.example https://b.example";
        assert_eq!(url_at(line, 20).as_deref(), Some("https://b.example"));
    }

    #[test]
    fn trailing_punctuation_is_not_part_of_the_url() {
        for (line, url) in [
            ("https://example.com.", "https://example.com"),
            ("https://example.com/a,", "https://example.com/a"),
            ("'https://example.com/'", "https://example.com/"),
            ("https://example.com/?!", "https://example.com/"),
            ("https://example.com/a:", "https://example.com/a"),
        ] {
            assert_eq!(url_in(line).as_deref(), Some(url), "{line}");
        }
    }

    #[test]
    fn unbalanced_closing_brackets_are_not_part_of_the_url() {
        for (line, url) in [
            ("(https://example.com/a)", "https://example.com/a"),
            ("(see https://example.com/a).", "https://example.com/a"),
            (
                "https://en.wikipedia.org/wiki/Rust_(language)",
                "https://en.wikipedia.org/wiki/Rust_(language)",
            ),
            (
                "(https://en.wikipedia.org/wiki/Rust_(language))",
                "https://en.wikipedia.org/wiki/Rust_(language)",
            ),
            ("[https://example.com/]", "https://example.com/"),
            ("https://example.com/[1]", "https://example.com/[1]"),
        ] {
            assert_eq!(url_in(line).as_deref(), Some(url), "{line}");
        }
    }

    #[test]
    fn characters_outside_urls_end_them() {
        assert_eq!(
            url_in("<https://example.com/>").as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(
            url_in("\"https://example.com/\"").as_deref(),
            Some("https://example.com/")
        );
        // 全角の文字が続けば、そこで切る。
        assert_eq!(
            url_in("https://example.com/を開く").as_deref(),
            Some("https://example.com/")
        );
    }

    #[test]
    fn wide_and_combined_cells_end_urls() {
        let cells = ["h", "t", "t", "p", ":", "/", "/", "a", "あ", "b"];
        assert_eq!(find_url(&cells, 0), Some(0..8));
        let cells = ["h", "t", "t", "p", ":", "/", "/", "a", "e\u{301}"];
        assert_eq!(find_url(&cells, 0), Some(0..8));
    }

    #[test]
    fn a_scheme_alone_or_inside_a_word_is_not_a_url() {
        assert_eq!(url_in("https://"), None);
        assert_eq!(url_in("https://."), None);
        assert_eq!(url_in("mailto:"), None);
        assert_eq!(url_in("xhttps://example.com"), None);
        assert_eq!(url_in("ftp://example.com"), None);
        assert_eq!(url_at("ab", 5), None);
    }

    fn line_cells(rows: &[&str], first_screen_row: u32) -> Vec<LineCell> {
        rows.iter()
            .enumerate()
            .flat_map(|(row, text)| {
                text.chars().enumerate().map(move |(col, c)| LineCell {
                    screen_row: first_screen_row + row as u32,
                    col: col as u16,
                    width: 1,
                    text: if c == ' ' {
                        String::new()
                    } else {
                        c.to_string()
                    },
                    uri: None,
                })
            })
            .collect()
    }

    #[test]
    fn a_wrapped_url_spans_rows() {
        let cells = line_cells(&["go https:/", "/example.c", "om/ now"], 10);
        let (url, range) = link_in_line(&cells, 12).unwrap();
        assert_eq!(url, "https://example.com/");
        assert_eq!(range, 3..23);
        assert_eq!(
            spans_of(&cells, range.clone(), 10, 3),
            [
                LinkSpan {
                    row: 0,
                    cols: 3..10
                },
                LinkSpan {
                    row: 1,
                    cols: 0..10
                },
                LinkSpan { row: 2, cols: 0..3 },
            ]
        );
        // 表示している範囲の外の行は除く。
        assert_eq!(
            spans_of(&cells, range, 11, 1),
            [LinkSpan {
                row: 0,
                cols: 0..10
            }]
        );
    }

    #[test]
    fn hyperlink_cells_are_preferred_and_grouped_by_uri() {
        let mut cells = line_cells(&["ab cd https://x.example"], 0);
        for cell in &mut cells[0..2] {
            cell.uri = Some("https://a.example".into());
        }
        // 空白のセルも、同じ URI ならリンクの一部。
        for cell in &mut cells[2..5] {
            cell.uri = Some("https://b.example".into());
        }
        assert_eq!(
            link_in_line(&cells, 1),
            Some(("https://a.example".into(), 0..2))
        );
        assert_eq!(
            link_in_line(&cells, 2),
            Some(("https://b.example".into(), 2..5))
        );
        assert_eq!(
            link_in_line(&cells, 8),
            Some(("https://x.example".into(), 6..23))
        );
        assert_eq!(link_in_line(&cells, 5), None);
    }

    #[test]
    fn wide_cells_cover_both_columns() {
        let cells = vec![
            LineCell {
                screen_row: 0,
                col: 0,
                width: 2,
                text: "日".into(),
                uri: Some("https://a.example".into()),
            },
            LineCell {
                screen_row: 0,
                col: 2,
                width: 2,
                text: "本".into(),
                uri: Some("https://a.example".into()),
            },
        ];
        assert_eq!(
            spans_of(&cells, 0..2, 0, 1),
            [LinkSpan { row: 0, cols: 0..4 }]
        );
    }

    #[test]
    fn web_links_are_opened_as_uris() {
        assert_eq!(
            open_target("https://example.com/a?b#c"),
            Ok(OpenTarget::Uri("https://example.com/a?b#c".into()))
        );
        assert_eq!(
            open_target("HTTP://example.com"),
            Ok(OpenTarget::Uri("http://example.com".into()))
        );
        assert_eq!(
            open_target("mailto:a@example.com"),
            Ok(OpenTarget::Uri("mailto:a@example.com".into()))
        );
        // ASCII でない文字はパーセントエンコードして渡す。
        assert_eq!(
            open_target("https://ja.wikipedia.org/wiki/端末"),
            Ok(OpenTarget::Uri(
                "https://ja.wikipedia.org/wiki/%E7%AB%AF%E6%9C%AB".into()
            ))
        );
    }

    #[test]
    fn other_schemes_are_refused() {
        for (uri, scheme) in [
            ("javascript:alert(1)", "javascript"),
            ("ms-settings:", "ms-settings"),
            ("C:/Windows/System32/calc.exe", "c"),
            ("ftp://example.com", "ftp"),
            ("shell:startup", "shell"),
        ] {
            assert_eq!(
                open_target(uri),
                Err(RefusedLink::Scheme(scheme.into())),
                "{uri}"
            );
        }
        assert_eq!(open_target("example.com"), Err(RefusedLink::NoScheme));
    }

    #[test]
    fn uris_with_unsafe_characters_are_refused() {
        for uri in [
            "https://example.com/a b",
            "https://example.com/\" --flag",
            "https://example.com/\n",
            "https://example.com/\\",
            "https://example.com/<a>",
        ] {
            assert_eq!(
                open_target(uri),
                Err(RefusedLink::InvalidCharacter),
                "{uri:?}"
            );
        }
    }

    #[test]
    fn empty_links_are_refused() {
        assert_eq!(open_target("https:"), Err(RefusedLink::Empty));
        assert_eq!(open_target("https://"), Err(RefusedLink::Empty));
        assert_eq!(open_target("https:///a"), Err(RefusedLink::Empty));
        assert_eq!(open_target("mailto:"), Err(RefusedLink::Empty));
    }

    #[test]
    fn file_uris_become_windows_paths() {
        for (uri, path) in [
            ("file:///C:/Users/xia/a.txt", r"C:\Users\xia\a.txt"),
            ("file://localhost/C:/a%20b/", r"C:\a b\"),
            ("file:///d:/%E6%97%A5%E6%9C%AC", r"d:\日本"),
            ("file:/C:/a", r"C:\a"),
            ("file://C:/a", r"C:\a"),
            ("file:///C:", "C:"),
            ("file://server/share/a.txt", r"\\server\share\a.txt"),
        ] {
            assert_eq!(open_target(uri), Ok(OpenTarget::Path(path.into())), "{uri}");
        }
    }

    #[test]
    fn file_uris_that_are_not_windows_paths_are_refused() {
        assert_eq!(
            open_target("file:///home/xia/a.txt"),
            Err(RefusedLink::NotWindowsPath)
        );
        assert_eq!(
            open_target("file:///C:/a%22b"),
            Err(RefusedLink::InvalidCharacter)
        );
        assert_eq!(
            open_target("file:///C:/a%0Ab"),
            Err(RefusedLink::InvalidCharacter)
        );
        assert_eq!(
            open_target("file:///C:/a%zz"),
            Err(RefusedLink::InvalidCharacter)
        );
        assert_eq!(open_target("file://server/"), Err(RefusedLink::Empty));
    }
}
