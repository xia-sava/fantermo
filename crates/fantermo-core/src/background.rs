//! 背景画像: 設定の一覧（`background.images`）から候補のファイルを並べ、ランダムに選び、端末の背景の
//! 範囲に収める位置を決める。
//!
//! ファイルを読むことと画像を読むことは呼び出し元に任せる（[`Files`] と [`pick`] の `load`）。

use std::path::{Path, PathBuf};

use crate::config::BackgroundFit;

/// ディレクトリとワイルドカードから選ぶ画像の拡張子（大文字小文字は区別しない）。一覧に名前で書いた
/// ファイルは、拡張子によらず候補にする。
pub const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "bmp", "gif", "webp"];

/// `path` の拡張子が [`IMAGE_EXTENSIONS`] のどれか。
pub fn is_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// 一覧を展開するときに使う、ファイルシステムへの問い合わせ。
pub trait Files {
    /// `path` がディレクトリか。
    fn is_dir(&self, path: &Path) -> bool;
    /// ディレクトリ `dir` の中のファイル（ディレクトリは除く）。読めなければ、その理由の文。
    fn files_in(&self, dir: &Path) -> Result<Vec<PathBuf>, String>;
}

/// 一覧を展開した結果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expanded {
    /// 候補のファイル。一覧の順で、ディレクトリとワイルドカードの中は名前の順。同じファイルは一度だけ。
    pub candidates: Vec<PathBuf>,
    /// 候補を出さなかった項目についての警告。
    pub warnings: Vec<String>,
}

/// 一覧 `entries` を候補のファイルに展開する。相対パスは `base`（設定ファイルのフォルダ）からのパスと
/// する。
///
/// - ディレクトリは、その中の画像のファイル（[`is_image_file`]）。下のディレクトリは見ない。
/// - 最後の部分に `*`（0 文字以上）か `?`（1 文字）を含むものは、そのディレクトリの中で名前が合う画像の
///   ファイル。名前は大文字小文字を区別しない。途中の部分のワイルドカードは扱わない。
/// - ほかはファイルとして、そのまま候補にする（在るかどうかは読むときに分かる）。
pub fn expand(entries: &[String], base: Option<&Path>, files: &impl Files) -> Expanded {
    let mut expanded = Expanded::default();
    for entry in entries {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let path = match base {
            Some(base) if Path::new(entry).is_relative() => base.join(entry),
            _ => PathBuf::from(entry),
        };
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let found = if has_wildcard(&path.to_string_lossy()) {
            let Some(dir) = path.parent() else {
                continue;
            };
            if !has_wildcard(&name) || has_wildcard(&dir.to_string_lossy()) {
                expanded.warnings.push(format!(
                    "ignoring background image {entry:?}: wildcards are only allowed in the file name"
                ));
                continue;
            }
            match files.files_in(dir) {
                Ok(listed) => listed
                    .into_iter()
                    .filter(|file| {
                        is_image_file(file)
                            && file
                                .file_name()
                                .and_then(|name| name.to_str())
                                .is_some_and(|file_name| wildcard_matches(&name, file_name))
                    })
                    .collect(),
                Err(e) => {
                    expanded
                        .warnings
                        .push(format!("cannot list background images {entry:?}: {e}"));
                    continue;
                }
            }
        } else if files.is_dir(&path) {
            match files.files_in(&path) {
                Ok(listed) => listed
                    .into_iter()
                    .filter(|file| is_image_file(file))
                    .collect(),
                Err(e) => {
                    expanded
                        .warnings
                        .push(format!("cannot list background images {entry:?}: {e}"));
                    continue;
                }
            }
        } else {
            vec![path]
        };
        let mut found: Vec<PathBuf> = found;
        found.sort_by_key(|file| file.to_string_lossy().to_lowercase());
        if found.is_empty() {
            expanded
                .warnings
                .push(format!("no background images found for {entry:?}"));
        }
        for file in found {
            let key = file.to_string_lossy().to_lowercase();
            if !expanded
                .candidates
                .iter()
                .any(|seen| seen.to_string_lossy().to_lowercase() == key)
            {
                expanded.candidates.push(file);
            }
        }
    }
    expanded
}

fn has_wildcard(text: &str) -> bool {
    text.contains(['*', '?'])
}

