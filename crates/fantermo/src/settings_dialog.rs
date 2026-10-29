//! 設定ダイアログ。左にページの一覧、右にそのページの項目のコントロールを並べる。
//!
//! 項目を変えると、その場で [`Host::preview`] に渡して端末に反映させる（入力欄は打ち終わるまで
//! [`PREVIEW_DELAY`] だけ待つ）。「OK」は設定ファイルに書いて閉じ、「適用」は書いて開いたまま、
//! 「キャンセル」（Esc、×）は最後に書いたとき（書いていなければ開いたとき）の設定に戻して閉じる。
//! 設定ファイルには、最後に書いたとき（または開いたとき）から変えた項目だけを書く。
//!
//! 開いている間は呼び出し元のウィンドウを無効にする（`modal`）。端末の出力や描き直しは、ダイアログのループ
//! から届く。

use std::{
    cell::{Cell, RefCell},
    ops::RangeInclusive,
    path::PathBuf,
    sync::OnceLock,
    time::Duration,
};

use fantermo_core::{
    background::IMAGE_EXTENSIONS,
    config::{
        self, BackendKind, BackgroundFit, BackgroundImage, ColorScheme, ColorTable, ColorsConfig,
        Config, PasteConfirm, Profile, WideGlyph, format_color,
    },
    frame::Rgb,
    keymap::{Action, KeyCombo, Keymap},
};
use windows::{
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            COLOR_BTNFACE, CreateFontIndirectW, CreateSolidBrush, DeleteObject, HBRUSH, HFONT,
            HGDIOBJ, InvalidateRect,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{
                BST_CHECKED, CB_SETMINVISIBLE,
                Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW},
                ICC_UPDOWN_CLASS, INITCOMMONCONTROLSEX, InitCommonControlsEx, NMHDR, NMUPDOWN,
                UDM_SETBUDDY, UDM_SETPOS32, UDM_SETRANGE32, UDN_DELTAPOS, UDS_ALIGNRIGHT,
                UDS_ARROWKEYS, UDS_HOTTRACK, UDS_NOTHOUSANDS, UPDOWN_CLASSW, WC_COMBOBOXW,
                WC_LISTBOXW,
            },
            HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow},
            Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled, SetFocus},
            WindowsAndMessaging::{
                BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON,
                BS_GROUPBOX, BS_PUSHBUTTON, CB_ADDSTRING, CB_DELETESTRING, CB_GETCURSEL,
                CB_INSERTSTRING, CB_SETCURSEL, CBN_SELCHANGE, CBS_DROPDOWNLIST, CW_USEDEFAULT,
                CreateWindowExW, DC_HASDEFID, DM_GETDEFID, DefWindowProcW, DestroyWindow,
                EN_CHANGE, ES_AUTOHSCROLL, GWLP_USERDATA, GetWindowLongPtrW, GetWindowRect,
                GetWindowTextLengthW, GetWindowTextW, HMENU, IDC_ARROW, IDCANCEL, IDOK, IDYES,
                KillTimer, LB_ADDSTRING, LB_DELETESTRING, LB_GETCURSEL, LB_INSERTSTRING,
                LB_RESETCONTENT, LB_SETCURSEL, LB_SETTABSTOPS, LBN_DBLCLK, LBN_SELCHANGE,
                LBS_NOTIFY, LBS_USETABSTOPS, LoadCursorW, MB_ICONQUESTION, MB_ICONWARNING, MB_OK,
                MB_YESNO, MessageBoxW, MoveWindow, RegisterClassW, SW_HIDE, SW_SHOW,
                SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetTimer, SetWindowLongPtrW,
                SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE,
                WM_COMMAND, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DPICHANGED, WM_NEXTDLGCTL, WM_NOTIFY,
                WM_SETFONT, WM_TIMER, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE,
                WS_EX_CONTROLPARENT, WS_EX_DLGMODALFRAME, WS_GROUP, WS_POPUP, WS_SYSMENU,
                WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{file_dialog, key_capture, modal, scroll_bar, ui_font};

const CLASS: PCWSTR = w!("fantermo-settings-dialog");
const TITLE: &str = "設定";

/// 入力欄を打ってから、端末に反映するまでの間。この間に打ったものはまとめる。
const PREVIEW_DELAY: Duration = Duration::from_millis(400);
/// 入力欄を打ったあと、端末に反映するまでのタイマー。
const PREVIEW_TIMER_ID: usize = 1;

/// コントロールの ID。ボタンは IDOK と IDCANCEL も使う。
const ID_PAGES: i32 = 100;
const ID_APPLY: i32 = 101;
const ID_ERROR: i32 = 102;
const ID_FONT: i32 = 200;
const ID_SIZE: i32 = 201;
const ID_SIZE_SPIN: i32 = 202;
/// `font.wide_glyph` のラジオボタン。[`WideGlyph::ALL`] の順に ID が続く。
const ID_WIDE_GLYPH: i32 = 210;
const ID_SCROLL_BAR_WIDTH: i32 = 220;
const ID_SCROLL_BAR_WIDTH_SPIN: i32 = 221;
const ID_PADDING: i32 = 230;
const ID_PADDING_SPIN: i32 = 231;
/// `paste.confirm` のラジオボタン。[`PasteConfirm::ALL`] の順に ID が続く。
const ID_PASTE_CONFIRM: i32 = 300;
const ID_PROFILES: i32 = 400;
const ID_PROFILE_ADD: i32 = 401;
const ID_PROFILE_REMOVE: i32 = 402;
const ID_PROFILE_DEFAULT: i32 = 403;
const ID_PROFILE_NAME: i32 = 404;
const ID_PROFILE_COMMAND: i32 = 405;
const ID_BACKGROUND_IMAGES: i32 = 450;
const ID_BACKGROUND_ADD_FILES: i32 = 451;
const ID_BACKGROUND_ADD_FOLDER: i32 = 452;
const ID_BACKGROUND_REMOVE: i32 = 453;
const ID_BACKGROUND_OPACITY: i32 = 454;
const ID_BACKGROUND_OPACITY_SPIN: i32 = 455;
const ID_BACKGROUND_FIT: i32 = 456;
const ID_BACKGROUND_ITEM_FIT: i32 = 457;
const ID_BACKGROUND_ITEM_OPACITY: i32 = 458;
const ID_BACKGROUND_ITEM_OPACITY_SPIN: i32 = 459;
/// 項目の不透明度の「既定」と「指定」のラジオボタン。
const ID_BACKGROUND_ITEM_OPACITY_DEFAULT: i32 = 460;
const ID_BACKGROUND_ITEM_OPACITY_CUSTOM: i32 = 461;
const ID_KEYS: i32 = 500;
const ID_KEY_ADD: i32 = 501;
const ID_KEY_CHANGE: i32 = 502;
const ID_KEY_REMOVE: i32 = 503;
const ID_KEY_RESET: i32 = 504;
/// 見出しや説明の文字。押せないので同じ ID でよい。
const ID_LABEL: i32 = -1;

/// 96 DPI のときの寸法（ピクセル）。ウィンドウの DPI に合わせて拡大する。
const BASE_DPI: u32 = 96;
const CLIENT_WIDTH: i32 = 640;
const CLIENT_HEIGHT: i32 = 490;
const MARGIN: i32 = 12;
const PAGES_WIDTH: i32 = 120;
/// ページの見出しの列の幅。コントロールはその右に置く。
const LABEL_WIDTH: i32 = 150;
const ROW_HEIGHT: i32 = 24;
/// 見出しの文字を、横のコントロールの文字と高さを揃えるための下げ幅。
const LABEL_OFFSET: i32 = 4;
const BUTTON_WIDTH: i32 = 88;
const BUTTON_HEIGHT: i32 = 28;
const BUTTON_GAP: i32 = 8;
const NUMBER_WIDTH: i32 = 72;
/// 上下の矢印のボタンの範囲（下限、上限）。上の矢印で増える向きにするためのもの。
const SPIN_RANGE: (i32, i32) = (0, 1000);
/// フォントの一覧を開いたときに見せる行の数。
const VISIBLE_FONTS: usize = 16;

/// ページ。一覧の順。
const PAGES: [&str; 6] = ["外観", "背景", "動作", "プロファイル", "キー", "色"];

/// プロファイルの一覧の高さ。
const PROFILE_LIST_HEIGHT: i32 = 170;
/// プロファイルの一覧の、コマンドラインの列の位置（ダイアログの単位）。
const PROFILE_COLUMN_TAB_STOP: i32 = 120;
/// `EM_SETSEL` と、すべてを選ぶときの範囲。
const EM_SETSEL: u32 = 0x00B1;
const SELECT_FROM_START: usize = 0;
const SELECT_TO_END: isize = -1;
/// 足したプロファイルの名前。同じ名前があれば後ろに番号を付ける。
const NEW_PROFILE_NAME: &str = "新しいプロファイル";

/// プロファイルの一覧の行。名前（既定なら印を添える）とコマンドラインをタブで区切る。
fn profile_row_text(profile: &Profile, is_default: bool) -> String {
    let mark = if is_default { "（既定）" } else { "" };
    format!("{}{mark}\t{}", profile.name, profile.command)
}

/// `profiles` のどれとも違う、足すプロファイルの名前。
fn new_profile_name(profiles: &[Profile]) -> String {
    let taken = |name: &str| profiles.iter().any(|profile| profile.name == name);
    if !taken(NEW_PROFILE_NAME) {
        return NEW_PROFILE_NAME.to_owned();
    }
    (2..)
        .map(|number| format!("{NEW_PROFILE_NAME} {number}"))
        .find(|name| !taken(name))
        .expect("some number is free")
}

/// プロファイルの入力欄。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProfileField {
    Name,
    Command,
}

/// 編集中のプロファイル `profiles` を設定ファイルと同じ決まりで確かめる。名前とコマンドラインが空で
/// なく、名前がどれも違えば `Ok`。そうでなければ、最初のエラーの位置と入力欄とエラーメッセージ。
fn check_profiles(profiles: &[Profile]) -> Result<(), (usize, ProfileField, &'static str)> {
    for (index, profile) in profiles.iter().enumerate() {
        if profile.name.trim().is_empty() {
            return Err((
                index,
                ProfileField::Name,
                "プロファイルの名前を書いてください。",
            ));
        }
        if profile.command.trim().is_empty() {
            return Err((
                index,
                ProfileField::Command,
                "プロファイルのコマンドラインを書いてください。",
            ));
        }
        if profiles[..index]
            .iter()
            .any(|other| other.name == profile.name)
        {
            return Err((
                index,
                ProfileField::Name,
                "ほかのプロファイルと同じ名前です。",
            ));
        }
    }
    Ok(())
}

/// 既定のプロファイルを `profiles` の `index` 番目にしたときの `default_profile`。先頭で、開いたときも
/// 書いていなかった（`base` が `None`）なら書かない。
fn default_profile_name(profiles: &[Profile], index: usize, base: Option<&str>) -> Option<String> {
    if index == 0 && base.is_none() {
        return None;
    }
    profiles.get(index).map(|profile| profile.name.clone())
}

/// 背景のページの枠（グループボックス）の、枠の線から中のコントロールまでの幅。上は見出しの分を足す。
const GROUP_PADDING: i32 = 10;
const GROUP_CAPTION_HEIGHT: i32 = 20;
/// 背景のページの枠の中の、見出しの列の幅。コントロールはその右に置く。
const BACKGROUND_LABEL_WIDTH: i32 = 190;
/// 背景画像の一覧の高さと、その右に並べるボタンの幅。
const BACKGROUND_LIST_HEIGHT: i32 = 106;
const BACKGROUND_BUTTON_WIDTH: i32 = 124;
/// 項目の不透明度の「指定」のラジオボタンの幅。入力欄はその右に置く。
const CUSTOM_OPACITY_WIDTH: i32 = 56;
/// 背景画像の不透明度を、上下の矢印で動かす幅。
const OPACITY_STEP: f32 = 0.05;

/// `background.fit` の選択肢の表示。[`BackgroundFit::ALL`] の順。Windows の壁紙の設定と同じ語。
const BACKGROUND_FIT_LABELS: [&str; 5] = [
    "ページ幅に合わせる",
    "画面のサイズに合わせる",
    "拡大して表示",
    "並べて表示",
    "中央に表示",
];

/// `fit` の表示。
fn fit_label(fit: BackgroundFit) -> &'static str {
    BackgroundFit::ALL
        .iter()
        .position(|&value| value == fit)
        .map_or("", |index| BACKGROUND_FIT_LABELS[index])
}

/// 一覧の項目の値を `[background]` の値に任せることの表示。`value` は任せる先の値の表示。
fn default_choice_label(value: &str) -> String {
    format!("既定（{value}）")
}

/// 一覧の項目の表示方法のコンボボックスで、`fit` の位置。先頭は既定に任せること、続いて
/// [`BackgroundFit::ALL`] の順。
fn item_fit_choice(fit: Option<BackgroundFit>) -> usize {
    fit.and_then(|fit| BackgroundFit::ALL.iter().position(|&value| value == fit))
        .map_or(0, |index| index + 1)
}

/// 一覧の項目の表示方法のコンボボックスの `choice` 番目の値。範囲の外なら `None`。
fn item_fit_from_choice(choice: usize) -> Option<Option<BackgroundFit>> {
    match choice {
        0 => Some(None),
        _ => BackgroundFit::ALL.get(choice - 1).copied().map(Some),
    }
}

/// 背景画像の一覧の行。パスと、項目に書いた表示方法と不透明度（「C:\a.png（並べて表示、不透明度
/// 0.3）」）。
fn background_row_text(image: &BackgroundImage) -> String {
    let settings: Vec<String> = image
        .fit
        .map(|fit| fit_label(fit).to_owned())
        .into_iter()
        .chain(
            image
                .image_opacity
                .map(|opacity| format!("不透明度 {}", format_opacity(opacity))),
        )
        .collect();
    if settings.is_empty() {
        return image.path.clone();
    }
    format!("{}（{}）", image.path, settings.join("、"))
}

/// 色のページのコントロールの ID の始まり。色ごとに [`COLOR_ID_STRIDE`] ずつ、見本・値・選ぶ・既定に
/// 戻すの順に続く。
const ID_COLORS: i32 = 600;
const COLOR_ID_STRIDE: i32 = 4;
const COLOR_SWATCH: i32 = 0;
const COLOR_CHOOSE: i32 = 2;
const COLOR_RESET: i32 = 3;
/// 色のページの一行の高さ。
const COLOR_ROW_HEIGHT: i32 = 22;
/// 色の名前を、表の見出しより右へ寄せる幅。
const COLOR_LABEL_INDENT: i32 = 4;
/// 色の見本の幅と、値の文字の幅。
const SWATCH_WIDTH: i32 = 32;
const COLOR_TEXT_WIDTH: i32 = 110;
const CHOOSE_WIDTH: i32 = 72;
const COLOR_RESET_WIDTH: i32 = 88;

/// 色の表の見出し。[`ColorTable::ALL`] の順。
const COLOR_TABLE_LABELS: [&str; 3] = ["端末", "タブバー", "スクロールバー"];

/// `colors.scheme` のラジオボタン。[`ColorScheme::ALL`] の順に ID が続く。
const ID_COLOR_SCHEME: i32 = 590;
/// `colors.scheme` の選択肢の表示。[`ColorScheme::ALL`] の順。
const COLOR_SCHEME_LABELS: [&str; 2] = ["ダーク", "ライト"];
/// 配色のラジオボタン一つの幅。
const COLOR_SCHEME_WIDTH: i32 = 96;

