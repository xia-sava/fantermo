use crate::common::Server;
use crate::ui::{
    BUTTON_CANCEL, BUTTON_OK, CMD_COMMAND, CMD_PROMPT, check_terminal_still_works, control, number,
    read_controls, wait_text_dialog,
};
use serde_json::Value;

/// `paste.confirm_size` の既定の、バイトの単位の値。
const DEFAULT_PASTE_CONFIRM_BYTES: usize = 8 * 1024;

/// 改行を含む貼り付けをそのまま送る設定。
const PASTE_NEVER_CONFIRMS: &str = "[paste]\nconfirm = \"never\"\n";

/// 全体が `bytes` バイトになる、`cmd.exe` で実行できる行の並び。最後の行は `marker-0-<tail>` を出す。
fn paste_commands(bytes: usize, tail: &str) -> String {
    const LINE: &str = "rem 0123456789012345678901234567890123456789\n";
    let last = format!("echo marker-%ERRORLEVEL%-{tail}\n");
    let mut text = LINE.repeat((bytes - last.len()) / LINE.len());
    let padding = bytes - last.len() - text.len();
    if padding > 0 {
        assert!(padding >= "rem \n".len(), "{padding}");
        text.push_str("rem ");
        text.push_str(&"x".repeat(padding - "rem \n".len()));
        text.push('\n');
    }
    text.push_str(&last);
    assert_eq!(text.len(), bytes);
    text
}

#[test]
fn a_paste_just_under_the_size_is_not_confirmed() {
    let server = Server::start(PASTE_NEVER_CONFIRMS, &["cmd.exe", "/k", "prompt", "e2e$G"]);
    server.wait_text(CMD_PROMPT);
    server.paste(&paste_commands(DEFAULT_PASTE_CONFIRM_BYTES - 1, "small"));
    let state = server.wait_text("marker-0-small");
    assert_eq!(state["dialogs"].as_array().map(Vec::len), Some(0));
}

#[test]
fn a_paste_of_the_size_is_confirmed_and_can_be_cancelled() {
    let server = Server::start(PASTE_NEVER_CONFIRMS, &["cmd.exe", "/k", "prompt", "e2e$G"]);
    server.wait_text(CMD_PROMPT);

    server.paste(&paste_commands(DEFAULT_PASTE_CONFIRM_BYTES, "cancelled"));
    let controls = wait_text_dialog(&server);
    assert!(
        controls
            .iter()
            .any(|text| text.contains("大きなテキストを貼り付けようとしています（8 KB）")),
        "{controls:?}"
    );
    assert!(
        controls
            .iter()
            .any(|text| text.contains("marker-%ERRORLEVEL%-cancelled")),
        "the text box does not show the text: {controls:?}"
    );
    server.press_dialog_button(BUTTON_CANCEL);
    server.wait_dialogs(0);

    server.paste(&paste_commands(DEFAULT_PASTE_CONFIRM_BYTES, "accepted"));
    wait_text_dialog(&server);
    server.press_dialog_button(BUTTON_OK);
    let state = server.wait_text("marker-0-accepted");
    let text = state["text"].as_str().unwrap_or_default();
    assert!(!text.contains("marker-0-cancelled"), "{text}");
}

