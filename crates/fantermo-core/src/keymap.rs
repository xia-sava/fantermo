//! fantermo が受け持つキーの割り当て。キーの組み合わせから動作を引く表と、組み合わせ・動作の名前の
//! 解釈を置く。
//!
//! 表が決めるのは「このキーはこの動作の候補」までで、動作が成り立つか（選んでいるときだけコピーする
//! など）と、押し続けたときの繰り返しの扱いは、動作を行う側が決める。成り立たなければ、キーはシェルへ
//! 送る。
//!
//! 名前は設定ファイルに書く形を想定している。組み合わせは `ctrl+shift+t` のように修飾キーとキーを `+`
//! でつなぎ、動作は `new_tab` のような snake_case。割り当てを外す値は [`UNBOUND`]。

use std::fmt;

use crate::vt::{Key, Mods};

/// 割り当てを外す値。動作の名前の代わりに書く。
pub const UNBOUND: &str = "none";

/// 割り当てられる動作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// 選んでいればコピーして選択を外す。選んでいなければ成り立たない。
    Copy,
    /// クリップボードの文字列を貼り付ける。
    Paste,
    /// テキストカーソルの位置にコンテキストメニューを出す。
    ContextMenu,
    /// 新しいタブを開いて表示する。
    NewTab,
    /// 表示しているタブを閉じる。
    CloseTab,
    /// 右隣のタブを表示する。
    NextTab,
    /// 左隣のタブを表示する。
    PrevTab,
}

/// 動作と名前。
const ACTION_NAMES: [(Action, &str); 7] = [
    (Action::Copy, "copy"),
    (Action::Paste, "paste"),
    (Action::ContextMenu, "context_menu"),
    (Action::NewTab, "new_tab"),
    (Action::CloseTab, "close_tab"),
    (Action::NextTab, "next_tab"),
    (Action::PrevTab, "prev_tab"),
];

impl Action {
    /// すべての動作。設定ファイルの説明と設定ダイアログの一覧の順。
    pub const ALL: [Action; 7] = [
        Action::Copy,
        Action::Paste,
        Action::ContextMenu,
        Action::NewTab,
        Action::CloseTab,
        Action::NextTab,
        Action::PrevTab,
    ];

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        ACTION_NAMES
            .iter()
            .find(|(action, _)| *action == self)
            .map(|(_, name)| *name)
            .expect("every action has a name")
    }

    /// 名前 `name` の動作。大文字小文字は区別しない。
    pub fn from_name(name: &str) -> Option<Self> {
        ACTION_NAMES
            .iter()
            .find(|(_, candidate)| candidate.eq_ignore_ascii_case(name))
            .map(|(action, _)| *action)
    }
}

/// 割り当ての値を解釈する。動作の名前なら `Some`、[`UNBOUND`] なら `None`。
pub fn parse_action(value: &str) -> Result<Option<Action>, String> {
    let value = value.trim();
    if value.eq_ignore_ascii_case(UNBOUND) {
        return Ok(None);
    }
    Action::from_name(value)
        .map(Some)
        .ok_or_else(|| format!("unknown action `{value}`"))
}

/// 割り当てで区別する修飾キー。ロックの状態と左右の別は区別しない。
pub const HELD_MODS: Mods = Mods::CTRL
    .union(Mods::SHIFT)
    .union(Mods::ALT)
    .union(Mods::SUPER);

/// 押されている修飾キーだけ。ロックの状態と左右の別は除く。
pub fn held_mods(mods: Mods) -> Mods {
    mods & HELD_MODS
}

/// 修飾キーと名前。組み合わせの文字列ではこの順に並べる。
const MOD_NAMES: [(Mods, &str); 4] = [
    (Mods::CTRL, "ctrl"),
    (Mods::SHIFT, "shift"),
    (Mods::ALT, "alt"),
    (Mods::SUPER, "win"),
];

