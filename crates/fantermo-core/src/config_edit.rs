//! 設定ファイルの中身を、変えた項目だけ書き換える。
//!
//! コメント、空行、項目の並び、知らないキーはそのまま残す。書き換えるのは、前の設定と新しい設定で値が
//! 違う項目だけ。
//!
//! - 書いてある項目は、値だけを書き換える（行末のコメントは残す）。
//! - 書いていない項目は、そのテーブルの中に `# キー = …` とコメントにした行があれば、その行を置き換えて
//!   書く（[`TEMPLATE`](crate::config::TEMPLATE) から作ったファイルで、説明の行の下に入る）。なければ
//!   テーブルの最後に足す。テーブルがなければ、ファイルの最後にテーブルごと足す。
//! - 既定値に戻した項目は、その行を既定値の `# キー = …` のコメントの行にする（`default_profile` は
//!   既定値を一つに書けないので、それまでの値をコメントにする）。上に書いた説明のコメントは残る。
//! - プロファイル（`[[profiles]]`）は、名前で照合して（一致しなければコマンドラインが同じテーブルと）
//!   変えた値だけを書き換え、なくなったテーブルを消し、新しいテーブルを作って、設定の順に並べる。
//!   テーブルがなければ最初のテーブルのヘッダーの前に足す。プロファイルを書いたら、最上位の `shell` の
//!   行はコメントにする。既定のプロファイルだけに戻したら、テーブルを消す。
//! - `[colors]` と `[colors.…]` の色は既定の色を一つに書けないので、既定に戻した色はそれまでの値を
//!   コメントにする。
//! - `[keys]` の組み合わせは、書き方（大文字小文字、修飾キーの順）によらず同じ組み合わせの行を探す。
//!   既定の割り当てに戻した組み合わせは、既定の割り当ての `# "…" = "…"` のコメントの行にし、既定で
//!   割り当てていない組み合わせは行を消す。
//! - 背景画像の一覧（`[[background.images]]`）は、パスで照合して変えた値だけを書き換え、一覧から外した
//!   テーブルを消し、足した画像のテーブルを最後に足す。テーブルは `[background]` の値の後ろに置く。
//!   一覧を空にしたらテーブルを消す。
//!
//! 書き換える前に、古い書き方を今の書き方に読み替える（[`config::migrate`]）。

use toml_edit::{Array, ArrayOfTables, Decor, DocumentMut, InlineTable, Item, Key, Table, Value};

use crate::{
    config::{
        self, BackendKind, BackgroundFit, BackgroundImage, ColorTable, Config, Profile,
        format_color, migrate,
    },
    keymap::{self, Action, KeyCombo},
};

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
    BackgroundImageOpacity,
    BackgroundFit,
}

impl Setting {
    const ALL: [Setting; 10] = [
        Setting::DefaultProfile,
        Setting::FontFamily,
        Setting::FontSize,
        Setting::WideGlyph,
        Setting::PasteConfirm,
        Setting::ScrollBarWidth,
        Setting::WindowPadding,
        Setting::ColorScheme,
        Setting::BackgroundImageOpacity,
        Setting::BackgroundFit,
    ];

    /// 項目を置くテーブルのキーのパス。最上位なら空。
    fn path(self) -> &'static [&'static str] {
        match self {
            Setting::DefaultProfile => &[],
            Setting::FontFamily | Setting::FontSize | Setting::WideGlyph => &["font"],
            Setting::PasteConfirm => &["paste"],
            Setting::ScrollBarWidth => &["scroll_bar"],
            Setting::WindowPadding => &["window"],
            Setting::ColorScheme => &["colors"],
            Setting::BackgroundImageOpacity | Setting::BackgroundFit => BACKGROUND_PATH,
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
            Setting::BackgroundImageOpacity => IMAGE_OPACITY,
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
            Setting::BackgroundImageOpacity => Some(number_value(config.background.image_opacity)),
            Setting::BackgroundFit => Some(Value::from(config.background.fit.name())),
        }
    }
}

/// `[keys]` のテーブルのキーのパス。
const KEYS_PATH: &[&str] = &["keys"];

/// 最上位のキーの名前。
const DEFAULT_PROFILE_KEY: &str = "default_profile";
const PROFILES_KEY: &str = "profiles";
const SHELL_KEY: &str = "shell";

