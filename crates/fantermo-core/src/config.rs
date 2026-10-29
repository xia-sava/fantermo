//! 設定ファイル（TOML）の型、既定値、解釈。
//!
//! 項目ごとに読む。書いていない項目は既定値にする。型の誤りや範囲外の値はその項目だけを前の値の
//! まま（起動したときは既定値）にし、知らないキーは無視する。どちらも、誤りのある行を添えた警告として
//! 返す。TOML として読めないときは、すべてを前の値のままにする。
//!
//! 書ける項目と既定値は [`TEMPLATE`] にある。

use std::{collections::HashMap, ops::RangeInclusive};

use serde::{Deserialize, de::DeserializeOwned};
use toml::{
    Table, Value,
    de::{DeTable, DeValue},
};

use crate::{
    frame::Rgb,
    keymap::{self, KeyCombo, Keymap},
};

/// 既定のフォント。先頭から探す。
pub const DEFAULT_FONT_FAMILIES: [&str; 3] = ["Bizin Gothic", "BIZ UDGothic", "Consolas"];

/// 既定のフォントの大きさ（ポイント）。
pub const DEFAULT_FONT_SIZE_PT: f32 = 12.0;

/// フォントの大きさとして受け付ける範囲（ポイント）。
pub const FONT_SIZE_RANGE_PT: RangeInclusive<f32> = 1.0..=128.0;

/// 既定のスクロールバーの幅（96 DPI でのピクセル）。
pub const DEFAULT_SCROLL_BAR_WIDTH: u32 = 12;

/// スクロールバーの幅として受け付ける範囲（96 DPI でのピクセル）。
pub const SCROLL_BAR_WIDTH_RANGE: RangeInclusive<u32> = 4..=64;

/// 設定ファイルが無いときに作る中身。どの項目も既定値のままコメントにして並べ、利用者が `#` を消して
/// 書き換えられるようにする。`shell` の値だけは既定値ではなく例（既定では書かない）。
pub const TEMPLATE: &str = r##"# fantermo の設定ファイル。
# 行頭の「#」を消して値を書き換え、保存すると読み直す。書いていない項目は既定値になる。

# 起動するコマンドライン。fantermo を引数つきで起動したときは、引数のほうを使う。
# 書かなければ PowerShell を起動する。書き換えたら、次に開くタブから使う。
# shell = 'C:\cygwin64\bin\zsh.exe -l'

[font]
# フォントの名前。先頭から探し、最初に見つかったものを使う。名前一つだけでもよい。
# family = ["Bizin Gothic", "BIZ UDGothic", "Consolas"]

# 大きさ（ポイント）。1 から 128 まで。小数でもよい。
# size = 12

# グリフがセルより広いとき（曖昧幅の ○ や ※ が全角の幅でデザインされているときなど）の描き方。
# セルの数は変えない。"shrink" は横に縮める、"clip" は縮めずにセルの左に置いてはみ出しを切る、
# "overflow" は縮めずに右のセルへはみ出して描く。
# wide_glyph = "shrink"

[paste]
# 貼り付ける前に、内容を編集できるダイアログで確かめるとき。
# "unsafe" は改行を含む文字列がコマンドとして走りうるとき（アプリが bracketed paste を有効にしていない
# ときなど）、"multiline" は bracketed paste に関わらず改行を含むとき、"never" は確かめずに改行ごと送る。
# confirm = "unsafe"

[scroll_bar]
# 端末の右のスクロールバーの幅（96 DPI でのピクセル。画面の拡大率に合わせて広げる）。4 から 64 まで。
# 書き換えたら、端末の桁数と行数を保ったまま窓の幅を変える。
# width = 12

[colors]
# 端末の配色。"dark" は黒の背景に白の文字、"light" は白の背景に濃い灰色の文字。背景色・文字の色・16 色を
# まとめて変える。カーソルは文字の色で描き、選んだ範囲は文字の色と背景色を入れ替えて描く。
# scheme = "dark"

# 端末の背景色と文字の色。"#rrggbb" の形で書く。書かない色は配色の色（下の値は "dark" の色）。
# background = "#000000"
# foreground = "#ffffff"

[colors.tab_bar]
# タブバーの色。"#rrggbb" の形で書く。書かない色は Windows のシステムの色から決める（下の値は Windows 11
# の既定の色のときのもの）。bar は地、active_tab は表示しているタブ、active_border はその左・右・上の縁の
# 線、active_title はその文字と閉じるボタンの「×」、inactive_title は表示していないタブの文字と「×」と
# マウスの下の「+」「≡」、separator は表示していないタブの間の区切りの線、hovered_tab はマウスの下の
# 表示していないタブ、idle_mark はマウスの下にない「+」「≡」、bell はベルが鳴ったタブの印。
# bar = "#cccccc"
# active_tab = "#ffffff"
# active_border = "#a0a0a0"
# active_title = "#000000"
# inactive_title = "#000000"
# separator = "#a0a0a0"
# hovered_tab = "#e6e6e6"
# idle_mark = "#6d6d6d"
# bell = "#f0b030"

[colors.scroll_bar]
# スクロールバーの色。"#rrggbb" の形で書く。書かない色は端末の背景色と文字の色から決める（下の値は配色が
# "dark" の黒の背景と白の文字のときのもの）。track はマウスを載せている間とドラッグしている間のトラック、thumb は
# つまみ、hovered_thumb はマウスを載せているつまみ、dragged_thumb はドラッグしているつまみ。
# track = "#0f0f0f"
# thumb = "#595959"
# hovered_thumb = "#808080"
# dragged_thumb = "#a6a6a6"

[keys]
# キーの組み合わせと、押したときの動作。書いた組み合わせだけ、下に並べた既定の割り当てを変える。
# 組み合わせは、修飾キー（ctrl、shift、alt、win）とキーを「+」でつなぐ。キーの名前はキーボードの上の
# 位置の名前で、a〜z、digit_0〜digit_9、f1〜f24、tab、enter、space、escape、backspace、insert、delete、
# home、end、page_up、page_down、arrow_up、arrow_down、arrow_left、arrow_right、context_menu、
# numpad_0〜numpad_9 など。
# 動作は copy、paste、context_menu、new_tab、close_tab、next_tab、prev_tab。"none" は割り当てを外し、
# キーをシェルへ送る。同じ動作に複数の組み合わせを割り当ててもよい。
# 例: Ctrl+Tab をシェルへ送り、Ctrl+PageDown で次のタブにするなら、次の 2 行を書く。
# 例: "ctrl+tab" = "none"
# 例: "ctrl+page_down" = "next_tab"
# 既定の割り当て:
# "ctrl+c" = "copy"
# "ctrl+insert" = "copy"
# "shift+insert" = "paste"
# "context_menu" = "context_menu"
# "shift+f10" = "context_menu"
# "ctrl+shift+t" = "new_tab"
# "ctrl+shift+w" = "close_tab"
# "ctrl+tab" = "next_tab"
# "ctrl+shift+tab" = "prev_tab"
"##;

