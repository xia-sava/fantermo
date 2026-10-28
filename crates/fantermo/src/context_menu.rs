//! 右クリックのコンテキストメニュー。

use windows::{
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, DestroyMenu, HMENU, MENU_ITEM_FLAGS, MF_GRAYED,
            MF_SEPARATOR, MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
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
}

impl MenuCommand {
    const ALL: [MenuCommand; 3] = [
        MenuCommand::Copy,
        MenuCommand::Paste,
        MenuCommand::SelectAll,
    ];

    /// メニューの項目の ID。0 は「何も選ばれなかった」に使われるので避ける。
    fn id(self) -> usize {
        match self {
            MenuCommand::Copy => 1,
            MenuCommand::Paste => 2,
            MenuCommand::SelectAll => 3,
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
        }
    }
}

/// 画面の (`x`, `y`) にメニューを出し、選ばれた操作を返す。`can_copy` でなければコピーを押せなくする。
pub fn show(owner: HWND, x: i32, y: i32, can_copy: bool) -> Option<MenuCommand> {
    let menu = match build(can_copy) {
        Ok(menu) => menu,
        Err(e) => {
            tracing::warn!("failed to build the context menu: {e}");
            return None;
        }
    };
    // SAFETY: menu は作ったばかりのメニューで、owner は有効な窓。選ばれた ID を戻り値で受け取る。
    let chosen =
        unsafe { TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, x, y, owner, None) };
    // SAFETY: menu はここで一度だけ壊す。
    if let Err(e) = unsafe { DestroyMenu(menu) } {
        tracing::warn!("failed to destroy the context menu: {e}");
    }
    MenuCommand::from_id(chosen.0 as usize)
}

fn build(can_copy: bool) -> windows::core::Result<HMENU> {
    // SAFETY: 空のメニューを作るだけ。
    let menu = unsafe { CreatePopupMenu()? };
    let append = |flags: MENU_ITEM_FLAGS, command: Option<MenuCommand>| {
        let label = command.map(|command| HSTRING::from(command.label()));
        // SAFETY: menu は有効なメニューで、文字列はこの呼び出しの間は生きている。
        unsafe {
            AppendMenuW(
                menu,
                flags,
                command.map_or(0, MenuCommand::id),
                label
                    .as_ref()
                    .map_or(windows::core::PCWSTR::null(), |label| {
                        windows::core::PCWSTR(label.as_ptr())
                    }),
            )
        }
    };
    let copy_flags = if can_copy {
        MF_STRING
    } else {
        MF_STRING | MF_GRAYED
    };
    append(copy_flags, Some(MenuCommand::Copy))?;
    append(MF_STRING, Some(MenuCommand::Paste))?;
    append(MF_SEPARATOR, None)?;
    append(MF_STRING, Some(MenuCommand::SelectAll))?;
    Ok(menu)
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
}