/// 端末の色の表示。[`config::TERMINAL_COLOR_KEYS`] の順。
const TERMINAL_COLOR_LABELS: [&str; 2] = ["背景色", "文字の色"];

/// タブバーの色の表示。[`config::TAB_BAR_COLOR_KEYS`] の順。
const TAB_BAR_COLOR_LABELS: [&str; 9] = [
    "背景",
    "表示しているタブ",
    "表示しているタブの縁",
    "表示しているタブの文字",
    "表示していないタブの文字",
    "タブの区切りの線",
    "マウスを載せているタブ",
    "「+」「▾」「≡」",
    "ベルの印",
];

/// スクロールバーの色の表示。[`config::SCROLL_BAR_COLOR_KEYS`] の順。
const SCROLL_BAR_COLOR_LABELS: [&str; 4] = [
    "トラック",
    "つまみ",
    "マウスを載せているつまみ",
    "ドラッグしているつまみ",
];

fn color_labels(table: ColorTable) -> &'static [&'static str] {
    match table {
        ColorTable::Terminal => &TERMINAL_COLOR_LABELS,
        ColorTable::TabBar => &TAB_BAR_COLOR_LABELS,
        ColorTable::ScrollBar => &SCROLL_BAR_COLOR_LABELS,
    }
}

/// 色のページの色の並び。表と、表の中の位置。
fn color_items() -> Vec<(ColorTable, usize)> {
    ColorTable::ALL
        .iter()
        .flat_map(|&table| (0..table.keys().len()).map(move |index| (table, index)))
        .collect()
}

/// 色を Win32 の `COLORREF`（`0x00BBGGRR`）にする。
fn colorref(color: Rgb) -> COLORREF {
    COLORREF(u32::from(color.r) | (u32::from(color.g) << 8) | (u32::from(color.b) << 16))
}

/// Win32 の `COLORREF` を色にする。
fn rgb_of(colorref: COLORREF) -> Rgb {
    let [r, g, b, _] = colorref.0.to_le_bytes();
    Rgb { r, g, b }
}

/// 色 `colors` のときの、表 `table` の `index` 番目の既定の色（設定に書いていないときに使う色）。
/// 端末の色は配色から、スクロールバーの色は端末の背景色と文字の色から、タブバーの色は
/// `tab_bar_defaults`（システムの色）から決める。
fn default_color(
    colors: &ColorsConfig,
    tab_bar_defaults: &[Rgb; config::TAB_BAR_COLOR_KEYS.len()],
    table: ColorTable,
    index: usize,
) -> Rgb {
    match table {
        ColorTable::Terminal => {
            let scheme = colors.scheme.colors();
            [scheme.background, scheme.foreground][index]
        }
        ColorTable::TabBar => tab_bar_defaults[index],
        ColorTable::ScrollBar => {
            let terminal = colors.terminal_colors();
            scroll_bar::Palette::from_terminal(terminal.background, terminal.foreground).colors()
                [index]
        }
    }
}

/// 開いたときに見せるページ。
const PAGE_APPEARANCE: usize = 0;

/// `font.wide_glyph` の選択肢の表示。[`WideGlyph::ALL`] の順。
const WIDE_GLYPH_LABELS: [&str; WideGlyph::ALL.len()] = [
    "横に縮めてセルに収める",
    "縮めずに、はみ出しを切る",
    "縮めずに、右のセルへはみ出す",
    "縮めずに、真ん中から左右のセルへはみ出す",
];

/// `paste.confirm` の選択肢の表示。[`PasteConfirm::ALL`] の順。
const PASTE_CONFIRM_LABELS: [&str; 3] = [
    "コマンドとして実行されうるとき",
    "改行を含むとき",
    "確かめない",
];

/// キーの割り当ての一覧の、組み合わせの列の位置（ダイアログの単位）。
const KEY_COLUMN_TAB_STOP: i32 = 110;

/// キーの割り当ての動作の表示。[`Action::ALL`] の順。
const ACTION_LABELS: [&str; 7] = [
    "コピー",
    "貼り付け",
    "コンテキストメニュー",
    "新しいタブ",
    "タブを閉じる",
    "次のタブ",
    "前のタブ",
];

fn action_label(action: Action) -> &'static str {
    let index = Action::ALL
        .iter()
        .position(|&candidate| candidate == action)
        .expect("every action is listed");
    ACTION_LABELS[index]
}

/// キーの割り当ての一覧の一行。動作と組み合わせ。組み合わせを割り当てていない動作は `None` で一行。
type KeyRow = (Action, Option<KeyCombo>);

/// キーの割り当て `keys` の一覧の行。動作の順に、割り当てた組み合わせを一行ずつ並べる。
fn key_rows(keys: &Keymap) -> Vec<KeyRow> {
    let mut rows = Vec::new();
    for action in Action::ALL {
        let combos = keys.combos_of(action);
        if combos.is_empty() {
            rows.push((action, None));
        }
        rows.extend(combos.into_iter().map(|combo| (action, Some(combo))));
    }
    rows
}

/// 一覧の行の文字。動作と組み合わせをタブで区切る。
fn key_row_text((action, combo): KeyRow) -> String {
    let combo = combo.map_or_else(|| "（なし）".to_owned(), |combo| combo.label());
    format!("{}\t{combo}", action_label(action))
}

/// 動作 `action` の割り当てが、既定の割り当てと同じか。
fn is_default_binding(keys: &Keymap, action: Action) -> bool {
    let mut current = keys.combos_of(action);
    let mut default = Keymap::default().combos_of(action);
    current.sort_by_key(KeyCombo::to_string);
    default.sort_by_key(KeyCombo::to_string);
    current == default
}

/// ダイアログの外の、端末と設定ファイルを受け持つもの。
pub trait Host {
    /// 編集中の設定 `config` を端末に反映する。
    fn preview(&mut self, config: &Config);
    /// 設定ファイルに、`base` から `edited` へ変えた項目を書く。書けなければ、利用者に見せる文を返す。
    fn save(&mut self, base: &Config, edited: &Config) -> Result<(), String>;
}

/// フォントの一覧の一つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontChoice {
    /// 見せる名前。選んだら、この名前を設定に書く。
    pub name: String,
    /// システムのフォントの中の番号。同じ書体の別の名前を見分けるのに使う。
    pub index: Option<u32>,
}

/// ダイアログを開くときに渡すもの。
pub struct Request {
    /// いま使っている設定。
    pub current: Config,
    /// `current.font.families` のそれぞれの、システムのフォントの中の番号（無ければ `None`）。
    pub family_indices: Vec<Option<u32>>,
    /// フォントの一覧（等幅の書体）。
    pub fonts: Vec<FontChoice>,
    /// いま使っているフォント。一覧に無ければ、一覧の先頭に足す。
    pub font_in_use: FontChoice,
    /// 色のページで、書いていないタブバーの色として見せる色（システムの色）。
    pub tab_bar_colors: [Rgb; config::TAB_BAR_COLOR_KEYS.len()],
}

/// 設定ダイアログを開き、閉じるまで待つ。
pub fn run(owner: HWND, request: Request, host: &mut dyn Host) {
    if let Err(e) = register_class() {
        tracing::warn!("failed to register the settings dialog: {e}");
        return;
    }
    let Request {
        current,
        family_indices,
        mut fonts,
        font_in_use,
        tab_bar_colors,
    } = request;
    let selected_font = match fonts.iter().position(|font| {
        font.index.is_some() && font.index == font_in_use.index || font.name == font_in_use.name
    }) {
        Some(index) => index,
        None => {
            fonts.insert(0, font_in_use);
            0
        }
    };
    let (hwnd, controls) = match create(owner) {
        Ok(created) => created,
        Err(e) => {
            tracing::warn!("failed to open the settings dialog: {e}");
            return;
        }
    };
    let state = State {
        hwnd,
        controls,
        fonts,
        host: RefCell::new(host),
        base: RefCell::new(current.clone()),
        base_indices: RefCell::new(family_indices),
        base_font: Cell::new(selected_font),
        previewed: RefCell::new(current.clone()),
        background_images: RefCell::new(current.background.images.clone()),
        profiles: RefCell::new(current.profiles.clone()),
        default_profile: Cell::new(current.default_profile_index()),
        filling_profile: Cell::new(false),
        keys: RefCell::new(current.keys.clone()),
        key_rows: RefCell::new(Vec::new()),
        colors: RefCell::new(current.colors.clone()),
        tab_bar_defaults: tab_bar_colors,
        swatch_brushes: RefCell::new(vec![HBRUSH::default(); color_items().len()]),
        custom_colors: Cell::new([COLORREF(0); CUSTOM_COLORS]),
        font: Cell::new(HFONT::default()),
        dpi: Cell::new(BASE_DPI),
        closed: Cell::new(false),
    };
    state.fill(&current, selected_font);
    // SAFETY: hwnd は作ったばかりのウィンドウ。ポインタは modal::run が戻るまで有効で、WM_DESTROY で外す。
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, std::ptr::from_ref(&state) as isize) };
    // SAFETY: owner は呼び出し元の有効なウィンドウ。
    let dpi = unsafe { GetDpiForWindow(owner) };
    state.set_dpi(dpi);
    if let Err(e) = place(owner, hwnd, dpi) {
        tracing::warn!("failed to place the settings dialog: {e}");
    }
    state.show_page(PAGE_APPEARANCE);
    // SAFETY: hwnd と一覧は有効なウィンドウ。
    unsafe {
        _ = ShowWindow(hwnd, SW_SHOW);
        _ = SetFocus(Some(state.controls.pages));
    }

    modal::run(owner, hwnd, &state.closed, |_| false);

    let font = state.font.get();
    if !font.is_invalid() {
        // SAFETY: フォントはダイアログのために作ったもので、ダイアログはもう閉じている。
        unsafe { _ = DeleteObject(HGDIOBJ(font.0)) };
    }
    for brush in state.swatch_brushes.take() {
        if !brush.is_invalid() {
            // SAFETY: ブラシは色の見本のために作ったもので、ダイアログはもう閉じている。
            unsafe { _ = DeleteObject(HGDIOBJ(brush.0)) };
        }
    }
}

fn register_class() -> windows::core::Result<()> {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    if REGISTERED.get().is_some() {
        return Ok(());
    }
    let controls = INITCOMMONCONTROLSEX {
        dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_UPDOWN_CLASS,
    };
    // SAFETY: controls は大きさを入れて初期化してある。
    if !unsafe { InitCommonControlsEx(&controls) }.as_bool() {
        tracing::warn!("failed to register the up-down control");
    }
    // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
    let instance = unsafe { GetModuleHandleW(None)? };
    let class = WNDCLASSW {
        lpfnWndProc: Some(dialog_proc),
        hInstance: instance.into(),
        // SAFETY: システムの矢印カーソルを読むだけ。
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        // システムの色の番号に 1 を足した値は、その色のブラシとして扱われる。
        hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as isize as *mut _),
        lpszClassName: CLASS,
        ..Default::default()
    };
    // SAFETY: class は初期化済みで、名前は静的な文字列。
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    _ = REGISTERED.set(());
    Ok(())
}

/// 96 DPI のときの矩形（左、上、幅、高さ）。
type DipRect = [i32; 4];

/// ダイアログのコントロール。
struct Controls {
    pages: HWND,
    /// 各ページに置くコントロール。[`PAGES`] の順。
    page_controls: Vec<Vec<HWND>>,
    font: HWND,
    size: HWND,
    size_spin: HWND,
    wide_glyph: [HWND; WideGlyph::ALL.len()],
    scroll_bar_width: HWND,
    scroll_bar_width_spin: HWND,
    padding: HWND,
    padding_spin: HWND,
    paste_confirm: [HWND; 3],
    /// プロファイルの一覧。
    profiles_list: HWND,
    /// 削除・既定にするのボタン。
    profile_remove: HWND,
    profile_default: HWND,
    /// 一覧で選んでいるプロファイルの名前とコマンドライン。
    profile_name: HWND,
    profile_command: HWND,
    /// 背景画像の一覧。
    background_images: HWND,
    /// 背景画像の一覧の「削除」のボタン。
    background_remove: HWND,
    /// 既定の不透明度と表示方法。
    background_opacity: HWND,
    background_opacity_spin: HWND,
    background_fit: HWND,
    /// 一覧で選んでいる項目の表示方法。
    background_item_fit: HWND,
    /// 一覧で選んでいる項目の不透明度の「既定」と「指定」のラジオボタンと、「指定」の値。
    background_item_opacity_default: HWND,
    background_item_opacity_custom: HWND,
    background_item_opacity: HWND,
    background_item_opacity_spin: HWND,
    /// 配色のラジオボタン。[`ColorScheme::ALL`] の順。
    color_scheme: [HWND; 2],
    /// キーの割り当ての一覧。
    keys_list: HWND,
    /// 追加・変更・外す・既定に戻すのボタン。
    key_buttons: [HWND; 4],
    /// 色のページの行。[`color_items`] の順。
    color_rows: Vec<ColorRow>,
    error: HWND,
    /// すべてのコントロールと、96 DPI のときの矩形。上下の矢印のボタンは入れない（相手の入力欄に
    /// 付けて置く）。
    placed: Vec<(HWND, DipRect)>,
}

/// ダイアログの状態。ウィンドウプロシージャからも触るので、共有の参照だけで読み書きする。
struct State<'a> {
    hwnd: HWND,
    controls: Controls,
    fonts: Vec<FontChoice>,
    host: RefCell<&'a mut dyn Host>,
    /// 最後に設定ファイルに書いたとき（書いていなければ開いたとき）の設定。キャンセルでここへ戻す。
    base: RefCell<Config>,
    /// `base.font.families` のそれぞれの、システムのフォントの中の番号。
    base_indices: RefCell<Vec<Option<u32>>>,
    /// `base` のときに選んでいたフォントの、一覧の中の位置。
    base_font: Cell<usize>,
    /// 最後に端末に反映した設定。
    previewed: RefCell<Config>,
    /// 編集中の背景画像の一覧。
    background_images: RefCell<Vec<BackgroundImage>>,
    /// 編集中のプロファイル。
    profiles: RefCell<Vec<Profile>>,
    /// 編集中の既定のプロファイルの、`profiles` の中の位置。
    default_profile: Cell<usize>,
    /// プロファイルの名前とコマンドラインの入力欄に、選んだプロファイルの値を入れている（その間に届く
    /// EN_CHANGE を編集とみなさない）。
    filling_profile: Cell<bool>,
    /// 編集中のキーの割り当て。
    keys: RefCell<Keymap>,
    /// キーの割り当ての一覧の行。一覧の順。
    key_rows: RefCell<Vec<KeyRow>>,
    font: Cell<HFONT>,
    dpi: Cell<u32>,
    /// 編集中の色。
    colors: RefCell<ColorsConfig>,
    /// 書いていないタブバーの色として見せる色。
    tab_bar_defaults: [Rgb; config::TAB_BAR_COLOR_KEYS.len()],
    /// 色の見本を塗るブラシ。[`color_items`] の順。
    swatch_brushes: RefCell<Vec<HBRUSH>>,
    /// 色を選ぶダイアログの「作成した色」。ダイアログを開いている間は覚えておく。
    custom_colors: Cell<[COLORREF; CUSTOM_COLORS]>,
    closed: Cell<bool>,
}

