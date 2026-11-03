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
    cell::{Cell, Ref, RefCell},
    ops::RangeInclusive,
    path::PathBuf,
    sync::OnceLock,
    time::Duration,
};

use fantermo_core::{
    background::IMAGE_EXTENSIONS,
    config::{
        self, BackendKind, BackgroundFit, BackgroundImage, ClipboardRead, ColorScheme, ColorTable,
        ColorsConfig, Config, PasteConfirm, Profile, ProfileError, WideGlyph, format_color,
    },
    frame::Rgb,
    keymap::{Action, KeyCombo, Keymap},
};
use windows::{
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            COLOR_BTNFACE, CreateFontIndirectW, CreateSolidBrush, DT_CALCRECT, DT_WORDBREAK,
            DeleteObject, DrawTextW, GetDC, GetMonitorInfoW, HBRUSH, HFONT, HGDIOBJ,
            InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, ReleaseDC,
            SelectObject,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{
                CB_SETMINVISIBLE,
                Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW},
                ICC_UPDOWN_CLASS, INITCOMMONCONTROLSEX, InitCommonControlsEx, NMHDR, NMUPDOWN,
                UDN_DELTAPOS,
            },
            HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow},
            Input::KeyboardAndMouse::{EnableWindow, SetFocus},
            WindowsAndMessaging::{
                BN_CLICKED, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBN_SELCHANGE, CW_USEDEFAULT,
                CreateWindowExW, DC_HASDEFID, DM_GETDEFID, DefWindowProcW, DestroyWindow,
                EN_CHANGE, GWLP_USERDATA, GetWindowLongPtrW, GetWindowRect, IDC_ARROW, IDCANCEL,
                IDOK, IDYES, KillTimer, LBN_DBLCLK, LBN_SELCHANGE, LoadCursorW, MB_ICONQUESTION,
                MB_ICONWARNING, MB_OK, MB_YESNO, MessageBoxW, MoveWindow, RegisterClassW, SW_HIDE,
                SW_SHOW, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW, SetTimer,
                SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WM_CLOSE, WM_COMMAND,
                WM_CTLCOLORSTATIC, WM_DESTROY, WM_DPICHANGED, WM_NEXTDLGCTL, WM_NOTIFY, WM_SETFONT,
                WM_TIMER, WNDCLASSW, WS_CAPTION, WS_EX_CLIENTEDGE, WS_EX_CONTROLPARENT,
                WS_EX_DLGMODALFRAME, WS_POPUP, WS_SYSMENU,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{file_dialog, key_capture, modal, scroll_bar, ui_font};

mod widgets;

use widgets::{
    Builder, ChoiceBox, DipRect, ListBox, NumberField, NumberInput, NumberKind, RadioGroup,
    get_text, set_text,
};

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
/// `clipboard.read` のラジオボタン。[`ClipboardRead::ALL`] の順に ID が続く。
const ID_CLIPBOARD_READ: i32 = 310;
const ID_PASTE_CONFIRM_SIZE: i32 = 320;
const ID_PASTE_CONFIRM_SIZE_SPIN: i32 = 321;
const ID_PROFILES: i32 = 400;
const ID_PROFILE_ADD: i32 = 401;
const ID_PROFILE_REMOVE: i32 = 402;
const ID_PROFILE_DEFAULT: i32 = 403;
const ID_PROFILE_NAME: i32 = 404;
const ID_PROFILE_COMMAND: i32 = 405;
const ID_PROFILE_BACKEND: i32 = 406;
/// `cygwin_root` か `distribution` の入力欄。どちらかは接続方式で決まる。
const ID_PROFILE_LOCATION: i32 = 407;
const ID_PROFILE_COMMAND_WARNING: i32 = 408;
const ID_PROFILE_ICON: i32 = 409;
const ID_PROFILE_ICON_CHOOSE: i32 = 410;
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
/// 項目の不透明度の「既定」と「指定」のラジオボタン。[`ITEM_OPACITY_CHOICES`] の順に ID が続く。
const ID_BACKGROUND_ITEM_OPACITY_CHOICE: i32 = 460;
const ID_KEYS: i32 = 500;
const ID_KEY_ADD: i32 = 501;
const ID_KEY_CHANGE: i32 = 502;
const ID_KEY_REMOVE: i32 = 503;
const ID_KEY_RESET: i32 = 504;

/// 96 DPI のときの寸法（ピクセル）。ウィンドウの DPI に合わせて拡大する。
const BASE_DPI: u32 = 96;
const CLIENT_WIDTH: i32 = 640;
const CLIENT_HEIGHT: i32 = 540;
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
/// フォントの一覧を開いたときに見せる行の数。
const VISIBLE_FONTS: usize = 16;
/// ページの左端と幅。
const PAGE_LEFT: i32 = MARGIN + PAGES_WIDTH + MARGIN;
const PAGE_WIDTH: i32 = CLIENT_WIDTH - PAGE_LEFT - MARGIN;
/// ページの見出しの右に置くコントロールの左端と幅。
const CONTROL_LEFT: i32 = PAGE_LEFT + LABEL_WIDTH;
const CONTROL_WIDTH: i32 = CLIENT_WIDTH - CONTROL_LEFT - MARGIN;
/// 下に並べるボタン（OK、キャンセル、適用）の上端。
const BUTTON_TOP: i32 = CLIENT_HEIGHT - MARGIN - BUTTON_HEIGHT;

/// ページの `index` 番目の行の上端。
fn row(index: i32) -> i32 {
    MARGIN + index * (ROW_HEIGHT + MARGIN)
}

/// 上端が `top` の行の見出しの矩形。
fn label_rect(top: i32) -> DipRect {
    [PAGE_LEFT, top + LABEL_OFFSET, LABEL_WIDTH, ROW_HEIGHT]
}

/// ページ。一覧の順。
const PAGES: [&str; 6] = ["外観", "背景", "動作", "プロファイル", "キー", "色"];

/// プロファイルの一覧の高さ。
const PROFILE_LIST_HEIGHT: i32 = 98;
/// プロファイルのページの下の説明の文の高さ。
const PROFILE_NOTE_HEIGHT: i32 = 3 * ROW_HEIGHT;
/// プロファイルの接続方式の選択肢。[`BackendKind::ALL`] の順。
const PROFILE_BACKEND_LABELS: [&str; BackendKind::ALL.len()] = [
    "ConPTY（Windows の疑似コンソール）",
    "Cygwin（ブリッジ）",
    "WSL（ブリッジ）",
];
/// `EM_SETCUEBANNER`（入力欄が空のときに薄く見せる文字）。
const EM_SETCUEBANNER: u32 = 0x1501;
/// `EM_SETCUEBANNER` の wParam。入力欄にフォーカスがあっても見せる。
const CUE_BANNER_ALWAYS: usize = 1;
/// プロファイルの一覧の、コマンドラインの列の位置（ダイアログの単位）。
const PROFILE_COLUMN_TAB_STOP: i32 = 120;
/// `EM_SETSEL` と、すべてを選ぶときの範囲。
const EM_SETSEL: u32 = 0x00B1;
const SELECT_FROM_START: usize = 0;
const SELECT_TO_END: isize = -1;
/// 足したプロファイルの名前。同じ名前があれば後ろに番号を付ける。
const NEW_PROFILE_NAME: &str = "新しいプロファイル";

/// プロファイルの一覧の行。名前（既定なら印を添える）とコマンドラインをタブで区切る。ブリッジなら
/// コマンドラインの前に接続方式を添え、空のコマンドラインは「（ログインシェル）」と見せる。
fn profile_row_text(profile: &Profile, is_default: bool) -> String {
    let mark = if is_default { "（既定）" } else { "" };
    let via = match profile.backend {
        BackendKind::Conpty => "",
        BackendKind::Cygwin => "Cygwin: ",
        BackendKind::Wsl => "WSL: ",
    };
    let command = if profile.command.is_empty() && profile.backend.is_bridge() {
        "（ログインシェル）"
    } else {
        &profile.command
    };
    format!("{}{mark}\t{via}{command}", profile.name)
}

/// 接続方式が `backend` のプロファイルの、場所の入力欄の見出しと、空のときに薄く見せる文字。
/// ConPTY では使わない。
fn profile_location_texts(backend: BackendKind) -> (&'static str, &'static str) {
    match backend {
        BackendKind::Conpty => ("場所(&O):", ""),
        BackendKind::Cygwin => (
            "Cygwin のフォルダ(&O):",
            "空欄なら PATH と標準のインストール先から探す",
        ),
        BackendKind::Wsl => (
            "ディストリビューション(&O):",
            "空欄なら既定のディストリビューション",
        ),
    }
}

/// 場所の入力欄の値 `location`（前後の空白を除いたもの）を、接続方式に合わせて `profile` に入れる。
fn set_profile_location(profile: &mut Profile, location: String) {
    let location = (!location.is_empty()).then_some(location);
    (profile.cygwin_root, profile.distribution) = match profile.backend {
        BackendKind::Conpty => (None, None),
        BackendKind::Cygwin => (location, None),
        BackendKind::Wsl => (None, location),
    };
}

/// `profile` のコマンドラインの入力欄の下に出す警告の文。ブリッジのプロファイルで Windows のプログラムを
/// 起動するときだけ。
fn profile_command_warning(profile: &Profile) -> Option<&'static str> {
    profile.windows_program_in_bridge()?;
    Some(match profile.backend {
        BackendKind::Cygwin => {
            "Windows のプログラムは指定しないでください。Cygwin の中のコマンド（例: /bin/zsh -l）を入力するか、空欄にしてください。空欄ならログインシェルを起動します。"
        }
        BackendKind::Conpty | BackendKind::Wsl => {
            "Windows のプログラムは指定しないでください。WSL の中のコマンド（例: /bin/bash）を入力するか、空欄にしてください。空欄ならログインシェルを起動します。"
        }
    })
}