/// 設定の全体。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub font: FontConfig,
    /// 起動するコマンドライン。`None` なら fantermo の既定。
    pub shell: Option<String>,
    pub paste: PasteConfig,
    pub scroll_bar: ScrollBarConfig,
    pub colors: ColorsConfig,
    /// キーの割り当て。`[keys]` に書いた組み合わせを、既定の割り当てに重ねたもの。
    pub keys: Keymap,
}

/// 色を書く表（`[colors]` と `[colors.<名前>]`）。
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
    /// すべての表。設定ファイルの順。
    pub const ALL: [ColorTable; 3] = [
        ColorTable::Terminal,
        ColorTable::TabBar,
        ColorTable::ScrollBar,
    ];

    /// 表の道筋（`["colors", "tab_bar"]` など）。
    pub fn path(self) -> &'static [&'static str] {
        match self {
            ColorTable::Terminal => &["colors"],
            ColorTable::TabBar => &["colors", "tab_bar"],
            ColorTable::ScrollBar => &["colors", "scroll_bar"],
        }
    }

    /// 表に書ける色のキー。設定ファイルの順。
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
/// 描き、選んだ範囲は文字の色と背景色を入れ替えて描くので、どちらも配色に従う。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorScheme {
    /// 黒の背景に白の文字。
    #[default]
    Dark,
    /// 白の背景に濃い灰色の文字。
    Light,
}

impl ColorScheme {
    /// 書ける値のすべて。設定ファイルの説明と同じ順。
    pub const ALL: [ColorScheme; 2] = [ColorScheme::Dark, ColorScheme::Light];

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        match self {
            ColorScheme::Dark => "dark",
            ColorScheme::Light => "light",
        }
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
const fn hex(value: u32) -> Rgb {
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
    /// 表 `table` の色。キーの並びの順。
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

/// `[scroll_bar]` の設定。
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollBarConfig {
    /// 幅（96 DPI でのピクセル）。
    pub width: u32,
}

impl Default for ScrollBarConfig {
    fn default() -> Self {
        Self {
            width: DEFAULT_SCROLL_BAR_WIDTH,
        }
    }
}

/// `[paste]` の設定。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PasteConfig {
    /// 貼り付ける前に、編集のダイアログで確かめるとき。
    pub confirm: PasteConfirm,
}

/// 貼り付ける前に、内容を編集のダイアログで確かめるとき（`paste.confirm`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteConfirm {
    /// コマンドとして走りうるとき。改行を含む文字列でアプリが bracketed paste を有効にしていない
    /// ときと、bracketed paste の終わりの印を含むとき。
    #[default]
    Unsafe,
    /// bracketed paste に関わらず、改行か bracketed paste の終わりの印を含むとき。
    Multiline,
    /// 確かめない。bracketed paste でなければ、改行は CR にして送る。
    Never,
}

impl PasteConfirm {
    /// 書ける値のすべて。設定ファイルの説明と同じ順。
    pub const ALL: [PasteConfirm; 3] = [
        PasteConfirm::Unsafe,
        PasteConfirm::Multiline,
        PasteConfirm::Never,
    ];

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        match self {
            PasteConfirm::Unsafe => "unsafe",
            PasteConfirm::Multiline => "multiline",
            PasteConfirm::Never => "never",
        }
    }
}

/// `[font]` の設定。
#[derive(Clone, Debug, PartialEq)]
pub struct FontConfig {
    /// 探すフォントの名前。先頭から探し、最初に見つかったものを使う。空にはならない。
    pub families: Vec<String>,
    /// 大きさ（ポイント）。
    pub size_pt: f32,
    /// セルより広いグリフの描き方。
    pub wide_glyph: WideGlyph,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            families: DEFAULT_FONT_FAMILIES.map(str::to_owned).to_vec(),
            size_pt: DEFAULT_FONT_SIZE_PT,
            wide_glyph: WideGlyph::default(),
        }
    }
}

/// フォントのグリフが、文字に割り当てたセルより広いときの描き方（`font.wide_glyph`）。
/// 曖昧幅の文字を 1 セルで数えることは変えず、描き方だけを選ぶ。罫線などの形として描く文字には効かない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WideGlyph {
    /// セルに収まるように横に縮める。
    #[default]
    Shrink,
    /// 縮めずにセルの左に置き、はみ出した分は描かない。
    Clip,
    /// 縮めずにセルの左に置き、右のセルへはみ出して描く。
    Overflow,
}

impl WideGlyph {
    /// 書ける値のすべて。設定ファイルの説明と同じ順。
    pub const ALL: [WideGlyph; 3] = [WideGlyph::Shrink, WideGlyph::Clip, WideGlyph::Overflow];

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        match self {
            WideGlyph::Shrink => "shrink",
            WideGlyph::Clip => "clip",
            WideGlyph::Overflow => "overflow",
        }
    }
}

/// 入力された文字列を、フォントの大きさ（ポイント）として読む。前後の空白は除く。数でないか、
/// [`FONT_SIZE_RANGE_PT`] の外なら `None`。
pub fn parse_font_size(text: &str) -> Option<f32> {
    let size: f32 = text.trim().parse().ok()?;
    FONT_SIZE_RANGE_PT.contains(&size).then_some(size)
}

/// 入力された文字列を、スクロールバーの幅として読む。前後の空白は除く。整数でないか、
/// [`SCROLL_BAR_WIDTH_RANGE`] の外なら `None`。
pub fn parse_scroll_bar_width(text: &str) -> Option<u32> {
    let width: u32 = text.trim().parse().ok()?;
    SCROLL_BAR_WIDTH_RANGE.contains(&width).then_some(width)
}

/// 入力された文字列を、`shell` の値にする。前後の空白は除き、空なら書かない（既定のシェル）。
pub fn shell_from_input(text: &str) -> Option<String> {
    let shell = text.trim();
    (!shell.is_empty()).then(|| shell.to_owned())
}

/// 解釈した設定と、読めなかった項目についての警告。
#[derive(Debug)]
pub struct Parsed {
    pub config: Config,
    pub warnings: Vec<Warning>,
    /// TOML として読めなかった。`config` は前の設定のままで、`warnings` はその誤りの一つだけ。
    pub syntax_error: bool,
}

