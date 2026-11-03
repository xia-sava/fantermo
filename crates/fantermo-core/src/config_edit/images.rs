//! 背景画像の一覧（`[[background.images]]`）の書き戻し。

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};

use super::document::{
    Following, decor_prefix, following, following_prefix, is_standard, number_value,
    remove_table_value, replace_line, section_mut, set_following_prefix, set_table_value,
    split_after_last_blank_line, table_at, table_at_mut, value_prefixes,
};
use crate::config::{self, BackgroundFit, BackgroundImage, Config, IMAGE_KEYS};

/// `[background]` のテーブルのキーのパス。
const BACKGROUND_PATH: &[&str] = &["background"];

/// `[background]` の、背景画像の一覧（`[[background.images]]`）のキー。
const IMAGES: &str = "images";

/// `[[background.images]]` のテーブル一つ。書かない値（全体の値に従う表示方法と不透明度）は入れない。
fn image_table(image: &BackgroundImage) -> Table {
    let mut table = Table::new();
    table.insert(
        IMAGE_KEYS.path,
        Item::Value(Value::from(image.path.as_str())),
    );
    if let Some(fit) = image.fit {
        table.insert(IMAGE_KEYS.fit, Item::Value(Value::from(fit.name())));
    }
    if let Some(opacity) = image.image_opacity {
        table.insert(IMAGE_KEYS.image_opacity, Item::Value(number_value(opacity)));
    }
    table
}

/// ファイルに書いてある `[[background.images]]` のテーブル一つ `table` のパス。読めないテーブル（パスの
/// ないテーブルや、パスが空のもの）なら `None`。
fn written_image_path(table: &Table) -> Option<&str> {
    let path = table.get(IMAGE_KEYS.path)?.as_str()?;
    (!path.trim().is_empty()).then_some(path)
}

/// ファイルに書いてある `[[background.images]]` のテーブル一つ `table` の、表示方法と不透明度。読めない
/// 値は書いていないものとする（設定を読むときと同じ）。
fn written_image_settings(table: &Table) -> (Option<BackgroundFit>, Option<f32>) {
    let fit = table
        .get(IMAGE_KEYS.fit)
        .and_then(Item::as_str)
        .and_then(BackgroundFit::from_name);
    let opacity = table
        .get(IMAGE_KEYS.image_opacity)
        .and_then(Item::as_value)
        .and_then(number_of)
        .map(|opacity| opacity as f32)
        .filter(|opacity| config::IMAGE_OPACITY_RANGE.contains(opacity));
    (fit, opacity)
}

/// 数の値（整数か小数）。
fn number_of(value: &Value) -> Option<f64> {
    value
        .as_float()
        .or_else(|| value.as_integer().map(|integer| integer as f64))
}

/// 背景画像の一覧が `base` と `edited` で違えば、`[[background.images]]` のテーブルを書き換える。
/// テーブルを書いていなければ `[background]` の値の後ろに足す。`images` にテーブルの配列でない値が
/// 書いてあれば（`images = 3` など）、その行を消してテーブルを足す。
pub(super) fn update_background_images(
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
    insert_image_tables(doc, tables)
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
/// 最初のテーブルの前に移す（[`TEMPLATE`] の一覧の説明の下にテーブルが入る）。
///
/// [`TEMPLATE`]: crate::config::TEMPLATE
fn insert_image_tables(doc: &mut DocumentMut, tables: Vec<(String, Table)>) -> Result<(), String> {
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
    if let Following::Table(_) = following {
        let after = following_prefix(doc, &following);
        let (moved, rest) = split_after_last_blank_line(&after);
        if !moved.is_empty() {
            first = moved;
        }
        set_following_prefix(doc, &following, format!("\n{rest}"));
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
            Some(fit) => set_table_value(table, IMAGE_KEYS.fit, Value::from(fit.name())),
            None => remove_table_value(table, IMAGE_KEYS.fit),
        }
    }
    if opacity != image.image_opacity {
        match image.image_opacity {
            Some(opacity) => {
                set_table_value(table, IMAGE_KEYS.image_opacity, number_value(opacity))
            }
            None => remove_table_value(table, IMAGE_KEYS.image_opacity),
        }
    }
}
