//! 押したキーの組み合わせを読み取る小さなダイアログ。設定ダイアログのキーの割り当てに使う。
//!
//! 開いている間のキーのメッセージは、ウィンドウに届ける前にループで読み取る（Tab や Enter や Alt の
//! 組み合わせも、ダイアログの操作やメニューにせずに組み合わせとして読む）。修飾キーだけを押している間は
//! 待ち、ほかのキーを押したらその組み合わせで閉じる。修飾キーなしの Esc はやめる。名前を持たないキー
//! （設定ファイルに書けないキー）は読まずに待つ。Alt を含む F4 と Space は断って待つ。読むのはこの
//! ダイアログ宛てのメッセージだけ。

use std::{cell::Cell, sync::OnceLock};

use fantermo_core::{
    keymap::KeyCombo,
    vt::{Key, Mods},
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::HFONT,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::SetFocus,
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, HMENU, MSG, SW_SHOW, SetWindowTextW, ShowWindow,
                WINDOW_EX_STYLE, WM_CHAR, WM_CLOSE, WM_DEADCHAR, WM_DESTROY, WM_KEYDOWN, WM_KEYUP,
                WM_SYSCHAR, WM_SYSDEADCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WS_CAPTION, WS_CHILD,
                WS_EX_DLGMODALFRAME, WS_POPUP, WS_SYSMENU, WS_VISIBLE,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{
    dialog::{self, scale},
    keyboard::{self, KeyPosition},
    modal,
};

const CLASS: PCWSTR = w!("fantermo-key-capture");
const TITLE: &str = "キーの割り当て";

/// 96 DPI のときの寸法（ピクセル）。
const CLIENT_WIDTH: i32 = 360;
/// 読み取りの文、断る文、Esc の案内の 3 行が入る高さ。
const CLIENT_HEIGHT: i32 = 96;
const MARGIN: i32 = 12;

/// やめかたの案内の行。
const CANCEL_GUIDE: &str = "Esc キーでキャンセルします。";

/// `owner` の真ん中に、`prompt` を見せるダイアログを開き、押したキーの組み合わせを返す。やめたら
/// `None`。`font` はダイアログの文字のフォント、`dpi` は寸法を決める DPI。
pub fn capture(owner: HWND, prompt: &str, font: HFONT, dpi: u32) -> Option<KeyCombo> {
    if let Err(e) = register_class() {
        tracing::warn!("failed to register the key capture dialog: {e}");
        return None;
    }
    let (hwnd, label) = match create(owner, prompt, font, dpi) {
        Ok(windows) => windows,
        Err(e) => {
            tracing::warn!("failed to open the key capture dialog: {e}");
            return None;
        }
    };
    let closed = Cell::new(false);
    // SAFETY: closed は modal::run が戻るまで生きていて、modal::run はダイアログが壊れる（WM_DESTROY で外す）まで
    // 戻らない。
    unsafe { dialog::attach_state(hwnd, &closed) };
    // SAFETY: hwnd は有効なウィンドウ。
    unsafe {
        _ = ShowWindow(hwnd, SW_SHOW);
        _ = SetFocus(Some(hwnd));
    }
    let captured = Cell::new(None);
    modal::run(owner, hwnd, &closed, |msg| {
        let Some(read) = read_key(msg, hwnd) else {
            return false;
        };
        match read {
            Read::Done(combo) => {
                captured.set(combo);
                // 閉じると WM_DESTROY で closed が立つ。
                dialog::destroy(hwnd);
            }
            Read::Refused(combo) => show_refusal(label, prompt, combo),
            Read::Wait => {}
        }
        true
    });
    captured.get()
}

/// 押された組み合わせ `combo` を断る文を、ダイアログの文 `label` に足して見せる。
fn show_refusal(label: HWND, prompt: &str, combo: KeyCombo) {
    let text = label_text(prompt, Some(combo));
    // SAFETY: label はこのダイアログの子のウィンドウで、文字列はこの呼び出しの間は生きている。
    if let Err(e) = unsafe { SetWindowTextW(label, &HSTRING::from(text)) } {
        tracing::warn!("cannot show the refusal: {e}");
    }
}

/// ダイアログの文。読み取りの文 `prompt`、断った組み合わせ `refused` があればその断る文、
/// [`CANCEL_GUIDE`] の順に行を並べる。
fn label_text(prompt: &str, refused: Option<KeyCombo>) -> String {
    match refused {
        Some(combo) => format!(
            "{prompt}\n{} は割り当てられません。\n{CANCEL_GUIDE}",
            combo.label()
        ),
        None => format!("{prompt}\n{CANCEL_GUIDE}"),
    }
}

/// キーのメッセージを読んだ結果。
enum Read {
    /// 読み取りを終える。組み合わせか、やめたなら `None`。
    Done(Option<KeyCombo>),
    /// 割り当てられない組み合わせ。断る文を見せて、待ち続ける。
    Refused(KeyCombo),
    /// ウィンドウには渡さず、待ち続ける。
    Wait,
}

/// このダイアログ `hwnd` 宛てのキーのメッセージ `msg` を読む。ほかのウィンドウ宛てのメッセージと、
/// キーのメッセージでないものは `None`（ほかの処理に渡す）。
fn read_key(msg: &MSG, hwnd: HWND) -> Option<Read> {
    if msg.hwnd != hwnd {
        return None;
    }
    match msg.message {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let position = KeyPosition::from_lparam(msg.lParam.0);
            if position.repeated {
                return Some(Read::Wait);
            }
            let key = keyboard::key_from_position(position);
            Some(combo_read(KeyCombo::new(keyboard::current_mods(), key)))
        }
        WM_KEYUP | WM_SYSKEYUP | WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR => {
            Some(Read::Wait)
        }
        _ => None,
    }
}

