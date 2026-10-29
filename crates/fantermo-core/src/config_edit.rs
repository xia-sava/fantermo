//! 設定ファイルの中身を、変えた項目だけ書き換える。
//!
//! コメント、空行、項目の並び、知らないキーはそのまま残す。書き換えるのは、前の設定と新しい設定で値が
//! 違う項目だけ。
//!
//! - 書いてある項目は、値だけを書き換える（行末のコメントは残す）。
//! - 書いていない項目は、その表の中に `# キー = …` とコメントにした行があれば、その行を置き換えて
//!   書く（[`TEMPLATE`](crate::config::TEMPLATE) から作ったファイルで、説明の行の下に入る）。なければ
//!   表の最後に足す。表がなければ、ファイルの最後に表ごと足す。
//! - 既定値に戻した項目は、その行を既定値の `# キー = …` のコメントの行にする（`default_profile` は
//!   既定値を一つに書けないので、それまでの値をコメントにする）。上に書いた説明のコメントは残る。
//! - プロファイル（`[[profiles]]`）は、名前で照合して（一致しなければコマンドラインが同じ表と）変えた
//!   値だけを書き換え、なくなった表を消し、新しい表を作って、設定の順に並べる。表がなければ最初の表の
//!   見出しの前に足す。プロファイルを書いたら、最上位の `shell` の行はコメントにする。既定のプロファイル
//!   だけに戻したら、表を消す。
//! - `[colors]` と `[colors.…]` の色は既定の色を一つに書けないので、既定に戻した色はそれまでの値を
//!   コメントにする。
//! - `[keys]` の組み合わせは、書き方（大文字小文字、修飾キーの順）によらず同じ組み合わせの行を探す。
//!   既定の割り当てに戻した組み合わせは、既定の割り当ての `# "…" = "…"` のコメントの行にし、既定で
//!   割り当てていない組み合わせは行を消す。

use toml_edit::{Array, ArrayOfTables, Decor, DocumentMut, InlineTable, Item, Key, Table, Value};

use crate::{
    config::{
        self, BackendKind, BackgroundFit, BackgroundImage, ColorTable, Config, Profile,
        format_color,
    },
    keymap::{self, Action, KeyCombo},
};

/// 書き換える項目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    DefaultProfile,
    FontFamily,
    FontSize,
    WideGlyph,
    PasteConfirm,
    ScrollBarWidth,
    WindowPadding,
    ColorScheme,
    BackgroundImages,
    BackgroundImageOpacity,
    BackgroundFit,
}

impl Setting {
    const ALL: [Setting; 11] = [
        Setting::DefaultProfile,
        Setting::FontFamily,
        Setting::FontSize,
        Setting::WideGlyph,
        Setting::PasteConfirm,
        Setting::ScrollBarWidth,
        Setting::WindowPadding,
        Setting::ColorScheme,
        Setting::BackgroundImages,
        Setting::BackgroundImageOpacity,
        Setting::BackgroundFit,
    ];

    /// 項目を置く表のキーのパス。最上位なら空。
    fn path(self) -> &'static [&'static str] {
        match self {
            Setting::DefaultProfile => &[],
            Setting::FontFamily | Setting::FontSize | Setting::WideGlyph => &["font"],
            Setting::PasteConfirm => &["paste"],
            Setting::ScrollBarWidth => &["scroll_bar"],
            Setting::WindowPadding => &["window"],
            Setting::ColorScheme => &["colors"],
            Setting::BackgroundImages
            | Setting::BackgroundImageOpacity
            | Setting::BackgroundFit => &["background"],
        }
    }

    fn key(self) -> &'static str {
        match self {
            Setting::DefaultProfile => DEFAULT_PROFILE_KEY,
            Setting::FontFamily => "family",
            Setting::FontSize => "size",
            Setting::WideGlyph => "wide_glyph",
            Setting::PasteConfirm => "confirm",
            Setting::ScrollBarWidth => "width",
            Setting::WindowPadding => "padding",
            Setting::ColorScheme => "scheme",
            Setting::BackgroundImages => "images",
            Setting::BackgroundImageOpacity => "image_opacity",
            Setting::BackgroundFit => "fit",
        }
    }

    /// `config` でのこの項目の値。書かないことを表す値（`default_profile` の `None`）なら `None`。
    fn value(self, config: &Config) -> Option<Value> {
        match self {
            Setting::DefaultProfile => config.default_profile.as_deref().map(Value::from),
            Setting::FontFamily => Some(families_value(&config.font.families)),
            Setting::FontSize => Some(number_value(config.font.size_pt)),
            Setting::WideGlyph => Some(Value::from(config.font.wide_glyph.name())),
            Setting::PasteConfirm => Some(Value::from(config.paste.confirm.name())),
            Setting::ScrollBarWidth => Some(Value::from(i64::from(config.scroll_bar.width))),
            Setting::WindowPadding => Some(Value::from(i64::from(config.window.padding))),
            Setting::ColorScheme => Some(Value::from(config.colors.scheme.name())),
            Setting::BackgroundImages => Some(Value::Array(
                config
                    .background
                    .images
                    .iter()
                    .map(image_value)
                    .collect::<Array>(),
            )),
            Setting::BackgroundImageOpacity => Some(number_value(config.background.image_opacity)),
            Setting::BackgroundFit => Some(Value::from(config.background.fit.name())),
        }
    }
}

