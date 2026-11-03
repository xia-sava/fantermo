//! キーと IME の入力。キーの割り当ての動作と、端末へ送る入力。

use fantermo_core::{
    keymap,
    vt::{Key, KeyAction, KeyInput, Mods},
};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        MSG, PM_REMOVE, PeekMessageW, WM_CHAR, WM_DEADCHAR, WM_SYSCHAR, WM_SYSDEADCHAR,
    },
};

use super::{App, Deferred, mouse::is_link_modifier, tabs::TabCommand};
use crate::{
    ime::{self, CursorRect},
    keyboard::{self, KeyPosition, Utf16Assembler},
};

impl App {
    /// キーを押した・離した。`chars` は同じキーから TranslateMessage が作った文字。
    pub(super) fn on_key(
        &mut self,
        virtual_key: u16,
        position: KeyPosition,
        action: KeyAction,
        chars: &[char],
    ) -> Option<Deferred> {
        self.on_key_input(
            keyboard::key_from_position(position),
            keyboard::current_mods(),
            keyboard::unshifted_codepoint(virtual_key),
            action,
            chars,
        )
    }

    /// キー `key` を修飾キー `mods` とともに押した・離した。`unshifted` はそのキーが修飾なしで打つ文字の
    /// コードポイント。`chars` は同じキーから TranslateMessage が作った文字。
    pub(super) fn on_key_input(
        &mut self,
        key: Key,
        mods: Mods,
        unshifted: char,
        action: KeyAction,
        chars: &[char],
    ) -> Option<Deferred> {
        // Ctrl を押した・離したときは、マウスが動かなくてもリンクの下線を出し入れする。押し続けたときの
        // 繰り返しでは、状態が変わらないので探し直さない。
        if is_modifier_key(key) && action != KeyAction::Repeat {
            self.refresh_hovered_link(is_link_modifier(mods));
        }
        // メニューやダイアログが離した側を受け取ると、止める印が残る。押し直したなら印は古い。
        if action == KeyAction::Press && self.suppressed_release == Some(key) {
            self.suppressed_release = None;
        }
        // 割り当ての動作に使ったキーは、離したときもシェルに送らない。
        if action == KeyAction::Release && self.suppressed_release == Some(key) {
            self.suppressed_release = None;
            return None;
        }
        if action != KeyAction::Release
            && let Some(bound) = self.keymap.lookup(key, mods)
            && let Some(deferred) = self.run_bound_action(bound, action)
        {
            self.suppressed_release = Some(key);
            return deferred;
        }

        let text = keyboard::printable_text(chars);
        let input = KeyInput {
            key,
            action,
            mods,
            consumed_mods: keyboard::consumed_mods(mods, text.is_some()),
            unshifted,
            text: text.as_deref(),
        };
        match self.session_mut().vt.encode_key(&input) {
            Ok(encoded) => self.send(&encoded),
            Err(e) => tracing::warn!("failed to encode {:?}: {e}", input.key),
        }
        None
    }

    /// キーに割り当てた動作 `bound` を、押した・押し続けた（`action`）ときに行う。動作が成り立たず、
    /// キーをシェルへ送るときは `None`。
    fn run_bound_action(
        &mut self,
        bound: keymap::Action,
        action: KeyAction,
    ) -> Option<Option<Deferred>> {
        let repeated = action == KeyAction::Repeat;
        match bound {
            // 選んでいなければシェルへ送る。押し続けたときも同じ。
            keymap::Action::Copy => self.copy_selection().then_some(None),
            // ここから下は、押し続けたときの繰り返しもシェルへ送らない。貼り付け・メニュー・開く・
            // 閉じるは繰り返さず、次・前は繰り返す。
            keymap::Action::Paste => Some((!repeated).then(|| self.paste_clipboard()).flatten()),
            keymap::Action::ContextMenu => Some((!repeated).then(|| self.context_menu_at_cursor())),
            keymap::Action::NewTab => Some(
                (!repeated)
                    .then(|| self.run_tab_command(TabCommand::New))
                    .flatten(),
            ),
            keymap::Action::CloseTab => Some(
                (!repeated)
                    .then(|| self.run_tab_command(TabCommand::Close))
                    .flatten(),
            ),
            keymap::Action::NextTab => Some(self.run_tab_command(TabCommand::Next)),
            keymap::Action::PrevTab => Some(self.run_tab_command(TabCommand::Previous)),
        }
    }

    /// キーを伴わずに届いた文字の 1 単位（IME の確定など）。
    pub(super) fn on_text_unit(&mut self, unit: u16) {
        let Some(c) = self.text_input.push(unit) else {
            return;
        };
        if c.is_control() {
            return;
        }
        let mut buf = [0u8; 4];
        self.send(c.encode_utf8(&mut buf).as_bytes());
    }

    /// IME の変換中のウィンドウと候補のウィンドウを、いまのカーソルの位置に出させる。
    pub(super) fn place_ime_windows(&self) {
        let (col, row) = match self.session().vt.cursor_cell() {
            Ok(cell) => cell,
            Err(e) => {
                tracing::warn!("failed to read the cursor position: {e}");
                return;
            }
        };
        let (x, y) = self.terminal_to_client(
            (u32::from(col) * self.font.cell_width_px) as i32,
            (u32::from(row) * self.font.cell_height_px) as i32,
        );
        let cursor = CursorRect {
            x,
            y,
            width: self.font.cell_width_px as i32,
            height: self.font.cell_height_px as i32,
        };
        ime::place_windows(self.hwnd, cursor, &self.font);
    }

    /// 利用者の入力を子プロセスへ送る。過去を表示していたら、いちばん新しい行に戻す。
    pub(super) fn send(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.session_mut().vt.scroll_to_bottom();
        self.refresh_hovered_link_now();
        // 選択を消して描き直すので、シェルが何も返さないキーでも、戻した範囲を描き、つまみを合わせる。
        self.clear_selection();
        self.session_mut().write(data);
    }
}

/// 修飾キーそのもの。押した・離したときに、リンクの下線を出し入れする。
fn is_modifier_key(key: Key) -> bool {
    matches!(
        key,
        Key::ControlLeft
            | Key::ControlRight
            | Key::ShiftLeft
            | Key::ShiftRight
            | Key::AltLeft
            | Key::AltRight
            | Key::MetaLeft
            | Key::MetaRight
    )
}

/// WM_KEYDOWN の処理中に、そのキーから TranslateMessage が積んだ文字のメッセージを取り出す。
/// デッドキーだったときは `None`。
pub(super) fn take_translated_chars(hwnd: HWND) -> Option<Vec<char>> {
    const CHAR_MESSAGES: [u32; 4] = [WM_CHAR, WM_SYSCHAR, WM_DEADCHAR, WM_SYSDEADCHAR];
    let mut assembler = Utf16Assembler::default();
    let mut chars = Vec::new();
    let mut dead = false;
    loop {
        let mut msg = MSG::default();
        // SAFETY: msg は書き込める。このウィンドウの、指定した種類のメッセージだけを取り出す。
        let found = CHAR_MESSAGES.into_iter().find(|&kind| unsafe {
            PeekMessageW(&mut msg, Some(hwnd), kind, kind, PM_REMOVE).as_bool()
        });
        match found {
            None => break,
            Some(WM_DEADCHAR | WM_SYSDEADCHAR) => dead = true,
            Some(_) => chars.extend(assembler.push(msg.wParam.0 as u16)),
        }
    }
    (!dead).then_some(chars)
}
