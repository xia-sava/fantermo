//! 設定ファイルの中身を、変えた項目だけ書き換える。
//!
//! コメント、空行、項目の並び、知らないキーはそのまま残す。書き換えるのは、前の設定と新しい設定で値が
//! 違う項目だけ。
//!
//! - 書いてある項目は、値だけを書き換える（行末のコメントは残す）。
//! - 書いていない項目は、その表の中に `# キー = …` とコメントにした行があれば、その行を置き換えて
//!   書く（[`TEMPLATE`](crate::config::TEMPLATE) から作ったファイルで、説明の行の下に入る）。なければ
//!   表の最後に足す。表がなければ、ファイルの最後に表ごと足す。
//! - 既定値に戻した項目は、その行を既定値の `# キー = …` のコメントの行にする（`shell` は既定値が
//!   ないので、それまでの値をコメントにする）。上に書いた説明のコメントは残る。

use toml_edit::{Array, Decor, DocumentMut, Item, Key, Table, Value};

use crate::config::Config;

/// 書き換える項目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    Shell,
    FontFamily,
    FontSize,
    WideGlyph,
    PasteConfirm,
    ScrollBarWidth,
}

impl Setting {
    const ALL: [Setting; 6] = [
        Setting::Shell,
        Setting::FontFamily,
        Setting::FontSize,
        Setting::WideGlyph,
        Setting::PasteConfirm,
        Setting::ScrollBarWidth,
    ];

    /// 項目を置く表の名前。最上位なら `None`。
    fn section(self) -> Option<&'static str> {
        match self {
            Setting::Shell => None,
            Setting::FontFamily | Setting::FontSize | Setting::WideGlyph => Some("font"),
            Setting::PasteConfirm => Some("paste"),
            Setting::ScrollBarWidth => Some("scroll_bar"),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Setting::Shell => "shell",
            Setting::FontFamily => "family",
            Setting::FontSize => "size",
            Setting::WideGlyph => "wide_glyph",
            Setting::PasteConfirm => "confirm",
            Setting::ScrollBarWidth => "width",
        }
    }

    /// `config` でのこの項目の値。書かないことを表す値（`shell` の `None`）なら `None`。
    fn value(self, config: &Config) -> Option<Value> {
        match self {
            Setting::Shell => config.shell.as_deref().map(Value::from),
            Setting::FontFamily => Some(families_value(&config.font.families)),
            Setting::FontSize => Some(size_value(config.font.size_pt)),
            Setting::WideGlyph => Some(Value::from(config.font.wide_glyph.name())),
            Setting::PasteConfirm => Some(Value::from(config.paste.confirm.name())),
            Setting::ScrollBarWidth => Some(Value::from(i64::from(config.scroll_bar.width))),
        }
    }
}

/// フォントの名前の並び。一つなら文字列、二つ以上なら配列。
fn families_value(families: &[String]) -> Value {
    match families {
        [family] => Value::from(family.as_str()),
        _ => Value::Array(families.iter().map(String::as_str).collect::<Array>()),
    }
}

/// フォントの大きさ。整数なら整数で、そうでなければ小数で書く（`f32` を `f64` に広げた端数を出さない
/// ように、`f32` の短い表記から読み直す）。
fn size_value(size_pt: f32) -> Value {
    if size_pt.fract() == 0.0 {
        return Value::from(size_pt as i64);
    }
    let size: f64 = size_pt.to_string().parse().unwrap_or(f64::from(size_pt));
    Value::from(size)
}

/// 値を書いたときの文字列。前後の空白やコメントは含めない。
fn bare_repr(value: &Value) -> String {
    let mut value = value.clone();
    value.decor_mut().clear();
    value.to_string()
}

fn same(a: &Option<Value>, b: &Option<Value>) -> bool {
    a.as_ref().map(bare_repr) == b.as_ref().map(bare_repr)
}