/// `[keys]` の表のキーのパス。
const KEYS_PATH: &[&str] = &["keys"];

/// 最上位のキーの名前。
const DEFAULT_PROFILE_KEY: &str = "default_profile";
const PROFILES_KEY: &str = "profiles";
const SHELL_KEY: &str = "shell";

/// `[[profiles]]` の表のキー。
const PROFILE_NAME: &str = "name";
const PROFILE_COMMAND: &str = "command";
const PROFILE_BACKEND: &str = "backend";

/// フォントの名前の並び。一つなら文字列、二つ以上なら配列。
fn families_value(families: &[String]) -> Value {
    match families {
        [family] => Value::from(family.as_str()),
        _ => Value::Array(families.iter().map(String::as_str).collect::<Array>()),
    }
}

/// `background.images` の項目一つ。収め方も濃さも全体の値に任せるなら文字列、そうでなければ
/// `{ path = …, fit = …, image_opacity = … }` の表（書かない値は入れない）。
fn image_value(image: &BackgroundImage) -> Value {
    image_item(Value::from(image.path.as_str()), image, Vec::new())
}

/// パスを `path` と書いた、`image` の項目一つ。表にするときは、`others`（知らないキー）を後ろに続ける。
fn image_item(mut path: Value, image: &BackgroundImage, others: Vec<(String, Value)>) -> Value {
    path.decor_mut().clear();
    if image.follows_background() && others.is_empty() {
        return path;
    }
    let mut table = InlineTable::new();
    table.insert(IMAGE_PATH, path);
    if let Some(fit) = image.fit {
        table.insert(IMAGE_FIT, Value::from(fit.name()));
    }
    if let Some(opacity) = image.image_opacity {
        table.insert(IMAGE_OPACITY, number_value(opacity));
    }
    for (key, mut value) in others {
        value.decor_mut().clear();
        table.insert(&key, value);
    }
    table.fmt();
    Value::InlineTable(table)
}

/// `background.images` の表の項目のキー。
const IMAGE_PATH: &str = "path";
const IMAGE_FIT: &str = "fit";
const IMAGE_OPACITY: &str = "image_opacity";

/// ファイルに書いてある `background.images` の項目一つ `item` のパス。読めない項目（パスのない表や、
/// 文字列でも表でもないもの）なら `None`。
fn written_image_path(item: &Value) -> Option<&str> {
    let path = match item {
        Value::String(path) => path.value(),
        Value::InlineTable(table) => table.get(IMAGE_PATH)?.as_str()?,
        _ => return None,
    };
    (!path.trim().is_empty()).then_some(path)
}

/// ファイルに書いてある `background.images` の項目一つ `item` の、収め方と濃さ。読めない値は書いて
/// いないものとする（設定を読むときと同じ）。
fn written_image_settings(item: &Value) -> (Option<BackgroundFit>, Option<f32>) {
    let Value::InlineTable(table) = item else {
        return (None, None);
    };
    let fit = table
        .get(IMAGE_FIT)
        .and_then(Value::as_str)
        .and_then(BackgroundFit::from_name);
    let opacity = table
        .get(IMAGE_OPACITY)
        .and_then(|value| {
            value
                .as_float()
                .or_else(|| value.as_integer().map(|integer| integer as f64))
        })
        .map(|opacity| opacity as f32)
        .filter(|opacity| config::IMAGE_OPACITY_RANGE.contains(opacity));
    (fit, opacity)
}

/// ファイルに書いてある `background.images` の配列 `written` を、一覧 `images` に合わせて書き換えた
/// 配列。項目はパスで照合し（同じパスが二つあれば前から順に）、一覧にない項目は消し、一覧にしか
/// ない項目は最後に足す。読めない項目、項目の前のコメント、並びは残す。収め方と濃さを変えた項目だけを
/// 書き換え、全体の値に任せるようにした表は（ほかのキーがなければ）文字列に戻す。
fn merged_images(written: &Array, images: &[BackgroundImage]) -> Array {
    let mut array = written.clone();
    let mut used = vec![false; images.len()];
    let mut index = 0;
    while index < array.len() {
        let Some(path) = array.get(index).and_then(written_image_path) else {
            index += 1;
            continue;
        };
        let found = images
            .iter()
            .enumerate()
            .position(|(i, image)| !used[i] && image.path == path);
        match found {
            Some(found) => {
                used[found] = true;
                if let Some(item) = array.get_mut(index) {
                    update_image_item(item, &images[found]);
                }
                index += 1;
            }
            None => remove_array_item(&mut array, index),
        }
    }
    for (image, _) in images.iter().zip(&used).filter(|(_, used)| !**used) {
        let prefix = appended_item_prefix(&array);
        let mut value = image_value(image);
        value.decor_mut().set_prefix(prefix);
        value.decor_mut().set_suffix("");
        array.push_formatted(value);
    }
    array
}