/// `[[profiles]]` のテーブルのキー。
const PROFILE_NAME: &str = "name";
const PROFILE_COMMAND: &str = "command";
const PROFILE_BACKEND: &str = "backend";
const PROFILE_CYGWIN_ROOT: &str = "cygwin_root";
const PROFILE_DISTRIBUTION: &str = "distribution";
const PROFILE_ICON: &str = "icon";

/// フォントの名前の並び。一つなら文字列、二つ以上なら配列。
fn families_value(families: &[String]) -> Value {
    match families {
        [family] => Value::from(family.as_str()),
        _ => Value::Array(families.iter().map(String::as_str).collect::<Array>()),
    }
}

/// `[background]` のテーブルのキーのパス。
pub(crate) const BACKGROUND_PATH: &[&str] = &["background"];

/// `[background]` の、背景画像の一覧（`[[background.images]]`）のキー。
pub(crate) const IMAGES: &str = "images";

/// `[[background.images]]` のテーブルのキー。`image_opacity` は `[background]` のキーと同じ名前。
pub(crate) const IMAGE_PATH: &str = "path";
const IMAGE_FIT: &str = "fit";
pub(crate) const IMAGE_OPACITY: &str = "image_opacity";

/// `[[background.images]]` のテーブル一つ。書かない値（全体の値に従う表示方法と不透明度）は入れない。
fn image_table(image: &BackgroundImage) -> Table {
    let mut table = Table::new();
    table.insert(IMAGE_PATH, Item::Value(Value::from(image.path.as_str())));
    if let Some(fit) = image.fit {
        table.insert(IMAGE_FIT, Item::Value(Value::from(fit.name())));
    }
    if let Some(opacity) = image.image_opacity {
        table.insert(IMAGE_OPACITY, Item::Value(number_value(opacity)));
    }
    table
}

/// ファイルに書いてある `[[background.images]]` のテーブル一つ `table` のパス。読めないテーブル（パスの
/// ないテーブルや、パスが空のもの）なら `None`。
fn written_image_path(table: &Table) -> Option<&str> {
    let path = table.get(IMAGE_PATH)?.as_str()?;
    (!path.trim().is_empty()).then_some(path)
}

/// ファイルに書いてある `[[background.images]]` のテーブル一つ `table` の、表示方法と不透明度。読めない
/// 値は書いていないものとする（設定を読むときと同じ）。
fn written_image_settings(table: &Table) -> (Option<BackgroundFit>, Option<f32>) {
    let fit = table
        .get(IMAGE_FIT)
        .and_then(Item::as_str)
        .and_then(BackgroundFit::from_name);
    let opacity = table
        .get(IMAGE_OPACITY)
        .and_then(Item::as_value)
        .and_then(number_of)
        .map(|opacity| opacity as f32)
        .filter(|opacity| config::IMAGE_OPACITY_RANGE.contains(opacity));
    (fit, opacity)
}

/// 数の値（整数か小数）。
pub(crate) fn number_of(value: &Value) -> Option<f64> {
    value
        .as_float()
        .or_else(|| value.as_integer().map(|integer| integer as f64))
}

/// 背景画像の一覧が `base` と `edited` で違えば、`[[background.images]]` のテーブルを書き換える。
/// テーブルを書いていなければ `[background]` の値の後ろに足す。`images` にテーブルの配列でない値が
/// 書いてあれば（`images = 3` など）、その行を消してテーブルを足す。
fn update_background_images(
    doc: &mut DocumentMut,
    base: &Config,
    edited: &Config,
) -> Result<(), String> {
    let images = &edited.background.images;
    if base.background.images == *images {
        return Ok(());
    }
    match table_at(doc, BACKGROUND_PATH).and_then(|table| table.get(IMAGES)) {
        None => {}
        Some(Item::ArrayOfTables(_)) => {
            merge_image_tables(doc, images);
            return Ok(());
        }
        Some(Item::Value(_)) => replace_line(doc, BACKGROUND_PATH, IMAGES, None),
        Some(_) => return Err(format!("background.{IMAGES} is not an array of tables")),
    }
    if images.is_empty() {
        return Ok(());
    }
    let tables = images
        .iter()
        .map(|image| (String::new(), image_table(image)))
        .collect();
    insert_image_tables(doc, tables, String::new())
}

