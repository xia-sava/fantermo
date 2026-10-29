//! 設定ファイルの古い書き方を、今の書き方に読み替える。
//!
//! 設定を解釈する前に、[`STEPS`] の手順を上から順に当てる。手順はそれぞれ、古い書き方を見つけたら
//! `toml_edit` の文書をその場で今の書き方に書き換え、何をどう読み替えたかを返す。変えたところの
//! ほかは、コメントも空行もそのまま残す。今の書き方で書いたファイルには何もしないので、何度当てても
//! 結果は変わらない。書き方を改めたら、[`STEPS`] の最後に手順を一つ足す。

use toml_edit::{DocumentMut, Item, Key, Table, Value};

use crate::config_edit::{
    self, BACKGROUND_PATH, Document, IMAGE_OPACITY, IMAGE_PATH, IMAGES, bare_repr, decor_prefix,
    number_of, number_value, value_prefixes,
};

/// 読み替えの手順一つ。
pub struct Step {
    /// 手順の名前。
    pub name: &'static str,
    /// 何をどう読み替えるか。
    pub description: &'static str,
    /// 古い書き方があれば今の書き方に書き換え、何をしたかを返す。なければ何もせずに `None`。
    apply: fn(&mut DocumentMut) -> Option<Change>,
}

/// 手順を当てて変えたこと。
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    /// 何をどう読み替えたか（ログに残す一行）。
    pub message: String,
    /// 読み替えられずに捨てたもの（ログに警告として残す）。
    pub dropped: Vec<String>,
}

impl Change {
    fn new(message: String) -> Self {
        Self {
            message,
            dropped: Vec::new(),
        }
    }
}

/// 当てる手順。上から順に当てる。
pub const STEPS: &[Step] = &[
    Step {
        name: "background.opacity",
        description: "background.opacity（画像の上に重ねる背景色の不透明度）を \
                      background.image_opacity = 1 - opacity（画像の不透明度）にする。\
                      image_opacity も書いてあれば、opacity を消す",
        apply: background_opacity,
    },
    Step {
        name: "background.images",
        description: "background.images = [ … ]（文字列とインラインの表の配列、文字列一つ、\
                      インラインの表一つ）を [[background.images]] の表の配列にする",
        apply: background_images,
    },
];

/// 読み替えた結果。
#[derive(Debug, PartialEq)]
pub struct Migrated {
    /// 読み替えた中身。読み替えるものがなければ、もとの中身のまま。
    pub text: String,
    /// 当てた手順と、それで変えたこと。手順の順。
    pub changes: Vec<(&'static Step, Change)>,
}

impl Migrated {
    /// 読み替えたものがある。
    pub fn changed(&self) -> bool {
        !self.changes.is_empty()
    }
}

impl PartialEq for Step {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name)
    }
}