/// 設定ファイルの誤りについての警告。
#[derive(Clone, Debug, PartialEq)]
pub struct Warning {
    /// 誤りのある行（1 から数える）。分からなければ `None`。
    pub line: Option<usize>,
    /// 何がおかしいか。
    pub message: String,
}

/// 一つの項目を読んだ結果。
enum Item<T> {
    /// 書いていない。
    Absent,
    /// 読めなかった。警告は残してある。
    Invalid,
    Valid(T),
}

impl<T> Item<T> {
    /// 読めた値を `field` に入れる。書いていなければ `default` を入れ、読めなければ `field` のまま。
    fn apply(self, field: &mut T, default: T) {
        match self {
            Item::Absent => *field = default,
            Item::Invalid => {}
            Item::Valid(value) => *field = value,
        }
    }

    fn map<U>(self, f: impl FnOnce(T) -> U) -> Item<U> {
        match self {
            Item::Absent => Item::Absent,
            Item::Invalid => Item::Invalid,
            Item::Valid(value) => Item::Valid(f(value)),
        }
    }

    /// 読めた値を `parse` で別の型にする。`parse` が警告の文を返したら、警告を残して読めなかったことに
    /// する。
    fn parse<U>(
        self,
        reader: &mut Reader,
        path: &str,
        parse: impl FnOnce(T) -> Result<U, String>,
    ) -> Item<U> {
        match self {
            Item::Absent => Item::Absent,
            Item::Invalid => Item::Invalid,
            Item::Valid(value) => match parse(value) {
                Ok(value) => Item::Valid(value),
                Err(message) => {
                    reader.warn(path, message);
                    Item::Invalid
                }
            },
        }
    }

    /// 読めた値を `check` で確かめる。`check` が警告の文を返したら、警告を残して読めなかったことにする。
    fn check(
        self,
        reader: &mut Reader,
        path: &str,
        check: impl FnOnce(&T) -> Option<String>,
    ) -> Item<T> {
        let Item::Valid(value) = self else {
            return self;
        };
        match check(&value) {
            Some(message) => {
                reader.warn(path, message);
                Item::Invalid
            }
            None => Item::Valid(value),
        }
    }
}

/// `font.family` に書ける形。
#[derive(Deserialize)]
#[serde(untagged, expecting = "a string or an array of strings")]
enum FontFamilies {
    One(String),
    Many(Vec<String>),
}

/// 設定ファイルの中身 `text` を解釈する。`previous` はいま使っている設定（起動したときは既定値）で、
/// 読めなかった項目と、TOML として読めないときの全体はこれのままにする。
pub fn parse(text: &str, previous: &Config) -> Parsed {
    // メモ帳などが UTF-8 の先頭に付ける BOM は、TOML の構文にない。
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut root = match toml::from_str::<Table>(text) {
        Ok(root) => root,
        Err(e) => {
            return Parsed {
                config: previous.clone(),
                warnings: vec![syntax_warning(text, &e)],
                syntax_error: true,
            };
        }
    };

    let mut reader = Reader {
        lines: key_lines(text),
        warnings: Vec::new(),
    };
    let mut config = previous.clone();
    match reader.take::<Table>(&mut root, None, "font") {
        Item::Absent => config.font = FontConfig::default(),
        Item::Invalid => {}
        Item::Valid(mut font) => {
            read_font(&mut reader, &mut font, &mut config.font);
            reader.warn_unknown(&font, Some("font"));
        }
    }
    reader
        .take::<String>(&mut root, None, "shell")
        .check(&mut reader, "shell", |shell| {
            shell
                .trim()
                .is_empty()
                .then(|| "ignoring shell: empty".to_owned())
        })
        .map(Some)
        .apply(&mut config.shell, None);
    match reader.take::<Table>(&mut root, None, "paste") {
        Item::Absent => config.paste = PasteConfig::default(),
        Item::Invalid => {}
        Item::Valid(mut paste) => {
            const SECTION: Option<&str> = Some("paste");
            reader
                .take::<PasteConfirm>(&mut paste, SECTION, "confirm")
                .apply(&mut config.paste.confirm, PasteConfirm::default());
            reader.warn_unknown(&paste, SECTION);
        }
    }
    match reader.take::<Table>(&mut root, None, "scroll_bar") {
        Item::Absent => config.scroll_bar = ScrollBarConfig::default(),
        Item::Invalid => {}
        Item::Valid(mut scroll_bar) => {
            const SECTION: Option<&str> = Some("scroll_bar");
            reader
                .take::<u32>(&mut scroll_bar, SECTION, "width")
                .check(&mut reader, "scroll_bar.width", |width| {
                    (!SCROLL_BAR_WIDTH_RANGE.contains(width)).then(|| {
                        format!(
                            "ignoring scroll_bar.width = {width}: must be between {} and {}",
                            SCROLL_BAR_WIDTH_RANGE.start(),
                            SCROLL_BAR_WIDTH_RANGE.end()
                        )
                    })
                })
                .apply(&mut config.scroll_bar.width, DEFAULT_SCROLL_BAR_WIDTH);
            reader.warn_unknown(&scroll_bar, SECTION);
        }
    }
    match reader.take::<Table>(&mut root, None, "colors") {
        Item::Absent => config.colors = ColorsConfig::default(),
        Item::Invalid => {}
        Item::Valid(mut colors) => {
            read_colors(&mut reader, &mut colors, &mut config.colors);
            reader.warn_unknown(&colors, Some("colors"));
        }
    }
    match reader.take::<Table>(&mut root, None, "keys") {
        Item::Absent => config.keys = Keymap::default(),
        Item::Invalid => {}
        Item::Valid(keys) => config.keys = read_keys(&mut reader, keys, &previous.keys),
    }
    reader.warn_unknown(&root, None);
    // ファイルの上から順に読めるように、行の順に並べる。
    reader
        .warnings
        .sort_by_key(|warning| warning.line.unwrap_or(usize::MAX));
    Parsed {
        config,
        warnings: reader.warnings,
        syntax_error: false,
    }
}