/// 割り当てに使えるキーと名前。名前は libghostty-vt のキー（物理的な位置）の名前を snake_case に
/// したもの。修飾キーそのものとメディアのキーなどは含めない。
const KEY_NAMES: &[(Key, &str)] = &[
    (Key::A, "a"),
    (Key::B, "b"),
    (Key::C, "c"),
    (Key::D, "d"),
    (Key::E, "e"),
    (Key::F, "f"),
    (Key::G, "g"),
    (Key::H, "h"),
    (Key::I, "i"),
    (Key::J, "j"),
    (Key::K, "k"),
    (Key::L, "l"),
    (Key::M, "m"),
    (Key::N, "n"),
    (Key::O, "o"),
    (Key::P, "p"),
    (Key::Q, "q"),
    (Key::R, "r"),
    (Key::S, "s"),
    (Key::T, "t"),
    (Key::U, "u"),
    (Key::V, "v"),
    (Key::W, "w"),
    (Key::X, "x"),
    (Key::Y, "y"),
    (Key::Z, "z"),
    (Key::Digit0, "digit_0"),
    (Key::Digit1, "digit_1"),
    (Key::Digit2, "digit_2"),
    (Key::Digit3, "digit_3"),
    (Key::Digit4, "digit_4"),
    (Key::Digit5, "digit_5"),
    (Key::Digit6, "digit_6"),
    (Key::Digit7, "digit_7"),
    (Key::Digit8, "digit_8"),
    (Key::Digit9, "digit_9"),
    (Key::Backquote, "backquote"),
    (Key::Backslash, "backslash"),
    (Key::BracketLeft, "bracket_left"),
    (Key::BracketRight, "bracket_right"),
    (Key::Comma, "comma"),
    (Key::Equal, "equal"),
    (Key::Minus, "minus"),
    (Key::Period, "period"),
    (Key::Quote, "quote"),
    (Key::Semicolon, "semicolon"),
    (Key::Slash, "slash"),
    (Key::IntlBackslash, "intl_backslash"),
    (Key::IntlRo, "intl_ro"),
    (Key::IntlYen, "intl_yen"),
    (Key::Backspace, "backspace"),
    (Key::Enter, "enter"),
    (Key::Space, "space"),
    (Key::Tab, "tab"),
    (Key::Escape, "escape"),
    (Key::ContextMenu, "context_menu"),
    (Key::Convert, "convert"),
    (Key::NonConvert, "non_convert"),
    (Key::KanaMode, "kana_mode"),
    (Key::Insert, "insert"),
    (Key::Delete, "delete"),
    (Key::Home, "home"),
    (Key::End, "end"),
    (Key::PageUp, "page_up"),
    (Key::PageDown, "page_down"),
    (Key::ArrowUp, "arrow_up"),
    (Key::ArrowDown, "arrow_down"),
    (Key::ArrowLeft, "arrow_left"),
    (Key::ArrowRight, "arrow_right"),
    (Key::PrintScreen, "print_screen"),
    (Key::ScrollLock, "scroll_lock"),
    (Key::Pause, "pause"),
    (Key::Numpad0, "numpad_0"),
    (Key::Numpad1, "numpad_1"),
    (Key::Numpad2, "numpad_2"),
    (Key::Numpad3, "numpad_3"),
    (Key::Numpad4, "numpad_4"),
    (Key::Numpad5, "numpad_5"),
    (Key::Numpad6, "numpad_6"),
    (Key::Numpad7, "numpad_7"),
    (Key::Numpad8, "numpad_8"),
    (Key::Numpad9, "numpad_9"),
    (Key::NumpadAdd, "numpad_add"),
    (Key::NumpadSubtract, "numpad_subtract"),
    (Key::NumpadMultiply, "numpad_multiply"),
    (Key::NumpadDivide, "numpad_divide"),
    (Key::NumpadDecimal, "numpad_decimal"),
    (Key::NumpadEnter, "numpad_enter"),
    (Key::F1, "f1"),
    (Key::F2, "f2"),
    (Key::F3, "f3"),
    (Key::F4, "f4"),
    (Key::F5, "f5"),
    (Key::F6, "f6"),
    (Key::F7, "f7"),
    (Key::F8, "f8"),
    (Key::F9, "f9"),
    (Key::F10, "f10"),
    (Key::F11, "f11"),
    (Key::F12, "f12"),
    (Key::F13, "f13"),
    (Key::F14, "f14"),
    (Key::F15, "f15"),
    (Key::F16, "f16"),
    (Key::F17, "f17"),
    (Key::F18, "f18"),
    (Key::F19, "f19"),
    (Key::F20, "f20"),
    (Key::F21, "f21"),
    (Key::F22, "f22"),
    (Key::F23, "f23"),
    (Key::F24, "f24"),
];

