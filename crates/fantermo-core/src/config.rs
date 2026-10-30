//! 設定ファイル（TOML）の型、既定値、解釈。
//!
//! 項目ごとに読む。書いていない項目は既定値にする。型の合わない値や範囲外の値はその項目だけを前の
//! 値のまま（起動したときは既定値）にし、知らないキーは無視する。どちらも、エラーのある行を添えた
//! 警告として返す。TOML として読めないときは、すべてを前の値のままにする。
//!
//! 書ける項目と既定値は [`TEMPLATE`] にある。古い書き方は、解釈する前に [`migrate`] で今の書き方に
//! 読み替える（[`parse`] は今の書き方だけを読む）。

pub mod migrate;

use std::{
    collections::HashMap,
    env,
    ffi::OsStr,
    ops::RangeInclusive,
    path::{Path, PathBuf},
};

use serde::{Deserialize, de::DeserializeOwned};
use toml::{
    Table, Value,
    de::{DeTable, DeValue},
};

use crate::{
    frame::Rgb,
    keymap::{self, KeyCombo, Keymap},
};

/// 既定のフォント。先頭から探す。
pub const DEFAULT_FONT_FAMILIES: [&str; 3] = ["Bizin Gothic", "BIZ UDGothic", "Consolas"];

/// 既定のフォントの大きさ（ポイント）。
pub const DEFAULT_FONT_SIZE_PT: f32 = 12.0;

/// フォントの大きさとして受け付ける範囲（ポイント）。
pub const FONT_SIZE_RANGE_PT: RangeInclusive<f32> = 1.0..=128.0;

/// 既定のスクロールバーの幅（96 DPI でのピクセル）。
pub const DEFAULT_SCROLL_BAR_WIDTH: u32 = 12;

/// スクロールバーの幅として受け付ける範囲（96 DPI でのピクセル）。
pub const SCROLL_BAR_WIDTH_RANGE: RangeInclusive<u32> = 4..=64;

/// 端末の画面の上・下・左・右の既定の余白（96 DPI でのピクセル）。
pub const DEFAULT_PADDING: u32 = 4;

/// 余白として受け付ける範囲（96 DPI でのピクセル）。
pub const PADDING_RANGE: RangeInclusive<u32> = 0..=64;

/// 背景画像の既定の不透明度。
pub const DEFAULT_IMAGE_OPACITY: f32 = 0.2;

/// 背景画像の不透明度として受け付ける範囲。
pub const IMAGE_OPACITY_RANGE: RangeInclusive<f32> = 0.0..=1.0;

/// プロファイルを書いていないときの、既定のプロファイルの名前。
pub const DEFAULT_PROFILE_NAME: &str = "PowerShell";

/// プロファイルを書いていないときに起動するコマンドライン。
pub const DEFAULT_COMMAND_LINE: &str = "powershell.exe -NoLogo";

/// 設定ファイルが無いときに作る中身。どの項目も既定値のままコメントにして並べ、利用者が `#` を消して
/// 書き換えられるようにする。プロファイルのテーブルは例として載せる。
pub const TEMPLATE: &str = r##"# fantermo の設定ファイル。
# 行頭の「#」を消して値を書き換え、保存すると読み直す。書いていない項目は既定値になる。

# 新しいタブで起動するプロファイル。[[profiles]] のテーブルを並べ、name に名前、command に起動するコマンドラインを
# 書く。backend はシェルとのつなぎ方で、"conpty"（Windows の疑似コンソール）、"cygwin"（Cygwin のブリッジ）、
# "wsl"（WSL のブリッジ）を書ける。書かなければ "conpty"。"cygwin" と "wsl" の command は Cygwin・WSL の側の
# コマンドラインで、書かなければログインシェルを起動する。"cygwin" では cygwin_root に Cygwin のフォルダを書ける
# （書かなければ環境変数 PATH と、よくある場所（C:\cygwin64、C:\cygwin）から探す）。"wsl" では distribution に
# ディストリビューションの名前を書ける（書かなければ既定のディストリビューション）。
# Cygwin と WSL はブリッジでつなぐことを勧める。ConPTY を通すと、アプリの出力の一部が書き換えられたり順番が
# 変わったりし、端末への問い合わせの一部に ConPTY が代わりに答えるか届かず、kitty graphics protocol と sixel の
# 画像も届かず、ウィンドウの大きさを変えるとスクロールバックの行が欠けることがある。
# タブバーの「+」は既定のプロファイルを開き、その右の「▾」か「+」の右クリックで一覧から選んで開く。fantermo を
# 引数つきで起動したときは、「+」とキーとメニューの「新しいタブ」も引数のコマンドラインを使う。
# 書かなければ PowerShell（powershell.exe -NoLogo）を起動する。書き換えたら、次に開くタブから使う。
# プロファイルを書かずに、最上位に shell = 'コマンドライン' と一つだけ書いてもよい（名前は実行ファイルの名前）。
# Cygwin の bin の下の実行ファイルと wsl.exe は、ブリッジでつなぐプロファイルにする。
# default_profile は「+」で開くプロファイルの名前。書かなければ先頭のプロファイル。[[profiles]] より前に書く。
# default_profile = "PowerShell"

# 例: [[profiles]]
# 例: name = "PowerShell"
# 例: command = "powershell.exe -NoLogo"
#
# 例: [[profiles]]
# 例: name = "zsh"
# 例: backend = "cygwin"
# 例: command = "zsh -l"
#
# 例: [[profiles]]
# 例: name = "Ubuntu"
# 例: backend = "wsl"
# 例: distribution = "Ubuntu"

[font]
# フォントの名前。先頭から探し、最初に見つかったものを使う。名前一つだけでもよい。
# family = ["Bizin Gothic", "BIZ UDGothic", "Consolas"]

# 大きさ（ポイント）。1 から 128 まで。小数でもよい。
# size = 12

# グリフがセルより広いとき（曖昧幅の ○ や ※ が全角の幅でデザインされているときなど）の描き方。
# セルの数は変えない。"shrink" は横に縮める、"clip" は縮めずにセルの左に置いてはみ出しを切る、
# "overflow" は縮めずに右のセルへはみ出して描く、"overflow_center" は縮めずにセルの真ん中に置いて
# 左右のセルへ同じだけはみ出して描く。
# wide_glyph = "shrink"

[paste]
# 貼り付ける前に、内容を編集できるダイアログで確かめるとき。
# "unsafe" は改行を含む文字列がコマンドとして実行されうるとき（アプリが bracketed paste を有効にしていない
# ときなど）、"multiline" は bracketed paste に関わらず改行を含むとき、"never" は確かめずに改行ごと送る。
# confirm = "unsafe"

[scroll_bar]
# 端末の右のスクロールバーの幅（96 DPI でのピクセル。画面の拡大率に合わせて広げる）。4 から 64 まで。
# 書き換えたら、端末の桁数と行数を保ったままウィンドウの幅を変える。
# width = 12

[window]
# 端末の画面の上・下・左・右に空ける余白（96 DPI でのピクセル。画面の拡大率に合わせて広げる）。0 から 64 まで。
# 余白は端末の背景色で塗る。書き換えたら、端末の桁数と行数を保ったままウィンドウの大きさを変える。
# padding = 4

[colors]
# 端末の配色。"dark" は黒の背景に白の文字、"light" は白の背景に濃い灰色の文字。背景色・文字の色・16 色を
# まとめて変える。カーソルは文字の色で描き、選択範囲は文字の色と背景色を入れ替えて描く。
# scheme = "dark"

# 端末の背景色と文字の色。"#rrggbb" の形で書く。書かない色は配色の色（下の値は "dark" の色）。
# background = "#000000"
# foreground = "#ffffff"

[colors.tab_bar]
# タブバーの色。"#rrggbb" の形で書く。書かない色は Windows のシステムの色から決める（下の値は Windows 11
# の既定の色のときのもの）。bar は背景、active_tab は表示しているタブ、active_border はその左・右・上の
# 縁の線、active_title はその文字と閉じるボタンの「×」、inactive_title は表示していないタブの文字と「×」と
# マウスを載せている「+」「▾」「≡」、separator は表示していないタブの間の区切りの線、hovered_tab は
# マウスを載せている表示していないタブ、idle_mark はマウスを載せていない「+」「▾」「≡」、bell はベルが
# 鳴ったタブの印。
# bar = "#cccccc"
# active_tab = "#ffffff"
# active_border = "#a0a0a0"
# active_title = "#000000"
# inactive_title = "#000000"
# separator = "#a0a0a0"
# hovered_tab = "#e6e6e6"
# idle_mark = "#6d6d6d"
# bell = "#f0b030"