fn read_font(reader: &mut Reader, table: &mut Table, font: &mut FontConfig) {
    const SECTION: Option<&str> = Some("font");
    let defaults = FontConfig::default();
    reader
        .take::<FontFamilies>(table, SECTION, "family")
        .map(|families| match families {
            FontFamilies::One(family) => vec![family],
            FontFamilies::Many(families) => families,
        })
        .check(reader, "font.family", |families| {
            (families.is_empty() || families.iter().any(|family| family.trim().is_empty()))
                .then(|| format!("ignoring font.family = {families:?}: names must not be empty"))
        })
        .apply(&mut font.families, defaults.families);
    reader
        .take::<f32>(table, SECTION, "size")
        .check(reader, "font.size", |size| {
            (!FONT_SIZE_RANGE_PT.contains(size)).then(|| {
                format!(
                    "ignoring font.size = {size}: must be between {} and {}",
                    FONT_SIZE_RANGE_PT.start(),
                    FONT_SIZE_RANGE_PT.end()
                )
            })
        })
        .apply(&mut font.size_pt, defaults.size_pt);
    reader
        .take::<WideGlyph>(table, SECTION, "wide_glyph")
        .apply(&mut font.wide_glyph, defaults.wide_glyph);
}

/// `[colors]` の配色と、その中の色の表を読む。
fn read_colors(reader: &mut Reader, colors: &mut Table, config: &mut ColorsConfig) {
    const SECTION: Option<&str> = Some("colors");
    reader
        .take::<ColorScheme>(colors, SECTION, "scheme")
        .apply(&mut config.scheme, ColorScheme::default());
    for table in ColorTable::ALL {
        let section = table.path().join(".");
        let slots = config.table_mut(table);
        let Some(name) = table.path().get(1) else {
            // 端末の色は `[colors]` に直に書く。残りのキーは呼び出し元が知らないものとして扱う。
            read_color_values(reader, colors, &section, table, slots);
            continue;
        };
        match reader.take::<Table>(colors, SECTION, name) {
            Item::Absent => slots.fill(None),
            Item::Invalid => {}
            Item::Valid(mut values) => {
                read_color_values(reader, &mut values, &section, table, slots);
                reader.warn_unknown(&values, Some(&section));
            }
        }
    }
}

/// 表 `values`（道筋は `section`）から、`table` の色を `slots` に読む。
fn read_color_values(
    reader: &mut Reader,
    values: &mut Table,
    section: &str,
    table: ColorTable,
    slots: &mut [Option<Rgb>],
) {
    for (slot, key) in slots.iter_mut().zip(table.keys()) {
        let path = key_path(Some(section), key);
        reader
            .take::<String>(values, Some(section), key)
            .parse(reader, &path, |text| {
                parse_color(&text)
                    .ok_or_else(|| format!("ignoring {path} = {text:?}: expected \"#rrggbb\""))
            })
            .map(Some)
            .apply(slot, None);
    }
}

/// `[keys]` の表を、既定の割り当てに上から順に重ねる。組み合わせを読めない行は飛ばす。動作を読めない
/// 行は、その組み合わせを前の割り当て（`previous`）のままにする。同じ組み合わせを二度書いたら、後の行を
/// 飛ばす。
fn read_keys(reader: &mut Reader, keys: Table, previous: &Keymap) -> Keymap {
    let mut entries: Vec<(String, Value)> = keys.into_iter().collect();
    entries.sort_by_key(|(name, _)| {
        reader
            .lines
            .get(&key_path(Some("keys"), name))
            .copied()
            .unwrap_or(usize::MAX)
    });
    let mut keymap = Keymap::default();
    let mut written: Vec<(KeyCombo, String)> = Vec::new();
    for (name, value) in entries {
        let path = key_path(Some("keys"), &name);
        let combo = match KeyCombo::parse(&name) {
            Ok(combo) => combo,
            Err(e) => {
                reader.warn(&path, format!("ignoring keys.\"{name}\": {e}"));
                continue;
            }
        };
        if let Some((_, first)) = written.iter().find(|(seen, _)| *seen == combo) {
            reader.warn(
                &path,
                format!("ignoring keys.\"{name}\": the same keys as \"{first}\" above"),
            );
            continue;
        }
        written.push((combo, name.clone()));
        let action = match value {
            Value::String(action) => keymap::parse_action(&action),
            _ => Err(format!(
                "expected an action name or \"{}\"",
                keymap::UNBOUND
            )),
        };
        match action {
            Ok(action) => keymap.bind(combo, action),
            Err(e) => {
                reader.warn(&path, format!("ignoring keys.\"{name}\": {e}"));
                keymap.bind(combo, previous.lookup_combo(combo));
            }
        }
    }
    keymap
}

/// 項目を読みながら、警告を誤りのある行と一緒に溜める。
struct Reader {
    /// キーの道筋（`font.size` など）から、そのキーを書いた行へ。
    lines: HashMap<String, usize>,
    warnings: Vec<Warning>,
}

impl Reader {
    /// キー `path` についての警告を残す。
    fn warn(&mut self, path: &str, message: String) {
        self.warnings.push(Warning {
            line: self.lines.get(path).copied(),
            message,
        });
    }

    /// `table` から `key` を取り出して `T` として読む。型が違えば警告する。`section` は `table` の
    /// 表の名前（最上位なら `None`）。
    fn take<T: DeserializeOwned>(
        &mut self,
        table: &mut Table,
        section: Option<&str>,
        key: &str,
    ) -> Item<T> {
        let Some(value) = table.remove(key) else {
            return Item::Absent;
        };
        match value.try_into::<T>() {
            Ok(value) => Item::Valid(value),
            Err(e) => {
                let path = key_path(section, key);
                self.warn(&path, format!("ignoring {path}: {}", message(&e)));
                Item::Invalid
            }
        }
    }

    /// 読み終えた `table` に残ったキーを、知らないものとして警告する。
    fn warn_unknown(&mut self, table: &Table, section: Option<&str>) {
        for (key, value) in table {
            let kind = match value {
                Value::Table(_) => "table",
                _ => "setting",
            };
            let path = key_path(section, key);
            self.warn(&path, format!("ignoring unknown {kind} {path}"));
        }
    }
}

/// TOML として読めなかったことの警告。
fn syntax_warning(text: &str, error: &toml::de::Error) -> Warning {
    Warning {
        line: error.span().map(|span| line_of(text, span.start)),
        message: format!("cannot parse the settings: {}", error.message().trim_end()),
    }
}

/// 表のキーの道筋（`font.size`、`colors.tab_bar.bar` など。キーは引用符を外した名前）から、そのキーを
/// 書いた行へ。`[font]` のような表の見出しは、見出しの行になる。
fn key_lines(text: &str) -> HashMap<String, usize> {
    let mut lines = HashMap::new();
    let Ok(root) = DeTable::parse(text) else {
        return lines;
    };
    collect_key_lines(text, root.get_ref(), None, &mut lines);
    lines
}

