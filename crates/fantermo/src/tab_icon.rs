//! タブのタイトルの左に出す、小さな正方形のアイコン。
//!
//! プロファイルに画像を書いていなければ、角を丸めた淡い色の正方形に、利用者の環境にある本物のアイコン
//! （`Cygwin.ico`、`wsl.exe`、ConPTY の実行ファイル）を載せて作る。読めなければ、接続方式ごとの絵（Cygwin の
//! C、WSL のペンギン、ConPTY の `>_`）を描く。同じ絵になるプロファイルが複数あるときだけ、絵を小さくして
//! プロファイル名の頭文字を添える。色はプロファイル名から決まるので、起動のたびに同じになる。
//!
//! このファイルはプロファイルからアイコン（[`TabIcon`]）を決める。ピクセル（乗算済みアルファの BGRA の
//! [`Decoded`]）を作るのは `draw` の [`Generator`]。
//!
//! ピクセルは `App` がタブを開いたときと、DPI か UI のフォントの書体が変わったときに作って [`Pixels`] に持ち、
//! 描くときにはファイルを読まない。管理者として動いているウィンドウでは、作ったピクセルの右下に UAC の盾を
//! 重ねてから持つ（[`Generator::overlay_shield`]）。

use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
};

use fantermo_core::{
    config::{self, BackendKind, Profile},
    frame::Rgb,
    vt,
};
use windows::{Win32::UI::Shell::ExtractIconExW, core::HSTRING};

use crate::{app_files::path_key, bridge, image_decoder::Decoded};

mod draw;

pub use draw::Generator;

/// プロファイル名から選ぶ背景の色。青・水色・青緑・緑・藍・紫の淡い色で、警告に使う赤・橙・黄は入れない。
/// 濃い色の絵と頭文字が読め、明るいタブの白と灰色のどちらからも、暗いタブからも分かれる明るさの色だけを
/// 並べる（テストで固定する）。
const PALETTE: [Rgb; 10] = [
    rgb(0xd2, 0xe4, 0xf9),
    rgb(0xc5, 0xe8, 0xf7),
    rgb(0xb3, 0xeb, 0xf4),
    rgb(0x9d, 0xf1, 0xe9),
    rgb(0xa1, 0xf2, 0xd7),
    rgb(0xaa, 0xf3, 0xc2),
    rgb(0xaa, 0xf3, 0xb0),
    rgb(0xde, 0xde, 0xfc),
    rgb(0xe4, 0xdc, 0xfb),
    rgb(0xec, 0xdc, 0xfb),
];

/// プロファイルなしで開いたタブの背景の色。
const NO_PROFILE_COLOR: Rgb = rgb(0xdc, 0xe3, 0xea);

/// Cygwin のフォルダにあるアイコンのファイル。
const CYGWIN_ICON_FILE: &str = "Cygwin.ico";

/// WSL のアイコンを読む実行ファイル。
const WSL_EXECUTABLE: &str = "wsl.exe";

const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

/// タブのアイコン。タブを開いたときに決まり、プロファイルを変えても消しても変わらない。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TabIcon {
    /// 画像のファイル。読めなければ下の値で作る。
    pub image: Option<PathBuf>,
    /// 背景の正方形に載せる本物のアイコン。ないときと読めないときは `backend` の絵を描く。
    pub real: Option<RealIcon>,
    pub backend: BackendKind,
    /// 絵に添える頭文字。同じ絵になるプロファイルが複数あるときだけ。
    pub initial: Option<String>,
    /// 背景の色。
    pub color: Rgb,
}

/// 利用者の環境にある、本物のアイコンの読み先。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RealIcon {
    /// アイコンのファイル（ICO）。
    File(PathBuf),
    /// アイコンを埋め込んだ実行ファイル。
    Executable(PathBuf),
}

impl RealIcon {
    fn path(&self) -> &Path {
        match self {
            Self::File(path) | Self::Executable(path) => path,
        }
    }
}

/// プロファイル `profile` の本物のアイコン。なければ `None`。
fn real_icon(profile: &Profile) -> Option<RealIcon> {
    real_icon_candidate(profile).filter(|real| has_icon(real.path()))
}

/// プロファイル `profile` の本物のアイコンの読み先。ファイルにアイコンがあるかは見ていない。
fn real_icon_candidate(profile: &Profile) -> Option<RealIcon> {
    real_icon_candidate_with(
        profile,
        |configured| bridge::cygwin_root(configured).ok(),
        search_path,
    )
}