/// 設定ファイルの中身 `text` に [`STEPS`] を順に当てる。TOML として読めなければ何もしない（読むときに
/// エラーとして知らせる）。先頭の BOM と、改行が CRLF かどうかは `text` に合わせる。
pub fn migrate(text: &str) -> Migrated {
    let unchanged = || Migrated {
        text: text.to_owned(),
        changes: Vec::new(),
    };
    let Ok(mut document) = Document::parse(text) else {
        return unchanged();
    };
    let changes: Vec<(&'static Step, Change)> = STEPS
        .iter()
        .filter_map(|step| (step.apply)(&mut document.doc).map(|change| (step, change)))
        .collect();
    if changes.is_empty() {
        return unchanged();
    }
    Migrated {
        text: document.finish(),
        changes,
    }
}

/// `[background]` の前の名前のキー。画像の上に重ねる背景色の不透明度で、`image_opacity` と向きが逆。
const OPACITY: &str = "opacity";

/// `background.opacity` を `background.image_opacity = 1 - opacity` にする。キーの名前と値だけを替え、
/// 行の位置とコメントは残す。`image_opacity` も書いてあれば、そちらを使うので `opacity` の行を消す
/// （上に書いたコメントは残す）。値が数でなければ何もしない（知らないキーとして警告する）。
fn background_opacity(doc: &mut DocumentMut) -> Option<Change> {
    let background = config_edit::table_at(doc, BACKGROUND_PATH)?;
    let value = background.get(OPACITY)?.as_value()?;
    let opacity = number_of(value)?;
    let written = bare_repr(value);
    if background.contains_key(IMAGE_OPACITY) {
        config_edit::replace_line(doc, BACKGROUND_PATH, OPACITY, None);
        return Some(Change::new(format!(
            "removed background.{OPACITY} = {written}; background.{IMAGE_OPACITY} is also written and is used"
        )));
    }
    let background = config_edit::table_at_mut(doc, BACKGROUND_PATH)?;
    let index = value_prefixes(background)
        .iter()
        .position(|(name, _)| name == OPACITY)?;
    let (key, item) = background.remove_entry(OPACITY)?;
    // f64 のまま引くと 1 - 0.8 が 0.19999999999999996 になるので、f32 にしてから短い表記で書く。
    let mut image_opacity = number_value((1.0 - opacity) as f32);
    if let Some(old) = item.as_value() {
        *image_opacity.decor_mut() = old.decor().clone();
    }
    let new = bare_repr(&image_opacity);
    let key = Key::new(IMAGE_OPACITY).with_leaf_decor(key.leaf_decor().clone());
    config_edit::insert_at(background, index, key, image_opacity);
    Some(Change::new(format!(
        "read background.{OPACITY} = {written} as background.{IMAGE_OPACITY} = {new} (1 - {OPACITY})"
    )))
}

/// `background.images = [ … ]` を `[[background.images]]` の表の配列にする。配列の要素の文字列は
/// `path` だけの表に、インラインの表はそのキー（知らないキーも）を持つ表にする。文字列一つ、
/// インラインの表一つも同じ。表は `[background]` の値の後ろに置く。要素の前後のコメントは、その
/// 要素の表の前に移す（要素の行末のコメントも）。`images` のキーの前のコメントと `[` の行のコメントは
/// 最初の表の前に、最後の要素の後ろと `]` の行のコメントは最後の表の後ろに移す。表にできない要素（数など）は捨て、
/// 捨てたものを返す。`[background]` を点で区切ったキーで書いたときと、`images` が配列・文字列・
/// インラインの表のどれでもないときは何もしない（読むときに警告する）。
fn background_images(doc: &mut DocumentMut) -> Option<Change> {
    let background = config_edit::table_at(doc, BACKGROUND_PATH)?;
    if !config_edit::is_standard(background) {
        return None;
    }
    let value = background.get(IMAGES)?.as_value()?.clone();
    let key_prefix = decor_prefix(background.key(IMAGES)?.leaf_decor());
    let was_first = value_prefixes(background)
        .first()
        .is_some_and(|(name, _)| name == IMAGES);
    // `[` の行のコメント。
    let mut general = String::new();
    let mut leftover = String::new();
    // 要素と、その表の前に置くコメント。
    let mut elements: Vec<(Value, String)> = Vec::new();
    match &value {
        Value::Array(array) => {
            let items: Vec<&Value> = array.iter().collect();
            let trailing = array.trailing().as_str().unwrap_or_default();
            for (index, item) in items.iter().enumerate() {
                let (head, tail) = split_first_line(&decor_prefix(item.decor()));
                if index == 0 {
                    general.push_str(&comment_lines(&head));
                }
                // 次の要素の前（最後なら `]` の前）の、この要素と同じ行のコメント。
                let next_prefix = match items.get(index + 1) {
                    Some(next) => decor_prefix(next.decor()),
                    None => trailing.to_owned(),
                };
                let (line_end, _) = split_first_line(&next_prefix);
                let comments = format!(
                    "{}{}{}",
                    comment_lines(&tail),
                    comment_lines(&decor_suffix(item)),
                    comment_lines(&line_end)
                );
                elements.push(((*item).clone(), comments));
            }
            let (head, tail) = split_first_line(trailing);
            if items.is_empty() {
                general.push_str(&comment_lines(&head));
            }
            leftover = format!(
                "{}{}",
                comment_lines(&tail),
                comment_lines(&decor_suffix(&value))
            );
        }
        Value::String(_) | Value::InlineTable(_) => {
            elements.push((value.clone(), comment_lines(&decor_suffix(&value))));
        }
        _ => return None,
    }

    let mut tables: Vec<(String, Table)> = Vec::new();
    let mut dropped = Vec::new();
    // 捨てた要素のコメントは、次の表の前（なければ最後の表の後ろ）に移す。
    let mut pending = String::new();
    for (index, (item, comments)) in elements.into_iter().enumerate() {
        match image_table(&item) {
            Some(table) => {
                tables.push((format!("{pending}{comments}"), table));
                pending.clear();
            }
            None => {
                dropped.push(format!(
                    "dropped background.{IMAGES}[{index}] = {}: expected a path or a table",
                    bare_repr(&item)
                ));
                pending.push_str(&comments);
            }
        }
    }
    leftover.insert_str(0, &pending);

    let count = tables.len();
    if tables.is_empty() {
        // 表にするものがなければ、行を消してコメントだけを残す（キーの前のコメントは行を消すときに残る）。
        config_edit::replace_line(
            doc,
            BACKGROUND_PATH,
            IMAGES,
            Some(format!("{general}{leftover}")),
        );
    } else {
        let background = config_edit::table_at_mut(doc, BACKGROUND_PATH)?;
        background.remove(IMAGES);
        if was_first {
            // 先頭の値の行を消したので、次の値の行の前の空行（見出しとの間）を消す。
            if let Some((next, prefix)) = value_prefixes(background).first()
                && let Some(mut next_key) = background.key_mut(next)
            {
                let trimmed = prefix.trim_start_matches(['\r', '\n']).to_owned();
                next_key.leaf_decor_mut().set_prefix(trimmed);
            }
        }
        if let Some((comments, _)) = tables.first_mut() {
            comments.insert_str(0, &format!("{}{general}", comment_lines(&key_prefix)));
        }
        config_edit::insert_image_tables(doc, tables, leftover).ok()?;
    }
    let message = if count == 0 {
        format!(
            "removed background.{IMAGES}: no images to rewrite as [[background.{IMAGES}]] tables"
        )
    } else {
        format!("rewrote background.{IMAGES} as {count} [[background.{IMAGES}]] table(s)")
    };
    Some(Change { message, dropped })
}

/// 配列の要素一つ `item` の表。文字列なら `path` だけの表、インラインの表ならそのキーの表。どちらでも
/// なければ `None`。
fn image_table(item: &Value) -> Option<Table> {
    match item {
        Value::String(_) => {
            let mut path = item.clone();
            path.decor_mut().clear();
            let mut table = Table::new();
            table.insert(IMAGE_PATH, Item::Value(path));
            Some(table)
        }
        Value::InlineTable(inline) => {
            let mut inline = inline.clone();
            inline.decor_mut().clear();
            Some(inline.into_table())
        }
        _ => None,
    }
}

/// 値の後ろに書いたもの（行末のコメントなど）。
fn decor_suffix(value: &Value) -> String {
    value
        .decor()
        .suffix()
        .and_then(|suffix| suffix.as_str())
        .unwrap_or_default()
        .to_owned()
}

/// `raw` のうちコメントの行を、字下げを外して一行ずつ並べたもの。
fn comment_lines(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('#'))
        .map(|line| format!("{line}\n"))
        .collect()
}

