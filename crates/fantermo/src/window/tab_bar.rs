//! タブバー。マウスを載せたとき、押して離したときの働き、描くもの。

use fantermo_core::{frame::Rgb, vt::MouseButton};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_BTNSHADOW, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT,
    GetSysColor, SYS_COLOR_INDEX,
};

use super::{
    App,
    proc::Deferred,
    tabs::{Launch, tab_title},
};
use crate::{
    paint::Paint,
    tab_bar::{self, Hit, TabView},
};

/// Windows のシステムの色。描くたびに問い合わせるので、システムの色が変わればそのまま従う。
pub(super) fn system_colors() -> tab_bar::SystemColors {
    let color = |index: SYS_COLOR_INDEX| {
        // SAFETY: 決まった番号の色を問い合わせるだけ。
        let colorref = unsafe { GetSysColor(index) };
        // COLORREF は下位のバイトから赤・緑・青。
        Rgb {
            r: (colorref & 0xff) as u8,
            g: ((colorref >> 8) & 0xff) as u8,
            b: ((colorref >> 16) & 0xff) as u8,
        }
    };
    tab_bar::SystemColors {
        face: color(COLOR_BTNFACE),
        text: color(COLOR_BTNTEXT),
        gray_text: color(COLOR_GRAYTEXT),
        shadow: color(COLOR_BTNSHADOW),
        window: color(COLOR_WINDOW),
        window_text: color(COLOR_WINDOWTEXT),
    }
}

impl App {
    /// タブバーの上の、マウスを載せているものを決め直す。変わったら描き直す。マウスの位置やタブの
    /// 並びが変わったときに使う。
    pub(super) fn update_tab_bar_hover(&mut self) {
        let hover = self
            .pointer_in_window
            .and_then(|(x, y)| self.tab_bar_hit(x, y))
            .flatten();
        if hover == self.tab_bar_hover {
            return;
        }
        tracing::debug!("tab bar under the pointer: {hover:?}");
        self.tab_bar_hover = hover;
        self.invalidate();
    }

    /// ウィンドウのクライアント領域の中の (`x`, `y`) がタブバーの上なら、そこにあるもの（タブの間や右の
    /// 空きなら `None`）を `Some` で返す。タブバーの外なら `None`。
    pub(super) fn tab_bar_hit(&self, x: i32, y: i32) -> Option<Option<Hit>> {
        self.tab_bar_area()
            .contains(x, y)
            .then(|| self.tab_bar_layout().hit(x, y))
    }

    /// タブバーの並べ方。
    pub(super) fn tab_bar_layout(&self) -> tab_bar::Layout {
        tab_bar::layout(
            tab_bar::Metrics { dpi: self.dpi },
            self.tab_bar_area().width,
            self.sessions.len(),
            self.active,
        )
    }

    /// タブバーの上で押した `button` を (`x`, `y`) で離した。押したときにマウスを載せていたものは
    /// `pressed`。
    /// 左ボタンは押したのと同じボタンの上で離したら、閉じるボタンならそのタブを閉じ、新しいタブのボタン
    /// なら新しいタブを開き、プロファイルの一覧のボタンなら一覧を出すよう返す（タブは押したときに
    /// 切り替えてある）。中ボタンを同じタブの上で離したらそのタブを閉じる。右ボタンは、新しいタブと
    /// プロファイルの一覧のボタンの上で押して離したら一覧を、ほかはコンテキストメニューを出すよう返す。
    /// 最後のタブを閉じたら、ウィンドウを閉じるよう返す。
    pub(super) fn on_tab_bar_click(
        &mut self,
        button: MouseButton,
        pressed: Option<Hit>,
        x: i32,
        y: i32,
    ) -> Option<Deferred> {
        let released = self.tab_bar_hit(x, y).flatten();
        match button {
            MouseButton::Left => {
                // 押している色を戻す。
                self.invalidate();
                if released != pressed {
                    return None;
                }
                match pressed? {
                    Hit::Close(index) => self.close_session(index).then_some(Deferred::CloseWindow),
                    Hit::NewTab => {
                        self.open_session(Launch::Default);
                        None
                    }
                    Hit::Profiles => Some(self.profile_menu()),
                    Hit::Menu => Some(self.main_menu()),
                    Hit::Tab(_) => None,
                }
            }
            MouseButton::Middle => {
                let index = pressed?.tab()?;
                if released.and_then(Hit::tab) != Some(index) {
                    return None;
                }
                self.close_session(index).then_some(Deferred::CloseWindow)
            }
            // 新しいタブのボタンとプロファイルの一覧のボタンの右クリックは、プロファイルの一覧。
            MouseButton::Right
                if matches!(pressed, Some(Hit::NewTab | Hit::Profiles)) && released == pressed =>
            {
                Some(self.profile_menu())
            }
            MouseButton::Right => Some(self.context_menu(x, y)),
            _ => None,
        }
    }

    /// タブバーに描くもの。色は Windows のシステムの色から決め、設定で書いた色を上書きする。
    pub(super) fn tab_bar_paints(&self) -> Vec<Paint> {
        let tabs: Vec<TabView> = self
            .sessions
            .iter()
            .map(|session| TabView {
                title: tab_title(session),
                bell: session.bell_marked,
                icon: &session.tab_icon,
            })
            .collect();
        let pressed = match self.tab_bar_press {
            Some((MouseButton::Left, hit)) => hit,
            _ => None,
        };
        tab_bar::paints(
            &self.tab_bar_layout(),
            &tabs,
            self.active,
            tab_bar::Highlight {
                hovered: self.tab_bar_hover,
                pressed,
            },
            &tab_bar::Palette::from_system(system_colors())
                .with_colors(&self.config.colors.tab_bar),
            &*self.ui_font,
        )
    }
}