/// プロファイル `profile` の本物のアイコン。`cygwin_root` は `cygwin_root` の設定から Cygwin のフォルダを、
/// `search` はプログラムの名前から実行ファイルを探し、`has_icon` はファイルにアイコンがあるかを答える。
#[cfg(test)]
fn real_icon_with(
    profile: &Profile,
    cygwin_root: impl Fn(Option<&str>) -> Option<PathBuf>,
    search: impl Fn(&str) -> Option<PathBuf>,
    has_icon: impl Fn(&Path) -> bool,
) -> Option<RealIcon> {
    real_icon_candidate_with(profile, cygwin_root, search).filter(|real| has_icon(real.path()))
}

/// プロファイル `profile` の本物のアイコンの読み先。`cygwin_root` は `cygwin_root` の設定から Cygwin のフォルダを、
/// `search` はプログラムの名前から実行ファイルを探す。
fn real_icon_candidate_with(
    profile: &Profile,
    cygwin_root: impl Fn(Option<&str>) -> Option<PathBuf>,
    search: impl Fn(&str) -> Option<PathBuf>,
) -> Option<RealIcon> {
    Some(match profile.backend {
        BackendKind::Cygwin => {
            RealIcon::File(cygwin_root(profile.cygwin_root.as_deref())?.join(CYGWIN_ICON_FILE))
        }
        BackendKind::Wsl => RealIcon::Executable(search(WSL_EXECUTABLE)?),
        BackendKind::Conpty => RealIcon::Executable(search(profile.command_program())?),
    })
}

/// プログラム `program` の実行ファイル。名前だけなら、ConPTY で起動するときと同じく
/// [`config::find_program_on_path`] で環境変数 `PATH` から探す。フォルダかドライブ名があれば、書いたパス
/// （拡張子がなければ `.exe` を足す）。
fn search_path(program: &str) -> Option<PathBuf> {
    // 空は名前だけでもパスでもないので、`.exe` だけのパスにしない。
    if program.is_empty() {
        return None;
    }
    if !config::is_bare_program_name(program) {
        let mut path = PathBuf::from(program);
        let has_extension = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().contains('.'));
        if !has_extension {
            path.set_extension("exe");
        }
        return Some(path);
    }
    config::find_program_on_path(program, env::var_os("PATH").as_deref(), Path::is_file)
}

/// ファイル `path` にアイコンがあるか。既定のアプリのアイコンは数えない。
fn has_icon(path: &Path) -> bool {
    /// アイコンを作らずに数だけを問い合わせる位置。
    const COUNT_ICONS: i32 = -1;
    // SAFETY: 文字列は呼び出しの間は生きている。出力先を渡さないので数を返すだけ。
    let count =
        unsafe { ExtractIconExW(&HSTRING::from(path.as_os_str()), COUNT_ICONS, None, None, 0) };
    // 読めないファイルでは u32::MAX が返る。
    count != 0 && count != u32::MAX
}

/// プロファイルがタブに出す絵。同じ値なら見分けがつかない。パスの大文字と小文字、`/` と `\` の違いは同じ
/// ファイル。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Picture {
    /// `icon` に書いた画像のファイル。
    Image(String),
    /// 本物のアイコンのファイル。接続方式によらず、ファイルが同じなら同じ絵。
    Real(String),
    /// 接続方式ごとの絵。
    Drawn(BackendKind),
}

impl Picture {
    fn of(icon: &TabIcon) -> Self {
        match (&icon.image, &icon.real) {
            (Some(path), _) => Self::Image(path_key(path)),
            (None, Some(real)) => Self::Real(path_key(real.path())),
            (None, None) => Self::Drawn(icon.backend),
        }
    }
}

/// `pictures` のそれぞれについて、同じ絵がほかにもあるか。
fn shared_pictures(pictures: &[Picture]) -> Vec<bool> {
    pictures
        .iter()
        .map(|picture| pictures.iter().filter(|other| *other == picture).count() > 1)
        .collect()
}

impl TabIcon {
    /// プロファイルなしで開いたタブ（fantermo の引数のコマンドライン）のアイコン。
    pub const fn without_profile() -> Self {
        Self {
            image: None,
            real: None,
            backend: BackendKind::Conpty,
            initial: None,
            color: NO_PROFILE_COLOR,
        }
    }

    /// 設定の `profiles` のそれぞれのアイコン。相対パスの画像は `config_dir` から辿る。頭文字は `profiles`
    /// 全体を見て、同じ絵になるプロファイルが複数あるものにだけ付ける。
    pub fn for_profiles(profiles: &[Profile], config_dir: Option<&Path>) -> Vec<Self> {
        Self::for_profiles_with(profiles, config_dir, real_icon)
    }

