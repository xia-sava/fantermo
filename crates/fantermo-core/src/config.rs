//! 設定ファイル（TOML）の型、既定値、解釈。
//!
//! 項目ごとに読む。書いていない項目は既定値にする。型の合わない値や範囲外の値はその項目だけを前の
//! 値のまま（起動したときは既定値）にし、知らないキーは無視する。どちらも、エラーのある行を添えた
//! 警告として返す。TOML として読めないときは、すべてを前の値のままにする。
//!
//! 書ける項目と既定値は [`TEMPLATE`] にある。古い書き方は、解釈する前に [`migrate`] で今の書き方に
//! 読み替える（[`parse`] は今の書き方だけを読む）。
//!
//! 各節の型と既定値、項目の一覧 [`SCALARS`] はこのファイルにある。プロファイルは [`profile`]、色は
//! [`colors`]、TOML から読む処理は [`read`] に分け、公開する名前は `config::` の下に再公開する。

mod colors;
pub mod migrate;
mod profile;
mod read;

use std::{ops::RangeInclusive, str::FromStr};

use crate::keymap::Keymap;

pub use colors::{
    ANSI_COLOR_COUNT, ColorScheme, ColorTable, ColorsConfig, DARK_COLORS, LIGHT_COLORS,
    SCROLL_BAR_COLOR_KEYS, TAB_BAR_COLOR_KEYS, TERMINAL_COLOR_KEYS, TerminalColors, format_color,
    parse_color,
};
pub use profile::{
    BackendKind, COMMON_CYGWIN_ROOTS, PROFILE_KEYS, Profile, ProfileError, ProfileKeys,
    cygwin_dll_path, default_profile_index, find_cygwin_root, find_cygwin_root_with,
    find_program_on_path, is_bare_program_name, is_cygwin_root, profile_position, shell_command,
    split_program, wsl_path, wslenv_with,
};
pub use read::{Parsed, Warning, parse};

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

/// 大きな貼り付けとして確認する既定の大きさ（KiB）。
pub const DEFAULT_PASTE_CONFIRM_SIZE: u32 = 8;

/// 大きな貼り付けとして確認する大きさとして受け付ける範囲（KiB）。`0` は確認しない。上限は 1 GiB。
pub const PASTE_CONFIRM_SIZE_RANGE: RangeInclusive<u32> = 0..=1_048_576;

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
# 行頭の「#」を消して値を書き換え、保存すると反映される。書いていない項目は既定値になる。

# 新しいタブで起動するプロファイル。[[profiles]] のテーブルを並べ、name に名前、command に起動するコマンドラインを
# 書く。backend はシェルとの接続方式で、"conpty"（Windows の疑似コンソール）、"cygwin"（Cygwin のブリッジ）、
# "wsl"（WSL のブリッジ）を書ける。書かなければ "conpty"。"cygwin" と "wsl" の command は Cygwin・WSL の中で実行する
# コマンドラインで、書かなければログインシェルを起動する。"cygwin" では cygwin_root に Cygwin のフォルダを書ける。
# 書かなければ環境変数 PATH と、標準のインストール先（C:\cygwin64、C:\cygwin）から探す。"wsl" では distribution に
# ディストリビューションの名前を書ける（書かなければ既定のディストリビューション）。
# icon はタブに表示するアイコンの画像ファイル（PNG、ICO など）。相対パスは設定ファイルのフォルダが基準。書かないか、
# 読み込めなければ、プロファイル名から決まる淡い色の正方形に絵を載せたアイコンを作る。絵は "cygwin" なら Cygwin の
# フォルダの Cygwin.ico、"wsl" なら wsl.exe のアイコン、"conpty" なら command の実行ファイルのアイコンで、
# 読み込めなければ fantermo に組み込みの絵（Cygwin の C、WSL のペンギン、ConPTY の >_）。同じ絵になるプロファイルが
# 複数あれば、プロファイル名の頭文字を添える。
# Cygwin と WSL はブリッジで接続することを勧める。"conpty" では、アプリの出力の一部が書き換えられたり順番が
# 変わったり、端末への問い合わせの一部に ConPTY が代わりに答えるか応答がなかったり、kitty graphics protocol と
# sixel の画像が表示されなかったり、ウィンドウの大きさを変えるとスクロールバックの行が欠けたりすることがある。
# タブバーの「+」は既定のプロファイルを開き、その右の「▾」か「+」の右クリックで一覧から選んで開く。fantermo を
# 引数つきで起動したときは、「+」とキーとメニューの「新しいタブ」も引数のコマンドラインを使う。
# 書かなければ PowerShell（powershell.exe -NoLogo）を起動する。変更は次に開くタブから反映される。
# プロファイルを書かずに、最上位に shell = 'コマンドライン' と一つだけ書いてもよい（名前は実行ファイルの名前）。
# shell に書いた Cygwin の bin の下の実行ファイルと wsl.exe は、ブリッジで接続するプロファイルになる。
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
# 例: icon = 'C:\画像\zsh.png'
#
# 例: [[profiles]]
# 例: name = "Ubuntu"
# 例: backend = "wsl"
# 例: distribution = "Ubuntu"

