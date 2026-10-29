//! 状態ファイル（TOML）の型と、文字列との変換。
//!
//! 状態ファイルは fantermo が窓を閉じるときに書き、次に起動したときに読む。利用者が手で書くもの
//! ではない。覚えるのは窓の位置・大きさ・最大化だけ。

use serde::{Deserialize, Serialize};

/// 書き出すときに先頭に付ける説明。
const HEADER: &str = "# fantermo が窓を閉じるときに書くファイル。手で書き換えるものではない。\n\n";

/// 状態の全体。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// 窓の位置・大きさ・最大化。`None` なら fantermo の既定の大きさで開く。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowState>,
}

/// 窓の位置・大きさ（ピクセル、枠を含む）と、最大化していたか。位置と大きさは最大化していないときの
/// もの（`GetWindowPlacement` の `rcNormalPosition`。作業領域の座標）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowState {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

/// 読めなかったわけ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// TOML として、または状態の型として読めない。
    Syntax(String),
    /// 読めたが、使えない値（大きさが 0 以下など）。
    Invalid(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Syntax(message) | ParseError::Invalid(message) => f.write_str(message),
        }
    }
}

/// 状態ファイルの中身を読む。知らないキーは無視する。
pub fn parse(text: &str) -> Result<State, ParseError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let state: State = toml::from_str(text)
        .map_err(|e| ParseError::Syntax(e.to_string().trim_end().to_owned()))?;
    if let Some(window) = &state.window
        && (window.width <= 0 || window.height <= 0)
    {
        return Err(ParseError::Invalid(format!(
            "the window size {}x{} is not positive",
            window.width, window.height
        )));
    }
    Ok(state)
}

/// 状態ファイルに書く中身。
pub fn to_toml(state: &State) -> String {
    let body = match toml::to_string(state) {
        Ok(body) => body,
        // 整数と真偽値だけの型なので、失敗しない。
        Err(e) => unreachable!("cannot serialize the state: {e}"),
    };
    format!("{HEADER}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: WindowState = WindowState {
        left: -20,
        top: 40,
        width: 1600,
        height: 1000,
        maximized: true,
    };

    #[test]
    fn written_state_reads_back() {
        let state = State {
            window: Some(WINDOW),
        };
        let text = to_toml(&state);
        assert_eq!(
            text,
            "# fantermo が窓を閉じるときに書くファイル。手で書き換えるものではない。\n\n\
             [window]\nleft = -20\ntop = 40\nwidth = 1600\nheight = 1000\nmaximized = true\n"
        );
        assert_eq!(parse(&text), Ok(state));
    }

    #[test]
    fn empty_text_has_no_window() {
        assert_eq!(parse(""), Ok(State::default()));
        assert_eq!(to_toml(&State::default()), HEADER);
        assert_eq!(parse(HEADER), Ok(State::default()));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let text = "version = 2\n[window]\nleft = 0\ntop = 0\nwidth = 10\nheight = 20\n\
                    maximized = false\nmonitor = 'x'\n[tabs]\ncount = 3\n";
        assert_eq!(
            parse(text),
            Ok(State {
                window: Some(WindowState {
                    left: 0,
                    top: 0,
                    width: 10,
                    height: 20,
                    maximized: false,
                }),
            })
        );
    }

    #[test]
    fn leading_bom_is_skipped() {
        let text = format!("\u{feff}{}", to_toml(&State::default()));
        assert_eq!(parse(&text), Ok(State::default()));
    }

    #[test]
    fn broken_text_is_a_syntax_error() {
        for text in [
            "[window\n",
            "[window]\nleft = 0\n",
            "[window]\nleft = 'x'\ntop = 0\nwidth = 1\nheight = 1\nmaximized = false\n",
            "window = 3\n",
        ] {
            assert!(
                matches!(parse(text), Err(ParseError::Syntax(_))),
                "{text:?}: {:?}",
                parse(text)
            );
        }
    }

    #[test]
    fn non_positive_size_is_invalid() {
        let text = "[window]\nleft = 0\ntop = 0\nwidth = 0\nheight = 20\nmaximized = false\n";
        assert_eq!(
            parse(text),
            Err(ParseError::Invalid(
                "the window size 0x20 is not positive".to_owned()
            ))
        );
    }
}
