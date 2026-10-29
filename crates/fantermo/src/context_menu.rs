//! 右クリック、メニューキー、Shift+F10 で出すコンテキストメニューと、タブバーのメニューのボタンで出す
//! メニューと、プロファイルの一覧。

use windows::{
    Win32::{
        Foundation::{HWND, RECT},
        UI::WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, DestroyMenu, HMENU, MENU_ITEM_FLAGS, MF_CHECKED,
            MF_GRAYED, MF_SEPARATOR, MF_STRING, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
            TPM_TOPALIGN, TPM_VERTICAL, TPMPARAMS, TrackPopupMenuEx,
        },
    },
    core::HSTRING,
};

use fantermo_core::keymap::{Action, KeyCombo, Keymap};

/// メニューで選ばれた操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    Copy,
    Paste,
    SelectAll,
    NewTab,
    CloseTab,
    /// 設定ダイアログを開く。
    EditSettings,
    OpenSettings,
    /// 「fantermo について」のダイアログを開く。
    ShowAbout,
    Exit,
}

impl MenuCommand {
    const ALL: [MenuCommand; 9] = [
        MenuCommand::Copy,
        MenuCommand::Paste,
        MenuCommand::SelectAll,
        MenuCommand::NewTab,
        MenuCommand::CloseTab,
        MenuCommand::EditSettings,
        MenuCommand::OpenSettings,
        MenuCommand::ShowAbout,
        MenuCommand::Exit,
    ];

    /// メニューの項目の ID。0 は「何も選ばれなかった」に使われるので避ける。
    fn id(self) -> usize {
        match self {
            MenuCommand::Copy => 1,
            MenuCommand::Paste => 2,
            MenuCommand::SelectAll => 3,
            MenuCommand::NewTab => 4,
            MenuCommand::CloseTab => 5,
            MenuCommand::OpenSettings => 6,
            MenuCommand::Exit => 7,
            MenuCommand::ShowAbout => 8,
            MenuCommand::EditSettings => 9,
        }
    }

    fn from_id(id: usize) -> Option<Self> {
        Self::ALL.into_iter().find(|command| command.id() == id)
    }

    /// 項目の名前。
    fn name(self) -> &'static str {
        match self {
            MenuCommand::Copy => "コピー(&C)",
            MenuCommand::Paste => "貼り付け(&P)",
            MenuCommand::SelectAll => "すべて選択(&A)",
            MenuCommand::NewTab => "新しいタブ(&T)",
            MenuCommand::CloseTab => "タブを閉じる(&W)",
            MenuCommand::EditSettings => "設定(&S)…",
            MenuCommand::OpenSettings => "設定ファイルを開く(&F)",
            MenuCommand::ShowAbout => "fantermo について(&B)…",
            MenuCommand::Exit => "終了(&X)",
        }
    }

    /// 同じことをするキーの割り当ての動作。
    fn action(self) -> Option<Action> {
        match self {
            MenuCommand::Copy => Some(Action::Copy),
            MenuCommand::Paste => Some(Action::Paste),
            MenuCommand::NewTab => Some(Action::NewTab),
            MenuCommand::CloseTab => Some(Action::CloseTab),
            _ => None,
        }
    }

    /// 項目の表示。キーを割り当てていれば、タブの後ろに最初の組み合わせを書く（ショートカットとして
    /// 右に寄せて出る）。
    fn label(self, keys: &Keymap) -> String {
        let shortcut = self
            .action()
            .and_then(|action| keys.combos_of(action).first().map(KeyCombo::label));
        match shortcut {
            Some(shortcut) => format!("{}\t{shortcut}", self.name()),
            None => self.name().to_owned(),
        }
    }
}

/// メニューで選ばれたもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chosen {
    Command(MenuCommand),
    /// プロファイルの一覧の、`MenuRequest::profiles` の中の位置がこの値のプロファイル。
    Profile(usize),
}

/// プロファイルの一覧の項目の ID の始まり。[`MenuCommand`] の ID と重ならないようにする。
const PROFILE_ID_BASE: usize = 100;

/// プロファイルの一覧の項目のうち、数字のアクセラレータ（1〜9）を付ける数。
const NUMBERED_PROFILES: usize = 9;