/// 編集のダイアログは、開いている間に DPI が変われば、その DPI の寸法で並べ直し、フォントを作り直す。モニターの DPI は
/// 変えられないので、ダイアログへ送った `WM_DPICHANGED` への反応を見る。送る DPI はモニターの DPI の 2 倍で、
/// 制御の口が受ける上限を超えるモニターでは半分にする。
#[test]
fn the_text_dialog_follows_a_dpi_change() {
    /// 入力欄の上の行（大きさを添える文）のコントロール ID。
    const HEADER_ID: i32 = 200;
    /// 入力欄のコントロール ID。
    const EDIT_ID: i32 = 100;
    /// `POST /e2e/dialog/dpichanged/<DPI>` が受ける DPI の上限。
    const MAX_DPI: u64 = 576;
    let server = Server::start(PASTE_NEVER_CONFIRMS, &CMD_COMMAND);
    server.wait_text(CMD_PROMPT);
    server.paste(&paste_commands(DEFAULT_PASTE_CONFIRM_BYTES, "dpi"));
    wait_text_dialog(&server);
    let before = read_controls(&server);
    let from = number(&before["dpi"]);
    let to = if 2 * from <= MAX_DPI {
        2 * from
    } else {
        from / 2
    };
    // 制御の口の推奨の矩形と同じく、DPI の比をかけて四捨五入する。
    let scaled = |value: u64| (value * to + from / 2) / from;
    let scaled_font = |height: i64| (height as f64 * to as f64 / from as f64).round() as i64;
    let font_height = |dialog: &Value, id: i32| {
        control(dialog, id)["font_height"]
            .as_i64()
            .unwrap_or_else(|| panic!("{id} has no font: {dialog:#}"))
    };

    let (status, body) = server.request("POST", &format!("/e2e/dialog/dpichanged/{to}"), b"");
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let after: Value = serde_json::from_slice(&body).expect("the controls are not JSON");
    // 推奨の矩形どおりに、クライアント領域が DPI の比で変わる。
    for key in ["client_width", "client_height"] {
        assert_eq!(
            number(&after[key]),
            scaled(number(&before[key])),
            "{key}: {after:#}"
        );
    }
    // ボタンの大きさと余白は 96 DPI のときのピクセルから決めるので、DPI の比になる（丸めの差の 1 ピクセルまで）。
    let right_margin = |dialog: &Value, id: i32| {
        let button = control(dialog, id);
        number(&dialog["client_width"]) - number(&button["x"]) - number(&button["width"])
    };
    for id in [BUTTON_OK, BUTTON_CANCEL] {
        for key in ["width", "height"] {
            let expected = scaled(number(&control(&before, id)[key]));
            let actual = number(&control(&after, id)[key]);
            assert!(
                actual.abs_diff(expected) <= 1,
                "{id} {key}: {actual} for {expected}: {after:#}"
            );
        }
        let expected = scaled(right_margin(&before, id));
        let actual = right_margin(&after, id);
        assert!(
            actual.abs_diff(expected) <= 1,
            "{id}: {actual} for {expected}: {after:#}"
        );
    }
    // 入力欄とボタンのフォントは、開いたときのフォントを DPI の比で作り直したもの。
    for id in [EDIT_ID, BUTTON_OK, BUTTON_CANCEL] {
        assert_eq!(
            font_height(&after, id),
            scaled_font(font_height(&before, id)),
            "{id}: {after:#}"
        );
    }
    // 上の行はメッセージのフォントを新しい DPI で作り直し、その 1 行の高さで並べ直す。
    let expected = scaled_font(font_height(&before, HEADER_ID));
    let actual = font_height(&after, HEADER_ID);
    assert!(
        actual.abs_diff(expected) <= 1,
        "{actual} for {expected}: {after:#}"
    );
    let (header_before, header_after) = (
        number(&control(&before, HEADER_ID)["height"]),
        number(&control(&after, HEADER_ID)["height"]),
    );
    assert_eq!(
        header_after > header_before,
        to > from,
        "{before:#}\n{after:#}"
    );

    server.press_dialog_button(BUTTON_CANCEL);
    server.wait_dialogs(0);
    check_terminal_still_works(&server, "after-text-dialog-dpi");
}

#[test]
fn a_large_paste_is_not_confirmed_when_the_size_is_zero() {
    let config = format!("{PASTE_NEVER_CONFIRMS}confirm_size = 0\n");
    let server = Server::start(&config, &["cmd.exe", "/k", "prompt", "e2e$G"]);
    server.wait_text(CMD_PROMPT);
    server.paste(&paste_commands(4 * DEFAULT_PASTE_CONFIRM_BYTES, "large"));
    let state = server.wait_text("marker-0-large");
    assert_eq!(state["dialogs"].as_array().map(Vec::len), Some(0));
}

#[test]
fn a_paste_with_newlines_is_confirmed_when_the_setting_asks_for_it() {
    let server = Server::start(
        "[paste]\nconfirm = \"multiline\"\n",
        &["cmd.exe", "/k", "prompt", "e2e$G"],
    );
    server.wait_text(CMD_PROMPT);
    server.paste("echo multi-%ERRORLEVEL%-line\n");
    let controls = wait_text_dialog(&server);
    assert!(
        !controls.iter().any(|text| text.contains("大きなテキスト")),
        "{controls:?}"
    );
    server.press_dialog_button(BUTTON_OK);
    server.wait_text("multi-0-line");
}
