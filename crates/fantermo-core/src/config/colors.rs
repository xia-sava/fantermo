//! `[colors]` の設定。配色、端末の色、タブバーとスクロールバーの色のキー、`"#rrggbb"` の読み書き。

use crate::frame::Rgb;

use super::{name_of, named_values, value_named};

/// 色を書くテーブル（`[colors]` と `[colors.<名前>]`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorTable {
    /// 端末の色（`[colors]` に書く）。
    Terminal,
    /// タブバーの色。
    TabBar,
    /// スクロールバーの色。
    ScrollBar,
}

impl ColorTable {
    /// すべてのテーブル。設定ファイルの順。
    pub const ALL: [ColorTable; 3] = [
        ColorTable::Terminal,
        ColorTable::TabBar,
        ColorTable::ScrollBar,
    ];

    /// テーブルのキーのパス（`["colors", "tab_bar"]` など）。
    pub fn path(self) -> &'static [&'static str] {
        match self {
            ColorTable::Terminal => &["colors"],
            ColorTable::TabBar => &["colors", "tab_bar"],
            ColorTable::ScrollBar => &["colors", "scroll_bar"],
        }
    }

    /// テーブルに書ける色のキー。設定ファイルの順。
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            ColorTable::Terminal => &TERMINAL_COLOR_KEYS,
            ColorTable::TabBar => &TAB_BAR_COLOR_KEYS,
            ColorTable::ScrollBar => &SCROLL_BAR_COLOR_KEYS,
        }
    }
}

/// `[colors]` に書ける端末の色のキー。
pub const TERMINAL_COLOR_KEYS: [&str; 2] = ["background", "foreground"];

/// 端末の配色（`colors.scheme`）。背景色・文字の色・16 色をまとめて決める。カーソルは文字の色で
/// 描き、選択範囲は文字の色と背景色を入れ替えて描くので、どちらも配色に従う。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorScheme {
    /// 黒の背景に白の文字。
    #[default]
    Dark,
    /// 白の背景に濃い灰色の文字。
    Light,
}

impl ColorScheme {
    /// 値と、設定ファイルに書く名前。設定ファイルの説明と同じ順。
    pub(super) const NAMES: [(ColorScheme, &'static str); 2] =
        [(ColorScheme::Dark, "dark"), (ColorScheme::Light, "light")];

    /// 書ける値のすべて。設定ファイルの説明と同じ順。
    pub const ALL: [ColorScheme; 2] = named_values(&Self::NAMES);

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        name_of(&Self::NAMES, self)
    }

    /// 設定ファイルに書く名前 `name` の値。
    pub fn from_name(name: &str) -> Option<Self> {
        value_named(&Self::NAMES, name)
    }

    /// 配色の端末の色。
    pub fn colors(self) -> TerminalColors {
        match self {
            ColorScheme::Dark => DARK_COLORS,
            ColorScheme::Light => LIGHT_COLORS,
        }
    }
}

/// 端末の 16 色（SGR 30〜37、90〜97 とその背景）の数。256 色の残りは標準の計算で決める。
pub const ANSI_COLOR_COUNT: usize = 16;

/// 端末の既定の色。アプリが OSC 10 / 11 / 4 で変えた色は、これより優先する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalColors {
    pub background: Rgb,
    pub foreground: Rgb,
    /// 黒・赤・緑・黄・青・マゼンタ・シアン・白と、その明るい色の順。
    pub ansi: [Rgb; ANSI_COLOR_COUNT],
}

/// `0xrrggbb` の色。
pub(super) const fn hex(value: u32) -> Rgb {
    Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}

/// ダークの配色。libghostty-vt の既定の色（黒の背景、白の文字、Ghostty の既定の 16 色）と同じ。
pub const DARK_COLORS: TerminalColors = TerminalColors {
    background: hex(0x000000),
    foreground: hex(0xffffff),
    ansi: [
        hex(0x1d1f21),
        hex(0xcc6666),
        hex(0xb5bd68),
        hex(0xf0c674),
        hex(0x81a2be),
        hex(0xb294bb),
        hex(0x8abeb7),
        hex(0xc5c8c6),
        hex(0x666666),
        hex(0xd54e53),
        hex(0xb9ca4a),
        hex(0xe7c547),
        hex(0x7aa6da),
        hex(0xc397d8),
        hex(0x70c0b1),
        hex(0xeaeaea),
    ],
};