/// 修飾キーとキーの組み合わせ一つ。修飾キーは [`HELD_MODS`] のものだけを持つ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyCombo {
    pub mods: Mods,
    pub key: Key,
}

impl KeyCombo {
    pub const fn new(mods: Mods, key: Key) -> Self {
        Self {
            mods: mods.intersection(HELD_MODS),
            key,
        }
    }

    /// `ctrl+shift+t` のような文字列を解釈する。大文字小文字と `+` の前後の空白は区別しない。
    /// 修飾キーは `ctrl`、`shift`、`alt`、`win` で、どの順に書いてもよい。キーは最後に一つだけ書く。
    pub fn parse(text: &str) -> Result<Self, String> {
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let (key_name, mod_names) = parts.split_last().expect("split yields at least one part");
        let mut mods = Mods::empty();
        for name in mod_names {
            let Some((flag, _)) = MOD_NAMES
                .iter()
                .find(|(_, candidate)| candidate.eq_ignore_ascii_case(name))
            else {
                return Err(format!("unknown modifier `{name}` in `{text}`"));
            };
            if mods.contains(*flag) {
                return Err(format!("modifier `{name}` is repeated in `{text}`"));
            }
            mods |= *flag;
        }
        let Some((key, _)) = KEY_NAMES
            .iter()
            .find(|(_, candidate)| candidate.eq_ignore_ascii_case(key_name))
        else {
            return Err(format!("unknown key `{key_name}` in `{text}`"));
        };
        Ok(Self::new(mods, *key))
    }

    /// キーが名前を持つ（設定ファイルに書ける）か。
    pub fn is_nameable(&self) -> bool {
        key_name(self.key).is_some()
    }

    /// メニューやダイアログに見せる形。`Ctrl+Shift+PageUp` のように、修飾キーとキーの名前の語の頭を
    /// 大文字にし、キーの名前の `_` を除く。
    pub fn label(&self) -> String {
        let mut label = String::new();
        for (flag, name) in MOD_NAMES {
            if self.mods.contains(flag) {
                label.push_str(&capitalize(name));
                label.push('+');
            }
        }
        match key_name(self.key) {
            Some(name) => label.extend(name.split('_').map(capitalize)),
            None => label.push_str(&format!("{:?}", self.key)),
        }
        label
    }
}

/// キー `key` の名前。名前を持たないキーなら `None`。
fn key_name(key: Key) -> Option<&'static str> {
    KEY_NAMES
        .iter()
        .find(|(candidate, _)| *candidate == key)
        .map(|(_, name)| *name)
}

/// 語の頭の文字を大文字にする。
fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, name) in MOD_NAMES {
            if self.mods.contains(flag) {
                write!(f, "{name}+")?;
            }
        }
        match key_name(self.key) {
            Some(name) => f.write_str(name),
            None => write!(f, "{:?}", self.key),
        }
    }
}

/// 既定の割り当て。
const DEFAULT_BINDINGS: [(KeyCombo, Action); 9] = {
    const CTRL_SHIFT: Mods = Mods::CTRL.union(Mods::SHIFT);
    [
        (KeyCombo::new(Mods::CTRL, Key::C), Action::Copy),
        (KeyCombo::new(Mods::CTRL, Key::Insert), Action::Copy),
        (KeyCombo::new(Mods::SHIFT, Key::Insert), Action::Paste),
        (
            KeyCombo::new(Mods::empty(), Key::ContextMenu),
            Action::ContextMenu,
        ),
        (KeyCombo::new(Mods::SHIFT, Key::F10), Action::ContextMenu),
        (KeyCombo::new(CTRL_SHIFT, Key::T), Action::NewTab),
        (KeyCombo::new(CTRL_SHIFT, Key::W), Action::CloseTab),
        (KeyCombo::new(Mods::CTRL, Key::Tab), Action::NextTab),
        (KeyCombo::new(CTRL_SHIFT, Key::Tab), Action::PrevTab),
    ]
};

/// 組み合わせ `combo` の既定の動作。既定で割り当てていなければ `None`。
pub fn default_action(combo: KeyCombo) -> Option<Action> {
    DEFAULT_BINDINGS
        .iter()
        .find(|(bound, _)| *bound == combo)
        .map(|(_, action)| *action)
}