    /// [`Self::for_profiles`] の本体。`real_of` はプロファイルの本物のアイコンを探す。
    fn for_profiles_with(
        profiles: &[Profile],
        config_dir: Option<&Path>,
        real_of: impl Fn(&Profile) -> Option<RealIcon>,
    ) -> Vec<Self> {
        let mut icons: Vec<Self> = profiles
            .iter()
            .map(|profile| Self {
                real: real_of(profile),
                ..Self::without_initial(profile, config_dir)
            })
            .collect();
        let pictures: Vec<Picture> = icons.iter().map(Picture::of).collect();
        let shared = shared_pictures(&pictures);
        for ((icon, profile), shared) in icons.iter_mut().zip(profiles).zip(shared) {
            if shared {
                icon.initial = initial(&profile.name);
            }
        }
        icons
    }

    /// 設定の `profiles` の `index` 番目（並びの中にあること）のアイコン。[`Self::for_profiles`] の同じ位置の
    /// ものと同じになる。ほかのプロファイルの本物のアイコンは、同じ絵になりうるものだけを探す。
    pub fn for_profile(profiles: &[Profile], index: usize, config_dir: Option<&Path>) -> Self {
        Self::for_profile_with(profiles, index, config_dir, real_icon_candidate, has_icon)
    }

    /// [`Self::for_profile`] の本体。`candidate_of` はプロファイルの本物のアイコンの読み先を探し、`has_icon` は
    /// ファイルにアイコンがあるかを答える。
    fn for_profile_with(
        profiles: &[Profile],
        index: usize,
        config_dir: Option<&Path>,
        candidate_of: impl Fn(&Profile) -> Option<RealIcon>,
        has_icon: impl Fn(&Path) -> bool,
    ) -> Self {
        let profile = &profiles[index];
        let mut icon = Self {
            real: candidate_of(profile).filter(|real| has_icon(real.path())),
            ..Self::without_initial(profile, config_dir)
        };
        let picture = Picture::of(&icon);
        let shared = profiles
            .iter()
            .enumerate()
            .filter(|(other_index, _)| *other_index != index)
            .any(|(_, other)| {
                let other_icon = Self::without_initial(other, config_dir);
                if other_icon.image.is_some() {
                    return Picture::of(&other_icon) == picture;
                }
                // 画像のないプロファイルの絵は、本物のアイコンがあればそのファイル、なければ接続方式で決まる。
                match &picture {
                    Picture::Image(_) => false,
                    Picture::Real(key) => candidate_of(other)
                        .is_some_and(|real| path_key(real.path()) == *key && has_icon(real.path())),
                    Picture::Drawn(backend) => {
                        other.backend == *backend
                            && candidate_of(other).is_none_or(|real| !has_icon(real.path()))
                    }
                }
            });
        if shared {
            icon.initial = initial(&profile.name);
        }
        icon
    }

    /// プロファイル `profile` の、本物のアイコンと頭文字を決める前のアイコン。
    fn without_initial(profile: &Profile, config_dir: Option<&Path>) -> Self {
        let image = profile.icon.as_deref().map(|path| match config_dir {
            Some(folder) => folder.join(path),
            None => PathBuf::from(path),
        });
        Self {
            image,
            real: None,
            backend: profile.backend,
            initial: None,
            color: color_of(&profile.name),
        }
    }
}

/// プロファイル名 `name` の頭文字。空白を除いた最初の書記素クラスタ（結合文字やサロゲートペアを分けない）で、
/// 大文字があれば大文字にする。幅のあるものがなければ `None`。
pub fn initial(name: &str) -> Option<String> {
    let grapheme = vt::split_graphemes(name)
        .into_iter()
        .find(|grapheme| grapheme.width > 0 && !grapheme.text.trim().is_empty())?;
    let upper = grapheme.text.to_uppercase();
    // 「ß」のように文字の数が変わるものは、一文字に見えなくなるのでそのまま。
    Some(if upper.chars().count() == grapheme.text.chars().count() {
        upper
    } else {
        grapheme.text
    })
}

/// プロファイル名 `name` の背景の色。
pub fn color_of(name: &str) -> Rgb {
    PALETTE[hash(name) as usize % PALETTE.len()]
}

/// FNV-1a。実行のたびにも Rust の版でも変わらないようにする。
fn hash(text: &str) -> u32 {
    const OFFSET_BASIS: u32 = 0x811c_9dc5;
    const PRIME: u32 = 0x0100_0193;
    text.bytes().fold(OFFSET_BASIS, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(PRIME)
    })
}