/// 設定ファイルの中身 `text` を、`base` から `edited` へ変えた項目だけ書き換えて返す。`base` は
/// 編集を始めたときの設定。TOML として読めなければ、その誤りの文を返す。
///
/// 先頭の BOM と、改行が CRLF かどうかは `text` に合わせる。
pub fn update(text: &str, base: &Config, edited: &Config) -> Result<String, String> {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(body) => ("\u{feff}", body),
        None => ("", text),
    };
    let crlf = body.contains("\r\n");
    let mut doc: DocumentMut = body
        .parse()
        .map_err(|e: toml_edit::TomlError| e.message().trim_end().to_owned())?;
    let defaults = Config::default();
    for setting in Setting::ALL {
        let new = setting.value(edited);
        if same(&setting.value(base), &new) {
            continue;
        }
        let default = setting.value(&defaults);
        match new {
            Some(value) if !same(&Some(value.clone()), &default) => {
                set(&mut doc, setting.section(), setting.key(), value)?;
            }
            _ => reset(&mut doc, setting.section(), setting.key(), default)?,
        }
    }
    let mut out = doc.to_string();
    if crlf {
        out = out.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    Ok(format!("{bom}{out}"))
}

/// 表のうち、ほかの表の見出しとファイルの最後との位置関係で扱えるもの（`[font]` の形の見出しを
/// 持つ表と最上位）か。`font.size = 9` のような点で区切ったキーや、インラインの表は違う。
fn is_standard(table: &Table) -> bool {
    !table.is_dotted()
}

/// `section` の表。無ければ、`create` なら最後に作る。表でないものが書いてあれば誤り。
fn section_mut<'a>(
    doc: &'a mut DocumentMut,
    section: Option<&str>,
    create: bool,
) -> Result<Option<&'a mut Table>, String> {
    let Some(section) = section else {
        return Ok(Some(doc.as_table_mut()));
    };
    if !doc.contains_key(section) {
        if !create {
            return Ok(None);
        }
        let last = tables(doc)
            .iter()
            .filter_map(|(_, p)| *p)
            .max()
            .unwrap_or(0);
        let mut table = Table::new();
        table.set_position(Some(last + 1));
        doc.insert(section, Item::Table(table));
    }
    match doc.get_mut(section) {
        Some(Item::Table(table)) => Ok(Some(table)),
        _ => Err(format!("{section} is not a table")),
    }
}

/// 最上位に書いた表の名前と、ファイルの中の位置。
fn tables(doc: &DocumentMut) -> Vec<(String, Option<isize>)> {
    doc.iter()
        .filter_map(|(name, item)| {
            let table = item.as_table()?;
            is_standard(table).then(|| (name.to_owned(), table.position()))
        })
        .collect()
}

/// 表の値の行のあとに続く場所。次の表の見出しの前か、ファイルの最後。
#[derive(Debug)]
enum Following {
    Table(String),
    End,
}

/// `section` の表の最後の値の行のあとに続く場所。
fn following(doc: &DocumentMut, section: Option<&str>) -> Following {
    let after = match section {
        None => None,
        Some(name) => doc
            .get(name)
            .and_then(Item::as_table)
            .and_then(Table::position),
    };
    tables(doc)
        .into_iter()
        .filter(|(name, position)| Some(name.as_str()) != section && position.is_some())
        .filter(|(_, position)| after.is_none_or(|after| position.is_some_and(|p| p > after)))
        .min_by_key(|(_, position)| *position)
        .map_or(Following::End, |(name, _)| Following::Table(name))
}

fn following_prefix(doc: &DocumentMut, following: &Following) -> String {
    match following {
        Following::Table(name) => doc
            .get(name)
            .and_then(Item::as_table)
            .map(|table| decor_prefix(table.decor()))
            .unwrap_or_default(),
        Following::End => doc.trailing().as_str().unwrap_or_default().to_owned(),
    }
}

fn set_following_prefix(doc: &mut DocumentMut, following: &Following, prefix: String) {
    match following {
        Following::Table(name) => {
            if let Some(Item::Table(table)) = doc.get_mut(name) {
                table.decor_mut().set_prefix(prefix);
            }
        }
        Following::End => doc.set_trailing(prefix),
    }
}

fn decor_prefix(decor: &Decor) -> String {
    decor
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .unwrap_or_default()
        .to_owned()
}