/// 書いてある `[[background.images]]` のテーブルを、一覧 `images` に合わせる。テーブルはパスで照合し
/// （同じパスが二つあれば前から順に）、一覧にないテーブルは消し、一覧にしかない画像はテーブルを最後に
/// 足す。読めないテーブル、テーブルのコメント、並びは残す。表示方法と不透明度を変えたテーブルだけを
/// 書き換える。最初のテーブルの前のコメントは、最初のテーブルを消しても次のテーブルの前に残し、
/// テーブルがなくなれば続くテーブルの前へ返す。
fn merge_image_tables(doc: &mut DocumentMut, images: &[BackgroundImage]) {
    let Some(background) = table_at_mut(doc, BACKGROUND_PATH) else {
        return;
    };
    let background_position = background.position();
    let old: Vec<Table> = match background.remove(IMAGES) {
        Some(Item::ArrayOfTables(old)) => old.into_iter().collect(),
        _ => return,
    };
    let head = old
        .first()
        .map(|table| decor_prefix(table.decor()))
        .unwrap_or_default();
    let position = old
        .iter()
        .filter_map(Table::position)
        .max()
        .or(background_position);
    let mut used = vec![false; images.len()];
    // 残すテーブルと、書いてあった位置（足すテーブルは `None`）。
    let mut kept: Vec<(Option<usize>, Table)> = Vec::new();
    for (index, mut table) in old.into_iter().enumerate() {
        let Some(path) = written_image_path(&table) else {
            kept.push((Some(index), table));
            continue;
        };
        let found = images
            .iter()
            .enumerate()
            .position(|(i, image)| !used[i] && image.path == path);
        if let Some(found) = found {
            used[found] = true;
            update_image_table(&mut table, &images[found]);
            kept.push((Some(index), table));
        }
    }
    for (image, _) in images.iter().zip(&used).filter(|(_, used)| !**used) {
        let mut table = image_table(image);
        table.decor_mut().set_prefix("\n");
        table.set_position(position);
        kept.push((None, table));
    }
    if kept.is_empty() {
        let following = following(doc, BACKGROUND_PATH);
        let after = following_prefix(doc, &following);
        let rest = after.strip_prefix('\n').unwrap_or(&after);
        let head = match following {
            // ファイルの最後なら、テーブルの前に空けた空行を残さない。
            Following::End => {
                let comments = head.trim_end_matches('\n');
                if comments.trim().is_empty() {
                    String::new()
                } else {
                    format!("{comments}\n")
                }
            }
            Following::Table(_) => head,
        };
        set_following_prefix(doc, &following, format!("{head}{rest}"));
        return;
    }
    if let Some((from, first)) = kept.first_mut()
        && *from != Some(0)
    {
        let own = decor_prefix(first.decor());
        let own = own.strip_prefix('\n').unwrap_or(&own);
        first.decor_mut().set_prefix(format!("{head}{own}"));
    }
    let mut array = ArrayOfTables::new();
    for (_, table) in kept {
        array.push(table);
    }
    if let Some(background) = table_at_mut(doc, BACKGROUND_PATH) {
        background.insert(IMAGES, Item::ArrayOfTables(array));
    }
}

/// `[background]` の値の後ろに、`[[background.images]]` のテーブル `tables` を置く（テーブルはまだ
/// 書いていないものとする）。`tables` はそれぞれ、テーブルの前に置くコメントの行とテーブル。
/// `[background]` がなければ作る。続くテーブルの前のコメントのうち、空行で見出しと離れたところまでを
/// 最初のテーブルの前に移す（[`TEMPLATE`] の一覧の説明の下にテーブルが入る）。`trailing` は最後の
/// テーブルの後ろに置くコメントの行。
///
/// [`TEMPLATE`]: crate::config::TEMPLATE
pub(crate) fn insert_image_tables(
    doc: &mut DocumentMut,
    tables: Vec<(String, Table)>,
    trailing: String,
) -> Result<(), String> {
    let background = section_mut(doc, BACKGROUND_PATH, true)?.expect("created");
    if !is_standard(background) {
        return Err("background is not written as a [background] table".to_owned());
    }
    let position = background.position();
    // 値の行があれば、その後ろに空行を挟む。
    let mut first = if value_prefixes(background).is_empty() {
        String::new()
    } else {
        "\n".to_owned()
    };
    let following = following(doc, BACKGROUND_PATH);
    match following {
        Following::Table(_) => {
            let after = following_prefix(doc, &following);
            let (moved, rest) = split_after_last_blank_line(&after);
            if !moved.is_empty() {
                first = moved;
            }
            set_following_prefix(doc, &following, format!("{trailing}\n{rest}"));
        }
        Following::End => {
            let after = following_prefix(doc, &following);
            set_following_prefix(doc, &following, format!("{trailing}{after}"));
        }
    }
    let mut array = ArrayOfTables::new();
    for (index, (comments, mut table)) in tables.into_iter().enumerate() {
        let separator = if index == 0 { first.as_str() } else { "\n" };
        table
            .decor_mut()
            .set_prefix(format!("{separator}{comments}"));
        table.set_position(position);
        array.push(table);
    }
    let background = section_mut(doc, BACKGROUND_PATH, false)?.expect("exists");
    background.insert(IMAGES, Item::ArrayOfTables(array));
    Ok(())
}

