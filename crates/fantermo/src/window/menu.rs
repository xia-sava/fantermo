//! メニュー。コンテキストメニュー、タブバーの ≡ のメニュー、プロファイルの一覧を出す要求と、選ばれた項目の働き。

use std::path::Path;

use windows::Win32::{
    Foundation::{HWND, RECT},
    UI::Shell::SIID_SHIELD,
};

use super::{
    App,
    proc::{Deferred, WithApp},
    tabs::TabCommand,
};
use crate::{
    command_line::{Options, TabRequest},
    context_menu::{Menu, MenuCommand, MenuRequest},
    elevate::{self, LaunchError},
    link_opener,
    notice::Topic,
    tab_bar,
    tab_icon::TabIcon,
};

impl App {
    /// タブバーの新しいタブのボタンとプロファイルの一覧のボタンの下に、ボタンを隠さずにプロファイルの
    /// 一覧を出すよう返す。
    pub(super) fn profile_menu(&mut self) -> Deferred {
        let (new_tab, profiles) = self.tab_bar_layout().new_tab_buttons();
        let side = tab_bar::tab_icon_size(self.dpi);
        let config_dir = self.config_path.as_deref().and_then(Path::parent);
        let generator = self.tab_icon_generator.as_ref();
        let family = &self.ui_font.family;
        let profile_icons = self
            .config
            .profiles
            .iter()
            .zip(TabIcon::for_profiles(&self.config.profiles, config_dir))
            .map(
                |(profile, icon)| match generator?.pixels(&icon, side, family) {
                    Ok(pixels) => Some(pixels),
                    Err(e) => {
                        tracing::warn!("failed to make the icon of {:?}: {e}", profile.name);
                        None
                    }
                },
            )
            .collect();
        let elevate_icon = if self.elevated {
            None
        } else {
            generator.and_then(|generator| {
                generator
                    .stock_icon(SIID_SHIELD, side)
                    .inspect_err(|e| {
                        tracing::warn!("failed to make the icon of the elevate item: {e}")
                    })
                    .ok()
            })
        };
        let (left, top) = self.client_to_screen(new_tab.x as i32, new_tab.y as i32);
        let (right, bottom) =
            self.client_to_screen(profiles.right() as i32, profiles.bottom() as i32);
        Deferred::Menu(MenuRequest {
            profiles: self
                .config
                .profiles
                .iter()
                .map(|profile| profile.name.clone())
                .collect(),
            default_profile: self.default_profile_index(),
            profile_icons,
            elevate_icon,
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
            keys: self.config.keys.clone(),
            menu,
            x: 0,
            y: 0,
            exclude: None,
            can_copy: self.has_selection(),
            can_open_settings: self.config_path.is_some(),
            profiles: Vec::new(),
            default_profile: 0,
            profile_icons: Vec::new(),
            elevated: self.elevated,
            elevate_icon: None,
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
        let cursor = self
            .cursor_cell_rect()
            .unwrap_or_else(|| self.cell_rect(0, 0));
        self.context_menu(cursor.x, cursor.y + cursor.height)
    }
}

/// プロファイルの一覧の「管理者として開く」で選ばれた、名前が `name` のプロファイルを管理者として開く。名前は
/// 利用者が見た一覧（メニューを作ったときの設定）のもの。UAC の確認が閉じるまで戻らないので、借用の外で呼ぶ。
/// 起動できなければ、警告を見せるよう返す。
///
/// 一覧を開いている間に設定が読み直されて、その名前のプロファイルがいまの設定になければ、何もしない。選んでいない
/// プロファイルで UAC の確認を出さないため。UAC の確認で断られたときも、利用者が自分で断ったので何も見せない。
pub(super) fn open_elevated(hwnd: HWND, name: &str, with_app: &WithApp) -> Option<Deferred> {
    let mut configured = false;
    with_app(&mut |app| configured = app.config.profiles.iter().any(|p| p.name == name));
    if !configured {
        tracing::debug!(
            "not opening {name:?} as administrator: the profile is gone or the window is unavailable"
        );
        return None;
    }
    let options = Options {
        tab: TabRequest {
            profile: Some(name.to_owned()),
            command: None,
        },
        ..Options::default()
    };
    let launched =
        elevate::launch_request(&options).and_then(|request| elevate::launch(Some(hwnd), &request));
    match launched {
        Ok(()) => None,
        Err(LaunchError::Cancelled) => {
            tracing::debug!("opening {name:?} as administrator was cancelled or refused");
            None
        }
        Err(LaunchError::Failed(e)) => {
            tracing::warn!("failed to open {name:?} as administrator: {e}");
            Some(Deferred::Warn {
                topic: Topic::Elevate,
                text: elevate::failure_message(&e),
            })
        }
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
