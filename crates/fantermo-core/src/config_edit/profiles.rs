//! プロファイル（`[[profiles]]`）の書き戻し。

use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

use super::document::{
    Following, Header, Reset, Target, decor_prefix, first_after, following_prefix, reset, set,
    set_following_prefix, set_or_remove_table_text, set_table_text, split_after_last_blank_line,
    tables,
};
use crate::config::{BackendKind, Config, PROFILE_KEYS, Profile};

/// 最上位のキーの名前。
const PROFILES_KEY: &str = "profiles";
const SHELL_KEY: &str = "shell";

/// プロファイルが `base` と `edited` で違えば書き換える。最上位の `shell` の行はコメントにする（書いた
/// プロファイルが使われ、`shell` は使われないため）。既定のプロファイルだけに戻したら、テーブルを消す。
pub(super) fn update_profiles(
    doc: &mut DocumentMut,
    base: &Config,
    edited: &Config,
) -> Result<(), String> {
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
    table.insert(PROFILE_KEYS.name, Value::from(profile.name.as_str()));
    if let Some(command) = profile_command(profile) {
        table.insert(PROFILE_KEYS.command, Value::from(command));
    }
    if profile.backend != BackendKind::default() {
        table.insert(PROFILE_KEYS.backend, Value::from(profile.backend.name()));
    }
    for (key, text) in [
        (PROFILE_KEYS.cygwin_root, &profile.cygwin_root),
        (PROFILE_KEYS.distribution, &profile.distribution),
        (PROFILE_KEYS.icon, &profile.icon),
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
                    text_of(table, PROFILE_KEYS.name),
                    text_of(table, PROFILE_KEYS.command),
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
    set_table_text(table, PROFILE_KEYS.name, &profile.name);
    set_or_remove_table_text(table, PROFILE_KEYS.command, profile_command(profile));
    if table.contains_key(PROFILE_KEYS.backend) || profile.backend != BackendKind::default() {
        set_table_text(table, PROFILE_KEYS.backend, profile.backend.name());
    }
    set_or_remove_table_text(
        table,
        PROFILE_KEYS.cygwin_root,
        profile.cygwin_root.as_deref(),
    );
    set_or_remove_table_text(
        table,
        PROFILE_KEYS.distribution,
        profile.distribution.as_deref(),
    );
    set_or_remove_table_text(table, PROFILE_KEYS.icon, profile.icon.as_deref());
}

/// プロファイルの `command` に書く値。ブリッジで空なら（相手のログインシェル）書かない。
fn profile_command(profile: &Profile) -> Option<&str> {
    (!(profile.backend.is_bridge() && profile.command.is_empty())).then_some(&profile.command)
}