[colors.scroll_bar]
# スクロールバーの色。"#rrggbb" の形で書く。書かない色は端末の背景色と文字の色から決める（下の値は配色が
# "dark" の黒の背景と白の文字のときのもの）。track はマウスを載せている間とドラッグしている間のトラック、thumb は
# つまみ、hovered_thumb はマウスを載せているつまみ、dragged_thumb はドラッグしているつまみ。
# track = "#0f0f0f"
# thumb = "#595959"
# hovered_thumb = "#808080"
# dragged_thumb = "#a6a6a6"

[background]
# 背景画像の不透明度。0 から 1 まで。1 は画像をそのまま見せ、0 は画像を消して端末の背景色だけにする。間の値は、
# 画像の上に端末の背景色を重ねて薄める。アプリが背景色を付けた文字、反転した文字、選択範囲は、その色で
# 塗りつぶす。
# image_opacity = 0.2

# 背景画像の表示方法。"fill"（ページ幅に合わせる）は縦横比を保って端末を覆い、はみ出しを切る、"fit"（画面の
# サイズに合わせる）は縦横比を保って端末に収め、余りは背景色にする、"stretch"（拡大して表示）は端末に合わせて
# 引き伸ばす、"tile"（並べて表示）は元の大きさで敷き詰める、"center"（中央に表示）は元の大きさで真ん中に置く。
# fit = "fill"

# 端末の背景に敷く画像の一覧。[[background.images]] のテーブルを画像の項目ごとに並べ、path に画像のファイル、
# フォルダ（中の画像から選ぶ。下のフォルダは見ない）、ファイルの名前に「*」「?」を使ったワイルドカードを書く。
# 新しいタブを開くたびに、この中からランダムに一枚選ぶ。フォルダとワイルドカードから選ぶのは png、jpg、jpeg、
# bmp、gif（最初のコマ）、webp（Windows で読めるとき）。読めない画像は飛ばして別の画像を選ぶ。相対パスはこの
# ファイルのフォルダから。書かなければ画像を敷かない。
# テーブルに fit と image_opacity を書くと、その項目（フォルダやワイルドカードなら、そこから選んだ画像）にだけ
# 効く。書かなかったものは上の値に従う。[[background.images]] は [background] の値より後ろに書く。
# 例: [[background.images]]
# 例: path = 'C:\画像\壁紙'
# 例: [[background.images]]
# 例: path = 'C:\画像\*.jpg'
# 例: [[background.images]]
# 例: path = 'C:\画像\模様.png'
# 例: fit = "tile"

[keys]
# キーの組み合わせと、押したときの動作。書いた組み合わせだけ、下に並べた既定の割り当てを変える。
# 組み合わせは、修飾キー（ctrl、shift、alt、win）とキーを「+」でつなぐ。キーの名前はキーボードの上の
# 位置の名前で、a〜z、digit_0〜digit_9、f1〜f24、tab、enter、space、escape、backspace、insert、delete、
# home、end、page_up、page_down、arrow_up、arrow_down、arrow_left、arrow_right、context_menu、
# numpad_0〜numpad_9 など。
# 動作は copy、paste、context_menu、new_tab、close_tab、next_tab、prev_tab。"none" は割り当てを外し、
# キーをシェルへ送る。同じ動作に複数の組み合わせを割り当ててもよい。
# 例: Ctrl+Tab をシェルへ送り、Ctrl+PageDown で次のタブにするなら、次の 2 行を書く。
# 例: "ctrl+tab" = "none"
# 例: "ctrl+page_down" = "next_tab"
# 既定の割り当て:
# "ctrl+c" = "copy"
# "ctrl+insert" = "copy"
# "shift+insert" = "paste"
# "context_menu" = "context_menu"
# "shift+f10" = "context_menu"
# "ctrl+shift+t" = "new_tab"
# "ctrl+shift+w" = "close_tab"
# "ctrl+tab" = "next_tab"
# "ctrl+shift+tab" = "prev_tab"
"##;

/// 設定の全体。
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub font: FontConfig,
    /// 新しいタブで起動するプロファイル。書いた順。少なくとも一つある（書いていなければ、最上位の
    /// `shell` から作った一つか、既定のプロファイル）。名前はどれも違う。
    pub profiles: Vec<Profile>,
    /// 既定のプロファイルの名前。`profiles` のどれかの名前で、`None` なら先頭のプロファイル。
    pub default_profile: Option<String>,
    pub paste: PasteConfig,
    pub scroll_bar: ScrollBarConfig,
    pub window: WindowConfig,
    pub colors: ColorsConfig,
    pub background: BackgroundConfig,
    /// キーの割り当て。`[keys]` に書いた組み合わせを、既定の割り当てに重ねたもの。
    pub keys: Keymap,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font: FontConfig::default(),
            profiles: vec![Profile::builtin()],
            default_profile: None,
            paste: PasteConfig::default(),
            scroll_bar: ScrollBarConfig::default(),
            window: WindowConfig::default(),
            colors: ColorsConfig::default(),
            background: BackgroundConfig::default(),
            keys: Keymap::default(),
        }
    }
}

impl Config {
    /// 既定のプロファイルの、`profiles` の中の位置。
    pub fn default_profile_index(&self) -> usize {
        default_profile_index(&self.profiles, self.default_profile.as_deref())
    }
}

/// プロファイルの並び `profiles` のうち、名前が `default` のものの位置。`None` か、その名前のものが
/// なければ先頭。
pub fn default_profile_index(profiles: &[Profile], default: Option<&str>) -> usize {
    default
        .and_then(|name| profiles.iter().position(|profile| profile.name == name))
        .unwrap_or(0)
}

/// 新しいタブで起動するもの（`[[profiles]]` のテーブル一つ）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    /// タブバーの一覧に見せる名前。空ではない。
    pub name: String,
    /// 起動するコマンドライン。ConPTY では空ではない。ブリッジ（`cygwin`・`wsl`）では Cygwin・WSL の
    /// 側のコマンドラインで、空なら相手のログインシェル。
    pub command: String,
    pub backend: BackendKind,
    /// `backend = "cygwin"` の Cygwin のフォルダ。`None` なら [`find_cygwin_root`] で探す。ほかの種類では
    /// `None`。
    pub cygwin_root: Option<String>,
    /// `backend = "wsl"` のディストリビューションの名前。`None` なら既定のディストリビューション。
    /// ほかの種類では `None`。
    pub distribution: Option<String>,
}

impl Profile {
    /// 名前が `name`、コマンドラインが `command` の、`backend` でつなぐプロファイル。
    pub fn new(name: &str, command: &str, backend: BackendKind) -> Self {
        Self {
            name: name.to_owned(),
            command: command.to_owned(),
            backend,
            cygwin_root: None,
            distribution: None,
        }
    }

    /// プロファイルも `shell` も書いていないときのプロファイル。
    pub fn builtin() -> Self {
        Self::new(
            DEFAULT_PROFILE_NAME,
            DEFAULT_COMMAND_LINE,
            BackendKind::default(),
        )
    }

    /// 最上位の `shell` に書いたコマンドライン `command` のプロファイル。名前は実行ファイルの名前。
    /// つなぎ方は [`Profile::from_shell_with`] で決め、Cygwin のフォルダかどうかは [`is_cygwin_root`]
    /// で見る。
    pub fn from_shell(command: &str) -> Self {
        Self::from_shell_with(command, is_cygwin_root)
    }

