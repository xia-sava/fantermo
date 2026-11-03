//! 設定ファイルの中身（TOML）を [`Config`] に解釈する。
//!
//! 項目ごとに読み、警告にはエラーのある行を添える。書く項目の一覧は [`SCALARS`]、プロファイルと色の
//! 決まりは [`Profile`] と [`ColorsConfig`] にある。

use std::{collections::HashMap, fmt::Display, ops::RangeInclusive};

use serde::{Deserialize, de::DeserializeOwned};
use toml::{
    Table, Value,
    de::{DeTable, DeValue},
};

use super::{
    BACKGROUND_TABLE, BackendKind, BackgroundConfig, BackgroundFit, BackgroundImage, COLORS_TABLE,
    ColorTable, ColorsConfig, Config, FONT_TABLE, FontConfig, IMAGE_KEYS, IMAGE_OPACITY_RANGE,
    PROFILE_KEYS, Profile, ProfileError, SCALARS, Scalar, ScalarKind, key_path, parse_color,
    profile::{check_profile_command, check_profile_name},
    profile_position, value_named,
};
use crate::{
    frame::Rgb,
    keymap::{self, KeyCombo, Keymap},
};

/// 解釈した設定と、読めなかった項目についての警告。
#[derive(Debug)]
pub struct Parsed {
    pub config: Config,
    pub warnings: Vec<Warning>,
    /// TOML として読めなかった。`config` は前の設定のままで、`warnings` はそのエラーの一つだけ。
    pub syntax_error: bool,
}

/// 設定ファイルのエラーについての警告。
#[derive(Clone, Debug, PartialEq)]
pub struct Warning {
    /// エラーのある行（1 から数える）。分からなければ `None`。
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

