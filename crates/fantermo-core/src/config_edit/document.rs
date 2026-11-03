//! 設定ファイルの `toml_edit` の文書を書き換える、ほかのモジュールが使う共通の処理。
//!
//! 文書を読み書きする [`Document`]、テーブルとその位置を探す処理、コメントと空行を保ったまま項目の行を
//! 書く（[`set`]）・既定値に戻す（[`reset`]）・置き換える（[`replace_line`]）処理を持つ。

use toml_edit::{Decor, DocumentMut, Item, Key, Table, Value};

/// 書き換える設定ファイルの中身。先頭の BOM と、改行が CRLF かどうかを覚えておき、書き出すときに戻す。
pub(crate) struct Document {
    pub(crate) doc: DocumentMut,
    bom: bool,
    crlf: bool,
}

impl Document {
    /// `text` を読む。TOML として読めなければ、そのエラーメッセージを返す。
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let (bom, body) = match text.strip_prefix('\u{feff}') {
            Some(body) => (true, body),
            None => (false, text),
        };
        let doc = body
            .parse()
            .map_err(|e: toml_edit::TomlError| e.message().trim_end().to_owned())?;
        Ok(Self {
            doc,
            bom,
            crlf: body.contains("\r\n"),
        })
    }

    /// 書き出す中身。BOM と改行は読んだ中身に合わせる。
    pub(crate) fn finish(self) -> String {
        let mut out = self.doc.to_string();
        if self.crlf {
            out = out.replace("\r\n", "\n").replace('\n', "\r\n");
        }
        if self.bom {
            out.insert(0, '\u{feff}');
        }
        out
    }
}

/// フォントの大きさや画像の不透明度の数。整数なら整数で、そうでなければ小数で書く（`f32` を `f64` に
/// 広げた端数を出さないように、`f32` の短い表記から読み直す）。
pub(super) fn number_value(number: f32) -> Value {
    if number.fract() == 0.0 {
        return Value::from(number as i64);
    }
    let widened: f64 = number.to_string().parse().unwrap_or(f64::from(number));
    Value::from(widened)
}

/// 値を書いたときの文字列。前後の空白やコメントは含めない。
pub(super) fn bare_repr(value: &Value) -> String {
    let mut value = value.clone();
    value.decor_mut().clear();
    value.to_string()
}

pub(super) fn same(a: &Option<Value>, b: &Option<Value>) -> bool {
    a.as_ref().map(bare_repr) == b.as_ref().map(bare_repr)
}

/// 書き換える項目の、テーブルの中のキー。
pub(super) struct Target<'a> {
    /// 足すときに書くキーの名前。
    pub(super) name: String,
    /// テーブルのキーの名前が、この項目のものか。
    pub(super) matches: &'a dyn Fn(&str) -> bool,
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
pub(super) enum Reset {
    /// 既定値をコメントにした行にする。
    Comment(Value),
    /// それまでの値をコメントにした行にする。
    CommentOld,
    /// 行を消す（上に書いた説明のコメントは残す）。
    Remove,
}

/// テーブル `table` の値の行 `key` を消す。その行の前に書いたコメントは、次の値の行の前に移す（次の
/// 値の行がなければ消す）。
pub(super) fn remove_table_value(table: &mut Table, key: &str) {
    let prefixes = value_prefixes(table);
    let Some(index) = prefixes.iter().position(|(name, _)| name == key) else {
        table.remove(key);
        return;
    };
    let Some((removed, _)) = table.remove_entry(key) else {
        return;
    };
    let kept = decor_prefix(removed.leaf_decor());
    if let Some((next, prefix)) = prefixes.get(index + 1)
        && let Some(mut next_key) = table.key_mut(next)
    {
        next_key
            .leaf_decor_mut()
            .set_prefix(format!("{kept}{prefix}"));
    }
}

/// テーブル `table` の `key` を、`text` があればその文字列に、なければ消す。
pub(super) fn set_or_remove_table_text(table: &mut Table, key: &str, text: Option<&str>) {
    match text {
        Some(text) => set_table_text(table, key, text),
        None => {
            table.remove(key);
        }
    }
}

/// テーブル `table` の `key` の値を文字列 `text` にする。同じなら（引用符の種類も）変えない。行の
/// コメントは残す。
pub(super) fn set_table_text(table: &mut Table, key: &str, text: &str) {
    if table.get(key).and_then(Item::as_str) == Some(text) {
        return;
    }
    set_table_value(table, key, Value::from(text));
}

