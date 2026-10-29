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
//! - `[colors]` と `[colors.…]` の色は既定の色を一つに書けないので、既定に戻した色はそれまでの値を
//!   コメントにする。
//! - `[keys]` の組み合わせは、書き方（大文字小文字、修飾キーの順）によらず同じ組み合わせの行を探す。
//!   既定の割り当てに戻した組み合わせは、既定の割り当ての `# "…" = "…"` のコメントの行にし、既定で
//!   割り当てていない組み合わせは行を消す。

use toml_edit::{Array, Decor, DocumentMut, Item, Key, Table, Value};

use crate::{
    config::{ColorTable, Config, format_color},
    keymap::{self, Action, KeyCombo},
};

/// 書き換える項目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    Shell,
    FontFamily,
    FontSize,
    WideGlyph,
    PasteConfirm,
    ScrollBarWidth,
    ColorScheme,
}

impl Setting {
    const ALL: [Setting; 7] = [
        Setting::Shell,
        Setting::FontFamily,
        Setting::FontSize,
        Setting::WideGlyph,
        Setting::PasteConfirm,
        Setting::ScrollBarWidth,
        Setting::ColorScheme,
    ];

    /// 項目を置く表の道筋。最上位なら空。
    fn path(self) -> &'static [&'static str] {
        match self {
            Setting::Shell => &[],
            Setting::FontFamily | Setting::FontSize | Setting::WideGlyph => &["font"],
            Setting::PasteConfirm => &["paste"],
            Setting::ScrollBarWidth => &["scroll_bar"],
            Setting::ColorScheme => &["colors"],
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
            Setting::ColorScheme => "scheme",
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
            Setting::ColorScheme => Some(Value::from(config.colors.scheme.name())),
        }
    }
}

/// `[keys]` の表の道筋。
const KEYS_PATH: &[&str] = &["keys"];

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

/// 組み合わせに割り当てる値。動作の名前か、外すなら [`keymap::UNBOUND`]。
fn action_value(action: Option<Action>) -> Value {
    Value::from(action.map_or(keymap::UNBOUND, Action::name))
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

/// 書き換える項目の、表の中のキー。
struct Target<'a> {
    /// 足すときに書くキーの名前。
    name: String,
    /// 表のキーの名前が、この項目のものか。
    matches: &'a dyn Fn(&str) -> bool,
}

impl Target<'_> {
    /// 足すときに書くキー。
    fn key(&self) -> Key {
        Key::new(self.name.as_str())
    }

    /// 足すときに書くキーを、ファイルに書いたときの形。
    fn repr(&self) -> String {
        self.key().display_repr().into_owned()
    }
}

/// 既定値に戻した項目の行をどうするか。
enum Reset {
    /// 既定値をコメントにした行にする。
    Comment(Value),
    /// それまでの値をコメントにした行にする。
    CommentOld,
    /// 行を消す（上に書いた説明のコメントは残す）。
    Remove,
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
        let key = setting.key();
        let matches = |name: &str| name == key;
        let target = Target {
            name: key.to_owned(),
            matches: &matches,
        };
        let default = setting.value(&defaults);
        match new {
            Some(value) if !same(&Some(value.clone()), &default) => {
                set(&mut doc, setting.path(), &target, value)?;
            }
            _ => {
                let reset = match default {
                    Some(default) => Reset::Comment(default),
                    None => Reset::CommentOld,
                };
                self::reset(&mut doc, setting.path(), &target, reset)?;
            }
        }
    }
    update_colors(&mut doc, base, edited)?;
    update_keys(&mut doc, base, edited)?;
    let mut out = doc.to_string();
    if crlf {
        out = out.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    Ok(format!("{bom}{out}"))
}

/// `[colors]` と `[colors.…]` の表で、`base` と `edited` で違う色を書き換える。既定の色に戻した色は、
/// 既定の色が配色やシステムの色などで決まり一つに書けないので、それまでの値をコメントにする。
fn update_colors(doc: &mut DocumentMut, base: &Config, edited: &Config) -> Result<(), String> {
    for table in ColorTable::ALL {
        let path = table.path();
        let old = base.colors.table(table);
        let new = edited.colors.table(table);
        for (index, key) in table.keys().iter().enumerate() {
            if old[index] == new[index] {
                continue;
            }
            let matches = |name: &str| name == *key;
            let target = Target {
                name: (*key).to_owned(),
                matches: &matches,
            };
            match new[index] {
                Some(color) => set(doc, path, &target, Value::from(format_color(color)))?,
                None => reset(doc, path, &target, Reset::CommentOld)?,
            }
        }
    }
    Ok(())
}

