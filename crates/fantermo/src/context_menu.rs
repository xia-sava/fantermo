//! 右クリック、メニューキー、Shift+F10 で出すコンテキストメニューと、タブバーのメニューのボタンで出す
//! メニューと、プロファイルの一覧。

use windows::{
    Win32::{
        Foundation::{HWND, RECT},
        Graphics::Gdi::{DeleteObject, HBITMAP},
        UI::WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, DestroyMenu, GetMenuItemCount, HMENU, InsertMenuItemW,
            MENU_ITEM_FLAGS, MENUITEMINFOW, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING,
            MIIM_BITMAP, MIIM_STRING, MIIM_SUBMENU, SetMenuItemInfoW, TPM_LEFTALIGN, TPM_RETURNCMD,
            TPM_RIGHTBUTTON, TPM_TOPALIGN, TPM_VERTICAL, TPMPARAMS, TrackPopupMenuEx,
        },
    },
    core::{HSTRING, PWSTR},
};

use fantermo_core::keymap::{Action, KeyCombo, Keymap};

use crate::{gdi_image, image_decoder::Decoded};

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

/// 項目の一覧。並びは [`MenuCommand`] の宣言の順で、組は（項目、名前、同じことをするキーの割り当ての
/// 動作）。
const COMMANDS: [(MenuCommand, &str, Option<Action>); 9] = [
    (MenuCommand::Copy, "コピー(&C)", Some(Action::Copy)),
    (MenuCommand::Paste, "貼り付け(&P)", Some(Action::Paste)),
    (MenuCommand::SelectAll, "すべて選択(&A)", None),
    (MenuCommand::NewTab, "新しいタブ(&T)", Some(Action::NewTab)),
    (
        MenuCommand::CloseTab,
        "タブを閉じる(&W)",
        Some(Action::CloseTab),
    ),
    (MenuCommand::EditSettings, "設定(&S)…", None),
    (MenuCommand::OpenSettings, "設定ファイルを開く(&F)", None),
    (MenuCommand::ShowAbout, "fantermo について(&B)…", None),
    (MenuCommand::Exit, "終了(&X)", None),
];

impl MenuCommand {
    /// メニューの項目の ID。一覧の位置 + 1 で、0（「何も選ばれなかった」に使われる）にならない。
    fn id(self) -> usize {
        self as usize + 1
    }

    fn from_id(id: usize) -> Option<Self> {
        let index = id.checked_sub(1)?;
        COMMANDS.get(index).map(|(command, ..)| *command)
    }

    /// 項目の名前。
    fn name(self) -> &'static str {
        COMMANDS[self as usize].1
    }

    /// 同じことをするキーの割り当ての動作。
    fn action(self) -> Option<Action> {
        COMMANDS[self as usize].2
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
    /// プロファイルの一覧の「管理者として開く」の中の、`MenuRequest::profiles` の中の位置がこの値のプロファイル。
    ElevatedProfile(usize),
}

/// プロファイルの一覧の項目の ID の始まり。[`MenuCommand`] の ID と重ならないようにする。
const PROFILE_ID_BASE: usize = 100;

/// 「管理者として開く」の中の項目の ID に立てるビット。ID は `ELEVATED_ID_FLAG | (PROFILE_ID_BASE + 位置)`。
/// メニューの ID は 16 ビットに収める。
const ELEVATED_ID_FLAG: usize = 0x4000;

/// プロファイルの一覧に並べるプロファイルの数の上限。これを超えると、項目の ID が [`ELEVATED_ID_FLAG`] に届く。
const MAX_MENU_PROFILES: usize = ELEVATED_ID_FLAG - PROFILE_ID_BASE;

/// プロファイルの一覧の項目のうち、数字のアクセラレータ（1〜9）を付ける数。
const NUMBERED_PROFILES: usize = 9;

/// プロファイルの一覧の「管理者として開く」の項目の表示。
const ELEVATE_LABEL: &str = "管理者として開く(&A)";

impl Chosen {
    fn from_id(id: usize) -> Option<Self> {
        if id & ELEVATED_ID_FLAG != 0 {
            let index = (id & !ELEVATED_ID_FLAG).checked_sub(PROFILE_ID_BASE)?;
            return Some(Chosen::ElevatedProfile(index));
        }
        match id.checked_sub(PROFILE_ID_BASE) {
            Some(index) => Some(Chosen::Profile(index)),
            None => MenuCommand::from_id(id).map(Chosen::Command),
        }
    }
}

/// プロファイルの一覧の項目。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    /// `MenuRequest::profiles` の中の位置がこの値のプロファイル。
    Profile(usize),
    Separator,
    /// 「管理者として開く」のサブメニュー。中に並べるプロファイルの、`MenuRequest::profiles` の中の位置。
    Elevated(Vec<usize>),
}