/// 配列の要素の前に書いたもの `prefix` を、前の要素（先頭なら `[`）と同じ行のところ（最初の改行まで）と、
/// 残りに分ける。
fn split_first_line(prefix: &str) -> (String, String) {
    match prefix.split_once('\n') {
        Some((head, tail)) => (head.to_owned(), tail.to_owned()),
        None => (prefix.to_owned(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, BackgroundConfig, BackgroundFit, BackgroundImage, Config};

    /// 読み替えた中身と、変えたことの文。
    fn run(text: &str) -> (String, Vec<String>) {
        let migrated = migrate(text);
        let messages = migrated
            .changes
            .iter()
            .flat_map(|(_, change)| {
                std::iter::once(change.message.clone()).chain(change.dropped.iter().cloned())
            })
            .collect();
        (migrated.text, messages)
    }

    /// 読み替えた中身をもう一度読み替えても変わらず、警告なしで読める。
    fn assert_settled(text: &str) -> Config {
        let again = migrate(text);
        assert!(!again.changed(), "{:?}\n{text}", again.changes);
        assert_eq!(again.text, text);
        let parsed = config::parse(text, &Config::default());
        assert!(parsed.warnings.is_empty(), "{:?}\n{text}", parsed.warnings);
        parsed.config
    }

    #[test]
    fn opacity_becomes_image_opacity_in_place() {
        for (opacity, image_opacity) in [
            ("0.8", "0.2"),
            ("0.5", "0.5"),
            ("0.3", "0.7"),
            ("0.25", "0.75"),
            ("0.0", "1"),
            ("1.0", "0"),
            ("0", "1"),
            ("1", "0"),
        ] {
            let text = format!(
                "[background]\n# dim the image\nopacity = {opacity} # mine\nfit = 'tile'\n"
            );
            let (out, messages) = run(&text);
            assert_eq!(
                out,
                format!(
                    "[background]\n# dim the image\nimage_opacity = {image_opacity} # mine\nfit = 'tile'\n"
                )
            );
            assert_eq!(
                messages,
                [format!(
                    "read background.opacity = {opacity} as background.image_opacity = {image_opacity} (1 - opacity)"
                )]
            );
            assert_settled(&out);
        }
    }

    #[test]
    fn opacity_is_removed_when_image_opacity_is_written() {
        let (out, messages) =
            run("[background]\nimage_opacity = 0.4\n# old\nopacity = 0.9\nfit = 'tile'\n");
        assert_eq!(
            out,
            "[background]\nimage_opacity = 0.4\n# old\nfit = 'tile'\n"
        );
        assert_eq!(
            messages,
            [
                "removed background.opacity = 0.9; background.image_opacity is also written and is used"
            ]
        );
        assert_eq!(assert_settled(&out).background.image_opacity, 0.4);
    }

    #[test]
    fn opacity_that_is_not_a_number_is_left_as_an_unknown_key() {
        let text = "[background]\nopacity = 'half'\n";
        assert!(!migrate(text).changed());
        assert_eq!(migrate(text).text, text);
        let parsed = config::parse(text, &Config::default());
        let warnings: Vec<&str> = parsed.warnings.iter().map(|w| w.message.as_str()).collect();
        assert_eq!(warnings, ["ignoring unknown setting background.opacity"]);
    }

    #[test]
    fn image_arrays_become_tables_after_the_background_values() {
        let text = "[background]\nimages = ['C:\\x\\tile', 'C:\\x\\scale']\n\nopacity = 0.5\nfit = \"tile\"\nimage_opacity = 0.5\n";
        let (out, messages) = run(text);
        assert_eq!(
            out,
            "[background]\nfit = \"tile\"\nimage_opacity = 0.5\n\n[[background.images]]\npath = 'C:\\x\\tile'\n\n[[background.images]]\npath = 'C:\\x\\scale'\n"
        );
        assert_eq!(
            messages,
            [
                "removed background.opacity = 0.5; background.image_opacity is also written and is used",
                "rewrote background.images as 2 [[background.images]] table(s)",
            ]
        );
        assert_eq!(
            assert_settled(&out).background,
            BackgroundConfig {
                images: vec![
                    BackgroundImage::new(r"C:\x\tile"),
                    BackgroundImage::new(r"C:\x\scale"),
                ],
                image_opacity: 0.5,
                fit: BackgroundFit::Tile,
            }
        );

        // 続く表があれば、その前に置く。
        let (out, _) = run(&format!("{text}\n[keys]\n\"ctrl+tab\" = \"none\"\n"));
        assert_eq!(
            out,
            "[background]\nfit = \"tile\"\nimage_opacity = 0.5\n\n[[background.images]]\npath = 'C:\\x\\tile'\n\n[[background.images]]\npath = 'C:\\x\\scale'\n\n[keys]\n\"ctrl+tab\" = \"none\"\n"
        );
        assert_settled(&out);
    }

    #[test]
    fn comments_of_the_items_go_before_their_tables() {
        let text = "[background]\n# mine\nimages = [ # list\n  # walls\n  'C:\\walls',\n  'C:\\a.png', # the cat\n  3, # a number\n  { path = 'C:\\b.png', fit = 'tile', note = 'x' },\n  # 'C:\\c.png',\n] # end\nopacity = 0.8 # dim\n\n[keys]\n";
        let (out, messages) = run(text);
        // 捨てた要素のコメントは次の表の前に、最後の要素の後ろのコメントは最後の表の後ろに移す。
        // 知らないキーもそのまま残す。
        assert_eq!(
            out,
            "[background]\nimage_opacity = 0.2 # dim\n\n# mine\n# list\n# walls\n[[background.images]]\npath = 'C:\\walls'\n\n# the cat\n[[background.images]]\npath = 'C:\\a.png'\n\n# a number\n[[background.images]]\npath = 'C:\\b.png'\nfit = 'tile'\nnote = 'x'\n# 'C:\\c.png',\n# end\n\n[keys]\n"
        );
        assert_eq!(
            messages,
            [
                "read background.opacity = 0.8 as background.image_opacity = 0.2 (1 - opacity)",
                "rewrote background.images as 3 [[background.images]] table(s)",
                "dropped background.images[2] = 3: expected a path or a table",
            ]
        );
        assert!(!migrate(&out).changed());
        let parsed = config::parse(&out, &Config::default());
        let warnings: Vec<(Option<usize>, &str)> = parsed
            .warnings
            .iter()
            .map(|w| (w.line, w.message.as_str()))
            .collect();
        assert_eq!(
            warnings,
            [(
                Some(18),
                "ignoring unknown setting background.images[2].note"
            )]
        );
        assert_eq!(
            parsed.config.background.images,
            [
                BackgroundImage::new(r"C:\walls"),
                BackgroundImage::new(r"C:\a.png"),
                BackgroundImage {
                    path: r"C:\b.png".to_owned(),
                    fit: Some(BackgroundFit::Tile),
                    image_opacity: None,
                },
            ]
        );
    }

    #[test]
    fn the_explanation_of_the_older_template_stays_above_the_settings() {
        let text = "[background]\n# 背景画像の説明\n# 例: images = ['C:\\画像\\壁紙']\nimages = ['C:\\walls', { path = 'C:\\tiles', fit = 'tile', image_opacity = 1 }]\n\n# 不透明度の説明\n# image_opacity = 0.2\n\n# 表示方法の説明\nfit = \"center\"\n\n[keys]\n# キーの説明\n";
        let (out, _) = run(text);
        assert_eq!(
            out,
            "[background]\n# 不透明度の説明\n# image_opacity = 0.2\n\n# 表示方法の説明\nfit = \"center\"\n\n# 背景画像の説明\n# 例: images = ['C:\\画像\\壁紙']\n[[background.images]]\npath = 'C:\\walls'\n\n[[background.images]]\npath = 'C:\\tiles'\nfit = 'tile'\nimage_opacity = 1\n\n[keys]\n# キーの説明\n"
        );
        let config = assert_settled(&out);
        assert_eq!(
            config.background.images[1],
            BackgroundImage {
                path: r"C:\tiles".to_owned(),
                fit: Some(BackgroundFit::Tile),
                image_opacity: Some(1.0),
            }
        );
    }

    #[test]
    fn single_items_and_empty_arrays_are_rewritten() {
        let (out, _) = run("[background]\nimages = 'C:\\one.png'\n");
        assert_eq!(
            out,
            "[background]\n[[background.images]]\npath = 'C:\\one.png'\n"
        );
        assert_settled(&out);

        let (out, _) = run("[background]\nimages = { path = 'x', fit = 'center' } # one\n");
        assert_eq!(
            out,
            "[background]\n# one\n[[background.images]]\npath = 'x'\nfit = 'center'\n"
        );
        assert_settled(&out);

        let (out, messages) =
            run("[background]\n# none yet\nimages = [ # later\n]\n# after\nfit = 'tile'\n");
        assert_eq!(
            out,
            "[background]\n# none yet\n# later\n# after\nfit = 'tile'\n"
        );
        assert_eq!(
            messages,
            ["removed background.images: no images to rewrite as [[background.images]] tables"]
        );
        assert_settled(&out);

        // 表にできるものがなければ、行を消す。
        let (out, messages) = run("[background]\nimages = [3, true]\n");
        assert_eq!(out, "[background]\n");
        assert_eq!(messages.len(), 3, "{messages:?}");
    }

    #[test]
    fn current_files_are_left_as_they_are() {
        assert!(!migrate(config::TEMPLATE).changed());
        assert_eq!(migrate(config::TEMPLATE).text, config::TEMPLATE);
        let text = "[background]\nimage_opacity = 0.5\n\n[[background.images]]\npath = 'a'\n";
        assert!(!migrate(text).changed());
        // TOML として読めなければ何もしない。
        assert_eq!(
            migrate("[background\nopacity = 1\n").text,
            "[background\nopacity = 1\n"
        );
        // 値でない images（[background.images] の表）は読み替えず、読むときに警告する。
        let text = "[background.images]\npath = 'a'\n";
        assert!(!migrate(text).changed());
        assert_eq!(config::parse(text, &Config::default()).warnings.len(), 1);
    }

    #[test]
    fn line_endings_and_byte_order_mark_are_kept() {
        let (out, _) = run("\u{feff}[background]\r\nopacity = 0.8\r\nimages = ['a']\r\n");
        assert_eq!(
            out,
            "\u{feff}[background]\r\nimage_opacity = 0.2\r\n\r\n[[background.images]]\r\npath = 'a'\r\n"
        );
        assert!(!migrate(&out).changed());
    }
}