/// `[keys]` の表で、`base` と `edited` で割り当てが違う組み合わせを書き換える。
fn update_keys(doc: &mut DocumentMut, base: &Config, edited: &Config) -> Result<(), String> {
    let mut combos: Vec<KeyCombo> = keymap::default_combos().collect();
    for (combo, _) in edited.keys.bindings().iter().chain(base.keys.bindings()) {
        if !combos.contains(combo) {
            combos.push(*combo);
        }
    }
    for combo in combos {
        let new = edited.keys.lookup_combo(combo);
        if base.keys.lookup_combo(combo) == new {
            continue;
        }
        let matches = |name: &str| KeyCombo::parse(name) == Ok(combo);
        let target = Target {
            name: combo.to_string(),
            matches: &matches,
        };
        let default = keymap::default_action(combo);
        if new == default {
            let reset = match default {
                Some(action) => Reset::Comment(action_value(Some(action))),
                None => Reset::Remove,
            };
            self::reset(doc, KEYS_PATH, &target, reset)?;
        } else {
            set(doc, KEYS_PATH, &target, action_value(new))?;
        }
    }
    Ok(())
}

/// 表のうち、ほかの表の見出しとファイルの最後との位置関係で扱えるもの（`[font]` の形の見出しを
/// 持つ表と最上位）か。`font.size = 9` のような点で区切ったキーや、インラインの表は違う。
fn is_standard(table: &Table) -> bool {
    !table.is_dotted()
}

fn path_name(path: &[&str]) -> String {
    path.join(".")
}

/// 道筋 `path` の表。
fn table_at<'a>(doc: &'a DocumentMut, path: &[&str]) -> Option<&'a Table> {
    let mut table = doc.as_table();
    for name in path {
        table = table.get(name)?.as_table()?;
    }
    Some(table)
}

fn table_at_mut<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Option<&'a mut Table> {
    let mut table = doc.as_table_mut();
    for name in path {
        table = table.get_mut(name)?.as_table_mut()?;
    }
    Some(table)
}

/// 道筋 `path` の表。無ければ、`create` なら最後に作る（途中の表は見出しを書かない表にする）。表で
/// ないものが書いてあれば誤り。
fn section_mut<'a>(
    doc: &'a mut DocumentMut,
    path: &[&str],
    create: bool,
) -> Result<Option<&'a mut Table>, String> {
    let last = tables(doc)
        .iter()
        .filter_map(|(_, p)| *p)
        .max()
        .unwrap_or(0);
    let mut table = doc.as_table_mut();
    for (depth, name) in path.iter().enumerate() {
        if !table.contains_key(name) {
            if !create {
                return Ok(None);
            }
            let mut inner = Table::new();
            if depth + 1 == path.len() {
                inner.set_position(Some(last + 1));
            } else {
                inner.set_implicit(true);
            }
            table.insert(name, Item::Table(inner));
        }
        table = match table.get_mut(name) {
            Some(Item::Table(inner)) => inner,
            _ => return Err(format!("{} is not a table", path_name(&path[..=depth]))),
        };
    }
    Ok(Some(table))
}

/// 見出しを書いた表の道筋と、ファイルの中の位置。見出しを書かない表（`[colors.tab_bar]` の `colors`
/// など）は含めず、その中の表を含める。
fn tables(doc: &DocumentMut) -> Vec<(Vec<String>, Option<isize>)> {
    let mut found = Vec::new();
    collect_tables(doc.as_table(), &mut Vec::new(), &mut found);
    found
}

fn collect_tables(
    table: &Table,
    path: &mut Vec<String>,
    found: &mut Vec<(Vec<String>, Option<isize>)>,
) {
    for (name, item) in table.iter() {
        let Some(inner) = item.as_table() else {
            continue;
        };
        if !is_standard(inner) {
            continue;
        }
        path.push(name.to_owned());
        if !inner.is_implicit() {
            found.push((path.clone(), inner.position()));
        }
        collect_tables(inner, path, found);
        path.pop();
    }
}