/// 書いてある `[[background.images]]` のテーブル一つ `table` の表示方法と不透明度を、`image` に合わせる。
/// 同じ値は変えない。パスは書いてあった形（引用符の種類など）のまま、知らないキーとコメントも残す。
fn update_image_table(table: &mut Table, image: &BackgroundImage) {
    let (fit, opacity) = written_image_settings(table);
    if fit != image.fit {
        match image.fit {
            Some(fit) => set_table_value(table, IMAGE_FIT, Value::from(fit.name())),
            None => remove_table_value(table, IMAGE_FIT),
        }
    }
    if opacity != image.image_opacity {
        match image.image_opacity {
            Some(opacity) => set_table_value(table, IMAGE_OPACITY, number_value(opacity)),
            None => remove_table_value(table, IMAGE_OPACITY),
        }
    }
}

/// テーブル `table` の値の行 `key` を消す。その行の前に書いたコメントは、次の値の行の前に移す（次の
/// 値の行がなければ消す）。
fn remove_table_value(table: &mut Table, key: &str) {
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

/// フォントの大きさや画像の不透明度の数。整数なら整数で、そうでなければ小数で書く（`f32` を `f64` に
/// 広げた端数を出さないように、`f32` の短い表記から読み直す）。
pub(crate) fn number_value(number: f32) -> Value {
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
pub(crate) fn bare_repr(value: &Value) -> String {
    let mut value = value.clone();
    value.decor_mut().clear();
    value.to_string()
}

fn same(a: &Option<Value>, b: &Option<Value>) -> bool {
    a.as_ref().map(bare_repr) == b.as_ref().map(bare_repr)
}

/// 書き換える項目の、テーブルの中のキー。
struct Target<'a> {
    /// 足すときに書くキーの名前。
    name: String,
    /// テーブルのキーの名前が、この項目のものか。
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
/// 先頭の BOM と、改行が CRLF かどうかは `text` に合わせる。古い書き方は今の書き方に読み替えてから
/// 書き換える（`base` は読み替えた中身から読んだ設定）。
pub fn update(text: &str, base: &Config, edited: &Config) -> Result<String, String> {
    let migrated = migrate::migrate(text);
    let mut document = Document::parse(&migrated.text)?;
    let doc = &mut document.doc;
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
                set(doc, setting.path(), &target, value)?;
            }
            _ => {
                let reset = match default {
                    Some(default) => Reset::Comment(default),
                    None => Reset::CommentOld,
                };
                self::reset(doc, setting.path(), &target, reset)?;
            }
        }
    }
    update_background_images(doc, base, edited)?;
    update_profiles(doc, base, edited)?;
    update_colors(doc, base, edited)?;
    update_keys(doc, base, edited)?;
    Ok(document.finish())
}

/// `[colors]` と `[colors.…]` のテーブルで、`base` と `edited` で違う色を書き換える。既定の色に戻した
/// 色は、既定の色が配色やシステムの色などで決まり一つに書けないので、それまでの値をコメントにする。
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

/// `[keys]` のテーブルで、`base` と `edited` で割り当てが違う組み合わせを書き換える。
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
/// プロファイルが使われ、`shell` は使われないため）。既定のプロファイルだけに戻したら、テーブルを消す。
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

