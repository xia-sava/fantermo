//! 設定ダイアログのキーのページ。キーの割り当ての一覧と、追加・変更・外す・既定に戻すのボタン。

use std::cell::RefCell;

use fantermo_core::{
    config::Config,
    keymap::{Action, KeyCombo, Keymap},
};
use windows::Win32::{
    Foundation::HWND,
    UI::{
        Input::KeyboardAndMouse::{EnableWindow, SetFocus},
        WindowsAndMessaging::{
            BN_CLICKED, IDYES, LBN_DBLCLK, LBN_SELCHANGE, MB_ICONQUESTION, MB_YESNO,
        },
    },
};

use super::{
    BUTTON_GAP, BUTTON_HEIGHT, BUTTON_TOP, DialogContext, MARGIN, PAGE_LEFT, PAGE_WIDTH, Page,
    ROW_HEIGHT, TITLE, row,
    widgets::{Builder, ListBox},
};
use crate::{dialog, key_capture};

/// このページのコントロールの ID の範囲。範囲がページの間で重ならないことと、このページの ID が範囲に
/// 入ることを単体のテストで見る。
#[cfg(test)]
pub(super) const ID_RANGE: std::ops::RangeInclusive<i32> = 500..=589;
const ID_KEYS: i32 = 500;
const ID_KEY_ADD: i32 = 501;
const ID_KEY_CHANGE: i32 = 502;
const ID_KEY_REMOVE: i32 = 503;
const ID_KEY_RESET: i32 = 504;

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
    "1 画面上へスクロール",
    "1 画面下へスクロール",
    "1 行上へスクロール",
    "1 行下へスクロール",
    "先頭へスクロール",
    "末尾へスクロール",
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

/// キーのページ。キーの割り当ての一覧と、追加・変更・外す・既定に戻すのボタン。
pub(super) struct KeysPage {
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
    pub(super) fn create(b: &mut Builder) -> windows::core::Result<Self> {
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
            // 借用は持っていない。
            let answer =
                dialog::message_box(Some(ctx.hwnd()), &text, TITLE, MB_YESNO | MB_ICONQUESTION);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ids_are_within_the_range_of_the_page() {
        for id in [
            ID_KEYS,
            ID_KEY_ADD,
            ID_KEY_CHANGE,
            ID_KEY_REMOVE,
            ID_KEY_RESET,
        ] {
            assert!(ID_RANGE.contains(&id), "{id}");
        }
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
                "1 画面上へスクロール\tCtrl+PageUp",
                "1 画面下へスクロール\t（なし）",
                "1 行上へスクロール\tCtrl+ArrowUp",
                "1 行下へスクロール\tCtrl+ArrowDown",
                "先頭へスクロール\tCtrl+Home",
                "末尾へスクロール\tCtrl+End",
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
}
