//! 設定ファイルの古い書き方を、今の書き方に読み替える枠組み。
//!
//! 設定を解釈する前に、[`STEPS`] の手順を上から順に当てる。手順はそれぞれ、古い書き方を見つけたら
//! `toml_edit` の文書をその場で今の書き方に書き換え、何をどう読み替えたかを返す。変えたところの
//! ほかは、コメントも空行もそのまま残す。今の書き方で書いたファイルには何もしないので、何度当てても
//! 結果は変わらない。書き方を改めたら、[`STEPS`] の最後に手順を一つ足す。

use toml_edit::DocumentMut;

use crate::config_edit::Document;

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

/// 当てる手順。上から順に当てる。
pub const STEPS: &[Step] = &[];

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
    migrate_with(text, STEPS)
}

/// 設定ファイルの中身 `text` に `steps` を順に当てる。
fn migrate_with(text: &str, steps: &'static [Step]) -> Migrated {
    let unchanged = || Migrated {
        text: text.to_owned(),
        changes: Vec::new(),
    };
    let Ok(mut document) = Document::parse(text) else {
        return unchanged();
    };
    let changes: Vec<(&'static Step, Change)> = steps
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

#[cfg(test)]
mod tests {
    use toml_edit::Key;

    use super::*;
    use crate::config::{self, Config};

    /// `[section]` のキー `from` を `to` に替える手順の本体。値、行の位置、コメントは残す。
    fn rename(doc: &mut DocumentMut, from: &str, to: &str) -> Option<Change> {
        let table = doc.get_mut("section")?.as_table_mut()?;
        let (key, item) = table.remove_entry(from)?;
        let key = Key::new(to).with_leaf_decor(key.leaf_decor().clone());
        table.insert_formatted(&key, item);
        Some(Change {
            message: format!("read section.{from} as section.{to}"),
            dropped: vec![format!("nothing was dropped from section.{from}")],
        })
    }

    fn old_to_middle(doc: &mut DocumentMut) -> Option<Change> {
        rename(doc, "old", "middle")
    }

    fn middle_to_new(doc: &mut DocumentMut) -> Option<Change> {
        rename(doc, "middle", "new")
    }

    /// 後の手順は、前の手順で書き換えた結果に当たる。
    static CHAIN: [Step; 2] = [
        Step {
            name: "old to middle",
            description: "section.old を section.middle にする",
            apply: old_to_middle,
        },
        Step {
            name: "middle to new",
            description: "section.middle を section.new にする",
            apply: middle_to_new,
        },
    ];

    #[test]
    fn steps_are_applied_in_order() {
        let migrated = migrate_with("[section]\nold = 1\n", &CHAIN);
        assert_eq!(migrated.text, "[section]\nnew = 1\n");
        assert!(migrated.changed());
        let names: Vec<&str> = migrated.changes.iter().map(|(step, _)| step.name).collect();
        assert_eq!(names, ["old to middle", "middle to new"]);
        assert_eq!(
            migrated.changes[0].1,
            Change {
                message: "read section.old as section.middle".to_owned(),
                dropped: vec!["nothing was dropped from section.old".to_owned()],
            }
        );
    }

    #[test]
    fn only_the_steps_that_find_an_old_form_are_reported() {
        let migrated = migrate_with("[section]\nmiddle = 1\n", &CHAIN);
        assert_eq!(migrated.text, "[section]\nnew = 1\n");
        let names: Vec<&str> = migrated.changes.iter().map(|(step, _)| step.name).collect();
        assert_eq!(names, ["middle to new"]);
    }

    #[test]
    fn the_rest_of_the_file_is_kept() {
        let text = "# mine\n[section]\n# explain\nold = 1 # note\nother = 2\n\n[next]\nold = 3\n";
        let migrated = migrate_with(text, &CHAIN);
        assert_eq!(
            migrated.text,
            "# mine\n[section]\nother = 2\n# explain\nnew = 1 # note\n\n[next]\nold = 3\n"
        );
    }

    #[test]
    fn migrated_text_is_left_as_it_is_when_migrated_again() {
        let migrated = migrate_with("[section]\nold = 1\n", &CHAIN);
        let again = migrate_with(&migrated.text, &CHAIN);
        assert!(!again.changed());
        assert_eq!(again.text, migrated.text);
    }

    #[test]
    fn line_endings_and_byte_order_mark_are_kept() {
        let migrated = migrate_with("\u{feff}[section]\r\nold = 1\r\n", &CHAIN);
        assert_eq!(migrated.text, "\u{feff}[section]\r\nnew = 1\r\n");
    }

    #[test]
    fn text_that_is_not_toml_is_left_as_it_is() {
        let text = "[section\nold = 1\n";
        let migrated = migrate_with(text, &CHAIN);
        assert!(!migrated.changed());
        assert_eq!(migrated.text, text);
    }

    #[test]
    fn current_files_are_left_as_they_are() {
        let migrated = migrate(config::TEMPLATE);
        assert!(!migrated.changed());
        assert_eq!(migrated.text, config::TEMPLATE);
        let parsed = config::parse(&migrated.text, &Config::default());
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    }
}