    /// 最上位の `shell` に書いたコマンドライン `command` のプロファイル。`is_cygwin_root` はフォルダが
    /// Cygwin のフォルダか。
    ///
    /// - 実行ファイルが絶対パスで、そのフォルダの一つ上が Cygwin のフォルダなら（`<Cygwin>\bin\zsh.exe`
    ///   など）、`backend = "cygwin"`。`cygwin_root` はそのフォルダ、`command` は実行ファイルを Cygwin の
    ///   パス（`/bin/zsh`）にして引数を続けたもの。
    /// - 実行ファイルが `wsl.exe` で、引数が `-d`（`--distribution`）とその名前、`-e`（`--exec`）か `--`
    ///   に続くコマンド、オプションでない最初の語から始まるコマンドだけなら、`backend = "wsl"`。
    ///   `distribution` は `-d` の名前、`command` はコマンドの部分（なければ空でログインシェル）。
    /// - どちらでもなければ ConPTY で、コマンドラインをそのまま使う。
    pub fn from_shell_with(command: &str, is_cygwin_root: impl Fn(&Path) -> bool) -> Self {
        let name = program_name(command);
        let (program, rest) = split_program(command);
        if let Some((root, cygwin_command)) = cygwin_shell(program, rest, is_cygwin_root) {
            return Self {
                cygwin_root: Some(root),
                ..Self::new(&name, &cygwin_command, BackendKind::Cygwin)
            };
        }
        if file_stem(program).eq_ignore_ascii_case(WSL_PROGRAM)
            && let Some((distribution, wsl_command)) = wsl_arguments(rest)
        {
            return Self {
                distribution,
                ..Self::new(&name, &wsl_command, BackendKind::Wsl)
            };
        }
        Self::new(&name, command, BackendKind::default())
    }

    /// ブリッジのプロファイルの `command` が Windows のプログラムを起動するなら、そのファイルの名前
    /// （フォルダを除き、書いたままの形）。相手の側から起動した Windows のプログラムは ConPTY を通るので、
    /// ブリッジで届くもの（画像など）が届かない。
    ///
    /// WSL では `wsl` と `.exe` で終わるものすべて。Cygwin ではプログラムの実行ファイルも `.exe` で
    /// 終わるので、[`CYGWIN_KNOWN_WINDOWS_PROGRAMS`] の名前だけ。最初の語だけを見る。
    pub fn windows_program_in_bridge(&self) -> Option<&str> {
        let program = file_name(bridge_program(&self.command));
        let stem = file_stem(program);
        let is_windows_program = match self.backend {
            BackendKind::Conpty => false,
            BackendKind::Wsl => stem.eq_ignore_ascii_case(WSL_PROGRAM) || stem != program,
            BackendKind::Cygwin => CYGWIN_KNOWN_WINDOWS_PROGRAMS
                .iter()
                .any(|known| stem.eq_ignore_ascii_case(known)),
        };
        is_windows_program.then_some(program)
    }
}

/// Cygwin のブリッジの `command` で、Windows のプログラムと分かる実行ファイルの名前（フォルダと `.exe`
/// を除く）。
const CYGWIN_KNOWN_WINDOWS_PROGRAMS: [&str; 4] = [WSL_PROGRAM, "cmd", "powershell", "pwsh"];

/// Cygwin・WSL の側のコマンドライン `command` の実行ファイル（`'` か `"` で囲んでいれば中身）。
fn bridge_program(command: &str) -> &str {
    let command = command.trim();
    match command.strip_prefix('\'') {
        Some(quoted) => quoted
            .split_once('\'')
            .map_or(quoted, |(program, _)| program),
        None => split_program(command).0,
    }
}

/// Cygwin のフォルダの `bin` にある DLL。
const CYGWIN_DLL: &str = "cygwin1.dll";
/// Cygwin のフォルダの `bin` にあるシェル。`cygwin1.dll` を同梱しているだけのアプリのフォルダと見分ける。
const CYGWIN_SHELL: &str = "sh.exe";
/// Cygwin のフォルダの下の、実行ファイルを置くフォルダの名前。
const CYGWIN_BIN: &str = "bin";

/// 環境変数 `PATH` になければ、この順に探す Cygwin のフォルダ。
pub const COMMON_CYGWIN_ROOTS: [&str; 2] = [r"C:\cygwin64", r"C:\cygwin"];

/// フォルダ `root` が Cygwin のフォルダだとしたときの、`cygwin1.dll` のパス。
pub fn cygwin_dll_path(root: &Path) -> PathBuf {
    root.join("bin").join(CYGWIN_DLL)
}

/// フォルダ `root` が Cygwin のフォルダか（`bin` に `cygwin1.dll` があるか）。
pub fn is_cygwin_root(root: &Path) -> bool {
    cygwin_dll_path(root).is_file()
}

/// `cygwin_root` を書いていないときに使う Cygwin のフォルダ。[`find_cygwin_root_with`] で、この
/// プロセスの環境変数 `PATH` と実際のファイルを見て探す。
pub fn find_cygwin_root() -> Option<PathBuf> {
    find_cygwin_root_with(env::var_os("PATH").as_deref(), Path::is_file)
}

/// 環境変数 `PATH` の値が `path_variable` のときの Cygwin のフォルダ。`is_file` はそのパスのファイルが
/// あるか。
///
/// `PATH` の絶対パスの項目を順に見て、フォルダの名前が `bin` で `cygwin1.dll` と `sh.exe` があるものの
/// 一つ上を返す。なければ [`COMMON_CYGWIN_ROOTS`] の順に、`bin` に `cygwin1.dll` があるもの。
pub fn find_cygwin_root_with(
    path_variable: Option<&OsStr>,
    is_file: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    path_variable
        .into_iter()
        .flat_map(env::split_paths)
        .find_map(|folder| cygwin_root_of_bin(&folder, &is_file))
        .or_else(|| {
            COMMON_CYGWIN_ROOTS
                .iter()
                .map(PathBuf::from)
                .find(|root| is_file(&cygwin_dll_path(root)))
        })
}

/// フォルダ `folder` が Cygwin の `bin` なら、Cygwin のフォルダ。
fn cygwin_root_of_bin(folder: &Path, is_file: &impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let is_bin = folder
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(CYGWIN_BIN));
    if !folder.is_absolute()
        || !is_bin
        || !is_file(&folder.join(CYGWIN_DLL))
        || !is_file(&folder.join(CYGWIN_SHELL))
    {
        return None;
    }
    folder.parent().map(Path::to_path_buf)
}

/// WSL を起動する実行ファイルの名前（フォルダと `.exe` を除く）。
const WSL_PROGRAM: &str = "wsl";

/// コマンドライン `command` の実行ファイルの名前（フォルダと `.exe` を除く）。読めなければ `shell`。
fn program_name(command: &str) -> String {
    let stem = file_stem(split_program(command).0);
    if stem.is_empty() {
        "shell".to_owned()
    } else {
        stem.to_owned()
    }
}

/// コマンドライン `command` を、実行ファイル（`"` で囲んでいれば中身）と、そのあとの引数（前後の空白を
/// 除いたもの）に分ける。
fn split_program(command: &str) -> (&str, &str) {
    let command = command.trim();
    match command.strip_prefix('"') {
        Some(quoted) => match quoted.split_once('"') {
            Some((program, rest)) => (program, rest.trim()),
            None => (quoted, ""),
        },
        None => match command.split_once(char::is_whitespace) {
            Some((program, rest)) => (program, rest.trim()),
            None => (command, ""),
        },
    }
}

/// パス `program` のファイルの名前。
fn file_name(program: &str) -> &str {
    program.rsplit(['\\', '/']).next().unwrap_or(program)
}

/// パス `program` のファイルの名前から `.exe` を除いたもの。
fn file_stem(program: &str) -> &str {
    const EXE: &str = ".exe";
    let file = file_name(program);
    match file.len().checked_sub(EXE.len()) {
        Some(end) if file.is_char_boundary(end) && file[end..].eq_ignore_ascii_case(EXE) => {
            &file[..end]
        }
        _ => file,
    }
}

