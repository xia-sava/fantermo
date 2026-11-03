//! メニュー。コンテキストメニュー、タブバーの ≡ のメニュー、プロファイルの一覧を出す要求と、選ばれた項目の働き。

use std::path::Path;

use windows::Win32::Foundation::RECT;

use super::{App, Deferred, WithApp, tabs::TabCommand};
use crate::{
    context_menu::{Menu, MenuCommand, MenuRequest},
    link_opener, tab_bar,
    tab_icon::TabIcon,
};

impl App {
    /// タブバーの新しいタブのボタンとプロファイルの一覧のボタンの下に、ボタンを隠さずにプロファイルの
    /// 一覧を出すよう返す。
    pub(super) fn profile_menu(&mut self) -> Deferred {
        let (new_tab, profiles) = self.tab_bar_layout().new_tab_buttons();
        let side = tab_bar::tab_icon_size(self.dpi);
        let config_dir = self.config_path.as_deref().and_then(Path::parent);
        let mut renderer = self.renderer.as_mut();
        let profile_icons = self
            .profiles
            .iter()
            .zip(TabIcon::for_profiles(&self.profiles, config_dir))
            .map(|(profile, icon)| {
                // `Renderer` がないとき（作り直しを待つ間と、描画をやめたあと）は、アイコンを付けない。
                match renderer.as_mut()?.tab_icon_pixels(&icon, side) {
                    Ok(pixels) => Some(pixels),
                    Err(e) => {
                        tracing::warn!("failed to make the icon of {:?}: {e}", profile.name);
                        None
                    }
                }
            })
            .collect();
        let (left, top) = self.client_to_screen(new_tab.x as i32, new_tab.y as i32);
        let (right, bottom) =
            self.client_to_screen(profiles.right() as i32, profiles.bottom() as i32);
        Deferred::Menu(MenuRequest {
            profiles: self
                .profiles
                .iter()
                .map(|profile| profile.name.clone())
                .collect(),
            default_profile: self.default_profile_index(),
            profile_icons,
            exclude: Some(RECT {
                left,
                top,
                right,
                bottom,
            }),
            x: left,
            y: bottom,
            ..self.menu_request(Menu::Profiles)
        })
    }

    /// `menu` を出す要求。位置はウィンドウの左上で、隠さない矩形はない。
    fn menu_request(&self, menu: Menu) -> MenuRequest {
        MenuRequest {
            keys: self.keymap.clone(),
            menu,
            x: 0,
            y: 0,
            exclude: None,
            can_copy: self.has_selection(),
            can_open_settings: self.config_path.is_some(),
            profiles: Vec::new(),
            default_profile: 0,
            profile_icons: Vec::new(),
        }
    }

    /// ウィンドウのクライアント領域の中の (`x`, `y`) にコンテキストメニューを出すよう返す。
    pub(super) fn context_menu(&self, x: i32, y: i32) -> Deferred {
        let (screen_x, screen_y) = self.client_to_screen(x, y);
        Deferred::Menu(MenuRequest {
            x: screen_x,
            y: screen_y,
            ..self.menu_request(Menu::Context)
        })
    }

    /// タブバーのメニューのボタンの下に、ボタンを隠さずにメニューを出すよう返す。
    pub(super) fn main_menu(&self) -> Deferred {
        let button = self.tab_bar_layout().menu();
        let (left, top) = self.client_to_screen(button.x as i32, button.y as i32);
        let (right, bottom) = self.client_to_screen(button.right() as i32, button.bottom() as i32);
        Deferred::Menu(MenuRequest {
            x: left,
            y: bottom,
            exclude: Some(RECT {
                left,
                top,
                right,
                bottom,
            }),
            ..self.menu_request(Menu::Main)
        })
    }

    /// 設定ファイルを既定のアプリで開く。
    fn open_settings(&self) {
        match &self.config_path {
            Some(path) => link_opener::open_settings(path.clone()),
            None => tracing::warn!("the settings file location is unknown"),
        }
    }

    /// テキストカーソルのセルの左下にコンテキストメニューを出すよう返す。キーボードから開くときに使う。
    pub(super) fn context_menu_at_cursor(&self) -> Deferred {
        let (col, row) = self.session().vt.cursor_cell().unwrap_or_else(|e| {
            tracing::warn!("failed to read the cursor position: {e}");
            (0, 0)
        });
        let (x, y) = self.terminal_to_client(
            (u32::from(col) * self.font.cell_width_px) as i32,
            ((u32::from(row) + 1) * self.font.cell_height_px) as i32,
        );
        self.context_menu(x, y)
    }
}

/// メニューで選ばれた `command` をする。続けてすることがあれば返す。
pub(super) fn run_menu_command(command: MenuCommand, with_app: &WithApp) -> Option<Deferred> {
    let mut next = None;
    match command {
        MenuCommand::Copy => with_app(&mut |app| {
            app.copy_selection();
        }),
        MenuCommand::Paste => with_app(&mut |app| next = app.paste_clipboard()),
        MenuCommand::SelectAll => with_app(&mut |app| app.select_all()),
        MenuCommand::NewTab => with_app(&mut |app| next = app.run_tab_command(TabCommand::New)),
        MenuCommand::CloseTab => with_app(&mut |app| next = app.run_tab_command(TabCommand::Close)),
        MenuCommand::EditSettings => next = Some(Deferred::EditSettings),
        MenuCommand::OpenSettings => with_app(&mut |app| app.open_settings()),
        MenuCommand::ShowAbout => with_app(&mut |app| {
            next = Some(Deferred::ShowAbout {
                font: app.font.clone(),
            })
        }),
        MenuCommand::Exit => next = Some(Deferred::CloseWindow),
    }
    next
}