fn decor_suffix(decor: &Decor) -> String {
    decor
        .suffix()
        .and_then(|suffix| suffix.as_str())
        .unwrap_or_default()
        .to_owned()
}

/// 表の値の行のキーと、その行の前に書いたもの（コメントと空行）。行の順。
fn value_prefixes(table: &Table) -> Vec<(String, String)> {
    table
        .iter()
        .filter(|(_, item)| item.is_value())
        .filter_map(|(name, _)| {
            let key = table.key(name)?;
            Some((name.to_owned(), decor_prefix(key.leaf_decor())))
        })
        .collect()
}

/// `text` の中の、`key` の値をコメントにした行（`# key = …`）を探し、その行の前と後ろに分ける。
fn split_at_commented(text: &str, key: &str) -> Option<(String, String)> {
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        if is_commented_setting(line, key) {
            let end = start + line.len();
            return Some((text[..start].to_owned(), text[end..].to_owned()));
        }
        start += line.len();
    }
    None
}

fn is_commented_setting(line: &str, key: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix('#') else {
        return false;
    };
    let Some(rest) = rest.trim_start().strip_prefix(key) else {
        return false;
    };
    rest.trim_start().starts_with('=')
}

/// `key` の値を `value` にする。
fn set(
    doc: &mut DocumentMut,
    section: Option<&str>,
    key: &str,
    value: Value,
) -> Result<(), String> {
    let table = section_mut(doc, section, true)?.expect("created");
    if let Some(item) = table.get_mut(key) {
        let mut value = value;
        if let Some(old) = item.as_value() {
            *value.decor_mut() = old.decor().clone();
        }
        *item = Item::Value(value);
        return Ok(());
    }
    if !is_standard(table) {
        table.insert(key, Item::Value(value));
        return Ok(());
    }
    let prefixes = value_prefixes(table);
    let found = prefixes
        .iter()
        .enumerate()
        .find_map(|(index, (_, prefix))| {
            split_at_commented(prefix, key).map(|split| (index, split))
        });
    if let Some((index, (before, after))) = found {
        let (next, _) = &prefixes[index];
        if let Some(mut next_key) = table.key_mut(next) {
            next_key.leaf_decor_mut().set_prefix(after);
        }
        insert_at(table, index, key_with_prefix(key, before), value);
        return Ok(());
    }
    let following = following(doc, section);
    let after_values = following_prefix(doc, &following);
    let table = section_mut(doc, section, false)?.expect("exists");
    match split_at_commented(&after_values, key) {
        Some((before, after)) => {
            table.insert_formatted(&key_with_prefix(key, before), Item::Value(value));
            set_following_prefix(doc, &following, after);
        }
        None if section.is_none() && prefixes.is_empty() => {
            // 最上位の値の行は表の見出しより前に出るので、ファイルの先頭のコメントより上に来ないよう、
            // 空行で見出しと離れたところまでをこの行の前に移す。
            let (before, after) = split_after_last_blank_line(&after_values);
            table.insert_formatted(&key_with_prefix(key, before), Item::Value(value));
            set_following_prefix(doc, &following, after);
        }
        None => {
            table.insert(key, Item::Value(value));
        }
    }
    Ok(())
}

/// `text` を、最後の空行までと、その後ろに分ける。空行がなければ、すべてを後ろにする。
fn split_after_last_blank_line(text: &str) -> (String, String) {
    let mut end = 0;
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        start += line.len();
        if line.trim().is_empty() {
            end = start;
        }
    }
    (text[..end].to_owned(), text[end..].to_owned())
}

fn key_with_prefix(key: &str, prefix: String) -> Key {
    Key::new(key).with_leaf_decor(Decor::new(prefix, " "))
}

/// 表の値の行の `index` 番目の前に、`key` の行を入れる。
fn insert_at(table: &mut Table, index: usize, key: Key, value: Value) {
    let later: Vec<String> = value_prefixes(table)
        .into_iter()
        .skip(index)
        .map(|(name, _)| name)
        .collect();
    let moved: Vec<(Key, Item)> = later
        .iter()
        .filter_map(|name| table.remove_entry(name))
        .collect();
    table.insert_formatted(&key, Item::Value(value));
    for (key, item) in moved {
        table.insert_formatted(&key, item);
    }
}

