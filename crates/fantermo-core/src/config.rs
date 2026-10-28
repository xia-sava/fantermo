//! 設定ファイル（TOML）の型、既定値、解釈。
//!
//! 項目ごとに読む。型の誤りや範囲外の値はその項目だけを既定値にし、知らないキーは無視する。
//! どちらも警告として返す。TOML として読めないときは、すべてを既定値にする。
//!
//! ```toml
//! # 起動するコマンドライン。fantermo の引数があれば、そちらを使う。
//! shell = 'C:\cygwin64\bin\zsh.exe -l'
//!
//! [font]
//! # 先頭から探して、最初に見つかったものを使う。文字列一つでもよい。
//! family = ["Bizin Gothic", "BIZ UDGothic", "Consolas"]
//! # ポイント。
//! size = 12
//! # グリフがセルより広いとき（曖昧幅の ○ や ※ が全角の幅でデザインされているときなど）の描き方。
//! # セルの数は変えない。"shrink" は横に縮める、"clip" は縮めずにセルの左に置いてはみ出しを切る、
//! # "overflow" は縮めずに右のセルへはみ出して描く。
//! wide_glyph = "shrink"
//!
//! [paste]
//! # 貼り付ける前に、内容を編集できるダイアログで確かめるとき。"unsafe" はアプリが bracketed paste を
//! # 有効にしていないなど、改行を含む文字列がコマンドとして走りうるとき、"multiline" は bracketed
//! # paste に関わらず改行を含むとき。"never" は確かめずに改行ごと送る。
//! confirm = "unsafe"
//! ```

use std::ops::RangeInclusive;

use serde::{Deserialize, de::DeserializeOwned};
use toml::{Table, Value};

/// 既定のフォント。先頭から探す。
pub const DEFAULT_FONT_FAMILIES: [&str; 3] = ["Bizin Gothic", "BIZ UDGothic", "Consolas"];

/// 既定のフォントの大きさ（ポイント）。
pub const DEFAULT_FONT_SIZE_PT: f32 = 12.0;

/// フォントの大きさとして受け付ける範囲（ポイント）。
pub const FONT_SIZE_RANGE_PT: RangeInclusive<f32> = 1.0..=128.0;

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
    pub warnings: Vec<String>,
}

/// `font.family` に書ける形。
#[derive(Deserialize)]
#[serde(untagged, expecting = "a string or an array of strings")]
enum FontFamilies {
    One(String),
    Many(Vec<String>),
}

/// 設定ファイルの中身を解釈する。
pub fn parse(text: &str) -> Parsed {
    let mut warnings = Vec::new();
    // メモ帳などが UTF-8 の先頭に付ける BOM は、TOML の構文にない。
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut root = match toml::from_str::<Table>(text) {
        Ok(root) => root,
        Err(e) => {
            warnings.push(format!(
                "cannot parse the settings, using the defaults: {}",
                message(&e)
            ));
            return Parsed {
                config: Config::default(),
                warnings,
            };
        }
    };

    let mut config = Config::default();
    if let Some(mut font) = take::<Table>(&mut root, None, "font", &mut warnings) {
        read_font(&mut font, &mut config.font, &mut warnings);
        warn_unknown(&font, Some("font"), &mut warnings);
    }
    if let Some(shell) = take::<String>(&mut root, None, "shell", &mut warnings) {
        if shell.trim().is_empty() {
            warnings.push("ignoring shell: empty".to_owned());
        } else {
            config.shell = Some(shell);
        }
    }
    if let Some(mut paste) = take::<Table>(&mut root, None, "paste", &mut warnings) {
        const SECTION: Option<&str> = Some("paste");
        if let Some(confirm) = take::<PasteConfirm>(&mut paste, SECTION, "confirm", &mut warnings) {
            config.paste.confirm = confirm;
        }
        warn_unknown(&paste, SECTION, &mut warnings);
    }
    warn_unknown(&root, None, &mut warnings);
    Parsed { config, warnings }
}

fn read_font(table: &mut Table, font: &mut FontConfig, warnings: &mut Vec<String>) {
    const SECTION: Option<&str> = Some("font");
    if let Some(families) = take::<FontFamilies>(table, SECTION, "family", warnings) {
        let families = match families {
            FontFamilies::One(family) => vec![family],
            FontFamilies::Many(families) => families,
        };
        if families.is_empty() || families.iter().any(|family| family.trim().is_empty()) {
            warnings.push(format!(
                "ignoring font.family = {families:?}: names must not be empty"
            ));
        } else {
            font.families = families;
        }
    }
    if let Some(size) = take::<f32>(table, SECTION, "size", warnings) {
        if FONT_SIZE_RANGE_PT.contains(&size) {
            font.size_pt = size;
        } else {
            warnings.push(format!(
                "ignoring font.size = {size}: must be between {} and {}",
                FONT_SIZE_RANGE_PT.start(),
                FONT_SIZE_RANGE_PT.end()
            ));
        }
    }
    if let Some(wide_glyph) = take::<WideGlyph>(table, SECTION, "wide_glyph", warnings) {
        font.wide_glyph = wide_glyph;
    }
}

/// `table` から `key` を取り出して `T` として読む。無ければ `None`、型が違えば警告して `None`。
/// `section` は `table` の表の名前（最上位なら `None`）で、警告に使う。
fn take<T: DeserializeOwned>(
    table: &mut Table,
    section: Option<&str>,
    key: &str,
    warnings: &mut Vec<String>,
) -> Option<T> {
    let value = table.remove(key)?;
    match value.try_into::<T>() {
        Ok(value) => Some(value),
        Err(e) => {
            warnings.push(format!(
                "ignoring {}: {}",
                key_path(section, key),
                message(&e)
            ));
            None
        }
    }
}

/// 読み終えた `table` に残ったキーを、知らないものとして警告する。
fn warn_unknown(table: &Table, section: Option<&str>, warnings: &mut Vec<String>) {
    for (key, value) in table {
        let kind = match value {
            Value::Table(_) => "table",
            _ => "setting",
        };
        warnings.push(format!(
            "ignoring unknown {kind} {}",
            key_path(section, key)
        ));
    }
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
