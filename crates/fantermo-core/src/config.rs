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

/// 既定のフォント。先頭から探す。
pub const DEFAULT_FONT_FAMILIES: [&str; 3] = ["Bizin Gothic", "BIZ UDGothic", "Consolas"];

/// 既定のフォントの大きさ（ポイント）。
pub const DEFAULT_FONT_SIZE_PT: f32 = 12.0;

/// フォントの大きさとして受け付ける範囲（ポイント）。
pub const FONT_SIZE_RANGE_PT: RangeInclusive<f32> = 1.0..=128.0;

/// 設定ファイルが無いときに作る中身。どの項目も既定値のままコメントにして並べ、利用者が `#` を消して
/// 書き換えられるようにする。`shell` の値だけは既定値ではなく例（既定では書かない）。
pub const TEMPLATE: &str = r#"# fantermo の設定ファイル。
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
"#;

/// 設定の全体。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub font: FontConfig,
    /// 起動するコマンドライン。`None` なら fantermo の既定。
    pub shell: Option<String>,
    pub paste: PasteConfig,
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

/// 最上位とその一段下の表のキーの道筋から、そのキーを書いた行へ。`[font]` のような表の見出しは、
/// 見出しの行になる。
fn key_lines(text: &str) -> HashMap<String, usize> {
    let mut lines = HashMap::new();
    let Ok(root) = DeTable::parse(text) else {
        return lines;
    };
    for (key, value) in root.get_ref() {
        let path = key.get_ref().to_string();
        if let DeValue::Table(table) = value.get_ref() {
            for inner in table.keys() {
                lines.insert(
                    format!("{path}.{}", inner.get_ref()),
                    line_of(text, inner.span().start),
                );
            }
        }
        lines.insert(path, line_of(text, key.span().start));
    }
    lines
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
        }
    }

    #[test]
    fn template_gives_the_defaults_without_warnings() {
        let parsed = super::parse(TEMPLATE, &customized());
        assert!(!parsed.syntax_error);
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.config, Config::default());
    }

    /// 行頭の `# ` に続けて `キー = ` と書いた行の `# ` を外す。
    fn uncomment_settings(text: &str) -> String {
        text.lines()
            .map(|line| {
                let Some(rest) = line.strip_prefix("# ") else {
                    return line.to_owned();
                };
                let is_setting = rest.split_once(" = ").is_some_and(|(key, _)| {
                    !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '_')
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
        assert_eq!(
            parsed.config,
            Config {
                shell: Some(r"C:\cygwin64\bin\zsh.exe -l".to_owned()),
                ..Config::default()
            }
        );
        // どの項目も外した行として書いてある。
        let settings = text
            .lines()
            .filter(|line| !line.starts_with('#') && line.contains(" = "))
            .count();
        assert_eq!(settings, 5, "{text}");
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
            "shell = ' '\n[font]\nfamily = 3\nsize = 500\nwide_glyph = 'wide'\n[paste]\nconfirm = 1\n",
            &previous,
        );
        assert!(!parsed.syntax_error);
        assert_eq!(parsed.config, previous);
        assert_eq!(parsed.warnings.len(), 5, "{:?}", parsed.warnings);

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

        let parsed = super::parse("font = 1\npaste = 2\n", &previous);
        assert_eq!(parsed.config.font, previous.font);
        assert_eq!(parsed.config.paste, previous.paste);
        assert_eq!(parsed.config.shell, None);
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
    fn syntax_errors_give_the_defaults() {
        let parsed = parse("[font\nsize = 10\n");
        assert_eq!(parsed.config, Config::default());
        assert_eq!(parsed.warnings.len(), 1);
        assert!(parsed.warnings[0].starts_with("cannot parse the settings"));
    }
}