/// 実行ファイル `program` が Cygwin のフォルダの下のものなら、そのフォルダと、Cygwin の側のコマンド
/// ライン（実行ファイルを Cygwin のパスにして引数 `arguments` を続けたもの）。
fn cygwin_shell(
    program: &str,
    arguments: &str,
    is_cygwin_root: impl Fn(&Path) -> bool,
) -> Option<(String, String)> {
    let path = Path::new(program);
    if !path.is_absolute() {
        return None;
    }
    let folder = path.parent()?;
    let root = folder.parent()?;
    let folder_name = folder.file_name()?.to_str()?;
    let stem = file_stem(program);
    if stem.is_empty() || !is_cygwin_root(root) {
        return None;
    }
    let mut command = format!("/{folder_name}/{stem}");
    if !arguments.is_empty() {
        command.push(' ');
        command.push_str(arguments);
    }
    Some((root.to_str()?.to_owned(), command))
}

/// `wsl.exe` の引数 `arguments` を、ディストリビューションの名前と WSL の側のコマンドラインに読む。
/// 読めない引数（ほかのオプション）があれば `None`。
fn wsl_arguments(arguments: &str) -> Option<(Option<String>, String)> {
    let mut distribution = None;
    let mut words = command_words(arguments).into_iter();
    while let Some((start, end)) = words.next() {
        let word = &arguments[start..end];
        match word {
            "-d" | "--distribution" => {
                let (start, end) = words.next()?;
                distribution = Some(arguments[start..end].replace('"', ""));
            }
            "-e" | "--exec" => {
                let command = arguments[end..].trim();
                return (!command.is_empty()).then(|| (distribution, command.to_owned()));
            }
            "--" => return Some((distribution, arguments[end..].trim().to_owned())),
            _ if word.starts_with('-') => return None,
            _ => return Some((distribution, arguments[start..].trim().to_owned())),
        }
    }
    Some((distribution, String::new()))
}

/// コマンドライン `text` の語の、始まりと終わりの位置。語は空白で区切り、`"` で囲んだところの空白は
/// 区切りにしない。
fn command_words(text: &str) -> Vec<(usize, usize)> {
    let mut words = Vec::new();
    let mut start = None;
    let mut quoted = false;
    for (index, c) in text.char_indices() {
        if c.is_whitespace() && !quoted {
            if let Some(start) = start.take() {
                words.push((start, index));
            }
            continue;
        }
        if c == '"' {
            quoted = !quoted;
        }
        start.get_or_insert(index);
    }
    if let Some(start) = start {
        words.push((start, text.len()));
    }
    words
}

/// プロファイルのシェルとのつなぎ方（`profiles[].backend`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BackendKind {
    /// Windows の疑似コンソール（ConPTY）。
    #[default]
    Conpty,
    /// Cygwin のブリッジ。
    Cygwin,
    /// WSL のブリッジ。
    Wsl,
}

impl BackendKind {
    /// 書ける値のすべて。
    pub const ALL: [BackendKind; 3] = [BackendKind::Conpty, BackendKind::Cygwin, BackendKind::Wsl];

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        match self {
            BackendKind::Conpty => "conpty",
            BackendKind::Cygwin => "cygwin",
            BackendKind::Wsl => "wsl",
        }
    }

    /// ブリッジでつなぐ。`command` を相手の側のコマンドラインとして渡し、空なら相手のログインシェル。
    pub fn is_bridge(self) -> bool {
        self != BackendKind::Conpty
    }

    /// 設定ファイルに書く名前 `name` の値。
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// `[background]` の設定。
#[derive(Clone, Debug, PartialEq)]
pub struct BackgroundConfig {
    /// 画像のファイル・フォルダ・ワイルドカードの一覧（書いた順）。空なら画像を敷かない。
    pub images: Vec<BackgroundImage>,
    /// 画像の不透明度（1 で画像をそのまま見せ、0 で消す）。
    pub image_opacity: f32,
    pub fit: BackgroundFit,
}

impl BackgroundConfig {
    /// `images` の `entry` 番目の項目から選んだ画像の表示方法と不透明度。項目に書いていなければ（項目が
    /// なければ）、全体の値。
    pub fn settings_for(&self, entry: usize) -> (BackgroundFit, f32) {
        let image = self.images.get(entry);
        (
            image.and_then(|image| image.fit).unwrap_or(self.fit),
            image
                .and_then(|image| image.image_opacity)
                .unwrap_or(self.image_opacity),
        )
    }

    /// `other` と、一覧の項目のパスが同じ順に並んでいる（項目ごとの表示方法と不透明度は比べない）。
    pub fn same_paths(&self, other: &BackgroundConfig) -> bool {
        self.images
            .iter()
            .map(|image| &image.path)
            .eq(other.images.iter().map(|image| &image.path))
    }
}

/// `background.images` の一つの項目。
#[derive(Clone, Debug, PartialEq)]
pub struct BackgroundImage {
    /// 画像のファイル・フォルダ・ワイルドカード（書いたまま）。
    pub path: String,
    /// この項目から選んだ画像の表示方法。`None` なら `[background]` の `fit`。
    pub fit: Option<BackgroundFit>,
    /// この項目から選んだ画像の不透明度。`None` なら `[background]` の `image_opacity`。
    pub image_opacity: Option<f32>,
}

impl BackgroundImage {
    /// 表示方法と不透明度を全体の値に任せる項目。
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            fit: None,
            image_opacity: None,
        }
    }
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            images: Vec::new(),
            image_opacity: DEFAULT_IMAGE_OPACITY,
            fit: BackgroundFit::default(),
        }
    }
}

/// 背景画像の表示方法（`background.fit`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundFit {
    /// 縦横比を保って範囲を覆い、はみ出しを切る。
    #[default]
    Fill,
    /// 縦横比を保って範囲に収める。余りは背景色。
    Fit,
    /// 範囲に合わせて引き伸ばす。
    Stretch,
    /// 元の大きさで、範囲の左上から敷き詰める。
    Tile,
    /// 元の大きさで真ん中に置く。はみ出しは切る。
    Center,
}

impl BackgroundFit {
    /// 書ける値のすべて。設定ファイルの説明と同じ順。
    pub const ALL: [BackgroundFit; 5] = [
        BackgroundFit::Fill,
        BackgroundFit::Fit,
        BackgroundFit::Stretch,
        BackgroundFit::Tile,
        BackgroundFit::Center,
    ];

    /// 設定ファイルに書く名前 `name` の値。
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|fit| fit.name() == name)
    }

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        match self {
            BackgroundFit::Fill => "fill",
            BackgroundFit::Fit => "fit",
            BackgroundFit::Stretch => "stretch",
            BackgroundFit::Tile => "tile",
            BackgroundFit::Center => "center",
        }
    }
}

/// 色を書くテーブル（`[colors]` と `[colors.<名前>]`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorTable {
    /// 端末の色（`[colors]` に書く）。
    Terminal,
    /// タブバーの色。
    TabBar,
    /// スクロールバーの色。
    ScrollBar,
}

impl ColorTable {
    /// すべてのテーブル。設定ファイルの順。
    pub const ALL: [ColorTable; 3] = [
        ColorTable::Terminal,
        ColorTable::TabBar,
        ColorTable::ScrollBar,
    ];

    /// テーブルのキーのパス（`["colors", "tab_bar"]` など）。
    pub fn path(self) -> &'static [&'static str] {
        match self {
            ColorTable::Terminal => &["colors"],
            ColorTable::TabBar => &["colors", "tab_bar"],
            ColorTable::ScrollBar => &["colors", "scroll_bar"],
        }
    }

    /// テーブルに書ける色のキー。設定ファイルの順。
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            ColorTable::Terminal => &TERMINAL_COLOR_KEYS,
            ColorTable::TabBar => &TAB_BAR_COLOR_KEYS,
            ColorTable::ScrollBar => &SCROLL_BAR_COLOR_KEYS,
        }
    }
}

/// `[colors]` に書ける端末の色のキー。
pub const TERMINAL_COLOR_KEYS: [&str; 2] = ["background", "foreground"];

/// 端末の配色（`colors.scheme`）。背景色・文字の色・16 色をまとめて決める。カーソルは文字の色で
/// 描き、選択範囲は文字の色と背景色を入れ替えて描くので、どちらも配色に従う。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorScheme {
    /// 黒の背景に白の文字。
    #[default]
    Dark,
    /// 白の背景に濃い灰色の文字。
    Light,
}