/// 押した組み合わせ `combo` をどう扱うか。修飾キーなしの Esc はやめ、名前を持たないキー（修飾キー
/// そのものなど）は待つ。Alt を含む F4 と Space（Alt+F4、Ctrl+Alt+Space など）は、メインウィンドウが
/// 修飾キーを見ずに Windows へ任せて割り当てても効かないので断る。
fn combo_read(combo: KeyCombo) -> Read {
    if combo.key == Key::Escape && combo.mods == Mods::empty() {
        return Read::Done(None);
    }
    if combo.mods.contains(Mods::ALT) && matches!(combo.key, Key::F4 | Key::Space) {
        return Read::Refused(combo);
    }
    if combo.is_nameable() {
        Read::Done(Some(combo))
    } else {
        Read::Wait
    }
}

fn register_class() -> windows::core::Result<()> {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    dialog::register_class(&REGISTERED, CLASS, Some(capture_proc), None)
}

/// ダイアログを作り、`owner` の真ん中に置く。ダイアログと、その中の文のウィンドウを返す。
fn create(owner: HWND, prompt: &str, font: HFONT, dpi: u32) -> windows::core::Result<(HWND, HWND)> {
    let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
    let (width, height) =
        dialog::window_size(CLIENT_WIDTH, CLIENT_HEIGHT, style, WS_EX_DLGMODALFRAME, dpi)?;
    let (left, top) = dialog::center_in(&dialog::window_rect(owner)?, width, height);
    // SAFETY: 自分のモジュールのハンドルを問い合わせるだけ。
    let instance = unsafe { GetModuleHandleW(None)? };
    // SAFETY: クラスは登録済みで、文字列はこの呼び出しの間は生きている。
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            CLASS,
            &HSTRING::from(TITLE),
            style,
            left,
            top,
            width,
            height,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        )?
    };
    let text = label_text(prompt, None);
    // SAFETY: hwnd は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
    let label = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            &HSTRING::from(text),
            WS_CHILD | WS_VISIBLE,
            scale(MARGIN, dpi),
            scale(MARGIN, dpi),
            scale(CLIENT_WIDTH - 2 * MARGIN, dpi),
            scale(CLIENT_HEIGHT - 2 * MARGIN, dpi),
            Some(hwnd),
            Some(HMENU(std::ptr::null_mut())),
            Some(instance.into()),
            None,
        )?
    };
    // フォントは呼び出し元のダイアログのもので、そのダイアログが閉じるまで消さない。
    dialog::set_font(label, font, false);
    Ok((hwnd, label))
}

extern "system" fn capture_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => {
            dialog::destroy(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: capture が結び付けたのは Cell<bool>。
            if let Some(closed) = unsafe { dialog::state_of::<Cell<bool>>(hwnd) } {
                dialog::on_destroy(hwnd, closed);
            }
            LRESULT(0)
        }
        // SAFETY: 受け取った引数をそのまま既定の処理に渡す。
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Option<Option<KeyCombo>> {
        match combo_read(KeyCombo::parse(text).unwrap()) {
            Read::Done(combo) => Some(combo),
            Read::Refused(_) | Read::Wait => None,
        }
    }

    #[test]
    fn alt_f4_and_alt_space_are_refused() {
        for text in [
            "alt+f4",
            "alt+space",
            "alt+shift+f4",
            "ctrl+alt+f4",
            "ctrl+alt+space",
        ] {
            let combo = KeyCombo::parse(text).unwrap();
            assert!(matches!(combo_read(combo), Read::Refused(refused) if refused == combo));
        }
        // Alt を含まない F4 と Space、Alt を含む F4 と Space 以外のキーは割り当てられる。
        for text in ["ctrl+f4", "f4", "ctrl+space", "alt+tab"] {
            assert!(matches!(
                combo_read(KeyCombo::parse(text).unwrap()),
                Read::Done(Some(_))
            ));
        }
        assert_eq!(
            label_text("押してください。", Some(KeyCombo::parse("alt+f4").unwrap())),
            "押してください。\nAlt+F4 は割り当てられません。\nEsc キーでキャンセルします。"
        );
        assert_eq!(
            label_text("押してください。", None),
            "押してください。\nEsc キーでキャンセルします。"
        );
    }

    #[test]
    fn named_combos_end_the_capture_and_escape_alone_cancels() {
        assert_eq!(
            read("ctrl+shift+t"),
            Some(Some(KeyCombo::parse("ctrl+shift+t").unwrap()))
        );
        assert_eq!(read("escape"), Some(None));
        assert_eq!(
            read("shift+escape"),
            Some(Some(KeyCombo::parse("shift+escape").unwrap()))
        );
        // 修飾キーそのものは名前を持たないので待つ。
        assert!(matches!(
            combo_read(KeyCombo::new(Mods::CTRL, Key::ControlLeft)),
            Read::Wait
        ));
    }
}