/// 色を選ぶダイアログの「作成した色」の数。
const CUSTOM_COLORS: usize = 16;

/// 色のページの一行のコントロール。
struct ColorRow {
    swatch: HWND,
    text: HWND,
    reset: HWND,
}

/// ダイアログとコントロールを作る。大きさと位置は後で決める（[`place`]）。
fn create(owner: HWND) -> windows::core::Result<(HWND, Controls)> {
    // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
    let instance = unsafe { GetModuleHandleW(None)? };
    // SAFETY: クラスは登録済みで、文字列はこの呼び出しの間は生きている。
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            CLASS,
            &HSTRING::from(TITLE),
            WS_POPUP | WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CLIENT_WIDTH,
            CLIENT_HEIGHT,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        )?
    };
    let mut placed = Vec::new();
    let mut child = |class: PCWSTR,
                     text: &str,
                     style: WINDOW_STYLE,
                     ex_style: WINDOW_EX_STYLE,
                     id: i32,
                     rect: Option<DipRect>|
     -> windows::core::Result<HWND> {
        // SAFETY: hwnd は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
        let control = unsafe {
            CreateWindowExW(
                ex_style,
                class,
                &HSTRING::from(text),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(id as isize as *mut _)),
                Some(instance.into()),
                None,
            )?
        };
        if let Some(rect) = rect {
            placed.push((control, rect));
        }
        Ok(control)
    };
    let none = WINDOW_EX_STYLE(0);
    let label = |text: &str, rect: DipRect| (w!("STATIC"), text.to_owned(), rect);
    let radio = |first: bool| {
        let group = if first {
            WS_GROUP | WS_TABSTOP
        } else {
            WINDOW_STYLE(0)
        };
        group | WINDOW_STYLE(BS_AUTORADIOBUTTON as u32)
    };
    let edit_style = WS_GROUP | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32);
    let spin_style = WINDOW_STYLE(UDS_ALIGNRIGHT | UDS_ARROWKEYS | UDS_NOTHOUSANDS | UDS_HOTTRACK);

    let page_left = MARGIN + PAGES_WIDTH + MARGIN;
    let page_width = CLIENT_WIDTH - page_left - MARGIN;
    let control_left = page_left + LABEL_WIDTH;
    let control_width = CLIENT_WIDTH - control_left - MARGIN;
    let button_top = CLIENT_HEIGHT - MARGIN - BUTTON_HEIGHT;
    let row = |index: i32| MARGIN + index * (ROW_HEIGHT + MARGIN);
    let label_rect = |top: i32| [page_left, top + LABEL_OFFSET, LABEL_WIDTH, ROW_HEIGHT];

    let pages = child(
        WC_LISTBOXW,
        "",
        WS_GROUP | WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(LBS_NOTIFY as u32),
        WS_EX_CLIENTEDGE,
        ID_PAGES,
        Some([MARGIN, MARGIN, PAGES_WIDTH, button_top - 2 * MARGIN]),
    )?;
    for page in PAGES {
        // SAFETY: pages は有効な一覧で、文字列はこの呼び出しの間は生きている。
        unsafe {
            SendMessageW(
                pages,
                LB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(page).as_ptr() as isize)),
            )
        };
    }

    // 外観
    let mut appearance = Vec::new();
    let (class, text, rect) = label("フォント(&F):", label_rect(row(0)));
    appearance.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let font = child(
        WC_COMBOBOXW,
        "",
        WS_GROUP | WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
        none,
        ID_FONT,
        Some([control_left, row(0), control_width, ROW_HEIGHT]),
    )?;
    appearance.push(font);
    // SAFETY: font は有効なコンボボックス。
    unsafe { SendMessageW(font, CB_SETMINVISIBLE, Some(WPARAM(VISIBLE_FONTS)), None) };
    let (class, text, rect) = label("大きさ(&S):", label_rect(row(1)));
    appearance.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let size = child(
        w!("EDIT"),
        "",
        edit_style,
        WS_EX_CLIENTEDGE,
        ID_SIZE,
        Some([control_left, row(1), NUMBER_WIDTH, ROW_HEIGHT]),
    )?;
    appearance.push(size);
    let size_spin = child(UPDOWN_CLASSW, "", spin_style, none, ID_SIZE_SPIN, None)?;
    appearance.push(size_spin);
    let (class, text, rect) = label(
        "ポイント（1〜128）",
        [
            control_left + NUMBER_WIDTH + BUTTON_GAP,
            row(1) + LABEL_OFFSET,
            control_width - NUMBER_WIDTH - BUTTON_GAP,
            ROW_HEIGHT,
        ],
    );
    appearance.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let (class, text, rect) = label("セルより広い文字(&W):", label_rect(row(2)));
    appearance.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let mut wide_glyph = [HWND::default(); WideGlyph::ALL.len()];
    for (index, text) in WIDE_GLYPH_LABELS.iter().enumerate() {
        let top = row(2) + index as i32 * ROW_HEIGHT;
        wide_glyph[index] = child(
            w!("BUTTON"),
            text,
            radio(index == 0),
            none,
            ID_WIDE_GLYPH + index as i32,
            Some([control_left, top, control_width, ROW_HEIGHT]),
        )?;
        appearance.push(wide_glyph[index]);
    }
    let width_row = row(2) + WideGlyph::ALL.len() as i32 * ROW_HEIGHT + MARGIN;
    let (class, text, rect) = label("スクロールバーの幅(&B):", label_rect(width_row));
    appearance.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let scroll_bar_width = child(
        w!("EDIT"),
        "",
        edit_style,
        WS_EX_CLIENTEDGE,
        ID_SCROLL_BAR_WIDTH,
        Some([control_left, width_row, NUMBER_WIDTH, ROW_HEIGHT]),
    )?;
    appearance.push(scroll_bar_width);
    let scroll_bar_width_spin = child(
        UPDOWN_CLASSW,
        "",
        spin_style,
        none,
        ID_SCROLL_BAR_WIDTH_SPIN,
        None,
    )?;
    appearance.push(scroll_bar_width_spin);
    let (class, text, rect) = label(
        "ピクセル（4〜64。画面の拡大率に合わせて広げる）",
        [
            control_left + NUMBER_WIDTH + BUTTON_GAP,
            width_row + LABEL_OFFSET,
            control_width - NUMBER_WIDTH - BUTTON_GAP,
            ROW_HEIGHT,
        ],
    );
    appearance.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let padding_row = width_row + ROW_HEIGHT + MARGIN;
    let (class, text, rect) = label("余白(&G):", label_rect(padding_row));
    appearance.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let padding = child(
        w!("EDIT"),
        "",
        edit_style,
        WS_EX_CLIENTEDGE,
        ID_PADDING,
        Some([control_left, padding_row, NUMBER_WIDTH, ROW_HEIGHT]),
    )?;
    appearance.push(padding);
    let padding_spin = child(UPDOWN_CLASSW, "", spin_style, none, ID_PADDING_SPIN, None)?;
    appearance.push(padding_spin);
    let (class, text, rect) = label(
        "ピクセル（0〜64。画面の拡大率に合わせて広げる）",
        [
            control_left + NUMBER_WIDTH + BUTTON_GAP,
            padding_row + LABEL_OFFSET,
            control_width - NUMBER_WIDTH - BUTTON_GAP,
            ROW_HEIGHT,
        ],
    );
    appearance.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);

    // 背景。上に既定の値の枠、下に背景画像の一覧と選択した項目の値の枠。
    let mut background_page = Vec::new();
    let group_style = WINDOW_STYLE(BS_GROUPBOX as u32);
    let inner_left = page_left + GROUP_PADDING;
    let inner_width = page_width - 2 * GROUP_PADDING;
    let background_control_left = inner_left + BACKGROUND_LABEL_WIDTH;
    let background_control_width = inner_left + inner_width - background_control_left;
    let background_label_rect = |top: i32| {
        [
            inner_left,
            top + LABEL_OFFSET,
            BACKGROUND_LABEL_WIDTH,
            ROW_HEIGHT,
        ]
    };
    let opacity_note_rect = |top: i32, left: i32| {
        [
            left + NUMBER_WIDTH + BUTTON_GAP,
            top + LABEL_OFFSET,
            inner_left + inner_width - left - NUMBER_WIDTH - BUTTON_GAP,
            ROW_HEIGHT,
        ]
    };

    let defaults_top = row(0);
    let fit_row = defaults_top + GROUP_CAPTION_HEIGHT;
    let opacity_row = fit_row + ROW_HEIGHT + BUTTON_GAP;
    let defaults_bottom = opacity_row + ROW_HEIGHT + GROUP_PADDING;
    background_page.push(child(
        w!("BUTTON"),
        "既定",
        group_style,
        none,
        ID_LABEL,
        Some([
            page_left,
            defaults_top,
            page_width,
            defaults_bottom - defaults_top,
        ]),
    )?);
    let (class, text, rect) = label("表示方法(&T):", background_label_rect(fit_row));
    background_page.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let background_fit = child(
        WC_COMBOBOXW,
        "",
        WS_GROUP | WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
        none,
        ID_BACKGROUND_FIT,
        Some([
            background_control_left,
            fit_row,
            background_control_width,
            ROW_HEIGHT,
        ]),
    )?;
    background_page.push(background_fit);
    for text in BACKGROUND_FIT_LABELS {
        // SAFETY: background_fit は有効なコンボボックスで、文字列はこの呼び出しの間は生きている。
        unsafe {
            SendMessageW(
                background_fit,
                CB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(text).as_ptr() as isize)),
            )
        };
    }
    let (class, text, rect) = label("不透明度(&Y):", background_label_rect(opacity_row));
    background_page.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let background_opacity = child(
        w!("EDIT"),
        "",
        edit_style,
        WS_EX_CLIENTEDGE,
        ID_BACKGROUND_OPACITY,
        Some([
            background_control_left,
            opacity_row,
            NUMBER_WIDTH,
            ROW_HEIGHT,
        ]),
    )?;
    background_page.push(background_opacity);
    let background_opacity_spin = child(
        UPDOWN_CLASSW,
        "",
        spin_style,
        none,
        ID_BACKGROUND_OPACITY_SPIN,
        None,
    )?;
    background_page.push(background_opacity_spin);
    let (class, text, rect) = label(
        "0（消す）〜1（そのまま）",
        opacity_note_rect(opacity_row, background_control_left),
    );
    background_page.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);

    let images_top = defaults_bottom + BUTTON_GAP;
    let list_top = images_top + GROUP_CAPTION_HEIGHT;
    let item_fit_row = list_top + BACKGROUND_LIST_HEIGHT + BUTTON_GAP;
    let item_opacity_row = item_fit_row + ROW_HEIGHT + BUTTON_GAP;
    let custom_opacity_row = item_opacity_row + ROW_HEIGHT;
    let images_bottom = custom_opacity_row + ROW_HEIGHT + GROUP_PADDING;
    background_page.push(child(
        w!("BUTTON"),
        "背景画像の一覧(&I)",
        group_style,
        none,
        ID_LABEL,
        Some([
            page_left,
            images_top,
            page_width,
            images_bottom - images_top,
        ]),
    )?);
    let list_width = inner_width - BUTTON_GAP - BACKGROUND_BUTTON_WIDTH;
    let background_images = child(
        WC_LISTBOXW,
        "",
        WS_GROUP | WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(LBS_NOTIFY as u32),
        WS_EX_CLIENTEDGE,
        ID_BACKGROUND_IMAGES,
        Some([inner_left, list_top, list_width, BACKGROUND_LIST_HEIGHT]),
    )?;
    background_page.push(background_images);
    let mut image_buttons = [HWND::default(); 3];
    for (index, (id, text)) in [
        (ID_BACKGROUND_ADD_FILES, "ファイルを追加(&L)…"),
        (ID_BACKGROUND_ADD_FOLDER, "フォルダを追加(&O)…"),
        (ID_BACKGROUND_REMOVE, "削除(&D)"),
    ]
    .into_iter()
    .enumerate()
    {
        image_buttons[index] = child(
            w!("BUTTON"),
            text,
            WS_GROUP | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
            none,
            id,
            Some([
                inner_left + list_width + BUTTON_GAP,
                list_top + index as i32 * (BUTTON_HEIGHT + BUTTON_GAP),
                BACKGROUND_BUTTON_WIDTH,
                BUTTON_HEIGHT,
            ]),
        )?;
        background_page.push(image_buttons[index]);
    }
    let (class, text, rect) = label(
        "選択した項目の表示方法(&N):",
        background_label_rect(item_fit_row),
    );
    background_page.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let background_item_fit = child(
        WC_COMBOBOXW,
        "",
        WS_GROUP | WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
        none,
        ID_BACKGROUND_ITEM_FIT,
        Some([
            background_control_left,
            item_fit_row,
            background_control_width,
            ROW_HEIGHT,
        ]),
    )?;
    background_page.push(background_item_fit);
    // 先頭の「既定（…）」は、既定の表示方法を入れるときに書き直す。
    for text in std::iter::once("").chain(BACKGROUND_FIT_LABELS) {
        // SAFETY: background_item_fit は有効なコンボボックスで、文字列はこの呼び出しの間は生きている。
        unsafe {
            SendMessageW(
                background_item_fit,
                CB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(text).as_ptr() as isize)),
            )
        };
    }
    let (class, text, rect) = label(
        "選択した項目の不透明度(&H):",
        background_label_rect(item_opacity_row),
    );
    background_page.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let background_item_opacity_default = child(
        w!("BUTTON"),
        "",
        radio(true),
        none,
        ID_BACKGROUND_ITEM_OPACITY_DEFAULT,
        Some([
            background_control_left,
            item_opacity_row,
            background_control_width,
            ROW_HEIGHT,
        ]),
    )?;
    background_page.push(background_item_opacity_default);
    let background_item_opacity_custom = child(
        w!("BUTTON"),
        "指定",
        radio(false),
        none,
        ID_BACKGROUND_ITEM_OPACITY_CUSTOM,
        Some([
            background_control_left,
            custom_opacity_row,
            CUSTOM_OPACITY_WIDTH,
            ROW_HEIGHT,
        ]),
    )?;
    background_page.push(background_item_opacity_custom);
    let custom_opacity_left = background_control_left + CUSTOM_OPACITY_WIDTH;
    let background_item_opacity = child(
        w!("EDIT"),
        "",
        edit_style,
        WS_EX_CLIENTEDGE,
        ID_BACKGROUND_ITEM_OPACITY,
        Some([
            custom_opacity_left,
            custom_opacity_row,
            NUMBER_WIDTH,
            ROW_HEIGHT,
        ]),
    )?;
    background_page.push(background_item_opacity);
    let background_item_opacity_spin = child(
        UPDOWN_CLASSW,
        "",
        spin_style,
        none,
        ID_BACKGROUND_ITEM_OPACITY_SPIN,
        None,
    )?;
    background_page.push(background_item_opacity_spin);
    let (class, text, rect) = label(
        "0〜1",
        opacity_note_rect(custom_opacity_row, custom_opacity_left),
    );
    background_page.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let (class, text, rect) = label(
        "新しいタブを開くたびに、一覧からランダムに一枚選ぶ（フォルダはその中の画像から）。画像には端末の背景色を重ね、不透明度のとおりに薄める。",
        [
            page_left,
            images_bottom + BUTTON_GAP,
            page_width,
            2 * ROW_HEIGHT,
        ],
    );
    background_page.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);

    // 動作
    let mut behavior = Vec::new();
    let (class, text, rect) = label("貼り付ける前に確かめる(&P):", label_rect(row(0)));
    behavior.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let mut paste_confirm = [HWND::default(); 3];
    for (index, text) in PASTE_CONFIRM_LABELS.iter().enumerate() {
        let top = row(0) + index as i32 * ROW_HEIGHT;
        paste_confirm[index] = child(
            w!("BUTTON"),
            text,
            radio(index == 0),
            none,
            ID_PASTE_CONFIRM + index as i32,
            Some([control_left, top, control_width, ROW_HEIGHT]),
        )?;
        behavior.push(paste_confirm[index]);
    }
    let (class, text, rect) = label(
        "改行を含む文字列を貼り付ける前に、内容を編集できるダイアログで確かめる。「コマンドとして実行されうるとき」は、アプリが bracketed paste を有効にしていないときなど。",
        [
            page_left,
            row(0) + 3 * ROW_HEIGHT + MARGIN,
            page_width,
            3 * ROW_HEIGHT,
        ],
    );
    behavior.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);

    // プロファイル
    let mut profiles_page = Vec::new();
    let (class, text, rect) = label(
        "新しいタブで起動するプロファイル(&R):",
        [page_left, row(0) + LABEL_OFFSET, page_width, ROW_HEIGHT],
    );
    profiles_page.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let profiles_top = row(0) + ROW_HEIGHT;
    let profiles_list = child(
        WC_LISTBOXW,
        "",
        WS_GROUP | WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE((LBS_NOTIFY | LBS_USETABSTOPS) as u32),
        WS_EX_CLIENTEDGE,
        ID_PROFILES,
        Some([page_left, profiles_top, page_width, PROFILE_LIST_HEIGHT]),
    )?;
    profiles_page.push(profiles_list);
    let profile_tab_stops = [PROFILE_COLUMN_TAB_STOP];
    // SAFETY: profiles_list は有効な一覧で、profile_tab_stops はこの呼び出しの間は生きている。
    unsafe {
        SendMessageW(
            profiles_list,
            LB_SETTABSTOPS,
            Some(WPARAM(profile_tab_stops.len())),
            Some(LPARAM(profile_tab_stops.as_ptr() as isize)),
        )
    };
    let profile_buttons_top = profiles_top + PROFILE_LIST_HEIGHT + BUTTON_GAP;
    let profile_button_width = (page_width - 2 * BUTTON_GAP) / 3;
    let mut profile_buttons = [HWND::default(); 3];
    for (index, (id, text)) in [
        (ID_PROFILE_ADD, "追加(&D)"),
        (ID_PROFILE_REMOVE, "削除(&L)"),
        (ID_PROFILE_DEFAULT, "既定にする(&J)"),
    ]
    .into_iter()
    .enumerate()
    {
        profile_buttons[index] = child(
            w!("BUTTON"),
            text,
            WS_GROUP | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
            none,
            id,
            Some([
                page_left + index as i32 * (profile_button_width + BUTTON_GAP),
                profile_buttons_top,
                profile_button_width,
                BUTTON_HEIGHT,
            ]),
        )?;
        profiles_page.push(profile_buttons[index]);
    }
    let profile_name_row = profile_buttons_top + BUTTON_HEIGHT + MARGIN;
    let (class, text, rect) = label("名前(&N):", label_rect(profile_name_row));
    profiles_page.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let profile_name = child(
        w!("EDIT"),
        "",
        edit_style,
        WS_EX_CLIENTEDGE,
        ID_PROFILE_NAME,
        Some([control_left, profile_name_row, control_width, ROW_HEIGHT]),
    )?;
    profiles_page.push(profile_name);
    let profile_command_row = profile_name_row + ROW_HEIGHT + MARGIN;
    let (class, text, rect) = label("コマンドライン(&C):", label_rect(profile_command_row));
    profiles_page.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let profile_command = child(
        w!("EDIT"),
        "",
        edit_style,
        WS_EX_CLIENTEDGE,
        ID_PROFILE_COMMAND,
        Some([control_left, profile_command_row, control_width, ROW_HEIGHT]),
    )?;
    profiles_page.push(profile_command);
    let (class, text, rect) = label(
        "タブバーの「+」は既定のプロファイルを開き、「▾」か「+」の右クリックで一覧から選んで開く。fantermo を引数つきで起動したときは、「+」とキーも引数のコマンドラインを使う。書き換えたら、次に開くタブから使う。",
        [
            page_left,
            profile_command_row + ROW_HEIGHT + MARGIN,
            page_width,
            3 * ROW_HEIGHT,
        ],
    );
    profiles_page.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);

    // キー
    let mut keys_page = Vec::new();
    let (class, text, rect) = label(
        "キーの割り当て(&K):",
        [page_left, row(0), page_width, ROW_HEIGHT],
    );
    keys_page.push(child(
        class,
        &text,
        WINDOW_STYLE(0),
        none,
        ID_LABEL,
        Some(rect),
    )?);
    let key_buttons_top = button_top - MARGIN - BUTTON_HEIGHT;
    let keys_list = child(
        WC_LISTBOXW,
        "",
        WS_GROUP | WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE((LBS_NOTIFY | LBS_USETABSTOPS) as u32),
        WS_EX_CLIENTEDGE,
        ID_KEYS,
        Some([
            page_left,
            row(0) + ROW_HEIGHT,
            page_width,
            key_buttons_top - BUTTON_GAP - row(0) - ROW_HEIGHT,
        ]),
    )?;
    keys_page.push(keys_list);
    let tab_stops = [KEY_COLUMN_TAB_STOP];
    // SAFETY: keys_list は有効な一覧で、tab_stops はこの呼び出しの間は生きている。
    unsafe {
        SendMessageW(
            keys_list,
            LB_SETTABSTOPS,
            Some(WPARAM(tab_stops.len())),
            Some(LPARAM(tab_stops.as_ptr() as isize)),
        )
    };
    let key_button_width = (page_width - 3 * BUTTON_GAP) / 4;
    let mut key_buttons = [HWND::default(); 4];
    for (index, (id, text)) in [
        (ID_KEY_ADD, "追加(&D)…"),
        (ID_KEY_CHANGE, "変更(&E)…"),
        (ID_KEY_REMOVE, "外す(&R)"),
        (ID_KEY_RESET, "既定に戻す(&U)"),
    ]
    .into_iter()
    .enumerate()
    {
        key_buttons[index] = child(
            w!("BUTTON"),
            text,
            WS_GROUP | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
            none,
            id,
            Some([
                page_left + index as i32 * (key_button_width + BUTTON_GAP),
                key_buttons_top,
                key_button_width,
                BUTTON_HEIGHT,
            ]),
        )?;
        keys_page.push(key_buttons[index]);
    }

    // 色
    let mut colors_page = Vec::new();
    let mut color_rows = Vec::new();
    let mut color_scheme = [HWND::default(); 2];
    let mut top = MARGIN;
    for (table_index, table) in ColorTable::ALL.into_iter().enumerate() {
        let (class, text, rect) = label(
            COLOR_TABLE_LABELS[table_index],
            [page_left, top + LABEL_OFFSET, page_width, COLOR_ROW_HEIGHT],
        );
        colors_page.push(child(
            class,
            &text,
            WINDOW_STYLE(0),
            none,
            ID_LABEL,
            Some(rect),
        )?);
        top += COLOR_ROW_HEIGHT;
        if table == ColorTable::Terminal {
            let (class, text, rect) = label(
                "配色(&M):",
                [
                    page_left + COLOR_LABEL_INDENT,
                    top + LABEL_OFFSET,
                    LABEL_WIDTH - COLOR_LABEL_INDENT,
                    COLOR_ROW_HEIGHT,
                ],
            );
            colors_page.push(child(
                class,
                &text,
                WINDOW_STYLE(0),
                none,
                ID_LABEL,
                Some(rect),
            )?);
            for (index, text) in COLOR_SCHEME_LABELS.iter().enumerate() {
                color_scheme[index] = child(
                    w!("BUTTON"),
                    text,
                    radio(index == 0),
                    none,
                    ID_COLOR_SCHEME + index as i32,
                    Some([
                        control_left + index as i32 * COLOR_SCHEME_WIDTH,
                        top,
                        COLOR_SCHEME_WIDTH,
                        COLOR_ROW_HEIGHT,
                    ]),
                )?;
                colors_page.push(color_scheme[index]);
            }
            top += COLOR_ROW_HEIGHT;
        }
        for label_text in color_labels(table) {
            let id = ID_COLORS + color_rows.len() as i32 * COLOR_ID_STRIDE;
            let (class, text, rect) = label(
                label_text,
                [
                    page_left + COLOR_LABEL_INDENT,
                    top + LABEL_OFFSET,
                    LABEL_WIDTH - COLOR_LABEL_INDENT,
                    COLOR_ROW_HEIGHT,
                ],
            );
            colors_page.push(child(
                class,
                &text,
                WINDOW_STYLE(0),
                none,
                ID_LABEL,
                Some(rect),
            )?);
            let swatch = child(
                w!("STATIC"),
                "",
                WINDOW_STYLE(0),
                WS_EX_CLIENTEDGE,
                id + COLOR_SWATCH,
                Some([control_left, top + 2, SWATCH_WIDTH, COLOR_ROW_HEIGHT - 4]),
            )?;
            let mut left = control_left + SWATCH_WIDTH + BUTTON_GAP;
            let text = child(
                w!("STATIC"),
                "",
                WINDOW_STYLE(0),
                none,
                id + 1,
                Some([left, top + LABEL_OFFSET, COLOR_TEXT_WIDTH, COLOR_ROW_HEIGHT]),
            )?;
            left += COLOR_TEXT_WIDTH + BUTTON_GAP;
            let choose = child(
                w!("BUTTON"),
                "選ぶ…",
                WS_GROUP | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
                none,
                id + COLOR_CHOOSE,
                Some([left, top, CHOOSE_WIDTH, COLOR_ROW_HEIGHT - 2]),
            )?;
            left += CHOOSE_WIDTH + BUTTON_GAP;
            let reset = child(
                w!("BUTTON"),
                "既定に戻す",
                WS_GROUP | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
                none,
                id + COLOR_RESET,
                Some([left, top, COLOR_RESET_WIDTH, COLOR_ROW_HEIGHT - 2]),
            )?;
            colors_page.extend([swatch, text, choose, reset]);
            color_rows.push(ColorRow {
                swatch,
                text,
                reset,
            });
            top += COLOR_ROW_HEIGHT;
        }
    }

    // 下のボタンと、入力のエラー
    let error = child(
        w!("STATIC"),
        "",
        WINDOW_STYLE(0),
        none,
        ID_ERROR,
        Some([
            MARGIN,
            button_top + LABEL_OFFSET,
            CLIENT_WIDTH - 2 * MARGIN - 3 * (BUTTON_WIDTH + BUTTON_GAP),
            ROW_HEIGHT,
        ]),
    )?;
    let buttons = [
        (IDOK.0, "OK", true),
        (IDCANCEL.0, "キャンセル", false),
        (ID_APPLY, "適用(&A)", false),
    ];
    let mut button_left = CLIENT_WIDTH
        - MARGIN
        - buttons.len() as i32 * BUTTON_WIDTH
        - (buttons.len() as i32 - 1) * BUTTON_GAP;
    for (id, text, default) in buttons {
        let style = if default {
            BS_DEFPUSHBUTTON
        } else {
            BS_PUSHBUTTON
        };
        child(
            w!("BUTTON"),
            text,
            WS_GROUP | WS_TABSTOP | WINDOW_STYLE(style as u32),
            none,
            id,
            Some([button_left, button_top, BUTTON_WIDTH, BUTTON_HEIGHT]),
        )?;
        button_left += BUTTON_WIDTH + BUTTON_GAP;
    }

    Ok((
        hwnd,
        Controls {
            pages,
            page_controls: vec![
                appearance,
                background_page,
                behavior,
                profiles_page,
                keys_page,
                colors_page,
            ],
            font,
            size,
            size_spin,
            wide_glyph,
            scroll_bar_width,
            scroll_bar_width_spin,
            padding,
            padding_spin,
            paste_confirm,
            profiles_list,
            profile_remove: profile_buttons[1],
            profile_default: profile_buttons[2],
            profile_name,
            profile_command,
            background_images,
            background_remove: image_buttons[2],
            background_opacity,
            background_opacity_spin,
            background_fit,
            background_item_fit,
            background_item_opacity_default,
            background_item_opacity_custom,
            background_item_opacity,
            background_item_opacity_spin,
            color_scheme,
            keys_list,
            key_buttons,
            color_rows,
            error,
            placed,
        },
    ))
}