/// ライトの配色。Windows Terminal の One Half Light を元に、白の背景でも読めるように黄・緑・シアンと
/// 明るい色を暗めにした（明るい色も白の背景に対してコントラスト比 3.4 以上。明るい白だけは 2.2）。白と
/// 明るい白は、文字として読めるように灰色にした。背景は表示しているタブと同じ白。
pub const LIGHT_COLORS: TerminalColors = TerminalColors {
    background: hex(0xffffff),
    foreground: hex(0x383a42),
    ansi: [
        hex(0x000000),
        hex(0xc4262e),
        hex(0x2e7d32),
        hex(0x8a6400),
        hex(0x1f5fc8),
        hex(0xa626a4),
        hex(0x0a7a8a),
        hex(0x6c6c6c),
        hex(0x8a8a8a),
        hex(0xe0443e),
        hex(0x3e9a3e),
        hex(0xa87a00),
        hex(0x3b82e0),
        hex(0xbf4fc9),
        hex(0x1593a3),
        hex(0xb0b0b0),
    ],
};

/// `[colors.tab_bar]` に書ける色のキー。
pub const TAB_BAR_COLOR_KEYS: [&str; 9] = [
    "bar",
    "active_tab",
    "active_border",
    "active_title",
    "inactive_title",
    "separator",
    "hovered_tab",
    "idle_mark",
    "bell",
];

/// `[colors.scroll_bar]` に書ける色のキー。
pub const SCROLL_BAR_COLOR_KEYS: [&str; 4] = ["track", "thumb", "hovered_thumb", "dragged_thumb"];

/// `[colors]` の設定。色は書いた色だけを持ち、書いていない色（`None`）は既定の色（配色、システムの
/// 色、端末の色から決めるもの）を使う。並びはそれぞれのキーの並び（[`ColorTable::keys`]）の順。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColorsConfig {
    pub scheme: ColorScheme,
    pub terminal: [Option<Rgb>; TERMINAL_COLOR_KEYS.len()],
    pub tab_bar: [Option<Rgb>; TAB_BAR_COLOR_KEYS.len()],
    pub scroll_bar: [Option<Rgb>; SCROLL_BAR_COLOR_KEYS.len()],
}

impl ColorsConfig {
    /// テーブル `table` の色。キーの並びの順。
    pub fn table(&self, table: ColorTable) -> &[Option<Rgb>] {
        match table {
            ColorTable::Terminal => &self.terminal,
            ColorTable::TabBar => &self.tab_bar,
            ColorTable::ScrollBar => &self.scroll_bar,
        }
    }

    pub fn table_mut(&mut self, table: ColorTable) -> &mut [Option<Rgb>] {
        match table {
            ColorTable::Terminal => &mut self.terminal,
            ColorTable::TabBar => &mut self.tab_bar,
            ColorTable::ScrollBar => &mut self.scroll_bar,
        }
    }

    /// 端末の既定の色。配色の色を、書いた背景色と文字の色で上書きしたもの。
    pub fn terminal_colors(&self) -> TerminalColors {
        let mut colors = self.scheme.colors();
        let [background, foreground] = self.terminal;
        colors.background = background.unwrap_or(colors.background);
        colors.foreground = foreground.unwrap_or(colors.foreground);
        colors
    }
}

/// `"#rrggbb"` の形の色を読む。16 進の数字の大文字小文字は区別しない。ほかの形なら `None`。
pub fn parse_color(text: &str) -> Option<Rgb> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |index: usize| u8::from_str_radix(&hex[index..index + 2], 16).ok();
    Some(Rgb {
        r: channel(0)?,
        g: channel(2)?,
        b: channel(4)?,
    })
}

/// 色を `"#rrggbb"` の形（小文字）にする。
pub fn format_color(color: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemes_differ_in_every_color() {
        let (dark, light) = (DARK_COLORS, LIGHT_COLORS);
        assert_ne!(dark.background, light.background);
        assert_ne!(dark.foreground, light.foreground);
        for index in 0..ANSI_COLOR_COUNT {
            assert_ne!(dark.ansi[index], light.ansi[index], "{index}");
        }
        // ライトの背景は表示しているタブと同じ白。
        assert_eq!(light.background, hex(0xffffff));
    }

    #[test]
    fn colors_are_written_back_as_read() {
        for text in ["#000000", "#ffffff", "#a0b1c2"] {
            assert_eq!(parse_color(text).map(format_color).as_deref(), Some(text));
        }
        assert_eq!(parse_color("#A0B1C2"), parse_color("#a0b1c2"));
        for text in [
            "",
            "#",
            "#fff",
            "#12345",
            "#1234567",
            "#12345678",
            "123456",
            "#12345g",
            "#ｆｆｆｆｆｆ",
        ] {
            assert_eq!(parse_color(text), None, "{text:?}");
        }
    }
}