[font]
# フォントの名前。先頭から探し、最初に見つかったものを使う。名前一つだけでもよい。
# family = ["Bizin Gothic", "BIZ UDGothic", "Consolas"]

# サイズ（ポイント）。1 から 128 まで。小数でもよい。
# size = 12

# 文字がセルより広いとき（曖昧幅の ○ や ※ が全角の幅でデザインされているときなど）の描き方。
# セルの数は変えない。"shrink" は横に縮める、"clip" は縮めずにセルの左に置いてはみ出た部分を切り取る、
# "overflow" は縮めずに右のセルへはみ出して描く、"overflow_center" は縮めずにセルの中央に置いて
# 左右のセルへ同じだけはみ出して描く。
# wide_glyph = "shrink"

[paste]
# 貼り付ける前に、内容を編集できるダイアログで確認するとき。
# "unsafe" は改行を含む文字列がコマンドとして実行される可能性があるとき（アプリが bracketed paste を有効にして
# いないときなど）、"multiline" は bracketed paste に関わらず改行を含むとき、"never" は確認せずに改行ごと送る。
# confirm = "unsafe"

# この大きさ（KiB）以上の文字列を貼り付ける前は、confirm の設定、改行の有無、bracketed paste に関わらず、
# 同じダイアログで確認する。0 で確認しない。0 から 1048576 まで。
# confirm_size = 8

[clipboard]
# アプリがクリップボードの内容を読み取ろうとしたとき（OSC 52）。
# "ask" は確認のダイアログを出す、"allow" は確認せずに渡す、"deny" は何も返さない。
# read = "ask"

# アプリがクリップボードへ書き込もうとしたとき（OSC 52 など）。
# "allow" は書き込む、"deny" は書き込まない。
# write = "allow"

[scroll_bar]
# 端末の右のスクロールバーの幅（96 DPI でのピクセル。画面の拡大率に合わせて広げる）。4 から 64 まで。
# 変更しても、ウィンドウの大きさは変わらず、端末の桁数が変わる。
# width = 12

[window]
# 端末の画面の上・下・左・右に空ける余白（96 DPI でのピクセル。画面の拡大率に合わせて広げる）。0 から 64 まで。
# 余白は端末の背景色で塗る。変更しても、ウィンドウの大きさは変わらず、端末の桁数と行数が変わる。
# padding = 4

[colors]
# 端末の配色。"dark" は黒の背景に白の文字、"light" は白の背景に濃い灰色の文字。背景色・文字色・16 色を
# まとめて変える。カーソルは文字色で描き、選択範囲は文字色と背景色を入れ替えて描く。
# scheme = "dark"

# 端末の背景色と文字色。"#rrggbb" の形で書く。書かない色は配色の色（下の値は "dark" の色）。
# background = "#000000"
# foreground = "#ffffff"