/// ファイルの名前 `name` が、`*` と `?` を使った `pattern` に合うか。大文字小文字は区別しない。
pub fn wildcard_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let name: Vec<char> = name.chars().flat_map(char::to_lowercase).collect();
    let (mut p, mut n) = (0, 0);
    // 最後に見た `*` の位置と、そのとき名前のどこまで飲み込んだか。
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, n));
                p += 1;
            }
            Some(&c) if c == '?' || c == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match star {
                Some((star_p, star_n)) => {
                    p = star_p + 1;
                    n = star_n + 1;
                    star = Some((star_p, star_n + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// 候補 `candidates` からランダムに（`random(n)` が 0〜n-1 を返す）一つ選び、`load` で読む。読めなければ
/// その候補を外して選び直す。読めたものの場所と中身を返し、どれも読めなければ `None`。
pub fn pick<T>(
    candidates: &mut Vec<PathBuf>,
    random: &mut impl FnMut(usize) -> usize,
    mut load: impl FnMut(&Path) -> Option<T>,
) -> Option<(PathBuf, T)> {
    while !candidates.is_empty() {
        let index = random(candidates.len()).min(candidates.len() - 1);
        if let Some(loaded) = load(&candidates[index]) {
            return Some((candidates[index].clone(), loaded));
        }
        candidates.remove(index);
    }
    None
}

/// 画像を選ぶための、xorshift64* の擬似乱数。
#[derive(Clone, Debug)]
pub struct Random {
    state: u64,
}

impl Random {
    /// 種 `seed` から始める。0 は使えないので別の値に置き換える。
    pub fn new(seed: u64) -> Self {
        const NONZERO_SEED: u64 = 0x9e37_79b9_7f4a_7c15;
        Self {
            state: if seed == 0 { NONZERO_SEED } else { seed },
        }
    }

    /// 0 以上 `count` 未満の数。`count` が 0 なら 0。
    pub fn below(&mut self, count: usize) -> usize {
        const MULTIPLIER: u64 = 0x2545_f491_4f6c_dd1d;
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let value = self.state.wrapping_mul(MULTIPLIER);
        if count == 0 {
            return 0;
        }
        // 上位のビットのほうが偏りが少ない。
        ((u128::from(value) * count as u128) >> 64) as usize
    }
}

/// 大きさ `image` の画像を持っておく大きさ。`cover`（画面の大きさ）を縦横比を保って覆える大きさより
/// 大きければ、覆える大きさまで縮める。さらに、どちらの辺も `max_side` を超えないように縮める。広げは
/// しない。`cover` のどちらかが 0 なら、覆える大きさでは縮めない。
pub fn stored_size(image: (u32, u32), cover: (u32, u32), max_side: u32) -> (u32, u32) {
    let (width, height) = (image.0 as f64, image.1 as f64);
    if width <= 0.0 || height <= 0.0 {
        return image;
    }
    let mut scale: f64 = 1.0;
    if cover.0 > 0 && cover.1 > 0 {
        scale = scale.min((f64::from(cover.0) / width).max(f64::from(cover.1) / height));
    }
    let max_side = f64::from(max_side.max(1));
    scale = scale.min(max_side / width).min(max_side / height);
    if scale >= 1.0 {
        return image;
    }
    let shrink = |length: f64| ((length * scale).round() as u32).clamp(1, max_side as u32);
    (shrink(width), shrink(height))
}

/// 濃さ `image_opacity`（0〜1）の画像を見せるために、画像の上に重ねる端末の背景色の濃さ（不透明度）。
/// 範囲の外は端に寄せる。
pub fn overlay_opacity(image_opacity: f32) -> f32 {
    (1.0 - image_opacity).clamp(0.0, 1.0)
}

/// ピクセルの矩形（左上の x, y と幅, 高さ）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// 画像を描く位置。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// 描く矩形（ピクセル）。
    pub rect: Rect,
    /// `rect` に写す画像の範囲。画像の幅と高さを 1 とする。
    pub uv: Rect,
    /// 画像を繰り返す（`uv` が 1 を超える）。
    pub repeat: bool,
}

/// 大きさ `image`（ピクセル）の画像を、範囲 `region` に `fit` のとおりに収める位置。画像か範囲が空なら
/// `None`。
pub fn place(fit: BackgroundFit, region: Rect, image: (u32, u32)) -> Option<Placement> {
    let (image_width, image_height) = (image.0 as f32, image.1 as f32);
    if image_width <= 0.0 || image_height <= 0.0 || region.width <= 0.0 || region.height <= 0.0 {
        return None;
    }
    const WHOLE: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };
    let placement = match fit {
        BackgroundFit::Fill => {
            let scale = (region.width / image_width).max(region.height / image_height);
            let (u, width) = centered_part(region.width, image_width * scale);
            let (v, height) = centered_part(region.height, image_height * scale);
            Placement {
                rect: region,
                uv: Rect {
                    x: u,
                    y: v,
                    width,
                    height,
                },
                repeat: false,
            }
        }
        BackgroundFit::Fit => {
            let scale = (region.width / image_width).min(region.height / image_height);
            let (width, height) = (image_width * scale, image_height * scale);
            Placement {
                rect: Rect {
                    x: region.x + (region.width - width) / 2.0,
                    y: region.y + (region.height - height) / 2.0,
                    width,
                    height,
                },
                uv: WHOLE,
                repeat: false,
            }
        }
        BackgroundFit::Stretch => Placement {
            rect: region,
            uv: WHOLE,
            repeat: false,
        },
        BackgroundFit::Tile => Placement {
            rect: region,
            uv: Rect {
                x: 0.0,
                y: 0.0,
                width: region.width / image_width,
                height: region.height / image_height,
            },
            repeat: true,
        },
        BackgroundFit::Center => {
            let (x, width, u, uv_width) = centered_axis(region.x, region.width, image_width);
            let (y, height, v, uv_height) = centered_axis(region.y, region.height, image_height);
            Placement {
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                uv: Rect {
                    x: u,
                    y: v,
                    width: uv_width,
                    height: uv_height,
                },
                repeat: false,
            }
        }
    };
    Some(placement)
}

/// 長さ `shown` に広げた画像の真ん中の、長さ `visible` の部分。画像の長さを 1 とした始まりと長さ。
fn centered_part(visible: f32, shown: f32) -> (f32, f32) {
    let length = (visible / shown).min(1.0);
    ((1.0 - length) / 2.0, length)
}

/// 縮めずに置いた長さ `image` の画像を、始まり `start`・長さ `length` の範囲の真ん中に置く。範囲より
/// 短ければ範囲の中に、長ければ真ん中の部分を範囲いっぱいに。描く始まりと長さ、画像の長さを 1 とした
/// 始まりと長さ。ピクセルの格子に合わせ、ずらす量は整数のピクセルに切り捨てる。
fn centered_axis(start: f32, length: f32, image: f32) -> (f32, f32, f32, f32) {
    if image <= length {
        let offset = ((length - image) / 2.0).floor();
        (start + offset, image, 0.0, 1.0)
    } else {
        let offset = ((image - length) / 2.0).floor();
        (start, length, offset / image, length / image)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// 決まった中身のファイルシステム。キーはディレクトリ、値はその中のファイルの名前。
    struct FakeFiles(HashMap<PathBuf, Vec<&'static str>>);

    impl Files for FakeFiles {
        fn is_dir(&self, path: &Path) -> bool {
            self.0.contains_key(path)
        }

        fn files_in(&self, dir: &Path) -> Result<Vec<PathBuf>, String> {
            self.0
                .get(dir)
                .map(|names| names.iter().map(|name| dir.join(name)).collect())
                .ok_or_else(|| "not found".to_owned())
        }
    }

    fn files() -> FakeFiles {
        FakeFiles(HashMap::from([
            (
                PathBuf::from(r"C:\pics"),
                vec![
                    "b.PNG",
                    "a.jpg",
                    "notes.txt",
                    "c.webp",
                    "d.jpeg",
                    "e.gif",
                    "f.bmp",
                ],
            ),
            (PathBuf::from(r"C:\conf\walls"), vec!["x.png", "y.tif"]),
            (PathBuf::from(r"C:\empty"), vec!["readme.md"]),
        ]))
    }

    fn entries(items: &[&str]) -> Vec<String> {
        items.iter().map(|&item| item.to_owned()).collect()
    }

    fn paths(items: &[&str]) -> Vec<PathBuf> {
        items.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn directories_give_their_images_in_name_order() {
        let expanded = expand(&entries(&[r"C:\pics"]), None, &files());
        assert_eq!(
            expanded.candidates,
            paths(&[
                r"C:\pics\a.jpg",
                r"C:\pics\b.PNG",
                r"C:\pics\c.webp",
                r"C:\pics\d.jpeg",
                r"C:\pics\e.gif",
                r"C:\pics\f.bmp",
            ])
        );
        assert!(expanded.warnings.is_empty());
    }

    #[test]
    fn wildcards_match_names_without_case_and_only_images() {
        let expanded = expand(
            &entries(&[r"C:\pics\*.png", r"C:\pics\?.JP*", r"C:\pics\*"]),
            None,
            &files(),
        );
        assert_eq!(
            expanded.candidates,
            paths(&[
                r"C:\pics\b.PNG",
                r"C:\pics\a.jpg",
                r"C:\pics\d.jpeg",
                r"C:\pics\c.webp",
                r"C:\pics\e.gif",
                r"C:\pics\f.bmp",
            ])
        );
        assert!(expanded.warnings.is_empty(), "{:?}", expanded.warnings);
    }

    #[test]
    fn files_are_taken_as_written_and_relative_paths_start_at_the_base() {
        let expanded = expand(
            &entries(&["walls", "one.tif", r"C:\pics\a.jpg", "  ", r"C:\PICS\A.JPG"]),
            Some(Path::new(r"C:\conf")),
            &files(),
        );
        // 名前で書いたファイルは拡張子によらず候補にする。同じファイルは一度だけ。
        assert_eq!(
            expanded.candidates,
            paths(&[r"C:\conf\walls\x.png", r"C:\conf\one.tif", r"C:\pics\a.jpg"])
        );
    }

    #[test]
    fn entries_without_images_are_reported() {
        let expanded = expand(
            &entries(&[
                r"C:\empty",
                r"C:\missing\*.png",
                r"C:\*\a.png",
                r"C:\pics\*.tif",
            ]),
            None,
            &files(),
        );
        assert!(expanded.candidates.is_empty());
        assert_eq!(expanded.warnings.len(), 4, "{:?}", expanded.warnings);
        assert!(expanded.warnings[0].starts_with("no background images found"));
        assert!(expanded.warnings[1].starts_with("cannot list background images"));
        assert!(expanded.warnings[2].contains("only allowed in the file name"));
    }

    #[test]
    fn wildcard_patterns() {
        assert!(wildcard_matches("*.jpg", "a.JPG"));
        assert!(wildcard_matches("*", "anything.png"));
        assert!(wildcard_matches("a?c.png", "abc.png"));
        assert!(!wildcard_matches("a?c.png", "ac.png"));
        assert!(wildcard_matches("*a*b*", "xxaxxbxx"));
        assert!(!wildcard_matches("*a*b", "xxaxxbxxc"));
        assert!(wildcard_matches("壁紙*.png", "壁紙01.png"));
        assert!(!wildcard_matches("*.png", "a.png.txt"));
    }

    #[test]
    fn unreadable_images_are_dropped_and_another_is_picked() {
        let mut candidates = paths(&["bad1", "good", "bad2"]);
        let mut tried = Vec::new();
        // いつも先頭を選ぶ。
        let picked = pick(&mut candidates, &mut |_| 0, |path| {
            tried.push(path.to_owned());
            (path != Path::new("bad1")).then_some(7)
        });
        assert_eq!(picked, Some((PathBuf::from("good"), 7)));
        assert_eq!(tried, paths(&["bad1", "good"]));
        assert_eq!(candidates, paths(&["good", "bad2"]));

        let mut candidates = paths(&["bad1", "bad2"]);
        assert_eq!(pick(&mut candidates, &mut |n| n - 1, |_| None::<()>), None);
        assert!(candidates.is_empty());
        assert_eq!(pick(&mut Vec::new(), &mut |_| 0, |_| Some(())), None);
    }

    #[test]
    fn random_numbers_stay_below_the_count_and_vary() {
        let mut random = Random::new(1);
        let mut seen = [false; 5];
        for _ in 0..200 {
            let value = random.below(5);
            assert!(value < 5);
            seen[value] = true;
        }
        assert!(seen.iter().all(|&seen| seen));
        assert_eq!(random.below(0), 0);
        assert_eq!(Random::new(0).below(10), Random::new(0).below(10));
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// 横 400・縦 200 の範囲（左上は 10, 20）。
    const REGION: Rect = Rect {
        x: 10.0,
        y: 20.0,
        width: 400.0,
        height: 200.0,
    };

    #[test]
    fn fill_covers_the_region_and_cuts_the_overflow_evenly() {
        // 正方形の画像は横に合わせて広げ、上下を同じだけ切る。
        let placed = place(BackgroundFit::Fill, REGION, (100, 100)).unwrap();
        assert_eq!(placed.rect, REGION);
        assert_eq!(placed.uv, rect(0.0, 0.25, 1.0, 0.5));
        assert!(!placed.repeat);
        // 横に長い画像は縦に合わせ、左右を切る。
        let placed = place(BackgroundFit::Fill, REGION, (800, 200)).unwrap();
        assert_eq!(placed.uv, rect(0.25, 0.0, 0.5, 1.0));
    }

    #[test]
    fn fit_keeps_the_whole_image_in_the_middle() {
        let placed = place(BackgroundFit::Fit, REGION, (100, 100)).unwrap();
        assert_eq!(placed.rect, rect(110.0, 20.0, 200.0, 200.0));
        assert_eq!(placed.uv, rect(0.0, 0.0, 1.0, 1.0));
        let placed = place(BackgroundFit::Fit, REGION, (800, 200)).unwrap();
        assert_eq!(placed.rect, rect(10.0, 70.0, 400.0, 100.0));
    }

    #[test]
    fn stretch_fills_the_region_with_the_whole_image() {
        let placed = place(BackgroundFit::Stretch, REGION, (30, 70)).unwrap();
        assert_eq!(placed.rect, REGION);
        assert_eq!(placed.uv, rect(0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    fn tile_repeats_the_image_at_its_size_from_the_top_left() {
        let placed = place(BackgroundFit::Tile, REGION, (64, 50)).unwrap();
        assert_eq!(placed.rect, REGION);
        assert_eq!(placed.uv, rect(0.0, 0.0, 6.25, 4.0));
        assert!(placed.repeat);
    }

    #[test]
    fn center_keeps_the_size_and_cuts_what_does_not_fit() {
        // 小さい画像は縮めずに真ん中へ。
        let placed = place(BackgroundFit::Center, REGION, (101, 50)).unwrap();
        assert_eq!(placed.rect, rect(159.0, 95.0, 101.0, 50.0));
        assert_eq!(placed.uv, rect(0.0, 0.0, 1.0, 1.0));
        // 横に大きい画像は、真ん中の部分を範囲の幅で。
        let placed = place(BackgroundFit::Center, REGION, (800, 100)).unwrap();
        assert_eq!(placed.rect, rect(10.0, 70.0, 400.0, 100.0));
        assert_eq!(placed.uv, rect(0.25, 0.0, 0.5, 1.0));
    }

    #[test]
    fn empty_images_and_regions_are_not_placed() {
        for fit in BackgroundFit::ALL {
            assert_eq!(place(fit, REGION, (0, 10)), None);
            assert_eq!(place(fit, rect(0.0, 0.0, 0.0, 10.0), (10, 10)), None);
        }
    }

    #[test]
    fn large_images_are_kept_just_large_enough_to_cover_the_screen() {
        // 画面より小さければそのまま。
        assert_eq!(stored_size((800, 600), (1920, 1080), 16384), (800, 600));
        // 横に長い画像は、高さを画面に合わせる（fill で覆える大きさ）。
        assert_eq!(stored_size((8000, 2000), (1920, 1080), 16384), (4320, 1080));
        assert_eq!(stored_size((4000, 4000), (1920, 1080), 16384), (1920, 1920));
        // 片方だけ画面より大きい画像は縮めない。
        assert_eq!(stored_size((3000, 500), (1920, 1080), 16384), (3000, 500));
        // 辺の上限。
        assert_eq!(stored_size((40000, 100), (0, 0), 16384), (16384, 41));
        assert_eq!(stored_size((0, 10), (1920, 1080), 16384), (0, 10));
    }

    #[test]
    fn the_background_color_is_laid_over_the_image_by_the_rest_of_its_opacity() {
        assert_eq!(overlay_opacity(1.0), 0.0);
        assert_eq!(overlay_opacity(0.0), 1.0);
        assert_eq!(overlay_opacity(0.25), 0.75);
        assert_eq!(overlay_opacity(1.5), 0.0);
        assert_eq!(overlay_opacity(-1.0), 1.0);
    }

    #[test]
    fn image_extensions_ignore_case() {
        assert!(is_image_file(Path::new("a.JPEG")));
        assert!(is_image_file(Path::new(r"C:\x\y.WebP")));
        assert!(!is_image_file(Path::new("a.tif")));
        assert!(!is_image_file(Path::new("png")));
    }
}