/// テーブル `table` の `key` の値を `value` にする。書いてあれば行のコメントは残し、なければテーブルの
/// 最後に足す。
pub(super) fn set_table_value(table: &mut Table, key: &str, mut value: Value) {
    match table.get_mut(key) {
        Some(item) => {
            if let Some(old) = item.as_value() {
                *value.decor_mut() = old.decor().clone();
            }
            *item = Item::Value(value);
        }
        None => {
            table.insert(key, Item::Value(value));
        }
    }
}

/// テーブルのうち、ほかのテーブルのヘッダーとファイルの最後との位置関係で扱えるもの（`[font]` の形の
/// ヘッダーを持つテーブルと最上位）か。`font.size = 9` のような点で区切ったキーや、インラインテーブルは
/// 違う。
pub(super) fn is_standard(table: &Table) -> bool {
    !table.is_dotted()
}

fn path_name(path: &[&str]) -> String {
    path.join(".")
}

/// キーのパス `path` のテーブル。
pub(super) fn table_at<'a>(doc: &'a DocumentMut, path: &[&str]) -> Option<&'a Table> {
    let mut table = doc.as_table();
    for name in path {
        table = table.get(name)?.as_table()?;
    }
    Some(table)
}

pub(super) fn table_at_mut<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Option<&'a mut Table> {
    let mut table = doc.as_table_mut();
    for name in path {
        table = table.get_mut(name)?.as_table_mut()?;
    }
    Some(table)
}

/// キーのパス `path` のテーブル。無ければ、`create` なら最後に作る（途中のテーブルは見出しを書かない
/// テーブルにする）。テーブルでないものが書いてあればエラー。
pub(super) fn section_mut<'a>(
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
    if create && table.is_implicit() && !path.is_empty() {
        // `[[background.images]]` だけを書いたときの `background` のような、見出しのないテーブルに値を
        // 書くので、見出しを中のテーブルの最初のものの前に書く。
        let first_inner = table
            .iter()
            .flat_map(|(_, item)| match item {
                Item::Table(inner) => vec![inner.position()],
                Item::ArrayOfTables(array) => array.iter().map(Table::position).collect(),
                _ => Vec::new(),
            })
            .flatten()
            .min();
        table.set_implicit(false);
        table.set_position(first_inner.or(Some(last + 1)));
    }
    Ok(Some(table))
}

/// 見出しを書いたテーブル一つ。テーブルの配列（`[[profiles]]` など）のテーブルなら、`index` はその中の
/// 位置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Header {
    pub(super) path: Vec<String>,
    pub(super) index: Option<usize>,
}

impl Header {
    pub(super) fn table<'a>(&self, doc: &'a DocumentMut) -> Option<&'a Table> {
        let path: Vec<&str> = self.path.iter().map(String::as_str).collect();
        match self.index {
            None => table_at(doc, &path),
            Some(index) => {
                let (last, parent) = path.split_last()?;
                table_at(doc, parent)?
                    .get(last)?
                    .as_array_of_tables()?
                    .get(index)
            }
        }
    }

    fn table_mut<'a>(&self, doc: &'a mut DocumentMut) -> Option<&'a mut Table> {
        let path: Vec<&str> = self.path.iter().map(String::as_str).collect();
        match self.index {
            None => table_at_mut(doc, &path),
            Some(index) => {
                let (last, parent) = path.split_last()?;
                table_at_mut(doc, parent)?
                    .get_mut(last)?
                    .as_array_of_tables_mut()?
                    .get_mut(index)
            }
        }
    }
}

/// 見出しを書いたテーブルと、ファイルの中の位置。見出しを書かないテーブル（`[colors.tab_bar]` の
/// `colors` など）は含めず、その中のテーブルを含める。テーブルの配列は、そのテーブルを一つずつ含める。
pub(super) fn tables(doc: &DocumentMut) -> Vec<(Header, Option<isize>)> {
    let mut found = Vec::new();
    collect_tables(doc.as_table(), &mut Vec::new(), &mut found);
    found
}

fn collect_tables(table: &Table, path: &mut Vec<String>, found: &mut Vec<(Header, Option<isize>)>) {
    for (name, item) in table.iter() {
        if let Some(array) = item.as_array_of_tables() {
            for (index, inner) in array.iter().enumerate() {
                let mut path = path.clone();
                path.push(name.to_owned());
                found.push((
                    Header {
                        path,
                        index: Some(index),
                    },
                    inner.position(),
                ));
            }
            continue;
        }
        let Some(inner) = item.as_table() else {
            continue;
        };
        if !is_standard(inner) {
            continue;
        }
        path.push(name.to_owned());
        if !inner.is_implicit() {
            found.push((
                Header {
                    path: path.clone(),
                    index: None,
                },
                inner.position(),
            ));
        }
        collect_tables(inner, path, found);
        path.pop();
    }
}

