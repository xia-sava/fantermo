//! 右クリック、メニューキー、Shift+F10 で出すコンテキストメニューと、タブバーのメニューのボタンで出す
//! メニュー。

use windows::{
    Win32::{
        Foundation::{HWND, RECT},
        UI::WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, DestroyMenu, HMENU, MENU_ITEM_FLAGS, MF_GRAYED,
            MF_SEPARATOR, MF_STRING, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TPM_TOPALIGN,
            TPM_VERTICAL, TPMPARAMS, TrackPopupMenuEx,
        },
    },
    core::HSTRING,
};

/// メニューで選ばれた操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    Copy,
    Paste,
    SelectAll,
    NewTab,
    CloseTab,
    OpenSettings,
    ShowLicenses,
    Exit,
}

impl MenuCommand {
    const ALL: [MenuCommand; 8] = [
        MenuCommand::Copy,
        MenuCommand::Paste,
        MenuCommand::SelectAll,
        MenuCommand::NewTab,
        MenuCommand::CloseTab,
        MenuCommand::OpenSettings,
        MenuCommand::ShowLicenses,
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
            MenuCommand::ShowLicenses => 8,
        }
    }

    fn from_id(id: usize) -> Option<Self> {
        Self::ALL.into_iter().find(|command| command.id() == id)
    }

    /// 項目の表示。タブの後ろはショートカットとして右に寄せて出る。
    fn label(self) -> &'static str {
        match self {
            MenuCommand::Copy => "コピー(&C)\tCtrl+C",
            MenuCommand::Paste => "貼り付け(&P)\tShift+Insert",
            MenuCommand::SelectAll => "すべて選択(&A)",
            MenuCommand::NewTab => "新しいタブ(&T)\tCtrl+Shift+T",
            MenuCommand::CloseTab => "タブを閉じる(&W)\tCtrl+Shift+W",
            MenuCommand::OpenSettings => "設定ファイルを開く(&S)",
            MenuCommand::ShowLicenses => "ライセンス(&L)",
            MenuCommand::Exit => "終了(&X)",
        }
    }
}

/// どのメニューか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    /// 右クリックなどで出すコンテキストメニュー。
    Context,
    /// タブバーのメニューのボタンで出すメニュー。
    Main,
}

impl Menu {
    /// 項目の並び。`None` は区切り。
    fn items(self) -> &'static [Option<MenuCommand>] {
        use MenuCommand::*;
        match self {
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
                Some(OpenSettings),
                Some(ShowLicenses),
                None,
                Some(Exit),
            ],
        }
    }
}

/// メニューを出す場所と、押せない項目を決めるもの。
#[derive(Clone, Copy, Debug)]
pub struct MenuRequest {
    pub menu: Menu,
    /// 左上を置く画面の位置。
    pub x: i32,
    pub y: i32,
    /// 隠さない画面の矩形（メニューを出したボタン）。メニューはこの下に出し、下に入らなければ上に出す。
    pub exclude: Option<RECT>,
    /// 選んでいる範囲がある。なければコピーを押せなくする。
    pub can_copy: bool,
    /// 設定ファイルの場所が分かっている。分からなければ「設定ファイルを開く」を押せなくする。
    pub can_open_settings: bool,
}

/// `request` のとおりにメニューを出し、選ばれた操作を返す。
pub fn show(owner: HWND, request: &MenuRequest) -> Option<MenuCommand> {
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
    // SAFETY: menu はここで一度だけ壊す。
    if let Err(e) = unsafe { DestroyMenu(menu) } {
        tracing::warn!("failed to destroy the menu: {e}");
    }
    MenuCommand::from_id(chosen.0 as usize)
}

/// 項目を押せるか。
fn enabled(command: MenuCommand, request: &MenuRequest) -> bool {
    match command {
        MenuCommand::Copy => request.can_copy,
        MenuCommand::OpenSettings => request.can_open_settings,
        _ => true,
    }
}

fn build(request: &MenuRequest) -> windows::core::Result<HMENU> {
    // SAFETY: 空のメニューを作るだけ。
    let menu = unsafe { CreatePopupMenu()? };
    for item in request.menu.items() {
        let (flags, id, label) = match item {
            Some(command) => {
                let flags = if enabled(*command, request) {
                    MF_STRING
                } else {
                    MF_STRING | MF_GRAYED
                };
                (flags, command.id(), Some(HSTRING::from(command.label())))
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
    }

    #[test]
    fn every_command_is_in_the_main_menu() {
        for command in MenuCommand::ALL {
            assert!(Menu::Main.items().contains(&Some(command)), "{command:?}");
        }
        // コンテキストメニューには、設定、ライセンス、終了を入れない。
        assert!(!Menu::Context.items().contains(&Some(MenuCommand::Exit)));
        assert!(
            !Menu::Context
                .items()
                .contains(&Some(MenuCommand::OpenSettings))
        );
        assert!(
            !Menu::Context
                .items()
                .contains(&Some(MenuCommand::ShowLicenses))
        );
    }
}