impl ColorScheme {
    /// 書ける値のすべて。設定ファイルの説明と同じ順。
    pub const ALL: [ColorScheme; 2] = [ColorScheme::Dark, ColorScheme::Light];

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        match self {
            ColorScheme::Dark => "dark",
            ColorScheme::Light => "light",
        }
    }

    /// 配色の端末の色。
    pub fn colors(self) -> TerminalColors {
        match self {
            ColorScheme::Dark => DARK_COLORS,
            ColorScheme::Light => LIGHT_COLORS,
        }
    }
}

/// 端末の 16 色（SGR 30〜37、90〜97 とその背景）の数。256 色の残りは標準の計算で決める。
pub const ANSI_COLOR_COUNT: usize = 16;

/// 端末の既定の色。アプリが OSC 10 / 11 / 4 で変えた色は、これより優先する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalColors {
    pub background: Rgb,
    pub foreground: Rgb,
    /// 黒・赤・緑・黄・青・マゼンタ・シアン・白と、その明るい色の順。
    pub ansi: [Rgb; ANSI_COLOR_COUNT],
}

/// `0xrrggbb` の色。
const fn hex(value: u32) -> Rgb {
    Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}

/// ダークの配色。libghostty-vt の既定の色（黒の背景、白の文字、Ghostty の既定の 16 色）と同じ。
pub const DARK_COLORS: TerminalColors = TerminalColors {
    background: hex(0x000000),
    foreground: hex(0xffffff),
    ansi: [
        hex(0x1d1f21),
        hex(0xcc6666),
        hex(0xb5bd68),
        hex(0xf0c674),
        hex(0x81a2be),
        hex(0xb294bb),
        hex(0x8abeb7),
        hex(0xc5c8c6),
        hex(0x666666),
        hex(0xd54e53),
        hex(0xb9ca4a),
        hex(0xe7c547),
        hex(0x7aa6da),
        hex(0xc397d8),
        hex(0x70c0b1),
        hex(0xeaeaea),
    ],
};

/// ライトの配色。Windows Terminal の One Half Light を元に、白の背景でも読めるように黄・緑・シアンと
/// 明るい色を暗めにした（明るい色も白の背景に対してコントラスト比 3.4 以上。明るい白だけは 2.2）。白と
/// 明るい白は、文字として読めるように灰色にした。背景は表示しているタブと同じ白。
pub const LIGHT_COLORS: TerminalColors = TerminalColors {
    background: hex(0xffffff),
    foreground: hex(0x383a42),
    ansi: [
        hex(0x000000),
        hex(0xc4262e),
        hex(0x2e7d32),
        hex(0x8a6400),
        hex(0x1f5fc8),
        hex(0xa626a4),
        hex(0x0a7a8a),
        hex(0x6c6c6c),
        hex(0x8a8a8a),
        hex(0xe0443e),
        hex(0x3e9a3e),
        hex(0xa87a00),
        hex(0x3b82e0),
        hex(0xbf4fc9),
        hex(0x1593a3),
        hex(0xb0b0b0),
    ],
};

/// `[colors.tab_bar]` に書ける色のキー。
pub const TAB_BAR_COLOR_KEYS: [&str; 9] = [
    "bar",
    "active_tab",
    "active_border",
    "active_title",
    "inactive_title",
    "separator",
    "hovered_tab",
    "idle_mark",
    "bell",
];

/// `[colors.scroll_bar]` に書ける色のキー。
pub const SCROLL_BAR_COLOR_KEYS: [&str; 4] = ["track", "thumb", "hovered_thumb", "dragged_thumb"];

/// `[colors]` の設定。色は書いた色だけを持ち、書いていない色（`None`）は既定の色（配色、システムの
/// 色、端末の色から決めるもの）を使う。並びはそれぞれのキーの並び（[`ColorTable::keys`]）の順。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColorsConfig {
    pub scheme: ColorScheme,
    pub terminal: [Option<Rgb>; TERMINAL_COLOR_KEYS.len()],
    pub tab_bar: [Option<Rgb>; TAB_BAR_COLOR_KEYS.len()],
    pub scroll_bar: [Option<Rgb>; SCROLL_BAR_COLOR_KEYS.len()],
}

impl ColorsConfig {
    /// テーブル `table` の色。キーの並びの順。
    pub fn table(&self, table: ColorTable) -> &[Option<Rgb>] {
        match table {
            ColorTable::Terminal => &self.terminal,
            ColorTable::TabBar => &self.tab_bar,
            ColorTable::ScrollBar => &self.scroll_bar,
        }
    }

    pub fn table_mut(&mut self, table: ColorTable) -> &mut [Option<Rgb>] {
        match table {
            ColorTable::Terminal => &mut self.terminal,
            ColorTable::TabBar => &mut self.tab_bar,
            ColorTable::ScrollBar => &mut self.scroll_bar,
        }
    }

    /// 端末の既定の色。配色の色を、書いた背景色と文字の色で上書きしたもの。
    pub fn terminal_colors(&self) -> TerminalColors {
        let mut colors = self.scheme.colors();
        let [background, foreground] = self.terminal;
        colors.background = background.unwrap_or(colors.background);
        colors.foreground = foreground.unwrap_or(colors.foreground);
        colors
    }
}

/// `"#rrggbb"` の形の色を読む。16 進の数字の大文字小文字は区別しない。ほかの形なら `None`。
pub fn parse_color(text: &str) -> Option<Rgb> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |index: usize| u8::from_str_radix(&hex[index..index + 2], 16).ok();
    Some(Rgb {
        r: channel(0)?,
        g: channel(2)?,
        b: channel(4)?,
    })
}

/// 色を `"#rrggbb"` の形（小文字）にする。
pub fn format_color(color: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

/// `[scroll_bar]` の設定。
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollBarConfig {
    /// 幅（96 DPI でのピクセル）。
    pub width: u32,
}

impl Default for ScrollBarConfig {
    fn default() -> Self {
        Self {
            width: DEFAULT_SCROLL_BAR_WIDTH,
        }
    }
}

/// `[window]` の設定。
#[derive(Clone, Debug, PartialEq)]
pub struct WindowConfig {
    /// 端末の画面の上・下・左・右の余白（96 DPI でのピクセル）。
    pub padding: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            padding: DEFAULT_PADDING,
        }
    }
}

/// `[paste]` の設定。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PasteConfig {
    /// 貼り付ける前に、編集のダイアログで確かめるとき。
    pub confirm: PasteConfirm,
}

/// 貼り付ける前に、内容を編集のダイアログで確かめるとき（`paste.confirm`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteConfirm {
    /// コマンドとして実行されうるとき。改行を含む文字列でアプリが bracketed paste を有効にしていない
    /// ときと、bracketed paste の終わりの印を含むとき。
    #[default]
    Unsafe,
    /// bracketed paste に関わらず、改行か bracketed paste の終わりの印を含むとき。
    Multiline,
    /// 確かめない。bracketed paste でなければ、改行は CR にして送る。
    Never,
}

impl PasteConfirm {
    /// 書ける値のすべて。設定ファイルの説明と同じ順。
    pub const ALL: [PasteConfirm; 3] = [
        PasteConfirm::Unsafe,
        PasteConfirm::Multiline,
        PasteConfirm::Never,
    ];

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        match self {
            PasteConfirm::Unsafe => "unsafe",
            PasteConfirm::Multiline => "multiline",
            PasteConfirm::Never => "never",
        }
    }
}

/// `[font]` の設定。
#[derive(Clone, Debug, PartialEq)]
pub struct FontConfig {
    /// 探すフォントの名前。先頭から探し、最初に見つかったものを使う。空にはならない。
    pub families: Vec<String>,
    /// 大きさ（ポイント）。
    pub size_pt: f32,
    /// セルより広いグリフの描き方。
    pub wide_glyph: WideGlyph,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            families: DEFAULT_FONT_FAMILIES.map(str::to_owned).to_vec(),
            size_pt: DEFAULT_FONT_SIZE_PT,
            wide_glyph: WideGlyph::default(),
        }
    }
}