/// `text` をフォント `font` で、幅 `width` に折り返したときの高さ。`control` の DC で測る。
fn wrapped_text_height(control: HWND, font: HFONT, text: &str, width: i32) -> i32 {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rect = RECT {
        right: width,
        ..RECT::default()
    };
    // SAFETY: control は有効なウィンドウで、font はダイアログが閉じるまで消さない。選んだフォントは
    // DC を返す前に元に戻す。wide と rect は書き込める。
    let height = unsafe {
        let dc = GetDC(Some(control));
        let previous = SelectObject(dc, HGDIOBJ(font.0));
        let height = DrawTextW(dc, &mut wide, &mut rect, DT_CALCRECT | DT_WORDBREAK);
        SelectObject(dc, previous);
        ReleaseDC(Some(control), dc);
        height
    };
    if height == 0 {
        tracing::warn!("failed to measure the text of a settings dialog control");
    }
    height
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

/// 編集中のプロファイル `profiles` を設定ファイルと同じ決まり（[`Profile::validate`]）でチェックする。
/// どれも合えば `Ok`。そうでなければ、最初のエラーの位置と入力欄とエラーメッセージ。
fn check_profiles(profiles: &[Profile]) -> Result<(), (usize, ProfileField, &'static str)> {
    for (index, profile) in profiles.iter().enumerate() {
        let Err(error) = profile.validate(&profiles[..index]) else {
            continue;
        };
        let (field, message) = match error {
            ProfileError::EmptyName => {
                (ProfileField::Name, "プロファイルの名前を入力してください。")
            }
            ProfileError::EmptyCommand => (
                ProfileField::Command,
                "プロファイルのコマンドラインを入力してください。",
            ),
            ProfileError::DuplicateName => {
                (ProfileField::Name, "ほかのプロファイルと同じ名前です。")
            }
        };
        return Err((index, field, message));
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
const BACKGROUND_FIT_LABELS: [&str; BackgroundFit::ALL.len()] = [
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
const COLOR_TEXT: i32 = 1;
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
const COLOR_TABLE_LABELS: [&str; ColorTable::ALL.len()] = ["端末", "タブバー", "スクロールバー"];

/// `colors.scheme` のラジオボタン。[`ColorScheme::ALL`] の順に ID が続く。
const ID_COLOR_SCHEME: i32 = 590;
/// `colors.scheme` の選択肢の表示。[`ColorScheme::ALL`] の順。
const COLOR_SCHEME_LABELS: [&str; ColorScheme::ALL.len()] = ["ダーク", "ライト"];
/// 配色のラジオボタン一つの幅。
const COLOR_SCHEME_WIDTH: i32 = 96;

/// 端末の色の表示。[`config::TERMINAL_COLOR_KEYS`] の順。
const TERMINAL_COLOR_LABELS: [&str; config::TERMINAL_COLOR_KEYS.len()] = ["背景色", "文字色"];

/// タブバーの色の表示。[`config::TAB_BAR_COLOR_KEYS`] の順。
const TAB_BAR_COLOR_LABELS: [&str; config::TAB_BAR_COLOR_KEYS.len()] = [
    "背景",
    "表示しているタブ",
    "表示しているタブの縁",
    "表示しているタブの文字",
    "表示していないタブの文字",
    "タブの区切り線",
    "マウスを載せているタブ",
    "「+」「▾」「≡」",
    "ベルの印",
];

/// スクロールバーの色の表示。[`config::SCROLL_BAR_COLOR_KEYS`] の順。
const SCROLL_BAR_COLOR_LABELS: [&str; config::SCROLL_BAR_COLOR_KEYS.len()] = [
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

/// 色のページの色の数。
const COLOR_ITEM_COUNT: usize = config::TERMINAL_COLOR_KEYS.len()
    + config::TAB_BAR_COLOR_KEYS.len()
    + config::SCROLL_BAR_COLOR_KEYS.len();

/// 色のページの色の並び。テーブルと、テーブルの中の位置。テーブルは [`ColorTable::ALL`] の順。
const COLOR_ITEMS: [(ColorTable, usize); COLOR_ITEM_COUNT] = {
    let tables = [
        (ColorTable::Terminal, config::TERMINAL_COLOR_KEYS.len()),
        (ColorTable::TabBar, config::TAB_BAR_COLOR_KEYS.len()),
        (ColorTable::ScrollBar, config::SCROLL_BAR_COLOR_KEYS.len()),
    ];
    let mut items = [(ColorTable::Terminal, 0); COLOR_ITEM_COUNT];
    let mut item = 0;
    let mut table = 0;
    while table < tables.len() {
        let mut index = 0;
        while index < tables[table].1 {
            items[item] = (tables[table].0, index);
            item += 1;
            index += 1;
        }
        table += 1;
    }
    items
};

/// 色を Win32 の `COLORREF`（`0x00BBGGRR`）にする。
fn colorref(color: Rgb) -> COLORREF {
    COLORREF(u32::from(color.r) | (u32::from(color.g) << 8) | (u32::from(color.b) << 16))
}

/// Win32 の `COLORREF` を色にする。
fn rgb_of(colorref: COLORREF) -> Rgb {
    let [r, g, b, _] = colorref.0.to_le_bytes();
    Rgb { r, g, b }
}

/// 色 `colors` のときの、テーブル `table` の `index` 番目の既定の色（設定に書いていないときに使う色）。
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
    "縮めずに、はみ出た部分を切り取る",
    "縮めずに、右のセルへはみ出す",
    "縮めずに、中央から左右のセルへはみ出す",
];

/// `paste.confirm` の選択肢の表示。[`PasteConfirm::ALL`] の順。
const PASTE_CONFIRM_LABELS: [&str; PasteConfirm::ALL.len()] = [
    "コマンドとして実行される可能性があるとき",
    "改行を含むとき",
    "確認しない",
];

/// `clipboard.read` の選択肢の表示。[`ClipboardRead::ALL`] の順。
const CLIPBOARD_READ_LABELS: [&str; ClipboardRead::ALL.len()] =
    ["確認する", "常に許可する", "常に拒否する"];

/// キーの割り当ての一覧の、組み合わせの列の位置（ダイアログの単位）。
const KEY_COLUMN_TAB_STOP: i32 = 110;

/// キーの割り当ての動作の表示。[`Action::ALL`] の順。
const ACTION_LABELS: [&str; Action::ALL.len()] = [
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
    let state = match create(
        owner,
        &current,
        fonts,
        family_indices,
        selected_font,
        tab_bar_colors,
        host,
    ) {
        Ok(state) => state,
        Err(e) => {
            tracing::warn!("failed to open the settings dialog: {e}");
            return;
        }
    };
    let hwnd = state.hwnd;
    // SAFETY: owner は呼び出し元の有効なウィンドウ。
    let dpi = unsafe { GetDpiForWindow(owner) };
    // fill が警告の文の高さをこの DPI のフォントで測れるように、先に決める。
    state.set_dpi(dpi);
    state.fill(&current);
    // SAFETY: hwnd は作ったばかりのウィンドウ。ポインタは modal::run が戻るまで有効で、WM_DESTROY で外す。
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, std::ptr::from_ref(&state) as isize) };
    if let Err(e) = place(owner, hwnd, dpi) {
        tracing::warn!("failed to place the settings dialog: {e}");
    }
    state.show_page(PAGE_APPEARANCE);
    // SAFETY: hwnd と一覧は有効なウィンドウ。
    unsafe {
        _ = ShowWindow(hwnd, SW_SHOW);
        _ = SetFocus(Some(state.controls.pages.hwnd()));
    }

    modal::run(owner, hwnd, &state.closed, |_| false);

    let font = state.font.get();
    if !font.is_invalid() {
        // SAFETY: フォントはダイアログのために作ったもので、ダイアログはもう閉じている。
        unsafe { _ = DeleteObject(HGDIOBJ(font.0)) };
    }
    state.colors.delete_swatch_brushes();
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

/// 一覧の項目の表示方法の選択肢。先頭は `[background]` の値に任せること、続いて [`BackgroundFit::ALL`]
/// の順。
const ITEM_FIT_CHOICES: [Option<BackgroundFit>; BackgroundFit::ALL.len() + 1] = {
    let mut choices = [None; BackgroundFit::ALL.len() + 1];
    let mut index = 0;
    while index < BackgroundFit::ALL.len() {
        choices[index + 1] = Some(BackgroundFit::ALL[index]);
        index += 1;
    }
    choices
};

/// 項目の不透明度の「既定」と「指定」。「指定」なら `true`。
const ITEM_OPACITY_CHOICES: [bool; 2] = [false, true];

/// `font.size_pt` の入力欄。
const FONT_SIZE_KIND: NumberKind<f32> = NumberKind {
    parse: |text| config::parse_input("font.size", text),
    format: format_size,
    step: step_size,
    start: |config| config.font.size_pt,
    message: "フォントサイズは、1 から 128 までの数値を入力してください。",
};

/// `scroll_bar.width` の入力欄。
const SCROLL_BAR_WIDTH_KIND: NumberKind<u32> = NumberKind {
    parse: |text| config::parse_input("scroll_bar.width", text),
    format: format_integer,
    step: |current, delta| step_integer(current, delta, &config::SCROLL_BAR_WIDTH_RANGE),
    start: |config| config.scroll_bar.width,
    message: "スクロールバーの幅は、4 から 64 までの整数を入力してください。",
};

/// `window.padding` の入力欄。
const PADDING_KIND: NumberKind<u32> = NumberKind {
    parse: |text| config::parse_input("window.padding", text),
    format: format_integer,
    step: |current, delta| step_integer(current, delta, &config::PADDING_RANGE),
    start: |config| config.window.padding,
    message: "余白は、0 から 64 までの整数を入力してください。",
};

/// `paste.confirm_size` の入力欄。
const PASTE_CONFIRM_SIZE_KIND: NumberKind<u32> = NumberKind {
    parse: |text| config::parse_input("paste.confirm_size", text),
    format: format_integer,
    step: |current, delta| step_integer(current, delta, &config::PASTE_CONFIRM_SIZE_RANGE),
    start: |config| config.paste.confirm_size,
    message: "大きなテキストの確認の大きさは、0 から 1048576 までの整数を入力してください。",
};

/// `background.image_opacity` の入力欄。
const BACKGROUND_OPACITY_KIND: NumberKind<f32> = NumberKind {
    parse: |text| config::parse_input("background.image_opacity", text),
    format: format_opacity,
    step: step_opacity,
    start: |config| config.background.image_opacity,
    message: "不透明度は、0 から 1 までの数値を入力してください。",
};

/// 背景画像の一覧で選んでいる項目の不透明度の入力欄。読めなければ、既定の不透明度から動かす
/// （背景のページの [`Page::step_start`] では、`background.image_opacity` を編集中の既定の不透明度に
/// 差し替えてある）。
const BACKGROUND_ITEM_OPACITY_KIND: NumberKind<f32> = NumberKind {
    parse: |text| config::parse_input("background.image_opacity", text),
    format: format_opacity,
    step: step_opacity,
    start: |config| config.background.image_opacity,
    message: "選択した項目の不透明度は、0 から 1 までの数値を入力してください。",
};

/// ダイアログ全体のコントロール。ページの中のコントロールは、それぞれのページが持つ。
struct Controls {
    pages: ListBox,
    error: HWND,
    /// すべてのコントロールと、96 DPI のときの矩形。上下の矢印のボタンは入れない（相手の入力欄に
    /// 付けて置く）。
    placed: Vec<(HWND, DipRect)>,
}

/// 設定ダイアログのページ。ページは自分のコントロールと編集中の値を持ち、ダイアログ全体の状態には
/// [`DialogContext`] を通して触る。
trait Page {
    /// ページに置くコントロール。ページを切り替えるときは、これをまとめて見せる・隠す。
    fn controls(&self) -> &[HWND];

    /// 数の入力欄。上下の矢印の配置と、矢印の知らせの振り分けは、すべてのページのこれを回す。
    fn number_fields(&self) -> Vec<&dyn NumberInput> {
        Vec::new()
    }

    /// 数の入力欄が読めないときに、上下の矢印で動かし始める値を取る設定。最後に書いたときの設定。
    fn step_start(&self, ctx: &dyn DialogContext) -> Config {
        ctx.base().clone()
    }

    /// コントロールに設定 `config` の値を入れる。
    fn fill(&self, ctx: &dyn DialogContext, config: &Config);

    /// コントロールの値を `config` に入れる。読めない入力があれば、その文と入力欄を返す。
    fn read(
        &self,
        ctx: &dyn DialogContext,
        config: &mut Config,
    ) -> Result<(), (&'static str, HWND)>;

    /// コントロール `id` から知らせ `code`（`WM_COMMAND`）が届いた。このページのコントロールなら処理して
    /// `true` を返す。
    fn on_command(&self, ctx: &dyn DialogContext, id: i32, code: u32) -> bool;
}

/// ページから触る、ダイアログ全体の状態と操作。
trait DialogContext {
    /// ダイアログの HWND。ページが開くダイアログやメッセージボックスの持ち主にする。
    fn hwnd(&self) -> HWND;
    /// 最後に設定ファイルに書いたとき（書いていなければ開いたとき）の設定。
    fn base(&self) -> Ref<'_, Config>;
    /// コントロールのフォント。
    fn font(&self) -> HFONT;
    /// ダイアログのいまの DPI。
    fn dpi(&self) -> u32;
    /// すべてのコントロールと、96 DPI のときの矩形。
    fn placed(&self) -> &[(HWND, DipRect)];
    /// 編集中の設定を読み、読めれば端末に反映する（前に反映したものと同じなら何もしない）。読めな
    /// ければエラーを見せる。
    fn preview(&self);
    /// 入力欄を打ったので、少し待ってから反映する。
    fn schedule_preview(&self);
}

/// ダイアログの状態。ウィンドウプロシージャからも触るので、共有の参照だけで読み書きする。
struct State<'a> {
    hwnd: HWND,
    controls: Controls,
    appearance: AppearancePage,
    background: BackgroundPage,
    behavior: BehaviorPage,
    profiles: ProfilesPage,
    keys: KeysPage,
    colors: ColorsPage,
    host: RefCell<&'a mut dyn Host>,
    /// 最後に設定ファイルに書いたとき（書いていなければ開いたとき）の設定。キャンセルでここへ戻す。
    base: RefCell<Config>,
    /// 最後に端末に反映した設定。
    previewed: RefCell<Config>,
    font: Cell<HFONT>,
    dpi: Cell<u32>,
    closed: Cell<bool>,
}

/// ダイアログとコントロールを作る。大きさと位置は後で決める（[`place`]）。`current` はいま使っている
/// 設定で、フォントの一覧 `fonts` の `selected_font` 番目がいま使っているフォント（`family_indices` は
/// `current.font.families` のそれぞれの、システムのフォントの中の番号）。
fn create<'a>(
    owner: HWND,
    current: &Config,
    fonts: Vec<FontChoice>,
    family_indices: Vec<Option<u32>>,
    selected_font: usize,
    tab_bar_colors: [Rgb; config::TAB_BAR_COLOR_KEYS.len()],
    host: &'a mut dyn Host,
) -> windows::core::Result<State<'a>> {
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
    let mut b = Builder::new(hwnd, instance.into());

    let pages = b.list(
        ID_PAGES,
        [MARGIN, MARGIN, PAGES_WIDTH, BUTTON_TOP - 2 * MARGIN],
        &[],
    )?;
    for page in PAGES {
        pages.add(page);
    }

    // Tab で移る順は作った順なので、ページの一覧の順に作る。
    let appearance = AppearancePage::create(&mut b, fonts, family_indices, selected_font)?;
    let background = BackgroundPage::create(&mut b)?;
    let behavior = BehaviorPage::create(&mut b)?;
    let profiles = ProfilesPage::create(&mut b)?;
    let keys = KeysPage::create(&mut b)?;
    let colors = ColorsPage::create(&mut b, tab_bar_colors)?;

    // 下のボタンと、入力のエラー
    let error = b.static_text(
        ID_ERROR,
        WINDOW_EX_STYLE(0),
        [
            MARGIN,
            BUTTON_TOP + LABEL_OFFSET,
            CLIENT_WIDTH - 2 * MARGIN - 3 * (BUTTON_WIDTH + BUTTON_GAP),
            ROW_HEIGHT,
        ],
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
        let rect = [button_left, BUTTON_TOP, BUTTON_WIDTH, BUTTON_HEIGHT];
        if default {
            b.default_button(id, text, rect)?;
        } else {
            b.button(id, text, rect)?;
        }
        button_left += BUTTON_WIDTH + BUTTON_GAP;
    }

    Ok(State {
        hwnd,
        controls: Controls {
            pages,
            error,
            placed: b.placed,
        },
        appearance,
        background,
        behavior,
        profiles,
        keys,
        colors,
        host: RefCell::new(host),
        base: RefCell::new(current.clone()),
        previewed: RefCell::new(current.clone()),
        font: Cell::new(HFONT::default()),
        dpi: Cell::new(BASE_DPI),
        closed: Cell::new(false),
    })
}

/// 外観のページ。フォント、サイズ、セルより広い文字、スクロールバーの幅、余白。
struct AppearancePage {
    controls: Vec<HWND>,
    font: HWND,
    size: NumberField<f32>,
    wide_glyph: RadioGroup<WideGlyph>,
    scroll_bar_width: NumberField<u32>,
    padding: NumberField<u32>,
    /// フォントの一覧。
    fonts: Vec<FontChoice>,
    /// 最後に書いたときの設定（[`DialogContext::base`]）の `font.families` のそれぞれの、システムの
    /// フォントの中の番号。
    base_indices: RefCell<Vec<Option<u32>>>,
    /// 最後に書いたときに選んでいたフォントの、一覧の中の位置。
    base_font: Cell<usize>,
}

impl AppearancePage {
    /// ページのコントロールを作る。フォントの一覧 `fonts` の `selected_font` 番目が、いま使っている
    /// フォント（`family_indices` は `font.families` のそれぞれの、システムのフォントの中の番号）。
    fn create(
        b: &mut Builder,
        fonts: Vec<FontChoice>,
        family_indices: Vec<Option<u32>>,
        selected_font: usize,
    ) -> windows::core::Result<Self> {
        let mut controls = Vec::new();
        controls.push(b.label("フォント(&F):", label_rect(row(0)))?);
        let font = b.combo(
            ID_FONT,
            [CONTROL_LEFT, row(0), CONTROL_WIDTH, ROW_HEIGHT],
            std::iter::empty::<&str>(),
        )?;
        controls.push(font);
        // SAFETY: font は有効なコンボボックス。
        unsafe { SendMessageW(font, CB_SETMINVISIBLE, Some(WPARAM(VISIBLE_FONTS)), None) };
        controls.push(b.label("サイズ(&S):", label_rect(row(1)))?);
        let size = NumberField::create(
            b,
            ID_SIZE,
            ID_SIZE_SPIN,
            [CONTROL_LEFT, row(1), NUMBER_WIDTH, ROW_HEIGHT],
            FONT_SIZE_KIND,
        )?;
        controls.extend(size.controls());
        controls.push(b.label(
            "ポイント（1〜128）",
            [
                CONTROL_LEFT + NUMBER_WIDTH + BUTTON_GAP,
                row(1) + LABEL_OFFSET,
                CONTROL_WIDTH - NUMBER_WIDTH - BUTTON_GAP,
                ROW_HEIGHT,
            ],
        )?);
        controls.push(b.label("セルより広い文字(&W):", label_rect(row(2)))?);
        let wide_glyph = RadioGroup::create(
            b,
            ID_WIDE_GLYPH,
            &WideGlyph::ALL,
            &WIDE_GLYPH_LABELS,
            |index| {
                [
                    CONTROL_LEFT,
                    row(2) + index * ROW_HEIGHT,
                    CONTROL_WIDTH,
                    ROW_HEIGHT,
                ]
            },
        )?;
        controls.extend_from_slice(wide_glyph.buttons());
        let width_row = row(2) + WideGlyph::ALL.len() as i32 * ROW_HEIGHT + MARGIN;
        controls.push(b.label("スクロールバーの幅(&B):", label_rect(width_row))?);
        let scroll_bar_width = NumberField::create(
            b,
            ID_SCROLL_BAR_WIDTH,
            ID_SCROLL_BAR_WIDTH_SPIN,
            [CONTROL_LEFT, width_row, NUMBER_WIDTH, ROW_HEIGHT],
            SCROLL_BAR_WIDTH_KIND,
        )?;
        controls.extend(scroll_bar_width.controls());
        controls.push(b.label(
            "ピクセル（4〜64。画面の拡大率に合わせて広げる）",
            [
                CONTROL_LEFT + NUMBER_WIDTH + BUTTON_GAP,
                width_row + LABEL_OFFSET,
                CONTROL_WIDTH - NUMBER_WIDTH - BUTTON_GAP,
                ROW_HEIGHT,
            ],
        )?);
        let padding_row = width_row + ROW_HEIGHT + MARGIN;
        controls.push(b.label("余白(&G):", label_rect(padding_row))?);
        let padding = NumberField::create(
            b,
            ID_PADDING,
            ID_PADDING_SPIN,
            [CONTROL_LEFT, padding_row, NUMBER_WIDTH, ROW_HEIGHT],
            PADDING_KIND,
        )?;
        controls.extend(padding.controls());
        controls.push(b.label(
            "ピクセル（0〜64。画面の拡大率に合わせて広げる）",
            [
                CONTROL_LEFT + NUMBER_WIDTH + BUTTON_GAP,
                padding_row + LABEL_OFFSET,
                CONTROL_WIDTH - NUMBER_WIDTH - BUTTON_GAP,
                ROW_HEIGHT,
            ],
        )?);
        Ok(Self {
            controls,
            font,
            size,
            wide_glyph,
            scroll_bar_width,
            padding,
            fonts,
            base_indices: RefCell::new(family_indices),
            base_font: Cell::new(selected_font),
        })
    }

    /// 設定ファイルに書けた。フォントを選び直していれば、選んでいるフォントを最後に書いたときの
    /// フォントにする。`base` は書く前の、最後に書いたときの設定。
    fn saved(&self, base: &Config) {
        // SAFETY: コンボボックスは有効なウィンドウ。
        let selected = unsafe { SendMessageW(self.font, CB_GETCURSEL, None, None) }.0;
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
    }
}

impl Page for AppearancePage {
    fn controls(&self) -> &[HWND] {
        &self.controls
    }

    fn number_fields(&self) -> Vec<&dyn NumberInput> {
        vec![
            &self.size as &dyn NumberInput,
            &self.scroll_bar_width,
            &self.padding,
        ]
    }

    /// コントロールに設定 `config` と、一覧の最後に書いたときのフォントを入れる。
    fn fill(&self, _ctx: &dyn DialogContext, config: &Config) {
        for font in &self.fonts {
            // SAFETY: コンボボックスは有効なウィンドウで、文字列はこの呼び出しの間は生きている。
            unsafe {
                SendMessageW(
                    self.font,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(font.name.as_str()).as_ptr() as isize)),
                )
            };
        }
        // SAFETY: コンボボックスは有効なウィンドウ。
        unsafe {
            SendMessageW(
                self.font,
                CB_SETCURSEL,
                Some(WPARAM(self.base_font.get())),
                None,
            )
        };
        self.size.fill(config.font.size_pt);
        self.wide_glyph.fill(config.font.wide_glyph);
        self.scroll_bar_width.fill(config.scroll_bar.width);
        self.padding.fill(config.window.padding);
    }

    fn read(
        &self,
        _ctx: &dyn DialogContext,
        config: &mut Config,
    ) -> Result<(), (&'static str, HWND)> {
        // SAFETY: コンボボックスは有効なウィンドウ。
        let selected = unsafe { SendMessageW(self.font, CB_GETCURSEL, None, None) }.0;
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
        config.font.size_pt = self.size.read()?;
        if let Some(wide_glyph) = self.wide_glyph.read() {
            config.font.wide_glyph = wide_glyph;
        }
        config.scroll_bar.width = self.scroll_bar_width.read()?;
        config.window.padding = self.padding.read()?;
        Ok(())
    }

    fn on_command(&self, ctx: &dyn DialogContext, id: i32, code: u32) -> bool {
        match (id, code) {
            (ID_FONT, CBN_SELCHANGE) => ctx.preview(),
            // ラジオボタンは、押したらすぐ反映する。
            (id, BN_CLICKED) if self.wide_glyph.owns(id) => ctx.preview(),
            // 数の入力欄は、打ち終わるのを待って反映する。
            (id, EN_CHANGE) if self.number_fields().iter().any(|field| field.owns(id)) => {
                ctx.schedule_preview();
            }
            _ => return false,
        }
        true
    }
}