/// プロファイルが `count` 個のときのプロファイルの一覧の並び。プロファイルを順に並べ、管理者として動いていなければ
/// （`elevated` が偽なら）、区切りのあとに同じプロファイルを並べた「管理者として開く」を置く。並べるのは
/// [`MAX_MENU_PROFILES`] 個まで。
fn profile_menu_entries(count: usize, elevated: bool) -> Vec<Entry> {
    let count = count.min(MAX_MENU_PROFILES);
    let mut entries: Vec<Entry> = (0..count).map(Entry::Profile).collect();
    if !elevated && count > 0 {
        entries.push(Entry::Separator);
        entries.push(Entry::Elevated((0..count).collect()));
    }
    entries
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
    /// プロファイルの一覧の項目に付けるアイコン。`profiles` と同じ順で、作れなかったものは `None`。
    pub profile_icons: Vec<Option<Decoded>>,
    /// このプロセスが管理者として動いている。プロファイルの一覧に「管理者として開く」を出さない。
    pub elevated: bool,
    /// プロファイルの一覧の「管理者として開く」に付けるアイコン（UAC の盾）。作れなければ `None`。
    pub elevate_icon: Option<Decoded>,
}

/// メニューの項目に付けたビットマップ。メニューは破棄しても項目のビットマップを消さないので、メニューを
/// 破棄したあとにこれを手放す。
#[derive(Default)]
struct ItemBitmaps(Vec<HBITMAP>);

impl Drop for ItemBitmaps {
    fn drop(&mut self) {
        for bitmap in self.0.drain(..) {
            // SAFETY: bitmap は gdi_image::bitmap が作ったもので、使っていたメニューはもうない。
            if !unsafe { DeleteObject(bitmap.into()) }.as_bool() {
                tracing::warn!("failed to delete a menu item bitmap");
            }
        }
    }
}

/// `menu` の ID が `id` の項目に `image` を付ける。作ったビットマップは `bitmaps` が持つ。付けられなければ
/// 警告して、絵のない項目のままにする。
fn set_item_bitmap(menu: HMENU, id: usize, image: &Decoded, bitmaps: &mut ItemBitmaps) {
    let bitmap = match gdi_image::bitmap(image) {
        Ok(bitmap) => bitmap,
        Err(e) => {
            tracing::warn!("failed to create a menu item bitmap: {e}");
            return;
        }
    };
    bitmaps.0.push(bitmap);
    let item = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_BITMAP,
        hbmpItem: bitmap,
        ..Default::default()
    };
    // SAFETY: menu は有効なメニューで、item は cbSize を入れて初期化済み。
    if let Err(e) = unsafe { SetMenuItemInfoW(menu, id as u32, false, &item) } {
        tracing::warn!("failed to set a menu item bitmap: {e}");
    }
}