/// 開いているタブのアイコンのピクセル。一辺か頭文字の書体が変わったら作り直す。作れなかったものは `None` で
/// 覚え、作り直さない。
#[derive(Default)]
pub struct Pixels {
    side: u32,
    family: String,
    made: HashMap<TabIcon, Option<Decoded>>,
}

impl Pixels {
    /// `icons` のそれぞれの、一辺 `side`、頭文字の書体 `family` のピクセルを揃える。持っていないものは `make` で
    /// 作り、`icons` にないものは捨てる。
    pub fn update<'a>(
        &mut self,
        icons: impl IntoIterator<Item = &'a TabIcon>,
        side: u32,
        family: &str,
        mut make: impl FnMut(&TabIcon) -> Option<Decoded>,
    ) {
        if side != self.side || family != self.family {
            self.made.clear();
            self.side = side;
            family.clone_into(&mut self.family);
        }
        let icons: Vec<&TabIcon> = icons.into_iter().collect();
        self.made.retain(|icon, _| icons.contains(&icon));
        for icon in icons {
            if !self.made.contains_key(icon) {
                let pixels = make(icon);
                self.made.insert(icon.clone(), pixels);
            }
        }
    }

    /// 作れたアイコンとそのピクセル。
    pub fn iter(&self) -> impl Iterator<Item = (&TabIcon, &Decoded)> {
        self.made
            .iter()
            .filter_map(|(icon, pixels)| Some((icon, pixels.as_ref()?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 相対輝度（WCAG）。
    fn luminance(color: Rgb) -> f32 {
        let channel = |value: u8| {
            let value = f32::from(value) / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
    }

    /// コントラスト比（WCAG）。
    fn contrast(a: Rgb, b: Rgb) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn every_color_reads_on_light_and_dark_tabs() {
        // タブの色: 既定の表示しているタブ、既定のタブバー、暗い `[colors.tab_bar]` の表示しているタブ、
        // タブバー。淡い色は白にも灰色にも近いので、明るいタブとは明るさの差を小さく、両方の間に置く。
        const LIGHT_TABS: [Rgb; 2] = [rgb(0xff, 0xff, 0xff), rgb(0xcc, 0xcc, 0xcc)];
        const DARK_TABS: [Rgb; 2] = [rgb(0x1e, 0x1e, 0x1e), rgb(0x30, 0x30, 0x30)];
        for color in PALETTE.iter().chain([&NO_PROFILE_COLOR]) {
            assert!(
                contrast(*color, draw::DARK_INK) >= 7.0,
                "{color:?} and its ink"
            );
            for tab in LIGHT_TABS {
                assert!(contrast(*color, tab) >= 1.2, "{color:?} on {tab:?}");
            }
            for tab in DARK_TABS {
                assert!(contrast(*color, tab) >= 7.0, "{color:?} on {tab:?}");
            }
        }
    }

    #[test]
    fn the_palette_has_tinted_colors_without_warning_hues() {
        /// 灰色と見分けられる色味（チャンネルの最大と最小の差）の下限。
        const MIN_TINT: u8 = 0x10;
        for color in PALETTE {
            let channels = [color.r, color.g, color.b];
            let (Some(&max), Some(&min)) = (channels.iter().max(), channels.iter().min()) else {
                unreachable!("three channels");
            };
            assert!(max - min >= MIN_TINT, "{color:?}");
            // 赤・橙・黄は赤のチャンネルが青より強い。
            assert!(color.r < color.b, "{color:?}");
        }
    }

    #[test]
    fn the_palette_has_distinct_colors() {
        for (index, color) in PALETTE.iter().enumerate() {
            assert!(!PALETTE[..index].contains(color));
        }
    }

    #[test]
    fn a_name_always_gets_the_same_color() {
        // FNV-1a の値を固定する。変わると、利用者のタブの色が変わる。
        assert_eq!(hash(""), 0x811c_9dc5);
        assert_eq!(hash("a"), 0xe40c_292c);
        assert_eq!(hash("foobar"), 0xbf9c_f968);
        assert_eq!(color_of("foobar"), PALETTE[0xbf9c_f968_usize % 10]);
        assert_eq!(color_of("zsh"), color_of("zsh"));
    }

    #[test]
    fn names_spread_over_the_palette() {
        let names = [
            "PowerShell",
            "zsh",
            "Ubuntu",
            "Cygwin",
            "cmd",
            "Debian",
            "pwsh",
            "bash",
        ];
        let mut colors: Vec<Rgb> = names.iter().map(|name| color_of(name)).collect();
        colors.sort_by_key(|color| (color.r, color.g, color.b));
        colors.dedup();
        assert!(colors.len() >= 4, "{colors:?}");
    }

    #[test]
    fn the_initial_is_the_first_grapheme_cluster_in_upper_case() {
        assert_eq!(initial("zsh").as_deref(), Some("Z"));
        assert_eq!(initial("Ubuntu").as_deref(), Some("U"));
        assert_eq!(initial("  bash").as_deref(), Some("B"));
        // 全角の空白も飛ばす。
        assert_eq!(initial("\u{3000}端末").as_deref(), Some("端"));
        // 結合文字は前の文字から離さない。
        assert_eq!(initial("e\u{301}x").as_deref(), Some("E\u{301}"));
        // サロゲートペアになる文字を半分にしない。
        assert_eq!(initial("𠮷野家").as_deref(), Some("𠮷"));
        assert_eq!(initial("😀 emoji").as_deref(), Some("😀"));
        // 肌の色の修飾子と ZWJ の並びも一つ。
        assert_eq!(initial("👍🏽ok").as_deref(), Some("👍🏽"));
        // 大文字にすると文字の数が変わるものはそのまま。
        assert_eq!(initial("ßeta").as_deref(), Some("ß"));
        assert_eq!(initial(""), None);
        assert_eq!(initial("   "), None);
        // 幅のない文字だけなら頭文字はない。
        assert_eq!(initial("\u{7}"), None);
        assert_eq!(initial("\u{7}a").as_deref(), Some("A"));
    }

    #[test]
    fn profiles_decide_the_icon() {
        let profile = Profile {
            icon: Some(r"icons\zsh.png".to_owned()),
            ..Profile::new("zsh", "zsh -l", BackendKind::Cygwin)
        };
        let one = |profile: &Profile, config_dir| {
            TabIcon::for_profiles(std::slice::from_ref(profile), config_dir).remove(0)
        };
        let icon = one(&profile, Some(Path::new(r"C:\conf")));
        assert_eq!(icon.image, Some(PathBuf::from(r"C:\conf\icons\zsh.png")));
        assert_eq!(icon.backend, BackendKind::Cygwin);
        assert_eq!(icon.color, color_of("zsh"));
        // 絶対パスは設定ファイルのフォルダによらない。
        let absolute = Profile {
            icon: Some(r"D:\pics\a.png".to_owned()),
            ..profile.clone()
        };
        assert_eq!(
            one(&absolute, Some(Path::new(r"C:\conf"))).image,
            Some(PathBuf::from(r"D:\pics\a.png"))
        );
        assert_eq!(
            one(&profile, None).image,
            Some(PathBuf::from(r"icons\zsh.png"))
        );
        let plain = one(&Profile::builtin(), None);
        assert_eq!(plain.image, None);
        assert_eq!(plain.backend, BackendKind::Conpty);
    }

    #[test]
    fn the_same_picture_in_several_profiles_is_told_apart_by_the_initial() {
        let initials = |profiles: &[Profile]| -> Vec<Option<String>> {
            TabIcon::for_profiles_with(profiles, None, |_| None)
                .into_iter()
                .map(|icon| icon.initial)
                .collect()
        };
        let zsh = Profile::new("zsh", "zsh -l", BackendKind::Cygwin);
        let bash = Profile::new("bash", "bash -l", BackendKind::Cygwin);
        let ubuntu = Profile::new("Ubuntu", "", BackendKind::Wsl);
        // 一つだけなら絵だけ。
        assert_eq!(initials(std::slice::from_ref(&zsh)), [None], "one profile");
        // 接続方式が違えば絵が違う。
        assert_eq!(initials(&[zsh.clone(), ubuntu.clone()]), [None, None]);
        // 同じ接続方式なら両方に付け、ほかの接続方式には付けない。
        assert_eq!(
            initials(&[zsh.clone(), ubuntu, bash]),
            [Some("Z".to_owned()), None, Some("B".to_owned())]
        );
        // 頭文字にできるものがない名前には付けない。
        let blank = Profile::new("\u{3000}", "ash", BackendKind::Cygwin);
        assert_eq!(initials(&[zsh, blank]), [Some("Z".to_owned()), None]);
    }

    #[test]
    fn written_images_are_the_same_picture_only_for_the_same_file() {
        let with_image = |name, image: &str| Profile {
            icon: Some(image.to_owned()),
            ..Profile::new(name, "x", BackendKind::Conpty)
        };
        let shared = |profiles: &[Profile]| -> Vec<bool> {
            TabIcon::for_profiles_with(profiles, None, |_| None)
                .iter()
                .map(|icon| icon.initial.is_some())
                .collect()
        };
        assert_eq!(
            shared(&[
                with_image("a", r"C:\p\a.png"),
                with_image("b", r"C:\p\b.png")
            ]),
            [false, false]
        );
        assert_eq!(
            shared(&[
                with_image("a", r"C:\P\a.png"),
                with_image("b", "c:/p/A.PNG")
            ]),
            [true, true]
        );
        // 画像と自前の絵は別の絵。
        assert_eq!(
            shared(&[
                with_image("a", r"C:\p\a.png"),
                Profile::new("b", "x", BackendKind::Conpty)
            ]),
            [false, false]
        );
    }

    /// `cygwin_root` を書かなければ `C:\found` を見つけ、`has_icon` がアイコンありと答えるファイルと、PATH に
    /// ある `cmd.exe` と `wsl.exe` だけがある環境で、本物のアイコンを探す。
    fn real_in_fake_environment(profile: &Profile, icons: &[&str]) -> Option<RealIcon> {
        real_icon_with(
            profile,
            |configured| Some(PathBuf::from(configured.unwrap_or(r"C:\found"))),
            |program| match program {
                "cmd" | "cmd.exe" => Some(PathBuf::from(r"C:\w\cmd.exe")),
                "wsl.exe" => Some(PathBuf::from(r"C:\w\wsl.exe")),
                _ => None,
            },
            |path| icons.iter().any(|icon| path == Path::new(icon)),
        )
    }

    #[test]
    fn each_backend_reads_its_real_icon() {
        let icons = [
            r"C:\found\Cygwin.ico",
            r"C:\mine\Cygwin.ico",
            r"C:\w\cmd.exe",
            r"C:\w\wsl.exe",
        ];
        let cygwin = Profile::new("zsh", "zsh -l", BackendKind::Cygwin);
        assert_eq!(
            real_in_fake_environment(&cygwin, &icons),
            Some(RealIcon::File(PathBuf::from(r"C:\found\Cygwin.ico")))
        );
        let configured = Profile {
            cygwin_root: Some(r"C:\mine".to_owned()),
            ..cygwin.clone()
        };
        assert_eq!(
            real_in_fake_environment(&configured, &icons),
            Some(RealIcon::File(PathBuf::from(r"C:\mine\Cygwin.ico")))
        );
        // WSL は command によらず wsl.exe。
        let wsl = Profile::new("Ubuntu", "zsh -l", BackendKind::Wsl);
        assert_eq!(
            real_in_fake_environment(&wsl, &icons),
            Some(RealIcon::Executable(PathBuf::from(r"C:\w\wsl.exe")))
        );
        // ConPTY は command の最初の語。
        let conpty = Profile::new("cmd", "cmd /k echo", BackendKind::Conpty);
        assert_eq!(
            real_in_fake_environment(&conpty, &icons),
            Some(RealIcon::Executable(PathBuf::from(r"C:\w\cmd.exe")))
        );
    }

    #[test]
    fn profiles_without_a_real_icon_have_none() {
        let cygwin = Profile::new("zsh", "zsh -l", BackendKind::Cygwin);
        let wsl = Profile::new("Ubuntu", "", BackendKind::Wsl);
        let conpty = Profile::new("cmd", "cmd.exe", BackendKind::Conpty);
        // Cygwin.ico がない、wsl.exe とその実行ファイルにアイコンがない。
        for profile in [&cygwin, &wsl, &conpty] {
            assert_eq!(real_in_fake_environment(profile, &[]), None);
        }
        // Cygwin のフォルダが見つからない。
        assert_eq!(real_icon_with(&cygwin, |_| None, |_| None, |_| true), None);
        // PATH にない実行ファイル。
        let missing = Profile::new("a", "no-such-program -x", BackendKind::Conpty);
        assert_eq!(real_in_fake_environment(&missing, &[r"C:\w\cmd.exe"]), None);
    }

    #[test]
    fn real_icons_decide_which_pictures_are_the_same() {
        let initials = |profiles: &[Profile]| -> Vec<Option<String>> {
            let real_of = |profile: &Profile| match profile.command_program() {
                "cmd" => Some(RealIcon::Executable(PathBuf::from(
                    r"C:\Windows\System32\cmd.exe",
                ))),
                r"c:/windows/system32/CMD.EXE" => Some(RealIcon::Executable(PathBuf::from(
                    r"c:/windows/system32/CMD.EXE",
                ))),
                "pwsh" => Some(RealIcon::Executable(PathBuf::from(r"C:\p\pwsh.exe"))),
                _ => None,
            };
            TabIcon::for_profiles_with(profiles, None, real_of)
                .into_iter()
                .map(|icon| icon.initial)
                .collect()
        };
        let conpty = |name, command| Profile::new(name, command, BackendKind::Conpty);
        // 違う実行ファイルは違う絵。
        assert_eq!(
            initials(&[conpty("a", "cmd"), conpty("b", "pwsh")]),
            [None, None]
        );
        // 書き方が違っても、同じファイルなら同じ絵。
        assert_eq!(
            initials(&[
                conpty("a", "cmd"),
                conpty("b", r"c:/windows/system32/CMD.EXE")
            ]),
            [Some("A".to_owned()), Some("B".to_owned())]
        );
        // アイコンのない実行ファイルどうしは、自前の絵が同じ。本物のアイコンのものは別の絵。
        assert_eq!(
            initials(&[conpty("a", "foo"), conpty("b", "bar"), conpty("c", "cmd")]),
            [Some("A".to_owned()), Some("B".to_owned()), None]
        );
    }

    #[test]
    fn one_profile_gets_the_same_icon_as_in_the_whole_list() {
        use std::cell::Cell;

        // `C:\w\cmd.exe` と `C:\w\pwsh.exe` と Cygwin.ico にはアイコンがあり、`C:\w\bash.exe` にはない。
        let icons = [r"C:\w\cmd.exe", r"C:\w\pwsh.exe", r"C:\found\Cygwin.ico"];
        let checks = Cell::new(0);
        let has_icon = |path: &Path| {
            checks.set(checks.get() + 1);
            icons
                .iter()
                .any(|icon| path_key(path) == path_key(Path::new(icon)))
        };
        let candidate_of = |profile: &Profile| {
            real_icon_candidate_with(
                profile,
                |configured| Some(PathBuf::from(configured.unwrap_or(r"C:\found"))),
                |program| match program {
                    "cmd" | "cmd.exe" => Some(PathBuf::from(r"C:\w\cmd.exe")),
                    r"C:\W\CMD.EXE" => Some(PathBuf::from(r"C:\W\CMD.EXE")),
                    "pwsh" => Some(PathBuf::from(r"C:\w\pwsh.exe")),
                    "bash" | "sh" => Some(PathBuf::from(r"C:\w\bash.exe")),
                    _ => None,
                },
            )
        };
        let conpty = |name, command| Profile::new(name, command, BackendKind::Conpty);
        let with_image = |name, image: &str| Profile {
            icon: Some(image.to_owned()),
            ..conpty(name, "cmd")
        };
        let lists = [
            vec![conpty("a", "cmd")],
            vec![conpty("a", "cmd"), conpty("b", r"C:\W\CMD.EXE")],
            vec![conpty("a", "cmd"), conpty("b", "pwsh")],
            // アイコンのない実行ファイルと、見つからない実行ファイルは、どちらも ConPTY の絵。
            vec![
                conpty("a", "bash"),
                conpty("b", "missing"),
                conpty("c", "cmd"),
            ],
            vec![conpty("a", "bash"), conpty("b", "sh")],
            vec![
                with_image("a", r"C:\p\a.png"),
                with_image("b", r"c:/P/A.PNG"),
                conpty("c", "cmd"),
            ],
            vec![with_image("a", r"C:\p\a.png"), conpty("b", "cmd")],
            vec![
                Profile::new("zsh", "zsh -l", BackendKind::Cygwin),
                Profile::new("bash", "bash -l", BackendKind::Cygwin),
                Profile::new("Ubuntu", "", BackendKind::Wsl),
                Profile::new("Debian", "", BackendKind::Wsl),
            ],
        ];
        for profiles in &lists {
            let real_of =
                |profile: &Profile| candidate_of(profile).filter(|real| has_icon(real.path()));
            let whole = TabIcon::for_profiles_with(profiles, None, real_of);
            for (index, expected) in whole.iter().enumerate() {
                let one = TabIcon::for_profile_with(profiles, index, None, candidate_of, has_icon);
                assert_eq!(one, *expected, "{index} of {profiles:?}");
            }
        }
        // 画像のプロファイルのアイコンを決めるのに、ほかのプロファイルの実行ファイルは見ない。
        let profiles = [
            with_image("a", r"C:\p\a.png"),
            conpty("b", "cmd"),
            conpty("c", "pwsh"),
        ];
        checks.set(0);
        TabIcon::for_profile_with(&profiles, 0, None, candidate_of, has_icon);
        assert_eq!(checks.get(), 1);
    }

    #[test]
    fn programs_are_found_and_checked_for_icons() {
        let cmd = search_path("cmd").expect("cmd.exe is on the system path");
        assert_eq!(search_path("cmd.exe").as_ref(), Some(&cmd));
        assert!(has_icon(&cmd));
        assert_eq!(search_path("no-such-program-for-fantermo"), None);
        assert_eq!(search_path(""), None);
        // フォルダを書いたものは探さずにそのパス。拡張子がなければ `.exe` を足す。
        assert_eq!(
            search_path(r"C:\some\tool"),
            Some(PathBuf::from(r"C:\some\tool.exe"))
        );
        assert_eq!(
            search_path(r"C:\some\tool.bat"),
            Some(PathBuf::from(r"C:\some\tool.bat"))
        );
        assert!(!has_icon(Path::new(r"C:\no-such-folder\no-such.exe")));
    }

    #[test]
    fn shared_pictures_looks_at_the_whole_list() {
        let drawn = Picture::Drawn(BackendKind::Wsl);
        let image = Picture::Image("a".to_owned());
        assert_eq!(shared_pictures(&[]), [false; 0]);
        assert_eq!(shared_pictures(std::slice::from_ref(&drawn)), [false]);
        assert_eq!(
            shared_pictures(&[drawn.clone(), image.clone(), drawn.clone()]),
            [true, false, true]
        );
        assert_eq!(
            shared_pictures(&[image.clone(), image, drawn]),
            [true, true, false]
        );
    }

    #[test]
    fn tabs_without_a_profile_have_no_initial() {
        let icon = TabIcon::without_profile();
        assert_eq!(icon.backend, BackendKind::Conpty);
        assert_eq!(icon.initial, None);
        assert_eq!(icon.image, None);
        assert!(!PALETTE.contains(&icon.color));
    }

    #[test]
    fn pixels_are_made_once_per_icon_side_and_family() {
        let wsl = TabIcon {
            backend: BackendKind::Wsl,
            ..TabIcon::without_profile()
        };
        let plain = TabIcon::without_profile();
        /// `icons` で `pixels` を揃え、作ったアイコンの接続方式と一辺と書体を `made` に足す。
        fn update(
            pixels: &mut Pixels,
            made: &mut Vec<(BackendKind, u32, String)>,
            icons: &[&TabIcon],
            side: u32,
            family: &str,
        ) {
            pixels.update(icons.iter().copied(), side, family, |icon| {
                made.push((icon.backend, side, family.to_owned()));
                Some(Decoded::blank(side, side))
            });
        }
        let mut pixels = Pixels::default();
        let mut made = Vec::new();
        // 同じアイコンのタブが複数あっても一度だけ作る。
        update(
            &mut pixels,
            &mut made,
            &[&plain, &plain, &wsl],
            18,
            "Segoe UI",
        );
        assert_eq!(made.len(), 2);
        // 持っているものは作り直さない。
        update(&mut pixels, &mut made, &[&plain, &wsl], 18, "Segoe UI");
        assert_eq!(made.len(), 2);
        // 閉じたタブのアイコンは捨てる。
        update(&mut pixels, &mut made, &[&plain], 18, "Segoe UI");
        assert_eq!(pixels.iter().count(), 1);
        assert_eq!(made.len(), 2);
        // 一辺が変わったら作り直す。
        update(&mut pixels, &mut made, &[&plain], 27, "Segoe UI");
        assert_eq!(
            made.last(),
            Some(&(BackendKind::Conpty, 27, "Segoe UI".to_owned()))
        );
        assert!(pixels.iter().all(|(_, decoded)| decoded.width() == 27));
        // 書体が変わったら作り直す。
        update(&mut pixels, &mut made, &[&plain], 27, "Yu Gothic UI");
        assert_eq!(made.len(), 4);
    }

    #[test]
    fn pixels_that_cannot_be_made_are_not_retried() {
        let icon = TabIcon::without_profile();
        let mut pixels = Pixels::default();
        let mut attempts = 0;
        for _ in 0..2 {
            pixels.update([&icon], 18, "Segoe UI", |_| {
                attempts += 1;
                None
            });
        }
        assert_eq!(attempts, 1);
        assert_eq!(pixels.iter().count(), 0);
    }
}