fn collect_key_lines(
    text: &str,
    table: &DeTable,
    section: Option<&str>,
    lines: &mut HashMap<String, usize>,
) {
    for (key, value) in table {
        let path = key_path(section, key.get_ref());
        if let DeValue::Table(inner) = value.get_ref() {
            collect_key_lines(text, inner, Some(&path), lines);
        }
        lines.insert(path, line_of(text, key.span().start));
    }
}

/// `text` の `offset` バイト目がある行（1 から数える）。
fn line_of(text: &str, offset: usize) -> usize {
    let before = text.get(..offset).unwrap_or(text);
    before.matches('\n').count() + 1
}

/// toml のエラーの文。末尾の改行を除く。
fn message(error: &toml::de::Error) -> String {
    error.to_string().trim_end().to_owned()
}

fn key_path(section: Option<&str>, key: &str) -> String {
    match section {
        Some(section) => format!("{section}.{key}"),
        None => key.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::Action;

    fn families(names: &[&str]) -> Vec<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }

    /// 起動したときと同じく既定値を前の設定として読んだ結果。警告は文だけ。
    struct Read {
        config: Config,
        warnings: Vec<String>,
    }

    fn parse(text: &str) -> Read {
        let parsed = super::parse(text, &Config::default());
        Read {
            config: parsed.config,
            warnings: parsed.warnings.into_iter().map(|w| w.message).collect(),
        }
    }

    /// 既定値とどの項目も違う設定。
    fn customized() -> Config {
        Config {
            font: FontConfig {
                families: families(&["Cascadia Mono"]),
                size_pt: 9.0,
                wide_glyph: WideGlyph::Clip,
            },
            shell: Some("zsh -l".to_owned()),
            paste: PasteConfig {
                confirm: PasteConfirm::Never,
            },
            scroll_bar: ScrollBarConfig { width: 20 },
            colors: ColorsConfig {
                scheme: ColorScheme::Light,
                terminal: [None, Some(Rgb { r: 4, g: 5, b: 6 })],
                tab_bar: {
                    let mut colors = [None; TAB_BAR_COLOR_KEYS.len()];
                    colors[0] = Some(Rgb { r: 1, g: 2, b: 3 });
                    colors
                },
                ..ColorsConfig::default()
            },
            keys: {
                let mut keys = Keymap::default();
                keys.bind(combo("ctrl+tab"), None);
                keys.bind(combo("ctrl+page_down"), Some(Action::NextTab));
                keys
            },
        }
    }

    fn combo(text: &str) -> KeyCombo {
        KeyCombo::parse(text).unwrap()
    }

    #[test]
    fn template_gives_the_defaults_without_warnings() {
        let parsed = super::parse(TEMPLATE, &customized());
        assert!(!parsed.syntax_error);
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.config, Config::default());
    }

    /// 行頭の `# ` に続けて `キー = ` と書いた行の `# ` を外す。キーは英小文字と `_` か、`"` で囲んだ
    /// キーの組み合わせ。
    fn uncomment_settings(text: &str) -> String {
        text.lines()
            .map(|line| {
                let Some(rest) = line.strip_prefix("# ") else {
                    return line.to_owned();
                };
                let is_setting = rest.split_once(" = ").is_some_and(|(key, _)| {
                    let bare =
                        !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '_');
                    let combo = key
                        .strip_prefix('"')
                        .and_then(|key| key.strip_suffix('"'))
                        .is_some_and(|key| KeyCombo::parse(key).is_ok());
                    bare || combo
                });
                if is_setting { rest } else { line }.to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn template_shows_every_setting_with_its_default() {
        let text = uncomment_settings(TEMPLATE);
        let parsed = parse(&text);
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        // 色の値は既定の色の例。
        let hex = |text: &str| parse_color(text);
        assert_eq!(
            parsed.config,
            Config {
                shell: Some(r"C:\cygwin64\bin\zsh.exe -l".to_owned()),
                colors: ColorsConfig {
                    terminal: ["#000000", "#ffffff"].map(hex),
                    tab_bar: [
                        "#cccccc", "#ffffff", "#a0a0a0", "#000000", "#000000", "#a0a0a0",
                        "#e6e6e6", "#6d6d6d", "#f0b030"
                    ]
                    .map(hex),
                    scroll_bar: ["#0f0f0f", "#595959", "#808080", "#a6a6a6"].map(hex),
                    ..ColorsConfig::default()
                },
                ..Config::default()
            }
        );
        // どの項目も外した行として書いてある。
        let settings = text
            .lines()
            .filter(|line| !line.starts_with('#') && line.contains(" = "))
            .count();
        assert_eq!(settings, 7 + 9 + 15, "{text}");
        // 背景色と文字の色の例は、既定の配色の色。
        assert_eq!(
            parsed.config.colors.terminal_colors(),
            ColorScheme::default().colors()
        );
    }

    #[test]
    fn colors_are_read_as_written() {
        let parsed = parse(
            "[colors.tab_bar]\nbar = \"#FF0000\"\nbell = '#00ff7f'\n[colors.scroll_bar]\nthumb = \"#123456\"\n",
        );
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        let colors = &parsed.config.colors;
        assert_eq!(colors.tab_bar[0], Some(Rgb { r: 255, g: 0, b: 0 }));
        assert_eq!(
            colors.tab_bar[8],
            Some(Rgb {
                r: 0,
                g: 255,
                b: 127
            })
        );
        assert_eq!(
            colors
                .tab_bar
                .iter()
                .filter(|color| color.is_some())
                .count(),
            2
        );
        assert_eq!(
            colors.scroll_bar,
            [
                None,
                Some(Rgb {
                    r: 0x12,
                    g: 0x34,
                    b: 0x56
                }),
                None,
                None
            ]
        );
        // 点で区切ったキーとインラインの表でも書ける。
        let parsed = parse("colors.scroll_bar.track = '#010203'\n");
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(
            parsed.config.colors.scroll_bar[0],
            Some(Rgb { r: 1, g: 2, b: 3 })
        );
    }

    #[test]
    fn scheme_and_terminal_colors_are_read() {
        for scheme in ColorScheme::ALL {
            let parsed = parse(&format!("[colors]\nscheme = \"{}\"\n", scheme.name()));
            assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
            assert_eq!(parsed.config.colors.scheme, scheme);
            assert_eq!(parsed.config.colors.terminal_colors(), scheme.colors());
        }
        assert_eq!(ColorScheme::default(), ColorScheme::Dark);
        assert_eq!(
            parse("").config.colors.terminal_colors(),
            ColorScheme::Dark.colors()
        );

        // 背景色と文字の色は、書いたほうだけ配色の色を上書きする。16 色は配色のまま。
        let parsed = parse("[colors]\nscheme = 'light'\nforeground = '#102030'\n");
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        let colors = parsed.config.colors.terminal_colors();
        assert_eq!(colors.background, LIGHT_COLORS.background);
        assert_eq!(colors.foreground, hex(0x102030));
        assert_eq!(colors.ansi, LIGHT_COLORS.ansi);
        let parsed = parse("colors.background = '#fafafa'\n");
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        let colors = parsed.config.colors.terminal_colors();
        assert_eq!(colors.background, hex(0xfafafa));
        assert_eq!(colors.foreground, DARK_COLORS.foreground);
        assert_eq!(colors.ansi, DARK_COLORS.ansi);
    }

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
    fn invalid_colors_keep_the_previous_values_with_their_lines() {
        let previous = customized();
        let parsed = super::parse(
            "[colors.tab_bar]\nbar = '#12345'\nactive_tab = 5\nseparator = '#00ff00'\nshade = '#000000'\n[colors.scroll_bar]\nthumb = 'gray'\n[colors]\ntheme = 'dark'\nscheme = 'sepia'\nbackground = 'white'\n",
            &previous,
        );
        let warnings: Vec<(Option<usize>, &str)> = parsed
            .warnings
            .iter()
            .map(|w| (w.line, w.message.as_str()))
            .collect();
        assert_eq!(warnings.len(), 7, "{warnings:?}");
        assert_eq!(
            warnings[0],
            (
                Some(2),
                "ignoring colors.tab_bar.bar = \"#12345\": expected \"#rrggbb\""
            )
        );
        assert_eq!(warnings[1].0, Some(3));
        assert!(
            warnings[1]
                .1
                .starts_with("ignoring colors.tab_bar.active_tab:"),
            "{warnings:?}"
        );
        assert_eq!(
            warnings[2],
            (Some(5), "ignoring unknown setting colors.tab_bar.shade")
        );
        assert_eq!(
            warnings[3],
            (
                Some(7),
                "ignoring colors.scroll_bar.thumb = \"gray\": expected \"#rrggbb\""
            )
        );
        assert_eq!(
            warnings[4],
            (Some(9), "ignoring unknown setting colors.theme")
        );
        assert_eq!(warnings[5].0, Some(10));
        assert!(
            warnings[5].1.starts_with("ignoring colors.scheme:"),
            "{warnings:?}"
        );
        assert_eq!(
            warnings[6],
            (
                Some(11),
                "ignoring colors.background = \"white\": expected \"#rrggbb\""
            )
        );
        // 誤りの色と配色は前の値のまま、書いた色は読み、書いていない色は既定に戻る。
        let colors = &parsed.config.colors;
        assert_eq!(colors.scheme, previous.colors.scheme);
        assert_eq!(colors.terminal, [previous.colors.terminal[0], None]);
        assert_eq!(colors.tab_bar[0], previous.colors.tab_bar[0]);
        assert_eq!(colors.tab_bar[5], Some(Rgb { r: 0, g: 255, b: 0 }));
        assert_eq!(colors.scroll_bar, [None; 4]);
        // 書いていない表は既定の色に戻る。
        assert_eq!(parse("[font]\n").config.colors, ColorsConfig::default());
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

    #[test]
    fn template_lists_the_default_keys() {
        let listed: Vec<String> = TEMPLATE
            .lines()
            .skip_while(|line| *line != "[keys]")
            .filter_map(|line| line.strip_prefix("# \""))
            .map(|line| format!("\"{line}"))
            .collect();
        let expected: Vec<String> = Keymap::default()
            .bindings()
            .iter()
            .map(|(combo, action)| format!("\"{combo}\" = \"{}\"", action.name()))
            .collect();
        assert_eq!(listed, expected);
    }

    #[test]
    fn keys_are_laid_over_the_defaults() {
        let parsed = parse(
            "[keys]\n\"ctrl+tab\" = \"none\"\n\"Ctrl + Page_Down\" = \"next_tab\"\n\"ctrl+shift+n\" = \"new_tab\"\n",
        );
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        let keys = &parsed.config.keys;
        assert_eq!(keys.lookup_combo(combo("ctrl+tab")), None);
        assert_eq!(
            keys.lookup_combo(combo("ctrl+page_down")),
            Some(Action::NextTab)
        );
        // 同じ動作に複数の組み合わせ。既定の Ctrl+Shift+T も残る。
        assert_eq!(
            keys.combos_of(Action::NewTab),
            [combo("ctrl+shift+t"), combo("ctrl+shift+n")]
        );
        assert_eq!(
            keys.lookup_combo(combo("ctrl+shift+tab")),
            Some(Action::PrevTab)
        );
        // 既定の組み合わせを別の動作に割り当て直す。
        let parsed = parse("[keys]\n\"ctrl+c\" = \"paste\"\n");
        assert_eq!(
            parsed.config.keys.lookup_combo(combo("ctrl+c")),
            Some(Action::Paste)
        );
        assert_eq!(parse("[keys]\n").config.keys, Keymap::default());
    }

    #[test]
    fn invalid_key_lines_are_skipped_with_their_lines() {
        let previous = customized();
        let parsed = super::parse(
            "[keys]\n\"ctrl+tabb\" = \"new_tab\"\n\"ctrl+tab\" = \"next\"\n\"ctrl+page_down\" = 1\n\"ctrl+shift+n\" = \"new_tab\"\n\"Shift+Ctrl+N\" = \"close_tab\"\n",
            &previous,
        );
        let warnings: Vec<(Option<usize>, &str)> = parsed
            .warnings
            .iter()
            .map(|w| (w.line, w.message.as_str()))
            .collect();
        assert_eq!(
            warnings,
            [
                (
                    Some(2),
                    "ignoring keys.\"ctrl+tabb\": unknown key `tabb` in `ctrl+tabb`"
                ),
                (Some(3), "ignoring keys.\"ctrl+tab\": unknown action `next`"),
                (
                    Some(4),
                    "ignoring keys.\"ctrl+page_down\": expected an action name or \"none\""
                ),
                (
                    Some(6),
                    "ignoring keys.\"Shift+Ctrl+N\": the same keys as \"ctrl+shift+n\" above"
                ),
            ]
        );
        let keys = &parsed.config.keys;
        // 動作を読めない組み合わせは前の割り当てのまま。
        assert_eq!(keys.lookup_combo(combo("ctrl+tab")), None);
        assert_eq!(
            keys.lookup_combo(combo("ctrl+page_down")),
            Some(Action::NextTab)
        );
        assert_eq!(
            keys.lookup_combo(combo("ctrl+shift+n")),
            Some(Action::NewTab)
        );
        // 起動したときは既定の割り当て。
        let parsed = parse("[keys]\n\"ctrl+tab\" = \"next\"\n");
        assert_eq!(parsed.config.keys, Keymap::default());
        // 表でなければ全体が前のまま。
        let parsed = super::parse("keys = 1\n", &previous);
        assert_eq!(parsed.config.keys, previous.keys);
        assert_eq!(parsed.warnings.len(), 1);
    }

    #[test]
    fn syntax_errors_keep_the_previous_settings() {
        let previous = customized();
        let parsed = super::parse("[font]\nsize = 10\n[paste\n", &previous);
        assert!(parsed.syntax_error);
        assert_eq!(parsed.config, previous);
        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
        assert_eq!(parsed.warnings[0].line, Some(3));
        assert!(
            parsed.warnings[0]
                .message
                .starts_with("cannot parse the settings: "),
            "{:?}",
            parsed.warnings
        );
        assert!(!parsed.warnings[0].message.contains('\n'));
    }

    #[test]
    fn invalid_items_keep_the_previous_values_and_missing_ones_get_the_defaults() {
        let previous = customized();
        let parsed = super::parse(
            "shell = ' '\n[font]\nfamily = 3\nsize = 500\nwide_glyph = 'wide'\n[paste]\nconfirm = 1\n[scroll_bar]\nwidth = 2\n[keys]\n\"ctrl+tab\" = 5\n\"ctrl+page_down\" = 'next'\n[colors]\nscheme = 'sepia'\nforeground = 'blue'\n[colors.tab_bar]\nbar = 'red'\n",
            &previous,
        );
        assert!(!parsed.syntax_error);
        assert_eq!(parsed.config, previous);
        assert_eq!(parsed.warnings.len(), 11, "{:?}", parsed.warnings);

        let parsed = super::parse("[font]\nsize = 500\n", &previous);
        assert_eq!(
            parsed.config,
            Config {
                font: FontConfig {
                    size_pt: 9.0,
                    ..FontConfig::default()
                },
                ..Config::default()
            }
        );

        let parsed = super::parse("font = 1\npaste = 2\nscroll_bar = 3\n", &previous);
        assert_eq!(parsed.config.font, previous.font);
        assert_eq!(parsed.config.paste, previous.paste);
        assert_eq!(parsed.config.scroll_bar, previous.scroll_bar);
        assert_eq!(parsed.config.shell, None);
    }

    #[test]
    fn scroll_bar_width_takes_integers_in_range() {
        let parsed = parse("[scroll_bar]\nwidth = 16\n");
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.config.scroll_bar.width, 16);
        for (text, message) in [
            (
                "width = 3",
                "ignoring scroll_bar.width = 3: must be between 4 and 64",
            ),
            (
                "width = 65",
                "ignoring scroll_bar.width = 65: must be between 4 and 64",
            ),
        ] {
            let parsed = parse(&format!("[scroll_bar]\n{text}\n"));
            assert_eq!(parsed.config.scroll_bar.width, DEFAULT_SCROLL_BAR_WIDTH);
            assert_eq!(parsed.warnings, vec![message.to_owned()]);
        }
        for text in ["width = 12.5", "width = -1", "width = 'wide'"] {
            let parsed = parse(&format!("[scroll_bar]\n{text}\n"));
            assert_eq!(parsed.config.scroll_bar.width, DEFAULT_SCROLL_BAR_WIDTH);
            assert_eq!(parsed.warnings.len(), 1, "{text}: {:?}", parsed.warnings);
        }
        let parsed = parse("[scroll_bar]\nwidth = 16\nside = 'left'\n");
        assert_eq!(parsed.config.scroll_bar.width, 16);
        assert_eq!(
            parsed.warnings,
            vec!["ignoring unknown setting scroll_bar.side".to_owned()]
        );
    }

    #[test]
    fn warnings_name_the_line() {
        let parsed = super::parse(
            "\u{feff}# comment\nshell = 1\n\n[font]\nfamily = 'Consolas'\nsize = 0\n[tabs]\nwidth = 3\n",
            &Config::default(),
        );
        let mut lines: Vec<(Option<usize>, &str)> = parsed
            .warnings
            .iter()
            .map(|w| (w.line, w.message.as_str()))
            .collect();
        lines.sort();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[0].0, Some(2));
        assert!(lines[0].1.starts_with("ignoring shell:"), "{lines:?}");
        assert_eq!(
            lines[1],
            (Some(6), "ignoring font.size = 0: must be between 1 and 128")
        );
        assert_eq!(lines[2], (Some(7), "ignoring unknown table tabs"));
    }

    #[test]
    fn empty_file_gives_the_defaults() {
        let parsed = parse("");
        assert_eq!(parsed.config, Config::default());
        assert!(parsed.warnings.is_empty());
        assert_eq!(
            parsed.config.font.families,
            families(&["Bizin Gothic", "BIZ UDGothic", "Consolas"])
        );
        assert_eq!(parsed.config.font.size_pt, 12.0);
        assert_eq!(parsed.config.shell, None);
    }

    #[test]
    fn all_items_are_read() {
        let parsed = parse(
            r#"
            shell = 'C:\cygwin64\bin\zsh.exe -l'
            [font]
            family = ["Cascadia Mono", "Consolas"]
            size = 10.5
            wide_glyph = "overflow"
            [paste]
            confirm = "never"
            [scroll_bar]
            width = 8
            "#,
        );
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(
            parsed.config,
            Config {
                font: FontConfig {
                    families: families(&["Cascadia Mono", "Consolas"]),
                    size_pt: 10.5,
                    wide_glyph: WideGlyph::Overflow,
                },
                shell: Some(r"C:\cygwin64\bin\zsh.exe -l".to_owned()),
                paste: PasteConfig {
                    confirm: PasteConfirm::Never,
                },
                scroll_bar: ScrollBarConfig { width: 8 },
                ..Config::default()
            }
        );
    }

    #[test]
    fn family_may_be_a_single_string_and_size_an_integer() {
        let parsed = parse("[font]\nfamily = \"Consolas\"\nsize = 14\n");
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.config.font.families, families(&["Consolas"]));
        assert_eq!(parsed.config.font.size_pt, 14.0);
    }

    #[test]
    fn byte_order_mark_is_skipped() {
        let parsed = parse("\u{feff}[font]\nsize = 9\n");
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.config.font.size_pt, 9.0);
    }

    #[test]
    fn wrong_types_fall_back_only_for_that_item() {
        let parsed = parse("shell = 1\n[font]\nfamily = \"Consolas\"\nsize = \"big\"\n");
        assert_eq!(parsed.config.font.families, families(&["Consolas"]));
        assert_eq!(parsed.config.font.size_pt, DEFAULT_FONT_SIZE_PT);
        assert_eq!(parsed.config.shell, None);
        assert_eq!(parsed.warnings.len(), 2, "{:?}", parsed.warnings);
        assert!(parsed.warnings.iter().any(|w| w.contains("font.size")));
        assert!(parsed.warnings.iter().any(|w| w.contains("shell")));
        // ログの 1 行に収まるように、toml のエラーの末尾の改行は除く。
        assert!(parsed.warnings.iter().all(|w| !w.ends_with('\n')));
    }

    #[test]
    fn family_of_another_type_names_the_accepted_forms() {
        let parsed = parse("[font]\nfamily = 3\n");
        assert_eq!(parsed.config.font, FontConfig::default());
        assert_eq!(parsed.warnings.len(), 1);
        assert!(
            parsed.warnings[0].contains("a string or an array of strings"),
            "{:?}",
            parsed.warnings
        );
    }

    #[test]
    fn wide_glyph_takes_each_name() {
        assert_eq!(parse("").config.font.wide_glyph, WideGlyph::Shrink);
        for (name, wide_glyph) in [
            ("shrink", WideGlyph::Shrink),
            ("clip", WideGlyph::Clip),
            ("overflow", WideGlyph::Overflow),
        ] {
            let parsed = parse(&format!("[font]\nwide_glyph = '{name}'\n"));
            assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
            assert_eq!(parsed.config.font.wide_glyph, wide_glyph);
        }
    }

    #[test]
    fn unknown_wide_glyph_names_the_accepted_values() {
        let parsed = parse("[font]\nwide_glyph = 'Shrink'\nsize = 10\n");
        assert_eq!(parsed.config.font.wide_glyph, WideGlyph::Shrink);
        assert_eq!(parsed.config.font.size_pt, 10.0);
        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
        let warning = &parsed.warnings[0];
        assert!(
            warning.starts_with("ignoring font.wide_glyph:"),
            "{warning}"
        );
        assert!(
            ["shrink", "clip", "overflow"]
                .iter()
                .all(|name| warning.contains(name)),
            "{warning}"
        );
    }

    #[test]
    fn paste_confirm_takes_each_name() {
        assert_eq!(parse("").config.paste.confirm, PasteConfirm::Unsafe);
        for (name, confirm) in [
            ("unsafe", PasteConfirm::Unsafe),
            ("multiline", PasteConfirm::Multiline),
            ("never", PasteConfirm::Never),
        ] {
            let parsed = parse(&format!("[paste]\nconfirm = '{name}'\n"));
            assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
            assert_eq!(parsed.config.paste.confirm, confirm);
        }
    }

    #[test]
    fn unknown_paste_settings_are_reported() {
        let parsed = parse("[paste]\nconfirm = 'ask'\ntrim = true\n");
        assert_eq!(parsed.config.paste, PasteConfig::default());
        let mut warnings = parsed.warnings;
        warnings.sort();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0].starts_with("ignoring paste.confirm:"),
            "{warnings:?}"
        );
        assert!(
            ["unsafe", "multiline", "never"]
                .iter()
                .all(|name| warnings[0].contains(name)),
            "{warnings:?}"
        );
        assert_eq!(warnings[1], "ignoring unknown setting paste.trim");
    }

    #[test]
    fn out_of_range_and_empty_values_are_ignored() {
        let parsed = parse("shell = ' '\n[font]\nfamily = []\nsize = 0\n");
        assert_eq!(parsed.config, Config::default());
        assert_eq!(parsed.warnings.len(), 3, "{:?}", parsed.warnings);

        let parsed = parse("[font]\nfamily = [\"Consolas\", \"\"]\nsize = 200\n");
        assert_eq!(parsed.config, Config::default());
        assert_eq!(parsed.warnings.len(), 2, "{:?}", parsed.warnings);

        let parsed = parse("[font]\nsize = nan\n");
        assert_eq!(parsed.config, Config::default());
        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
    }

    #[test]
    fn unknown_keys_are_reported_and_the_rest_is_read() {
        let parsed = parse("colour = 'red'\n[font]\nsize = 11\nweight = 'bold'\n[tabs]\n");
        assert_eq!(parsed.config.font.size_pt, 11.0);
        let mut warnings = parsed.warnings;
        warnings.sort();
        assert_eq!(
            warnings,
            [
                "ignoring unknown setting colour",
                "ignoring unknown setting font.weight",
                "ignoring unknown table tabs",
            ]
        );
    }

    #[test]
    fn font_that_is_not_a_table_is_ignored() {
        let parsed = parse("font = 'Consolas'\n");
        assert_eq!(parsed.config, Config::default());
        assert_eq!(parsed.warnings.len(), 1);
        assert!(parsed.warnings[0].starts_with("ignoring font:"));
    }

    #[test]
    fn names_are_read_back() {
        for wide_glyph in WideGlyph::ALL {
            let parsed = parse(&format!("[font]\nwide_glyph = '{}'\n", wide_glyph.name()));
            assert_eq!(parsed.config.font.wide_glyph, wide_glyph);
        }
        for confirm in PasteConfirm::ALL {
            let parsed = parse(&format!("[paste]\nconfirm = '{}'\n", confirm.name()));
            assert_eq!(parsed.config.paste.confirm, confirm);
        }
    }

    #[test]
    fn inputs_are_checked_like_the_file() {
        assert_eq!(parse_font_size(" 10.5 "), Some(10.5));
        assert_eq!(parse_font_size("1"), Some(1.0));
        assert_eq!(parse_font_size("128"), Some(128.0));
        for text in ["", "0", "0.5", "129", "nan", "inf", "12pt", "-3"] {
            assert_eq!(parse_font_size(text), None, "{text:?}");
        }
        assert_eq!(parse_scroll_bar_width(" 16 "), Some(16));
        assert_eq!(parse_scroll_bar_width("4"), Some(4));
        assert_eq!(parse_scroll_bar_width("64"), Some(64));
        for text in ["", "3", "65", "12.5", "-1", "wide"] {
            assert_eq!(parse_scroll_bar_width(text), None, "{text:?}");
        }
        assert_eq!(shell_from_input("  zsh -l "), Some("zsh -l".to_owned()));
        assert_eq!(shell_from_input(" \t"), None);
    }

    #[test]
    fn syntax_errors_give_the_defaults() {
        let parsed = parse("[font\nsize = 10\n");
        assert_eq!(parsed.config, Config::default());
        assert_eq!(parsed.warnings.len(), 1);
        assert!(parsed.warnings[0].starts_with("cannot parse the settings"));
    }
}