/// 背景のページ。上に既定の値の枠、下に背景画像の一覧と選択した項目の値の枠。
struct BackgroundPage {
    controls: Vec<HWND>,
    /// 背景画像の一覧。
    background_images: ListBox,
    /// 背景画像の一覧の「削除」のボタン。
    background_remove: HWND,
    /// 既定の不透明度と表示方法。
    background_opacity: NumberField<f32>,
    background_fit: ChoiceBox<BackgroundFit>,
    /// 一覧で選んでいる項目の表示方法。
    background_item_fit: ChoiceBox<Option<BackgroundFit>>,
    /// 一覧で選んでいる項目の不透明度の「既定」と「指定」のラジオボタンと、「指定」の値。
    background_item_opacity_choice: RadioGroup<bool>,
    background_item_opacity: NumberField<f32>,
    /// 編集中の背景画像の一覧。
    images: RefCell<Vec<BackgroundImage>>,
}

impl BackgroundPage {
    /// ページのコントロールを作る。
    fn create(b: &mut Builder) -> windows::core::Result<Self> {
        let mut controls = Vec::new();
        let inner_left = PAGE_LEFT + GROUP_PADDING;
        let inner_width = PAGE_WIDTH - 2 * GROUP_PADDING;
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
        controls.push(b.group(
            "既定",
            [
                PAGE_LEFT,
                defaults_top,
                PAGE_WIDTH,
                defaults_bottom - defaults_top,
            ],
        )?);
        controls.push(b.label("表示方法(&T):", background_label_rect(fit_row))?);
        let background_fit = ChoiceBox::create(
            b,
            ID_BACKGROUND_FIT,
            [
                background_control_left,
                fit_row,
                background_control_width,
                ROW_HEIGHT,
            ],
            &BackgroundFit::ALL,
            BACKGROUND_FIT_LABELS,
        )?;
        controls.push(background_fit.hwnd());
        controls.push(b.label("不透明度(&Y):", background_label_rect(opacity_row))?);
        let background_opacity = NumberField::create(
            b,
            ID_BACKGROUND_OPACITY,
            ID_BACKGROUND_OPACITY_SPIN,
            [
                background_control_left,
                opacity_row,
                NUMBER_WIDTH,
                ROW_HEIGHT,
            ],
            BACKGROUND_OPACITY_KIND,
        )?;
        controls.extend(background_opacity.controls());
        controls.push(b.label(
            "0（透明）〜1（不透明）",
            opacity_note_rect(opacity_row, background_control_left),
        )?);

        let images_top = defaults_bottom + BUTTON_GAP;
        let list_top = images_top + GROUP_CAPTION_HEIGHT;
        let item_fit_row = list_top + BACKGROUND_LIST_HEIGHT + BUTTON_GAP;
        let item_opacity_row = item_fit_row + ROW_HEIGHT + BUTTON_GAP;
        let custom_opacity_row = item_opacity_row + ROW_HEIGHT;
        let images_bottom = custom_opacity_row + ROW_HEIGHT + GROUP_PADDING;
        controls.push(b.group(
            "背景画像の一覧(&I)",
            [
                PAGE_LEFT,
                images_top,
                PAGE_WIDTH,
                images_bottom - images_top,
            ],
        )?);
        let list_width = inner_width - BUTTON_GAP - BACKGROUND_BUTTON_WIDTH;
        let background_images = b.list(
            ID_BACKGROUND_IMAGES,
            [inner_left, list_top, list_width, BACKGROUND_LIST_HEIGHT],
            &[],
        )?;
        controls.push(background_images.hwnd());
        let image_button_rect = |index: i32| {
            [
                inner_left + list_width + BUTTON_GAP,
                list_top + index * (BUTTON_HEIGHT + BUTTON_GAP),
                BACKGROUND_BUTTON_WIDTH,
                BUTTON_HEIGHT,
            ]
        };
        controls.push(b.button(
            ID_BACKGROUND_ADD_FILES,
            "ファイルを追加(&L)…",
            image_button_rect(0),
        )?);
        controls.push(b.button(
            ID_BACKGROUND_ADD_FOLDER,
            "フォルダを追加(&O)…",
            image_button_rect(1),
        )?);
        let background_remove = b.button(ID_BACKGROUND_REMOVE, "削除(&D)", image_button_rect(2))?;
        controls.push(background_remove);
        controls.push(b.label(
            "選択した項目の表示方法(&N):",
            background_label_rect(item_fit_row),
        )?);
        // 先頭の「既定（…）」は、既定の表示方法を入れるときに書き直す。
        let background_item_fit = ChoiceBox::create(
            b,
            ID_BACKGROUND_ITEM_FIT,
            [
                background_control_left,
                item_fit_row,
                background_control_width,
                ROW_HEIGHT,
            ],
            &ITEM_FIT_CHOICES,
            std::iter::once("").chain(BACKGROUND_FIT_LABELS),
        )?;
        controls.push(background_item_fit.hwnd());
        controls.push(b.label(
            "選択した項目の不透明度(&H):",
            background_label_rect(item_opacity_row),
        )?);
        // 「既定」の文字は、既定の不透明度を入れるときに書き直す。「指定」の右に入力欄を置く。
        let background_item_opacity_choice = RadioGroup::create(
            b,
            ID_BACKGROUND_ITEM_OPACITY_CHOICE,
            &ITEM_OPACITY_CHOICES,
            &["", "指定"],
            |index| {
                if index == 0 {
                    [
                        background_control_left,
                        item_opacity_row,
                        background_control_width,
                        ROW_HEIGHT,
                    ]
                } else {
                    [
                        background_control_left,
                        custom_opacity_row,
                        CUSTOM_OPACITY_WIDTH,
                        ROW_HEIGHT,
                    ]
                }
            },
        )?;
        controls.extend_from_slice(background_item_opacity_choice.buttons());
        let custom_opacity_left = background_control_left + CUSTOM_OPACITY_WIDTH;
        let background_item_opacity = NumberField::create(
            b,
            ID_BACKGROUND_ITEM_OPACITY,
            ID_BACKGROUND_ITEM_OPACITY_SPIN,
            [
                custom_opacity_left,
                custom_opacity_row,
                NUMBER_WIDTH,
                ROW_HEIGHT,
            ],
            BACKGROUND_ITEM_OPACITY_KIND,
        )?;
        controls.extend(background_item_opacity.controls());
        controls.push(b.label(
            "0〜1",
            opacity_note_rect(custom_opacity_row, custom_opacity_left),
        )?);
        controls.push(b.label(
            "新しいタブを開くたびに、一覧からランダムに 1 枚選ぶ（フォルダはその中の画像から選ぶ）。画像は、不透明度に応じて端末の背景色を重ねて薄くする。",
            [
                PAGE_LEFT,
                images_bottom + BUTTON_GAP,
                PAGE_WIDTH,
                2 * ROW_HEIGHT,
            ],
        )?);
        Ok(Self {
            controls,
            background_images,
            background_remove,
            background_opacity,
            background_fit,
            background_item_fit,
            background_item_opacity_choice,
            background_item_opacity,
            images: RefCell::new(Vec::new()),
        })
    }