/// プロファイル一つの、インラインテーブル。
fn profile_inline_table(profile: &Profile) -> Value {
    let mut table = InlineTable::new();
    table.insert(PROFILE_NAME, Value::from(profile.name.as_str()));
    if let Some(command) = profile_command(profile) {
        table.insert(PROFILE_COMMAND, Value::from(command));
    }
    if profile.backend != BackendKind::default() {
        table.insert(PROFILE_BACKEND, Value::from(profile.backend.name()));
    }
    for (key, text) in [
        (PROFILE_CYGWIN_ROOT, &profile.cygwin_root),
        (PROFILE_DISTRIBUTION, &profile.distribution),
        (PROFILE_ICON, &profile.icon),
    ] {
        if let Some(text) = text {
            table.insert(key, Value::from(text.as_str()));
        }
    }
    table.fmt();
    Value::InlineTable(table)
}

/// `[[profiles]]` のテーブルを、`wanted` に合わせて `wanted` の順に並べ直す。書いてあるテーブルとは
/// 名前で照合し、一致しなかったものはコマンドラインで照合する（名前だけを変えたとき）。一致したテーブルは、
/// 変えた値だけを書き換えてコメントと知らないキーを残す。一致しなかったテーブルは消し、新しいプロファイルは
/// テーブルを作る。名前のないテーブル（読めないテーブル）は最後にそのまま残す。
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
    // wanted の位置から、書いてあるテーブルの位置へ。
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
            !used[index] && named(index) && written[index].1.as_deref() == profile_command(want)
        }) {
            used[index] = true;
            *pair = Some(index);
        }
    }
    // プロファイルのテーブルの後ろに続くテーブル（プロファイルのテーブルがなければ、最初のテーブル）。
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
    // 新しい並び。テーブルと、書いてあった位置（新しいテーブルは `None`）。
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
    // 同じ位置のテーブルは配列の順に並ぶので、どれも書いてあった最初のテーブルの位置（なければ続く
    // テーブルのすぐ前）にする。
    let position = old
        .iter()
        .filter_map(Table::position)
        .min()
        .or(following_position.map(|position| position - 1))
        .unwrap_or(0);
    // 最初のテーブルの前のコメント（ファイルの先頭のコメントなど）は、並べ直しても最初のテーブルの前に
    // 置く。
    match old.first().map(|table| decor_prefix(table.decor())) {
        Some(head) => {
            if tables.is_empty() {
                // テーブルがなくなるので、続くテーブルの前へ返す。
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
            // 続くテーブルの前のコメントのうち、空行で見出しと離れたところまでを最初のテーブルの前に
            // 移す。ファイルの最後のコメントは、テーブルの後ろに残す。
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
    set_or_remove_table_text(table, PROFILE_COMMAND, profile_command(profile));
    if table.contains_key(PROFILE_BACKEND) || profile.backend != BackendKind::default() {
        set_table_text(table, PROFILE_BACKEND, profile.backend.name());
    }
    set_or_remove_table_text(table, PROFILE_CYGWIN_ROOT, profile.cygwin_root.as_deref());
    set_or_remove_table_text(table, PROFILE_DISTRIBUTION, profile.distribution.as_deref());
    set_or_remove_table_text(table, PROFILE_ICON, profile.icon.as_deref());
}

/// プロファイルの `command` に書く値。ブリッジで空なら（相手のログインシェル）書かない。
fn profile_command(profile: &Profile) -> Option<&str> {
    (!(profile.backend.is_bridge() && profile.command.is_empty())).then_some(&profile.command)
}

/// テーブル `table` の `key` を、`text` があればその文字列に、なければ消す。
fn set_or_remove_table_text(table: &mut Table, key: &str, text: Option<&str>) {
    match text {
        Some(text) => set_table_text(table, key, text),
        None => {
            table.remove(key);
        }
    }
}

/// テーブル `table` の `key` の値を文字列 `text` にする。同じなら（引用符の種類も）変えない。行の
/// コメントは残す。
fn set_table_text(table: &mut Table, key: &str, text: &str) {
    if table.get(key).and_then(Item::as_str) == Some(text) {
        return;
    }
    set_table_value(table, key, Value::from(text));
}

/// テーブル `table` の `key` の値を `value` にする。書いてあれば行のコメントは残し、なければテーブルの
/// 最後に足す。
fn set_table_value(table: &mut Table, key: &str, mut value: Value) {
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
pub(crate) fn is_standard(table: &Table) -> bool {
    !table.is_dotted()
}

fn path_name(path: &[&str]) -> String {
    path.join(".")
}

/// キーのパス `path` のテーブル。
pub(crate) fn table_at<'a>(doc: &'a DocumentMut, path: &[&str]) -> Option<&'a Table> {
    let mut table = doc.as_table();
    for name in path {
        table = table.get(name)?.as_table()?;
    }
    Some(table)
}

pub(crate) fn table_at_mut<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Option<&'a mut Table> {
    let mut table = doc.as_table_mut();
    for name in path {
        table = table.get_mut(name)?.as_table_mut()?;
    }
    Some(table)
}