impl Chosen {
    fn from_id(id: usize) -> Option<Self> {
        match id.checked_sub(PROFILE_ID_BASE) {
            Some(index) => Some(Chosen::Profile(index)),
            None => MenuCommand::from_id(id).map(Chosen::Command),
        }
    }
}

/// プロファイルの一覧の `index` 番目の項目の表示。9 番目までは「&1 」〜「&9 」を前に付けて数字で選べる
/// ようにする。名前の「&」は、アクセラレータの印にならないように重ねる。
fn profile_label(index: usize, name: &str) -> String {
    let name = name.replace('&', "&&");
    if index < NUMBERED_PROFILES {
        format!("&{} {name}", index + 1)
    } else {
        name
    }
}

/// どのメニューか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    /// 右クリックなどで出すコンテキストメニュー。
    Context,
    /// タブバーのメニューのボタンで出すメニュー。
    Main,
    /// タブバーのプロファイルの一覧のボタン（または新しいタブのボタンの右クリック）で出す、
    /// プロファイルの一覧。項目は `MenuRequest::profiles` から作る。
    Profiles,
}

impl Menu {
    /// 項目の並び。`None` は区切り。
    fn items(self) -> &'static [Option<MenuCommand>] {
        use MenuCommand::*;
        match self {
            Menu::Profiles => &[],
            Menu::Context => &[
                Some(Copy),
                Some(Paste),
                None,
                Some(SelectAll),
                None,
                Some(NewTab),
                Some(CloseTab),
            ],
            Menu::Main => &[
                Some(NewTab),
                Some(CloseTab),
                None,
                Some(Copy),
                Some(Paste),
                Some(SelectAll),
                None,
                Some(EditSettings),
                Some(OpenSettings),
                Some(ShowAbout),
                None,
                Some(Exit),
            ],
        }
    }
}

/// メニューを出す場所と、押せない項目を決めるもの。
#[derive(Clone, Debug)]
pub struct MenuRequest {
    /// キーの割り当て。項目に添えるキーの組み合わせを決める。
    pub keys: Keymap,
    pub menu: Menu,
    /// 左上を置く画面の位置。
    pub x: i32,
    pub y: i32,
    /// 隠さない画面の矩形（メニューを出したボタン）。メニューはこの下に出し、下に入らなければ上に出す。
    pub exclude: Option<RECT>,
    /// 選択範囲がある。なければコピーを押せなくする。
    pub can_copy: bool,
    /// 設定ファイルの場所が分かっている。分からなければ「設定…」と「設定ファイルを開く」を押せなく
    /// する。
    pub can_open_settings: bool,
    /// プロファイルの一覧に並べる名前（[`Menu::Profiles`] のとき）。
    pub profiles: Vec<String>,
    /// 既定のプロファイルの、`profiles` の中の位置。一覧で印を付ける。
    pub default_profile: usize,
}

/// `request` のとおりにメニューを出し、選ばれたものを返す。
pub fn show(owner: HWND, request: &MenuRequest) -> Option<Chosen> {
    let menu = match build(request) {
        Ok(menu) => menu,
        Err(e) => {
            tracing::warn!("failed to build the menu: {e}");
            return None;
        }
    };
    let mut flags = TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN;
    let params = request.exclude.map(|exclude| {
        flags |= TPM_VERTICAL;
        TPMPARAMS {
            cbSize: size_of::<TPMPARAMS>() as u32,
            rcExclude: exclude,
        }
    });
    // SAFETY: menu は作ったばかりのメニューで、owner は有効な窓。params はこの呼び出しの間は生きて
    // いる。選ばれた ID を戻り値で受け取る。
    let chosen = unsafe {
        TrackPopupMenuEx(
            menu,
            flags.0,
            request.x,
            request.y,
            owner,
            params.as_ref().map(|params| params as *const TPMPARAMS),
        )
    };
    // SAFETY: menu はここで一度だけ破棄する。
    if let Err(e) = unsafe { DestroyMenu(menu) } {
        tracing::warn!("failed to destroy the menu: {e}");
    }
    Chosen::from_id(chosen.0 as usize)
}

/// 項目を押せるか。
fn enabled(command: MenuCommand, request: &MenuRequest) -> bool {
    match command {
        MenuCommand::Copy => request.can_copy,
        MenuCommand::EditSettings | MenuCommand::OpenSettings => request.can_open_settings,
        _ => true,
    }
}

