//! 設定ダイアログのプロファイルのページ。プロファイルの一覧と、一覧で選んでいるプロファイルの値。

use std::cell::{Cell, RefCell};

use fantermo_core::{
    background::IMAGE_EXTENSIONS,
    config::{self, BackendKind, Config, Profile, ProfileError},
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, RECT, WPARAM},
        Graphics::Gdi::{
            DT_CALCRECT, DT_WORDBREAK, DrawTextW, GetDC, HFONT, HGDIOBJ, ReleaseDC, SelectObject,
        },
        UI::{
            Input::KeyboardAndMouse::{EnableWindow, SetFocus},
            WindowsAndMessaging::{
                BN_CLICKED, CBN_SELCHANGE, EN_CHANGE, LBN_SELCHANGE, MoveWindow, SWP_NOACTIVATE,
                SWP_NOSIZE, SWP_NOZORDER, SendMessageW, SetWindowPos, WINDOW_EX_STYLE,
            },
        },
    },
    core::HSTRING,
};

use super::{
    BUTTON_GAP, BUTTON_HEIGHT, BUTTON_WIDTH, CONTROL_LEFT, CONTROL_WIDTH, DialogContext,
    LABEL_OFFSET, MARGIN, PAGE_LEFT, PAGE_WIDTH, Page, ROW_HEIGHT, filters_for, label_rect, row,
    scale,
    widgets::{Builder, ChoiceBox, ListBox, get_text, set_text},
};
use crate::file_dialog;

/// このページのコントロールの ID の範囲。範囲がページの間で重ならないことと、このページの ID が範囲に
/// 入ることを単体のテストで見る。
#[cfg(test)]
pub(super) const ID_RANGE: std::ops::RangeInclusive<i32> = 400..=449;
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

/// プロファイルのアイコンを選ぶダイアログの、ファイルの種類の絞り込み。
fn icon_filters() -> Vec<file_dialog::Filter> {
    filters_for(std::iter::once("ico").chain(IMAGE_EXTENSIONS.iter().copied()))
}

/// プロファイルのページ。プロファイルの一覧と、一覧で選んでいるプロファイルの値。
pub(super) struct ProfilesPage {
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
    pub(super) fn create(b: &mut Builder) -> windows::core::Result<Self> {
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
    pub(super) fn show_profile_command_warning(&self, ctx: &dyn DialogContext) {
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

#[cfg(test)]
mod tests {
    use super::super::CLIENT_HEIGHT;
    use super::*;

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
    fn the_ids_are_within_the_range_of_the_page() {
        for id in [
            ID_PROFILES,
            ID_PROFILE_ADD,
            ID_PROFILE_REMOVE,
            ID_PROFILE_DEFAULT,
            ID_PROFILE_NAME,
            ID_PROFILE_COMMAND,
            ID_PROFILE_BACKEND,
            ID_PROFILE_LOCATION,
            ID_PROFILE_COMMAND_WARNING,
            ID_PROFILE_ICON,
            ID_PROFILE_ICON_CHOOSE,
        ] {
            assert!(ID_RANGE.contains(&id), "{id}");
        }
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
}