/// キーのパス `path` のテーブル。無ければ、`create` なら最後に作る（途中のテーブルは見出しを書かない
/// テーブルにする）。テーブルでないものが書いてあればエラー。
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

/// 見出しを書いたテーブルと、ファイルの中の位置。見出しを書かないテーブル（`[colors.tab_bar]` の
/// `colors` など）は含めず、その中のテーブルを含める。テーブルの配列は、そのテーブルを一つずつ含める。
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

/// テーブルの値の行のあとに続く場所。次のテーブルのヘッダーの前か、ファイルの最後。
#[derive(Debug)]
enum Following {
    Table(Header),
    End,
}

/// `path` のテーブルの最後の値の行のあとに続く場所。
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

/// `me` のほかの、位置が `after` より後ろ（`None` ならどこでも）の、いちばん前のテーブル。`me` の中の
/// テーブル（`[background]` の中の `[[background.images]]` など）は、位置が `after` と同じでも後ろに
/// あるものとする（同じ位置のテーブルは、外のテーブル、中のテーブルの順に書き出される）。
fn first_after(doc: &DocumentMut, me: &Header, after: Option<isize>) -> Following {
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

pub(crate) fn decor_prefix(decor: &Decor) -> String {
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
pub(crate) fn value_prefixes(table: &Table) -> Vec<(String, String)> {
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

/// テーブルの値の行の `index` 番目の前に、`key` の行を入れる。
pub(crate) fn insert_at(table: &mut Table, index: usize, key: Key, value: Value) {
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
pub(crate) fn replace_line(doc: &mut DocumentMut, path: &[&str], name: &str, line: Option<String>) {
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
        // 説明の行はそのまま、コメントにした行が値の行になる。足した組み合わせはテーブルの最後。
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
            .replace("# image_opacity = 0.2", "image_opacity = 0.65")
            .replace(r#"# fit = "fill""#, r#"fit = "center""#)
            .replace(
                "# 例: fit = \"tile\"\n\n[keys]",
                "# 例: fit = \"tile\"\n\n[[background.images]]\npath = 'C:\\walls'\n\n[[background.images]]\npath = 'D:\\pics\\*.png'\n\n[keys]",
            )
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
        // プロファイルはテーブルを消す。
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
        // 見出しのすぐ上のコメントは、そのテーブルのものとして見出しの上に残す。
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

        // 最後の項目なら、続くテーブルのヘッダーの前に置く。
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
            "[font]\nsize = 9\n\n[background]\nimage_opacity = 1\n\n[[background.images]]\npath = 'C:\\walls'\n"
        );
        assert_reads_as(&updated, &edited);
        // 一覧を空に戻すとテーブルを消し、不透明度は既定値をコメントにした行にする。
        let back = update(&updated, &edited, &base).unwrap();
        assert_eq!(
            back,
            "[font]\nsize = 9\n\n[background]\n# image_opacity = 0.2\n"
        );
        assert_reads_as(&back, &base);
    }

    #[test]
    fn background_header_is_written_when_only_image_tables_are_written() {
        let text = "[font]\nsize = 9\n\n[[background.images]]\npath = 'a'\n\n[keys]\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited.background.image_opacity = 0.5;
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "[font]\nsize = 9\n\n[background]\nimage_opacity = 0.5\n\n[[background.images]]\npath = 'a'\n\n[keys]\n"
        );
        assert_reads_as(&updated, &edited);
    }

    #[test]
    fn old_forms_are_read_in_the_current_form_before_writing() {
        let text = "[background]\nimages = ['C:\\a.png'] # mine\nopacity = 0.7\n";
        let base = config::parse(&migrate::migrate(text).text, &Config::default()).config;
        let mut edited = base.clone();
        edited.background.images[0].fit = Some(BackgroundFit::Tile);
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "[background]\nimage_opacity = 0.3\n\n[[background.images]]\npath = 'C:\\a.png'\nfit = \"tile\"\n# mine\n"
        );
        assert_reads_as(&updated, &edited);
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
    fn image_tables_change_only_the_changed_values() {
        let text = "[background]\nfit = 'fit'\n\n# walls\n[[background.images]]\npath = 'C:\\walls'\n\n[[background.images]]\npath = 'C:\\a.png' # the cat\n\n[[background.images]]\n# tiled\npath = 'C:\\b.png'\nfit = 'tile' # mine\nimage_opacity = 0.5\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited.background.images[0].image_opacity = Some(0.3);
        edited.background.images[1].fit = Some(BackgroundFit::Center);
        edited.background.images[2].fit = None;
        let updated = update(text, &base, &edited).unwrap();
        // コメントと並びは残し、変えた値だけを書き換える。パスの引用符は書いたまま。
        assert_eq!(
            updated,
            "[background]\nfit = 'fit'\n\n# walls\n[[background.images]]\npath = 'C:\\walls'\nimage_opacity = 0.3\n\n[[background.images]]\npath = 'C:\\a.png' # the cat\nfit = \"center\"\n\n[[background.images]]\n# tiled\npath = 'C:\\b.png'\nimage_opacity = 0.5\n"
        );
        assert_reads_as(&updated, &edited);
        // 全体の値に従うように戻すと、その行を消す。
        let back = update(&updated, &edited, &base).unwrap();
        assert_eq!(
            back,
            "[background]\nfit = 'fit'\n\n# walls\n[[background.images]]\npath = 'C:\\walls'\n\n[[background.images]]\npath = 'C:\\a.png' # the cat\n\n[[background.images]]\n# tiled\npath = 'C:\\b.png'\nimage_opacity = 0.5\nfit = \"tile\"\n"
        );
        assert_reads_as(&back, &base);
    }

    #[test]
    fn image_tables_are_removed_and_added_keeping_the_others() {
        let text = "[background]\n[[background.images]]\npath = 'C:\\a.png'\n\n# b\n[[background.images]]\npath = 'C:\\b.png'\nfit = 'tile'\nnote = 'x'\n\n[[background.images]]\nfit = 'center'\n\n[[background.images]]\npath = 'C:\\c.png'\n\n[keys]\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        // 先頭を外し、知らないキーのあるテーブルを全体の値に従わせ、二つ足す。
        edited.background.images.remove(0);
        edited.background.images[0].fit = None;
        edited
            .background
            .images
            .push(image(r"D:\d.png", Some(BackgroundFit::Fit), None));
        edited.background.images.push(BackgroundImage::new(r"D:\e"));
        let updated = update(text, &base, &edited).unwrap();
        // 読めないテーブル（パスのないテーブル）と知らないキーはそのまま。足すテーブルは最後のテーブルの
        // 後ろ。
        assert_eq!(
            updated,
            "[background]\n# b\n[[background.images]]\npath = 'C:\\b.png'\nnote = 'x'\n\n[[background.images]]\nfit = 'center'\n\n[[background.images]]\npath = 'C:\\c.png'\n\n[[background.images]]\npath = 'D:\\d.png'\nfit = \"fit\"\n\n[[background.images]]\npath = 'D:\\e'\n\n[keys]\n"
        );
        let reread = config::parse(&updated, &Config::default());
        assert_eq!(reread.config.background, edited.background);

        // 一覧を空にするとテーブルを消し、最初のテーブルの前のコメントは続くテーブルの前に残す。
        let text = "[background]\nfit = 'tile'\n\n# my images\n[[background.images]]\npath = 'a'\n\n[[background.images]]\npath = 'b'\n\n[keys]\n";
        let base = config::parse(text, &Config::default()).config;
        let mut edited = base.clone();
        edited.background.images.clear();
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "[background]\nfit = 'tile'\n\n# my images\n[keys]\n"
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
        // 一覧の説明の下、続くテーブルの前に置く。
        assert_eq!(
            text,
            TEMPLATE.replace(
                "# 例: fit = \"tile\"\n\n[keys]",
                "# 例: fit = \"tile\"\n\n[[background.images]]\npath = 'C:\\walls'\n\n[[background.images]]\npath = 'C:\\tiles'\nfit = \"tile\"\nimage_opacity = 0.25\n\n[keys]"
            )
        );
        assert_eq!(
            update(&text, &edited, &Config::default()).unwrap(),
            TEMPLATE
        );
    }

    fn profile(name: &str, command: &str) -> Profile {
        Profile::new(name, command, BackendKind::Conpty)
    }

    #[test]
    fn bridge_profiles_write_their_own_keys() {
        let parse = |text: &str| config::parse(text, &Config::default()).config;
        let text = "[[profiles]]\nname = 'cyg'\nbackend = 'cygwin'\ncommand = 'zsh -l' # mine\n\n[[profiles]]\nname = 'u'\nbackend = 'wsl'\ndistribution = 'Ubuntu'\n\n[font]\nsize = 9\n";
        let base = parse(text);
        assert_eq!(base.profiles[1].distribution.as_deref(), Some("Ubuntu"));
        let mut edited = base.clone();
        edited.profiles[0].command = String::new();
        edited.profiles[0].cygwin_root = Some(r"C:\app\cygwin".to_owned());
        edited.profiles[1].distribution = None;
        edited.profiles.push(Profile {
            distribution: Some("Debian".to_owned()),
            ..Profile::new("d", "bash", BackendKind::Wsl)
        });
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "[[profiles]]\nname = 'cyg'\nbackend = 'cygwin'\ncygwin_root = 'C:\\app\\cygwin'\n\n[[profiles]]\nname = 'u'\nbackend = 'wsl'\n\n[[profiles]]\nname = \"d\"\ncommand = \"bash\"\nbackend = \"wsl\"\ndistribution = \"Debian\"\n\n[font]\nsize = 9\n"
        );
        assert_eq!(parse(&updated).profiles, edited.profiles);

        let text = "profiles = [{ name = 'a', command = 'cmd.exe' }]\n";
        let base = parse(text);
        let mut edited = base.clone();
        edited.profiles[0] = Profile {
            cygwin_root: Some("C:/cygwin".to_owned()),
            ..Profile::new("a", "", BackendKind::Cygwin)
        };
        assert_eq!(
            update(text, &base, &edited).unwrap(),
            "profiles = [{ name = \"a\", backend = \"cygwin\", cygwin_root = \"C:/cygwin\" }]\n"
        );
    }

    #[test]
    fn profile_icons_are_written_and_removed() {
        let parse = |text: &str| config::parse(text, &Config::default()).config;
        let text = "[[profiles]]\nname = 'a'\ncommand = 'cmd.exe'\nicon = 'old.png' # mine\n\n[[profiles]]\nname = 'b'\ncommand = 'pwsh.exe'\n\n[font]\nsize = 9\n";
        let base = parse(text);
        let mut edited = base.clone();
        edited.profiles[0].icon = None;
        edited.profiles[1].icon = Some(r"C:\pics\b.png".to_owned());
        edited.profiles.push(Profile {
            icon: Some("c.ico".to_owned()),
            ..profile("c", "zsh")
        });
        let updated = update(text, &base, &edited).unwrap();
        assert_eq!(
            updated,
            "[[profiles]]\nname = 'a'\ncommand = 'cmd.exe'\n\n[[profiles]]\nname = 'b'\ncommand = 'pwsh.exe'\nicon = 'C:\\pics\\b.png'\n\n[[profiles]]\nname = \"c\"\ncommand = \"zsh\"\nicon = \"c.ico\"\n\n[font]\nsize = 9\n"
        );
        assert_eq!(parse(&updated).profiles, edited.profiles);

        let text = "profiles = [{ name = 'a', command = 'cmd.exe' }]\n";
        let base = parse(text);
        let mut edited = base.clone();
        edited.profiles[0].icon = Some("a.png".to_owned());
        assert_eq!(
            update(text, &base, &edited).unwrap(),
            "profiles = [{ name = \"a\", command = \"cmd.exe\", icon = \"a.png\" }]\n"
        );
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
        // 名前もコマンドラインも違う c のテーブルは、どのテーブルとも一致しないので作り直す。先頭の
        // コメントは最初のテーブルの前に残す。
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
        // shell はコメントにし、テーブルは最初のテーブルのヘッダーの前に置く。
        assert_eq!(
            updated,
            "# head\n# shell = 'zsh -l' # mine\n\n[[profiles]]\nname = \"zsh\"\ncommand = \"zsh -l\"\n\n[[profiles]]\nname = \"cmd\"\ncommand = \"cmd.exe\"\n\n# about font\n[font]\nsize = 9\n"
        );
        assert_reads_as(&updated, &edited);

        // 何もないファイルと、テーブルのないファイル。
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