/// 配列 `array` の `index` 番目の項目を消す。先頭を消したら、次の項目の前の空白（コメントでなければ）を
/// 消した項目の前のものにする（`['a', 'b']` から `'a'` を消して `[ 'b']` にしないため）。
fn remove_array_item(array: &mut Array, index: usize) {
    let removed_prefix = array
        .get(index)
        .map(|item| decor_prefix(item.decor()))
        .unwrap_or_default();
    array.remove(index);
    if index != 0 {
        return;
    }
    if let Some(next) = array.get_mut(0)
        && decor_prefix(next.decor()).trim().is_empty()
    {
        next.decor_mut().set_prefix(removed_prefix);
    }
}

/// 配列 `array` の最後に足す項目の前に置く空白。二つ目からの項目と同じにする（項目を行ごとに書いて
/// いればその改行と字下げ、一行に並べていれば空白一つ）。
fn appended_item_prefix(array: &Array) -> String {
    let last = array.iter().last().map(|item| decor_prefix(item.decor()));
    match (array.len(), last) {
        (0, _) | (_, None) => String::new(),
        (1, Some(prefix)) if !prefix.contains('\n') => " ".to_owned(),
        (_, Some(prefix)) => {
            // コメントを写さないように、最後の行（改行のあとの字下げ）だけを使う。
            match prefix.rfind('\n') {
                Some(newline) => format!("\n{}", &prefix[newline + 1..]),
                None => prefix,
            }
        }
    }
}

/// ファイルに書いてある項目一つ `item` の収め方と濃さを、`image` に合わせる。同じなら変えない。パスは
/// 書いてあった形（引用符の種類など）のまま、知らないキーも残す。項目の前後の空白とコメントは残す。
fn update_image_item(item: &mut Value, image: &BackgroundImage) {
    if written_image_settings(item) == (image.fit, image.image_opacity) {
        return;
    }
    let (path, others) = match &*item {
        Value::InlineTable(table) => (
            table.get(IMAGE_PATH).cloned(),
            table
                .iter()
                .filter(|(key, _)| ![IMAGE_PATH, IMAGE_FIT, IMAGE_OPACITY].contains(key))
                .map(|(key, value)| (key.to_owned(), value.clone()))
                .collect(),
        ),
        other => (Some(other.clone()), Vec::new()),
    };
    let path = path.unwrap_or_else(|| Value::from(image.path.as_str()));
    let mut value = image_item(path, image, others);
    *value.decor_mut() = item.decor().clone();
    *item = value;
}