/// ダイアログを、DPI が `dpi` のときの大きさにして、呼び出し元のウィンドウの真ん中に置く。
fn place(owner: HWND, dialog: HWND, dpi: u32) -> windows::core::Result<()> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: scale(CLIENT_WIDTH, dpi),
        bottom: scale(CLIENT_HEIGHT, dpi),
    };
    // SAFETY: rect は書き込める。
    unsafe {
        AdjustWindowRectExForDpi(
            &mut rect,
            WS_POPUP | WS_CAPTION | WS_SYSMENU,
            false,
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            dpi,
        )?
    };
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    let mut owner_rect = RECT::default();
    // SAFETY: owner は有効なウィンドウで、owner_rect は書き込める。
    unsafe { GetWindowRect(owner, &mut owner_rect)? };
    let left = owner_rect.left + (owner_rect.right - owner_rect.left - width) / 2;
    let top = owner_rect.top + (owner_rect.bottom - owner_rect.top - height) / 2;
    // SAFETY: dialog は有効なウィンドウ。
    unsafe { SetWindowPos(dialog, None, left, top, width, height, SWP_NOZORDER) }
}

/// 96 DPI のときの `value` ピクセルを、DPI が `dpi` のときのピクセルにする。
fn scale(value: i32, dpi: u32) -> i32 {
    (value as f32 * dpi as f32 / BASE_DPI as f32).round() as i32
}