/// 表の値の行のあとに続く場所。次の表の見出しの前か、ファイルの最後。
#[derive(Debug)]
enum Following {
    Table(Vec<String>),
    End,
}

/// `path` の表の最後の値の行のあとに続く場所。
fn following(doc: &DocumentMut, path: &[&str]) -> Following {
    let after = if path.is_empty() {
        None
    } else {
        table_at(doc, path).and_then(Table::position)
    };
    tables(doc)
        .into_iter()
        .filter(|(other, position)| *other != path && position.is_some())
        .filter(|(_, position)| after.is_none_or(|after| position.is_some_and(|p| p > after)))
        .min_by_key(|(_, position)| *position)
        .map_or(Following::End, |(other, _)| Following::Table(other))
}

fn following_prefix(doc: &DocumentMut, following: &Following) -> String {
    match following {
        Following::Table(path) => {
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            table_at(doc, &path)
                .map(|table| decor_prefix(table.decor()))
                .unwrap_or_default()
        }
        Following::End => doc.trailing().as_str().unwrap_or_default().to_owned(),
    }
}

fn set_following_prefix(doc: &mut DocumentMut, following: &Following, prefix: String) {
    match following {
        Following::Table(path) => {
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            if let Some(table) = table_at_mut(doc, &path) {
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

/// 表の値の行のうち、`target` のもの。行の順。
fn matching_names(table: &Table, target: &Target) -> Vec<String> {
    table
        .iter()
        .filter(|(name, item)| item.is_value() && (target.matches)(name))
        .map(|(name, _)| name.to_owned())
        .collect()
}

/// `text` の中の、`target` の値をコメントにした行（`# キー = …`）を探し、その行の前と後ろに分ける。
fn split_at_commented(text: &str, target: &Target) -> Option<(String, String)> {
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        if commented_key(line).is_some_and(|name| (target.matches)(&name)) {
            let end = start + line.len();
            return Some((text[..start].to_owned(), text[end..].to_owned()));
        }
        start += line.len();
    }
    None
}

/// 値をコメントにした行（`# キー = …`）のキーの名前。キーを引用符で囲んでいれば外す。
fn commented_key(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix('#')?;
    let (key, _) = rest.split_once('=')?;
    let key = key.trim();
    let unquoted = ['"', '\'']
        .iter()
        .find_map(|quote| key.strip_prefix(*quote)?.strip_suffix(*quote));
    let name = unquoted.unwrap_or(key);
    (!name.is_empty()).then(|| name.to_owned())
}

/// `target` の値を `value` にする。同じ項目の行が二つ以上あれば、最初の行を書き換え、ほかは消す。
fn set(doc: &mut DocumentMut, path: &[&str], target: &Target, value: Value) -> Result<(), String> {
    let existed = table_at(doc, path).is_some();
    let table = section_mut(doc, path, true)?.expect("created");
    let names = matching_names(table, target);
    if let Some((first, rest)) = names.split_first() {
        let item = table.get_mut(first).expect("the key exists");
        let mut value = value;
        if let Some(old) = item.as_value() {
            *value.decor_mut() = old.decor().clone();
        }
        *item = Item::Value(value);
        for name in rest {
            replace_line(doc, path, name, None);
        }
        return Ok(());
    }
    if !is_standard(table) {
        table.insert(&target.name, Item::Value(value));
        return Ok(());
    }
    let prefixes = value_prefixes(table);
    let found = prefixes
        .iter()
        .enumerate()
        .find_map(|(index, (_, prefix))| {
            split_at_commented(prefix, target).map(|split| (index, split))
        });
    if let Some((index, (before, after))) = found {
        let (next, _) = &prefixes[index];
        if let Some(mut next_key) = table.key_mut(next) {
            next_key.leaf_decor_mut().set_prefix(after);
        }
        insert_at(table, index, key_with_prefix(target.key(), before), value);
        return Ok(());
    }
    let following = following(doc, path);
    let after_values = following_prefix(doc, &following);
    let table = section_mut(doc, path, false)?.expect("exists");
    match split_at_commented(&after_values, target) {
        Some((before, after)) => {
            table.insert_formatted(&key_with_prefix(target.key(), before), Item::Value(value));
            set_following_prefix(doc, &following, after);
        }
        None if path.is_empty() && prefixes.is_empty() => {
            // 最上位の値の行は表の見出しより前に出るので、ファイルの先頭のコメントより上に来ないよう、
            // 空行で見出しと離れたところまでをこの行の前に移す。
            let (before, after) = split_after_last_blank_line(&after_values);
            table.insert_formatted(&key_with_prefix(target.key(), before), Item::Value(value));
            set_following_prefix(doc, &following, after);
        }
        None if existed && matches!(following, Following::End) && !path.is_empty() => {
            // ファイルの最後の表なら、その後ろのコメント（ひな形の説明や既定の割り当ての一覧）は
            // この表のものなので、足す行はそれより後ろに置く。
            table.insert_formatted(
                &key_with_prefix(target.key(), after_values),
                Item::Value(value),
            );
            set_following_prefix(doc, &following, String::new());
        }
        None => {
            table.insert_formatted(&target.key(), Item::Value(value));
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

fn key_with_prefix(key: Key, prefix: String) -> Key {
    key.with_leaf_decor(Decor::new(prefix, " "))
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

/// `target` を既定値に戻す。書いてあれば、最初の行を `reset` のとおりにし、同じ項目のほかの行は消す。
fn reset(
    doc: &mut DocumentMut,
    path: &[&str],
    target: &Target,
    reset: Reset,
) -> Result<(), String> {
    let Some(table) = section_mut(doc, path, false)? else {
        return Ok(());
    };
    let names = matching_names(table, target);
    let Some((first, rest)) = names.split_first() else {
        // 値でないもの（表など）が書いてあれば消す。
        if table.contains_key(&target.name) {
            table.remove(&target.name);
        }
        return Ok(());
    };
    if !is_standard(table) {
        for name in &names {
            table.remove(name);
        }
        return Ok(());
    }
    let old = table
        .get(first)
        .and_then(Item::as_value)
        .cloned()
        .expect("the key is a value");
    let line = match reset {
        Reset::Comment(default) => Some(comment_line(&target.repr(), &default, &old)),
        Reset::CommentOld => {
            let repr = table
                .key(first)
                .map_or_else(|| target.repr(), |key| key.display_repr().into_owned());
            Some(comment_line(&repr, &old, &old))
        }
        Reset::Remove => None,
    };
    for name in rest {
        replace_line(doc, path, name, None);
    }
    replace_line(doc, path, first, line);
    Ok(())
}

/// `# key = value` の行。行末のコメントは `old` のものを残す。
fn comment_line(key: &str, value: &Value, old: &Value) -> String {
    format!(
        "# {key} = {}{}\n",
        bare_repr(value),
        decor_suffix(old.decor())
    )
}

/// 表 `path` の値の行 `name` を消し、その行の前に書いたもの（コメントと空行）と `line` を、次の値の
/// 行の前（なければ表の後ろ）に置く。
fn replace_line(doc: &mut DocumentMut, path: &[&str], name: &str, line: Option<String>) {
    let Some(table) = table_at_mut(doc, path) else {
        return;
    };
    let prefixes = value_prefixes(table);
    let Some(index) = prefixes.iter().position(|(other, _)| other == name) else {
        return;
    };
    let Some((removed, _)) = table.remove_entry(name) else {
        return;
    };
    let kept = format!(
        "{}{}",
        decor_prefix(removed.leaf_decor()),
        line.unwrap_or_default()
    );
    if let Some((next, prefix)) = prefixes.get(index + 1) {
        if let Some(mut next_key) = table.key_mut(next) {
            next_key
                .leaf_decor_mut()
                .set_prefix(format!("{kept}{prefix}"));
        }
        return;
    }
    let following = following(doc, path);
    let after_values = following_prefix(doc, &following);
    set_following_prefix(doc, &following, format!("{kept}{after_values}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{
            self, ColorScheme, ColorsConfig, FontConfig, PasteConfig, PasteConfirm,
            ScrollBarConfig, TEMPLATE, WideGlyph,
        },
        frame::Rgb,
        keymap::Keymap,
    };

    const RED: Rgb = Rgb { r: 255, g: 0, b: 0 };
    const BLUE: Rgb = Rgb { r: 0, g: 0, b: 255 };

    #[test]
    fn colors_replace_the_commented_lines_and_are_commented_out_when_reset() {
        let mut edited = Config::default();
        edited.colors.tab_bar[0] = Some(RED);
        edited.colors.scroll_bar[1] = Some(BLUE);
        let text = update(TEMPLATE, &Config::default(), &edited).unwrap();
        assert_reads_as(&text, &edited);
        assert_eq!(
            text,
            TEMPLATE
                .replace("# bar = \"#cccccc\"", "bar = \"#ff0000\"")
                .replace("# thumb = \"#595959\"", "thumb = \"#0000ff\"")
        );
        // 既定の色に戻すと、それまでの値をコメントにする。
        let back = update(&text, &edited, &Config::default()).unwrap();
        assert_reads_as(&back, &Config::default());
        assert_eq!(
            back,
            TEMPLATE
                .replace("# bar = \"#cccccc\"", "# bar = \"#ff0000\"")
                .replace("# thumb = \"#595959\"", "# thumb = \"#0000ff\"")
        );
    }

    #[test]
    fn scheme_and_terminal_colors_replace_the_commented_lines_of_the_template() {
        let mut edited = Config::default();
        edited.colors.scheme = ColorScheme::Light;
        edited.colors.terminal[1] = Some(BLUE);
        let text = update(TEMPLATE, &Config::default(), &edited).unwrap();
        assert_reads_as(&text, &edited);
        assert_eq!(
            text,
            TEMPLATE
                .replace("# scheme = \"dark\"", "scheme = \"light\"")
                .replace("# foreground = \"#ffffff\"", "foreground = \"#0000ff\"")
        );
        // 配色は既定のコメントの行に、色はそれまでの値をコメントにした行に戻す。
        let back = update(&text, &edited, &Config::default()).unwrap();
        assert_reads_as(&back, &Config::default());
        assert_eq!(
            back,
            TEMPLATE.replace("# foreground = \"#ffffff\"", "# foreground = \"#0000ff\"")
        );
    }

    #[test]
    fn scheme_is_added_before_the_color_tables() {
        let mut edited = Config::default();
        edited.colors.scheme = ColorScheme::Light;
        edited.colors.terminal[0] = Some(RED);
        // `[colors]` の見出しのないファイル（`[colors.tab_bar]` だけ）。
        let text =
            "[font]\nsize = 9\n\n[colors.tab_bar]\nbar = '#000000'\n\n[paste]\nconfirm = 'never'\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited_from_text = base.clone();
        edited_from_text.colors.scheme = edited.colors.scheme;
        edited_from_text.colors.terminal = edited.colors.terminal;
        let updated = update(text, &base, &edited_from_text).unwrap();
        assert_reads_as(&updated, &edited_from_text);
        assert_eq!(
            updated,
            "[font]\nsize = 9\n\n[colors]\nscheme = \"light\"\nbackground = \"#ff0000\"\n\n[colors.tab_bar]\nbar = '#000000'\n\n[paste]\nconfirm = 'never'\n"
        );
        // 何もないファイル。
        let updated = update("", &Config::default(), &edited).unwrap();
        assert_reads_as(&updated, &edited);
        assert_eq!(
            updated,
            "[colors]\nscheme = \"light\"\nbackground = \"#ff0000\"\n"
        );
    }

    #[test]
    fn color_tables_are_added_without_a_colors_header() {
        let mut edited = Config::default();
        edited.colors.scroll_bar[0] = Some(RED);
        let updated = update("[font]\nsize = 12\n", &Config::default(), &edited).unwrap();
        assert_eq!(
            updated,
            "[font]\nsize = 12\n\n[colors.scroll_bar]\ntrack = \"#ff0000\"\n"
        );
        let updated = update("", &Config::default(), &edited).unwrap();
        assert_eq!(updated, "[colors.scroll_bar]\ntrack = \"#ff0000\"\n");
        assert_reads_as(&updated, &edited);
    }

    #[test]
    fn nested_tables_keep_their_place_between_other_tables() {
        let text =
            "[font]\nsize = 9\n\n[colors.tab_bar]\nbar = '#000000'\n\n[paste]\nconfirm = 'never'\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited.colors.tab_bar[1] = Some(RED);
        edited.font.wide_glyph = WideGlyph::Clip;
        edited.colors.tab_bar[0] = None;
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "[font]\nsize = 9\nwide_glyph = \"clip\"\n\n[colors.tab_bar]\nactive_tab = \"#ff0000\"\n# bar = '#000000'\n\n[paste]\nconfirm = 'never'\n"
        );
        assert_reads_as(&updated, &edited);
    }

    fn families(names: &[&str]) -> Vec<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }

    fn combo(text: &str) -> KeyCombo {
        KeyCombo::parse(text).unwrap()
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
            // 色は既定に戻すとひな形と違う行になるので、色の書き戻しは別に確かめる。
            colors: ColorsConfig::default(),
            keys: {
                let mut keys = Keymap::default();
                keys.bind(combo("ctrl+tab"), None);
                keys.bind(combo("ctrl+page_down"), Some(Action::NextTab));
                keys
            },
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
        // 説明の行はそのまま、コメントにした行が値の行になる。足した組み合わせは表の最後。
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
            .replace("# width = 12", "width = 20")
            .replace(
                "\n# \"ctrl+tab\" = \"next_tab\"",
                "\n\"ctrl+tab\" = \"none\"",
            )
            + "\"ctrl+page_down\" = \"next_tab\"\n";
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
        let steps: [fn(&mut Config, &Config); 7] = [
            |c, d| c.font.size_pt = d.font.size_pt,
            |c, d| c.shell = d.shell.clone(),
            |c, _| c.keys.bind(combo("ctrl+tab"), Some(Action::NextTab)),
            |c, d| c.scroll_bar = d.scroll_bar.clone(),
            |c, d| c.font.families = d.font.families.clone(),
            |c, d| c.keys = d.keys.clone(),
            |c, d| {
                c.paste = d.paste.clone();
                c.font.wide_glyph = d.font.wide_glyph;
            },
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
    fn keys_are_found_however_they_are_written() {
        let text = "[keys]\n\"Shift + Ctrl + T\" = 'close_tab' # mine\n\"ctrl+page_down\" = \"next_tab\"\n'CTRL+PAGE_DOWN' = \"prev_tab\"\n";
        let base = config::parse(text, &Config::default()).config;
        assert_eq!(
            base.keys.lookup_combo(combo("ctrl+page_down")),
            Some(Action::NextTab)
        );
        let mut edited = base.clone();
        edited.keys.bind(combo("ctrl+shift+t"), None);
        edited
            .keys
            .bind(combo("ctrl+page_down"), Some(Action::PrevTab));
        let updated = update(text, &base, &edited).unwrap();
        // 書いてある行の値だけを替え、同じ組み合わせの二つ目の行は消す。
        assert_eq!(
            updated,
            "[keys]\n\"Shift + Ctrl + T\" = \"none\" # mine\n\"ctrl+page_down\" = \"prev_tab\"\n"
        );
        assert_reads_as(&updated, &edited);

        // 既定に戻すと、既定の割り当てがあればそのコメントの行に、なければ行を消す。
        let back = update(&updated, &edited, &Config::default()).unwrap();
        assert_eq!(back, "[keys]\n# \"ctrl+shift+t\" = \"new_tab\" # mine\n");
        assert_reads_as(&back, &Config::default());
    }

    #[test]
    fn keys_table_is_added_when_missing() {
        let text = "[font]\nsize = 9\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited.keys.bind(combo("ctrl+tab"), None);
        edited.keys.bind(combo("alt+t"), Some(Action::NewTab));
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "[font]\nsize = 9\n\n[keys]\n\"ctrl+tab\" = \"none\"\n\"alt+t\" = \"new_tab\"\n"
        );
        assert_reads_as(&updated, &edited);
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
    fn commented_keys_are_read_with_or_without_quotes() {
        assert_eq!(commented_key("# size = 12\n"), Some("size".to_owned()));
        assert_eq!(commented_key("  #size=12"), Some("size".to_owned()));
        assert_eq!(
            commented_key("# \"ctrl+tab\" = \"next_tab\"\n"),
            Some("ctrl+tab".to_owned())
        );
        assert_eq!(
            commented_key("# 'ctrl+tab' = 'none'"),
            Some("ctrl+tab".to_owned())
        );
        assert_eq!(
            commented_key("# 例: \"ctrl+tab\" = \"none\""),
            Some("例: \"ctrl+tab\"".to_owned())
        );
        assert_eq!(commented_key("# 説明の行。"), None);
        assert_eq!(commented_key("size = 12"), None);
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