/// フォントの大きさや画像の濃さの数。整数なら整数で、そうでなければ小数で書く（`f32` を `f64` に
/// 広げた端数を出さないように、`f32` の短い表記から読み直す）。
fn number_value(number: f32) -> Value {
    if number.fract() == 0.0 {
        return Value::from(number as i64);
    }
    let widened: f64 = number.to_string().parse().unwrap_or(f64::from(number));
    Value::from(widened)
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
/// 編集を始めたときの設定。TOML として読めなければ、そのエラーメッセージを返す。
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
                let value = match (setting, written_value(&doc, setting)) {
                    (Setting::BackgroundImages, Some(Value::Array(written))) => {
                        Value::Array(merged_images(written, &edited.background.images))
                    }
                    _ => value,
                };
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
    update_profiles(&mut doc, base, edited)?;
    update_colors(&mut doc, base, edited)?;
    update_keys(&mut doc, base, edited)?;
    let mut out = doc.to_string();
    if crlf {
        out = out.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    Ok(format!("{bom}{out}"))
}

/// ファイルに書いてある、`setting` の最初の値。
fn written_value(doc: &DocumentMut, setting: Setting) -> Option<&Value> {
    table_at(doc, setting.path())?
        .get(setting.key())?
        .as_value()
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

/// プロファイルが `base` と `edited` で違えば書き換える。最上位の `shell` の行はコメントにする（書いた
/// プロファイルが使われ、`shell` は使われないため）。既定のプロファイルだけに戻したら、表を消す。
fn update_profiles(doc: &mut DocumentMut, base: &Config, edited: &Config) -> Result<(), String> {
    if base.profiles == edited.profiles {
        return Ok(());
    }
    let shell = |name: &str| name == SHELL_KEY;
    reset(
        doc,
        &[],
        &Target {
            name: SHELL_KEY.to_owned(),
            matches: &shell,
        },
        Reset::CommentOld,
    )?;
    let wanted: &[Profile] = if edited.profiles == Config::default().profiles {
        &[]
    } else {
        &edited.profiles
    };
    match doc.get(PROFILES_KEY) {
        None if wanted.is_empty() => Ok(()),
        None | Some(Item::ArrayOfTables(_)) => {
            merge_profile_tables(doc, wanted);
            Ok(())
        }
        Some(Item::Value(Value::Array(_))) => {
            let matches = |name: &str| name == PROFILES_KEY;
            let target = Target {
                name: PROFILES_KEY.to_owned(),
                matches: &matches,
            };
            if wanted.is_empty() {
                return reset(doc, &[], &target, Reset::Remove);
            }
            let array: Array = wanted.iter().map(profile_inline_table).collect();
            set(doc, &[], &target, Value::Array(array))
        }
        Some(_) => Err(format!("{PROFILES_KEY} is not an array of tables")),
    }
}

/// プロファイル一つの、インラインの表。
fn profile_inline_table(profile: &Profile) -> Value {
    let mut table = InlineTable::new();
    table.insert(PROFILE_NAME, Value::from(profile.name.as_str()));
    table.insert(PROFILE_COMMAND, Value::from(profile.command.as_str()));
    if profile.backend != BackendKind::default() {
        table.insert(PROFILE_BACKEND, Value::from(profile.backend.name()));
    }
    table.fmt();
    Value::InlineTable(table)
}

/// `[[profiles]]` の表を、`wanted` に合わせて `wanted` の順に並べ直す。書いてある表とは名前で照合し、
/// 一致しなかったものはコマンドラインで照合する（名前だけを変えたとき）。一致した表は、変えた値だけを
/// 書き換えてコメントと知らないキーを残す。一致しなかった表は消し、新しいプロファイルは表を作る。名前の
/// ない表（読めない表）は最後にそのまま残す。
fn merge_profile_tables(doc: &mut DocumentMut, wanted: &[Profile]) {
    let text_of =
        |table: &Table, key: &str| table.get(key).and_then(Item::as_str).map(str::to_owned);
    let written: Vec<(Option<String>, Option<String>)> = match doc.get(PROFILES_KEY) {
        Some(Item::ArrayOfTables(array)) => array
            .iter()
            .map(|table| {
                (
                    text_of(table, PROFILE_NAME),
                    text_of(table, PROFILE_COMMAND),
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    // wanted の位置から、書いてある表の位置へ。
    let mut pairs: Vec<Option<usize>> = vec![None; wanted.len()];
    let mut used = vec![false; written.len()];
    let named = |index: usize| written[index].0.is_some();
    for (want, pair) in wanted.iter().zip(pairs.iter_mut()) {
        if let Some(index) = (0..written.len())
            .find(|&index| !used[index] && written[index].0.as_deref() == Some(want.name.as_str()))
        {
            used[index] = true;
            *pair = Some(index);
        }
    }
    for (want, pair) in wanted.iter().zip(pairs.iter_mut()) {
        if pair.is_some() {
            continue;
        }
        if let Some(index) = (0..written.len()).find(|&index| {
            !used[index]
                && named(index)
                && written[index].1.as_deref() == Some(want.command.as_str())
        }) {
            used[index] = true;
            *pair = Some(index);
        }
    }
    // プロファイルの表の後ろに続く表（プロファイルの表がなければ、最初の表）。
    let me = Header {
        path: vec![PROFILES_KEY.to_owned()],
        index: None,
    };
    let positions: Vec<isize> = tables(doc)
        .into_iter()
        .filter(|(header, _)| header.path == me.path)
        .filter_map(|(_, position)| position)
        .collect();
    let following = first_after(doc, &me, positions.iter().max().copied());
    let following_position = match &following {
        Following::Table(header) => header.table(doc).and_then(Table::position),
        Following::End => None,
    };
    let root_has_values = doc.as_table().iter().any(|(_, item)| item.is_value());

    let old: Vec<Table> = match doc.as_table_mut().remove(PROFILES_KEY) {
        Some(Item::ArrayOfTables(array)) => array.into_iter().collect(),
        _ => Vec::new(),
    };
    // 新しい並び。表と、書いてあった位置（新しい表は `None`）。
    let mut tables: Vec<(Option<usize>, Table)> = Vec::new();
    for (want, pair) in wanted.iter().zip(&pairs) {
        let mut table = match pair {
            Some(index) => old[*index].clone(),
            None => {
                let mut table = Table::new();
                table.decor_mut().set_prefix("\n");
                table
            }
        };
        update_profile_table(&mut table, want);
        tables.push((*pair, table));
    }
    tables.extend(
        old.iter()
            .enumerate()
            .filter(|(index, _)| !named(*index))
            .map(|(index, table)| (Some(index), table.clone())),
    );
    // 同じ位置の表は配列の順に並ぶので、どれも書いてあった最初の表の位置（なければ続く表のすぐ前）にする。
    let position = old
        .iter()
        .filter_map(Table::position)
        .min()
        .or(following_position.map(|position| position - 1))
        .unwrap_or(0);
    // 最初の表の前のコメント（ファイルの先頭のコメントなど）は、並べ直しても最初の表の前に置く。
    match old.first().map(|table| decor_prefix(table.decor())) {
        Some(head) => {
            if tables.is_empty() {
                // 表がなくなるので、続く表の前へ返す。
                let after = following_prefix(doc, &following);
                let rest = after.strip_prefix('\n').unwrap_or(&after);
                set_following_prefix(doc, &following, format!("{head}{rest}"));
                return;
            }
            for (from, table) in tables.iter_mut().skip(1) {
                if *from == Some(0) {
                    table.decor_mut().set_prefix("\n");
                }
            }
            let (from, first) = &mut tables[0];
            if *from != Some(0) {
                let own = decor_prefix(first.decor());
                let own = own.strip_prefix('\n').unwrap_or(&own);
                first.decor_mut().set_prefix(format!("{head}{own}"));
            }
        }
        None => {
            if tables.is_empty() {
                return;
            }
            // 続く表の前のコメントのうち、空行で見出しと離れたところまでを最初の表の前に移す。ファイルの
            // 最後のコメントは、表の後ろに残す。
            let mut before = String::new();
            if let Following::Table(_) = following {
                let after = following_prefix(doc, &following);
                let (moved, rest) = split_after_last_blank_line(&after);
                before = moved;
                set_following_prefix(doc, &following, format!("\n{rest}"));
            }
            if before.is_empty() && root_has_values {
                before = "\n".to_owned();
            }
            tables[0].1.decor_mut().set_prefix(before);
        }
    }
    let mut array = ArrayOfTables::new();
    for (_, mut table) in tables {
        table.set_position(Some(position));
        array.push(table);
    }
    doc.as_table_mut()
        .insert(PROFILES_KEY, Item::ArrayOfTables(array));
}

fn update_profile_table(table: &mut Table, profile: &Profile) {
    set_table_text(table, PROFILE_NAME, &profile.name);
    set_table_text(table, PROFILE_COMMAND, &profile.command);
    if table.contains_key(PROFILE_BACKEND) || profile.backend != BackendKind::default() {
        set_table_text(table, PROFILE_BACKEND, profile.backend.name());
    }
}

/// 表 `table` の `key` の値を文字列 `text` にする。同じなら（引用符の種類も）変えない。行のコメントは残す。
fn set_table_text(table: &mut Table, key: &str, text: &str) {
    if table.get(key).and_then(Item::as_str) == Some(text) {
        return;
    }
    let mut value = Value::from(text);
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

/// 表のうち、ほかの表の見出しとファイルの最後との位置関係で扱えるもの（`[font]` の形の見出しを
/// 持つ表と最上位）か。`font.size = 9` のような点で区切ったキーや、インラインの表は違う。
fn is_standard(table: &Table) -> bool {
    !table.is_dotted()
}

fn path_name(path: &[&str]) -> String {
    path.join(".")
}

/// キーのパス `path` の表。
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

/// キーのパス `path` の表。無ければ、`create` なら最後に作る（途中の表は見出しを書かない表にする）。
/// 表でないものが書いてあればエラー。
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

/// 見出しを書いた表一つ。表の配列（`[[profiles]]` など）の表なら、`index` はその中の位置。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Header {
    path: Vec<String>,
    index: Option<usize>,
}

impl Header {
    fn table<'a>(&self, doc: &'a DocumentMut) -> Option<&'a Table> {
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

/// 見出しを書いた表と、ファイルの中の位置。見出しを書かない表（`[colors.tab_bar]` の `colors`
/// など）は含めず、その中の表を含める。表の配列は、その表を一つずつ含める。
fn tables(doc: &DocumentMut) -> Vec<(Header, Option<isize>)> {
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

/// 表の値の行のあとに続く場所。次の表の見出しの前か、ファイルの最後。
#[derive(Debug)]
enum Following {
    Table(Header),
    End,
}

/// `path` の表の最後の値の行のあとに続く場所。
fn following(doc: &DocumentMut, path: &[&str]) -> Following {
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

/// `me` のほかの、位置が `after` より後ろ（`None` ならどこでも）の、いちばん前の表。
fn first_after(doc: &DocumentMut, me: &Header, after: Option<isize>) -> Following {
    tables(doc)
        .into_iter()
        .filter(|(other, position)| other != me && position.is_some())
        .filter(|(_, position)| after.is_none_or(|after| position.is_some_and(|p| p > after)))
        .min_by_key(|(_, position)| *position)
        .map_or(Following::End, |(other, _)| Following::Table(other))
}

fn following_prefix(doc: &DocumentMut, following: &Following) -> String {
    match following {
        Following::Table(header) => header
            .table(doc)
            .map(|table| decor_prefix(table.decor()))
            .unwrap_or_default(),
        Following::End => doc.trailing().as_str().unwrap_or_default().to_owned(),
    }
}

fn set_following_prefix(doc: &mut DocumentMut, following: &Following, prefix: String) {
    match following {
        Following::Table(header) => {
            if let Some(table) = header.table_mut(doc) {
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
            self, BackgroundConfig, BackgroundFit, ColorScheme, ColorsConfig, FontConfig,
            PasteConfig, PasteConfirm, ScrollBarConfig, TEMPLATE, WideGlyph, WindowConfig,
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

    #[test]
    fn every_wide_glyph_is_written_by_its_name() {
        for wide_glyph in WideGlyph::ALL {
            let mut edited = Config::default();
            edited.font.wide_glyph = wide_glyph;
            let text = "[font]\nwide_glyph = 'clip'\n";
            let base = config::parse(text, &Config::default()).config;
            let updated = update(text, &base, &edited).unwrap();
            assert_reads_as(&updated, &edited);
        }
        let mut edited = Config::default();
        edited.font.wide_glyph = WideGlyph::OverflowCenter;
        let updated = update("", &Config::default(), &edited).unwrap();
        assert_eq!(updated, "[font]\nwide_glyph = \"overflow_center\"\n");
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
            profiles: vec![Profile::from_shell("zsh -l"), Profile::builtin()],
            default_profile: Some(config::DEFAULT_PROFILE_NAME.to_owned()),
            paste: PasteConfig {
                confirm: PasteConfirm::Never,
            },
            scroll_bar: ScrollBarConfig { width: 20 },
            window: WindowConfig { padding: 0 },
            // 色は既定に戻すとひな形と違う行になるので、色の書き戻しは別に確かめる。
            colors: ColorsConfig::default(),
            background: BackgroundConfig {
                images: vec![
                    BackgroundImage::new(r"C:\walls"),
                    BackgroundImage::new(r"D:\pics\*.png"),
                ],
                image_opacity: 0.65,
                fit: BackgroundFit::Center,
            },
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
                r#"# default_profile = "PowerShell""#,
                r#"default_profile = "PowerShell""#,
            )
            .replace(
                "\n\n[font]\n",
                "\n\n[[profiles]]\nname = \"zsh\"\ncommand = \"zsh -l\"\n\n[[profiles]]\nname = \"PowerShell\"\ncommand = \"powershell.exe -NoLogo\"\n\n[font]\n",
            )
            .replace(
                r#"# family = ["Bizin Gothic", "BIZ UDGothic", "Consolas"]"#,
                r#"family = ["Cascadia Mono", "Consolas"]"#,
            )
            .replace("# size = 12", "size = 10.5")
            .replace(r#"# wide_glyph = "shrink""#, r#"wide_glyph = "overflow""#)
            .replace(r#"# confirm = "unsafe""#, r#"confirm = "never""#)
            .replace("# width = 12", "width = 20")
            .replace("# padding = 4", "padding = 0")
            .replace(
                "\n# images = []",
                r"
images = ['C:\walls', 'D:\pics\*.png']",
            )
            .replace("# image_opacity = 0.2", "image_opacity = 0.65")
            .replace(r#"# fit = "fill""#, r#"fit = "center""#)
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
        // default_profile は既定値がないので、それまでの値をコメントにする（ひな形の例と同じ値）。
        // プロファイルは表を消す。
        assert_eq!(back, TEMPLATE);

        // 一つずつ戻しても同じ。
        let mut current = edited.clone();
        let mut text = text;
        let defaults = Config::default();
        let steps: [fn(&mut Config, &Config); 10] = [
            |c, d| c.font.size_pt = d.font.size_pt,
            |c, d| c.window = d.window.clone(),
            |c, d| c.background.image_opacity = d.background.image_opacity,
            |c, d| {
                c.background.images = d.background.images.clone();
                c.background.fit = d.background.fit;
            },
            |c, d| {
                c.profiles = d.profiles.clone();
                c.default_profile = d.default_profile.clone();
            },
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
        edited.default_profile = Some("PowerShell".to_owned());
        let updated = update(text, &base, &edited).unwrap();
        // 見出しのすぐ上のコメントは、その表のものとして見出しの上に残す。
        assert_eq!(
            updated,
            "default_profile = \"PowerShell\"\n# only a comment\n[font]\nsize = 9\nwide_glyph = \"clip\"\n\n[scroll_bar]\nwidth = 16\n"
        );
        assert_reads_as(&updated, &edited);

        // 空行で見出しと離れたコメントは、ファイルの先頭のものとして最上位の項目の上に置く。
        let text = "# head\n\n# about font\n[font]\nsize = 9\n";
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "# head\n\ndefault_profile = \"PowerShell\"\n# about font\n[font]\nsize = 9\nwide_glyph = \"clip\"\n\n[scroll_bar]\nwidth = 16\n"
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
    fn numbers_are_written_without_widening_errors() {
        assert_eq!(bare_repr(&number_value(12.0)), "12");
        assert_eq!(bare_repr(&number_value(10.1)), "10.1");
        assert_eq!(bare_repr(&number_value(10.5)), "10.5");
        assert_eq!(bare_repr(&number_value(0.8)), "0.8");
        assert_eq!(bare_repr(&number_value(0.35)), "0.35");
    }

    #[test]
    fn background_table_is_added_when_missing() {
        let text = "[font]\nsize = 9\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited.background.images = vec![BackgroundImage::new(r"C:\walls")];
        edited.background.image_opacity = 1.0;
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "[font]\nsize = 9\n\n[background]\nimages = ['C:\\walls']\nimage_opacity = 1\n"
        );
        assert_reads_as(&updated, &edited);
        // 一覧を空に戻すと、既定値をコメントにした行にする。
        let back = update(&updated, &edited, &base).unwrap();
        assert_eq!(
            back,
            "[font]\nsize = 9\n\n[background]\n# images = []\n# image_opacity = 0.2\n"
        );
    }

    fn image(
        path: &str,
        fit: Option<BackgroundFit>,
        image_opacity: Option<f32>,
    ) -> BackgroundImage {
        BackgroundImage {
            path: path.to_owned(),
            fit,
            image_opacity,
        }
    }

    #[test]
    fn image_items_become_tables_and_strings_again_in_place() {
        let text = "[background]\n# mine\nimages = [\n  # walls\n  'C:\\walls',\n  'C:\\a.png', # the cat\n  { path = 'C:\\b.png', fit = 'tile' },\n]\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited.background.images[0].image_opacity = Some(0.3);
        edited.background.images[1].fit = Some(BackgroundFit::Center);
        edited.background.images[2].fit = None;
        let updated = update(text, &base, &edited).unwrap();
        // コメントと並びは残し、変えた項目だけを書き換える。パスの引用符は書いたまま。
        assert_eq!(
            updated,
            "[background]\n# mine\nimages = [\n  # walls\n  { path = 'C:\\walls', image_opacity = 0.3 },\n  { path = 'C:\\a.png', fit = \"center\" }, # the cat\n  'C:\\b.png',\n]\n"
        );
        assert_reads_as(&updated, &edited);
        // 全体の設定に従うように戻すと、文字列に戻る。
        let back = update(&updated, &edited, &base).unwrap();
        assert_eq!(
            back,
            "[background]\n# mine\nimages = [\n  # walls\n  'C:\\walls',\n  'C:\\a.png', # the cat\n  { path = 'C:\\b.png', fit = \"tile\" },\n]\n"
        );
        assert_reads_as(&back, &base);
    }

    #[test]
    fn image_items_are_removed_and_added_keeping_the_others() {
        let text = "[background]\nimages = ['C:\\a.png', { path = 'C:\\b.png', fit = 'tile', note = 'x' }, 3, 'C:\\c.png']\n";
        let parsed = config::parse(text, &Config::default());
        let base = parsed.config;
        let mut edited = base.clone();
        // 先頭を外し、知らないキーのある表を全体の設定に従わせ、二つ足す。
        edited.background.images.remove(0);
        edited.background.images[0].fit = None;
        edited
            .background
            .images
            .push(image(r"D:\d.png", Some(BackgroundFit::Fit), None));
        edited.background.images.push(BackgroundImage::new(r"D:\e"));
        let updated = update(text, &base, &edited).unwrap();
        // 読めない項目（3）はそのまま。知らないキーのある表は表のまま。
        assert_eq!(
            updated,
            "[background]\nimages = [{ path = 'C:\\b.png', note = 'x' }, 3, 'C:\\c.png', { path = 'D:\\d.png', fit = \"fit\" }, 'D:\\e']\n"
        );
        let reread = config::parse(&updated, &Config::default());
        assert_eq!(reread.config.background, edited.background);

        // 行ごとに書いた配列には、同じ字下げで足す。
        let text = "[background]\nimages = [\n    # first\n    'C:\\a.png',\n]\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited
            .background
            .images
            .push(image(r"C:\b.png", None, Some(0.5)));
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "[background]\nimages = [\n    # first\n    'C:\\a.png',\n    { path = 'C:\\b.png', image_opacity = 0.5 },\n]\n"
        );
        assert_reads_as(&updated, &edited);
    }

    #[test]
    fn image_tables_are_written_into_the_template() {
        let mut edited = Config::default();
        edited.background.images = vec![
            BackgroundImage::new(r"C:\walls"),
            image(r"C:\tiles", Some(BackgroundFit::Tile), Some(0.25)),
        ];
        let text = update(TEMPLATE, &Config::default(), &edited).unwrap();
        assert_reads_as(&text, &edited);
        assert_eq!(
            text,
            TEMPLATE.replace(
                "\n# images = []",
                "\nimages = ['C:\\walls', { path = 'C:\\tiles', fit = \"tile\", image_opacity = 0.25 }]"
            )
        );
        assert_eq!(
            update(&text, &edited, &Config::default()).unwrap(),
            TEMPLATE
        );
    }

    fn profile(name: &str, command: &str) -> Profile {
        Profile {
            name: name.to_owned(),
            command: command.to_owned(),
            backend: BackendKind::Conpty,
        }
    }

    #[test]
    fn profiles_are_merged_by_name_keeping_the_comments() {
        let text = "default_profile = 'zsh'\n\n# mine\n[[profiles]]\nname = 'PowerShell'\ncommand = 'powershell.exe -NoLogo' # plain\n\n# cygwin\n[[profiles]]\nname = 'zsh'\ncommand = 'C:\\cygwin64\\bin\\zsh.exe -l'\nbackend = 'conpty'\n\n[[profiles]]\nname = 'old'\ncommand = 'cmd.exe'\n\n[font]\nsize = 9\n";
        let base = config::parse(text, &Config::default()).config;
        assert_eq!(base.profiles.len(), 3);
        let mut edited = base.clone();
        // 1 番目のコマンドを変え、2 番目の名前を変え（既定も追う）、3 番目を消して 2 つ足す。
        edited.profiles[0].command = "pwsh.exe".to_owned();
        edited.profiles[1].name = "Cygwin zsh".to_owned();
        edited.default_profile = Some("Cygwin zsh".to_owned());
        edited.profiles.remove(2);
        edited.profiles.push(profile("WSL", "wsl.exe"));
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "default_profile = \"Cygwin zsh\"\n\n# mine\n[[profiles]]\nname = 'PowerShell'\ncommand = \"pwsh.exe\" # plain\n\n# cygwin\n[[profiles]]\nname = \"Cygwin zsh\"\ncommand = 'C:\\cygwin64\\bin\\zsh.exe -l'\nbackend = 'conpty'\n\n[[profiles]]\nname = \"WSL\"\ncommand = \"wsl.exe\"\n\n[font]\nsize = 9\n"
        );
        assert_reads_as(&updated, &edited);
    }

    #[test]
    fn profiles_follow_the_new_order_and_new_names_get_new_tables() {
        let text = "default_profile = 'b'\n\n# head\n[[profiles]]\nname = 'a'\ncommand = 'cmd.exe'\n\n[[profiles]]\nname = 'b'\ncommand = 'pwsh.exe' # mine\n\n[[profiles]]\nname = 'c'\ncommand = 'zsh.exe'\nbackend = 'conpty'\n\n[font]\nsize = 9\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        // a を消し、c の名前とコマンドラインを変え、新しいものを足す。
        edited.profiles.remove(0);
        edited.profiles[1] = profile("c2", "zsh.exe -l");
        edited.profiles.push(profile("d", "wsl.exe"));
        edited.default_profile = Some("c2".to_owned());
        let updated = update(text, &base, &edited).unwrap();
        // 名前もコマンドラインも違う c の表は、どの表とも一致しないので作り直す。先頭のコメントは最初の
        // 表の前に残す。
        assert_eq!(
            updated,
            "default_profile = \"c2\"\n\n# head\n[[profiles]]\nname = 'b'\ncommand = 'pwsh.exe' # mine\n\n[[profiles]]\nname = \"c2\"\ncommand = \"zsh.exe -l\"\n\n[[profiles]]\nname = \"d\"\ncommand = \"wsl.exe\"\n\n[font]\nsize = 9\n"
        );
        assert_reads_as(&updated, &edited);

        // 並べ替えたら、その順に書く。
        let mut swapped = edited.clone();
        swapped.profiles.swap(0, 2);
        let reordered = update(&updated, &edited, &swapped).unwrap();
        assert_reads_as(&reordered, &swapped);
        assert!(
            reordered.starts_with("default_profile = \"c2\"\n\n# head\n[[profiles]]\nname = \"d\"")
        );
    }

    #[test]
    fn profiles_replace_the_shell_and_go_before_the_first_table() {
        let text = "# head\nshell = 'zsh -l' # mine\n\n# about font\n[font]\nsize = 9\n";
        let base = config::parse(text, &Config::default()).config;
        assert_eq!(base.profiles, [Profile::from_shell("zsh -l")]);
        let mut edited = base.clone();
        edited.profiles.push(profile("cmd", "cmd.exe"));
        let updated = update(text, &base, &edited).unwrap();
        // shell はコメントにし、表は最初の表の見出しの前に置く。
        assert_eq!(
            updated,
            "# head\n# shell = 'zsh -l' # mine\n\n[[profiles]]\nname = \"zsh\"\ncommand = \"zsh -l\"\n\n[[profiles]]\nname = \"cmd\"\ncommand = \"cmd.exe\"\n\n# about font\n[font]\nsize = 9\n"
        );
        assert_reads_as(&updated, &edited);

        // 何もないファイルと、表のないファイル。
        let updated = update("", &Config::default(), &edited).unwrap();
        assert_reads_as(&updated, &edited);
        assert_eq!(
            updated,
            "[[profiles]]\nname = \"zsh\"\ncommand = \"zsh -l\"\n\n[[profiles]]\nname = \"cmd\"\ncommand = \"cmd.exe\"\n\n[font]\nsize = 9\n"
        );
        let updated = update("# a\n", &Config::default(), &edited).unwrap();
        assert_reads_as(&updated, &edited);
    }

    #[test]
    fn profiles_back_to_the_default_remove_the_tables() {
        let text = "# head\n\n[[profiles]]\nname = 'a'\ncommand = 'cmd.exe'\n\n[[profiles]]\nname = 'b'\ncommand = 'wsl.exe'\n\n[font]\nsize = 9\n";
        let base = config::parse(text, &Config::default()).config;
        let edited = Config {
            profiles: Config::default().profiles,
            ..base.clone()
        };
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(updated, "# head\n\n[font]\nsize = 9\n");
        assert_reads_as(&updated, &edited);
        // インラインの配列なら、配列を書き換える。
        let text = "profiles = [{ name = 'a', command = 'cmd.exe' }]\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited.profiles[0].command = "pwsh.exe".to_owned();
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "profiles = [{ name = \"a\", command = \"pwsh.exe\" }]\n"
        );
        assert_reads_as(&updated, &edited);
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