impl State<'_> {
    /// コントロールに、設定 `config` と、一覧の `selected_font` 番目のフォントを入れる。
    fn fill(&self, config: &Config, selected_font: usize) {
        let c = &self.controls;
        for font in &self.fonts {
            // SAFETY: コンボボックスは有効なウィンドウで、文字列はこの呼び出しの間は生きている。
            unsafe {
                SendMessageW(
                    c.font,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(font.name.as_str()).as_ptr() as isize)),
                )
            };
        }
        // SAFETY: コンボボックスは有効なウィンドウ。
        unsafe { SendMessageW(c.font, CB_SETCURSEL, Some(WPARAM(selected_font)), None) };
        set_text(c.size, &format_size(config.font.size_pt));
        let wide_glyph = WideGlyph::ALL
            .iter()
            .position(|&value| value == config.font.wide_glyph);
        check_one(&c.wide_glyph, wide_glyph);
        set_text(c.scroll_bar_width, &config.scroll_bar.width.to_string());
        set_text(c.padding, &config.window.padding.to_string());
        let paste_confirm = PasteConfirm::ALL
            .iter()
            .position(|&value| value == config.paste.confirm);
        check_one(&c.paste_confirm, paste_confirm);
        *self.profiles.borrow_mut() = config.profiles.clone();
        self.default_profile.set(config.default_profile_index());
        self.refresh_profiles(config.default_profile_index());
        set_text(
            c.background_opacity,
            &format_opacity(config.background.image_opacity),
        );
        let fit = BackgroundFit::ALL
            .iter()
            .position(|&value| value == config.background.fit);
        if let Some(fit) = fit {
            // SAFETY: コンボボックスは有効なウィンドウ。
            unsafe { SendMessageW(c.background_fit, CB_SETCURSEL, Some(WPARAM(fit)), None) };
        }
        self.refresh_background_defaults();
        *self.background_images.borrow_mut() = config.background.images.clone();
        self.refresh_background_images(None);
        *self.keys.borrow_mut() = config.keys.clone();
        self.refresh_keys(None);
        *self.colors.borrow_mut() = config.colors.clone();
        let scheme = ColorScheme::ALL
            .iter()
            .position(|&value| value == config.colors.scheme);
        check_one(&c.color_scheme, scheme);
        self.refresh_colors();
    }

    /// 色のページのすべての色を、編集中の色に合わせる（配色や端末の色を変えると、ほかの色の既定の
    /// 色も変わる）。
    fn refresh_colors(&self) {
        for item in 0..self.controls.color_rows.len() {
            self.refresh_color(item);
        }
    }

    /// 表 `table` の `index` 番目の色の、編集中の色での既定の色。
    fn default_color(&self, table: ColorTable, index: usize) -> Rgb {
        default_color(&self.colors.borrow(), &self.tab_bar_defaults, table, index)
    }

    /// 色のページの `item` 番目の色の、見本・値・既定に戻すボタンを、編集中の色に合わせる。
    fn refresh_color(&self, item: usize) {
        let (table, index) = color_items()[item];
        let written = self.colors.borrow().table(table)[index];
        let color = written.unwrap_or(self.default_color(table, index));
        let row = &self.controls.color_rows[item];
        let text = match written {
            Some(_) => format_color(color),
            None => format!("{}（既定）", format_color(color)),
        };
        set_text(row.text, &text);
        // SAFETY: 色を渡してブラシを作るだけ。
        let brush = unsafe { CreateSolidBrush(colorref(color)) };
        let previous = std::mem::replace(&mut self.swatch_brushes.borrow_mut()[item], brush);
        // SAFETY: row のコントロールはダイアログの子の有効なウィンドウ。
        unsafe {
            _ = EnableWindow(row.reset, written.is_some());
            _ = InvalidateRect(Some(row.swatch), None, true);
        }
        if !previous.is_invalid() {
            // SAFETY: 前のブラシは、描き直しを頼んだ見本がもう使わない（WM_CTLCOLORSTATIC で渡すのは
            // いまのブラシ）。
            unsafe { _ = DeleteObject(HGDIOBJ(previous.0)) };
        }
    }

    /// 色のページのボタン `id` が押された。
    fn on_color_button(&self, id: i32) {
        let offset = id - ID_COLORS;
        let item = (offset / COLOR_ID_STRIDE) as usize;
        let Some(&(table, index)) = color_items().get(item) else {
            return;
        };
        let new = match offset % COLOR_ID_STRIDE {
            COLOR_CHOOSE => {
                let written = self.colors.borrow().table(table)[index];
                let current = written.unwrap_or(self.default_color(table, index));
                let Some(chosen) = self.choose_color(current) else {
                    return;
                };
                Some(chosen)
            }
            COLOR_RESET => None,
            _ => return,
        };
        self.colors.borrow_mut().table_mut(table)[index] = new;
        self.refresh_colors();
        self.preview();
    }

    /// 配色のラジオボタンが押された。
    fn on_color_scheme(&self) {
        let Some(index) = checked(&self.controls.color_scheme) else {
            return;
        };
        self.colors.borrow_mut().scheme = ColorScheme::ALL[index];
        self.refresh_colors();
        self.preview();
    }

    /// 色を選ぶダイアログを、`current` を選んだ状態で開く。やめたら `None`。
    fn choose_color(&self, current: Rgb) -> Option<Rgb> {
        let mut custom = self.custom_colors.get();
        let mut choose = CHOOSECOLORW {
            lStructSize: size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: self.hwnd,
            rgbResult: colorref(current),
            lpCustColors: custom.as_mut_ptr(),
            Flags: CC_RGBINIT | CC_FULLOPEN,
            ..Default::default()
        };
        // SAFETY: choose は初期化済みで、custom はこの呼び出しの間は生きている 16 個の COLORREF。
        // 借用は持っていない。
        let chosen = unsafe { ChooseColorW(&mut choose) }.as_bool();
        self.custom_colors.set(custom);
        chosen.then(|| rgb_of(choose.rgbResult))
    }

    /// 色の見本 `control` を塗るブラシ。色の見本でなければ `None`。
    fn swatch_brush(&self, control: HWND) -> Option<HBRUSH> {
        let item = self
            .controls
            .color_rows
            .iter()
            .position(|row| row.swatch == control)?;
        self.swatch_brushes.borrow().get(item).copied()
    }

    /// プロファイルの一覧を作り直し、`selected` 番目（一覧より後ろなら最後）を選んで、名前と
    /// コマンドラインの入力欄にその値を入れる。
    fn refresh_profiles(&self, selected: usize) {
        let list = self.controls.profiles_list;
        let profiles = self.profiles.borrow();
        // SAFETY: 一覧は有効なウィンドウ。
        unsafe { SendMessageW(list, LB_RESETCONTENT, None, None) };
        for (index, profile) in profiles.iter().enumerate() {
            let text = profile_row_text(profile, index == self.default_profile.get());
            // SAFETY: 一覧は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
            unsafe {
                SendMessageW(
                    list,
                    LB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(text).as_ptr() as isize)),
                )
            };
        }
        let index = selected.min(profiles.len().saturating_sub(1));
        drop(profiles);
        // SAFETY: 一覧は有効なウィンドウ。
        unsafe { SendMessageW(list, LB_SETCURSEL, Some(WPARAM(index)), None) };
        self.on_profile_selected();
    }

    /// プロファイルの一覧で選んでいる位置。
    fn selected_profile(&self) -> Option<usize> {
        // SAFETY: 一覧は有効なウィンドウ。
        let index =
            unsafe { SendMessageW(self.controls.profiles_list, LB_GETCURSEL, None, None) }.0;
        usize::try_from(index)
            .ok()
            .filter(|&index| index < self.profiles.borrow().len())
    }

    /// プロファイルの一覧で選んだものが変わった。名前とコマンドラインの入力欄に値を入れ、ボタンを
    /// 押せるかを決める。
    fn on_profile_selected(&self) {
        let c = &self.controls;
        let selected = self.selected_profile();
        let profile = selected.and_then(|index| self.profiles.borrow().get(index).cloned());
        self.filling_profile.set(true);
        set_text(
            c.profile_name,
            profile.as_ref().map_or("", |profile| &profile.name),
        );
        set_text(
            c.profile_command,
            profile.as_ref().map_or("", |profile| &profile.command),
        );
        self.filling_profile.set(false);
        let count = self.profiles.borrow().len();
        for (control, enabled) in [
            (c.profile_remove, selected.is_some() && count > 1),
            (
                c.profile_default,
                selected.is_some_and(|index| index != self.default_profile.get()),
            ),
            (c.profile_name, selected.is_some()),
            (c.profile_command, selected.is_some()),
        ] {
            // SAFETY: control はダイアログの子の有効なウィンドウ。
            unsafe { _ = EnableWindow(control, enabled) };
        }
    }

    /// プロファイルの名前かコマンドラインの入力欄が変わった。選んでいるプロファイルに入れ、一覧の行を
    /// 書き直す。端末への反映は少し待つ（次に開くタブから使うので、反映しても見た目は変わらない）。
    fn on_profile_edited(&self) {
        if self.filling_profile.get() {
            return;
        }
        let Some(index) = self.selected_profile() else {
            return;
        };
        let name = get_text(self.controls.profile_name).trim().to_owned();
        let command = get_text(self.controls.profile_command).trim().to_owned();
        let text = {
            let mut profiles = self.profiles.borrow_mut();
            let profile = &mut profiles[index];
            profile.name = name;
            profile.command = command;
            profile_row_text(profile, index == self.default_profile.get())
        };
        let list = self.controls.profiles_list;
        // SAFETY: 一覧は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
        unsafe {
            SendMessageW(list, LB_DELETESTRING, Some(WPARAM(index)), None);
            SendMessageW(
                list,
                LB_INSERTSTRING,
                Some(WPARAM(index)),
                Some(LPARAM(HSTRING::from(text).as_ptr() as isize)),
            );
            SendMessageW(list, LB_SETCURSEL, Some(WPARAM(index)), None);
        }
        self.schedule_preview();
    }

    /// プロファイルのボタン `id` が押された。
    fn on_profile_button(&self, id: i32) {
        let selected = match id {
            ID_PROFILE_ADD => {
                let mut profiles = self.profiles.borrow_mut();
                let name = new_profile_name(&profiles);
                profiles.push(Profile {
                    name,
                    command: config::DEFAULT_COMMAND_LINE.to_owned(),
                    backend: BackendKind::default(),
                });
                profiles.len() - 1
            }
            ID_PROFILE_REMOVE => {
                let Some(index) = self.selected_profile() else {
                    return;
                };
                let mut profiles = self.profiles.borrow_mut();
                if profiles.len() <= 1 {
                    return;
                }
                profiles.remove(index);
                let default = self.default_profile.get();
                // 既定のものを消したら先頭を、その前を消したら一つ前へずらす。
                self.default_profile.set(match default.cmp(&index) {
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => default - 1,
                    std::cmp::Ordering::Less => default,
                });
                index
            }
            ID_PROFILE_DEFAULT => {
                let Some(index) = self.selected_profile() else {
                    return;
                };
                self.default_profile.set(index);
                index
            }
            _ => return,
        };
        self.refresh_profiles(selected);
        self.preview();
        let focus = if id == ID_PROFILE_ADD {
            self.controls.profile_name
        } else {
            self.controls.profiles_list
        };
        // SAFETY: focus はダイアログの子の有効なウィンドウ。押したボタンが押せなくなることがあるので移す。
        unsafe { _ = SetFocus(Some(focus)) };
        if id == ID_PROFILE_ADD {
            // SAFETY: 入力欄は有効なウィンドウ。名前をすぐ打ち替えられるように全体を選ぶ。
            unsafe {
                SendMessageW(
                    focus,
                    EM_SETSEL,
                    Some(WPARAM(SELECT_FROM_START)),
                    Some(LPARAM(SELECT_TO_END)),
                )
            };
        }
    }

    /// 背景画像の一覧を作り直し、`selected` 番目（一覧より後ろなら最後）を選ぶ。`None` なら選ばない。
    fn refresh_background_images(&self, selected: Option<usize>) {
        let list = self.controls.background_images;
        let images = self.background_images.borrow();
        // SAFETY: 一覧は有効なウィンドウ。
        unsafe { SendMessageW(list, LB_RESETCONTENT, None, None) };
        for image in images.iter() {
            // SAFETY: 一覧は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
            unsafe {
                SendMessageW(
                    list,
                    LB_ADDSTRING,
                    None,
                    Some(LPARAM(
                        HSTRING::from(background_row_text(image)).as_ptr() as isize
                    )),
                )
            };
        }
        if let Some(selected) = selected
            && !images.is_empty()
        {
            let index = selected.min(images.len() - 1);
            // SAFETY: 一覧は有効なウィンドウ。
            unsafe { SendMessageW(list, LB_SETCURSEL, Some(WPARAM(index)), None) };
        }
        drop(images);
        self.update_background_buttons();
    }

    /// 背景画像の一覧で選んでいる位置。
    fn selected_background_image(&self) -> Option<usize> {
        // SAFETY: 一覧は有効なウィンドウ。
        let index =
            unsafe { SendMessageW(self.controls.background_images, LB_GETCURSEL, None, None) }.0;
        usize::try_from(index)
            .ok()
            .filter(|&index| index < self.background_images.borrow().len())
    }

    /// 背景画像の一覧で選んでいるときだけ「削除」と項目の表示方法・不透明度を触れるようにし、表示方法と
    /// 不透明度に選んでいる項目の値を見せる。不透明度の入力欄は「指定」のときだけ触れる。
    fn update_background_buttons(&self) {
        let c = &self.controls;
        let selected = self
            .selected_background_image()
            .and_then(|index| self.background_images.borrow().get(index).cloned());
        for control in [
            c.background_remove,
            c.background_item_fit,
            c.background_item_opacity_default,
            c.background_item_opacity_custom,
        ] {
            // SAFETY: control はダイアログの子の有効なウィンドウ。
            unsafe { _ = EnableWindow(control, selected.is_some()) };
        }
        let (fit, opacity) = match &selected {
            Some(image) => (item_fit_choice(image.fit), Some(image.image_opacity)),
            // 何も選ばない。
            None => (usize::MAX, None),
        };
        // SAFETY: コンボボックスは有効なウィンドウ。
        unsafe { SendMessageW(c.background_item_fit, CB_SETCURSEL, Some(WPARAM(fit)), None) };
        let custom = opacity.flatten();
        let choice = opacity.map(|opacity| usize::from(opacity.is_some()));
        check_one(
            &[
                c.background_item_opacity_default,
                c.background_item_opacity_custom,
            ],
            choice,
        );
        self.enable_item_opacity(custom.is_some());
        // 借用は持っていない（入力欄の EN_CHANGE はこの中で届く）。
        set_text(
            c.background_item_opacity,
            &custom.map(format_opacity).unwrap_or_default(),
        );
    }

    /// 項目の不透明度の入力欄と上下の矢印を、`enabled` なら触れるようにする。
    fn enable_item_opacity(&self, enabled: bool) {
        let c = &self.controls;
        for control in [c.background_item_opacity, c.background_item_opacity_spin] {
            // SAFETY: control はダイアログの子の有効なウィンドウ。
            unsafe { _ = EnableWindow(control, enabled) };
        }
    }

    /// 項目の不透明度で「指定」を選んでいるか。
    fn custom_item_opacity(&self) -> bool {
        checked(&[self.controls.background_item_opacity_custom]).is_some()
    }

    /// 編集中の既定の表示方法。
    fn default_fit(&self) -> BackgroundFit {
        // SAFETY: コンボボックスは有効なウィンドウ。
        let choice =
            unsafe { SendMessageW(self.controls.background_fit, CB_GETCURSEL, None, None) }.0;
        usize::try_from(choice)
            .ok()
            .and_then(|choice| BackgroundFit::ALL.get(choice).copied())
            .unwrap_or(self.base.borrow().background.fit)
    }

    /// 編集中の既定の不透明度。読めなければ `None`。
    fn default_opacity(&self) -> Option<f32> {
        config::parse_image_opacity(&get_text(self.controls.background_opacity))
    }

    /// 項目の表示方法の先頭の選択肢と、不透明度の「既定」のラジオボタンに、編集中の既定の値を添える。
    /// 既定の不透明度が読めなければ、ラジオボタンは前の添え書きのままにする。
    fn refresh_background_defaults(&self) {
        let c = &self.controls;
        let fit = default_choice_label(fit_label(self.default_fit()));
        // SAFETY: コンボボックスは有効なウィンドウで、文字列はこの呼び出しの間は生きている。
        unsafe {
            let selected = SendMessageW(c.background_item_fit, CB_GETCURSEL, None, None).0;
            SendMessageW(
                c.background_item_fit,
                CB_DELETESTRING,
                Some(WPARAM(0)),
                None,
            );
            SendMessageW(
                c.background_item_fit,
                CB_INSERTSTRING,
                Some(WPARAM(0)),
                Some(LPARAM(HSTRING::from(fit).as_ptr() as isize)),
            );
            SendMessageW(
                c.background_item_fit,
                CB_SETCURSEL,
                Some(WPARAM(selected as usize)),
                None,
            );
        }
        if let Some(opacity) = self.default_opacity() {
            set_text(
                c.background_item_opacity_default,
                &default_choice_label(&format_opacity(opacity)),
            );
        }
    }

    /// 背景画像の一覧の `index` 番目の行を、編集中の項目に合わせて書き直す（選んだままにする）。
    fn refresh_background_row(&self, index: usize) {
        let list = self.controls.background_images;
        let Some(text) = self
            .background_images
            .borrow()
            .get(index)
            .map(background_row_text)
        else {
            return;
        };
        // SAFETY: 一覧は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
        unsafe {
            SendMessageW(list, LB_DELETESTRING, Some(WPARAM(index)), None);
            SendMessageW(
                list,
                LB_INSERTSTRING,
                Some(WPARAM(index)),
                Some(LPARAM(HSTRING::from(text).as_ptr() as isize)),
            );
            SendMessageW(list, LB_SETCURSEL, Some(WPARAM(index)), None);
        }
    }

    /// 一覧で選んでいる項目の表示方法のコンボボックスが変わった。
    fn on_background_item_fit(&self) {
        let Some(index) = self.selected_background_image() else {
            return;
        };
        // SAFETY: コンボボックスは有効なウィンドウ。
        let choice =
            unsafe { SendMessageW(self.controls.background_item_fit, CB_GETCURSEL, None, None) }.0;
        let Some(fit) = usize::try_from(choice).ok().and_then(item_fit_from_choice) else {
            return;
        };
        self.background_images.borrow_mut()[index].fit = fit;
        self.refresh_background_row(index);
        self.preview();
    }

    /// 一覧で選んでいる項目の不透明度の入力欄が変わった。「指定」で読めれば項目に入れる。端末への反映は
    /// 少し待つ（読めなければ、反映するときにエラーを見せる）。
    fn on_background_item_opacity(&self) {
        if let Some(index) = self.selected_background_image()
            && self.custom_item_opacity()
            && let Some(opacity) =
                config::parse_image_opacity(&get_text(self.controls.background_item_opacity))
        {
            self.set_item_opacity(index, Some(opacity));
        }
        self.schedule_preview();
    }

    /// 一覧で選んでいる項目の不透明度の「既定」か「指定」が選ばれた。「指定」で入力欄が読めなければ、
    /// 既定の不透明度を入れる。
    fn on_background_item_opacity_choice(&self) {
        let Some(index) = self.selected_background_image() else {
            return;
        };
        let c = &self.controls;
        let custom = self.custom_item_opacity();
        self.enable_item_opacity(custom);
        if custom {
            let opacity = config::parse_image_opacity(&get_text(c.background_item_opacity))
                .or_else(|| self.default_opacity())
                .unwrap_or(self.base.borrow().background.image_opacity);
            // 借用は持っていない（入力欄の EN_CHANGE はこの中で届く）。
            set_text(c.background_item_opacity, &format_opacity(opacity));
            self.set_item_opacity(index, Some(opacity));
        } else {
            set_text(c.background_item_opacity, "");
            self.set_item_opacity(index, None);
        }
        self.preview();
    }

    /// 一覧の `index` 番目の項目の不透明度を `opacity` にし、変わったら一覧の行を書き直す。
    fn set_item_opacity(&self, index: usize, opacity: Option<f32>) {
        let changed = {
            let mut images = self.background_images.borrow_mut();
            let changed = images[index].image_opacity != opacity;
            images[index].image_opacity = opacity;
            changed
        };
        if changed {
            self.refresh_background_row(index);
        }
    }

    /// 背景画像の一覧のボタン `id` が押された。
    fn on_background_button(&self, id: i32) {
        let selected = match id {
            ID_BACKGROUND_ADD_FILES => {
                let filters = image_filters();
                let chosen = match file_dialog::open_files(self.hwnd, "背景画像を追加", &filters)
                {
                    Ok(chosen) => chosen,
                    Err(e) => {
                        tracing::warn!("failed to choose background images: {e}");
                        return;
                    }
                };
                self.add_background_images(chosen)
            }
            ID_BACKGROUND_ADD_FOLDER => {
                let chosen = match file_dialog::open_folder(self.hwnd, "背景画像のフォルダを追加")
                {
                    Ok(chosen) => chosen,
                    Err(e) => {
                        tracing::warn!("failed to choose a background image folder: {e}");
                        return;
                    }
                };
                self.add_background_images(chosen.into_iter().collect())
            }
            ID_BACKGROUND_REMOVE => {
                let Some(index) = self.selected_background_image() else {
                    return;
                };
                self.background_images.borrow_mut().remove(index);
                Some(index)
            }
            _ => return,
        };
        let Some(selected) = selected else {
            return;
        };
        self.refresh_background_images(Some(selected));
        self.preview();
        // SAFETY: 一覧はダイアログの子の有効なウィンドウ。押したボタンが押せなくなることがあるので、
        // 一覧へ戻す。
        unsafe { _ = SetFocus(Some(self.controls.background_images)) };
    }

    /// 背景画像の一覧の最後に `paths` を足す（同じものがあれば足さない）。足した最後の位置を返し、
    /// 何も足さなければ `None`。
    fn add_background_images(&self, paths: Vec<PathBuf>) -> Option<usize> {
        let mut images = self.background_images.borrow_mut();
        let mut added = None;
        for path in paths {
            let text = path.to_string_lossy().into_owned();
            if images
                .iter()
                .any(|image| image.path.eq_ignore_ascii_case(&text))
            {
                continue;
            }
            images.push(BackgroundImage::new(text));
            added = Some(images.len() - 1);
        }
        added
    }

    /// キーの割り当ての一覧を作り直し、`selected` の行（なければ同じ動作の行、それもなければ先頭）を
    /// 選ぶ。
    fn refresh_keys(&self, selected: Option<KeyRow>) {
        let c = &self.controls;
        let rows = key_rows(&self.keys.borrow());
        // SAFETY: 一覧は有効なウィンドウ。
        unsafe { SendMessageW(c.keys_list, LB_RESETCONTENT, None, None) };
        for row in &rows {
            // SAFETY: 一覧は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
            unsafe {
                SendMessageW(
                    c.keys_list,
                    LB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(key_row_text(*row)).as_ptr() as isize)),
                )
            };
        }
        let index = selected
            .and_then(|selected| {
                rows.iter()
                    .position(|row| *row == selected)
                    .or_else(|| rows.iter().position(|(action, _)| *action == selected.0))
            })
            .unwrap_or(0);
        // SAFETY: 一覧は有効なウィンドウ。
        unsafe { SendMessageW(c.keys_list, LB_SETCURSEL, Some(WPARAM(index)), None) };
        *self.key_rows.borrow_mut() = rows;
        self.update_key_buttons();
    }

    /// キーの割り当ての一覧で選んでいる行。
    fn selected_key_row(&self) -> Option<KeyRow> {
        // SAFETY: 一覧は有効なウィンドウ。
        let index = unsafe { SendMessageW(self.controls.keys_list, LB_GETCURSEL, None, None) }.0;
        let index = usize::try_from(index).ok()?;
        self.key_rows.borrow().get(index).copied()
    }

    /// 選んでいる行で押せるボタンだけを押せるようにする。
    fn update_key_buttons(&self) {
        let row = self.selected_key_row();
        let has_combo = row.is_some_and(|(_, combo)| combo.is_some());
        let not_default =
            row.is_some_and(|(action, _)| !is_default_binding(&self.keys.borrow(), action));
        let [add, change, remove, reset] = self.controls.key_buttons;
        for (button, enabled) in [
            (add, row.is_some()),
            (change, has_combo),
            (remove, has_combo),
            (reset, not_default),
        ] {
            // SAFETY: button はダイアログの子の有効なウィンドウ。
            unsafe { _ = EnableWindow(button, enabled) };
        }
    }

    /// キーのボタン `id` が押された。
    fn on_key_button(&self, id: i32) {
        let Some((action, combo)) = self.selected_key_row() else {
            return;
        };
        let selected = match id {
            ID_KEY_ADD => {
                let Some(new) = self.capture_key(action) else {
                    return;
                };
                if !self.assign(action, new, None) {
                    return;
                }
                (action, Some(new))
            }
            ID_KEY_CHANGE => {
                let Some(old) = combo else {
                    return;
                };
                let Some(new) = self.capture_key(action) else {
                    return;
                };
                if !self.assign(action, new, Some(old)) {
                    return;
                }
                (action, Some(new))
            }
            ID_KEY_REMOVE => {
                let Some(old) = combo else {
                    return;
                };
                self.keys.borrow_mut().bind(old, None);
                (action, None)
            }
            ID_KEY_RESET => {
                self.keys.borrow_mut().reset_action(action);
                (action, combo)
            }
            _ => return,
        };
        self.refresh_keys(Some(selected));
        self.preview();
        // SAFETY: 一覧はダイアログの子の有効なウィンドウ。押したボタンが押せなくなることがあるので、
        // 一覧へ戻す。
        unsafe { _ = SetFocus(Some(self.controls.keys_list)) };
    }

    /// 動作 `action` に割り当てるキーを、押したキーを読み取るダイアログで読む。
    fn capture_key(&self, action: Action) -> Option<KeyCombo> {
        let prompt = format!("「{}」に割り当てるキーを押す。", action_label(action));
        key_capture::capture(self.hwnd, &prompt, self.font.get(), self.dpi.get())
    }

    /// 組み合わせ `new` を動作 `action` に割り当てる。`replacing` があれば、その組み合わせを外す。`new`
    /// をほかの動作に割り当てていれば、割り当て直すかを尋ね、やめたら `false`。
    fn assign(&self, action: Action, new: KeyCombo, replacing: Option<KeyCombo>) -> bool {
        let bound = self.keys.borrow().lookup_combo(new);
        if bound == Some(action) && replacing.is_none_or(|old| old == new) {
            return true;
        }
        if let Some(other) = bound
            && other != action
        {
            let text = format!(
                "{} は「{}」に割り当てています。「{}」に割り当て直しますか？",
                new.label(),
                action_label(other),
                action_label(action)
            );
            // SAFETY: hwnd は有効なウィンドウで、文字列はこの呼び出しの間は生きている。借用は持っていない。
            let answer = unsafe {
                MessageBoxW(
                    Some(self.hwnd),
                    &HSTRING::from(text),
                    &HSTRING::from(TITLE),
                    MB_YESNO | MB_ICONQUESTION,
                )
            };
            if answer != IDYES {
                return false;
            }
        }
        let mut keys = self.keys.borrow_mut();
        if let Some(old) = replacing {
            keys.bind(old, None);
        }
        keys.bind(new, Some(action));
        true
    }

    /// DPI を `dpi` にし、フォントを作り直してコントロールを並べ直す。
    fn set_dpi(&self, dpi: u32) {
        self.dpi.set(dpi);
        let font = match ui_font::message_logfont(dpi) {
            // SAFETY: logfont は初期化済み。作ったフォントはダイアログが閉じた後に消す。
            Ok(logfont) => unsafe { CreateFontIndirectW(&logfont) },
            Err(e) => {
                tracing::warn!("failed to read the message font: {e}");
                HFONT::default()
            }
        };
        let previous = self.font.replace(font);
        self.layout();
        if !previous.is_invalid() {
            // SAFETY: 前のフォントは、どのコントロールももう使っていない。
            unsafe { _ = DeleteObject(HGDIOBJ(previous.0)) };
        }
    }

    /// コントロールを、いまの DPI の大きさで並べ、フォントを付ける。
    fn layout(&self) {
        let dpi = self.dpi.get();
        let c = &self.controls;
        let font = self.font.get();
        let all = c.placed.iter().map(|(control, _)| *control).chain([
            c.size_spin,
            c.scroll_bar_width_spin,
            c.padding_spin,
            c.background_opacity_spin,
            c.background_item_opacity_spin,
        ]);
        for control in all {
            // SAFETY: control はダイアログの子で、フォントはダイアログが閉じるまで消さない。
            unsafe {
                SendMessageW(
                    control,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(0)),
                )
            };
        }
        for &(control, [left, top, width, height]) in &c.placed {
            // SAFETY: control はダイアログの子の有効なウィンドウ。
            unsafe {
                _ = MoveWindow(
                    control,
                    scale(left, dpi),
                    scale(top, dpi),
                    scale(width, dpi),
                    scale(height, dpi),
                    true,
                );
            }
        }
        // 上下の矢印のボタンは、相手の入力欄の右端に付け直す（付けるときに入力欄の幅を詰める）。
        for (spin, buddy) in [
            (c.size_spin, c.size),
            (c.scroll_bar_width_spin, c.scroll_bar_width),
            (c.padding_spin, c.padding),
            (c.background_opacity_spin, c.background_opacity),
            (c.background_item_opacity_spin, c.background_item_opacity),
        ] {
            // 範囲は、上の矢印で数が増える向き（既定は逆）にするためだけに決める。数は step で決める。
            // SAFETY: spin と buddy はダイアログの子の有効なウィンドウ。
            unsafe {
                SendMessageW(spin, UDM_SETBUDDY, Some(WPARAM(buddy.0 as usize)), None);
                SendMessageW(
                    spin,
                    UDM_SETRANGE32,
                    Some(WPARAM(SPIN_RANGE.0 as usize)),
                    Some(LPARAM(SPIN_RANGE.1 as isize)),
                );
                // 端にいると矢印を押しても知らせないので、真ん中に置く（step で動かさない）。
                SendMessageW(
                    spin,
                    UDM_SETPOS32,
                    None,
                    Some(LPARAM(((SPIN_RANGE.0 + SPIN_RANGE.1) / 2) as isize)),
                );
                // 付け直すと矢印が触れるようになるので、相手の入力欄に合わせる。
                _ = EnableWindow(spin, IsWindowEnabled(buddy).as_bool());
            }
        }
    }

    /// `page` 番目のページのコントロールだけを見せる。
    fn show_page(&self, page: usize) {
        let c = &self.controls;
        // SAFETY: 一覧は有効なウィンドウ。
        unsafe { SendMessageW(c.pages, LB_SETCURSEL, Some(WPARAM(page)), None) };
        for (index, controls) in c.page_controls.iter().enumerate() {
            let show = if index == page { SW_SHOW } else { SW_HIDE };
            for &control in controls {
                // SAFETY: control はダイアログの子の有効なウィンドウ。
                unsafe { _ = ShowWindow(control, show) };
            }
        }
    }

    /// コントロールから編集中の設定を組み立てる。読めない入力があれば、その文と入力欄を返す。
    fn read(&self) -> Result<Config, (&'static str, HWND)> {
        let c = &self.controls;
        let mut config = self.base.borrow().clone();
        // SAFETY: コンボボックスは有効なウィンドウ。
        let selected = unsafe { SendMessageW(c.font, CB_GETCURSEL, None, None) }.0;
        if let Ok(selected) = usize::try_from(selected)
            && selected != self.base_font.get()
            && let Some(choice) = self.fonts.get(selected)
        {
            config.font.families =
                families_choosing(&config.font.families, &self.base_indices.borrow(), choice)
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect();
        }
        config.font.size_pt = config::parse_font_size(&get_text(c.size)).ok_or((
            "フォントの大きさは、1 から 128 までの数で書いてください。",
            c.size,
        ))?;
        if let Some(index) = checked(&c.wide_glyph) {
            config.font.wide_glyph = WideGlyph::ALL[index];
        }
        config.scroll_bar.width = config::parse_scroll_bar_width(&get_text(c.scroll_bar_width))
            .ok_or((
                "スクロールバーの幅は、4 から 64 までの整数で書いてください。",
                c.scroll_bar_width,
            ))?;
        config.window.padding = config::parse_padding(&get_text(c.padding))
            .ok_or(("余白は、0 から 64 までの整数で書いてください。", c.padding))?;
        if let Some(index) = checked(&c.paste_confirm) {
            config.paste.confirm = PasteConfirm::ALL[index];
        }
        let profiles = self.profiles.borrow().clone();
        if let Err((index, field, message)) = check_profiles(&profiles) {
            // エラーのあるプロファイルを選び、その入力欄へ移れるようにする。
            if self.selected_profile() != Some(index) {
                // SAFETY: 一覧は有効なウィンドウ。
                unsafe { SendMessageW(c.profiles_list, LB_SETCURSEL, Some(WPARAM(index)), None) };
                self.on_profile_selected();
            }
            let control = match field {
                ProfileField::Name => c.profile_name,
                ProfileField::Command => c.profile_command,
            };
            return Err((message, control));
        }
        config.default_profile = default_profile_name(
            &profiles,
            self.default_profile.get(),
            self.base.borrow().default_profile.as_deref(),
        );
        config.profiles = profiles;
        if self.selected_background_image().is_some()
            && self.custom_item_opacity()
            && config::parse_image_opacity(&get_text(c.background_item_opacity)).is_none()
        {
            return Err((
                "選択した項目の不透明度は、0 から 1 までの数で書いてください。",
                c.background_item_opacity,
            ));
        }
        config.background.images = self.background_images.borrow().clone();
        config.background.image_opacity = self.default_opacity().ok_or((
            "不透明度は、0 から 1 までの数で書いてください。",
            c.background_opacity,
        ))?;
        // SAFETY: コンボボックスは有効なウィンドウ。
        let fit = unsafe { SendMessageW(c.background_fit, CB_GETCURSEL, None, None) }.0;
        if let Some(&fit) = usize::try_from(fit)
            .ok()
            .and_then(|fit| BackgroundFit::ALL.get(fit))
        {
            config.background.fit = fit;
        }
        config.keys = self.keys.borrow().clone();
        config.colors = self.colors.borrow().clone();
        Ok(config)
    }

    /// 編集中の設定を読み、読めれば端末に反映する（前に反映したものと同じなら何もしない）。読めた
    /// 設定を返す。読めなければエラーを見せる。
    fn preview(&self) -> Option<Config> {
        // SAFETY: hwnd は有効なウィンドウ。掛けていないタイマーを止めても害はない。
        unsafe { _ = KillTimer(Some(self.hwnd), PREVIEW_TIMER_ID) };
        let config = match self.read() {
            Ok(config) => config,
            Err((message, _)) => {
                set_text(self.controls.error, message);
                return None;
            }
        };
        set_text(self.controls.error, "");
        self.preview_config(&config);
        Some(config)
    }

    fn preview_config(&self, config: &Config) {
        if *self.previewed.borrow() == *config {
            return;
        }
        let Ok(mut host) = self.host.try_borrow_mut() else {
            tracing::debug!("skipped a nested settings preview");
            return;
        };
        host.preview(config);
        *self.previewed.borrow_mut() = config.clone();
    }

    /// 編集中の設定を反映して設定ファイルに書く。書けたら `true`。
    fn apply(&self) -> bool {
        let Some(edited) = self.preview() else {
            // 読めない入力欄へ移る。エラーメッセージは preview が見せている。
            if let Err((_, control)) = self.read() {
                // SAFETY: control はダイアログの子の有効なウィンドウ。
                unsafe { _ = SetFocus(Some(control)) };
            }
            return false;
        };
        let base = self.base.borrow().clone();
        let saved = match self.host.try_borrow_mut() {
            Ok(mut host) => host.save(&base, &edited),
            Err(_) => return false,
        };
        if let Err(text) = saved {
            // SAFETY: hwnd は有効なウィンドウで、文字列はこの呼び出しの間は生きている。借用は持っていない。
            unsafe {
                MessageBoxW(
                    Some(self.hwnd),
                    &HSTRING::from(text),
                    &HSTRING::from("fantermo"),
                    MB_OK | MB_ICONWARNING,
                )
            };
            return false;
        }
        // SAFETY: コンボボックスは有効なウィンドウ。
        let selected = unsafe { SendMessageW(self.controls.font, CB_GETCURSEL, None, None) }.0;
        if let Ok(selected) = usize::try_from(selected)
            && selected != self.base_font.get()
            && let Some(choice) = self.fonts.get(selected)
        {
            let indices =
                families_choosing(&base.font.families, &self.base_indices.borrow(), choice)
                    .into_iter()
                    .map(|(_, index)| index)
                    .collect();
            *self.base_indices.borrow_mut() = indices;
            self.base_font.set(selected);
        }
        *self.base.borrow_mut() = edited;
        true
    }

    /// 最後に書いたときの設定に戻して閉じる。
    fn cancel(&self) {
        // SAFETY: hwnd は有効なウィンドウ。掛けていないタイマーを止めても害はない。
        unsafe { _ = KillTimer(Some(self.hwnd), PREVIEW_TIMER_ID) };
        let base = self.base.borrow().clone();
        self.preview_config(&base);
        self.close();
    }

    fn close(&self) {
        // SAFETY: hwnd はこのダイアログ。
        unsafe { _ = DestroyWindow(self.hwnd) };
    }

    /// 入力欄を打ったので、少し待ってから反映する。
    fn schedule_preview(&self) {
        // SAFETY: hwnd は有効なウィンドウ。同じ番号のタイマーは掛け直しになる。
        if unsafe {
            SetTimer(
                Some(self.hwnd),
                PREVIEW_TIMER_ID,
                PREVIEW_DELAY.as_millis() as u32,
                None,
            )
        } == 0
        {
            tracing::warn!(
                "failed to schedule the settings preview: {}",
                windows::core::Error::from_thread()
            );
            self.preview();
        }
    }

    /// 上下の矢印のボタン `spin` が押された。相手の入力欄の数を `delta` の向きに一つ動かす。
    fn step(&self, spin: HWND, delta: i32) {
        let c = &self.controls;
        if spin == c.size_spin {
            let text = get_text(c.size);
            let current = config::parse_font_size(&text).unwrap_or(self.base.borrow().font.size_pt);
            set_text(c.size, &format_size(step_size(current, delta)));
        } else if spin == c.scroll_bar_width_spin {
            let text = get_text(c.scroll_bar_width);
            let current = config::parse_scroll_bar_width(&text)
                .unwrap_or(self.base.borrow().scroll_bar.width);
            set_text(
                c.scroll_bar_width,
                &step_pixels(current, delta, &config::SCROLL_BAR_WIDTH_RANGE).to_string(),
            );
        } else if spin == c.padding_spin {
            let current = config::parse_padding(&get_text(c.padding))
                .unwrap_or(self.base.borrow().window.padding);
            set_text(
                c.padding,
                &step_pixels(current, delta, &config::PADDING_RANGE).to_string(),
            );
        } else if spin == c.background_opacity_spin {
            let text = get_text(c.background_opacity);
            let current = config::parse_image_opacity(&text)
                .unwrap_or(self.base.borrow().background.image_opacity);
            set_text(
                c.background_opacity,
                &format_opacity(step_opacity(current, delta)),
            );
        } else if spin == c.background_item_opacity_spin {
            // 読めなければ、既定の不透明度から動かす。
            let current = config::parse_image_opacity(&get_text(c.background_item_opacity))
                .or_else(|| self.default_opacity())
                .unwrap_or(self.base.borrow().background.image_opacity);
            set_text(
                c.background_item_opacity,
                &format_opacity(step_opacity(current, delta)),
            );
        }
        // 矢印は打ち間違えないので、待たずに反映する。
        self.preview();
    }
}