/// フォントのグリフが、文字に割り当てたセルより広いときの描き方（`font.wide_glyph`）。
/// 曖昧幅の文字を 1 セルで数えることは変えず、描き方だけを選ぶ。罫線などの形として描く文字には効かない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WideGlyph {
    /// セルに収まるように横に縮める。
    #[default]
    Shrink,
    /// 縮めずにセルの左に置き、はみ出した分は描かない。
    Clip,
    /// 縮めずにセルの左に置き、右のセルへはみ出して描く。
    Overflow,
    /// 縮めずにセルの真ん中に置き、左右のセルへ同じだけはみ出して描く。
    OverflowCenter,
}

impl WideGlyph {
    /// 書ける値のすべて。設定ファイルの説明と同じ順。
    pub const ALL: [WideGlyph; 4] = [
        WideGlyph::Shrink,
        WideGlyph::Clip,
        WideGlyph::Overflow,
        WideGlyph::OverflowCenter,
    ];

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        match self {
            WideGlyph::Shrink => "shrink",
            WideGlyph::Clip => "clip",
            WideGlyph::Overflow => "overflow",
            WideGlyph::OverflowCenter => "overflow_center",
        }
    }
}

/// 入力された文字列を、フォントの大きさ（ポイント）として読む。前後の空白は除く。数でないか、
/// [`FONT_SIZE_RANGE_PT`] の外なら `None`。
pub fn parse_font_size(text: &str) -> Option<f32> {
    let size: f32 = text.trim().parse().ok()?;
    FONT_SIZE_RANGE_PT.contains(&size).then_some(size)
}

/// 入力された文字列を、スクロールバーの幅として読む。前後の空白は除く。整数でないか、
/// [`SCROLL_BAR_WIDTH_RANGE`] の外なら `None`。
pub fn parse_scroll_bar_width(text: &str) -> Option<u32> {
    let width: u32 = text.trim().parse().ok()?;
    SCROLL_BAR_WIDTH_RANGE.contains(&width).then_some(width)
}

/// 入力された文字列を、余白として読む。前後の空白は除く。整数でないか、[`PADDING_RANGE`] の外なら
/// `None`。
pub fn parse_padding(text: &str) -> Option<u32> {
    let padding: u32 = text.trim().parse().ok()?;
    PADDING_RANGE.contains(&padding).then_some(padding)
}