/// 既定の割り当ての組み合わせ。既定の表の順。
pub fn default_combos() -> impl Iterator<Item = KeyCombo> {
    DEFAULT_BINDINGS.iter().map(|(combo, _)| *combo)
}

/// キーの組み合わせから動作を引く表。組み合わせ一つに動作は一つで、並びの順は比べない。
#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: Vec<(KeyCombo, Action)>,
}

impl PartialEq for Keymap {
    fn eq(&self, other: &Self) -> bool {
        self.bindings.len() == other.bindings.len()
            && self
                .bindings
                .iter()
                .all(|(combo, action)| other.lookup_combo(*combo) == Some(*action))
    }
}

impl Eq for Keymap {}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: DEFAULT_BINDINGS.to_vec(),
        }
    }
}

impl Keymap {
    /// キー `key` を修飾キー `mods` とともに押したときの動作の候補。ロックの状態と左右の別は見ない。
    /// ほかの修飾キーを足した組み合わせは別の組み合わせとして扱う。
    pub fn lookup(&self, key: Key, mods: Mods) -> Option<Action> {
        self.lookup_combo(KeyCombo::new(mods, key))
    }

    /// 組み合わせ `combo` に割り当てた動作。
    pub fn lookup_combo(&self, combo: KeyCombo) -> Option<Action> {
        self.bindings
            .iter()
            .find(|(bound, _)| *bound == combo)
            .map(|(_, action)| *action)
    }

    /// 動作 `action` に割り当てた組み合わせ。割り当てた順。
    pub fn combos_of(&self, action: Action) -> Vec<KeyCombo> {
        self.bindings
            .iter()
            .filter(|(_, bound)| *bound == action)
            .map(|(combo, _)| *combo)
            .collect()
    }

    /// 動作 `action` の割り当てを既定に戻す。ほかの組み合わせからは外し、既定の組み合わせは（ほかの
    /// 動作に割り当て直していても）この動作に戻す。
    pub fn reset_action(&mut self, action: Action) {
        self.bindings.retain(|(_, bound)| *bound != action);
        for (combo, default) in DEFAULT_BINDINGS {
            if default == action {
                self.bind(combo, Some(action));
            }
        }
    }

    /// `combo` の割り当てを `action` にする。`None` なら外す。
    pub fn bind(&mut self, combo: KeyCombo, action: Option<Action>) {
        self.bindings.retain(|(bound, _)| *bound != combo);
        if let Some(action) = action {
            self.bindings.push((combo, action));
        }
    }