/// フォントの名前の並び `families`（それぞれの番号は `indices`）で、`chosen` を先頭にした並び。同じ
/// 書体（名前か番号が同じもの）は後ろから除く。名前と番号の組で返す。
fn families_choosing(
    families: &[String],
    indices: &[Option<u32>],
    chosen: &FontChoice,
) -> Vec<(String, Option<u32>)> {
    let same = |name: &str, index: Option<u32>| {
        name.eq_ignore_ascii_case(&chosen.name) || (index.is_some() && index == chosen.index)
    };
    std::iter::once((chosen.name.clone(), chosen.index))
        .chain(
            families
                .iter()
                .enumerate()
                .map(|(i, name)| (name.clone(), indices.get(i).copied().flatten()))
                .filter(|(name, index)| !same(name, *index)),
        )
        .collect()
}

/// フォントの大きさを、矢印で一つ動かした値。整数の大きさへ動かし、範囲の端で止める。
fn step_size(current: f32, delta: i32) -> f32 {
    let next = if delta > 0 {
        current.floor() + 1.0
    } else {
        current.ceil() - 1.0
    };
    next.clamp(
        *config::FONT_SIZE_RANGE_PT.start(),
        *config::FONT_SIZE_RANGE_PT.end(),
    )
}

/// スクロールバーの幅や余白のピクセルを、矢印で一つ動かした値。範囲 `range` の端で止める。
fn step_pixels(current: u32, delta: i32, range: &RangeInclusive<u32>) -> u32 {
    let next = if delta > 0 {
        current.saturating_add(1)
    } else {
        current.saturating_sub(1)
    };
    next.clamp(*range.start(), *range.end())
}