    /// 背景画像の一覧を作り直し、`selected` 番目（一覧より後ろなら最後）を選ぶ。`None` なら選ばない。
    fn refresh_background_images(&self, selected: Option<usize>) {
        let list = self.background_images;
        let images = self.images.borrow();
        list.reset_with(images.iter().map(background_row_text));
        if let Some(selected) = selected
            && !images.is_empty()
        {
            list.select(selected.min(images.len() - 1));
        }
        drop(images);
        self.update_background_buttons();
    }

    /// 背景画像の一覧で選んでいる位置。
    fn selected_background_image(&self) -> Option<usize> {
        self.background_images
            .selected()
            .filter(|&index| index < self.images.borrow().len())
    }

    /// 背景画像の一覧で選んでいるときだけ「削除」と項目の表示方法・不透明度を触れるようにし、表示方法と
    /// 不透明度に選んでいる項目の値を見せる。不透明度の入力欄は「指定」のときだけ触れる。
    fn update_background_buttons(&self) {
        let selected = self
            .selected_background_image()
            .and_then(|index| self.images.borrow().get(index).cloned());
        let buttons = self.background_item_opacity_choice.buttons();
        for &control in [self.background_remove, self.background_item_fit.hwnd()]
            .iter()
            .chain(buttons)
        {
            // SAFETY: control はダイアログの子の有効なウィンドウ。
            unsafe { _ = EnableWindow(control, selected.is_some()) };
        }
        match &selected {
            Some(image) => self.background_item_fit.fill(image.fit),
            None => self.background_item_fit.clear(),
        }
        let opacity = selected.as_ref().map(|image| image.image_opacity);
        let custom = opacity.flatten();
        match opacity {
            Some(opacity) => self.background_item_opacity_choice.fill(opacity.is_some()),
            None => self.background_item_opacity_choice.clear(),
        }
        self.enable_item_opacity(custom.is_some());
        // 値が変わったときだけ書く（書くと EN_CHANGE が届いて、端末への反映の予約が掛かる）。借用は
        // 持っていない。
        self.background_item_opacity.fill_if_changed(custom);
    }

    /// 項目の不透明度の入力欄と上下の矢印を、`enabled` なら触れるようにする。
    fn enable_item_opacity(&self, enabled: bool) {
        for control in self.background_item_opacity.controls() {
            // SAFETY: control はダイアログの子の有効なウィンドウ。
            unsafe { _ = EnableWindow(control, enabled) };
        }
    }

    /// 項目の不透明度で「指定」を選んでいるか。
    fn custom_item_opacity(&self) -> bool {
        self.background_item_opacity_choice.read() == Some(true)
    }

    /// 編集中の既定の表示方法。
    fn default_fit(&self, ctx: &dyn DialogContext) -> BackgroundFit {
        self.background_fit
            .read()
            .unwrap_or(ctx.base().background.fit)
    }

    /// 編集中の既定の不透明度。読めなければ `None`。
    fn default_opacity(&self) -> Option<f32> {
        self.background_opacity.read().ok()
    }

    /// 項目の表示方法の先頭の選択肢と、不透明度の「既定」のラジオボタンに、編集中の既定の値を添える。
    /// 既定の不透明度が読めなければ、ラジオボタンは前の添え書きのままにする。
    fn refresh_background_defaults(&self, ctx: &dyn DialogContext) {
        let fit = default_choice_label(fit_label(self.default_fit(ctx)));
        self.background_item_fit.relabel(None, &fit);
        if let Some(opacity) = self.default_opacity()
            && let Some(button) = self.background_item_opacity_choice.button(false)
        {
            set_text(button, &default_choice_label(&format_opacity(opacity)));
        }
    }

    /// 背景画像の一覧の `index` 番目の行を、編集中の項目に合わせて書き直す（選んだままにする）。
    fn refresh_background_row(&self, index: usize) {
        let Some(text) = self.images.borrow().get(index).map(background_row_text) else {
            return;
        };
        self.background_images.replace_row(index, &text);
    }

    /// 一覧で選んでいる項目の表示方法のコンボボックスが変わった。
    fn on_background_item_fit(&self, ctx: &dyn DialogContext) {
        let Some(index) = self.selected_background_image() else {
            return;
        };
        let Some(fit) = self.background_item_fit.read() else {
            return;
        };
        self.images.borrow_mut()[index].fit = fit;
        self.refresh_background_row(index);
        ctx.preview();
    }

    /// 一覧で選んでいる項目の不透明度の入力欄が変わった。「指定」で読めれば項目に入れる。端末への反映は
    /// 少し待つ（読めなければ、反映するときにエラーを見せる）。
    fn on_background_item_opacity(&self, ctx: &dyn DialogContext) {
        if let Some(index) = self.selected_background_image()
            && self.custom_item_opacity()
            && let Ok(opacity) = self.background_item_opacity.read()
        {
            self.set_item_opacity(index, Some(opacity));
        }
        ctx.schedule_preview();
    }

    /// 一覧で選んでいる項目の不透明度の「既定」か「指定」が選ばれた。「指定」で入力欄が読めなければ、
    /// 既定の不透明度を入れる。
    fn on_background_item_opacity_choice(&self, ctx: &dyn DialogContext) {
        let Some(index) = self.selected_background_image() else {
            return;
        };
        let custom = self.custom_item_opacity();
        self.enable_item_opacity(custom);
        if custom {
            let opacity = self
                .background_item_opacity
                .read()
                .ok()
                .or_else(|| self.default_opacity())
                .unwrap_or(ctx.base().background.image_opacity);
            // 借用は持っていない（入力欄の EN_CHANGE はこの中で届く）。
            self.background_item_opacity.fill(opacity);
            self.set_item_opacity(index, Some(opacity));
        } else {
            self.background_item_opacity.clear();
            self.set_item_opacity(index, None);
        }
        ctx.preview();
    }

    /// 一覧の `index` 番目の項目の不透明度を `opacity` にし、変わったら一覧の行を書き直す。
    fn set_item_opacity(&self, index: usize, opacity: Option<f32>) {
        let changed = {
            let mut images = self.images.borrow_mut();
            let changed = images[index].image_opacity != opacity;
            images[index].image_opacity = opacity;
            changed
        };
        if changed {
            self.refresh_background_row(index);
        }
    }

    /// 背景画像の一覧のボタン `id` が押された。
    fn on_background_button(&self, ctx: &dyn DialogContext, id: i32) {
        let selected = match id {
            ID_BACKGROUND_ADD_FILES => {
                let filters = image_filters();
                let chosen = match file_dialog::open_files(ctx.hwnd(), "背景画像を追加", &filters)
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
                let chosen = match file_dialog::open_folder(ctx.hwnd(), "背景画像のフォルダを追加")
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
                self.images.borrow_mut().remove(index);
                Some(index)
            }
            _ => return,
        };
        let Some(selected) = selected else {
            return;
        };
        self.refresh_background_images(Some(selected));
        ctx.preview();
        // SAFETY: 一覧はダイアログの子の有効なウィンドウ。押したボタンが押せなくなることがあるので、
        // 一覧へ戻す。
        unsafe { _ = SetFocus(Some(self.background_images.hwnd())) };
    }

    /// 背景画像の一覧の最後に `paths` を足す（同じものがあれば足さない）。足した最後の位置を返し、
    /// 何も足さなければ `None`。
    fn add_background_images(&self, paths: Vec<PathBuf>) -> Option<usize> {
        let mut images = self.images.borrow_mut();
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
}

impl Page for BackgroundPage {
    fn controls(&self) -> &[HWND] {
        &self.controls
    }

    fn number_fields(&self) -> Vec<&dyn NumberInput> {
        vec![&self.background_opacity, &self.background_item_opacity]
    }

    /// 既定の不透明度は、入力欄が読めればその値にする（項目の不透明度が読めないときは、既定の
    /// 不透明度から動かす）。
    fn step_start(&self, ctx: &dyn DialogContext) -> Config {
        let mut start = ctx.base().clone();
        if let Some(opacity) = self.default_opacity() {
            start.background.image_opacity = opacity;
        }
        start
    }

    fn fill(&self, ctx: &dyn DialogContext, config: &Config) {
        self.background_opacity
            .fill(config.background.image_opacity);
        self.background_fit.fill(config.background.fit);
        self.refresh_background_defaults(ctx);
        *self.images.borrow_mut() = config.background.images.clone();
        self.refresh_background_images(None);
    }

    fn read(
        &self,
        _ctx: &dyn DialogContext,
        config: &mut Config,
    ) -> Result<(), (&'static str, HWND)> {
        // 選んでいる項目の値は、読めたときに項目へ入れている。ここでは読めるかだけを見る。
        if self.selected_background_image().is_some() && self.custom_item_opacity() {
            self.background_item_opacity.read()?;
        }
        config.background.images = self.images.borrow().clone();
        config.background.image_opacity = self.background_opacity.read()?;
        if let Some(fit) = self.background_fit.read() {
            config.background.fit = fit;
        }
        Ok(())
    }