[colors.tab_bar]
# タブバーの色。"#rrggbb" の形で書く。書かない色は Windows のシステムの色から決める（下の値は Windows 11
# の既定の色のときのもの）。bar は背景、active_tab は表示しているタブ、active_border はその左・右・上の
# 縁の線、active_title はその文字と閉じるボタンの「×」、inactive_title は表示していないタブの文字と「×」と
# マウスを載せている「+」「▾」「≡」、separator は表示していないタブの間の区切り線、hovered_tab は
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
# スクロールバーの色。"#rrggbb" の形で書く。書かない色は端末の背景色と文字色から決める（下の値は配色が
# "dark" の黒の背景と白の文字のときのもの）。track はマウスを載せている間とドラッグしている間のトラック（背景画像が
# あるときは半透明にして画像を透かす）、thumb はつまみ、hovered_thumb はマウスを載せているつまみ、dragged_thumb は
# ドラッグしているつまみ。
# track = "#0f0f0f"
# thumb = "#595959"
# hovered_thumb = "#808080"
# dragged_thumb = "#a6a6a6"

[background]
# 背景画像の不透明度。0 から 1 まで。1 は画像をそのまま見せ、0 は画像を消して端末の背景色だけにする。間の値は、
# 画像の上に端末の背景色を重ねて薄める。アプリが背景色を付けた文字、反転した文字、選択範囲は、その色で
# 塗りつぶす。
# image_opacity = 0.2

# 背景画像の表示方法。"fill"（ページ幅に合わせる）は縦横比を保って端末を覆い、はみ出た部分を切り取る、"fit"（画面の
# サイズに合わせる）は縦横比を保って端末に収め、余りは背景色にする、"stretch"（拡大して表示）は端末に合わせて
# 引き伸ばす、"tile"（並べて表示）は元の大きさで敷き詰める、"center"（中央に表示）は元の大きさで中央に置く。
# fit = "fill"

# 端末の背景に表示する画像の一覧。[[background.images]] のテーブルを画像の項目ごとに並べ、path に画像のファイル、
# フォルダ（中の画像から選ぶ。サブフォルダは対象外）、ファイルの名前に「*」「?」を使ったワイルドカードを書く。
# 新しいタブを開くたびに、この中からランダムに一枚選ぶ。フォルダとワイルドカードから選ぶのは png、jpg、jpeg、
# bmp、gif（最初のコマ）、webp（Windows で読めるとき）。読み込めない画像は飛ばして別の画像を選ぶ。相対パスはこの
# ファイルのフォルダが基準。書かなければ画像を表示しない。画像はタブバーの下の全体（端末とスクロールバー）に敷く。
# テーブルに fit と image_opacity を書くと、その項目（フォルダやワイルドカードなら、そこから選んだ画像）にだけ
# 適用する。書かなかったものは上の値に従う。[[background.images]] は [background] の値より後ろに書く。
# 例: [[background.images]]
# 例: path = 'C:\画像\壁紙'
# 例: [[background.images]]
# 例: path = 'C:\画像\*.jpg'
# 例: [[background.images]]
# 例: path = 'C:\画像\模様.png'
# 例: fit = "tile"

[keys]
# キーの組み合わせと、押したときの動作。書いた組み合わせだけ、下に並べた既定の割り当てを変える。
# 組み合わせは、修飾キー（ctrl、shift、alt、win）とキーを「+」でつなぐ。キーの名前はキーボード上の
# 位置を表す名前で、a〜z、digit_0〜digit_9、f1〜f24、tab、enter、space、escape、backspace、insert、delete、
# home、end、page_up、page_down、arrow_up、arrow_down、arrow_left、arrow_right、context_menu、
# numpad_0〜numpad_9 など。
# 動作は copy、paste、context_menu、new_tab、close_tab、next_tab、prev_tab、scroll_page_up、
# scroll_page_down、scroll_line_up、scroll_line_down。"none" は割り当てを外し、キーをシェルへ送る。
# 同じ動作に複数の組み合わせを割り当ててもよい。scroll_ で始まる動作は、代替画面ではキーをアプリへ送る。
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
# "ctrl+page_up" = "scroll_page_up"
# "ctrl+page_down" = "scroll_page_down"
# "ctrl+arrow_up" = "scroll_line_up"
# "ctrl+arrow_down" = "scroll_line_down"
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
    pub clipboard: ClipboardConfig,
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
            clipboard: ClipboardConfig::default(),
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