fn build(request: &MenuRequest) -> windows::core::Result<HMENU> {
    // SAFETY: 空のメニューを作るだけ。
    let menu = unsafe { CreatePopupMenu()? };
    if request.menu == Menu::Profiles {
        for (index, name) in request.profiles.iter().enumerate() {
            let flags = if index == request.default_profile {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let label = HSTRING::from(profile_label(index, name));
            append(menu, flags, PROFILE_ID_BASE + index, Some(&label))?;
        }
        return Ok(menu);
    }
    for item in request.menu.items() {
        let (flags, id, label) = match item {
            Some(command) => {
                let flags = if enabled(*command, request) {
                    MF_STRING
                } else {
                    MF_STRING | MF_GRAYED
                };
                (
                    flags,
                    command.id(),
                    Some(HSTRING::from(command.label(&request.keys))),
                )
            }
            None => (MF_SEPARATOR, 0, None),
        };
        append(menu, flags, id, label.as_ref())?;
    }
    Ok(menu)
}

fn append(
    menu: HMENU,
    flags: MENU_ITEM_FLAGS,
    id: usize,
    label: Option<&HSTRING>,
) -> windows::core::Result<()> {
    let label = label.map_or(windows::core::PCWSTR::null(), |label| {
        windows::core::PCWSTR(label.as_ptr())
    });
    // SAFETY: menu は有効なメニューで、文字列はこの呼び出しの間は生きている。
    unsafe { AppendMenuW(menu, flags, id, label) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_avoid_zero() {
        for command in MenuCommand::ALL {
            assert_ne!(command.id(), 0);
            assert_eq!(MenuCommand::from_id(command.id()), Some(command));
        }
        assert_eq!(MenuCommand::from_id(0), None);
        assert_eq!(Chosen::from_id(0), None);
        assert_eq!(
            Chosen::from_id(MenuCommand::Copy.id()),
            Some(Chosen::Command(MenuCommand::Copy))
        );
        assert_eq!(
            Chosen::from_id(PROFILE_ID_BASE + 2),
            Some(Chosen::Profile(2))
        );
        assert!(MenuCommand::ALL.iter().all(|c| c.id() < PROFILE_ID_BASE));
    }

    #[test]
    fn profile_labels_are_numbered_and_keep_ampersands() {
        assert_eq!(profile_label(0, "PowerShell"), "&1 PowerShell");
        assert_eq!(profile_label(8, "R&D"), "&9 R&&D");
        assert_eq!(profile_label(9, "WSL"), "WSL");
    }

    #[test]
    fn labels_show_the_first_bound_keys() {
        let mut keys = Keymap::default();
        assert_eq!(MenuCommand::Copy.label(&keys), "コピー(&C)\tCtrl+C");
        assert_eq!(
            MenuCommand::Paste.label(&keys),
            "貼り付け(&P)\tShift+Insert"
        );
        assert_eq!(
            MenuCommand::NewTab.label(&keys),
            "新しいタブ(&T)\tCtrl+Shift+T"
        );
        assert_eq!(
            MenuCommand::CloseTab.label(&keys),
            "タブを閉じる(&W)\tCtrl+Shift+W"
        );
        assert_eq!(MenuCommand::Exit.label(&keys), "終了(&X)");
        keys.bind(KeyCombo::parse("ctrl+shift+t").unwrap(), None);
        assert_eq!(MenuCommand::NewTab.label(&keys), "新しいタブ(&T)");
        keys.bind(KeyCombo::parse("alt+t").unwrap(), Some(Action::NewTab));
        assert_eq!(MenuCommand::NewTab.label(&keys), "新しいタブ(&T)\tAlt+T");
    }

    #[test]
    fn every_command_is_in_the_main_menu() {
        for command in MenuCommand::ALL {
            assert!(Menu::Main.items().contains(&Some(command)), "{command:?}");
        }
        // コンテキストメニューには、設定、「fantermo について」、終了を入れない。
        for command in [
            MenuCommand::EditSettings,
            MenuCommand::OpenSettings,
            MenuCommand::ShowAbout,
            MenuCommand::Exit,
        ] {
            assert!(!Menu::Context.items().contains(&Some(command)));
        }
    }
}