/// 入力された文字列を、背景画像の不透明度として読む。前後の空白は除く。数でないか、
/// [`IMAGE_OPACITY_RANGE`] の外なら `None`。
pub fn parse_image_opacity(text: &str) -> Option<f32> {
    let opacity: f32 = text.trim().parse().ok()?;
    IMAGE_OPACITY_RANGE.contains(&opacity).then_some(opacity)
}

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

    /// 読めた値を `check` で確かめる。`check` が警告の文を返したら、警告を残して読めなかったことにする。
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
    match reader.take::<Table>(&mut root, None, "font") {
        Item::Absent => config.font = FontConfig::default(),
        Item::Invalid => {}
        Item::Valid(mut font) => {
            read_font(&mut reader, &mut font, &mut config.font);
            reader.warn_unknown(&font, Some("font"));
        }
    }
    read_profiles(&mut reader, &mut root, &mut config);
    match reader.take::<Table>(&mut root, None, "paste") {
        Item::Absent => config.paste = PasteConfig::default(),
        Item::Invalid => {}
        Item::Valid(mut paste) => {
            const SECTION: Option<&str> = Some("paste");
            reader
                .take::<PasteConfirm>(&mut paste, SECTION, "confirm")
                .apply(&mut config.paste.confirm, PasteConfirm::default());
            reader.warn_unknown(&paste, SECTION);
        }
    }
    match reader.take::<Table>(&mut root, None, "scroll_bar") {
        Item::Absent => config.scroll_bar = ScrollBarConfig::default(),
        Item::Invalid => {}
        Item::Valid(mut scroll_bar) => {
            const SECTION: Option<&str> = Some("scroll_bar");
            reader
                .take::<u32>(&mut scroll_bar, SECTION, "width")
                .check(&mut reader, "scroll_bar.width", |width| {
                    (!SCROLL_BAR_WIDTH_RANGE.contains(width)).then(|| {
                        format!(
                            "ignoring scroll_bar.width = {width}: must be between {} and {}",
                            SCROLL_BAR_WIDTH_RANGE.start(),
                            SCROLL_BAR_WIDTH_RANGE.end()
                        )
                    })
                })
                .apply(&mut config.scroll_bar.width, DEFAULT_SCROLL_BAR_WIDTH);
            reader.warn_unknown(&scroll_bar, SECTION);
        }
    }
    match reader.take::<Table>(&mut root, None, "window") {
        Item::Absent => config.window = WindowConfig::default(),
        Item::Invalid => {}
        Item::Valid(mut window) => {
            const SECTION: Option<&str> = Some("window");
            reader
                .take::<u32>(&mut window, SECTION, "padding")
                .check(&mut reader, "window.padding", |padding| {
                    (!PADDING_RANGE.contains(padding)).then(|| {
                        format!(
                            "ignoring window.padding = {padding}: must be between {} and {}",
                            PADDING_RANGE.start(),
                            PADDING_RANGE.end()
                        )
                    })
                })
                .apply(&mut config.window.padding, DEFAULT_PADDING);
            reader.warn_unknown(&window, SECTION);
        }
    }
    match reader.take::<Table>(&mut root, None, "colors") {
        Item::Absent => config.colors = ColorsConfig::default(),
        Item::Invalid => {}
        Item::Valid(mut colors) => {
            read_colors(&mut reader, &mut colors, &mut config.colors);
            reader.warn_unknown(&colors, Some("colors"));
        }
    }
    match reader.take::<Table>(&mut root, None, "background") {
        Item::Absent => config.background = BackgroundConfig::default(),
        Item::Invalid => {}
        Item::Valid(mut background) => {
            read_background(&mut reader, &mut background, &mut config.background);
            reader.warn_unknown(&background, Some("background"));
        }
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
                if profiles.iter().any(|other| other.name == profile.name) {
                    reader.warn(
                        &key_path(Some(&path), "name"),
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
    let text = |reader: &mut Reader, table: &mut Table, key: &str, required: bool| match reader
        .take::<String>(
        table, section, key,
    ) {
        Item::Valid(text) if !text.trim().is_empty() => Some(text),
        Item::Valid(_) if !required => Some(String::new()),
        Item::Valid(_) => {
            reader.warn(
                &key_path(section, key),
                format!("ignoring {path}: {key} must not be empty"),
            );
            None
        }
        Item::Absent if !required => Some(String::new()),
        Item::Absent => {
            reader.warn(path, format!("ignoring {path}: expected a {key}"));
            None
        }
        Item::Invalid => None,
    };
    let name = text(reader, &mut table, "name", true);
    let command_required = !backend.is_some_and(BackendKind::is_bridge);
    let command = text(reader, &mut table, "command", command_required);
    let cygwin_root = read_profile_option(
        reader,
        &mut table,
        path,
        "cygwin_root",
        backend,
        BackendKind::Cygwin,
    );
    let distribution = read_profile_option(
        reader,
        &mut table,
        path,
        "distribution",
        backend,
        BackendKind::Wsl,
    );
    reader.warn_unknown(&table, section);
    Some(Profile {
        name: name?,
        command: command?,
        backend: backend?,
        cygwin_root,
        distribution,
    })
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
    match reader.take::<String>(table, section, "backend") {
        Item::Valid(name) => match BackendKind::from_name(&name) {
            Some(kind) => Some(kind),
            None => {
                let known: Vec<&str> = BackendKind::ALL.iter().map(|kind| kind.name()).collect();
                reader.warn(
                    &key_path(section, "backend"),
                    format!("ignoring {path}: unknown backend {name:?}, expected one of {known:?}"),
                );
                None
            }
        },
        Item::Absent => Some(BackendKind::default()),
        Item::Invalid => None,
    }
}

fn read_font(reader: &mut Reader, table: &mut Table, font: &mut FontConfig) {
    const SECTION: Option<&str> = Some("font");
    let defaults = FontConfig::default();
    reader
        .take::<Strings>(table, SECTION, "family")
        .map(Strings::into_vec)
        .check(reader, "font.family", |families| {
            (families.is_empty() || families.iter().any(|family| family.trim().is_empty()))
                .then(|| format!("ignoring font.family = {families:?}: names must not be empty"))
        })
        .apply(&mut font.families, defaults.families);
    reader
        .take::<f32>(table, SECTION, "size")
        .check(reader, "font.size", |size| {
            (!FONT_SIZE_RANGE_PT.contains(size)).then(|| {
                format!(
                    "ignoring font.size = {size}: must be between {} and {}",
                    FONT_SIZE_RANGE_PT.start(),
                    FONT_SIZE_RANGE_PT.end()
                )
            })
        })
        .apply(&mut font.size_pt, defaults.size_pt);
    reader
        .take::<WideGlyph>(table, SECTION, "wide_glyph")
        .apply(&mut font.wide_glyph, defaults.wide_glyph);
}

fn read_background(reader: &mut Reader, table: &mut Table, background: &mut BackgroundConfig) {
    const SECTION: Option<&str> = Some("background");
    let defaults = BackgroundConfig::default();
    let images = match reader.take::<Value>(table, SECTION, "images") {
        Item::Valid(value) => read_background_images(reader, value),
        Item::Absent => Item::Absent,
        Item::Invalid => Item::Invalid,
    };
    images.apply(&mut background.images, defaults.images);
    read_image_opacity(reader, table, SECTION)
        .apply(&mut background.image_opacity, defaults.image_opacity);
    reader
        .take::<BackgroundFit>(table, SECTION, "fit")
        .apply(&mut background.fit, defaults.fit);
}

/// テーブル `table`（キーのパスは `section`）の `image_opacity` を読む。範囲の外は警告して読めなかったことに
/// する。
fn read_image_opacity(reader: &mut Reader, table: &mut Table, section: Option<&str>) -> Item<f32> {
    let path = key_path(section, "image_opacity");
    reader
        .take::<f32>(table, section, "image_opacity")
        .check(reader, &path, |opacity| {
            (!IMAGE_OPACITY_RANGE.contains(opacity)).then(|| {
                format!(
                    "ignoring {path} = {opacity}: must be between {} and {}",
                    IMAGE_OPACITY_RANGE.start(),
                    IMAGE_OPACITY_RANGE.end()
                )
            })
        })
}

/// `background.images`（`[[background.images]]` のテーブルの配列）を読む。読めないテーブルは、警告して
/// そのテーブルだけを飛ばす。テーブルの配列でなければ、全体を読めなかったことにする。
fn read_background_images(reader: &mut Reader, value: Value) -> Item<Vec<BackgroundImage>> {
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
    let name = match reader.take::<String>(&mut table, section, "path") {
        Item::Valid(name) if !name.trim().is_empty() => name,
        Item::Valid(_) => {
            reader.warn(
                &key_path(section, "path"),
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
    let fit = match reader.take::<BackgroundFit>(&mut table, section, "fit") {
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

/// `[colors]` の配色と、その中の色のテーブルを読む。
fn read_colors(reader: &mut Reader, colors: &mut Table, config: &mut ColorsConfig) {
    const SECTION: Option<&str> = Some("colors");
    reader
        .take::<ColorScheme>(colors, SECTION, "scheme")
        .apply(&mut config.scheme, ColorScheme::default());
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

fn key_path(section: Option<&str>, key: &str) -> String {
    match section {
        Some(section) => format!("{section}.{key}"),
        None => key.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::Action;

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
        // 前の書き方（文字列やインラインテーブルの配列、文字列一つ）は読み替えてから読む。
        for (old, warning) in [
            (
                "[background]\nimages = 'C:\\one.png'\n",
                "ignoring background.images: expected [[background.images]] tables",
            ),
            (
                "[background]\nimages = ['C:\\one.png']\n",
                "ignoring background.images[0]: expected a table",
            ),
        ] {
            assert_eq!(parse(old).warnings, [warning]);
            let migrated = migrate::migrate(old);
            let parsed = parse(&migrated.text);
            assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
            assert_eq!(
                parsed.config.background.images,
                [BackgroundImage::new(r"C:\one.png")]
            );
        }
        // `opacity` は知らないキー（読み替える前）。
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
        let settings = text
            .lines()
            .filter(|line| !line.starts_with('#') && line.contains(" = "))
            .count();
        assert_eq!(settings, 8 + 9 + 2 + 15, "{text}");
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
                Profile::new("zsh", "zsh -l", BackendKind::Cygwin),
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
    fn schemes_differ_in_every_color() {
        let (dark, light) = (DARK_COLORS, LIGHT_COLORS);
        assert_ne!(dark.background, light.background);
        assert_ne!(dark.foreground, light.foreground);
        for index in 0..ANSI_COLOR_COUNT {
            assert_ne!(dark.ansi[index], light.ansi[index], "{index}");
        }
        // ライトの背景は表示しているタブと同じ白。
        assert_eq!(light.background, hex(0xffffff));
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
    fn colors_are_written_back_as_read() {
        for text in ["#000000", "#ffffff", "#a0b1c2"] {
            assert_eq!(parse_color(text).map(format_color).as_deref(), Some(text));
        }
        assert_eq!(parse_color("#A0B1C2"), parse_color("#a0b1c2"));
        for text in [
            "",
            "#",
            "#fff",
            "#12345",
            "#1234567",
            "#12345678",
            "123456",
            "#12345g",
            "#ｆｆｆｆｆｆ",
        ] {
            assert_eq!(parse_color(text), None, "{text:?}");
        }
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
            "shell = ' '\ndefault_profile = 3\n[font]\nfamily = 3\nsize = 500\nwide_glyph = 'wide'\n[paste]\nconfirm = 1\n[scroll_bar]\nwidth = 2\n[window]\npadding = 65\n[keys]\n\"ctrl+tab\" = 5\n\"ctrl+page_down\" = 'next'\n[colors]\nscheme = 'sepia'\nforeground = 'blue'\n[colors.tab_bar]\nbar = 'red'\n[background]\nimages = ''\nimage_opacity = 2\nfit = 0\n",
            &previous,
        );
        assert!(!parsed.syntax_error);
        assert_eq!(parsed.config, previous);
        assert_eq!(parsed.warnings.len(), 16, "{:?}", parsed.warnings);
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

    #[test]
    fn inputs_are_checked_like_the_file() {
        assert_eq!(parse_font_size(" 10.5 "), Some(10.5));
        assert_eq!(parse_font_size("1"), Some(1.0));
        assert_eq!(parse_font_size("128"), Some(128.0));
        for text in ["", "0", "0.5", "129", "nan", "inf", "12pt", "-3"] {
            assert_eq!(parse_font_size(text), None, "{text:?}");
        }
        assert_eq!(parse_padding(" 0 "), Some(0));
        assert_eq!(parse_padding("64"), Some(64));
        for text in ["", "65", "4.5", "-1", "wide"] {
            assert_eq!(parse_padding(text), None, "{text:?}");
        }
        assert_eq!(parse_image_opacity(" 0.75 "), Some(0.75));
        assert_eq!(parse_image_opacity("0"), Some(0.0));
        assert_eq!(parse_image_opacity("1"), Some(1.0));
        for text in ["", "-0.01", "1.01", "nan", "80%"] {
            assert_eq!(parse_image_opacity(text), None, "{text:?}");
        }
        assert_eq!(parse_scroll_bar_width(" 16 "), Some(16));
        assert_eq!(parse_scroll_bar_width("4"), Some(4));
        assert_eq!(parse_scroll_bar_width("64"), Some(64));
        for text in ["", "3", "65", "12.5", "-1", "wide"] {
            assert_eq!(parse_scroll_bar_width(text), None, "{text:?}");
        }
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
    fn shell_of_cygwin_uses_the_cygwin_bridge() {
        let cygwin = |root: &Path| root == Path::new(r"C:\app\cygwin");
        let profile = |command: &str| Profile::from_shell_with(command, cygwin);
        let bridge = |name: &str, command: &str| Profile {
            cygwin_root: Some(r"C:\app\cygwin".to_owned()),
            ..Profile::new(name, command, BackendKind::Cygwin)
        };
        assert_eq!(
            profile(r"C:\app\cygwin\bin\zsh.exe -l"),
            bridge("zsh", "/bin/zsh -l")
        );
        assert_eq!(
            profile(r#""C:\app\cygwin\bin\bash.EXE"  --login -i "#),
            bridge("bash", "/bin/bash --login -i")
        );
        // Cygwin のフォルダは書いたままの区切りで。
        assert_eq!(
            profile(r"C:/app/cygwin/bin/zsh"),
            Profile {
                cygwin_root: Some(r"C:/app/cygwin".to_owned()),
                ..bridge("zsh", "/bin/zsh")
            }
        );
        // bin の隣のフォルダでも、一つ上が Cygwin のフォルダならよい。
        assert_eq!(
            profile(r"C:\app\cygwin\sbin\tool.exe x"),
            bridge("tool", "/sbin/tool x")
        );
        // Cygwin のフォルダの下でないもの、フォルダのないもの、相対パスは ConPTY のまま。
        for command in [
            r"C:\cygwin64\bin\zsh.exe -l",
            r"C:\app\cygwin\bin\x\zsh.exe",
            "zsh -l",
            r"cygwin\bin\zsh.exe",
            r"C:\zsh.exe",
        ] {
            assert_eq!(
                profile(command),
                Profile::new(&program_name(command), command, BackendKind::Conpty),
                "{command}"
            );
        }
    }

    #[test]
    fn windows_programs_are_found_in_bridge_commands() {
        let found = |backend: BackendKind, command: &str| {
            Profile::new("p", command, backend)
                .windows_program_in_bridge()
                .map(str::to_owned)
        };
        let wsl = |command: &str| found(BackendKind::Wsl, command);
        let cygwin = |command: &str| found(BackendKind::Cygwin, command);

        // wsl はどちらのブリッジでも。パスと大文字小文字を問わず、書いたままの名前を返す。
        for (command, program) in [
            ("wsl", "wsl"),
            ("wsl.exe", "wsl.exe"),
            ("WSL.EXE -d Ubuntu", "WSL.EXE"),
            (r"C:\Windows\System32\wsl.exe", "wsl.exe"),
            ("/mnt/c/Windows/System32/wsl.exe -d Ubuntu", "wsl.exe"),
            (r#"  "C:\Windows\System32\Wsl.exe" -d Ubuntu "#, "Wsl.exe"),
            (
                "'/cygdrive/c/Windows/System32/wsl.exe' -d Ubuntu",
                "wsl.exe",
            ),
        ] {
            assert_eq!(wsl(command).as_deref(), Some(program), "{command}");
            assert_eq!(cygwin(command).as_deref(), Some(program), "{command}");
        }

        // WSL では .exe で終わるものはすべて Windows のプログラム。
        for command in [
            "docker.exe run -it --rm otiai10/amesh",
            "cmd.exe",
            "'/mnt/c/Program Files/PowerShell/7/pwsh.EXE' -NoLogo",
        ] {
            assert!(wsl(command).is_some(), "{command}");
        }
        for command in [
            "",
            "   ",
            "zsh -l",
            "/usr/bin/docker run",
            "cmd",
            "a.exe.bak",
            "echo wsl.exe",
        ] {
            assert_eq!(wsl(command), None, "{command}");
        }

        // Cygwin では分かっている名前だけ。Cygwin のプログラムも .exe で終わる。
        for command in [
            "cmd.exe /c dir",
            "cmd",
            "powershell.exe -NoLogo",
            "PWSH.EXE",
        ] {
            assert!(cygwin(command).is_some(), "{command}");
        }
        for command in [
            "",
            "zsh.exe -l",
            "/bin/bash.exe",
            "docker.exe run",
            "zsh -l",
            "echo cmd.exe",
        ] {
            assert_eq!(cygwin(command), None, "{command}");
        }

        // ConPTY では wsl.exe が正しい。
        for command in ["wsl.exe", "cmd.exe", "powershell.exe -NoLogo"] {
            assert_eq!(found(BackendKind::Conpty, command), None, "{command}");
        }
    }

    /// `path_variable` を `PATH` の値、`files` をあるファイルの全部として Cygwin のフォルダを探す。
    fn find_cygwin(path_variable: Option<&str>, files: &[&str]) -> Option<PathBuf> {
        find_cygwin_root_with(path_variable.map(OsStr::new), |path| {
            files.iter().any(|file| Path::new(file) == path)
        })
    }

    #[test]
    fn cygwin_root_is_found_from_path_in_order() {
        let files = [
            r"C:\first\bin\cygwin1.dll",
            r"C:\first\bin\sh.exe",
            r"C:\second\bin\cygwin1.dll",
            r"C:\second\bin\sh.exe",
            r"C:\cygwin64\bin\cygwin1.dll",
        ];
        assert_eq!(
            find_cygwin(Some(r"C:\Windows;C:\second\bin;C:\first\bin"), &files),
            Some(PathBuf::from(r"C:\second"))
        );
        assert_eq!(
            find_cygwin(Some(r"C:\first\bin;C:\second\bin"), &files),
            Some(PathBuf::from(r"C:\first"))
        );
        // 区切りと大文字小文字は問わない。末尾の区切りがあっても一つ上を返す。
        let files = [r"C:\first\BIN\cygwin1.dll", r"C:\first\BIN\sh.exe"];
        assert_eq!(
            find_cygwin(Some(r"C:\first\BIN\"), &files),
            Some(PathBuf::from(r"C:\first"))
        );
        let files = [r"C:/first/bin/cygwin1.dll", r"C:/first/bin/sh.exe"];
        assert_eq!(
            find_cygwin(Some("C:/first/bin"), &files),
            Some(PathBuf::from(r"C:\first"))
        );
    }

    #[test]
    fn cygwin_root_skips_folders_that_are_not_cygwin_bin() {
        let files = [
            // sh.exe がない。
            r"C:\no_shell\bin\cygwin1.dll",
            // フォルダの名前が bin でない。
            r"C:\app\cygwin1.dll",
            r"C:\app\sh.exe",
            // cygwin1.dll がない。
            r"C:\msys\bin\sh.exe",
            // 相対パス。
            r"rel\bin\cygwin1.dll",
            r"rel\bin\sh.exe",
            // ここまでを飛ばして見つかるもの。
            r"C:\real\bin\cygwin1.dll",
            r"C:\real\bin\sh.exe",
        ];
        assert_eq!(
            find_cygwin(
                Some(r"C:\no_shell\bin;C:\app;C:\msys\bin;rel\bin;C:\real\bin"),
                &files
            ),
            Some(PathBuf::from(r"C:\real"))
        );
    }

    #[test]
    fn cygwin_root_reads_quoted_and_empty_path_entries() {
        let files = [
            r"C:\Program Files\cyg;win\bin\cygwin1.dll",
            r"C:\Program Files\cyg;win\bin\sh.exe",
        ];
        assert_eq!(
            find_cygwin(Some(r#";;"C:\Program Files\cyg;win\bin";"#), &files),
            Some(PathBuf::from(r"C:\Program Files\cyg;win"))
        );
        // 空の項目は Cygwin のフォルダにしない。
        assert_eq!(find_cygwin(Some(";;"), &["cygwin1.dll", "sh.exe"]), None);
    }

    #[test]
    fn cygwin_root_falls_back_to_common_roots() {
        let both = [r"C:\cygwin\bin\cygwin1.dll", r"C:\cygwin64\bin\cygwin1.dll"];
        assert_eq!(
            find_cygwin(None, &both),
            Some(PathBuf::from(r"C:\cygwin64"))
        );
        assert_eq!(
            find_cygwin(Some(r"C:\Windows"), &both[..1]),
            Some(PathBuf::from(r"C:\cygwin"))
        );
        // PATH の項目と違い、よくある場所は sh.exe がなくてもよい。PATH で見つかればそちらが先。
        let files = [
            r"C:\cygwin64\bin\cygwin1.dll",
            r"C:\real\bin\cygwin1.dll",
            r"C:\real\bin\sh.exe",
        ];
        assert_eq!(
            find_cygwin(Some(r"C:\real\bin"), &files),
            Some(PathBuf::from(r"C:\real"))
        );
        assert_eq!(find_cygwin(None, &[]), None);
        assert_eq!(find_cygwin(Some(r"C:\Windows"), &[]), None);
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