/// 名前で書く値の表 `names` の値。表の順。
const fn named_values<T: Copy, const N: usize>(names: &[(T, &str); N]) -> [T; N] {
    let mut values = [names[0].0; N];
    let mut index = 1;
    while index < N {
        values[index] = names[index].0;
        index += 1;
    }
    values
}

/// 名前で書く値の表 `names` での、`value` の名前。
fn name_of<T: Copy + PartialEq>(names: &[(T, &'static str)], value: T) -> &'static str {
    names
        .iter()
        .find(|(candidate, _)| *candidate == value)
        .map(|(_, name)| *name)
        .expect("every value has a name")
}

/// 名前で書く値の表 `names` で、名前が `name` の値。大文字小文字は区別する。
fn value_named<T: Copy>(names: &[(T, &'static str)], name: &str) -> Option<T> {
    names
        .iter()
        .find(|(_, candidate)| *candidate == name)
        .map(|(value, _)| *value)
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

/// `[[background.images]]` のテーブルのキー。
pub struct ImageKeys {
    pub path: &'static str,
    pub fit: &'static str,
    /// `[background]` の不透明度のキーと同じ名前。
    pub image_opacity: &'static str,
}

/// `[[background.images]]` のテーブルのキー。読む処理と書き戻す処理が使う。
pub const IMAGE_KEYS: ImageKeys = ImageKeys {
    path: "path",
    fit: "fit",
    image_opacity: "image_opacity",
};

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    /// 値と、設定ファイルに書く名前。設定ファイルの説明と同じ順。
    const NAMES: [(BackgroundFit, &'static str); 5] = [
        (BackgroundFit::Fill, "fill"),
        (BackgroundFit::Fit, "fit"),
        (BackgroundFit::Stretch, "stretch"),
        (BackgroundFit::Tile, "tile"),
        (BackgroundFit::Center, "center"),
    ];

    /// 書ける値のすべて。設定ファイルの説明と同じ順。
    pub const ALL: [BackgroundFit; 5] = named_values(&Self::NAMES);

    /// 設定ファイルに書く名前 `name` の値。
    pub fn from_name(name: &str) -> Option<Self> {
        value_named(&Self::NAMES, name)
    }

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        name_of(&Self::NAMES, self)
    }
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
#[derive(Clone, Debug, PartialEq)]
pub struct PasteConfig {
    /// 貼り付ける前に、編集のダイアログで確認するとき。
    pub confirm: PasteConfirm,
    /// この大きさ（KiB）以上なら、`confirm` に関わらず編集のダイアログで確認する。`0` は確認しない。
    pub confirm_size: u32,
}

impl Default for PasteConfig {
    fn default() -> Self {
        Self {
            confirm: PasteConfirm::default(),
            confirm_size: DEFAULT_PASTE_CONFIRM_SIZE,
        }
    }
}

impl PasteConfig {
    /// `bytes` バイトの文字列が、大きな貼り付けとして確認する大きさか。
    pub fn is_large(&self, bytes: usize) -> bool {
        const BYTES_PER_KIB: u64 = 1024;
        self.confirm_size != 0 && bytes as u64 >= u64::from(self.confirm_size) * BYTES_PER_KIB
    }
}

/// 貼り付ける前に、内容を編集のダイアログで確認するとき（`paste.confirm`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PasteConfirm {
    /// コマンドとして実行される可能性があるとき。改行を含む文字列でアプリが bracketed paste を有効にしていない
    /// ときと、bracketed paste の終わりの印を含むとき。
    #[default]
    Unsafe,
    /// bracketed paste に関わらず、改行か bracketed paste の終わりの印を含むとき。
    Multiline,
    /// 確認しない。bracketed paste でなければ、改行は CR にして送る。
    Never,
}

impl PasteConfirm {
    /// 値と、設定ファイルに書く名前。設定ファイルの説明と同じ順。
    const NAMES: [(PasteConfirm, &'static str); 3] = [
        (PasteConfirm::Unsafe, "unsafe"),
        (PasteConfirm::Multiline, "multiline"),
        (PasteConfirm::Never, "never"),
    ];

    /// 書ける値のすべて。設定ファイルの説明と同じ順。
    pub const ALL: [PasteConfirm; 3] = named_values(&Self::NAMES);

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        name_of(&Self::NAMES, self)
    }

    /// 設定ファイルに書く名前 `name` の値。
    pub fn from_name(name: &str) -> Option<Self> {
        value_named(&Self::NAMES, name)
    }
}

/// `[clipboard]` の設定。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipboardConfig {
    /// アプリがクリップボードの内容を読み取ろうとしたとき。
    pub read: ClipboardRead,
    /// アプリがクリップボードへ書き込もうとしたとき。
    pub write: ClipboardWrite,
}

/// アプリがクリップボードへ書き込もうとしたときにどうするか（`clipboard.write`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClipboardWrite {
    /// 書き込む。
    #[default]
    Allow,
    /// 書き込まない。
    Deny,
}

impl ClipboardWrite {
    /// 値と、設定ファイルに書く名前。設定ファイルの説明と同じ順。
    const NAMES: [(ClipboardWrite, &'static str); 2] = [
        (ClipboardWrite::Allow, "allow"),
        (ClipboardWrite::Deny, "deny"),
    ];

    /// 書ける値のすべて。設定ファイルの説明と同じ順。
    pub const ALL: [ClipboardWrite; 2] = named_values(&Self::NAMES);

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        name_of(&Self::NAMES, self)
    }

    /// 設定ファイルに書く名前 `name` の値。
    pub fn from_name(name: &str) -> Option<Self> {
        value_named(&Self::NAMES, name)
    }
}

/// アプリがクリップボードの内容を読み取ろうとしたときにどうするか（`clipboard.read`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClipboardRead {
    /// 確認のダイアログを出す。
    #[default]
    Ask,
    /// 確認せずに渡す。
    Allow,
    /// 何も返さない。
    Deny,
}

impl ClipboardRead {
    /// 値と、設定ファイルに書く名前。設定ファイルの説明と同じ順。
    const NAMES: [(ClipboardRead, &'static str); 3] = [
        (ClipboardRead::Ask, "ask"),
        (ClipboardRead::Allow, "allow"),
        (ClipboardRead::Deny, "deny"),
    ];

    /// 書ける値のすべて。設定ファイルの説明と同じ順。
    pub const ALL: [ClipboardRead; 3] = named_values(&Self::NAMES);

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        name_of(&Self::NAMES, self)
    }

    /// 設定ファイルに書く名前 `name` の値。
    pub fn from_name(name: &str) -> Option<Self> {
        value_named(&Self::NAMES, name)
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    /// 値と、設定ファイルに書く名前。設定ファイルの説明と同じ順。
    const NAMES: [(WideGlyph, &'static str); 4] = [
        (WideGlyph::Shrink, "shrink"),
        (WideGlyph::Clip, "clip"),
        (WideGlyph::Overflow, "overflow"),
        (WideGlyph::OverflowCenter, "overflow_center"),
    ];

    /// 書ける値のすべて。設定ファイルの説明と同じ順。
    pub const ALL: [WideGlyph; 4] = named_values(&Self::NAMES);

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        name_of(&Self::NAMES, self)
    }

    /// 設定ファイルに書く名前 `name` の値。
    pub fn from_name(name: &str) -> Option<Self> {
        value_named(&Self::NAMES, name)
    }
}

/// 名前で書く値の表 `names` の名前。表の順。
const fn names_of<T: Copy, const N: usize>(names: &[(T, &'static str); N]) -> [&'static str; N] {
    let mut list = [""; N];
    let mut index = 0;
    while index < N {
        list[index] = names[index].1;
        index += 1;
    }
    list
}

/// `[font]` のテーブルの名前。
const FONT_TABLE: &str = "font";
/// `[colors]` のテーブルの名前。
const COLORS_TABLE: &str = "colors";
/// `[background]` のテーブルの名前。
const BACKGROUND_TABLE: &str = "background";

/// 数か名前一つだけの設定の項目。読む処理（[`parse`]）と書き戻す処理
/// （[`config_edit::update`](crate::config_edit::update)）は [`SCALARS`] を上から順に処理し、設定
/// ダイアログの入力欄から読む処理（[`parse_input`]）は項目を名前で引く。
pub struct Scalar {
    /// 項目を置く最上位のテーブルの名前（`font` など）。
    pub table: &'static str,
    /// テーブルの中のキー。
    pub key: &'static str,
    /// 値の種類と、読み書きするフィールド。
    pub kind: ScalarKind,
}

impl Scalar {
    /// テーブルとキーを点でつないだ名前（`font.size` など）。
    pub fn name(&self) -> String {
        key_path(Some(self.table), self.key)
    }

    /// 名前（`font.size` など）が `name` か。
    pub fn is_named(&self, name: &str) -> bool {
        name.split_once('.') == Some((self.table, self.key))
    }
}

/// 設定の項目の値の種類と、[`Config`] のどのフィールドに入れるか。既定値は `Config::default()` の
/// そのフィールドの値。
pub enum ScalarKind {
    /// 小数でもよい数。`range` の外は読めない。
    Float {
        range: RangeInclusive<f32>,
        get: fn(&Config) -> f32,
        set: fn(&mut Config, f32),
    },
    /// 0 以上の整数。`range` の外は読めない。
    Int {
        range: RangeInclusive<u32>,
        get: fn(&Config) -> u32,
        set: fn(&mut Config, u32),
    },
    /// 名前で書く値。`names` は書ける名前で、`set` は知らない名前なら何もせずに `false` を返す。
    Name {
        names: &'static [&'static str],
        get: fn(&Config) -> &'static str,
        set: fn(&mut Config, &str) -> bool,
    },
}

/// `value` があれば `field` に入れて `true`。なければ `false`。
fn set_named<T>(field: &mut T, value: Option<T>) -> bool {
    let Some(value) = value else {
        return false;
    };
    *field = value;
    true
}

/// 数か名前一つだけの設定の項目。設定ファイルの説明と同じ順。ここに載らない項目（`font.family`、
/// `default_profile`、`profiles`、`background.images`、`[colors]` の色、`[keys]`）は、読む処理と
/// 書き戻す処理にそれぞれ書く。
pub const SCALARS: &[Scalar] = &[
    Scalar {
        table: FONT_TABLE,
        key: "size",
        kind: ScalarKind::Float {
            range: FONT_SIZE_RANGE_PT,
            get: |config| config.font.size_pt,
            set: |config, size| config.font.size_pt = size,
        },
    },
    Scalar {
        table: FONT_TABLE,
        key: "wide_glyph",
        kind: ScalarKind::Name {
            names: &names_of(&WideGlyph::NAMES),
            get: |config| config.font.wide_glyph.name(),
            set: |config, name| set_named(&mut config.font.wide_glyph, WideGlyph::from_name(name)),
        },
    },
    Scalar {
        table: "paste",
        key: "confirm",
        kind: ScalarKind::Name {
            names: &names_of(&PasteConfirm::NAMES),
            get: |config| config.paste.confirm.name(),
            set: |config, name| set_named(&mut config.paste.confirm, PasteConfirm::from_name(name)),
        },
    },
    Scalar {
        table: "paste",
        key: "confirm_size",
        kind: ScalarKind::Int {
            range: PASTE_CONFIRM_SIZE_RANGE,
            get: |config| config.paste.confirm_size,
            set: |config, size| config.paste.confirm_size = size,
        },
    },
    Scalar {
        table: "clipboard",
        key: "read",
        kind: ScalarKind::Name {
            names: &names_of(&ClipboardRead::NAMES),
            get: |config| config.clipboard.read.name(),
            set: |config, name| {
                set_named(&mut config.clipboard.read, ClipboardRead::from_name(name))
            },
        },
    },
    Scalar {
        table: "clipboard",
        key: "write",
        kind: ScalarKind::Name {
            names: &names_of(&ClipboardWrite::NAMES),
            get: |config| config.clipboard.write.name(),
            set: |config, name| {
                set_named(&mut config.clipboard.write, ClipboardWrite::from_name(name))
            },
        },
    },
    Scalar {
        table: "scroll_bar",
        key: "width",
        kind: ScalarKind::Int {
            range: SCROLL_BAR_WIDTH_RANGE,
            get: |config| config.scroll_bar.width,
            set: |config, width| config.scroll_bar.width = width,
        },
    },
    Scalar {
        table: "window",
        key: "padding",
        kind: ScalarKind::Int {
            range: PADDING_RANGE,
            get: |config| config.window.padding,
            set: |config, padding| config.window.padding = padding,
        },
    },
    Scalar {
        table: COLORS_TABLE,
        key: "scheme",
        kind: ScalarKind::Name {
            names: &names_of(&ColorScheme::NAMES),
            get: |config| config.colors.scheme.name(),
            set: |config, name| set_named(&mut config.colors.scheme, ColorScheme::from_name(name)),
        },
    },
    Scalar {
        table: BACKGROUND_TABLE,
        key: IMAGE_KEYS.image_opacity,
        kind: ScalarKind::Float {
            range: IMAGE_OPACITY_RANGE,
            get: |config| config.background.image_opacity,
            set: |config, opacity| config.background.image_opacity = opacity,
        },
    },
    Scalar {
        table: BACKGROUND_TABLE,
        key: "fit",
        kind: ScalarKind::Name {
            names: &names_of(&BackgroundFit::NAMES),
            get: |config| config.background.fit.name(),
            set: |config, name| {
                set_named(&mut config.background.fit, BackgroundFit::from_name(name))
            },
        },
    },
];

/// 設定ダイアログの入力欄に書ける数の型。
pub trait InputNumber: Copy + PartialOrd + FromStr {
    /// 項目の種類 `kind` がこの型の数なら、その範囲。
    fn range(kind: &ScalarKind) -> Option<&RangeInclusive<Self>>;
}

impl InputNumber for f32 {
    fn range(kind: &ScalarKind) -> Option<&RangeInclusive<Self>> {
        match kind {
            ScalarKind::Float { range, .. } => Some(range),
            _ => None,
        }
    }
}

impl InputNumber for u32 {
    fn range(kind: &ScalarKind) -> Option<&RangeInclusive<Self>> {
        match kind {
            ScalarKind::Int { range, .. } => Some(range),
            _ => None,
        }
    }
}

/// 設定ダイアログの入力欄の文字列 `text` を、[`SCALARS`] の項目 `name`（`font.size` など）の値として
/// 読む。前後の空白は除く。`T` の数として読めないか、項目の範囲の外なら `None`。項目が `T` の数の
/// 項目でなくても `None`。
pub fn parse_input<T: InputNumber>(name: &str, text: &str) -> Option<T> {
    let scalar = SCALARS.iter().find(|scalar| scalar.is_named(name))?;
    let range = T::range(&scalar.kind)?;
    let value: T = text.trim().parse().ok()?;
    range.contains(&value).then_some(value)
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

    #[test]
    fn large_paste_is_decided_by_the_size_in_kib() {
        let config = |confirm_size| PasteConfig {
            confirm_size,
            ..PasteConfig::default()
        };
        assert_eq!(PasteConfig::default().confirm_size, 8);
        assert!(!config(8).is_large(8 * 1024 - 1));
        assert!(config(8).is_large(8 * 1024));
        assert!(config(1).is_large(1024));
        // 0 は、どの大きさでも確認しない。
        assert!(!config(0).is_large(usize::MAX));
        // 上限でも桁があふれない。
        assert!(!config(1_048_576).is_large(1024 * 1024 * 1024 - 1));
        assert!(config(1_048_576).is_large(1024 * 1024 * 1024));
    }
}
