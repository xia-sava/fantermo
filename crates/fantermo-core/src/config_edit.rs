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

mod document;
mod images;
mod profiles;

use toml_edit::{Array, DocumentMut, Value};

use crate::{
    config::{ColorTable, Config, SCALARS, Scalar, ScalarKind, format_color, migrate},
    keymap::{self, Action, KeyCombo},
};

pub(crate) use document::Document;
use document::{Reset, Target, number_value, reset, same, set};
use images::update_background_images;
use profiles::update_profiles;

/// 一覧の項目 `scalar` の、`config` での値。
fn scalar_value(scalar: &Scalar, config: &Config) -> Value {
    match &scalar.kind {
        ScalarKind::Float { get, .. } => number_value(get(config)),
        ScalarKind::Int { get, .. } => Value::from(i64::from(get(config))),
        ScalarKind::Name { get, .. } => Value::from(get(config)),
    }
}

/// `[font]` のテーブルのキーのパス。
const FONT_PATH: &[&str] = &["font"];

/// `[font]` の、フォントの名前のキー。
const FONT_FAMILY_KEY: &str = "family";

/// `[keys]` のテーブルのキーのパス。
const KEYS_PATH: &[&str] = &["keys"];

/// 最上位のキーの名前。
const DEFAULT_PROFILE_KEY: &str = "default_profile";

/// フォントの名前の並び。一つなら文字列、二つ以上なら配列。
fn families_value(families: &[String]) -> Value {
    match families {
        [family] => Value::from(family.as_str()),
        _ => Value::Array(families.iter().map(String::as_str).collect::<Array>()),
    }
}

/// 組み合わせに割り当てる値。動作の名前か、外すなら [`keymap::UNBOUND`]。
fn action_value(action: Option<Action>) -> Value {
    Value::from(action.map_or(keymap::UNBOUND, Action::name))
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
    let configs = Configs {
        base,
        edited,
        defaults: &Config::default(),
    };
    configs.update_value(doc, &[], DEFAULT_PROFILE_KEY, |config| {
        config.default_profile.as_deref().map(Value::from)
    })?;
    configs.update_value(doc, FONT_PATH, FONT_FAMILY_KEY, |config| {
        Some(families_value(&config.font.families))
    })?;
    for scalar in SCALARS {
        configs.update_value(doc, &[scalar.table], scalar.key, |config| {
            Some(scalar_value(scalar, config))
        })?;
    }
    update_background_images(doc, base, edited)?;
    update_profiles(doc, base, edited)?;
    update_colors(doc, base, edited)?;
    update_keys(doc, base, edited)?;
    Ok(document.finish())
}

/// 書き換える前と後の設定と、既定値。
struct Configs<'a> {
    /// 編集を始めたときの設定。
    base: &'a Config,
    edited: &'a Config,
    defaults: &'a Config,
}

impl Configs<'_> {
    /// テーブル `path` のキー `key` の項目を、`base` と `edited` で値が違えば書き換える。`value` は設定
    /// でのその項目の値で、書かないことを表す値（`default_profile` の `None`）なら `None`。既定値に
    /// 戻したら既定値をコメントにした行にし、既定値が `None` ならそれまでの値をコメントにする。
    fn update_value(
        &self,
        doc: &mut DocumentMut,
        path: &[&str],
        key: &str,
        value: impl Fn(&Config) -> Option<Value>,
    ) -> Result<(), String> {
        let new = value(self.edited);
        if same(&value(self.base), &new) {
            return Ok(());
        }
        let matches = |name: &str| name == key;
        let target = Target {
            name: key.to_owned(),
            matches: &matches,
        };
        let default = value(self.defaults);
        match new {
            Some(value) if !same(&Some(value.clone()), &default) => set(doc, path, &target, value),
            _ => {
                let reset = match default {
                    Some(default) => Reset::Comment(default),
                    None => Reset::CommentOld,
                };
                self::reset(doc, path, &target, reset)
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::document::{bare_repr, commented_key};
    use super::*;
    use crate::{
        config::{
            self, BackendKind, BackgroundConfig, BackgroundFit, BackgroundImage, ClipboardConfig,
            ClipboardRead, ColorScheme, ColorsConfig, FontConfig, PasteConfig, PasteConfirm,
            Profile, ScrollBarConfig, TEMPLATE, WideGlyph, WindowConfig,
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
                confirm_size: 64,
            },
            clipboard: ClipboardConfig {
                read: ClipboardRead::Deny,
            },
            scroll_bar: ScrollBarConfig { width: 20 },
            window: WindowConfig { padding: 0 },
            // 色は既定に戻すとひな形と違う行になるので、色の書き戻しは別に確認する。
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

    /// `config` の一覧の項目 `scalar` を、`defaults` の値に戻す。
    fn reset_scalar(scalar: &Scalar, config: &mut Config, defaults: &Config) {
        match &scalar.kind {
            ScalarKind::Float { get, set, .. } => set(config, get(defaults)),
            ScalarKind::Int { get, set, .. } => set(config, get(defaults)),
            ScalarKind::Name { get, set, .. } => assert!(set(config, get(defaults))),
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
            .replace("# confirm_size = 8", "confirm_size = 64")
            .replace(r#"# read = "ask""#, r#"read = "deny""#)
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

        // 一つずつ戻しても同じ。一覧の項目を上から一つずつ戻してから（`[background]` の項目は画像の
        // テーブルが残ったまま戻す）、一覧に載らない項目を戻す。
        let mut current = edited.clone();
        let mut text = text;
        let defaults = Config::default();
        /// 戻す手順。二つ目の引数は既定値。
        type Step = Box<dyn Fn(&mut Config, &Config)>;
        let mut steps: Vec<Step> = Vec::new();
        for scalar in SCALARS {
            steps.push(Box::new(|c, d| reset_scalar(scalar, c, d)));
        }
        let others: Vec<Step> = vec![
            Box::new(|c, d| c.background.images = d.background.images.clone()),
            Box::new(|c, d| {
                c.profiles = d.profiles.clone();
                c.default_profile = d.default_profile.clone();
            }),
            Box::new(|c, _| c.keys.bind(combo("ctrl+tab"), Some(Action::NextTab))),
            Box::new(|c, d| c.font.families = d.font.families.clone()),
            Box::new(|c, d| c.keys = d.keys.clone()),
        ];
        steps.extend(others);
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