/// テーブルの値の行のあとに続く場所。次のテーブルのヘッダーの前か、ファイルの最後。
#[derive(Debug)]
pub(super) enum Following {
    Table(Header),
    End,
}

/// `path` のテーブルの最後の値の行のあとに続く場所。
pub(super) fn following(doc: &DocumentMut, path: &[&str]) -> Following {
    let after = if path.is_empty() {
        None
    } else {
        table_at(doc, path).and_then(Table::position)
    };
    let me = Header {
        path: path.iter().map(|name| (*name).to_owned()).collect(),
        index: None,
    };
    first_after(doc, &me, after)
}

/// `me` のほかの、位置が `after` より後ろ（`None` ならどこでも）の、いちばん前のテーブル。`me` の中の
/// テーブル（`[background]` の中の `[[background.images]]` など）は、位置が `after` と同じでも後ろに
/// あるものとする（同じ位置のテーブルは、外のテーブル、中のテーブルの順に書き出される）。
pub(super) fn first_after(doc: &DocumentMut, me: &Header, after: Option<isize>) -> Following {
    let inside_me =
        |other: &Header| other.path.len() > me.path.len() && other.path.starts_with(&me.path);
    tables(doc)
        .into_iter()
        .filter(|(other, position)| other != me && position.is_some())
        .filter(|(other, position)| {
            after.is_none_or(|after| {
                position.is_some_and(|p| p > after || (p == after && inside_me(other)))
            })
        })
        .min_by_key(|(_, position)| *position)
        .map_or(Following::End, |(other, _)| Following::Table(other))
}

pub(super) fn following_prefix(doc: &DocumentMut, following: &Following) -> String {
    match following {
        Following::Table(header) => header
            .table(doc)
            .map(|table| decor_prefix(table.decor()))
            .unwrap_or_default(),
        Following::End => doc.trailing().as_str().unwrap_or_default().to_owned(),
    }
}

pub(super) fn set_following_prefix(doc: &mut DocumentMut, following: &Following, prefix: String) {
    match following {
        Following::Table(header) => {
            if let Some(table) = header.table_mut(doc) {
                table.decor_mut().set_prefix(prefix);
            }
        }
        Following::End => doc.set_trailing(prefix),
    }
}

pub(super) fn decor_prefix(decor: &Decor) -> String {
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

/// テーブルの値の行のキーと、その行の前に書いたもの（コメントと空行）。行の順。
pub(super) fn value_prefixes(table: &Table) -> Vec<(String, String)> {
    table
        .iter()
        .filter(|(_, item)| item.is_value())
        .filter_map(|(name, _)| {
            let key = table.key(name)?;
            Some((name.to_owned(), decor_prefix(key.leaf_decor())))
        })
        .collect()
}

/// テーブルの値の行のうち、`target` のもの。行の順。
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
pub(super) fn commented_key(line: &str) -> Option<String> {
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
pub(super) fn set(
    doc: &mut DocumentMut,
    path: &[&str],
    target: &Target,
    value: Value,
) -> Result<(), String> {
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
            // 最上位の値の行はテーブルのヘッダーより前に出るので、ファイルの先頭のコメントより上に
            // 来ないよう、空行で見出しと離れたところまでをこの行の前に移す。
            let (before, after) = split_after_last_blank_line(&after_values);
            table.insert_formatted(&key_with_prefix(target.key(), before), Item::Value(value));
            set_following_prefix(doc, &following, after);
        }
        None if existed && matches!(following, Following::End) && !path.is_empty() => {
            // ファイルの最後のテーブルなら、その後ろのコメント（ひな形の説明や既定の割り当ての一覧）は
            // このテーブルのものなので、足す行はそれより後ろに置く。
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
pub(super) fn split_after_last_blank_line(text: &str) -> (String, String) {
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

/// テーブルの値の行の `index` 番目の前に、`key` の行を入れる。
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
pub(super) fn reset(
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
        // 値でないもの（テーブルなど）が書いてあれば消す。
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

/// テーブル `path` の値の行 `name` を消し、その行の前に書いたもの（コメントと空行）と `line` を、次の
/// 値の行の前（なければテーブルの後ろ）に置く。
pub(super) fn replace_line(doc: &mut DocumentMut, path: &[&str], name: &str, line: Option<String>) {
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