/// `key` を既定値に戻す。書いてあれば、その行を `# key = <default>` のコメントの行にする。`default`
/// が `None` なら、それまでの値をコメントにする。
fn reset(
    doc: &mut DocumentMut,
    section: Option<&str>,
    key: &str,
    default: Option<Value>,
) -> Result<(), String> {
    let Some(table) = section_mut(doc, section, false)? else {
        return Ok(());
    };
    let Some(item) = table.get(key) else {
        return Ok(());
    };
    let Some(old) = item.as_value().cloned() else {
        table.remove(key);
        return Ok(());
    };
    if !is_standard(table) {
        table.remove(key);
        return Ok(());
    }
    let prefixes = value_prefixes(table);
    let index = prefixes
        .iter()
        .position(|(name, _)| name == key)
        .expect("the key is a value");
    let (removed, _) = table.remove_entry(key).expect("the key exists");
    let shown = bare_repr(default.as_ref().unwrap_or(&old));
    let line = format!(
        "{}# {key} = {shown}{}\n",
        decor_prefix(removed.leaf_decor()),
        decor_suffix(old.decor()),
    );
    if let Some((next, prefix)) = prefixes.get(index + 1) {
        if let Some(mut next_key) = table.key_mut(next) {
            next_key
                .leaf_decor_mut()
                .set_prefix(format!("{line}{prefix}"));
        }
        return Ok(());
    }
    let following = following(doc, section);
    let after_values = following_prefix(doc, &following);
    set_following_prefix(doc, &following, format!("{line}{after_values}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        self, FontConfig, PasteConfig, PasteConfirm, ScrollBarConfig, TEMPLATE, WideGlyph,
    };

    fn families(names: &[&str]) -> Vec<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }

    /// 既定値とどの項目も違う設定。
    fn customized() -> Config {
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
            scroll_bar: ScrollBarConfig { width: 20 },
        }
    }

    /// 書いた結果を読むと、警告なしで `expected` になる。
    fn assert_reads_as(text: &str, expected: &Config) {
        let parsed = config::parse(text, &Config::default());
        assert!(parsed.warnings.is_empty(), "{:?}\n{text}", parsed.warnings);
        assert_eq!(&parsed.config, expected, "{text}");
    }

    #[test]
    fn settings_replace_the_commented_lines_of_the_template() {
        let edited = customized();
        let text = update(TEMPLATE, &Config::default(), &edited).unwrap();
        assert_reads_as(&text, &edited);
        // 説明の行はそのまま、コメントにした行が値の行になる。
        let expected = TEMPLATE
            .replace(
                r"# shell = 'C:\cygwin64\bin\zsh.exe -l'",
                r"shell = 'C:\cygwin64\bin\zsh.exe -l'",
            )
            .replace(
                r#"# family = ["Bizin Gothic", "BIZ UDGothic", "Consolas"]"#,
                r#"family = ["Cascadia Mono", "Consolas"]"#,
            )
            .replace("# size = 12", "size = 10.5")
            .replace(r#"# wide_glyph = "shrink""#, r#"wide_glyph = "overflow""#)
            .replace(r#"# confirm = "unsafe""#, r#"confirm = "never""#)
            .replace("# width = 12", "width = 20");
        assert_eq!(text, expected);
    }

    #[test]
    fn resetting_to_the_defaults_gives_back_the_template() {
        let edited = customized();
        let text = update(TEMPLATE, &Config::default(), &edited).unwrap();
        let back = update(&text, &edited, &Config::default()).unwrap();
        // shell は既定値がないので、それまでの値をコメントにする（ひな形の例と同じ値）。
        assert_eq!(back, TEMPLATE);

        // 一つずつ戻しても同じ。
        let mut current = edited.clone();
        let mut text = text;
        let defaults = Config::default();
        let steps: [fn(&mut Config, &Config); 6] = [
            |c, d| c.font.size_pt = d.font.size_pt,
            |c, d| c.shell = d.shell.clone(),
            |c, d| c.scroll_bar = d.scroll_bar.clone(),
            |c, d| c.font.families = d.font.families.clone(),
            |c, d| c.paste = d.paste.clone(),
            |c, d| c.font.wide_glyph = d.font.wide_glyph,
        ];
        for step in steps {
            let mut next = current.clone();
            step(&mut next, &defaults);
            text = update(&text, &current, &next).unwrap();
            assert_reads_as(&text, &next);
            current = next;
        }
        assert_eq!(text, TEMPLATE);
    }

    #[test]
    fn only_changed_settings_are_written() {
        let text = "# mine\nshell = \"pwsh\"  # my shell\n\n[font]\n# big\nsize = 16 # mine\nfamily = 'Consolas'\nweight = 'bold'\n\n[tabs]\nx = 1\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited.font.size_pt = 14.0;
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            text.replace("size = 16 # mine", "size = 14 # mine")
        );
        // 変えていなければ、そのまま。
        assert_eq!(update(text, &base, &base).unwrap(), text);
    }

    #[test]
    fn missing_settings_and_tables_are_added() {
        let text = "# only a comment\n[font]\nsize = 9\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited.font.wide_glyph = WideGlyph::Clip;
        edited.scroll_bar.width = 16;
        edited.shell = Some("cmd.exe".to_owned());
        let updated = update(text, &base, &edited).unwrap();
        // 見出しのすぐ上のコメントは、その表のものとして見出しの上に残す。
        assert_eq!(
            updated,
            "shell = \"cmd.exe\"\n# only a comment\n[font]\nsize = 9\nwide_glyph = \"clip\"\n\n[scroll_bar]\nwidth = 16\n"
        );
        assert_reads_as(&updated, &edited);

        // 空行で見出しと離れたコメントは、ファイルの先頭のものとして最上位の項目の上に置く。
        let text = "# head\n\n# about font\n[font]\nsize = 9\n";
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "# head\n\nshell = \"cmd.exe\"\n# about font\n[font]\nsize = 9\nwide_glyph = \"clip\"\n\n[scroll_bar]\nwidth = 16\n"
        );

        let updated = update("", &Config::default(), &edited).unwrap();
        assert_reads_as(&updated, &edited);
    }

    #[test]
    fn resetting_keeps_the_comments_above() {
        let text = "[font]\n# my favorite\nsize = 9\n# then\nfamily = 'Consolas'\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited.font.size_pt = config::DEFAULT_FONT_SIZE_PT;
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "[font]\n# my favorite\n# size = 12\n# then\nfamily = 'Consolas'\n"
        );
        assert_reads_as(&updated, &edited);

        // 最後の項目なら、続く表の見出しの前に置く。
        let text = "[font]\nsize = 9\n\n[paste]\nconfirm = 'never'\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited.font.size_pt = config::DEFAULT_FONT_SIZE_PT;
        edited.paste.confirm = PasteConfirm::Unsafe;
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "[font]\n# size = 12\n\n[paste]\n# confirm = \"unsafe\"\n"
        );
    }

    #[test]
    fn line_endings_and_byte_order_mark_are_kept() {
        let text = "\u{feff}# a\r\n[font]\r\nsize = 9\r\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited.font.size_pt = 11.0;
        edited.font.wide_glyph = WideGlyph::Clip;
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "\u{feff}# a\r\n[font]\r\nsize = 11\r\nwide_glyph = \"clip\"\r\n"
        );
    }

    #[test]
    fn sizes_are_written_without_widening_errors() {
        assert_eq!(bare_repr(&size_value(12.0)), "12");
        assert_eq!(bare_repr(&size_value(10.1)), "10.1");
        assert_eq!(bare_repr(&size_value(10.5)), "10.5");
    }

    #[test]
    fn single_family_is_a_string() {
        assert_eq!(
            bare_repr(&families_value(&families(&["Consolas"]))),
            "\"Consolas\""
        );
    }

    #[test]
    fn unreadable_files_are_not_rewritten() {
        let edited = customized();
        assert!(update("[font\n", &Config::default(), &edited).is_err());
        assert_eq!(
            update("font = 1\n", &Config::default(), &edited),
            Err("font is not a table".to_owned())
        );
    }
}