    fn on_command(&self, ctx: &dyn DialogContext, id: i32, code: u32) -> bool {
        match (id, code) {
            (ID_BACKGROUND_IMAGES, LBN_SELCHANGE) => self.update_background_buttons(),
            (
                ID_BACKGROUND_ADD_FILES | ID_BACKGROUND_ADD_FOLDER | ID_BACKGROUND_REMOVE,
                BN_CLICKED,
            ) => self.on_background_button(ctx, id),
            (ID_BACKGROUND_FIT, CBN_SELCHANGE) => {
                self.refresh_background_defaults(ctx);
                ctx.preview();
            }
            (ID_BACKGROUND_OPACITY, EN_CHANGE) => {
                self.refresh_background_defaults(ctx);
                ctx.schedule_preview();
            }
            (ID_BACKGROUND_ITEM_FIT, CBN_SELCHANGE) => self.on_background_item_fit(ctx),
            (ID_BACKGROUND_ITEM_OPACITY, EN_CHANGE) => self.on_background_item_opacity(ctx),
            (id, BN_CLICKED) if self.background_item_opacity_choice.owns(id) => {
                self.on_background_item_opacity_choice(ctx);
            }
            _ => return false,
        }
        true
    }
}

/// 動作のページ。貼り付け前の確認、大きなテキストの確認、クリップボードの読み取り。
struct BehaviorPage {
    controls: Vec<HWND>,
    paste_confirm: RadioGroup<PasteConfirm>,
    paste_confirm_size: NumberField<u32>,
    clipboard_read: RadioGroup<ClipboardRead>,
}

impl BehaviorPage {
    /// ページのコントロールを作る。
    fn create(b: &mut Builder) -> windows::core::Result<Self> {
        let mut controls = Vec::new();
        controls.push(b.label("貼り付け前の確認(&P):", label_rect(row(0)))?);
        let paste_confirm = RadioGroup::create(
            b,
            ID_PASTE_CONFIRM,
            &PasteConfirm::ALL,
            &PASTE_CONFIRM_LABELS,
            |index| {
                [
                    CONTROL_LEFT,
                    row(0) + index * ROW_HEIGHT,
                    CONTROL_WIDTH,
                    ROW_HEIGHT,
                ]
            },
        )?;
        controls.extend_from_slice(paste_confirm.buttons());
        controls.push(b.label(
            "改行を含む文字列を貼り付ける前に、内容を編集できるダイアログで確認する。「コマンドとして実行される可能性があるとき」は、アプリが bracketed paste を有効にしていないときなど。",
            [
                PAGE_LEFT,
                row(0) + 3 * ROW_HEIGHT + MARGIN,
                PAGE_WIDTH,
                3 * ROW_HEIGHT,
            ],
        )?);
        let paste_size_row = row(0) + 6 * ROW_HEIGHT + 2 * MARGIN;
        controls.push(b.label("大きなテキストの確認(&Z):", label_rect(paste_size_row))?);
        let paste_confirm_size = NumberField::create(
            b,
            ID_PASTE_CONFIRM_SIZE,
            ID_PASTE_CONFIRM_SIZE_SPIN,
            [CONTROL_LEFT, paste_size_row, NUMBER_WIDTH, ROW_HEIGHT],
            PASTE_CONFIRM_SIZE_KIND,
        )?;
        controls.extend(paste_confirm_size.controls());
        controls.push(b.label(
            "KiB 以上は常に確認する（0 で無効）",
            [
                CONTROL_LEFT + NUMBER_WIDTH + BUTTON_GAP,
                paste_size_row + LABEL_OFFSET,
                CONTROL_WIDTH - NUMBER_WIDTH - BUTTON_GAP,
                ROW_HEIGHT,
            ],
        )?);
        let clipboard_row = paste_size_row + ROW_HEIGHT + MARGIN;
        controls.push(b.label("クリップボードの読み取り(&L):", label_rect(clipboard_row))?);
        let clipboard_read = RadioGroup::create(
            b,
            ID_CLIPBOARD_READ,
            &ClipboardRead::ALL,
            &CLIPBOARD_READ_LABELS,
            |index| {
                [
                    CONTROL_LEFT,
                    clipboard_row + index * ROW_HEIGHT,
                    CONTROL_WIDTH,
                    ROW_HEIGHT,
                ]
            },
        )?;
        controls.extend_from_slice(clipboard_read.buttons());
        controls.push(b.label(
            "アプリがクリップボードの内容を読み取ろうとしたとき（OSC 52）。アプリからの書き込みは、この設定に関係なく受け付ける。",
            [
                PAGE_LEFT,
                clipboard_row + 3 * ROW_HEIGHT + MARGIN,
                PAGE_WIDTH,
                2 * ROW_HEIGHT,
            ],
        )?);
        Ok(Self {
            controls,
            paste_confirm,
            paste_confirm_size,
            clipboard_read,
        })
    }
}

impl Page for BehaviorPage {
    fn controls(&self) -> &[HWND] {
        &self.controls
    }

    fn number_fields(&self) -> Vec<&dyn NumberInput> {
        vec![&self.paste_confirm_size]
    }

    fn fill(&self, _ctx: &dyn DialogContext, config: &Config) {
        self.paste_confirm.fill(config.paste.confirm);
        self.paste_confirm_size.fill(config.paste.confirm_size);
        self.clipboard_read.fill(config.clipboard.read);
    }

    fn read(
        &self,
        _ctx: &dyn DialogContext,
        config: &mut Config,
    ) -> Result<(), (&'static str, HWND)> {
        if let Some(confirm) = self.paste_confirm.read() {
            config.paste.confirm = confirm;
        }
        config.paste.confirm_size = self.paste_confirm_size.read()?;
        if let Some(read) = self.clipboard_read.read() {
            config.clipboard.read = read;
        }
        Ok(())
    }

    fn on_command(&self, ctx: &dyn DialogContext, id: i32, code: u32) -> bool {
        match (id, code) {
            // ラジオボタンは、押したらすぐ反映する。
            (id, BN_CLICKED) if self.paste_confirm.owns(id) || self.clipboard_read.owns(id) => {
                ctx.preview();
            }
            // 数の入力欄は、打ち終わるのを待って反映する。
            (id, EN_CHANGE) if self.number_fields().iter().any(|field| field.owns(id)) => {
                ctx.schedule_preview();
            }
            _ => return false,
        }
        true
    }
}

/// プロファイルのページ。プロファイルの一覧と、一覧で選んでいるプロファイルの値。
struct ProfilesPage {
    controls: Vec<HWND>,
    /// プロファイルの一覧。
    profiles_list: ListBox,
    /// 削除・既定にするのボタン。
    profile_remove: HWND,
    profile_default: HWND,
    /// 一覧で選んでいるプロファイルの名前とコマンドライン。
    profile_name: HWND,
    profile_command: HWND,
    /// コマンドラインの入力欄の下の警告の文と、文があるとその高さの分だけ下がるコントロール。
    profile_command_warning: HWND,
    profile_below_command_warning: [HWND; 4],
    /// 一覧で選んでいるプロファイルの接続方式と、Cygwin のフォルダか
    /// ディストリビューションの入力欄とその見出し。
    profile_backend: ChoiceBox<BackendKind>,
    profile_location_label: HWND,
    profile_location: HWND,
    /// プロファイルのアイコンの画像のファイルの入力欄と、選ぶボタン。
    profile_icon: HWND,
    profile_icon_choose: HWND,
    /// 編集中のプロファイル。
    profiles: RefCell<Vec<Profile>>,
    /// 編集中の既定のプロファイルの、`profiles` の中の位置。
    default_profile: Cell<usize>,
    /// プロファイルの名前とコマンドラインの入力欄に、選んだプロファイルの値を入れている（その間に届く
    /// EN_CHANGE を編集とみなさない）。
    filling_profile: Cell<bool>,
}

impl ProfilesPage {
    /// ページのコントロールを作る。
    fn create(b: &mut Builder) -> windows::core::Result<Self> {
        let mut controls = Vec::new();
        controls.push(b.label(
            "新しいタブで起動するプロファイル(&R):",
            [PAGE_LEFT, row(0) + LABEL_OFFSET, PAGE_WIDTH, ROW_HEIGHT],
        )?);
        let profiles_top = row(0) + ROW_HEIGHT;
        let profiles_list = b.list(
            ID_PROFILES,
            [PAGE_LEFT, profiles_top, PAGE_WIDTH, PROFILE_LIST_HEIGHT],
            &[PROFILE_COLUMN_TAB_STOP],
        )?;
        controls.push(profiles_list.hwnd());
        let profile_buttons_top = profiles_top + PROFILE_LIST_HEIGHT + BUTTON_GAP;
        let profile_button_width = (PAGE_WIDTH - 2 * BUTTON_GAP) / 3;
        let profile_button_rect = |index: i32| {
            [
                PAGE_LEFT + index * (profile_button_width + BUTTON_GAP),
                profile_buttons_top,
                profile_button_width,
                BUTTON_HEIGHT,
            ]
        };
        controls.push(b.button(ID_PROFILE_ADD, "追加(&D)", profile_button_rect(0))?);
        let profile_remove = b.button(ID_PROFILE_REMOVE, "削除(&L)", profile_button_rect(1))?;
        controls.push(profile_remove);
        let profile_default =
            b.button(ID_PROFILE_DEFAULT, "既定にする(&J)", profile_button_rect(2))?;
        controls.push(profile_default);
        let profile_name_row = profile_buttons_top + BUTTON_HEIGHT + MARGIN;
        controls.push(b.label("名前(&N):", label_rect(profile_name_row))?);
        let profile_name = b.edit(
            ID_PROFILE_NAME,
            [CONTROL_LEFT, profile_name_row, CONTROL_WIDTH, ROW_HEIGHT],
        )?;
        controls.push(profile_name);
        let profile_backend_row = profile_name_row + ROW_HEIGHT + MARGIN;
        controls.push(b.label("接続方式(&B):", label_rect(profile_backend_row))?);
        let profile_backend = ChoiceBox::create(
            b,
            ID_PROFILE_BACKEND,
            [CONTROL_LEFT, profile_backend_row, CONTROL_WIDTH, ROW_HEIGHT],
            &BackendKind::ALL,
            PROFILE_BACKEND_LABELS,
        )?;
        controls.push(profile_backend.hwnd());
        let profile_location_row = profile_backend_row + ROW_HEIGHT + MARGIN;
        let profile_location_label = b.label(
            profile_location_texts(BackendKind::Conpty).0,
            label_rect(profile_location_row),
        )?;
        controls.push(profile_location_label);
        let profile_location = b.edit(
            ID_PROFILE_LOCATION,
            [
                CONTROL_LEFT,
                profile_location_row,
                CONTROL_WIDTH,
                ROW_HEIGHT,
            ],
        )?;
        controls.push(profile_location);
        let profile_command_row = profile_location_row + ROW_HEIGHT + MARGIN;
        controls.push(b.label("コマンドライン(&C):", label_rect(profile_command_row))?);
        let profile_command = b.edit(
            ID_PROFILE_COMMAND,
            [CONTROL_LEFT, profile_command_row, CONTROL_WIDTH, ROW_HEIGHT],
        )?;
        controls.push(profile_command);
        // 高さは文に合わせて決める（文がなければ 0）。
        let profile_command_warning = b.static_text(
            ID_PROFILE_COMMAND_WARNING,
            WINDOW_EX_STYLE(0),
            [
                CONTROL_LEFT,
                profile_command_row + ROW_HEIGHT + LABEL_OFFSET,
                CONTROL_WIDTH,
                0,
            ],
        )?;
        controls.push(profile_command_warning);
        let profile_icon_row = profile_command_row + ROW_HEIGHT + MARGIN;
        let profile_icon_label = b.label("アイコン(&I):", label_rect(profile_icon_row))?;
        controls.push(profile_icon_label);
        let profile_icon = b.edit(
            ID_PROFILE_ICON,
            [
                CONTROL_LEFT,
                profile_icon_row,
                CONTROL_WIDTH - BUTTON_GAP - BUTTON_WIDTH,
                ROW_HEIGHT,
            ],
        )?;
        controls.push(profile_icon);
        let profile_icon_cue = HSTRING::from("空欄なら自動で作成する");
        // SAFETY: profile_icon は有効な入力欄で、文字列はこの呼び出しの間は生きている。
        unsafe {
            SendMessageW(
                profile_icon,
                EM_SETCUEBANNER,
                Some(WPARAM(CUE_BANNER_ALWAYS)),
                Some(LPARAM(profile_icon_cue.as_ptr() as isize)),
            )
        };
        let profile_icon_choose = b.button(
            ID_PROFILE_ICON_CHOOSE,
            "参照(&E)…",
            [
                CONTROL_LEFT + CONTROL_WIDTH - BUTTON_WIDTH,
                profile_icon_row,
                BUTTON_WIDTH,
                ROW_HEIGHT,
            ],
        )?;
        controls.push(profile_icon_choose);
        let profile_note = b.label(
            "タブバーの「+」は既定のプロファイルを開き、「▾」か「+」の右クリックで一覧から選んで開く。fantermo を引数つきで起動したときは、「+」とキーとメニューの「新しいタブ」も引数のコマンドラインを使う。Cygwin と WSL のコマンドラインは Cygwin・WSL の中で実行するもの（空欄ならログインシェル）。変更は次に開くタブから反映される。",
            [
                PAGE_LEFT,
                profile_icon_row + ROW_HEIGHT + MARGIN,
                PAGE_WIDTH,
                PROFILE_NOTE_HEIGHT,
            ],
        )?;
        controls.push(profile_note);
        Ok(Self {
            controls,
            profiles_list,
            profile_remove,
            profile_default,
            profile_name,
            profile_command,
            profile_command_warning,
            profile_below_command_warning: [
                profile_icon_label,
                profile_icon,
                profile_icon_choose,
                profile_note,
            ],
            profile_backend,
            profile_location_label,
            profile_location,
            profile_icon,
            profile_icon_choose,
            profiles: RefCell::new(Vec::new()),
            default_profile: Cell::new(0),
            filling_profile: Cell::new(false),
        })
    }

    /// プロファイルの一覧を作り直し、`selected` 番目（一覧より後ろなら最後）を選んで、名前と
    /// コマンドラインの入力欄にその値を入れる。
    fn refresh_profiles(&self, ctx: &dyn DialogContext, selected: usize) {
        let list = self.profiles_list;
        let profiles = self.profiles.borrow();
        list.reset_with(profiles.iter().enumerate().map(|(index, profile)| {
            profile_row_text(profile, index == self.default_profile.get())
        }));
        let index = selected.min(profiles.len().saturating_sub(1));
        drop(profiles);
        list.select(index);
        self.on_profile_selected(ctx);
    }