    /// 割り当ての一覧。
    pub fn bindings(&self) -> &[(KeyCombo, Action)] {
        &self.bindings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL_SHIFT: Mods = Mods::CTRL.union(Mods::SHIFT);

    #[test]
    fn copy_is_ctrl_c_and_ctrl_insert_alone() {
        let keymap = Keymap::default();
        assert_eq!(keymap.lookup(Key::C, Mods::CTRL), Some(Action::Copy));
        assert_eq!(keymap.lookup(Key::Insert, Mods::CTRL), Some(Action::Copy));
        // ロックの状態と左右の別は関係しない。
        assert_eq!(
            keymap.lookup(Key::C, Mods::CTRL | Mods::CAPS_LOCK),
            Some(Action::Copy)
        );
        assert_eq!(
            keymap.lookup(Key::C, Mods::CTRL | Mods::CTRL_SIDE),
            Some(Action::Copy)
        );
        assert_eq!(keymap.lookup(Key::C, CTRL_SHIFT), None);
        assert_eq!(keymap.lookup(Key::C, Mods::empty()), None);
        assert_eq!(keymap.lookup(Key::V, Mods::CTRL), None);
    }

    #[test]
    fn paste_is_shift_insert_alone() {
        let keymap = Keymap::default();
        assert_eq!(keymap.lookup(Key::Insert, Mods::SHIFT), Some(Action::Paste));
        assert_eq!(
            keymap.lookup(Key::Insert, Mods::SHIFT | Mods::NUM_LOCK),
            Some(Action::Paste)
        );
        assert_eq!(keymap.lookup(Key::Insert, CTRL_SHIFT), None);
        // Ctrl+V はシェルへ送る。
        assert_eq!(keymap.lookup(Key::V, Mods::CTRL), None);
    }

    #[test]
    fn context_menu_is_menu_key_and_shift_f10() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.lookup(Key::ContextMenu, Mods::empty()),
            Some(Action::ContextMenu)
        );
        assert_eq!(
            keymap.lookup(Key::ContextMenu, Mods::NUM_LOCK),
            Some(Action::ContextMenu)
        );
        assert_eq!(
            keymap.lookup(Key::F10, Mods::SHIFT),
            Some(Action::ContextMenu)
        );
        assert_eq!(keymap.lookup(Key::F10, Mods::empty()), None);
        assert_eq!(keymap.lookup(Key::F10, CTRL_SHIFT), None);
        assert_eq!(keymap.lookup(Key::ContextMenu, Mods::CTRL), None);
    }

    #[test]
    fn tab_keys_need_ctrl_and_shift_as_listed() {
        let keymap = Keymap::default();
        assert_eq!(keymap.lookup(Key::T, CTRL_SHIFT), Some(Action::NewTab));
        assert_eq!(
            keymap.lookup(Key::T, CTRL_SHIFT | Mods::CAPS_LOCK),
            Some(Action::NewTab)
        );
        assert_eq!(keymap.lookup(Key::W, CTRL_SHIFT), Some(Action::CloseTab));
        assert_eq!(keymap.lookup(Key::Tab, Mods::CTRL), Some(Action::NextTab));
        assert_eq!(keymap.lookup(Key::Tab, CTRL_SHIFT), Some(Action::PrevTab));
        // Ctrl+T・Ctrl+W はシェルのキー（readline の入れ替え・単語の削除）として送る。
        assert_eq!(keymap.lookup(Key::T, Mods::CTRL), None);
        assert_eq!(keymap.lookup(Key::W, Mods::CTRL), None);
        assert_eq!(keymap.lookup(Key::Tab, Mods::empty()), None);
        assert_eq!(keymap.lookup(Key::Tab, Mods::CTRL | Mods::ALT), None);
        assert_eq!(keymap.lookup(Key::T, CTRL_SHIFT | Mods::ALT), None);
    }

    #[test]
    fn default_bindings_are_written_as_listed() {
        let written: Vec<(String, &str)> = Keymap::default()
            .bindings()
            .iter()
            .map(|(combo, action)| (combo.to_string(), action.name()))
            .collect();
        let expected = [
            ("ctrl+c", "copy"),
            ("ctrl+insert", "copy"),
            ("shift+insert", "paste"),
            ("context_menu", "context_menu"),
            ("shift+f10", "context_menu"),
            ("ctrl+shift+t", "new_tab"),
            ("ctrl+shift+w", "close_tab"),
            ("ctrl+tab", "next_tab"),
            ("ctrl+shift+tab", "prev_tab"),
        ];
        assert_eq!(
            written,
            expected.map(|(combo, action)| (combo.to_owned(), action))
        );
        // 書いた形を読み直すと同じ割り当てになる。
        for (combo, action) in Keymap::default().bindings() {
            assert_eq!(KeyCombo::parse(&combo.to_string()), Ok(*combo));
            assert_eq!(parse_action(action.name()), Ok(Some(*action)));
        }
    }

    #[test]
    fn combos_ignore_case_order_and_spaces() {
        let expected = KeyCombo::new(CTRL_SHIFT, Key::T);
        assert_eq!(KeyCombo::parse("ctrl+shift+t"), Ok(expected));
        assert_eq!(KeyCombo::parse("Shift + Ctrl + T"), Ok(expected));
        assert_eq!(
            KeyCombo::parse("win+alt+page_up"),
            Ok(KeyCombo::new(Mods::SUPER | Mods::ALT, Key::PageUp))
        );
        assert_eq!(
            KeyCombo::parse("f10"),
            Ok(KeyCombo::new(Mods::empty(), Key::F10))
        );
        assert_eq!(
            KeyCombo::parse("ctrl+digit_1"),
            Ok(KeyCombo::new(Mods::CTRL, Key::Digit1))
        );
    }

    #[test]
    fn malformed_combos_are_rejected() {
        for text in [
            "",
            "ctrl+",
            "ctrl",
            "ctrl+shift+ctrl+t",
            "ctrl+t+shift",
            "meta+t",
            "ctrl+shift_left",
            "ctrl+tt",
        ] {
            assert!(KeyCombo::parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn every_key_name_reads_back() {
        for (key, name) in KEY_NAMES {
            let combo = KeyCombo::parse(name).unwrap();
            assert_eq!(combo.key, *key);
            assert_eq!(combo.to_string(), *name);
        }
    }

    #[test]
    fn actions_are_named_in_snake_case_and_none_unbinds() {
        assert_eq!(parse_action("new_tab"), Ok(Some(Action::NewTab)));
        assert_eq!(parse_action("Prev_Tab"), Ok(Some(Action::PrevTab)));
        assert_eq!(parse_action("none"), Ok(None));
        assert!(parse_action("newtab").is_err());
        assert!(parse_action("").is_err());
        for (action, name) in ACTION_NAMES {
            assert_eq!(action.name(), name);
            assert_eq!(Action::from_name(name), Some(action));
        }
    }

    #[test]
    fn binding_replaces_or_removes_a_combo() {
        let mut keymap = Keymap::default();
        let ctrl_tab = KeyCombo::new(Mods::CTRL, Key::Tab);
        keymap.bind(ctrl_tab, None);
        assert_eq!(keymap.lookup(Key::Tab, Mods::CTRL), None);
        keymap.bind(
            KeyCombo::new(Mods::CTRL, Key::PageDown),
            Some(Action::NextTab),
        );
        assert_eq!(
            keymap.lookup(Key::PageDown, Mods::CTRL),
            Some(Action::NextTab)
        );
        // 割り当て直すと前の動作に代わる。
        keymap.bind(KeyCombo::new(Mods::CTRL, Key::C), Some(Action::Paste));
        assert_eq!(keymap.lookup(Key::C, Mods::CTRL), Some(Action::Paste));
        // ほかの割り当てはそのまま。
        assert_eq!(keymap.lookup(Key::Tab, CTRL_SHIFT), Some(Action::PrevTab));
    }

    #[test]
    fn keymaps_compare_regardless_of_order() {
        let mut a = Keymap::default();
        a.bind(KeyCombo::new(Mods::CTRL, Key::Tab), None);
        a.bind(KeyCombo::new(Mods::CTRL, Key::Tab), Some(Action::NextTab));
        assert_eq!(a, Keymap::default());
        a.bind(
            KeyCombo::new(Mods::CTRL, Key::PageDown),
            Some(Action::NextTab),
        );
        assert_ne!(a, Keymap::default());
    }

    #[test]
    fn resetting_an_action_restores_its_default_keys() {
        let mut keymap = Keymap::default();
        let ctrl_tab = KeyCombo::new(Mods::CTRL, Key::Tab);
        let ctrl_page_down = KeyCombo::new(Mods::CTRL, Key::PageDown);
        keymap.bind(ctrl_tab, Some(Action::NewTab));
        keymap.bind(ctrl_page_down, Some(Action::NextTab));
        assert_eq!(keymap.combos_of(Action::NextTab), [ctrl_page_down]);
        keymap.reset_action(Action::NextTab);
        assert_eq!(keymap.combos_of(Action::NextTab), [ctrl_tab]);
        assert_eq!(keymap.lookup_combo(ctrl_page_down), None);
        // 既定の組み合わせを取り戻された動作には、ほかの組み合わせが残る。
        keymap.reset_action(Action::NewTab);
        assert_eq!(keymap, Keymap::default());
        assert_eq!(default_action(ctrl_tab), Some(Action::NextTab));
        assert_eq!(default_action(ctrl_page_down), None);
    }

    #[test]
    fn labels_capitalize_each_word() {
        let label = |text: &str| KeyCombo::parse(text).unwrap().label();
        assert_eq!(label("shift+ctrl+t"), "Ctrl+Shift+T");
        assert_eq!(label("ctrl+page_up"), "Ctrl+PageUp");
        assert_eq!(label("win+alt+f10"), "Alt+Win+F10");
        assert_eq!(label("context_menu"), "ContextMenu");
        assert!(KeyCombo::new(Mods::CTRL, Key::T).is_nameable());
        assert!(!KeyCombo::new(Mods::CTRL, Key::ShiftLeft).is_nameable());
    }

    #[test]
    fn every_action_is_listed_once() {
        assert_eq!(Action::ALL.len(), ACTION_NAMES.len());
        for (action, (named, _)) in Action::ALL.iter().zip(ACTION_NAMES) {
            assert_eq!(*action, named);
        }
    }
}