/// `request` のとおりにメニューを出し、選ばれたものを返す。
pub fn show(owner: HWND, request: &MenuRequest) -> Option<Chosen> {
    let mut bitmaps = ItemBitmaps::default();
    let menu = match build(request, &mut bitmaps) {
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
    // SAFETY: menu は作ったばかりのメニューで、owner は有効なウィンドウ。params はこの呼び出しの間は生きて
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
    destroy_menu(menu);
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

/// `request` のメニューを作る。作れなければ、作りかけのメニューは破棄する。
fn build(request: &MenuRequest, bitmaps: &mut ItemBitmaps) -> windows::core::Result<HMENU> {
    // SAFETY: 空のメニューを作るだけ。
    let menu = unsafe { CreatePopupMenu()? };
    let filled = if request.menu == Menu::Profiles {
        fill_profiles(menu, request, bitmaps)
    } else {
        fill_commands(menu, request)
    };
    if let Err(e) = filled {
        destroy_menu(menu);
        return Err(e);
    }
    Ok(menu)
}

/// `menu` を破棄する。付いているサブメニューも破棄される。
fn destroy_menu(menu: HMENU) {
    // SAFETY: menu は作ったメニューで、ここで一度だけ破棄する。
    if let Err(e) = unsafe { DestroyMenu(menu) } {
        tracing::warn!("failed to destroy the menu: {e}");
    }
}

/// `menu` にプロファイルの一覧の項目を並べる（[`profile_menu_entries`]）。
fn fill_profiles(
    menu: HMENU,
    request: &MenuRequest,
    bitmaps: &mut ItemBitmaps,
) -> windows::core::Result<()> {
    for entry in profile_menu_entries(request.profiles.len(), request.elevated) {
        match entry {
            Entry::Profile(index) => {
                append_profile(menu, request, index, PROFILE_ID_BASE + index, bitmaps)?;
            }
            Entry::Separator => append(menu, MF_SEPARATOR, 0, None)?,
            Entry::Elevated(indices) => append_elevated(menu, request, &indices, bitmaps)?,
        }
    }
    Ok(())
}

/// `menu` の末尾に、`request.profiles` の `index` 番目のプロファイルの項目を ID `id` で足す。
fn append_profile(
    menu: HMENU,
    request: &MenuRequest,
    index: usize,
    id: usize,
    bitmaps: &mut ItemBitmaps,
) -> windows::core::Result<()> {
    let flags = if index == request.default_profile {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    };
    let label = HSTRING::from(profile_label(index, &request.profiles[index]));
    append(menu, flags, id, Some(&label))?;
    if let Some(Some(icon)) = request.profile_icons.get(index) {
        set_item_bitmap(menu, id, icon, bitmaps);
    }
    Ok(())
}

/// `menu` の末尾に「管理者として開く」のサブメニューを足し、中に `indices` のプロファイルを並べる。
fn append_elevated(
    menu: HMENU,
    request: &MenuRequest,
    indices: &[usize],
    bitmaps: &mut ItemBitmaps,
) -> windows::core::Result<()> {
    // SAFETY: 空のメニューを作るだけ。
    let submenu = unsafe { CreatePopupMenu()? };
    let inserted = fill_elevated(submenu, request, indices, bitmaps)
        .and_then(|()| insert_submenu(menu, submenu, request.elevate_icon.as_ref(), bitmaps));
    if inserted.is_err() {
        // 親のメニューに付いていないので、親を破棄しても破棄されない。
        destroy_menu(submenu);
    }
    inserted
}

/// 「管理者として開く」のサブメニュー `submenu` に、`indices` のプロファイルを並べる。
fn fill_elevated(
    submenu: HMENU,
    request: &MenuRequest,
    indices: &[usize],
    bitmaps: &mut ItemBitmaps,
) -> windows::core::Result<()> {
    for &index in indices {
        let id = ELEVATED_ID_FLAG | (PROFILE_ID_BASE + index);
        append_profile(submenu, request, index, id, bitmaps)?;
    }
    Ok(())
}

/// `menu` の末尾に、`submenu` を開く「管理者として開く」の項目を足す。`icon` があれば付ける。サブメニューを開く
/// 項目は ID で引けないので、位置を指定して一度に足す。
fn insert_submenu(
    menu: HMENU,
    submenu: HMENU,
    icon: Option<&Decoded>,
    bitmaps: &mut ItemBitmaps,
) -> windows::core::Result<()> {
    let bitmap = icon.and_then(|icon| match gdi_image::bitmap(icon) {
        Ok(bitmap) => {
            bitmaps.0.push(bitmap);
            Some(bitmap)
        }
        Err(e) => {
            tracing::warn!("failed to create the bitmap of the elevate item: {e}");
            None
        }
    });
    // SAFETY: menu は有効なメニュー。
    let position = u32::try_from(unsafe { GetMenuItemCount(Some(menu)) })
        .map_err(|_| windows::core::Error::from_thread())?;
    let label = HSTRING::from(ELEVATE_LABEL);
    let mut item = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_SUBMENU | MIIM_STRING,
        hSubMenu: submenu,
        // InsertMenuItemW は文字を読むだけで書き換えない。
        dwTypeData: PWSTR(label.as_ptr().cast_mut()),
        ..Default::default()
    };
    if let Some(bitmap) = bitmap {
        item.fMask |= MIIM_BITMAP;
        item.hbmpItem = bitmap;
    }
    // SAFETY: menu と submenu は有効なメニューで、item は cbSize を入れて初期化済み。文字はこの呼び出しの間は
    // 生きている。
    unsafe { InsertMenuItemW(menu, position, true, &item) }
}

/// `menu` に、`request.menu` の項目を並べる。
fn fill_commands(menu: HMENU, request: &MenuRequest) -> windows::core::Result<()> {
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
    Ok(())
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

    fn all_commands() -> impl Iterator<Item = MenuCommand> {
        COMMANDS.iter().map(|(command, ..)| *command)
    }

    fn request(menu: Menu, can_copy: bool, can_open_settings: bool) -> MenuRequest {
        MenuRequest {
            keys: Keymap::default(),
            menu,
            x: 0,
            y: 0,
            exclude: None,
            can_copy,
            can_open_settings,
            profiles: Vec::new(),
            default_profile: 0,
            profile_icons: Vec::new(),
            elevated: false,
            elevate_icon: None,
        }
    }

    #[test]
    fn the_command_table_follows_the_declaration_order() {
        for (index, (command, ..)) in COMMANDS.iter().enumerate() {
            assert_eq!(*command as usize, index, "{command:?}");
        }
    }

    #[test]
    fn ids_round_trip_and_avoid_zero() {
        for command in all_commands() {
            assert_ne!(command.id(), 0);
            assert_eq!(MenuCommand::from_id(command.id()), Some(command));
        }
        assert_eq!(MenuCommand::from_id(0), None);
        assert_eq!(MenuCommand::from_id(COMMANDS.len() + 1), None);
        assert_eq!(Chosen::from_id(0), None);
        assert_eq!(
            Chosen::from_id(MenuCommand::Copy.id()),
            Some(Chosen::Command(MenuCommand::Copy))
        );
        assert_eq!(
            Chosen::from_id(PROFILE_ID_BASE + 2),
            Some(Chosen::Profile(2))
        );
        assert!(all_commands().all(|c| c.id() < PROFILE_ID_BASE));

        // 「管理者として開く」の中の項目は、フラグを立てた ID で、プロファイルの項目と重ならない。
        for index in [0, 2, MAX_MENU_PROFILES - 1] {
            let id = ELEVATED_ID_FLAG | (PROFILE_ID_BASE + index);
            assert_eq!(Chosen::from_id(id), Some(Chosen::ElevatedProfile(index)));
            assert!(id <= usize::from(u16::MAX), "{id:#x}");
            assert_eq!(
                Chosen::from_id(PROFILE_ID_BASE + index),
                Some(Chosen::Profile(index))
            );
        }
        assert_eq!(
            PROFILE_ID_BASE + MAX_MENU_PROFILES - 1,
            ELEVATED_ID_FLAG - 1
        );
        assert_eq!(Chosen::from_id(ELEVATED_ID_FLAG), None);
        assert_eq!(
            Chosen::from_id(ELEVATED_ID_FLAG | MenuCommand::Copy.id()),
            None
        );
    }

    #[test]
    fn the_profile_menu_ends_with_the_elevate_submenu_unless_elevated() {
        assert_eq!(
            profile_menu_entries(3, false),
            [
                Entry::Profile(0),
                Entry::Profile(1),
                Entry::Profile(2),
                Entry::Separator,
                Entry::Elevated(vec![0, 1, 2]),
            ]
        );
        assert_eq!(
            profile_menu_entries(3, true),
            [Entry::Profile(0), Entry::Profile(1), Entry::Profile(2)]
        );
        assert_eq!(
            profile_menu_entries(1, false),
            [
                Entry::Profile(0),
                Entry::Separator,
                Entry::Elevated(vec![0])
            ]
        );
        assert!(profile_menu_entries(0, false).is_empty());
        let many = profile_menu_entries(MAX_MENU_PROFILES + 1, false);
        assert_eq!(many.len(), MAX_MENU_PROFILES + 2);
        assert_eq!(
            many.last(),
            Some(&Entry::Elevated((0..MAX_MENU_PROFILES).collect()))
        );
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
        for command in all_commands() {
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

    #[test]
    fn menu_items_keep_their_order_and_separators() {
        use MenuCommand::*;
        assert_eq!(
            Menu::Context.items(),
            [
                Some(Copy),
                Some(Paste),
                None,
                Some(SelectAll),
                None,
                Some(NewTab),
                Some(CloseTab),
            ]
        );
        assert_eq!(
            Menu::Main.items(),
            [
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
            ]
        );
        assert!(Menu::Profiles.items().is_empty());
    }

    #[test]
    fn only_copy_and_the_settings_items_can_be_disabled() {
        for command in all_commands() {
            let all_off = request(Menu::Main, false, false);
            let all_on = request(Menu::Main, true, true);
            assert!(enabled(command, &all_on), "{command:?}");
            let expected = !matches!(
                command,
                MenuCommand::Copy | MenuCommand::EditSettings | MenuCommand::OpenSettings
            );
            assert_eq!(enabled(command, &all_off), expected, "{command:?}");
        }
        let no_selection = request(Menu::Main, false, true);
        assert!(!enabled(MenuCommand::Copy, &no_selection));
        assert!(enabled(MenuCommand::OpenSettings, &no_selection));
        let no_settings_file = request(Menu::Main, true, false);
        assert!(enabled(MenuCommand::Copy, &no_settings_file));
        assert!(!enabled(MenuCommand::EditSettings, &no_settings_file));
        assert!(!enabled(MenuCommand::OpenSettings, &no_settings_file));
    }
}