    /// プロファイルの一覧で選んでいる位置。
    fn selected_profile(&self) -> Option<usize> {
        self.profiles_list
            .selected()
            .filter(|&index| index < self.profiles.borrow().len())
    }

    /// プロファイルの一覧で選んだものが変わった。名前とコマンドラインの入力欄に値を入れ、ボタンを
    /// 押せるかを決める。
    fn on_profile_selected(&self, ctx: &dyn DialogContext) {
        let selected = self.selected_profile();
        let profile = selected.and_then(|index| self.profiles.borrow().get(index).cloned());
        self.filling_profile.set(true);
        set_text(
            self.profile_name,
            profile.as_ref().map_or("", |profile| &profile.name),
        );
        set_text(
            self.profile_command,
            profile.as_ref().map_or("", |profile| &profile.command),
        );
        let backend = profile
            .as_ref()
            .map_or_else(BackendKind::default, |p| p.backend);
        self.profile_backend.fill(backend);
        let location = profile.as_ref().and_then(|profile| {
            profile
                .cygwin_root
                .as_deref()
                .or(profile.distribution.as_deref())
        });
        set_text(self.profile_location, location.unwrap_or(""));
        set_text(
            self.profile_icon,
            profile
                .as_ref()
                .and_then(|profile| profile.icon.as_deref())
                .unwrap_or(""),
        );
        self.filling_profile.set(false);
        self.show_profile_backend(backend);
        self.show_profile_command_warning(ctx);
        let count = self.profiles.borrow().len();
        for (control, enabled) in [
            (self.profile_remove, selected.is_some() && count > 1),
            (
                self.profile_default,
                selected.is_some_and(|index| index != self.default_profile.get()),
            ),
            (self.profile_name, selected.is_some()),
            (self.profile_command, selected.is_some()),
            (self.profile_backend.hwnd(), selected.is_some()),
            (
                self.profile_location,
                selected.is_some() && backend.is_bridge(),
            ),
            (self.profile_icon, selected.is_some()),
            (self.profile_icon_choose, selected.is_some()),
        ] {
            // SAFETY: control はダイアログの子の有効なウィンドウ。
            unsafe { _ = EnableWindow(control, enabled) };
        }
    }

    /// 場所の入力欄の見出しと、コマンドラインと場所の入力欄が空のときに薄く見せる文字を、接続方式
    /// `backend` に合わせる。
    fn show_profile_backend(&self, backend: BackendKind) {
        let (label, cue) = profile_location_texts(backend);
        set_text(self.profile_location_label, label);
        let command_cue = if backend.is_bridge() {
            "空欄ならログインシェル"
        } else {
            ""
        };
        for (control, text) in [
            (self.profile_location, cue),
            (self.profile_command, command_cue),
        ] {
            let text = HSTRING::from(text);
            // SAFETY: control は有効な入力欄で、文字列はこの呼び出しの間は生きている。
            unsafe {
                SendMessageW(
                    control,
                    EM_SETCUEBANNER,
                    Some(WPARAM(CUE_BANNER_ALWAYS)),
                    Some(LPARAM(text.as_ptr() as isize)),
                )
            };
        }
    }