    /// 入れる値。読めた値か、書いていなければ `default`。読めなければ `None`。
    fn value_or(self, default: T) -> Option<T> {
        match self {
            Item::Absent => Some(default),
            Item::Invalid => None,
            Item::Valid(value) => Some(value),
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

    /// 読めた値を `check` でチェックする。`check` が警告の文を返したら、警告を残して読めなかったことにする。
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
enum Strings {
    One(String),
    Many(Vec<String>),
}

impl Strings {
    fn into_vec(self) -> Vec<String> {
        match self {
            Strings::One(one) => vec![one],
            Strings::Many(many) => many,
        }
    }
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
    let defaults = Config::default();
    read_profiles(&mut reader, &mut root, &mut config);
    for name in scalar_tables() {
        // 書いていないテーブルは、どの項目も書いていない空のテーブルとして読み、既定値にする。
        // テーブルでなければ、どの項目も前の値のまま。
        let mut table = match reader.take::<Table>(&mut root, None, name) {
            Item::Absent => Table::new(),
            Item::Invalid => continue,
            Item::Valid(table) => table,
        };
        read_table(&mut reader, name, &mut table, &mut config, &defaults);
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

/// 最上位の `profiles`・`shell`・`default_profile` を読む。`profiles` に使えるプロファイルがあれば
/// それを使い、`shell` は警告して使わない。なければ `shell` から作った一つ、それもなければ既定の
/// プロファイル。`profiles` が読めなければ（配列でなければ）、前のプロファイルのまま。
fn read_profiles(reader: &mut Reader, root: &mut Table, config: &mut Config) {
    const PROFILES: &str = "profiles";
    const SHELL: &str = "shell";
    let profiles = match reader.take::<Value>(root, None, PROFILES) {
        Item::Valid(Value::Array(items)) => {
            if items.is_empty() {
                reader.warn(
                    PROFILES,
                    format!("ignoring {PROFILES}: no profiles are written"),
                );
            }
            let mut profiles: Vec<Profile> = Vec::new();
            for (index, item) in items.into_iter().enumerate() {
                let path = format!("{PROFILES}[{index}]");
                let Some(profile) = read_profile(reader, &path, item) else {
                    continue;
                };
                if profile_position(&profiles, &profile.name).is_some() {
                    reader.warn(
                        &key_path(Some(&path), PROFILE_KEYS.name),
                        format!(
                            "ignoring {path}: the name {:?} is already used above",
                            profile.name
                        ),
                    );
                    continue;
                }
                profiles.push(profile);
            }
            Item::Valid(profiles)
        }
        Item::Valid(_) => {
            reader.warn(
                PROFILES,
                format!("ignoring {PROFILES}: expected an array of tables ([[{PROFILES}]])"),
            );
            Item::Invalid
        }
        Item::Absent => Item::Absent,
        Item::Invalid => Item::Invalid,
    };
    let shell = reader
        .take::<String>(root, None, SHELL)
        .check(reader, SHELL, |shell| {
            shell
                .trim()
                .is_empty()
                .then(|| format!("ignoring {SHELL}: empty"))
        });
    match profiles {
        Item::Valid(profiles) if !profiles.is_empty() => {
            if let Item::Valid(_) = shell {
                reader.warn(
                    SHELL,
                    format!("ignoring {SHELL}: {PROFILES} are written, so they are used"),
                );
            }
            config.profiles = profiles;
        }
        Item::Invalid => {}
        Item::Valid(_) | Item::Absent => match shell {
            Item::Valid(shell) => config.profiles = vec![Profile::from_shell(&shell)],
            Item::Absent => config.profiles = vec![Profile::builtin()],
            Item::Invalid => {}
        },
    }
    const DEFAULT_PROFILE: &str = "default_profile";
    let names = |config: &Config, name: &str| config.profiles.iter().any(|p| p.name == name);
    match reader.take::<String>(root, None, DEFAULT_PROFILE) {
        Item::Valid(name) if names(config, &name) => config.default_profile = Some(name),
        Item::Valid(name) => {
            reader.warn(
                DEFAULT_PROFILE,
                format!("ignoring {DEFAULT_PROFILE} = {name:?}: no profile has that name"),
            );
            config.default_profile = None;
        }
        Item::Absent => config.default_profile = None,
        Item::Invalid => {
            // 前の名前が、読んだプロファイルにもあればそのまま。
            if let Some(name) = config.default_profile.take()
                && names(config, &name)
            {
                config.default_profile = Some(name);
            }
        }
    }
}

/// `profiles` の項目一つ（キーのパスは `path`）を読む。テーブルで、`name` が空でない文字列で、`backend` が
/// 書いていないか知っている名前で、`command` が空でない文字列（ブリッジなら書かなくても空でもよい）なら、
/// そのプロファイル。そうでなければ警告して `None`。`cygwin_root`・`distribution` は、その種類の
/// プロファイルでなければ警告して使わない。
fn read_profile(reader: &mut Reader, path: &str, value: Value) -> Option<Profile> {
    let Value::Table(mut table) = value else {
        reader.warn(path, format!("ignoring {path}: expected a table"));
        return None;
    };
    let section = Some(path);
    let backend = read_profile_backend(reader, &mut table, path);
    // `check` はプロファイルの決まり。空（空白だけ）を断る決まりなら、書いていないものも断る。断らない
    // なら、空白だけは空にする。
    let text = |reader: &mut Reader,
                table: &mut Table,
                key: &str,
                check: &dyn Fn(&str) -> Result<(), ProfileError>| match reader
        .take::<String>(table, section, key)
    {
        Item::Valid(text) if check(&text).is_err() => {
            reader.warn(
                &key_path(section, key),
                format!("ignoring {path}: {key} must not be empty"),
            );
            None
        }
        Item::Valid(text) if text.trim().is_empty() => Some(String::new()),
        Item::Valid(text) => Some(text),
        Item::Absent if check("").is_err() => {
            reader.warn(path, format!("ignoring {path}: expected a {key}"));
            None
        }
        Item::Absent => Some(String::new()),
        Item::Invalid => None,
    };
    let name = text(reader, &mut table, PROFILE_KEYS.name, &check_profile_name);
    // 接続方式を読めなければ、既定の ConPTY の決まりでコマンドラインを読む。
    let command_backend = backend.unwrap_or_default();
    let command = text(reader, &mut table, PROFILE_KEYS.command, &|command| {
        check_profile_command(command, command_backend)
    });
    let cygwin_root = read_profile_option(
        reader,
        &mut table,
        path,
        PROFILE_KEYS.cygwin_root,
        backend,
        BackendKind::Cygwin,
    );
    let distribution = read_profile_option(
        reader,
        &mut table,
        path,
        PROFILE_KEYS.distribution,
        backend,
        BackendKind::Wsl,
    );
    let icon = read_profile_icon(reader, &mut table, path);
    reader.warn_unknown(&table, section);
    Some(Profile {
        name: name?,
        command: command?,
        backend: backend?,
        cygwin_root,
        distribution,
        icon,
    })
}

/// プロファイルのテーブルの `icon` を読む。書いていない・空なら（空は警告して）`None`。
fn read_profile_icon(reader: &mut Reader, table: &mut Table, path: &str) -> Option<String> {
    const ICON: &str = PROFILE_KEYS.icon;
    let section = Some(path);
    let Item::Valid(text) = reader.take::<String>(table, section, ICON) else {
        return None;
    };
    if text.trim().is_empty() {
        let key_path = key_path(section, ICON);
        reader.warn(&key_path, format!("ignoring {key_path}: empty"));
        return None;
    }
    Some(text)
}

/// プロファイルのテーブルの `cygwin_root` のような、種類が `owner` のプロファイルだけが使うキー `key` を
/// 読む。書いていない・空・ほかの種類なら（警告して）`None`。
fn read_profile_option(
    reader: &mut Reader,
    table: &mut Table,
    path: &str,
    key: &str,
    backend: Option<BackendKind>,
    owner: BackendKind,
) -> Option<String> {
    let section = Some(path);
    let Item::Valid(text) = reader.take::<String>(table, section, key) else {
        return None;
    };
    let key_path = key_path(section, key);
    if text.trim().is_empty() {
        reader.warn(&key_path, format!("ignoring {key_path}: empty"));
        return None;
    }
    if backend.is_some_and(|backend| backend != owner) {
        reader.warn(
            &key_path,
            format!(
                "ignoring {key_path}: used only with backend = {:?}",
                owner.name()
            ),
        );
        return None;
    }
    Some(text)
}

/// プロファイルのテーブルの `backend` を読む。書いていなければ既定の種類、知らない名前なら警告して `None`。
fn read_profile_backend(reader: &mut Reader, table: &mut Table, path: &str) -> Option<BackendKind> {
    let section = Some(path);
    match reader.take::<String>(table, section, PROFILE_KEYS.backend) {
        Item::Valid(name) => match BackendKind::from_name(&name) {
            Some(kind) => Some(kind),
            None => {
                let known: Vec<&str> = BackendKind::ALL.iter().map(|kind| kind.name()).collect();
                reader.warn(
                    &key_path(section, PROFILE_KEYS.backend),
                    format!("ignoring {path}: unknown backend {name:?}, expected one of {known:?}"),
                );
                None
            }
        },
        Item::Absent => Some(BackendKind::default()),
        Item::Invalid => None,
    }
}

/// [`parse`] が [`read_table`] で読む最上位のテーブル。[`SCALARS`] に出てくる順。一覧の項目を持たない
/// テーブル（`[keys]` など）は入らないので、[`parse`] に別に書く。
fn scalar_tables() -> Vec<&'static str> {
    let mut tables: Vec<&'static str> = Vec::new();
    for scalar in SCALARS {
        if !tables.contains(&scalar.table) {
            tables.push(scalar.table);
        }
    }
    tables
}

/// 最上位のテーブル `name` の中身 `table` を読む。[`SCALARS`] の項目のほかに、一覧に載らない項目も、
/// 設定ファイルの説明と同じ順に読む（`font.family` と `background.images` は一覧の項目より前、`[colors]`
/// の色は後）。最後に、残ったキーを知らないものとして警告する。
fn read_table(
    reader: &mut Reader,
    name: &str,
    table: &mut Table,
    config: &mut Config,
    defaults: &Config,
) {
    match name {
        FONT_TABLE => read_font_family(reader, table, &mut config.font.families),
        BACKGROUND_TABLE => read_background_images(reader, table, &mut config.background.images),
        _ => {}
    }
    for scalar in SCALARS.iter().filter(|scalar| scalar.table == name) {
        read_scalar(reader, table, scalar, config, defaults);
    }
    if name == COLORS_TABLE {
        read_colors(reader, table, &mut config.colors);
    }
    reader.warn_unknown(table, Some(name));
}

/// テーブル `table` から一覧の項目 `scalar` を読んで `config` に入れる。書いていなければ `defaults` の値、
/// 型が違うか範囲の外か知らない名前なら、警告して `config` の値のまま。
fn read_scalar(
    reader: &mut Reader,
    table: &mut Table,
    scalar: &Scalar,
    config: &mut Config,
    defaults: &Config,
) {
    let section = Some(scalar.table);
    let path = scalar.name();
    match &scalar.kind {
        ScalarKind::Float { range, get, set } => {
            let value =
                reader
                    .take::<f32>(table, section, scalar.key)
                    .check(reader, &path, |value| out_of_range(&path, value, range));
            if let Some(value) = value.value_or(get(defaults)) {
                set(config, value);
            }
        }
        ScalarKind::Int { range, get, set } => {
            let value =
                reader
                    .take::<u32>(table, section, scalar.key)
                    .check(reader, &path, |value| out_of_range(&path, value, range));
            if let Some(value) = value.value_or(get(defaults)) {
                set(config, value);
            }
        }
        ScalarKind::Name { names, get, set } => {
            let Some(name) = reader
                .take::<String>(table, section, scalar.key)
                .value_or(get(defaults).to_owned())
            else {
                return;
            };
            if !set(config, &name) {
                reader.warn(&path, unknown_name(&path, &name, names));
            }
        }
    }
}

/// 値 `value` が範囲 `range` の外なら、キー `path` についての警告の文。
fn out_of_range<T: PartialOrd + Display>(
    path: &str,
    value: &T,
    range: &RangeInclusive<T>,
) -> Option<String> {
    (!range.contains(value)).then(|| {
        format!(
            "ignoring {path} = {value}: must be between {} and {}",
            range.start(),
            range.end()
        )
    })
}

/// キー `path` に知らない名前 `name` を書いたことの警告の文。`known` は書ける名前。
fn unknown_name(path: &str, name: &str, known: &[&str]) -> String {
    format!("ignoring {path}: unknown name {name:?}, expected one of {known:?}")
}

/// `[font]` の `family` を読む。
fn read_font_family(reader: &mut Reader, table: &mut Table, families: &mut Vec<String>) {
    const KEY: &str = "family";
    let path = key_path(Some(FONT_TABLE), KEY);
    reader
        .take::<Strings>(table, Some(FONT_TABLE), KEY)
        .map(Strings::into_vec)
        .check(reader, &path, |families| {
            (families.is_empty() || families.iter().any(|family| family.trim().is_empty()))
                .then(|| format!("ignoring {path} = {families:?}: names must not be empty"))
        })
        .apply(families, FontConfig::default().families);
}

/// テーブル `table`（キーのパスは `section`）の `image_opacity` を読む。範囲の外は警告して読めなかったことに
/// する。
fn read_image_opacity(reader: &mut Reader, table: &mut Table, section: Option<&str>) -> Item<f32> {
    let path = key_path(section, IMAGE_KEYS.image_opacity);
    reader
        .take::<f32>(table, section, IMAGE_KEYS.image_opacity)
        .check(reader, &path, |opacity| {
            out_of_range(&path, opacity, &IMAGE_OPACITY_RANGE)
        })
}

/// `[background]` の `images`（`[[background.images]]` のテーブルの配列）を読む。
fn read_background_images(
    reader: &mut Reader,
    table: &mut Table,
    images: &mut Vec<BackgroundImage>,
) {
    let images_item = match reader.take::<Value>(table, Some(BACKGROUND_TABLE), "images") {
        Item::Valid(value) => read_background_image_tables(reader, value),
        Item::Absent => Item::Absent,
        Item::Invalid => Item::Invalid,
    };
    images_item.apply(images, BackgroundConfig::default().images);
}

/// `background.images`（`[[background.images]]` のテーブルの配列）を読む。読めないテーブルは、警告して
/// そのテーブルだけを飛ばす。テーブルの配列でなければ、全体を読めなかったことにする。
fn read_background_image_tables(reader: &mut Reader, value: Value) -> Item<Vec<BackgroundImage>> {
    const PATH: &str = "background.images";
    let Value::Array(items) = value else {
        reader.warn(PATH, format!("ignoring {PATH}: expected [[{PATH}]] tables"));
        return Item::Invalid;
    };
    Item::Valid(
        items
            .into_iter()
            .enumerate()
            .filter_map(|(index, item)| {
                read_background_image(reader, &format!("{PATH}[{index}]"), item)
            })
            .collect(),
    )
}

/// `[[background.images]]` のテーブル一つ（キーのパスは `path`）を読む。`path` と、書いてあれば `fit` と
/// `image_opacity`。読めなければ警告して `None`。読めない `fit` と `image_opacity` は、警告してその値
/// だけを全体の値に任せる。
fn read_background_image(reader: &mut Reader, path: &str, value: Value) -> Option<BackgroundImage> {
    let Value::Table(mut table) = value else {
        reader.warn(path, format!("ignoring {path}: expected a table"));
        return None;
    };
    let section = Some(path);
    let name = match reader.take::<String>(&mut table, section, IMAGE_KEYS.path) {
        Item::Valid(name) if !name.trim().is_empty() => name,
        Item::Valid(_) => {
            reader.warn(
                &key_path(section, IMAGE_KEYS.path),
                format!("ignoring {path}: the path must not be empty"),
            );
            return None;
        }
        Item::Absent => {
            reader.warn(path, format!("ignoring {path}: expected a path"));
            return None;
        }
        Item::Invalid => return None,
    };
    let fit = match reader.take_named(&mut table, section, IMAGE_KEYS.fit, &BackgroundFit::NAMES) {
        Item::Valid(fit) => Some(fit),
        Item::Absent | Item::Invalid => None,
    };
    let image_opacity = match read_image_opacity(reader, &mut table, section) {
        Item::Valid(opacity) => Some(opacity),
        Item::Absent | Item::Invalid => None,
    };
    reader.warn_unknown(&table, section);
    Some(BackgroundImage {
        path: name,
        fit,
        image_opacity,
    })
}

/// `[colors]` の端末の色と、その中の色のテーブルを読む。
fn read_colors(reader: &mut Reader, colors: &mut Table, config: &mut ColorsConfig) {
    const SECTION: Option<&str> = Some(COLORS_TABLE);
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

/// テーブル `values`（キーのパスは `section`）から、`table` の色を `slots` に読む。
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

/// `[keys]` のテーブルを、既定の割り当てに上から順に重ねる。組み合わせを読めない行は飛ばす。動作を
/// 読めない行は、その組み合わせを前の割り当て（`previous`）のままにする。同じ組み合わせを二度書いたら、後の行を
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

/// 項目を読みながら、警告をエラーのある行と一緒に溜める。
struct Reader {
    /// キーのパス（`font.size` など）から、そのキーを書いた行へ。
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
    /// テーブルの名前（最上位なら `None`）。
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

    /// `table` から `key` を取り出して、名前で書く値の表 `names` の名前として読む。文字列でなければ
    /// 警告し、知らない名前なら書ける名前を添えて警告する。
    fn take_named<T: Copy>(
        &mut self,
        table: &mut Table,
        section: Option<&str>,
        key: &str,
        names: &[(T, &'static str)],
    ) -> Item<T> {
        let path = key_path(section, key);
        self.take::<String>(table, section, key)
            .parse(self, &path, |name| {
                value_named(names, &name).ok_or_else(|| {
                    let known: Vec<&str> = names.iter().map(|(_, name)| *name).collect();
                    unknown_name(&path, &name, &known)
                })
            })
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

/// テーブルのキーのパス（`font.size`、`colors.tab_bar.bar` など。キーは引用符を外した名前）から、その
/// キーを書いた行へ。`[font]` のようなテーブルのヘッダーは、ヘッダーの行になる。
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
        match value.get_ref() {
            DeValue::Table(inner) => collect_key_lines(text, inner, Some(&path), lines),
            DeValue::Array(items) => {
                // 配列の要素は `background.images[2]`、その中のテーブルのキーは `background.images[2].fit`。
                for (index, item) in items.iter().enumerate() {
                    let item_path = format!("{path}[{index}]");
                    if let DeValue::Table(inner) = item.get_ref() {
                        collect_key_lines(text, inner, Some(&item_path), lines);
                    }
                    lines.insert(item_path, line_of(text, item.span().start));
                }
            }
            _ => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{colors::hex, profile::program_name, *},
        keymap::Action,
    };

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
            profiles: vec![Profile::from_shell("zsh -l"), Profile::builtin()],
            default_profile: Some(DEFAULT_PROFILE_NAME.to_owned()),
            paste: PasteConfig {
                confirm: PasteConfirm::Never,
                confirm_size: 64,
            },
            clipboard: ClipboardConfig {
                read: ClipboardRead::Deny,
                write: ClipboardWrite::Deny,
            },
            scroll_bar: ScrollBarConfig { width: 20 },
            window: WindowConfig { padding: 10 },
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
            background: BackgroundConfig {
                images: vec![BackgroundImage::new(r"C:\walls")],
                image_opacity: 0.5,
                fit: BackgroundFit::Tile,
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
    fn background_is_read() {
        let parsed = parse(
            "[background]\nimage_opacity = 0.65\nfit = \"center\"\n\n[[background.images]]\npath = 'C:\\walls'\n\n[[background.images]]\npath = 'D:\\a\\*.png'\n",
        );
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(
            parsed.config.background,
            BackgroundConfig {
                images: vec![
                    BackgroundImage::new(r"C:\walls"),
                    BackgroundImage::new(r"D:\a\*.png")
                ],
                image_opacity: 0.65,
                fit: BackgroundFit::Center,
            }
        );
        // `[background]` の見出しはなくてもよく、不透明度は整数でもよい。
        let parsed = parse("[[background.images]]\npath = 'C:\\one.png'\nimage_opacity = 1\n");
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(
            parsed.config.background.images,
            [BackgroundImage {
                path: r"C:\one.png".to_owned(),
                fit: None,
                image_opacity: Some(1.0),
            }]
        );
        // テーブルでない書き方（文字列や文字列の配列）は警告して読まない。
        for (text, warning) in [
            (
                "[background]\nimages = 'C:\\one.png'\n",
                "ignoring background.images: expected [[background.images]] tables",
            ),
            (
                "[background]\nimages = ['C:\\one.png']\n",
                "ignoring background.images[0]: expected a table",
            ),
        ] {
            assert_eq!(parse(text).warnings, [warning]);
        }
        // `opacity` は知らないキー。
        let parsed = parse("[background]\nopacity = 0.8\n");
        assert_eq!(parsed.config.background, BackgroundConfig::default());
        assert_eq!(
            parsed.warnings,
            ["ignoring unknown setting background.opacity"]
        );
        for fit in BackgroundFit::ALL {
            let parsed = parse(&format!("[background]\nfit = '{}'\n", fit.name()));
            assert_eq!(parsed.config.background.fit, fit);
        }
    }

    #[test]
    fn background_images_may_be_tables_with_their_own_fit_and_opacity() {
        let parsed = parse(
            "[background]\nfit = 'fit'\nimage_opacity = 0.4\n\n[[background.images]]\npath = 'C:\\walls'\n\n[[background.images]]\npath = 'C:\\tiles\\*.png'\nfit = 'tile'\n\n[[background.images]]\npath = 'C:\\a.png'\nimage_opacity = 0.3\n\n[[background.images]]\npath = 'C:\\b.png'\nfit = 'center'\nimage_opacity = 1\n",
        );
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        let background = &parsed.config.background;
        assert_eq!(
            background.images,
            [
                BackgroundImage::new(r"C:\walls"),
                BackgroundImage {
                    path: r"C:\tiles\*.png".to_owned(),
                    fit: Some(BackgroundFit::Tile),
                    image_opacity: None,
                },
                BackgroundImage {
                    path: r"C:\a.png".to_owned(),
                    fit: None,
                    image_opacity: Some(0.3),
                },
                BackgroundImage {
                    path: r"C:\b.png".to_owned(),
                    fit: Some(BackgroundFit::Center),
                    image_opacity: Some(1.0),
                },
            ]
        );
        // 書かなかった値は全体の値に従う。項目がなければ全体の値。
        assert_eq!(background.settings_for(0), (BackgroundFit::Fit, 0.4));
        assert_eq!(background.settings_for(1), (BackgroundFit::Tile, 0.4));
        assert_eq!(background.settings_for(2), (BackgroundFit::Fit, 0.3));
        assert_eq!(background.settings_for(3), (BackgroundFit::Center, 1.0));
        assert_eq!(background.settings_for(9), (BackgroundFit::Fit, 0.4));
    }

    #[test]
    fn invalid_background_items_are_reported_with_their_lines() {
        let previous = customized();
        let parsed = super::parse(
            "[background]\n\n[[background.images]]\npath = 'C:\\ok'\n\n[[background.images]]\npath = 'C:\\a.png'\nfit = 'cover'\nimage_opacity = 2\n\n[[background.images]]\nfit = 'tile'\n\n[[background.images]]\npath = ' '\n\n[[background.images]]\npath = 3\n\n[[background.images]]\npath = 'C:\\b.png'\nblur = 1\n",
            &previous,
        );
        // 読めない値は全体の値に任せ、パスのないテーブルと、パスを読めないテーブルは飛ばす。
        assert_eq!(
            parsed.config.background.images,
            [
                BackgroundImage::new(r"C:\ok"),
                BackgroundImage::new(r"C:\a.png"),
                BackgroundImage::new(r"C:\b.png"),
            ]
        );
        // テーブルの中のキーはその行、パスのないテーブルは見出しの行。
        let warnings: Vec<(Option<usize>, &str)> = parsed
            .warnings
            .iter()
            .map(|w| (w.line, w.message.as_str()))
            .collect();
        assert_eq!(warnings.len(), 6, "{warnings:?}");
        assert_eq!(warnings[0].0, Some(8));
        assert!(
            warnings[0]
                .1
                .starts_with("ignoring background.images[1].fit")
        );
        assert_eq!(
            warnings[1..],
            [
                (
                    Some(9),
                    "ignoring background.images[1].image_opacity = 2: must be between 0 and 1"
                ),
                (Some(11), "ignoring background.images[2]: expected a path"),
                (
                    Some(15),
                    "ignoring background.images[3]: the path must not be empty"
                ),
                (
                    Some(18),
                    "ignoring background.images[4].path: invalid type: integer `3`, expected a string"
                ),
                (
                    Some(22),
                    "ignoring unknown setting background.images[5].blur"
                ),
            ]
        );
        // テーブルの配列でなければ、一覧は前のまま。
        let parsed = super::parse("[background]\nimages = 3\n", &previous);
        assert_eq!(parsed.config.background.images, previous.background.images);
        assert_eq!(parsed.warnings.len(), 1);
    }

    #[test]
    fn background_paths_are_compared_without_the_item_settings() {
        let mut a = BackgroundConfig {
            images: vec![BackgroundImage::new("x"), BackgroundImage::new("y")],
            ..BackgroundConfig::default()
        };
        let mut b = a.clone();
        b.images[1].fit = Some(BackgroundFit::Tile);
        b.image_opacity = 1.0;
        assert!(a.same_paths(&b));
        b.images.pop();
        assert!(!a.same_paths(&b));
        a.images.reverse();
        b = a.clone();
        b.images.reverse();
        assert!(!a.same_paths(&b));
    }

    #[test]
    fn invalid_background_values_keep_the_previous_values_with_their_lines() {
        let previous = customized();
        let parsed = super::parse(
            "[background]\nimage_opacity = 1.5\nfit = 'cover'\nblur = 3\n[[background.images]]\npath = 'C:\\ok'\n[[background.images]]\npath = ' '\n",
            &previous,
        );
        // 一覧の空のパスは、そのテーブルだけを飛ばす。
        assert_eq!(
            parsed.config.background,
            BackgroundConfig {
                images: vec![BackgroundImage::new(r"C:\ok")],
                ..previous.background.clone()
            }
        );
        let lines: Vec<Option<usize>> = parsed.warnings.iter().map(|w| w.line).collect();
        assert_eq!(
            lines,
            [Some(2), Some(3), Some(4), Some(8)],
            "{:?}",
            parsed.warnings
        );
        assert!(parsed.warnings[0].message.contains("between 0 and 1"));
        assert!(
            ["fill", "fit", "stretch", "tile", "center"]
                .iter()
                .all(|name| parsed.warnings[1].message.contains(name)),
            "{:?}",
            parsed.warnings
        );
        assert_eq!(
            parsed.warnings[2].message,
            "ignoring unknown setting background.blur"
        );
        assert!(parsed.warnings[3].message.contains("must not be empty"));
        // テーブルでなければ全体が前のまま。書いていなければ既定値。
        let parsed = super::parse("background = 'C:\\walls'\n", &previous);
        assert_eq!(parsed.config.background, previous.background);
        assert_eq!(parsed.warnings.len(), 1);
        let parsed = super::parse("[font]\n", &previous);
        assert_eq!(parsed.config.background, BackgroundConfig::default());
        // 不透明度の範囲の外と、数でないもの。
        let parsed = parse("[background]\nimage_opacity = -0.1\n");
        assert_eq!(
            parsed.config.background.image_opacity,
            DEFAULT_IMAGE_OPACITY
        );
        assert_eq!(parsed.warnings.len(), 1);
        let parsed = parse("[background]\nimage_opacity = nan\nimages = 3\n");
        assert_eq!(parsed.config.background, BackgroundConfig::default());
        assert_eq!(parsed.warnings.len(), 2, "{:?}", parsed.warnings);
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
                default_profile: Some(DEFAULT_PROFILE_NAME.to_owned()),
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
        let mut section = String::new();
        let mut written: Vec<String> = Vec::new();
        for line in text.lines() {
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|line| line.strip_suffix(']'))
            {
                section = name.to_owned();
            } else if let Some((key, _)) = line.split_once(" = ")
                && !line.starts_with('#')
            {
                written.push(if section.is_empty() {
                    key.to_owned()
                } else {
                    format!("{section}.{key}")
                });
            }
        }
        let mut expected: Vec<String> = ["default_profile", "font.family"]
            .map(str::to_owned)
            .into_iter()
            .chain(SCALARS.iter().map(Scalar::name))
            .chain(ColorTable::ALL.into_iter().flat_map(|table| {
                let section = table.path().join(".");
                table
                    .keys()
                    .iter()
                    .map(move |key| format!("{section}.{key}"))
            }))
            .chain(
                Keymap::default()
                    .bindings()
                    .iter()
                    .map(|(combo, _)| format!("keys.\"{combo}\"")),
            )
            .collect();
        written.sort();
        expected.sort();
        assert_eq!(written, expected, "{text}");
        // 背景色と文字の色の例は、既定の配色の色。
        assert_eq!(
            parsed.config.colors.terminal_colors(),
            ColorScheme::default().colors()
        );
    }

    #[test]
    fn template_examples_read_as_written() {
        let text: String = TEMPLATE
            .lines()
            .map(|line| {
                let example = line.strip_prefix("# 例: ").filter(|rest| {
                    rest.starts_with("[[")
                        || rest.split_once(" = ").is_some_and(|(key, _)| {
                            key.starts_with('"')
                                || key.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                        })
                });
                format!("{}\n", example.unwrap_or(line))
            })
            .collect();
        let parsed = parse(&text);
        assert!(parsed.warnings.is_empty(), "{:?}\n{text}", parsed.warnings);
        assert_eq!(
            parsed.config.background.images,
            [
                BackgroundImage::new(r"C:\画像\壁紙"),
                BackgroundImage::new(r"C:\画像\*.jpg"),
                BackgroundImage {
                    path: r"C:\画像\模様.png".to_owned(),
                    fit: Some(BackgroundFit::Tile),
                    image_opacity: None,
                },
            ]
        );
        assert_eq!(
            parsed.config.profiles,
            [
                Profile::builtin(),
                Profile {
                    icon: Some(r"C:\画像\zsh.png".to_owned()),
                    ..Profile::new("zsh", "zsh -l", BackendKind::Cygwin)
                },
                Profile {
                    distribution: Some("Ubuntu".to_owned()),
                    ..Profile::new("Ubuntu", "", BackendKind::Wsl)
                },
            ]
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
        // 点で区切ったキーとインラインテーブルでも書ける。
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
        // 誤った値の色と配色は前の値のまま、書いた色は読み、書いていない色は既定に戻る。
        let colors = &parsed.config.colors;
        assert_eq!(colors.scheme, previous.colors.scheme);
        assert_eq!(colors.terminal, [previous.colors.terminal[0], None]);
        assert_eq!(colors.tab_bar[0], previous.colors.tab_bar[0]);
        assert_eq!(colors.tab_bar[5], Some(Rgb { r: 0, g: 255, b: 0 }));
        assert_eq!(colors.scroll_bar, [None; 4]);
        // 書いていないテーブルは既定の色に戻る。
        assert_eq!(parse("[font]\n").config.colors, ColorsConfig::default());
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
        // テーブルでなければ全体が前のまま。
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
            "shell = ' '\ndefault_profile = 3\n[font]\nfamily = 3\nsize = 500\nwide_glyph = 'wide'\n[paste]\nconfirm = 1\nconfirm_size = 'big'\n[clipboard]\nread = 1\nwrite = 1\n[scroll_bar]\nwidth = 2\n[window]\npadding = 65\n[keys]\n\"ctrl+tab\" = 5\n\"ctrl+page_down\" = 'next'\n[colors]\nscheme = 'sepia'\nforeground = 'blue'\n[colors.tab_bar]\nbar = 'red'\n[background]\nimages = ''\nimage_opacity = 2\nfit = 0\n",
            &previous,
        );
        assert!(!parsed.syntax_error);
        assert_eq!(parsed.config, previous);
        // 一覧の項目と、一覧に載らない項目のどれにも、一つずつ警告がある。
        let mut warned: Vec<String> = parsed
            .warnings
            .iter()
            .map(|warning| {
                // 知らないキーの警告（一覧から抜けた項目）も、キーの名前を取り出して差に出す。
                let rest = warning.message.trim_start_matches("ignoring ");
                let rest = rest
                    .strip_prefix("unknown setting ")
                    .or_else(|| rest.strip_prefix("unknown table "))
                    .unwrap_or(rest);
                let end = rest.find([':', ' ']).unwrap_or(rest.len());
                rest[..end].to_owned()
            })
            .collect();
        let mut expected: Vec<String> = SCALARS
            .iter()
            .map(Scalar::name)
            .chain(
                [
                    "shell",
                    "default_profile",
                    "font.family",
                    "keys.\"ctrl+tab\"",
                    "keys.\"ctrl+page_down\"",
                    "colors.foreground",
                    "colors.tab_bar.bar",
                    "background.images",
                ]
                .map(str::to_owned),
            )
            .collect();
        warned.sort();
        expected.sort();
        assert_eq!(warned, expected, "{:?}", parsed.warnings);
        // 配列でない profiles も、前のプロファイルのまま。
        let parsed = super::parse("profiles = 'zsh'\ndefault_profile = 'zsh'\n", &previous);
        assert_eq!(parsed.config.profiles, previous.profiles);
        assert_eq!(parsed.config.default_profile.as_deref(), Some("zsh"));
        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);

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
        assert_eq!(parsed.config.profiles, [Profile::builtin()]);
    }

    #[test]
    fn paste_confirm_size_takes_integers_in_range() {
        assert_eq!(parse("").config.paste.confirm_size, 8);
        for size in [0, 64, 1_048_576] {
            let parsed = parse(&format!("[paste]\nconfirm_size = {size}\n"));
            assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
            assert_eq!(parsed.config.paste.confirm_size, size);
        }
        let parsed = parse("[paste]\nconfirm_size = 1048577\n");
        assert_eq!(parsed.config.paste.confirm_size, 8);
        assert_eq!(
            parsed.warnings,
            ["ignoring paste.confirm_size = 1048577: must be between 0 and 1048576"]
        );
        for text in [
            "confirm_size = 1.5",
            "confirm_size = -1",
            "confirm_size = 'x'",
        ] {
            let parsed = parse(&format!("[paste]\n{text}\n"));
            assert_eq!(parsed.config.paste.confirm_size, 8);
            assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
        }
        // 読めない値は、前の値のまま。
        let previous = customized();
        let parsed = super::parse("[paste]\nconfirm_size = 'x'\n", &previous);
        assert_eq!(parsed.config.paste.confirm_size, 64);

        let input = |text: &str| parse_input::<u32>("paste.confirm_size", text);
        assert_eq!(input(" 0 "), Some(0));
        assert_eq!(input("1048576"), Some(1_048_576));
        for text in ["", "1048577", "1.5", "-1", "big"] {
            assert_eq!(input(text), None, "{text:?}");
        }
    }

    #[test]
    fn padding_takes_integers_in_range() {
        let parsed = parse("[window]\npadding = 0\n");
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.config.window.padding, 0);
        let parsed = parse("[window]\npadding = 64\n");
        assert_eq!(parsed.config.window.padding, 64);
        let parsed = parse("[window]\npadding = 65\n");
        assert_eq!(parsed.config.window.padding, DEFAULT_PADDING);
        assert_eq!(
            parsed.warnings,
            ["ignoring window.padding = 65: must be between 0 and 64"]
        );
        for text in ["padding = 4.5", "padding = -1", "padding = 'wide'"] {
            let parsed = parse(&format!("[window]\n{text}\n"));
            assert_eq!(parsed.config.window.padding, DEFAULT_PADDING);
            assert_eq!(parsed.warnings.len(), 1, "{text}: {:?}", parsed.warnings);
        }
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
        assert_eq!(
            parsed.config.profiles,
            [Profile::new(
                "PowerShell",
                "powershell.exe -NoLogo",
                BackendKind::Conpty
            )]
        );
        assert_eq!(parsed.config.default_profile_index(), 0);
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
            confirm_size = 0
            [scroll_bar]
            width = 8
            [window]
            padding = 0
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
                profiles: vec![Profile::new(
                    "zsh",
                    r"C:\cygwin64\bin\zsh.exe -l",
                    BackendKind::Conpty,
                )],
                paste: PasteConfig {
                    confirm: PasteConfirm::Never,
                    confirm_size: 0,
                },
                scroll_bar: ScrollBarConfig { width: 8 },
                window: WindowConfig { padding: 0 },
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
        assert_eq!(parsed.config.profiles, [Profile::builtin()]);
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
            ("overflow_center", WideGlyph::OverflowCenter),
        ] {
            assert_eq!(wide_glyph.name(), name);
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
            ["shrink", "clip", "overflow", "overflow_center"]
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
    fn clipboard_read_takes_each_name() {
        assert_eq!(parse("").config.clipboard.read, ClipboardRead::Ask);
        for read in ClipboardRead::ALL {
            let parsed = parse(&format!("[clipboard]\nread = '{}'\n", read.name()));
            assert_eq!(parsed.warnings, Vec::<String>::new());
            assert_eq!(parsed.config.clipboard.read, read);
        }
    }

    #[test]
    fn an_invalid_clipboard_read_warns_and_keeps_the_previous_value() {
        let previous = customized();
        let parsed = super::parse("[clipboard]\nread = 'always'\ncolor = 1\n", &previous);
        assert_eq!(parsed.config.clipboard.read, ClipboardRead::Deny);
        let mut warnings: Vec<String> = parsed.warnings.into_iter().map(|w| w.message).collect();
        warnings.sort();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0].starts_with("ignoring clipboard.read:"),
            "{warnings:?}"
        );
        assert_eq!(warnings[1], "ignoring unknown setting clipboard.color");
        // 消したら既定に戻す。
        assert_eq!(
            super::parse("", &previous).config.clipboard.read,
            ClipboardRead::Ask
        );
    }

    #[test]
    fn clipboard_write_takes_each_name() {
        assert_eq!(parse("").config.clipboard.write, ClipboardWrite::Allow);
        for write in ClipboardWrite::ALL {
            let parsed = parse(&format!("[clipboard]\nwrite = '{}'\n", write.name()));
            assert_eq!(parsed.warnings, Vec::<String>::new());
            assert_eq!(parsed.config.clipboard.write, write);
        }
    }

    #[test]
    fn an_invalid_clipboard_write_warns_and_keeps_the_previous_value() {
        let previous = customized();
        let parsed = super::parse("[clipboard]\nwrite = 'never'\n", &previous);
        assert_eq!(parsed.config.clipboard.write, ClipboardWrite::Deny);
        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
        assert!(
            parsed.warnings[0]
                .message
                .starts_with("ignoring clipboard.write:"),
            "{:?}",
            parsed.warnings
        );
        // 消したら既定に戻す。
        assert_eq!(
            super::parse("", &previous).config.clipboard.write,
            ClipboardWrite::Allow
        );
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

    /// 入力欄の文字列 `text` を、数の項目 `scalar` の値として読めるか。
    fn input_is_read(scalar: &Scalar, text: &str) -> bool {
        let name = scalar.name();
        match scalar.kind {
            ScalarKind::Float { .. } => parse_input::<f32>(&name, text).is_some(),
            ScalarKind::Int { .. } => parse_input::<u32>(&name, text).is_some(),
            ScalarKind::Name { .. } => panic!("{name} is not a number"),
        }
    }

    #[test]
    fn inputs_are_checked_like_the_file() {
        // 数の項目は、範囲の中を読み、範囲の外と数でないものを読まない。ファイルに書いたときも同じ。
        for scalar in SCALARS {
            let (accepted, rejected) = match &scalar.kind {
                ScalarKind::Float { range, .. } => {
                    let (start, end) = (*range.start(), *range.end());
                    (
                        [start, end, (start + end) / 2.0].map(|value| value.to_string()),
                        vec![
                            (start - 0.01).to_string(),
                            (end + 0.01).to_string(),
                            "nan".to_owned(),
                        ],
                    )
                }
                ScalarKind::Int { range, .. } => {
                    let mut rejected = vec![
                        (range.end() + 1).to_string(),
                        "-1".to_owned(),
                        "1.5".to_owned(),
                    ];
                    rejected.extend(range.start().checked_sub(1).map(|below| below.to_string()));
                    (
                        [
                            *range.start(),
                            *range.end(),
                            (range.start() + range.end()) / 2,
                        ]
                        .map(|value| value.to_string()),
                        rejected,
                    )
                }
                ScalarKind::Name { .. } => continue,
            };
            let file_is_read = |text: &str| {
                let parsed = parse(&format!("[{}]\n{} = {text}\n", scalar.table, scalar.key));
                parsed.warnings.is_empty()
            };
            let name = scalar.name();
            for text in &accepted {
                assert!(input_is_read(scalar, &format!(" {text} ")), "{name} {text}");
                assert!(file_is_read(text), "{name} {text}");
            }
            for text in &rejected {
                assert!(!input_is_read(scalar, text), "{name} {text}");
                assert!(!file_is_read(text), "{name} {text}");
            }
            for text in ["", "12pt", "wide"] {
                assert!(!input_is_read(scalar, text), "{name} {text:?}");
            }
        }
        assert_eq!(parse_input::<f32>("font.size", " 10.5 "), Some(10.5));
        assert_eq!(
            parse_input::<f32>("background.image_opacity", "0.75"),
            Some(0.75)
        );
        // 一覧にない名前と、種類の違う項目は読まない。
        assert_eq!(parse_input::<f32>("font.weight", "1"), None);
        assert_eq!(parse_input::<u32>("font.size", "12"), None);
        assert_eq!(parse_input::<f32>("font.wide_glyph", "1"), None);
    }

    #[test]
    fn profiles_are_read_in_order() {
        let parsed = parse(
            r#"
default_profile = "zsh"

# いつもの
[[profiles]]
name = "PowerShell"
command = "powershell.exe -NoLogo"

[[profiles]]
name = "zsh"
command = 'C:\cygwin64\bin\zsh.exe -l'
backend = "conpty"

[[profiles]]
name = "WSL"
command = "wsl.exe"

[font]
size = 10
"#,
        );
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        let names: Vec<&str> = parsed
            .config
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect();
        assert_eq!(names, ["PowerShell", "zsh", "WSL"]);
        assert_eq!(
            parsed.config.profiles[1].command,
            r"C:\cygwin64\bin\zsh.exe -l"
        );
        assert_eq!(parsed.config.default_profile.as_deref(), Some("zsh"));
        assert_eq!(parsed.config.default_profile_index(), 1);
        assert_eq!(parsed.config.font.size_pt, 10.0);
        // インラインの配列でも書ける。
        let parsed = parse("profiles = [{ name = 'a', command = 'cmd.exe' }]\n");
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.config.profiles[0].name, "a");
        assert_eq!(parsed.config.default_profile_index(), 0);
    }

    #[test]
    fn unusable_profiles_are_skipped_with_their_lines() {
        let parsed = super::parse(
            "default_profile = 'gone'\n[[profiles]]\nname = 'a'\ncommand = 'cmd.exe'\n[[profiles]]\nname = 'b'\ncommand = 'wsl.exe'\nbackend = 'bridge'\n[[profiles]]\nname = ''\ncommand = 'x'\n[[profiles]]\ncommand = 'y'\n[[profiles]]\nname = 'a'\ncommand = 'z'\ncolor = 'red'\n[[profiles]]\nname = 'c'\ncommand = 'pwsh.exe'\ncolor = 'red'\n",
            &Config::default(),
        );
        let names: Vec<&str> = parsed
            .config
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect();
        assert_eq!(names, ["a", "c"]);
        assert_eq!(parsed.config.default_profile, None);
        let warnings: Vec<(Option<usize>, &str)> = parsed
            .warnings
            .iter()
            .map(|w| (w.line, w.message.as_str()))
            .collect();
        assert_eq!(
            warnings,
            [
                (
                    Some(1),
                    "ignoring default_profile = \"gone\": no profile has that name"
                ),
                (
                    Some(8),
                    "ignoring profiles[1]: unknown backend \"bridge\", expected one of [\"conpty\", \"cygwin\", \"wsl\"]"
                ),
                (Some(10), "ignoring profiles[2]: name must not be empty"),
                (Some(12), "ignoring profiles[3]: expected a name"),
                (
                    Some(15),
                    "ignoring profiles[4]: the name \"a\" is already used above"
                ),
                (Some(17), "ignoring unknown setting profiles[4].color"),
                (Some(21), "ignoring unknown setting profiles[5].color"),
            ]
        );
    }

    #[test]
    fn bridge_profiles_take_their_own_keys() {
        let parsed = super::parse(
            "[[profiles]]\nname = 'cyg'\nbackend = 'cygwin'\ncygwin_root = 'C:\\app\\cygwin'\n[[profiles]]\nname = 'u'\nbackend = 'wsl'\ncommand = 'zsh -l'\ndistribution = 'Ubuntu'\n[[profiles]]\nname = 'w'\nbackend = 'wsl'\ncommand = ''\n[[profiles]]\nname = 'x'\ncommand = 'cmd.exe'\ndistribution = 'Ubuntu'\ncygwin_root = ''\n[[profiles]]\nname = 'y'\nbackend = 'wsl'\ncygwin_root = 'C:\\cygwin'\n[[profiles]]\nname = 'z'\n",
            &Config::default(),
        );
        assert_eq!(
            parsed.config.profiles,
            [
                Profile {
                    cygwin_root: Some(r"C:\app\cygwin".to_owned()),
                    ..Profile::new("cyg", "", BackendKind::Cygwin)
                },
                Profile {
                    distribution: Some("Ubuntu".to_owned()),
                    ..Profile::new("u", "zsh -l", BackendKind::Wsl)
                },
                Profile::new("w", "", BackendKind::Wsl),
                Profile::new("x", "cmd.exe", BackendKind::Conpty),
                Profile::new("y", "", BackendKind::Wsl),
            ]
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
                    Some(17),
                    "ignoring profiles[3].distribution: used only with backend = \"wsl\""
                ),
                (Some(18), "ignoring profiles[3].cygwin_root: empty"),
                (
                    Some(22),
                    "ignoring profiles[4].cygwin_root: used only with backend = \"cygwin\""
                ),
                (Some(23), "ignoring profiles[5]: expected a command"),
            ]
        );
    }

    #[test]
    fn profiles_take_an_icon_with_any_backend() {
        let parsed = parse(
            "[[profiles]]\nname = 'a'\ncommand = 'cmd.exe'\nicon = 'C:\\pics\\a.png'\n[[profiles]]\nname = 'b'\nbackend = 'wsl'\nicon = 'b.ico'\n[[profiles]]\nname = 'c'\ncommand = 'x'\nicon = ' '\n[[profiles]]\nname = 'd'\ncommand = 'x'\nicon = 3\n",
        );
        let icons: Vec<Option<&str>> = parsed
            .config
            .profiles
            .iter()
            .map(|profile| profile.icon.as_deref())
            .collect();
        assert_eq!(icons, [Some(r"C:\pics\a.png"), Some("b.ico"), None, None]);
        assert!(
            parsed
                .warnings
                .iter()
                .any(|warning| warning == "ignoring profiles[2].icon: empty"),
        );
    }

    #[test]
    fn shell_makes_a_single_profile_unless_profiles_are_written() {
        let parsed = parse(r#"shell = '"C:\Program Files\PowerShell\7\pwsh.EXE" -NoLogo'"#);
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(
            parsed.config.profiles,
            [Profile::new(
                "pwsh",
                r#""C:\Program Files\PowerShell\7\pwsh.EXE" -NoLogo"#,
                BackendKind::Conpty,
            )]
        );
        assert_eq!(Profile::from_shell("zsh -l").name, "zsh");
        assert_eq!(Profile::from_shell(r"C:\cygwin64\bin\zsh.exe").name, "zsh");
        assert_eq!(Profile::from_shell("\"\" x").name, "shell");
        // shell の名前でも既定にできる。
        let parsed = parse("shell = 'zsh -l'\ndefault_profile = 'zsh'\n");
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.config.default_profile.as_deref(), Some("zsh"));

        // 両方あれば profiles を使い、shell は警告して使わない。
        let parsed = super::parse(
            "shell = 'zsh -l'\n[[profiles]]\nname = 'cmd'\ncommand = 'cmd.exe'\n",
            &Config::default(),
        );
        assert_eq!(parsed.config.profiles.len(), 1);
        assert_eq!(parsed.config.profiles[0].name, "cmd");
        assert_eq!(parsed.warnings.len(), 1);
        assert_eq!(parsed.warnings[0].line, Some(1));
        assert!(parsed.warnings[0].message.starts_with("ignoring shell:"));

        // 使えるプロファイルが一つもなければ、shell、それもなければ既定のプロファイル。
        let parsed = parse("shell = 'zsh -l'\nprofiles = []\n");
        assert_eq!(parsed.config.profiles, [Profile::from_shell("zsh -l")]);
        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
        let parsed = parse("[[profiles]]\nname = 'a'\n");
        assert_eq!(parsed.config.profiles, [Profile::builtin()]);
        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
    }

    #[test]
    fn shell_of_wsl_uses_the_wsl_bridge() {
        let profile = |command: &str| Profile::from_shell_with(command, |_| false);
        let bridge = |distribution: Option<&str>, command: &str| Profile {
            distribution: distribution.map(str::to_owned),
            ..Profile::new("wsl", command, BackendKind::Wsl)
        };
        assert_eq!(profile("wsl.exe"), bridge(None, ""));
        assert_eq!(profile("wsl.exe -d Ubuntu"), bridge(Some("Ubuntu"), ""));
        assert_eq!(
            profile(r"C:\Windows\System32\WSL.EXE --distribution Ubuntu"),
            Profile {
                name: "WSL".to_owned(),
                ..bridge(Some("Ubuntu"), "")
            }
        );
        assert_eq!(
            profile(r#"wsl -d "Ubuntu 24.04" --exec zsh -l"#),
            bridge(Some("Ubuntu 24.04"), "zsh -l")
        );
        assert_eq!(
            profile("wsl.exe -d Ubuntu -- ls -l 'a b' | less"),
            bridge(Some("Ubuntu"), "ls -l 'a b' | less")
        );
        assert_eq!(profile("wsl.exe -e  htop "), bridge(None, "htop"));
        assert_eq!(profile("wsl.exe top -d 1"), bridge(None, "top -d 1"));
        // 読めない引数があれば ConPTY のまま。
        for command in [
            "wsl.exe -u root",
            "wsl.exe -d Ubuntu --cd ~",
            "wsl.exe -d",
            "wsl.exe --exec",
            "wslconfig.exe",
        ] {
            assert_eq!(
                profile(command),
                Profile::new(&program_name(command), command, BackendKind::Conpty),
                "{command}"
            );
        }
        // shell からプロファイルを作るときだけで、profiles に書いたものはそのまま。
        let parsed = parse("[[profiles]]\nname = 'u'\ncommand = 'wsl.exe -d Ubuntu'\n");
        assert_eq!(
            parsed.config.profiles,
            [Profile::new("u", "wsl.exe -d Ubuntu", BackendKind::Conpty)]
        );
        let parsed = parse("shell = 'wsl.exe -d Ubuntu'\n");
        assert_eq!(parsed.config.profiles, [bridge(Some("Ubuntu"), "")]);
    }

    #[test]
    fn syntax_errors_give_the_defaults() {
        let parsed = parse("[font\nsize = 10\n");
        assert_eq!(parsed.config, Config::default());
        assert_eq!(parsed.warnings.len(), 1);
        assert!(parsed.warnings[0].starts_with("cannot parse the settings"));
    }
}