/// 入力欄に見せる、フォントの大きさ。整数なら小数点を付けない。
fn format_size(size_pt: f32) -> String {
    size_pt.to_string()
}

/// 画像の不透明度を、矢印で一つ動かした値。[`OPACITY_STEP`] の倍数へ動かし、範囲の端で止める。
fn step_opacity(current: f32, delta: i32) -> f32 {
    // 0.35 のように刻みの倍数を書いた値が、f32 の誤差で倍数の手前に見えないようにする。
    const TOLERANCE: f32 = 1e-3;
    let steps = current / OPACITY_STEP;
    let next = if delta > 0 {
        (steps + TOLERANCE).floor() + 1.0
    } else {
        (steps - TOLERANCE).ceil() - 1.0
    };
    // 刻みの倍数を f32 で掛けた端数（0.35000002 など）を出さないように、百分の一に丸める。
    let next = (next * OPACITY_STEP * 100.0).round() / 100.0;
    next.clamp(
        *config::IMAGE_OPACITY_RANGE.start(),
        *config::IMAGE_OPACITY_RANGE.end(),
    )
}

/// 入力欄に見せる、画像の不透明度。
fn format_opacity(opacity: f32) -> String {
    opacity.to_string()
}

/// 背景画像を選ぶダイアログの、ファイルの種類の絞り込み。
fn image_filters() -> Vec<file_dialog::Filter> {
    let spec = IMAGE_EXTENSIONS
        .iter()
        .map(|extension| format!("*.{extension}"))
        .collect::<Vec<_>>()
        .join(";");
    vec![
        file_dialog::Filter {
            name: "画像".to_owned(),
            spec,
        },
        file_dialog::Filter {
            name: "すべてのファイル".to_owned(),
            spec: "*.*".to_owned(),
        },
    ]
}

fn set_text(control: HWND, text: &str) {
    // SAFETY: control は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
    unsafe { _ = SetWindowTextW(control, &HSTRING::from(text)) };
}

fn get_text(control: HWND) -> String {
    // SAFETY: control は有効なウィンドウ。
    let length = unsafe { GetWindowTextLengthW(control) }.max(0) as usize;
    let mut buffer = vec![0u16; length + 1];
    // SAFETY: buffer は length + 1 個の u16 を書き込める。
    let copied = unsafe { GetWindowTextW(control, &mut buffer) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..copied])
}

/// ラジオボタンの並び `buttons` のうち、`index` 番目だけを選ぶ。
fn check_one(buttons: &[HWND], index: Option<usize>) {
    for (i, &button) in buttons.iter().enumerate() {
        let state = if Some(i) == index { BST_CHECKED.0 } else { 0 };
        // SAFETY: button は有効なボタン。
        unsafe { SendMessageW(button, BM_SETCHECK, Some(WPARAM(state as usize)), None) };
    }
}

/// ラジオボタンの並び `buttons` のうち、選んでいるものの位置。
fn checked(buttons: &[HWND]) -> Option<usize> {
    buttons.iter().position(|&button| {
        // SAFETY: button は有効なボタン。
        unsafe { SendMessageW(button, BM_GETCHECK, None, None) }.0 == BST_CHECKED.0 as isize
    })
}

/// ダイアログに結び付けた状態。WM_DESTROY で外すまで有効。
fn state_of<'a>(dialog: HWND) -> Option<&'a State<'a>> {
    // SAFETY: GWLP_USERDATA には run が入れた State のポインタか 0 が入っている。
    let pointer = unsafe { GetWindowLongPtrW(dialog, GWLP_USERDATA) } as *const State;
    // SAFETY: 0 でなければ、run の中の State を指し、ダイアログが閉じるまで有効。
    unsafe { pointer.as_ref() }
}

extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(state) = state_of(hwnd) else {
        // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            let code = ((wparam.0 >> 16) & 0xffff) as u32;
            match (id, code) {
                (id, _) if id == IDOK.0 => {
                    if state.apply() {
                        state.close();
                    }
                }
                (id, _) if id == IDCANCEL.0 => state.cancel(),
                (ID_APPLY, BN_CLICKED) => {
                    state.apply();
                }
                (ID_PAGES, LBN_SELCHANGE) => {
                    // SAFETY: 一覧は有効なウィンドウ。
                    let page =
                        unsafe { SendMessageW(state.controls.pages, LB_GETCURSEL, None, None) }.0;
                    if let Ok(page) = usize::try_from(page)
                        && page < PAGES.len()
                    {
                        state.show_page(page);
                    }
                }
                (ID_FONT, CBN_SELCHANGE) => {
                    state.preview();
                }
                (ID_PROFILES, LBN_SELCHANGE) => state.on_profile_selected(),
                (ID_PROFILE_ADD | ID_PROFILE_REMOVE | ID_PROFILE_DEFAULT, BN_CLICKED) => {
                    state.on_profile_button(id);
                }
                (ID_PROFILE_NAME | ID_PROFILE_COMMAND, EN_CHANGE) => state.on_profile_edited(),
                (ID_BACKGROUND_IMAGES, LBN_SELCHANGE) => state.update_background_buttons(),
                (
                    ID_BACKGROUND_ADD_FILES | ID_BACKGROUND_ADD_FOLDER | ID_BACKGROUND_REMOVE,
                    BN_CLICKED,
                ) => state.on_background_button(id),
                (ID_BACKGROUND_FIT, CBN_SELCHANGE) => {
                    state.refresh_background_defaults();
                    state.preview();
                }
                (ID_BACKGROUND_OPACITY, EN_CHANGE) => {
                    state.refresh_background_defaults();
                    state.schedule_preview();
                }
                (ID_BACKGROUND_ITEM_FIT, CBN_SELCHANGE) => state.on_background_item_fit(),
                (ID_BACKGROUND_ITEM_OPACITY, EN_CHANGE) => state.on_background_item_opacity(),
                (
                    ID_BACKGROUND_ITEM_OPACITY_DEFAULT | ID_BACKGROUND_ITEM_OPACITY_CUSTOM,
                    BN_CLICKED,
                ) => state.on_background_item_opacity_choice(),
                (ID_KEYS, LBN_SELCHANGE) => state.update_key_buttons(),
                (ID_KEYS, LBN_DBLCLK) => state.on_key_button(ID_KEY_CHANGE),
                (ID_KEY_ADD | ID_KEY_CHANGE | ID_KEY_REMOVE | ID_KEY_RESET, BN_CLICKED) => {
                    state.on_key_button(id);
                }
                (id, BN_CLICKED) if id >= ID_COLORS => state.on_color_button(id),
                (id, BN_CLICKED)
                    if (ID_COLOR_SCHEME..ID_COLOR_SCHEME + ColorScheme::ALL.len() as i32)
                        .contains(&id) =>
                {
                    state.on_color_scheme();
                }
                (ID_SIZE | ID_SCROLL_BAR_WIDTH | ID_PADDING, EN_CHANGE) => state.schedule_preview(),
                (id, BN_CLICKED)
                    if (ID_WIDE_GLYPH..ID_WIDE_GLYPH + WideGlyph::ALL.len() as i32)
                        .contains(&id)
                        || (ID_PASTE_CONFIRM..ID_PASTE_CONFIRM + 3).contains(&id) =>
                {
                    state.preview();
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => match state.swatch_brush(HWND(lparam.0 as *mut _)) {
            Some(brush) => LRESULT(brush.0 as isize),
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
            None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        },
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY の lparam は NMHDR で始まる構造体を指す。
            let header = unsafe { &*(lparam.0 as *const NMHDR) };
            if header.code == UDN_DELTAPOS {
                // SAFETY: UDN_DELTAPOS の lparam は NMUPDOWN を指す。
                let updown = unsafe { &*(lparam.0 as *const NMUPDOWN) };
                state.step(header.hwndFrom, updown.iDelta);
                // 矢印のボタン自身の位置は動かさない（入力欄の数はこちらで決める）。
                return LRESULT(1);
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == PREVIEW_TIMER_ID => {
            state.preview();
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xffff) as u32;
            // SAFETY: WM_DPICHANGED の lparam は推奨された矩形を指す。
            let suggested = unsafe { &*(lparam.0 as *const RECT) };
            // SAFETY: hwnd はこのダイアログ。
            if let Err(e) = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            } {
                tracing::warn!("failed to move the settings dialog for {dpi} DPI: {e}");
            }
            state.set_dpi(dpi);
            LRESULT(0)
        }
        // Enter を自分で使わないコントロールは、既定のボタンをこの二つで問い合わせて押させる。
        DM_GETDEFID => LRESULT(((DC_HASDEFID as isize) << 16) | IDOK.0 as isize),
        WM_NEXTDLGCTL => {
            // lparam が 0 でなければ、wparam は移す先の子のウィンドウ。順に移すほうは使われないので
            // 扱わない。
            if lparam.0 & 0xffff != 0 {
                // SAFETY: wparam はこのダイアログの子のウィンドウのハンドルとして渡される。
                unsafe { _ = SetFocus(Some(HWND(wparam.0 as *mut _))) };
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            state.cancel();
            LRESULT(0)
        }
        WM_DESTROY => {
            state.closed.set(true);
            // SAFETY: hwnd はこのダイアログ。以後、状態のポインタは使わせない。
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            LRESULT(0)
        }
        _ => {
            // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(name: &str, index: Option<u32>) -> FontChoice {
        FontChoice {
            name: name.to_owned(),
            index,
        }
    }

    #[test]
    fn chosen_family_goes_first_without_duplicates() {
        let families = ["Bizin Gothic", "BIZ UDGothic", "Consolas"].map(str::to_owned);
        let indices = [Some(96), Some(77), Some(5)];
        assert_eq!(
            families_choosing(&families, &indices, &choice("BIZ UDゴシック", Some(77))),
            [
                ("BIZ UDゴシック".to_owned(), Some(77)),
                ("Bizin Gothic".to_owned(), Some(96)),
                ("Consolas".to_owned(), Some(5)),
            ]
        );
        assert_eq!(
            families_choosing(&families, &indices, &choice("consolas", None)),
            [
                ("consolas".to_owned(), None),
                ("Bizin Gothic".to_owned(), Some(96)),
                ("BIZ UDGothic".to_owned(), Some(77)),
            ]
        );
        // 番号の分からない名前は、名前だけで比べる。
        assert_eq!(
            families_choosing(&families, &[], &choice("Cascadia Mono", Some(3))).len(),
            4
        );
    }

    #[test]
    fn arrows_step_to_whole_sizes_within_the_range() {
        assert_eq!(step_size(12.0, 1), 13.0);
        assert_eq!(step_size(12.0, -1), 11.0);
        assert_eq!(step_size(10.5, 1), 11.0);
        assert_eq!(step_size(10.5, -1), 10.0);
        assert_eq!(step_size(1.0, -1), 1.0);
        assert_eq!(step_size(128.0, 1), 128.0);
        let width = config::SCROLL_BAR_WIDTH_RANGE;
        assert_eq!(step_pixels(12, 1, &width), 13);
        assert_eq!(step_pixels(4, -1, &width), 4);
        assert_eq!(step_pixels(64, 1, &width), 64);
        let padding = config::PADDING_RANGE;
        assert_eq!(step_pixels(4, -1, &padding), 3);
        assert_eq!(step_pixels(0, -1, &padding), 0);
        assert_eq!(step_pixels(64, 1, &padding), 64);
    }

    #[test]
    fn key_rows_list_every_action_in_order() {
        let combo = |text: &str| KeyCombo::parse(text).unwrap();
        let mut keys = Keymap::default();
        keys.bind(combo("ctrl+shift+t"), None);
        keys.bind(combo("ctrl+page_down"), Some(Action::NextTab));
        let rows = key_rows(&keys);
        let texts: Vec<String> = rows.iter().map(|row| key_row_text(*row)).collect();
        assert_eq!(
            texts,
            [
                "コピー\tCtrl+C",
                "コピー\tCtrl+Insert",
                "貼り付け\tShift+Insert",
                "コンテキストメニュー\tContextMenu",
                "コンテキストメニュー\tShift+F10",
                "新しいタブ\t（なし）",
                "タブを閉じる\tCtrl+Shift+W",
                "次のタブ\tCtrl+Tab",
                "次のタブ\tCtrl+PageDown",
                "前のタブ\tCtrl+Shift+Tab",
            ]
        );
        assert!(!is_default_binding(&keys, Action::NewTab));
        assert!(!is_default_binding(&keys, Action::NextTab));
        assert!(is_default_binding(&keys, Action::Copy));
        // 外して付け直すと並びは変わるが、既定と同じ。
        keys.bind(combo("ctrl+c"), None);
        keys.bind(combo("ctrl+c"), Some(Action::Copy));
        assert!(is_default_binding(&keys, Action::Copy));
    }

    fn profile(name: &str, command: &str) -> Profile {
        Profile {
            name: name.to_owned(),
            command: command.to_owned(),
            backend: BackendKind::Conpty,
        }
    }

    #[test]
    fn profile_rows_mark_the_default() {
        let zsh = profile("zsh", "zsh -l");
        assert_eq!(profile_row_text(&zsh, false), "zsh\tzsh -l");
        assert_eq!(profile_row_text(&zsh, true), "zsh（既定）\tzsh -l");
    }

    #[test]
    fn added_profiles_get_a_free_name() {
        let mut profiles = vec![profile("a", "x")];
        assert_eq!(new_profile_name(&profiles), NEW_PROFILE_NAME);
        profiles.push(profile(NEW_PROFILE_NAME, "x"));
        profiles.push(profile(&format!("{NEW_PROFILE_NAME} 2"), "x"));
        assert_eq!(new_profile_name(&profiles), format!("{NEW_PROFILE_NAME} 3"));
    }

    #[test]
    fn profiles_are_checked_like_the_file() {
        assert_eq!(
            check_profiles(&[profile("a", "x"), profile("b", "y")]),
            Ok(())
        );
        assert!(matches!(
            check_profiles(&[profile("a", "x"), profile(" ", "y")]),
            Err((1, ProfileField::Name, _))
        ));
        assert!(matches!(
            check_profiles(&[profile("a", "")]),
            Err((0, ProfileField::Command, _))
        ));
        assert!(matches!(
            check_profiles(&[profile("a", "x"), profile("a", "y")]),
            Err((1, ProfileField::Name, _))
        ));
    }

    #[test]
    fn the_first_profile_stays_unwritten_as_the_default() {
        let profiles = [profile("a", "x"), profile("b", "y")];
        assert_eq!(default_profile_name(&profiles, 0, None), None);
        assert_eq!(
            default_profile_name(&profiles, 1, None).as_deref(),
            Some("b")
        );
        // 開いたときに書いてあれば、先頭でも名前を書く。
        assert_eq!(
            default_profile_name(&profiles, 0, Some("b")).as_deref(),
            Some("a")
        );
    }

    #[test]
    fn colorrefs_put_red_in_the_low_byte() {
        let color = Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        };
        assert_eq!(colorref(color), COLORREF(0x0056_3412));
        assert_eq!(rgb_of(COLORREF(0x0056_3412)), color);
    }

    #[test]
    fn arrows_step_the_opacity_by_twentieths_within_the_range() {
        assert_eq!(step_opacity(0.2, 1), 0.25);
        assert_eq!(step_opacity(0.2, -1), 0.15);
        assert_eq!(step_opacity(0.8, 1), 0.85);
        assert_eq!(step_opacity(0.8, -1), 0.75);
        assert_eq!(step_opacity(0.35, 1), 0.4);
        assert_eq!(step_opacity(0.35, -1), 0.3);
        assert_eq!(step_opacity(0.33, 1), 0.35);
        assert_eq!(step_opacity(0.33, -1), 0.3);
        assert_eq!(step_opacity(1.0, 1), 1.0);
        assert_eq!(step_opacity(0.0, -1), 0.0);
        assert_eq!(format_opacity(step_opacity(0.8, 1)), "0.85");
        assert_eq!(format_opacity(1.0), "1");
    }

    #[test]
    fn background_rows_show_the_item_settings() {
        let mut image = BackgroundImage::new(r"C:\a.png");
        assert_eq!(background_row_text(&image), r"C:\a.png");
        image.fit = Some(BackgroundFit::Stretch);
        assert_eq!(background_row_text(&image), r"C:\a.png（拡大して表示）");
        image.image_opacity = Some(0.3);
        assert_eq!(
            background_row_text(&image),
            r"C:\a.png（拡大して表示、不透明度 0.3）"
        );
        image.fit = None;
        assert_eq!(background_row_text(&image), r"C:\a.png（不透明度 0.3）");
    }

    #[test]
    fn item_fits_follow_the_background_first() {
        assert_eq!(item_fit_choice(None), 0);
        assert_eq!(item_fit_from_choice(0), Some(None));
        for (index, fit) in BackgroundFit::ALL.into_iter().enumerate() {
            assert_eq!(item_fit_choice(Some(fit)), index + 1);
            assert_eq!(item_fit_from_choice(index + 1), Some(Some(fit)));
        }
        assert_eq!(item_fit_from_choice(BackgroundFit::ALL.len() + 1), None);
        assert_eq!(
            default_choice_label(fit_label(BackgroundFit::Tile)),
            "既定（並べて表示）"
        );
        assert_eq!(default_choice_label(&format_opacity(0.5)), "既定（0.5）");
    }

    #[test]
    fn every_fit_has_a_label() {
        assert_eq!(BACKGROUND_FIT_LABELS.len(), BackgroundFit::ALL.len());
        let filters = image_filters();
        assert_eq!(filters[0].spec, "*.png;*.jpg;*.jpeg;*.bmp;*.gif;*.webp");
    }

    #[test]
    fn every_color_has_a_label_and_fits_above_the_buttons() {
        for table in ColorTable::ALL {
            assert_eq!(color_labels(table).len(), table.keys().len());
        }
        assert_eq!(COLOR_TABLE_LABELS.len(), ColorTable::ALL.len());
        assert_eq!(COLOR_SCHEME_LABELS.len(), ColorScheme::ALL.len());
        let items = color_items();
        assert_eq!(items.len(), 15);
        assert_eq!(items[0], (ColorTable::Terminal, 0));
        assert_eq!(items[11], (ColorTable::ScrollBar, 0));
        // 表の見出しと配色の行と色の行。
        let rows = (ColorTable::ALL.len() + 1 + items.len()) as i32;
        let button_top = CLIENT_HEIGHT - MARGIN - BUTTON_HEIGHT;
        assert!(MARGIN + rows * COLOR_ROW_HEIGHT <= button_top - MARGIN);
        let row_width = LABEL_WIDTH
            + SWATCH_WIDTH
            + COLOR_TEXT_WIDTH
            + CHOOSE_WIDTH
            + COLOR_RESET_WIDTH
            + 3 * BUTTON_GAP;
        assert!(row_width <= CLIENT_WIDTH - (MARGIN + PAGES_WIDTH + MARGIN) - MARGIN);
    }

    #[test]
    fn default_colors_follow_the_scheme_and_the_terminal_colors() {
        let tab_bar = [Rgb { r: 9, g: 9, b: 9 }; config::TAB_BAR_COLOR_KEYS.len()];
        let light = ColorsConfig {
            scheme: ColorScheme::Light,
            ..ColorsConfig::default()
        };
        let scheme = config::LIGHT_COLORS;
        assert_eq!(
            default_color(&light, &tab_bar, ColorTable::Terminal, 0),
            scheme.background
        );
        assert_eq!(
            default_color(&light, &tab_bar, ColorTable::Terminal, 1),
            scheme.foreground
        );
        assert_eq!(
            default_color(&light, &tab_bar, ColorTable::TabBar, 3),
            tab_bar[3]
        );
        let thumb =
            |colors: &ColorsConfig| default_color(colors, &tab_bar, ColorTable::ScrollBar, 1);
        assert_eq!(
            thumb(&light),
            scroll_bar::Palette::from_terminal(scheme.background, scheme.foreground).colors()[1]
        );
        // 書いた背景色は、端末の色の既定には効かず、スクロールバーの既定には効く。
        let black = Rgb { r: 0, g: 0, b: 0 };
        let written = ColorsConfig {
            terminal: [Some(black), None],
            ..light.clone()
        };
        assert_eq!(
            default_color(&written, &tab_bar, ColorTable::Terminal, 0),
            scheme.background
        );
        assert_eq!(
            thumb(&written),
            scroll_bar::Palette::from_terminal(black, scheme.foreground).colors()[1]
        );
    }

    #[test]
    fn sizes_are_shown_as_written() {
        assert_eq!(format_size(12.0), "12");
        assert_eq!(format_size(10.5), "10.5");
    }
}