    /// コマンドラインの入力欄の下の警告の文を、選んでいるプロファイルに合わせる。文があれば、その高さの
    /// 分だけ下の行をずらす。
    fn show_profile_command_warning(&self, ctx: &dyn DialogContext) {
        let warning = self.selected_profile().and_then(|index| {
            self.profiles
                .borrow()
                .get(index)
                .and_then(profile_command_warning)
        });
        let dpi = ctx.dpi();
        let placed = ctx.placed();
        set_text(self.profile_command_warning, warning.unwrap_or(""));
        let Some(&(_, [left, top, width, _])) = placed
            .iter()
            .find(|(control, _)| *control == self.profile_command_warning)
        else {
            return;
        };
        let width = scale(width, dpi);
        let height = warning.map_or(0, |text| {
            wrapped_text_height(self.profile_command_warning, ctx.font(), text, width)
        });
        // SAFETY: 警告の文はダイアログの子の有効なウィンドウ。
        unsafe {
            _ = MoveWindow(
                self.profile_command_warning,
                scale(left, dpi),
                scale(top, dpi),
                width,
                height,
                true,
            );
        }
        let shift = if height > 0 {
            height + scale(LABEL_OFFSET, dpi)
        } else {
            0
        };
        for &(control, [left, top, _, _]) in placed {
            if !self.profile_below_command_warning.contains(&control) {
                continue;
            }
            // SAFETY: control はダイアログの子の有効なウィンドウ。
            unsafe {
                _ = SetWindowPos(
                    control,
                    None,
                    scale(left, dpi),
                    scale(top, dpi) + shift,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }

    /// 選んでいるプロファイルの接続方式が変わった。場所の入力欄を空にして見出しを替え、プロファイルに
    /// 入れる。
    fn on_profile_backend_changed(&self, ctx: &dyn DialogContext) {
        let Some(index) = self.selected_profile() else {
            return;
        };
        let Some(backend) = self.profile_backend.read() else {
            return;
        };
        self.profiles.borrow_mut()[index].backend = backend;
        self.filling_profile.set(true);
        set_text(self.profile_location, "");
        self.filling_profile.set(false);
        self.show_profile_backend(backend);
        // SAFETY: 入力欄は有効なウィンドウ。
        unsafe { _ = EnableWindow(self.profile_location, backend.is_bridge()) };
        self.on_profile_edited(ctx);
    }

    /// アイコンの「参照…」が押された。選んだファイルの絶対パスを入力欄に入れる（入力欄の変更の知らせで
    /// プロファイルに入る）。
    fn on_profile_icon_choose(&self, ctx: &dyn DialogContext) {
        let chosen = match file_dialog::open_file(ctx.hwnd(), "アイコンの選択", &icon_filters())
        {
            Ok(chosen) => chosen,
            Err(e) => {
                tracing::warn!("failed to choose a profile icon: {e}");
                return;
            }
        };
        if let Some(path) = chosen {
            set_text(self.profile_icon, &path.to_string_lossy());
        }
    }

    /// プロファイルの名前かコマンドラインの入力欄が変わった。選んでいるプロファイルに入れ、一覧の行を
    /// 書き直す。端末への反映は少し待つ（次に開くタブから使うので、反映しても見た目は変わらない）。
    fn on_profile_edited(&self, ctx: &dyn DialogContext) {
        if self.filling_profile.get() {
            return;
        }
        let Some(index) = self.selected_profile() else {
            return;
        };
        let name = get_text(self.profile_name).trim().to_owned();
        let command = get_text(self.profile_command).trim().to_owned();
        let location = get_text(self.profile_location).trim().to_owned();
        let icon = get_text(self.profile_icon).trim().to_owned();
        let text = {
            let mut profiles = self.profiles.borrow_mut();
            let profile = &mut profiles[index];
            profile.name = name;
            profile.command = command;
            set_profile_location(profile, location);
            profile.icon = (!icon.is_empty()).then_some(icon);
            profile_row_text(profile, index == self.default_profile.get())
        };
        self.profiles_list.replace_row(index, &text);
        self.show_profile_command_warning(ctx);
        ctx.schedule_preview();
    }

    /// プロファイルのボタン `id` が押された。
    fn on_profile_button(&self, ctx: &dyn DialogContext, id: i32) {
        let selected = match id {
            ID_PROFILE_ADD => {
                let mut profiles = self.profiles.borrow_mut();
                let name = new_profile_name(&profiles);
                profiles.push(Profile::new(
                    &name,
                    config::DEFAULT_COMMAND_LINE,
                    BackendKind::default(),
                ));
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
        self.refresh_profiles(ctx, selected);
        ctx.preview();
        let focus = if id == ID_PROFILE_ADD {
            self.profile_name
        } else {
            self.profiles_list.hwnd()
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
}

impl Page for ProfilesPage {
    fn controls(&self) -> &[HWND] {
        &self.controls
    }

    fn fill(&self, ctx: &dyn DialogContext, config: &Config) {
        *self.profiles.borrow_mut() = config.profiles.clone();
        self.default_profile.set(config.default_profile_index());
        self.refresh_profiles(ctx, config.default_profile_index());
    }

    fn read(
        &self,
        ctx: &dyn DialogContext,
        config: &mut Config,
    ) -> Result<(), (&'static str, HWND)> {
        let profiles = self.profiles.borrow().clone();
        if let Err((index, field, message)) = check_profiles(&profiles) {
            // エラーのあるプロファイルを選び、その入力欄へ移れるようにする。
            if self.selected_profile() != Some(index) {
                self.profiles_list.select(index);
                self.on_profile_selected(ctx);
            }
            let control = match field {
                ProfileField::Name => self.profile_name,
                ProfileField::Command => self.profile_command,
            };
            return Err((message, control));
        }
        config.default_profile = default_profile_name(
            &profiles,
            self.default_profile.get(),
            ctx.base().default_profile.as_deref(),
        );
        config.profiles = profiles;
        Ok(())
    }

    fn on_command(&self, ctx: &dyn DialogContext, id: i32, code: u32) -> bool {
        match (id, code) {
            (ID_PROFILES, LBN_SELCHANGE) => self.on_profile_selected(ctx),
            (ID_PROFILE_ADD | ID_PROFILE_REMOVE | ID_PROFILE_DEFAULT, BN_CLICKED) => {
                self.on_profile_button(ctx, id);
            }
            (
                ID_PROFILE_NAME | ID_PROFILE_COMMAND | ID_PROFILE_LOCATION | ID_PROFILE_ICON,
                EN_CHANGE,
            ) => {
                self.on_profile_edited(ctx);
            }
            (ID_PROFILE_ICON_CHOOSE, BN_CLICKED) => self.on_profile_icon_choose(ctx),
            (ID_PROFILE_BACKEND, CBN_SELCHANGE) => self.on_profile_backend_changed(ctx),
            _ => return false,
        }
        true
    }
}

/// キーのページ。キーの割り当ての一覧と、追加・変更・外す・既定に戻すのボタン。
struct KeysPage {
    controls: Vec<HWND>,
    /// キーの割り当ての一覧。
    keys_list: ListBox,
    /// 追加・変更・外す・既定に戻すのボタン。
    key_buttons: [HWND; 4],
    /// 編集中のキーの割り当て。
    keys: RefCell<Keymap>,
    /// キーの割り当ての一覧の行。一覧の順。
    key_rows: RefCell<Vec<KeyRow>>,
}

impl KeysPage {
    /// ページのコントロールを作る。
    fn create(b: &mut Builder) -> windows::core::Result<Self> {
        let mut controls = Vec::new();
        controls.push(b.label(
            "キーの割り当て(&K):",
            [PAGE_LEFT, row(0), PAGE_WIDTH, ROW_HEIGHT],
        )?);
        let key_buttons_top = BUTTON_TOP - MARGIN - BUTTON_HEIGHT;
        let keys_list = b.list(
            ID_KEYS,
            [
                PAGE_LEFT,
                row(0) + ROW_HEIGHT,
                PAGE_WIDTH,
                key_buttons_top - BUTTON_GAP - row(0) - ROW_HEIGHT,
            ],
            &[KEY_COLUMN_TAB_STOP],
        )?;
        controls.push(keys_list.hwnd());
        let key_button_width = (PAGE_WIDTH - 3 * BUTTON_GAP) / 4;
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
            key_buttons[index] = b.button(
                id,
                text,
                [
                    PAGE_LEFT + index as i32 * (key_button_width + BUTTON_GAP),
                    key_buttons_top,
                    key_button_width,
                    BUTTON_HEIGHT,
                ],
            )?;
            controls.push(key_buttons[index]);
        }
        Ok(Self {
            controls,
            keys_list,
            key_buttons,
            keys: RefCell::new(Keymap::default()),
            key_rows: RefCell::new(Vec::new()),
        })
    }

    /// キーの割り当ての一覧を作り直し、`selected` の行（なければ同じ動作の行、それもなければ先頭）を
    /// 選ぶ。
    fn refresh_keys(&self, selected: Option<KeyRow>) {
        let rows = key_rows(&self.keys.borrow());
        let list = self.keys_list;
        list.reset_with(rows.iter().copied().map(key_row_text));
        let index = selected
            .and_then(|selected| {
                rows.iter()
                    .position(|row| *row == selected)
                    .or_else(|| rows.iter().position(|(action, _)| *action == selected.0))
            })
            .unwrap_or(0);
        list.select(index);
        *self.key_rows.borrow_mut() = rows;
        self.update_key_buttons();
    }

    /// キーの割り当ての一覧で選んでいる行。
    fn selected_key_row(&self) -> Option<KeyRow> {
        let index = self.keys_list.selected()?;
        self.key_rows.borrow().get(index).copied()
    }

    /// 選んでいる行で押せるボタンだけを押せるようにする。
    fn update_key_buttons(&self) {
        let row = self.selected_key_row();
        let has_combo = row.is_some_and(|(_, combo)| combo.is_some());
        let not_default =
            row.is_some_and(|(action, _)| !is_default_binding(&self.keys.borrow(), action));
        let [add, change, remove, reset] = self.key_buttons;
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
    fn on_key_button(&self, ctx: &dyn DialogContext, id: i32) {
        let Some((action, combo)) = self.selected_key_row() else {
            return;
        };
        let selected = match id {
            ID_KEY_ADD => {
                let Some(new) = self.capture_key(ctx, action) else {
                    return;
                };
                if !self.assign(ctx, action, new, None) {
                    return;
                }
                (action, Some(new))
            }
            ID_KEY_CHANGE => {
                let Some(old) = combo else {
                    return;
                };
                let Some(new) = self.capture_key(ctx, action) else {
                    return;
                };
                if !self.assign(ctx, action, new, Some(old)) {
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
        ctx.preview();
        // SAFETY: 一覧はダイアログの子の有効なウィンドウ。押したボタンが押せなくなることがあるので、
        // 一覧へ戻す。
        unsafe { _ = SetFocus(Some(self.keys_list.hwnd())) };
    }

    /// 動作 `action` に割り当てるキーを、押したキーを読み取るダイアログで読む。
    fn capture_key(&self, ctx: &dyn DialogContext, action: Action) -> Option<KeyCombo> {
        let prompt = format!(
            "「{}」に割り当てるキーを押してください。",
            action_label(action)
        );
        key_capture::capture(ctx.hwnd(), &prompt, ctx.font(), ctx.dpi())
    }

    /// 組み合わせ `new` を動作 `action` に割り当てる。`replacing` があれば、その組み合わせを外す。`new`
    /// をほかの動作に割り当てていれば、割り当て直すかを尋ね、やめたら `false`。
    fn assign(
        &self,
        ctx: &dyn DialogContext,
        action: Action,
        new: KeyCombo,
        replacing: Option<KeyCombo>,
    ) -> bool {
        let bound = self.keys.borrow().lookup_combo(new);
        if bound == Some(action) && replacing.is_none_or(|old| old == new) {
            return true;
        }
        if let Some(other) = bound
            && other != action
        {
            let text = format!(
                "{} は「{}」に割り当てられています。「{}」に割り当て直しますか？",
                new.label(),
                action_label(other),
                action_label(action)
            );
            // SAFETY: hwnd は有効なウィンドウで、文字列はこの呼び出しの間は生きている。借用は持っていない。
            let answer = unsafe {
                MessageBoxW(
                    Some(ctx.hwnd()),
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
}

impl Page for KeysPage {
    fn controls(&self) -> &[HWND] {
        &self.controls
    }

    fn fill(&self, _ctx: &dyn DialogContext, config: &Config) {
        *self.keys.borrow_mut() = config.keys.clone();
        self.refresh_keys(None);
    }

    fn read(
        &self,
        _ctx: &dyn DialogContext,
        config: &mut Config,
    ) -> Result<(), (&'static str, HWND)> {
        config.keys = self.keys.borrow().clone();
        Ok(())
    }

    fn on_command(&self, ctx: &dyn DialogContext, id: i32, code: u32) -> bool {
        match (id, code) {
            (ID_KEYS, LBN_SELCHANGE) => self.update_key_buttons(),
            (ID_KEYS, LBN_DBLCLK) => self.on_key_button(ctx, ID_KEY_CHANGE),
            (ID_KEY_ADD | ID_KEY_CHANGE | ID_KEY_REMOVE | ID_KEY_RESET, BN_CLICKED) => {
                self.on_key_button(ctx, id);
            }
            _ => return false,
        }
        true
    }
}

/// 色を選ぶダイアログの「作成した色」の数。
const CUSTOM_COLORS: usize = 16;

/// 色のページの一行のコントロール。
struct ColorRow {
    swatch: HWND,
    text: HWND,
    reset: HWND,
}

/// 色のページ。配色と、端末・タブバー・スクロールバーの色の表。
struct ColorsPage {
    controls: Vec<HWND>,
    /// 配色のラジオボタン。
    color_scheme: RadioGroup<ColorScheme>,
    /// 色のページの行。[`COLOR_ITEMS`] の順。
    color_rows: Vec<ColorRow>,
    /// 編集中の色。
    colors: RefCell<ColorsConfig>,
    /// 書いていないタブバーの色として見せる色。
    tab_bar_defaults: [Rgb; config::TAB_BAR_COLOR_KEYS.len()],
    /// 色の見本を塗るブラシ。[`COLOR_ITEMS`] の順。
    swatch_brushes: RefCell<Vec<HBRUSH>>,
    /// 色を選ぶダイアログの「作成した色」。ダイアログを開いている間は覚えておく。
    custom_colors: Cell<[COLORREF; CUSTOM_COLORS]>,
}

impl ColorsPage {
    /// ページのコントロールを作る。`tab_bar_defaults` は、書いていないタブバーの色として見せる色。
    fn create(
        b: &mut Builder,
        tab_bar_defaults: [Rgb; config::TAB_BAR_COLOR_KEYS.len()],
    ) -> windows::core::Result<Self> {
        let mut controls = Vec::new();
        let mut color_rows = Vec::new();
        // 端末の色の表の見出しの下に作る。
        let mut color_scheme = None;
        let mut top = MARGIN;
        for (table_index, table) in ColorTable::ALL.into_iter().enumerate() {
            controls.push(b.label(
                COLOR_TABLE_LABELS[table_index],
                [PAGE_LEFT, top + LABEL_OFFSET, PAGE_WIDTH, COLOR_ROW_HEIGHT],
            )?);
            top += COLOR_ROW_HEIGHT;
            if table == ColorTable::Terminal {
                controls.push(b.label(
                    "配色(&M):",
                    [
                        PAGE_LEFT + COLOR_LABEL_INDENT,
                        top + LABEL_OFFSET,
                        LABEL_WIDTH - COLOR_LABEL_INDENT,
                        COLOR_ROW_HEIGHT,
                    ],
                )?);
                let scheme = RadioGroup::create(
                    b,
                    ID_COLOR_SCHEME,
                    &ColorScheme::ALL,
                    &COLOR_SCHEME_LABELS,
                    |index| {
                        [
                            CONTROL_LEFT + index * COLOR_SCHEME_WIDTH,
                            top,
                            COLOR_SCHEME_WIDTH,
                            COLOR_ROW_HEIGHT,
                        ]
                    },
                )?;
                controls.extend_from_slice(scheme.buttons());
                color_scheme = Some(scheme);
                top += COLOR_ROW_HEIGHT;
            }
            for label_text in color_labels(table) {
                let id = ID_COLORS + color_rows.len() as i32 * COLOR_ID_STRIDE;
                controls.push(b.label(
                    label_text,
                    [
                        PAGE_LEFT + COLOR_LABEL_INDENT,
                        top + LABEL_OFFSET,
                        LABEL_WIDTH - COLOR_LABEL_INDENT,
                        COLOR_ROW_HEIGHT,
                    ],
                )?);
                let swatch = b.static_text(
                    id + COLOR_SWATCH,
                    WS_EX_CLIENTEDGE,
                    [CONTROL_LEFT, top + 2, SWATCH_WIDTH, COLOR_ROW_HEIGHT - 4],
                )?;
                let mut left = CONTROL_LEFT + SWATCH_WIDTH + BUTTON_GAP;
                let text = b.static_text(
                    id + COLOR_TEXT,
                    WINDOW_EX_STYLE(0),
                    [left, top + LABEL_OFFSET, COLOR_TEXT_WIDTH, COLOR_ROW_HEIGHT],
                )?;
                left += COLOR_TEXT_WIDTH + BUTTON_GAP;
                let choose = b.button(
                    id + COLOR_CHOOSE,
                    "選択…",
                    [left, top, CHOOSE_WIDTH, COLOR_ROW_HEIGHT - 2],
                )?;
                left += CHOOSE_WIDTH + BUTTON_GAP;
                let reset = b.button(
                    id + COLOR_RESET,
                    "既定に戻す",
                    [left, top, COLOR_RESET_WIDTH, COLOR_ROW_HEIGHT - 2],
                )?;
                controls.extend([swatch, text, choose, reset]);
                color_rows.push(ColorRow {
                    swatch,
                    text,
                    reset,
                });
                top += COLOR_ROW_HEIGHT;
            }
        }
        Ok(Self {
            controls,
            color_scheme: color_scheme.expect("the terminal colors are listed"),
            color_rows,
            colors: RefCell::new(ColorsConfig::default()),
            tab_bar_defaults,
            swatch_brushes: RefCell::new(vec![HBRUSH::default(); COLOR_ITEMS.len()]),
            custom_colors: Cell::new([COLORREF(0); CUSTOM_COLORS]),
        })
    }

    /// 色のページのすべての色を、編集中の色に合わせる（配色や端末の色を変えると、ほかの色の既定の
    /// 色も変わる）。
    fn refresh_colors(&self) {
        for item in 0..self.color_rows.len() {
            self.refresh_color(item);
        }
    }

    /// テーブル `table` の `index` 番目の色の、編集中の色での既定の色。
    fn default_color(&self, table: ColorTable, index: usize) -> Rgb {
        default_color(&self.colors.borrow(), &self.tab_bar_defaults, table, index)
    }

    /// 色のページの `item` 番目の色の、見本・値・既定に戻すボタンを、編集中の色に合わせる。
    fn refresh_color(&self, item: usize) {
        let (table, index) = COLOR_ITEMS[item];
        let written = self.colors.borrow().table(table)[index];
        let color = written.unwrap_or(self.default_color(table, index));
        let row = &self.color_rows[item];
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
    fn on_color_button(&self, ctx: &dyn DialogContext, id: i32) {
        let offset = id - ID_COLORS;
        let item = (offset / COLOR_ID_STRIDE) as usize;
        let Some(&(table, index)) = COLOR_ITEMS.get(item) else {
            return;
        };
        let new = match offset % COLOR_ID_STRIDE {
            COLOR_CHOOSE => {
                let written = self.colors.borrow().table(table)[index];
                let current = written.unwrap_or(self.default_color(table, index));
                let Some(chosen) = self.choose_color(ctx, current) else {
                    return;
                };
                Some(chosen)
            }
            COLOR_RESET => None,
            _ => return,
        };
        self.colors.borrow_mut().table_mut(table)[index] = new;
        self.refresh_colors();
        ctx.preview();
    }

    /// 配色のラジオボタンが押された。
    fn on_color_scheme(&self, ctx: &dyn DialogContext) {
        let Some(scheme) = self.color_scheme.read() else {
            return;
        };
        self.colors.borrow_mut().scheme = scheme;
        self.refresh_colors();
        ctx.preview();
    }

    /// 色を選ぶダイアログを、`current` を選んだ状態で開く。やめたら `None`。
    fn choose_color(&self, ctx: &dyn DialogContext, current: Rgb) -> Option<Rgb> {
        let mut custom = self.custom_colors.get();
        let mut choose = CHOOSECOLORW {
            lStructSize: size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: ctx.hwnd(),
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
            .color_rows
            .iter()
            .position(|row| row.swatch == control)?;
        self.swatch_brushes.borrow().get(item).copied()
    }

    /// 色の見本を塗るブラシを消す。ダイアログが閉じてから呼ぶ。
    fn delete_swatch_brushes(&self) {
        for brush in self.swatch_brushes.take() {
            if !brush.is_invalid() {
                // SAFETY: ブラシは色の見本のために作ったもので、ダイアログはもう閉じている。
                unsafe { _ = DeleteObject(HGDIOBJ(brush.0)) };
            }
        }
    }
}

impl Page for ColorsPage {
    fn controls(&self) -> &[HWND] {
        &self.controls
    }

    fn fill(&self, _ctx: &dyn DialogContext, config: &Config) {
        *self.colors.borrow_mut() = config.colors.clone();
        self.color_scheme.fill(config.colors.scheme);
        self.refresh_colors();
    }

    fn read(
        &self,
        _ctx: &dyn DialogContext,
        config: &mut Config,
    ) -> Result<(), (&'static str, HWND)> {
        config.colors = self.colors.borrow().clone();
        Ok(())
    }

    fn on_command(&self, ctx: &dyn DialogContext, id: i32, code: u32) -> bool {
        match (id, code) {
            (id, BN_CLICKED) if id >= ID_COLORS => self.on_color_button(ctx, id),
            (id, BN_CLICKED) if self.color_scheme.owns(id) => self.on_color_scheme(ctx),
            _ => return false,
        }
        true
    }
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
    let mut monitor = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: owner は有効なウィンドウで、monitor は cbSize を入れて初期化済み。
    let found = unsafe {
        GetMonitorInfoW(
            MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        )
    };
    let (left, top) = if found.as_bool() {
        let work = monitor.rcWork;
        (
            keep_within(left, width, work.left, work.right),
            keep_within(top, height, work.top, work.bottom),
        )
    } else {
        tracing::warn!("failed to read the work area for the settings dialog");
        (left, top)
    };
    // SAFETY: dialog は有効なウィンドウ。
    unsafe { SetWindowPos(dialog, None, left, top, width, height, SWP_NOZORDER) }
}

/// 位置 `start` から長さ `length` の範囲が `low` から `high` までに収まる位置。収まらなければ `low`（タイトルバーを画面に
/// 残す）。
fn keep_within(start: i32, length: i32, low: i32, high: i32) -> i32 {
    start.min(high - length).max(low)
}

/// 96 DPI のときの `value` ピクセルを、DPI が `dpi` のときのピクセルにする。
fn scale(value: i32, dpi: u32) -> i32 {
    (value as f32 * dpi as f32 / BASE_DPI as f32).round() as i32
}

impl State<'_> {
    /// ページ。[`PAGES`] の順。
    fn pages(&self) -> [&dyn Page; PAGES.len()] {
        [
            &self.appearance,
            &self.background,
            &self.behavior,
            &self.profiles,
            &self.keys,
            &self.colors,
        ]
    }

    /// コントロールに設定 `config` を入れる。
    fn fill(&self, config: &Config) {
        // 読む順（[`Self::read`]）と同じ順に入れる。
        self.appearance.fill(self, config);
        self.behavior.fill(self, config);
        self.profiles.fill(self, config);
        self.background.fill(self, config);
        self.keys.fill(self, config);
        self.colors.fill(self, config);
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
        let number_fields: Vec<&dyn NumberInput> = self
            .pages()
            .into_iter()
            .flat_map(|page| page.number_fields())
            .collect();
        let all = c
            .placed
            .iter()
            .map(|(control, _)| *control)
            .chain(number_fields.iter().map(|field| field.spin()));
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
        // 上下の矢印は、入力欄を置いたあとで付ける（付けるときに入力欄の幅を詰める）。
        for field in &number_fields {
            field.attach_spin();
        }
        self.profiles.show_profile_command_warning(self);
    }

    /// `page` 番目のページのコントロールだけを見せる。
    fn show_page(&self, page: usize) {
        self.controls.pages.select(page);
        for (index, controls) in self.pages().into_iter().map(Page::controls).enumerate() {
            let show = if index == page { SW_SHOW } else { SW_HIDE };
            for &control in controls {
                // SAFETY: control はダイアログの子の有効なウィンドウ。
                unsafe { _ = ShowWindow(control, show) };
            }
        }
    }

    /// コントロールから編集中の設定を組み立てる。読めない入力があれば、その文と入力欄を返す。
    fn read(&self) -> Result<Config, (&'static str, HWND)> {
        let mut config = self.base.borrow().clone();
        // 読めない入力がいくつかあれば、この順で最初のもののエラーを見せる。
        self.appearance.read(self, &mut config)?;
        self.behavior.read(self, &mut config)?;
        self.profiles.read(self, &mut config)?;
        self.background.read(self, &mut config)?;
        self.keys.read(self, &mut config)?;
        self.colors.read(self, &mut config)?;
        Ok(config)
    }

    /// [`DialogContext::preview`] をして、読めた設定を返す。読めなければ、見せた文と入力欄を返す。
    fn preview_edited(&self) -> Result<Config, (&'static str, HWND)> {
        // SAFETY: hwnd は有効なウィンドウ。掛けていないタイマーを止めても害はない。
        unsafe { _ = KillTimer(Some(self.hwnd), PREVIEW_TIMER_ID) };
        let config = match self.read() {
            Ok(config) => config,
            Err((message, control)) => {
                set_text(self.controls.error, message);
                return Err((message, control));
            }
        };
        set_text(self.controls.error, "");
        self.preview_config(&config);
        Ok(config)
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
        let edited = match self.preview_edited() {
            Ok(edited) => edited,
            Err((_, control)) => {
                // 読めない入力欄のページを見せて、その入力欄へ移る。エラーメッセージは preview_edited が
                // 見せている。
                if let Some(page) = self
                    .pages()
                    .into_iter()
                    .position(|page| page.controls().contains(&control))
                {
                    self.show_page(page);
                }
                // SAFETY: control はダイアログの子の有効なウィンドウ。
                unsafe { _ = SetFocus(Some(control)) };
                return false;
            }
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
        self.appearance.saved(&base);
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

    /// 上下の矢印のボタン `spin` が押された。相手の入力欄の数を `delta` の向きに一つ動かす。入力欄が
    /// 読めなければ、その入力欄のページの [`Page::step_start`] の設定の値から動かす。
    fn step(&self, spin: HWND, delta: i32) {
        for page in self.pages() {
            let fields = page.number_fields();
            let Some(field) = fields.iter().find(|field| field.spin() == spin) else {
                continue;
            };
            let start = page.step_start(self);
            // 借用は持っていない（入力欄の EN_CHANGE はこの中で届く）。
            field.step(delta, &start);
            break;
        }
        // 矢印は打ち間違えないので、待たずに反映する。
        self.preview();
    }
}

impl DialogContext for State<'_> {
    fn hwnd(&self) -> HWND {
        self.hwnd
    }

    fn base(&self) -> Ref<'_, Config> {
        self.base.borrow()
    }

    fn font(&self) -> HFONT {
        self.font.get()
    }

    fn dpi(&self) -> u32 {
        self.dpi.get()
    }

    fn placed(&self) -> &[(HWND, DipRect)] {
        &self.controls.placed
    }

    fn preview(&self) {
        _ = self.preview_edited();
    }

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

/// スクロールバーの幅、余白、貼り付けの確認の大きさを、矢印で一つ動かした値。範囲 `range` の端で止める。
fn step_integer(current: u32, delta: i32, range: &RangeInclusive<u32>) -> u32 {
    let next = if delta > 0 {
        current.saturating_add(1)
    } else {
        current.saturating_sub(1)
    };
    next.clamp(*range.start(), *range.end())
}

/// 入力欄に見せる、整数の値。
fn format_integer(value: u32) -> String {
    value.to_string()
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
    filters_for(IMAGE_EXTENSIONS.iter().copied())
}

/// プロファイルのアイコンを選ぶダイアログの、ファイルの種類の絞り込み。
fn icon_filters() -> Vec<file_dialog::Filter> {
    filters_for(std::iter::once("ico").chain(IMAGE_EXTENSIONS.iter().copied()))
}

/// 拡張子が `extensions` の画像と、すべてのファイルの絞り込み。
fn filters_for<'a>(extensions: impl Iterator<Item = &'a str>) -> Vec<file_dialog::Filter> {
    let spec = extensions
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
                    if let Some(page) = state.controls.pages.selected()
                        && page < PAGES.len()
                    {
                        state.show_page(page);
                    }
                }
                // ページのコントロールの ID はページの間で重ならないので、受け持つページが処理したら
                // 止める。
                _ => {
                    _ = state
                        .pages()
                        .into_iter()
                        .any(|page| page.on_command(state, id, code));
                }
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => match state.colors.swatch_brush(HWND(lparam.0 as *mut _)) {
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
        assert_eq!(step_integer(12, 1, &width), 13);
        assert_eq!(step_integer(4, -1, &width), 4);
        assert_eq!(step_integer(64, 1, &width), 64);
        let padding = config::PADDING_RANGE;
        assert_eq!(step_integer(4, -1, &padding), 3);
        assert_eq!(step_integer(0, -1, &padding), 0);
        assert_eq!(step_integer(64, 1, &padding), 64);
    }

    #[test]
    fn number_fields_read_what_they_show() {
        // `parse` に渡す項目の名前の誤りは、このテストだけが見つける。`NumberKind` の定数を足したら、
        // ここにも足す（`Page::number_fields` はウィンドウが要るので使えない）。
        fn reads_back<T: Copy + PartialEq + std::fmt::Debug>(kind: &NumberKind<T>) {
            let value = (kind.start)(&Config::default());
            assert_eq!(
                (kind.parse)(&(kind.format)(value)),
                Some(value),
                "{}",
                kind.message
            );
        }
        reads_back(&FONT_SIZE_KIND);
        reads_back(&SCROLL_BAR_WIDTH_KIND);
        reads_back(&PADDING_KIND);
        reads_back(&PASTE_CONFIRM_SIZE_KIND);
        reads_back(&BACKGROUND_OPACITY_KIND);
        reads_back(&BACKGROUND_ITEM_OPACITY_KIND);
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
        Profile::new(name, command, BackendKind::Conpty)
    }

    #[test]
    fn profile_rows_mark_the_default() {
        let zsh = profile("zsh", "zsh -l");
        assert_eq!(profile_row_text(&zsh, false), "zsh\tzsh -l");
        assert_eq!(profile_row_text(&zsh, true), "zsh（既定）\tzsh -l");
        let cygwin = Profile::new("c", "", BackendKind::Cygwin);
        assert_eq!(
            profile_row_text(&cygwin, false),
            "c\tCygwin: （ログインシェル）"
        );
        let wsl = Profile::new("u", "zsh -l", BackendKind::Wsl);
        assert_eq!(profile_row_text(&wsl, false), "u\tWSL: zsh -l");
    }

    #[test]
    fn profile_location_follows_the_backend() {
        let mut cygwin = Profile::new("c", "", BackendKind::Cygwin);
        set_profile_location(&mut cygwin, r"C:\app\cygwin".to_owned());
        assert_eq!(cygwin.cygwin_root.as_deref(), Some(r"C:\app\cygwin"));
        assert_eq!(cygwin.distribution, None);
        set_profile_location(&mut cygwin, String::new());
        assert_eq!(cygwin.cygwin_root, None);
        let mut wsl = Profile::new("u", "", BackendKind::Wsl);
        set_profile_location(&mut wsl, "Ubuntu".to_owned());
        assert_eq!(wsl.distribution.as_deref(), Some("Ubuntu"));
        assert_eq!(wsl.cygwin_root, None);
        let mut conpty = profile("p", "cmd.exe");
        set_profile_location(&mut conpty, "x".to_owned());
        assert_eq!((conpty.cygwin_root, conpty.distribution), (None, None));
        // ブリッジのコマンドラインは空でよい。
        assert_eq!(check_profiles(&[cygwin, wsl]), Ok(()));
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
        assert_eq!(ITEM_FIT_CHOICES[0], None);
        for (index, fit) in BackgroundFit::ALL.into_iter().enumerate() {
            assert_eq!(ITEM_FIT_CHOICES[index + 1], Some(fit));
        }
        assert_eq!(ITEM_FIT_CHOICES.len(), BackgroundFit::ALL.len() + 1);
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
        let items = COLOR_ITEMS;
        let expected: Vec<(ColorTable, usize)> = ColorTable::ALL
            .iter()
            .flat_map(|&table| (0..table.keys().len()).map(move |index| (table, index)))
            .collect();
        assert_eq!(items.to_vec(), expected);
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
    fn profile_command_warning_says_what_to_write_on_each_side() {
        let warning = |backend: BackendKind, command: &str| {
            profile_command_warning(&Profile::new("p", command, backend))
        };
        let wsl = warning(BackendKind::Wsl, r"C:\Windows\System32\wsl.exe -d Ubuntu")
            .expect("wsl.exe in a bridge");
        assert!(wsl.contains("WSL の中のコマンド（例: /bin/bash）"), "{wsl}");
        let cygwin = warning(BackendKind::Cygwin, "cmd.exe").expect("cmd.exe in a bridge");
        assert!(
            cygwin.contains("Cygwin の中のコマンド（例: /bin/zsh -l）"),
            "{cygwin}"
        );
        assert_eq!(warning(BackendKind::Conpty, "wsl.exe"), None);
        assert_eq!(warning(BackendKind::Cygwin, "zsh.exe -l"), None);
        assert_eq!(warning(BackendKind::Wsl, ""), None);
    }

    #[test]
    fn the_profile_page_fits_above_the_buttons() {
        let name_row =
            MARGIN + ROW_HEIGHT + PROFILE_LIST_HEIGHT + BUTTON_GAP + BUTTON_HEIGHT + MARGIN;
        // 名前の下に接続方式、場所、コマンドライン、アイコンの 4 行と説明の文。
        let note_top = name_row + 5 * (ROW_HEIGHT + MARGIN);
        let button_top = CLIENT_HEIGHT - MARGIN - BUTTON_HEIGHT;
        // 警告の文（幅に対して 3 行、既定のメッセージのフォントで 1 行 16 ピクセルまで）が出ると、説明の
        // 文もその分だけ下がる。
        let warning_shift = LABEL_OFFSET + 3 * 16;
        assert!(note_top + warning_shift + PROFILE_NOTE_HEIGHT <= button_top - MARGIN);
    }

    #[test]
    fn the_dialog_position_stays_within_the_work_area() {
        assert_eq!(keep_within(100, 50, 0, 1000), 100, "already inside");
        assert_eq!(keep_within(-30, 50, 0, 1000), 0, "past the start");
        assert_eq!(keep_within(980, 50, 0, 1000), 950, "past the end");
        assert_eq!(keep_within(100, 2000, 0, 1000), 0, "longer than the area");
    }

    #[test]
    fn sizes_are_shown_as_written() {
        assert_eq!(format_size(12.0), "12");
        assert_eq!(format_size(10.5), "10.5");
    }
}
