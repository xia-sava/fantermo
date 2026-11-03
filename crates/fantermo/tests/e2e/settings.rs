use crate::common;
use crate::common::Server;
use crate::ui::{
    BAND_BOTTOM, BAND_TOP, BANDS_IMAGE, BUTTON_CANCEL, BackdropArea, CMD_COMMAND, CMD_PROMPT,
    EMPTY_COL, EMPTY_ROWS_BELOW, ID_CANCEL, ID_NO, ID_OK, ID_YES, MENU_POLL_INTERVAL, MENU_TIMEOUT,
    ONE_PROFILE, PAGE_APPEARANCE, SETTINGS_DIALOG_CLASS, SYSTEM_DIALOG_CLASS, TERMINAL_BACKGROUND,
    TEXT_DIALOG_CLASS, assert_cell_is_drawn, assert_controls_fit, assert_layout_fits_the_client,
    cell_pixel, check_terminal_still_works, click, close_message_box, config_path, control,
    control_enabled, current_window, dialog_text, find_cell, number, number_of, open_settings,
    over_terminal_background, press_cancel, read_controls, solid_image, start_settings_test,
    start_uneven, start_with_background, tab_count, wait_dialog_of_class, wait_message_box,
    wait_pixels, wait_tabs, wait_top_dialog,
};
use fantermo_core::config;
use fantermo_core::config::BackendKind;
use fantermo_core::config::BackgroundFit;
use fantermo_core::config::BackgroundImage;
use fantermo_core::config::ClipboardRead;
use fantermo_core::config::ClipboardWrite;
use fantermo_core::config::ColorScheme;
use fantermo_core::config::Config;
use fantermo_core::config::PasteConfirm;
use fantermo_core::config::WideGlyph;
use fantermo_core::frame::Rgb;
use fantermo_core::keymap::Action;
use fantermo_core::keymap::KeyCombo;
use fantermo_core::keymap::Keymap;
use serde_json::Value;
use std::os::windows::fs::OpenOptionsExt;
use std::time::Duration;
use std::time::Instant;

const ID_PAGES: i32 = 100;
const ID_APPLY: i32 = 101;
const ID_ERROR: i32 = 102;
const ID_FONT: i32 = 200;
const ID_SIZE: i32 = 201;
const ID_SIZE_SPIN: i32 = 202;
const ID_WIDE_GLYPH: i32 = 210;
const ID_SCROLL_BAR_WIDTH: i32 = 220;
const ID_SCROLL_BAR_WIDTH_SPIN: i32 = 221;
const ID_PADDING: i32 = 230;
const ID_PASTE_CONFIRM: i32 = 300;
const ID_CLIPBOARD_READ: i32 = 310;
const ID_CLIPBOARD_WRITE: i32 = 315;
const ID_PASTE_CONFIRM_SIZE: i32 = 320;
const ID_PASTE_CONFIRM_SIZE_SPIN: i32 = 321;
const ID_PROFILES: i32 = 400;
const ID_PROFILE_ADD: i32 = 401;
const ID_PROFILE_REMOVE: i32 = 402;
const ID_PROFILE_DEFAULT: i32 = 403;
const ID_PROFILE_NAME: i32 = 404;
const ID_PROFILE_COMMAND: i32 = 405;
const ID_PROFILE_BACKEND: i32 = 406;
const ID_PROFILE_LOCATION: i32 = 407;
const ID_PROFILE_COMMAND_WARNING: i32 = 408;
const ID_PROFILE_ICON: i32 = 409;
const ID_PROFILE_ICON_CHOOSE: i32 = 410;
const ID_BACKGROUND_IMAGES: i32 = 450;
const ID_BACKGROUND_ADD_FILES: i32 = 451;
const ID_BACKGROUND_ADD_FOLDER: i32 = 452;
const ID_BACKGROUND_REMOVE: i32 = 453;
const ID_BACKGROUND_OPACITY: i32 = 454;
const ID_BACKGROUND_FIT: i32 = 456;
const ID_BACKGROUND_ITEM_FIT: i32 = 457;
const ID_BACKGROUND_ITEM_OPACITY: i32 = 458;
/// 項目の不透明度の「既定」と、続く「指定」のラジオボタン。
const ID_BACKGROUND_ITEM_OPACITY_CHOICE: i32 = 460;
const ID_KEYS: i32 = 500;
const ID_KEY_ADD: i32 = 501;
const ID_KEY_CHANGE: i32 = 502;
const ID_KEY_REMOVE: i32 = 503;
const ID_KEY_RESET: i32 = 504;
const ID_COLOR_SCHEME: i32 = 590;
const ID_COLORS: i32 = 600;
/// 色のページの一行のコントロールの ID の間隔と、値の文字・選ぶ・既定に戻すの位置。
const COLOR_ID_STRIDE: i32 = 4;
const COLOR_TEXT: i32 = 1;
const COLOR_CHOOSE: i32 = 2;
const COLOR_RESET: i32 = 3;
/// 色のページの行の位置。端末の背景色と文字色、タブバーの背景、スクロールバーのつまみ。
const COLOR_ROW_BACKGROUND: i32 = 0;
const COLOR_ROW_FOREGROUND: i32 = 1;
const COLOR_ROW_TAB_BAR: i32 = 2;
const COLOR_ROW_THUMB: i32 = 12;

const PAGE_BACKGROUND: usize = 1;
const PAGE_BEHAVIOR: usize = 2;
const PAGE_PROFILES: usize = 3;
const PAGE_KEYS: usize = 4;
const PAGE_COLORS: usize = 5;

/// 接続方式のコンボボックスの位置。
const BACKEND_CONPTY: usize = 0;
const BACKEND_CYGWIN: usize = 1;
const BACKEND_WSL: usize = 2;
/// 表示方法のコンボボックスの位置（項目のものは先頭に「既定」があり、一つずれる）。
const FIT_TILE: usize = 3;
const FIT_CENTER: usize = 4;
const ITEM_FIT_DEFAULT: usize = 0;
const ITEM_FIT_CENTER: usize = FIT_CENTER + 1;

/// 色を選ぶダイアログの赤・緑・青の入力欄。
const COLOR_DIALOG_RED: i32 = 706;
const COLOR_DIALOG_GREEN: i32 = 707;
const COLOR_DIALOG_BLUE: i32 = 708;

const KEY_CAPTURE_CLASS: &str = "fantermo-key-capture";

/// いまの ID が `id` のコントロールの文字。
#[track_caller]
fn control_text(server: &Server, id: i32) -> String {
    let dialog = read_controls(server);
    control(&dialog, id)["text"]
        .as_str()
        .expect("no text")
        .to_owned()
}

/// いまの一覧やコンボボックス `id` の項目の数と、選んでいる位置。
#[track_caller]
fn list_state(server: &Server, id: i32) -> (usize, Value) {
    let dialog = read_controls(server);
    let list = control(&dialog, id);
    (
        list["items"].as_array().map_or(0, Vec::len),
        list["selected"].clone(),
    )
}

/// ラジオボタン `id` を選んでいるか。
#[track_caller]
fn radio_checked(dialog: &Value, id: i32) -> bool {
    control(dialog, id)["checked"] == true
}

fn set_text(server: &Server, id: i32, text: &str) {
    server.post(&format!("/e2e/dialog/text/{id}"), text.as_bytes());
}

fn select(server: &Server, id: i32, position: usize) {
    server.post(&format!("/e2e/dialog/select/{id}/{position}"), b"");
}

fn choose_radio(server: &Server, id: i32) {
    server.post(&format!("/e2e/dialog/radio/{id}"), b"");
}

fn spin(server: &Server, id: i32, delta: i32) {
    server.post(&format!("/e2e/dialog/spin/{id}/{delta}"), b"");
}

fn press_key_in_dialog(server: &Server, combo: &str) {
    server.post(&format!("/e2e/dialog/key/{combo}"), b"");
}

fn show_page(server: &Server, page: usize) {
    select(server, ID_PAGES, page);
}

/// 「OK」を押して、閉じるのを待つ。
#[track_caller]
fn press_ok(server: &Server) {
    click(server, ID_OK);
    server.wait_dialogs(0);
}

/// 「適用」を押す。続く要求はこの処理のあとなので、状態を読んだあとにファイルを読めば書いたものが読める。
#[track_caller]
fn press_apply(server: &Server) {
    click(server, ID_APPLY);
    server.state();
}

fn config_text(server: &Server) -> String {
    std::fs::read_to_string(config_path(server)).expect("failed to read the settings")
}

/// 設定ファイルを読んで解釈する。
#[track_caller]
fn saved_config(server: &Server) -> Config {
    let text = config_text(server);
    let parsed = config::parse(&text, &Config::default());
    assert!(!parsed.syntax_error, "{text}");
    parsed.config
}

/// 起動時の値と、ページごとに見えるコントロールと、大きさの確認の設定。
const EVERY_PAGE_CONFIG: &str = r##"default_profile = "two"

[[profiles]]
name = "one"
command = "cmd.exe /k prompt e2e$G"

[[profiles]]
name = "two"
command = "cmd.exe /k prompt e2e$G"
icon = 'two.png'

[[profiles]]
name = "ubuntu"
backend = "wsl"
distribution = "Ubuntu"
command = "bash -l"

[[profiles]]
name = "zsh"
backend = "cygwin"
cygwin_root = 'C:\cygwin-of-the-test'

[font]
family = ["Consolas"]
size = 14
wide_glyph = "clip"

[paste]
confirm = "multiline"
confirm_size = 16

[clipboard]
read = "deny"
write = "deny"

[scroll_bar]
width = 20

[window]
padding = 8

[colors]
scheme = "light"
background = "#102030"

[colors.tab_bar]
bar = "#112233"

[colors.scroll_bar]
thumb = "#445566"

[background]
image_opacity = 0.5
fit = "tile"

[[background.images]]
path = 'a.png'

[[background.images]]
path = 'b.png'
fit = "center"
image_opacity = 0.25

[keys]
"ctrl+shift+n" = "new_tab"
"ctrl+tab" = "none"
"##;

/// `id` の色の行の、`offset` のコントロールの ID。
const fn color_control(row: i32, offset: i32) -> i32 {
    ID_COLORS + row * COLOR_ID_STRIDE + offset
}

/// キーの割り当ての一覧の行の数（組み合わせのない動作も一行）と、`action` の `combo` の行の位置（`combo` が `None` なら
/// 組み合わせのない行）。
fn key_row_of(keys: &Keymap, action: Action, combo: Option<&str>) -> usize {
    let combo = combo.map(|combo| KeyCombo::parse(combo).expect("a valid combination"));
    let mut position = 0;
    for candidate in Action::ALL {
        let combos = keys.combos_of(candidate);
        if candidate == action {
            let offset = combo.map_or(0, |combo| {
                combos
                    .iter()
                    .position(|&other| other == combo)
                    .unwrap_or_else(|| panic!("{combo} is not bound to {action:?}"))
            });
            return position + offset;
        }
        position += combos.len().max(1);
    }
    unreachable!("every action is listed")
}

/// 全ページが開いて、見えるコントロールがページごとに替わり、起動時の値が設定ファイルのとおり。
#[test]
fn every_settings_page_opens_and_shows_the_values_of_the_file() {
    let server = start_settings_test_with_images(EVERY_PAGE_CONFIG, &["a.png", "b.png"]);
    let config = saved_config(&server);
    let dialog = open_settings(&server);
    assert_eq!(control(&dialog, ID_PAGES)["selected"], PAGE_APPEARANCE);
    let pages = control(&dialog, ID_PAGES)["items"]
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(pages, 6, "{dialog:#}");

    // 外観
    assert_eq!(control(&dialog, ID_FONT)["text"], "Consolas", "{dialog:#}");
    assert_eq!(control(&dialog, ID_SIZE)["text"], "14");
    assert!(radio_checked(&dialog, ID_WIDE_GLYPH + 1), "{dialog:#}");
    assert_eq!(control(&dialog, ID_SCROLL_BAR_WIDTH)["text"], "20");
    assert_eq!(control(&dialog, ID_PADDING)["text"], "8");

    // 背景
    assert_eq!(control(&dialog, ID_BACKGROUND_OPACITY)["text"], "0.5");
    assert_eq!(control(&dialog, ID_BACKGROUND_FIT)["selected"], FIT_TILE);
    assert_eq!(list_state(&server, ID_BACKGROUND_IMAGES), (2, Value::Null));
    assert!(!control_enabled_on_page(
        &server,
        PAGE_BACKGROUND,
        ID_BACKGROUND_REMOVE
    ));
    select(&server, ID_BACKGROUND_IMAGES, 0);
    let dialog = read_controls(&server);
    assert_eq!(
        control(&dialog, ID_BACKGROUND_ITEM_FIT)["selected"],
        ITEM_FIT_DEFAULT
    );
    assert!(radio_checked(&dialog, ID_BACKGROUND_ITEM_OPACITY_CHOICE));
    select(&server, ID_BACKGROUND_IMAGES, 1);
    let dialog = read_controls(&server);
    assert_eq!(
        control(&dialog, ID_BACKGROUND_ITEM_FIT)["selected"],
        ITEM_FIT_CENTER
    );
    assert!(radio_checked(
        &dialog,
        ID_BACKGROUND_ITEM_OPACITY_CHOICE + 1
    ));
    assert_eq!(control(&dialog, ID_BACKGROUND_ITEM_OPACITY)["text"], "0.25");

    // 動作
    assert!(radio_checked(&dialog, ID_PASTE_CONFIRM + 1), "{dialog:#}");
    assert!(radio_checked(&dialog, ID_CLIPBOARD_READ + 2), "{dialog:#}");
    assert!(radio_checked(&dialog, ID_CLIPBOARD_WRITE + 1), "{dialog:#}");
    assert_eq!(control(&dialog, ID_PASTE_CONFIRM_SIZE)["text"], "16");

    // プロファイル。開くと既定のプロファイルを選んでいる。
    assert_eq!(list_state(&server, ID_PROFILES), (4, Value::from(1)));
    assert_eq!(control(&dialog, ID_PROFILE_NAME)["text"], "two");
    assert_eq!(
        control(&dialog, ID_PROFILE_COMMAND)["text"],
        "cmd.exe /k prompt e2e$G"
    );
    assert_eq!(control(&dialog, ID_PROFILE_ICON)["text"], "two.png");
    assert_eq!(
        control(&dialog, ID_PROFILE_BACKEND)["selected"],
        BACKEND_CONPTY
    );
    show_page(&server, PAGE_PROFILES);
    assert!(!control_enabled(&server, ID_PROFILE_LOCATION));
    select(&server, ID_PROFILES, 2);
    let dialog = read_controls(&server);
    assert_eq!(
        control(&dialog, ID_PROFILE_BACKEND)["selected"],
        BACKEND_WSL
    );
    assert_eq!(control(&dialog, ID_PROFILE_LOCATION)["text"], "Ubuntu");
    select(&server, ID_PROFILES, 3);
    let dialog = read_controls(&server);
    assert_eq!(
        control(&dialog, ID_PROFILE_BACKEND)["selected"],
        BACKEND_CYGWIN
    );
    assert_eq!(
        control(&dialog, ID_PROFILE_LOCATION)["text"],
        r"C:\cygwin-of-the-test"
    );
    assert_eq!(control(&dialog, ID_PROFILE_COMMAND)["text"], "");

    // キー。組み合わせごとに一行で、組み合わせのない動作も一行。
    let rows: usize = Action::ALL
        .iter()
        .map(|&action| config.keys.combos_of(action).len().max(1))
        .sum();
    assert_eq!(list_state(&server, ID_KEYS).0, rows);

    // 色
    assert!(radio_checked(&dialog, ID_COLOR_SCHEME + 1), "{dialog:#}");
    for (row, color) in [
        (COLOR_ROW_BACKGROUND, "#102030"),
        (COLOR_ROW_TAB_BAR, "#112233"),
        (COLOR_ROW_THUMB, "#445566"),
    ] {
        let value = &control(&dialog, color_control(row, COLOR_TEXT))["text"];
        assert!(
            value.as_str().is_some_and(|text| text.contains(color)),
            "{row}: {value}"
        );
    }

    // ページごとに見えるコントロールが替わる。
    let on_page: [(usize, &[i32]); 6] = [
        (
            PAGE_APPEARANCE,
            &[ID_FONT, ID_SIZE, ID_WIDE_GLYPH, ID_PADDING],
        ),
        (
            PAGE_BACKGROUND,
            &[
                ID_BACKGROUND_IMAGES,
                ID_BACKGROUND_OPACITY,
                ID_BACKGROUND_FIT,
            ],
        ),
        (
            PAGE_BEHAVIOR,
            &[
                ID_PASTE_CONFIRM,
                ID_CLIPBOARD_READ,
                ID_CLIPBOARD_WRITE,
                ID_PASTE_CONFIRM_SIZE,
            ],
        ),
        (
            PAGE_PROFILES,
            &[ID_PROFILES, ID_PROFILE_NAME, ID_PROFILE_BACKEND],
        ),
        (PAGE_KEYS, &[ID_KEYS, ID_KEY_ADD, ID_KEY_RESET]),
        (
            PAGE_COLORS,
            &[
                ID_COLOR_SCHEME,
                color_control(COLOR_ROW_FOREGROUND, COLOR_RESET),
            ],
        ),
    ];
    for &(page, shown) in &on_page {
        show_page(&server, page);
        let dialog = read_controls(&server);
        for &(other, ids) in &on_page {
            for &id in ids {
                assert_eq!(
                    control(&dialog, id)["visible"],
                    other == page,
                    "the control {id} on the page {page}"
                );
            }
        }
        for id in [ID_PAGES, ID_OK, ID_CANCEL, ID_APPLY] {
            assert_eq!(control(&dialog, id)["visible"], true, "{id} on {page}");
        }
        assert!(
            shown
                .iter()
                .all(|&id| control(&dialog, id)["visible"] == true)
        );
        assert_controls_fit(&dialog, page);
    }
    press_cancel(&server);
}

/// 小さいウィンドウの真ん中に置くと画面の外へはみ出す設定ダイアログは、作業領域の中に置く。
#[test]
fn the_settings_dialog_stays_on_the_screen_over_a_small_window() {
    const SMALL_GRID: (u16, u16) = (20, 5);
    let server = start_settings_test(ONE_PROFILE);
    server.resize(SMALL_GRID.0, SMALL_GRID.1);
    let dialog = open_settings(&server);
    let work = &dialog["work_area"];
    let field = |value: &Value| value.as_i64().expect("not a number");
    let (left, top) = (field(&dialog["left"]), field(&dialog["top"]));
    let (width, height) = (field(&dialog["width"]), field(&dialog["height"]));
    assert!(
        left >= field(&work["left"])
            && top >= field(&work["top"])
            && left + width <= field(&work["right"])
            && top + height <= field(&work["bottom"]),
        "{dialog:#}"
    );
    press_cancel(&server);
}

/// `page` を見せて、コントロール `id` を押せるかを読む。
#[track_caller]
fn control_enabled_on_page(server: &Server, page: usize, id: i32) -> bool {
    show_page(server, page);
    control_enabled(server, id)
}

/// 設定 `config` で起動する前に、`images` の名前で色の画像を置く。
fn start_settings_test_with_images(config: &str, images: &[&str]) -> Server {
    let server = Server::start_with_setup(config, &[], |dir| {
        for image in images {
            std::fs::write(dir.join(image), solid_image([255, 0, 0]))
                .expect("failed to write the image");
        }
    });
    server.wait_text(CMD_PROMPT);
    server
}

/// 外観のページを変えると端末にすぐ反映するが、設定ファイルは「OK」まで変わらない。「OK」で変えた項目が入る。
/// ウィンドウの位置と大きさは変わらず、桁数と行数がいまのクライアント領域に合わせて変わる。
#[test]
fn the_appearance_page_previews_at_once_and_ok_writes_it() {
    let original = format!(
        "{ONE_PROFILE}[font]\nfamily = [\"Consolas\"]\nsize = 11\n\n[scroll_bar]\nwidth = 12\n\n[window]\npadding = 4\n"
    );
    let server = start_settings_test(&original);
    server.resize(50, 15);
    let before = server.state();
    let window = current_window(&before);
    let dialog = open_settings(&server);
    assert_eq!(server.state()["window_enabled"], false);

    set_text(&server, ID_SIZE, "20");
    spin(&server, ID_SIZE_SPIN, 1);
    assert_eq!(control_text(&server, ID_SIZE), "21");
    let taller = server.wait_until("taller cells", |state| {
        number(&state["cell_height"]) > number(&before["cell_height"])
    });
    assert_eq!(current_window(&taller), window, "{taller:#}");
    assert!(
        number(&taller["cols"]) < number(&before["cols"])
            && number(&taller["rows"]) < number(&before["rows"]),
        "{taller:#}"
    );
    assert_layout_fits_the_client(&taller);

    set_text(&server, ID_SCROLL_BAR_WIDTH, "30");
    let wider = server.wait_until("the wider scroll bar", |state| {
        state["scroll_bar"]["width"] == 30
    });
    assert_eq!(current_window(&wider), window, "{wider:#}");
    assert_layout_fits_the_client(&wider);
    set_text(&server, ID_PADDING, "16");
    let padding_grew = number(&before["terminal_x"]) + 12;
    let padded = server.wait_until("the wider padding", |state| {
        number(&state["terminal_x"]) == padding_grew
    });
    assert_eq!(current_window(&padded), window, "{padded:#}");
    assert_layout_fits_the_client(&padded);
    choose_radio(&server, ID_WIDE_GLYPH + 2);

    // 一覧の別のフォントを選ぶ。ランナーに一つしかなければ選ばない。
    let fonts = control(&dialog, ID_FONT)["items"]
        .as_array()
        .expect("no fonts");
    let other_font = fonts
        .iter()
        .position(|font| font != "Consolas")
        .map(|position| {
            (
                position,
                fonts[position].as_str().expect("a name").to_owned(),
            )
        });
    if let Some((position, _)) = &other_font {
        select(&server, ID_FONT, *position);
    }
    assert_eq!(config_text(&server), original, "written before OK");

    press_ok(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.font.size_pt, 21.0);
    assert_eq!(saved.scroll_bar.width, 30);
    assert_eq!(saved.window.padding, 16);
    assert_eq!(saved.font.wide_glyph, WideGlyph::Overflow);
    if let Some((_, name)) = other_font {
        assert_eq!(saved.font.families[0], name);
    }
    let state = server.wait_until("the window enabled again", |state| {
        state["window_enabled"] == true
    });
    assert_eq!(state["scroll_bar"]["width"], 30, "{state:#}");
    assert_eq!(current_window(&state), window, "{state:#}");
    check_terminal_still_works(&server, "after-ok");
}

/// 試し表示のあいだも「キャンセル」で戻したあとも、ウィンドウの位置と大きさは 1 ピクセルも変わらない。桁数と行数は
/// 元に戻る。セルで割り切れない大きさのウィンドウで確認する。
#[test]
fn the_preview_and_cancel_keep_the_window_rect() {
    let (server, before) = start_uneven(
        &format!("{ONE_PROFILE}[font]\nfamily = [\"Consolas\"]\nsize = 11\n"),
        &[],
        CMD_PROMPT,
    );
    let window = current_window(&before);
    open_settings(&server);

    set_text(&server, ID_SIZE, "20");
    let previewed = server.wait_until("taller cells", |state| {
        number(&state["cell_height"]) > number(&before["cell_height"])
    });
    assert_eq!(current_window(&previewed), window, "{previewed:#}");
    assert_ne!(previewed["rows"], before["rows"], "{previewed:#}");
    assert_layout_fits_the_client(&previewed);

    press_cancel(&server);
    let state = server.wait_until("the cells back", |state| {
        state["cell_height"] == before["cell_height"] && state["window_enabled"] == true
    });
    assert_eq!(current_window(&state), window, "{state:#}");
    assert_eq!(state["cols"], before["cols"], "{state:#}");
    assert_eq!(state["rows"], before["rows"], "{state:#}");
    assert_layout_fits_the_client(&state);
}

/// 「適用」は書いて開いたまま。そのあとの「キャンセル」は適用したときへ戻す。
#[test]
fn apply_keeps_the_dialog_open_and_cancel_returns_to_what_was_applied() {
    let server = start_settings_test(&format!("{ONE_PROFILE}[window]\npadding = 4\n"));
    let first_x = number(&server.state()["terminal_x"]);
    open_settings(&server);

    set_text(&server, ID_PADDING, "10");
    press_apply(&server);
    assert_eq!(saved_config(&server).window.padding, 10);
    assert_eq!(server.state()["dialogs"].as_array().map_or(0, Vec::len), 1);
    let applied_x = first_x + 6;
    server.wait_until("the applied padding", |state| {
        number(&state["terminal_x"]) == applied_x
    });
    let applied = config_text(&server);

    set_text(&server, ID_PADDING, "20");
    let previewed_x = first_x + 16;
    server.wait_until("the previewed padding", |state| {
        number(&state["terminal_x"]) == previewed_x
    });
    press_cancel(&server);
    let state = server.state();
    assert_eq!(number(&state["terminal_x"]), applied_x, "{state:#}");
    assert_eq!(config_text(&server), applied);
}

/// 「キャンセル」、×、Esc は、どのページの変更も端末から取り下げ、設定ファイルを変えない。Enter は「OK」。
#[test]
fn cancelling_leaves_the_file_and_the_window_as_they_were() {
    let original = format!(
        "{ONE_PROFILE}\n[font]\nsize = 11\n\n[window]\npadding = 4\n\n[paste]\nconfirm = \"never\"\n"
    );
    let server = start_settings_test(&original);
    let before = server.state();
    type CloseBy = (&'static str, fn(&Server));
    let close_by: [CloseBy; 3] = [
        ("the cancel button", |server| {
            click(server, ID_CANCEL);
        }),
        ("the close button of the window", |server| {
            server.post("/e2e/dialog/close", b"");
        }),
        ("the escape key", |server| {
            press_key_in_dialog(server, "escape");
        }),
    ];
    for (name, close) in close_by {
        open_settings(&server);
        set_text(&server, ID_SIZE, "20");
        set_text(&server, ID_PADDING, "16");
        show_page(&server, PAGE_BEHAVIOR);
        choose_radio(&server, ID_PASTE_CONFIRM + 1);
        show_page(&server, PAGE_PROFILES);
        click(&server, ID_PROFILE_ADD);
        server.wait_until("the previewed settings", |state| {
            number(&state["cell_height"]) > number(&before["cell_height"])
                && state["profiles"].as_array().map_or(0, Vec::len) == 2
        });
        close(&server);
        server.wait_dialogs(0);
        let state = server.state();
        assert_eq!(state["cell_height"], before["cell_height"], "{name}");
        assert_eq!(state["terminal_x"], before["terminal_x"], "{name}");
        assert_eq!(state["profiles"], before["profiles"], "{name}");
        assert_eq!(config_text(&server), original, "{name}");
        // 取り下げた貼り付けの設定に戻っている。改行があっても確認しない。
        server.paste("rem back\r\n");
        server.wait_text("rem back");
        assert_eq!(server.state()["dialogs"].as_array().map_or(0, Vec::len), 0);
    }

    open_settings(&server);
    set_text(&server, ID_PADDING, "9");
    press_key_in_dialog(&server, "enter");
    server.wait_dialogs(0);
    assert_eq!(saved_config(&server).window.padding, 9);
}

/// 設定ファイルの、ダイアログで扱わないコメントと未知のキーとテーブルは、保存で消えない。
#[test]
fn saving_keeps_the_comments_and_the_unknown_keys_of_the_file() {
    let original = r##"# 先頭のコメント
unknown_top = "kept"

# プロファイルのコメント
[[profiles]]
name = "one" # 名前のコメント
command = "cmd.exe /k prompt e2e$G"
unknown_in_profile = 1

[font]
# サイズのコメント
size = 11 # 行末のコメント
unknown_in_font = true

[window]
padding = 4

[unknown_table]
key = [1, 2, 3] # 未知のテーブルのコメント

[background]
image_opacity = 0.5

# 画像のコメント
[[background.images]]
path = 'a.png' # パスのコメント
unknown_in_image = "kept"

[[background.images]]
path = 'b.png'

[keys]
# キーのコメント
"ctrl+tab" = "none"
"##;
    let server = start_settings_test_with_images(original, &["a.png", "b.png"]);
    // 未知のキーは起動の警告で知らせる。
    wait_message_box(&server);
    close_message_box(&server, 0);

    // 何も変えなければ書かない。
    open_settings(&server);
    press_ok(&server);
    assert_eq!(config_text(&server), original);

    // 一つ変えれば、その値だけが変わる。
    open_settings(&server);
    set_text(&server, ID_SIZE, "14");
    press_ok(&server);
    assert_eq!(
        config_text(&server),
        original.replace("size = 11 #", "size = 14 #")
    );

    // いろいろなページを変えても、ほかの行は残る。
    open_settings(&server);
    set_text(&server, ID_PADDING, "6");
    show_page(&server, PAGE_BACKGROUND);
    select(&server, ID_BACKGROUND_FIT, FIT_CENTER);
    select(&server, ID_BACKGROUND_IMAGES, 0);
    select(&server, ID_BACKGROUND_ITEM_FIT, ITEM_FIT_CENTER);
    select(&server, ID_BACKGROUND_IMAGES, 1);
    click(&server, ID_BACKGROUND_REMOVE);
    show_page(&server, PAGE_PROFILES);
    set_text(&server, ID_PROFILE_COMMAND, "cmd.exe /k prompt renamed$G");
    click(&server, ID_PROFILE_ADD);
    show_page(&server, PAGE_COLORS);
    choose_radio(&server, ID_COLOR_SCHEME + 1);
    press_ok(&server);

    let text = config_text(&server);
    for line in original.lines() {
        // 変えた値の行と、一覧から外した項目の行のほかは残る。
        let changed = [
            "size = 11",
            "padding = 4",
            "command = ",
            "path = 'b.png'",
            "[[background.images]]",
            "[[profiles]]",
        ]
        .iter()
        .any(|written| line.starts_with(written));
        assert!(changed || text.contains(line), "lost {line:?} in:\n{text}");
    }
    let saved = saved_config(&server);
    assert_eq!(saved.window.padding, 6);
    assert_eq!(saved.background.images.len(), 1);
    assert_eq!(saved.background.images[0].fit, Some(BackgroundFit::Center));
    assert_eq!(saved.profiles.len(), 2);
    assert_eq!(saved.profiles[0].command, "cmd.exe /k prompt renamed$G");
    assert_eq!(saved.colors.scheme, ColorScheme::Light);
}

/// 数値の欄の不正な入力は、エラーの文を出し、書かず、その入力欄へ移る。直せば消える。
#[test]
fn invalid_numbers_show_an_error_and_are_not_saved() {
    let original = format!(
        "{ONE_PROFILE}[font]\nsize = 11\n\n[scroll_bar]\nwidth = 12\n\n[window]\npadding = 4\n\n[paste]\nconfirm_size = 8\n\n[background]\nimage_opacity = 0.5\n\n[[background.images]]\npath = 'a.png'\nimage_opacity = 0.25\n"
    );
    let server = start_settings_test_with_images(&original, &["a.png"]);
    let before = server.state();
    open_settings(&server);
    show_page(&server, PAGE_BACKGROUND);
    select(&server, ID_BACKGROUND_IMAGES, 0);

    let cases: [(usize, i32, &[&str], &str); 6] = [
        (
            PAGE_APPEARANCE,
            ID_SIZE,
            &["abc", "", "0", "129", "-1"],
            "11",
        ),
        (
            PAGE_APPEARANCE,
            ID_SCROLL_BAR_WIDTH,
            &["abc", "3", "65", "1.5"],
            "12",
        ),
        (PAGE_APPEARANCE, ID_PADDING, &["abc", "-1", "65"], "4"),
        (
            PAGE_BEHAVIOR,
            ID_PASTE_CONFIRM_SIZE,
            &["abc", "-1", "1048577"],
            "8",
        ),
        (
            PAGE_BACKGROUND,
            ID_BACKGROUND_OPACITY,
            &["abc", "-0.1", "1.1"],
            "0.5",
        ),
        (
            PAGE_BACKGROUND,
            ID_BACKGROUND_ITEM_OPACITY,
            &["abc", "2"],
            "0.25",
        ),
    ];
    for (page, id, bad_values, good) in cases {
        show_page(&server, page);
        for bad in bad_values {
            set_text(&server, id, bad);
            press_apply(&server);
            let dialog = read_controls(&server);
            assert_ne!(
                control(&dialog, ID_ERROR)["text"],
                "",
                "{id} accepted {bad:?}"
            );
            assert_eq!(dialog["focus"], id, "{id} with {bad:?}");
            assert_eq!(config_text(&server), original, "{id} with {bad:?}");
            // 端末は読めた値のまま。
            let state = server.state();
            assert_eq!(state["cell_height"], before["cell_height"], "{id}");
            assert_eq!(state["terminal_x"], before["terminal_x"], "{id}");
            // 「OK」でも閉じない。
            click(&server, ID_OK);
            assert_eq!(open_dialog_count(&server), 1, "{id} with {bad:?}");
        }
        set_text(&server, id, good);
        press_apply(&server);
        assert_eq!(control_text(&server, ID_ERROR), "", "{id}");
        assert_eq!(config_text(&server), original, "{id}");
    }

    // 打っているだけでも、少し待ってエラーの文を出す。
    show_page(&server, PAGE_APPEARANCE);
    set_text(&server, ID_SIZE, "abc");
    let deadline = Instant::now() + MENU_TIMEOUT;
    while control_text(&server, ID_ERROR).is_empty() {
        assert!(Instant::now() < deadline, "no error for what was typed");
        std::thread::sleep(MENU_POLL_INTERVAL);
    }
    // 矢印は読めない入力なら開いたときの値から動かす。
    spin(&server, ID_SIZE_SPIN, 1);
    assert_eq!(control_text(&server, ID_SIZE), "12");
    assert_eq!(control_text(&server, ID_ERROR), "");

    // 矢印は範囲の外へ出ない。
    set_text(&server, ID_SCROLL_BAR_WIDTH, "64");
    spin(&server, ID_SCROLL_BAR_WIDTH_SPIN, 1);
    assert_eq!(control_text(&server, ID_SCROLL_BAR_WIDTH), "64");
    set_text(&server, ID_SCROLL_BAR_WIDTH, "4");
    spin(&server, ID_SCROLL_BAR_WIDTH_SPIN, -1);
    assert_eq!(control_text(&server, ID_SCROLL_BAR_WIDTH), "4");
    show_page(&server, PAGE_BEHAVIOR);
    set_text(&server, ID_PASTE_CONFIRM_SIZE, "0");
    spin(&server, ID_PASTE_CONFIRM_SIZE_SPIN, -1);
    assert_eq!(control_text(&server, ID_PASTE_CONFIRM_SIZE), "0");

    // 不正な入力があってもキャンセルでは閉じる。
    show_page(&server, PAGE_APPEARANCE);
    set_text(&server, ID_PADDING, "abc");
    press_cancel(&server);
    assert_eq!(config_text(&server), original);
    let state = server.state();
    assert_eq!(state["cell_height"], before["cell_height"], "{state:#}");
}

/// 開いているダイアログの数を読む。前の要求の処理のあとに読むので、閉じたならここで分かる。
fn open_dialog_count(server: &Server) -> usize {
    server.state()["dialogs"].as_array().map_or(0, Vec::len)
}

/// 動作のページの値が入り、貼り付けとクリップボードの動作が変わる。
#[test]
fn the_behavior_page_changes_how_pastes_are_confirmed() {
    let server = start_settings_test(&format!("{ONE_PROFILE}[paste]\nconfirm = \"never\"\n"));
    server.paste("rem first\r\nrem second\r\n");
    server.wait_text("rem second");
    assert_eq!(open_dialog_count(&server), 0);

    open_settings(&server);
    show_page(&server, PAGE_BEHAVIOR);
    choose_radio(&server, ID_PASTE_CONFIRM + 1);
    let dialog = read_controls(&server);
    assert!(radio_checked(&dialog, ID_PASTE_CONFIRM + 1));
    assert!(!radio_checked(&dialog, ID_PASTE_CONFIRM + 2));
    choose_radio(&server, ID_CLIPBOARD_READ + 1);
    choose_radio(&server, ID_CLIPBOARD_WRITE + 1);
    spin(&server, ID_PASTE_CONFIRM_SIZE_SPIN, 1);
    press_ok(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.paste.confirm, PasteConfirm::Multiline);
    assert_eq!(saved.paste.confirm_size, 9);
    assert_eq!(saved.clipboard.read, ClipboardRead::Allow);
    assert_eq!(saved.clipboard.write, ClipboardWrite::Deny);

    server.paste("rem third\r\nrem fourth\r\n");
    wait_dialog_of_class(&server, TEXT_DIALOG_CLASS);
    server.press_dialog_button(BUTTON_CANCEL);
    server.wait_dialogs(0);
}

/// 開いているダイアログにないコントロールと、押せないコントロールの操作は断る。
#[test]
fn dialog_requests_are_refused_for_what_a_user_cannot_touch() {
    let server = start_settings_test(ONE_PROFILE);
    let refused = |path: &str, body: &[u8]| {
        let (status, answer) = server.request("POST", path, body);
        assert_eq!(status, 400, "{path}: {}", String::from_utf8_lossy(&answer));
    };
    let (status, _) = server.request("GET", "/e2e/dialog/controls", b"");
    assert_eq!(status, 400, "no dialog is open");
    refused(&format!("/e2e/dialog/click/{ID_OK}"), b"");
    refused("/e2e/dialog/close", b"");

    open_settings(&server);
    // 隠れているページのコントロール。
    refused(&format!("/e2e/dialog/click/{ID_KEY_ADD}"), b"");
    // 無いコントロールと、クラスの違うコントロール。
    refused("/e2e/dialog/text/9999", b"x");
    refused(&format!("/e2e/dialog/text/{ID_OK}"), b"x");
    refused(&format!("/e2e/dialog/radio/{ID_OK}"), b"");
    refused(&format!("/e2e/dialog/select/{ID_SIZE}/0"), b"");
    // 並びの外の項目。
    refused(&format!("/e2e/dialog/select/{ID_PAGES}/6"), b"");
    show_page(&server, PAGE_PROFILES);
    // 一つしかないプロファイルは削除できない。ConPTY には場所がない。
    assert!(!control_enabled(&server, ID_PROFILE_REMOVE));
    refused(&format!("/e2e/dialog/click/{ID_PROFILE_REMOVE}"), b"");
    refused(&format!("/e2e/dialog/text/{ID_PROFILE_LOCATION}"), b"x");
    press_cancel(&server);
}

/// プロファイルを足し、名前とコマンドラインを変え、既定にして、次に開くタブから使う。削除すると先頭が既定に戻る。
#[test]
fn profiles_are_added_edited_made_default_and_removed() {
    let server = start_settings_test(&format!("# 残すコメント\n{ONE_PROFILE}"));
    open_settings(&server);
    show_page(&server, PAGE_PROFILES);
    assert_eq!(list_state(&server, ID_PROFILES), (1, Value::from(0)));

    click(&server, ID_PROFILE_ADD);
    assert_eq!(list_state(&server, ID_PROFILES), (2, Value::from(1)));
    let added = control_text(&server, ID_PROFILE_NAME);
    assert!(!added.is_empty());
    assert_eq!(read_controls(&server)["focus"], ID_PROFILE_NAME);
    assert!(control_enabled(&server, ID_PROFILE_REMOVE));
    // もう一つ足すと、名前は重ならない。
    click(&server, ID_PROFILE_ADD);
    assert_ne!(control_text(&server, ID_PROFILE_NAME), added);
    click(&server, ID_PROFILE_REMOVE);
    assert_eq!(list_state(&server, ID_PROFILES), (2, Value::from(1)));

    set_text(&server, ID_PROFILE_NAME, "  two  ");
    set_text(&server, ID_PROFILE_COMMAND, "cmd.exe /k prompt two$G");
    set_text(&server, ID_PROFILE_ICON, "two.png");
    // 既定にすると、選んでいる間は押せない。
    assert!(control_enabled(&server, ID_PROFILE_DEFAULT));
    click(&server, ID_PROFILE_DEFAULT);
    assert!(!control_enabled(&server, ID_PROFILE_DEFAULT));
    press_ok(&server);

    let saved = saved_config(&server);
    assert_eq!(saved.profiles.len(), 2);
    assert_eq!(saved.profiles[1].name, "two", "前後の空白を除く");
    assert_eq!(saved.profiles[1].command, "cmd.exe /k prompt two$G");
    assert_eq!(saved.profiles[1].icon.as_deref(), Some("two.png"));
    assert_eq!(saved.default_profile.as_deref(), Some("two"));
    assert!(config_text(&server).contains("# 残すコメント"));
    let state = server.state();
    assert_eq!(state["profiles"], serde_json::json!(["one", "two"]));

    // 既定のプロファイルで新しいタブが開く。
    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    let state = server.wait_text("two>");
    assert_eq!(state["tabs"][1]["profile"], "two", "{state:#}");
    server.post("/e2e/tab/close", b"");
    server.wait_until("one tab", |state| tab_count(state) == 1);

    // 既定のものを削除すると先頭が既定。
    open_settings(&server);
    show_page(&server, PAGE_PROFILES);
    assert_eq!(list_state(&server, ID_PROFILES), (2, Value::from(1)));
    click(&server, ID_PROFILE_REMOVE);
    press_ok(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.profiles.len(), 1);
    assert_eq!(saved.profiles[0].name, "one");
    assert_eq!(saved.default_profile_index(), 0);
    server.post("/e2e/tab/new", b"");
    let state = server.wait_until("two tabs", |state| tab_count(state) == 2);
    assert_eq!(state["tabs"][1]["profile"], "one", "{state:#}");
}

/// 空の名前、重なった名前、ConPTY の空のコマンドラインは、エラーの文を出してそのプロファイルを選び、書かない。
#[test]
fn profiles_with_empty_or_repeated_values_are_not_saved() {
    let original = format!(
        "{ONE_PROFILE}[[profiles]]\nname = \"two\"\ncommand = \"cmd.exe /k prompt e2e$G\"\n"
    );
    let server = start_settings_test(&original);
    open_settings(&server);
    show_page(&server, PAGE_PROFILES);

    let cases = [
        (ID_PROFILE_NAME, "", "two"),
        (ID_PROFILE_NAME, "   ", "two"),
        (ID_PROFILE_NAME, "one", "two"),
        (ID_PROFILE_COMMAND, "", "cmd.exe /k prompt e2e$G"),
    ];
    for (id, bad, good) in cases {
        select(&server, ID_PROFILES, 1);
        set_text(&server, id, bad);
        // ほかのプロファイルを選んでいても、エラーのあるプロファイルへ戻る。
        select(&server, ID_PROFILES, 0);
        press_apply(&server);
        let dialog = read_controls(&server);
        assert_ne!(control(&dialog, ID_ERROR)["text"], "", "{id} {bad:?}");
        assert_eq!(control(&dialog, ID_PROFILES)["selected"], 1, "{id} {bad:?}");
        assert_eq!(dialog["focus"], id, "{id} {bad:?}");
        assert_eq!(config_text(&server), original, "{id} {bad:?}");
        set_text(&server, id, good);
        press_apply(&server);
        assert_eq!(control_text(&server, ID_ERROR), "");
    }
    press_cancel(&server);
    assert_eq!(config_text(&server), original);
}

/// 不正な入力が見えていないページにあれば、「適用」でも「OK」でも、そのページへ移ってフォーカスする。
#[test]
fn an_invalid_input_on_a_hidden_page_is_shown_and_focused() {
    let original = format!(
        "{ONE_PROFILE}[paste]\nconfirm_size = 8\n\n[background]\nimage_opacity = 0.5\n\n[[background.images]]\npath = 'a.png'\n"
    );
    let server = start_settings_test_with_images(&original, &["a.png"]);
    open_settings(&server);

    let cases = [
        (PAGE_APPEARANCE, ID_SIZE, "12"),
        (PAGE_APPEARANCE, ID_PADDING, "4"),
        (PAGE_BACKGROUND, ID_BACKGROUND_OPACITY, "0.5"),
        (PAGE_BEHAVIOR, ID_PASTE_CONFIRM_SIZE, "8"),
        (PAGE_PROFILES, ID_PROFILE_COMMAND, "cmd.exe /k prompt e2e$G"),
    ];
    for (page, id, good) in cases {
        show_page(&server, page);
        let shown = |server: &Server, how: &str| {
            let dialog = read_controls(server);
            assert_ne!(control(&dialog, ID_ERROR)["text"], "", "{id} {how}");
            assert_eq!(control(&dialog, ID_PAGES)["selected"], page, "{id} {how}");
            assert_eq!(control(&dialog, id)["visible"], true, "{id} {how}");
            assert_eq!(dialog["focus"], id, "{id} {how}");
        };
        set_text(&server, id, "");
        show_page(&server, PAGE_COLORS);
        press_apply(&server);
        shown(&server, "applying");

        show_page(&server, PAGE_COLORS);
        click(&server, ID_OK);
        shown(&server, "pressing OK");
        assert_eq!(open_dialog_count(&server), 1, "{id}");

        set_text(&server, id, good);
        press_apply(&server);
        assert_eq!(control_text(&server, ID_ERROR), "", "{id}");
    }
    press_cancel(&server);
}

/// 接続方式を切り替えると、場所の入力欄が替わり、ブリッジの `command` に Windows のプログラムを書くと警告を出す。警告が
/// あっても保存できる。
#[test]
fn the_connection_method_changes_the_location_and_warns_about_windows_programs() {
    let server = start_settings_test(&format!(
        "{ONE_PROFILE}[[profiles]]\nname = \"bridge\"\ncommand = \"bash -l\"\n"
    ));
    open_settings(&server);
    show_page(&server, PAGE_PROFILES);
    select(&server, ID_PROFILES, 1);
    assert!(!control_enabled(&server, ID_PROFILE_LOCATION));
    let without_warning = read_controls(&server);
    assert_eq!(
        control(&without_warning, ID_PROFILE_COMMAND_WARNING)["text"],
        ""
    );

    // Cygwin: 場所は Cygwin のフォルダ。`cmd` のような Windows のプログラムで警告を出す。
    select(&server, ID_PROFILE_BACKEND, BACKEND_CYGWIN);
    assert!(control_enabled(&server, ID_PROFILE_LOCATION));
    set_text(&server, ID_PROFILE_LOCATION, r"C:\cygwin-of-the-test");
    set_text(&server, ID_PROFILE_COMMAND, "cmd /k");
    let warned = read_controls(&server);
    assert_ne!(control(&warned, ID_PROFILE_COMMAND_WARNING)["text"], "");
    // 警告の高さの分だけ、下のコントロールが下がる。
    assert!(
        number_of(control(&warned, ID_PROFILE_ICON), "y")
            > number_of(control(&without_warning, ID_PROFILE_ICON), "y"),
        "{warned:#}"
    );
    assert_controls_fit(&warned, PAGE_PROFILES);
    press_apply(&server);
    assert_eq!(control_text(&server, ID_ERROR), "");
    let saved = saved_config(&server);
    assert_eq!(saved.profiles[1].backend, BackendKind::Cygwin);
    assert_eq!(saved.profiles[1].command, "cmd /k");
    assert_eq!(
        saved.profiles[1].cygwin_root.as_deref(),
        Some(r"C:\cygwin-of-the-test")
    );

    // ブリッジではコマンドラインが空でよい。警告は消える。
    set_text(&server, ID_PROFILE_COMMAND, "");
    assert_eq!(control_text(&server, ID_PROFILE_COMMAND_WARNING), "");
    assert_eq!(
        number_of(control(&read_controls(&server), ID_PROFILE_ICON), "y"),
        number_of(control(&without_warning, ID_PROFILE_ICON), "y")
    );
    press_apply(&server);
    assert_eq!(control_text(&server, ID_ERROR), "");
    assert_eq!(saved_config(&server).profiles[1].command, "");

    // WSL: 切り替えると場所は空に戻り、ディストリビューションになる。`.exe` で警告を出す。
    select(&server, ID_PROFILE_BACKEND, BACKEND_WSL);
    assert_eq!(control_text(&server, ID_PROFILE_LOCATION), "");
    set_text(&server, ID_PROFILE_LOCATION, "Ubuntu");
    set_text(&server, ID_PROFILE_COMMAND, "docker.exe ps");
    assert_ne!(control_text(&server, ID_PROFILE_COMMAND_WARNING), "");
    press_apply(&server);
    assert_eq!(control_text(&server, ID_ERROR), "");
    let saved = saved_config(&server);
    assert_eq!(saved.profiles[1].backend, BackendKind::Wsl);
    assert_eq!(saved.profiles[1].distribution.as_deref(), Some("Ubuntu"));
    assert_eq!(saved.profiles[1].cygwin_root, None);

    // ConPTY: 場所は押せなくなり、空のコマンドラインはエラー。
    select(&server, ID_PROFILE_BACKEND, BACKEND_CONPTY);
    assert!(!control_enabled(&server, ID_PROFILE_LOCATION));
    assert_eq!(control_text(&server, ID_PROFILE_COMMAND_WARNING), "");
    set_text(&server, ID_PROFILE_COMMAND, "");
    press_apply(&server);
    assert_ne!(control_text(&server, ID_ERROR), "");
    set_text(&server, ID_PROFILE_COMMAND, "cmd.exe /k prompt bridge$G");
    press_ok(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.profiles[1].backend, BackendKind::Conpty);
    assert_eq!(saved.profiles[1].distribution, None);
    assert!(!config_text(&server).contains("distribution"));
}

/// 背景のページの項目の表示方法と不透明度、既定の値が、端末の絵と `[[background.images]]` に入る。
#[test]
fn the_background_page_changes_the_items_and_the_picture() {
    let server = start_with_background(
        &format!(
            "[background]\nimage_opacity = 1.0\nfit = \"stretch\"\n\n# 項目のコメント\n[[background.images]]\npath = '{BANDS_IMAGE}'\n"
        ),
        |_| {},
    );
    let state = server.state();
    let area = BackdropArea::of(&state);
    let top = area.at(0.5, 0.01);
    let bottom = area.at(0.5, 0.99);
    wait_pixels(&server, &[(top, BAND_TOP), (bottom, BAND_BOTTOM)]);

    open_settings(&server);
    show_page(&server, PAGE_BACKGROUND);
    // 選ぶまでは項目の値を触れない。
    assert!(!control_enabled(&server, ID_BACKGROUND_ITEM_FIT));
    assert!(!control_enabled(&server, ID_BACKGROUND_REMOVE));
    select(&server, ID_BACKGROUND_IMAGES, 0);
    assert!(control_enabled(&server, ID_BACKGROUND_ITEM_FIT));
    assert!(!control_enabled(&server, ID_BACKGROUND_ITEM_OPACITY));

    // 既定の不透明度は、打ってから少し待って反映する。
    set_text(&server, ID_BACKGROUND_OPACITY, "0.5");
    wait_pixels(&server, &[(top, over_terminal_background(BAND_TOP, 0.5))]);
    // 項目の不透明度は既定より優先する。
    choose_radio(&server, ID_BACKGROUND_ITEM_OPACITY_CHOICE + 1);
    assert!(control_enabled(&server, ID_BACKGROUND_ITEM_OPACITY));
    set_text(&server, ID_BACKGROUND_ITEM_OPACITY, "0.25");
    wait_pixels(&server, &[(top, over_terminal_background(BAND_TOP, 0.25))]);
    // 項目の表示方法で、元の大きさで真ん中に置く。上の端に画像がなくなる。
    select(&server, ID_BACKGROUND_ITEM_FIT, ITEM_FIT_CENTER);
    wait_pixels(&server, &[(top, TERMINAL_BACKGROUND)]);
    let rows = read_controls(&server);
    let row = &control(&rows, ID_BACKGROUND_IMAGES)["items"][0];
    assert!(
        row.as_str().is_some_and(|row| row.contains("0.25")),
        "{row}"
    );
    press_apply(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.background.image_opacity, 0.5);
    assert_eq!(saved.background.images[0].fit, Some(BackgroundFit::Center));
    assert_eq!(saved.background.images[0].image_opacity, Some(0.25));
    assert!(config_text(&server).contains("# 項目のコメント"));

    // 項目の値を既定に戻すと、テーブルからキーが消え、既定の値で描く。
    select(&server, ID_BACKGROUND_ITEM_FIT, ITEM_FIT_DEFAULT);
    choose_radio(&server, ID_BACKGROUND_ITEM_OPACITY_CHOICE);
    assert!(!control_enabled(&server, ID_BACKGROUND_ITEM_OPACITY));
    assert_eq!(control_text(&server, ID_BACKGROUND_ITEM_OPACITY), "");
    wait_pixels(&server, &[(top, over_terminal_background(BAND_TOP, 0.5))]);
    press_apply(&server);
    let saved = saved_config(&server);
    assert_eq!(
        saved.background.images[0],
        BackgroundImage::new(BANDS_IMAGE)
    );

    // 一覧から外すと画像がなくなる。
    click(&server, ID_BACKGROUND_REMOVE);
    assert_eq!(list_state(&server, ID_BACKGROUND_IMAGES).0, 0);
    assert!(!control_enabled(&server, ID_BACKGROUND_REMOVE));
    server.wait_until("no background image", |state| {
        state["background"]["image"].is_null()
    });
    wait_pixels(&server, &[(top, TERMINAL_BACKGROUND)]);
    press_ok(&server);
    let saved = saved_config(&server);
    assert!(saved.background.images.is_empty());
    assert!(!config_text(&server).contains("[[background.images]]"));
}

/// 背景画像の一覧は、項目ごとの値を項目に入れ、ほかの項目は変えない。
#[test]
fn the_background_list_removes_only_the_chosen_item() {
    let server = start_settings_test_with_images(
        &format!(
            "{ONE_PROFILE}[[background.images]]\npath = 'a.png'\nfit = \"tile\"\n\n[[background.images]]\npath = 'b.png'\n\n[[background.images]]\npath = 'c.png'\nimage_opacity = 0.75\n"
        ),
        &["a.png", "b.png", "c.png"],
    );
    open_settings(&server);
    show_page(&server, PAGE_BACKGROUND);
    assert_eq!(list_state(&server, ID_BACKGROUND_IMAGES).0, 3);
    select(&server, ID_BACKGROUND_IMAGES, 1);
    click(&server, ID_BACKGROUND_REMOVE);
    // 同じ位置の項目を選び続ける。
    let (count, selected) = list_state(&server, ID_BACKGROUND_IMAGES);
    assert_eq!((count, selected), (2, Value::from(1)));
    assert_eq!(
        control(&read_controls(&server), ID_BACKGROUND_ITEM_OPACITY)["text"],
        "0.75"
    );
    press_ok(&server);
    let saved = saved_config(&server);
    let paths: Vec<&str> = saved
        .background
        .images
        .iter()
        .map(|image| image.path.as_str())
        .collect();
    assert_eq!(paths, ["a.png", "c.png"]);
    assert_eq!(saved.background.images[0].fit, Some(BackgroundFit::Tile));
    assert_eq!(saved.background.images[1].image_opacity, Some(0.75));
}

/// OS のファイルを選ぶダイアログは、やめれば何も変えない。
#[test]
fn cancelling_the_file_dialogs_changes_nothing() {
    let original = format!("{ONE_PROFILE}[[background.images]]\npath = 'a.png'\n");
    let server = start_settings_test_with_images(&original, &["a.png"]);
    open_settings(&server);
    for (page, id, list) in [
        (
            PAGE_BACKGROUND,
            ID_BACKGROUND_ADD_FILES,
            ID_BACKGROUND_IMAGES,
        ),
        (
            PAGE_BACKGROUND,
            ID_BACKGROUND_ADD_FOLDER,
            ID_BACKGROUND_IMAGES,
        ),
        (PAGE_PROFILES, ID_PROFILE_ICON_CHOOSE, ID_PROFILES),
    ] {
        show_page(&server, page);
        let before = list_state(&server, list);
        click(&server, id);
        wait_top_dialog(&server, SYSTEM_DIALOG_CLASS);
        click(&server, ID_CANCEL);
        wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
        assert_eq!(list_state(&server, list), before, "{id}");
    }
    assert_eq!(control_text(&server, ID_PROFILE_ICON), "");
    press_ok(&server);
    assert_eq!(config_text(&server), original);
}

/// 環境変数があるときだけ、ファイルを選ぶダイアログで実際に選ぶ。OS がダイアログの履歴（レジストリの
/// `LastVisitedPidlMRU`）に一時フォルダを残すので、手元では付けない（CI で付ける）。
const FILE_DIALOG_ENV: &str = "FANTERMO_E2E_FILE_DIALOG";

/// いちばん上のダイアログの、最初の入力欄（ファイルを選ぶダイアログのファイル名の欄）の ID。
#[track_caller]
fn first_edit_id(server: &Server) -> i32 {
    let dialog = read_controls(server);
    let edit = dialog["controls"]
        .as_array()
        .and_then(|controls| {
            controls
                .iter()
                .find(|control| control["class"] == "Edit" && control["visible"] == true)
        })
        .unwrap_or_else(|| panic!("no edit control: {dialog:#}"));
    i32::try_from(edit["id"].as_i64().expect("no ID")).expect("the ID fits")
}

/// `button` で開くファイルを選ぶダイアログで `path` を選ぶ。
#[track_caller]
fn choose_file(server: &Server, button: i32, path: &std::path::Path) {
    click(server, button);
    wait_top_dialog(server, SYSTEM_DIALOG_CLASS);
    set_text(
        server,
        first_edit_id(server),
        path.to_str().expect("a UTF-8 path"),
    );
    click(server, ID_OK);
    wait_top_dialog(server, SETTINGS_DIALOG_CLASS);
}

/// ファイルを選ぶダイアログで選んだ画像とアイコンが、絶対パスで入る。同じ画像は足さない。
#[test]
fn the_file_dialogs_add_a_background_image_and_a_profile_icon() {
    let test = "the_file_dialogs_add_a_background_image_and_a_profile_icon";
    if std::env::var_os(FILE_DIALOG_ENV).is_none() {
        common::report_skip(test, FILE_DIALOG_ENV);
        return;
    }
    let server = start_settings_test_with_images(ONE_PROFILE, &["added.png"]);
    let image = server.dir().join("added.png");
    open_settings(&server);
    show_page(&server, PAGE_BACKGROUND);
    choose_file(&server, ID_BACKGROUND_ADD_FILES, &image);
    assert_eq!(
        list_state(&server, ID_BACKGROUND_IMAGES),
        (1, Value::from(0))
    );
    choose_file(&server, ID_BACKGROUND_ADD_FILES, &image);
    assert_eq!(list_state(&server, ID_BACKGROUND_IMAGES).0, 1);

    show_page(&server, PAGE_PROFILES);
    choose_file(&server, ID_PROFILE_ICON_CHOOSE, &image);
    press_ok(&server);
    let saved = saved_config(&server);
    // ダイアログは長い名前のパスを返し、一時フォルダは短い名前（`RUNNER~1` など）のことがあるので、
    // 同じファイルを指すかで比べる。
    let same_file = |path: &str| {
        std::fs::canonicalize(path).expect("the written path exists")
            == std::fs::canonicalize(&image).expect("the image exists")
    };
    assert!(
        same_file(&saved.background.images[0].path),
        "{}",
        saved.background.images[0].path
    );
    let icon = saved.profiles[0]
        .icon
        .as_deref()
        .expect("an icon is written");
    assert!(same_file(icon), "{icon}");
    assert!(std::path::Path::new(icon).is_absolute(), "{icon}");
}

/// 色のページの配色と、色を選ぶダイアログ、既定に戻すが、設定ファイルと端末の絵に入る。
#[test]
fn the_colors_page_changes_the_scheme_and_the_colors() {
    let server = start_settings_test(ONE_PROFILE);
    let state = server.state();
    let at = cell_pixel(&state, EMPTY_COL, EMPTY_ROWS_BELOW, 0, 0);
    wait_pixels(&server, &[(at, [0, 0, 0])]);
    let choose = color_control(COLOR_ROW_BACKGROUND, COLOR_CHOOSE);
    let reset = color_control(COLOR_ROW_BACKGROUND, COLOR_RESET);
    let value = color_control(COLOR_ROW_BACKGROUND, COLOR_TEXT);

    open_settings(&server);
    show_page(&server, PAGE_COLORS);
    assert!(!control_enabled(&server, reset));

    // 配色はすぐ反映する。
    choose_radio(&server, ID_COLOR_SCHEME + 1);
    wait_pixels(&server, &[(at, [255, 255, 255])]);

    // 色を選ぶダイアログをやめても変わらない。
    click(&server, choose);
    wait_top_dialog(&server, SYSTEM_DIALOG_CLASS);
    click(&server, ID_CANCEL);
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    assert!(!control_enabled(&server, reset));

    // 選ぶと入る。
    click(&server, choose);
    wait_top_dialog(&server, SYSTEM_DIALOG_CLASS);
    set_text(&server, COLOR_DIALOG_RED, "16");
    set_text(&server, COLOR_DIALOG_GREEN, "32");
    set_text(&server, COLOR_DIALOG_BLUE, "48");
    click(&server, ID_OK);
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    wait_pixels(&server, &[(at, TERMINAL_BACKGROUND)]);
    assert!(control_text(&server, value).contains("#102030"));
    assert!(control_enabled(&server, reset));
    press_apply(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.colors.scheme, ColorScheme::Light);
    assert_eq!(
        saved.colors.terminal[0],
        Some(Rgb {
            r: 16,
            g: 32,
            b: 48
        })
    );

    // 既定に戻すと配色の色になる。
    click(&server, reset);
    wait_pixels(&server, &[(at, [255, 255, 255])]);
    assert!(!control_enabled(&server, reset));
    press_ok(&server);
    assert_eq!(saved_config(&server).colors.terminal[0], None);
    assert!(config_text(&server).contains("# background = \"#102030\""));
}

/// `action` の行を選ぶ。
#[track_caller]
fn select_key_row(server: &Server, keys: &Keymap, action: Action, combo: Option<&str>) {
    select(server, ID_KEYS, key_row_of(keys, action, combo));
}

/// 「追加」か「変更」のボタン `button` を押して、キーを読み取るダイアログで `combo` を押す。
#[track_caller]
fn capture_key(server: &Server, button: i32, combo: &str) {
    click(server, button);
    wait_top_dialog(server, KEY_CAPTURE_CLASS);
    press_key_in_dialog(server, combo);
}

/// Alt を含む F4 は断って、読み取りのダイアログを開いたまま待つ。割り当ては変わらない。
#[test]
fn the_key_capture_refuses_alt_f4_and_keeps_waiting() {
    let server = start_settings_test(ONE_PROFILE);
    open_settings(&server);
    show_page(&server, PAGE_KEYS);
    let default = Keymap::default();
    let rows = list_state(&server, ID_KEYS).0;

    select_key_row(&server, &default, Action::NewTab, None);
    capture_key(&server, ID_KEY_ADD, "alt+f4");
    let state = server.wait_until("the refusal in the key capture dialog", |state| {
        state["dialogs"][0]["class"] == KEY_CAPTURE_CLASS
            && dialog_text(&state["dialogs"][0]).contains("割り当てられません")
    });
    // 断る文のあとにも、やめかたの案内が残る。
    assert!(dialog_text(&state["dialogs"][0]).contains("Esc"));

    // 読み取りは続いていて、Esc でやめられる。
    press_key_in_dialog(&server, "escape");
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    assert_eq!(list_state(&server, ID_KEYS).0, rows);
}

/// キーの割り当てを足し、変え、外し、既定に戻す。Esc なら読み取りをやめる。
#[test]
fn the_keys_page_adds_changes_removes_and_resets_bindings() {
    let server = start_settings_test(ONE_PROFILE);
    open_settings(&server);
    show_page(&server, PAGE_KEYS);
    let default = Keymap::default();
    let rows = list_state(&server, ID_KEYS).0;

    // やめても変わらない。
    select_key_row(&server, &default, Action::NewTab, None);
    capture_key(&server, ID_KEY_ADD, "escape");
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    assert_eq!(list_state(&server, ID_KEYS).0, rows);

    // 修飾キーのある組み合わせを足す。
    capture_key(&server, ID_KEY_ADD, "ctrl+shift+n");
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    assert_eq!(list_state(&server, ID_KEYS).0, rows + 1);
    assert!(control_enabled(&server, ID_KEY_RESET));
    press_ok(&server);
    let keys = saved_config(&server).keys;
    let new_combo = KeyCombo::parse("ctrl+shift+n").expect("a valid combination");
    assert_eq!(keys.lookup_combo(new_combo), Some(Action::NewTab));
    assert!(config_text(&server).contains("\"ctrl+shift+n\" = \"new_tab\""));

    // 足した組み合わせは端末で動く。
    server.press("ctrl+shift+n");
    wait_tabs(&server, 2);
    server.press("ctrl+shift+w");
    server.wait_until("one tab", |state| tab_count(state) == 1);

    // 修飾キーのない組み合わせに変える。
    open_settings(&server);
    show_page(&server, PAGE_KEYS);
    select_key_row(&server, &keys, Action::NewTab, Some("ctrl+shift+n"));
    capture_key(&server, ID_KEY_CHANGE, "f5");
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    press_apply(&server);
    let keys = saved_config(&server).keys;
    assert_eq!(keys.lookup_combo(new_combo), None);
    let f5 = KeyCombo::parse("f5").expect("a valid combination");
    assert_eq!(keys.lookup_combo(f5), Some(Action::NewTab));
    assert!(!config_text(&server).contains("ctrl+shift+n"));

    // 外す。組み合わせがなくなると、変更と外すは押せない。
    select_key_row(&server, &keys, Action::CloseTab, Some("ctrl+shift+w"));
    click(&server, ID_KEY_REMOVE);
    assert!(!control_enabled(&server, ID_KEY_CHANGE));
    assert!(!control_enabled(&server, ID_KEY_REMOVE));
    press_apply(&server);
    let close_combo = KeyCombo::parse("ctrl+shift+w").expect("a valid combination");
    assert_eq!(saved_config(&server).keys.lookup_combo(close_combo), None);
    assert!(config_text(&server).contains("\"ctrl+shift+w\" = \"none\""));

    // 既定に戻す。動作ごとに戻し、戻したら押せない。
    let keys = saved_config(&server).keys;
    select_key_row(&server, &keys, Action::CloseTab, None);
    assert!(control_enabled(&server, ID_KEY_RESET));
    click(&server, ID_KEY_RESET);
    assert!(!control_enabled(&server, ID_KEY_RESET));
    select_key_row(&server, &saved_config(&server).keys, Action::NewTab, None);
    click(&server, ID_KEY_RESET);
    press_ok(&server);
    let keys = saved_config(&server).keys;
    assert_eq!(keys, Keymap::default());
    assert!(!config_text(&server).contains("f5"));
    assert!(config_text(&server).contains("\"ctrl+shift+w\""));
    server.press("ctrl+shift+t");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
}

/// ほかの動作に割り当てた組み合わせは、割り当て直すかを尋ね、「いいえ」なら変えず、「はい」なら移す。
#[test]
fn a_key_bound_to_another_action_asks_before_moving_it() {
    let server = start_settings_test(ONE_PROFILE);
    open_settings(&server);
    show_page(&server, PAGE_KEYS);
    let default = Keymap::default();
    let close_combo = KeyCombo::parse("ctrl+shift+w").expect("a valid combination");
    let rows = list_state(&server, ID_KEYS).0;
    select_key_row(&server, &default, Action::NewTab, None);

    capture_key(&server, ID_KEY_ADD, "ctrl+shift+w");
    wait_message_box(&server);
    // メッセージボックスが開いている間は、設定ダイアログは無効。
    assert_eq!(read_controls(&server)["class"], SYSTEM_DIALOG_CLASS);
    click(&server, ID_NO);
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    assert_eq!(list_state(&server, ID_KEYS).0, rows);
    press_apply(&server);
    assert_eq!(saved_config(&server).keys, Keymap::default());

    // 同じ動作の同じ組み合わせは尋ねない。
    capture_key(&server, ID_KEY_ADD, "ctrl+shift+t");
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    assert_eq!(list_state(&server, ID_KEYS).0, rows);

    capture_key(&server, ID_KEY_ADD, "ctrl+shift+w");
    wait_message_box(&server);
    click(&server, ID_YES);
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    press_ok(&server);
    let keys = saved_config(&server).keys;
    assert_eq!(keys.lookup_combo(close_combo), Some(Action::NewTab));
    assert!(keys.combos_of(Action::CloseTab).is_empty());

    // 移した組み合わせで、タブが開く。
    server.press("ctrl+shift+w");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
}

/// ダイアログの間も端末の出力を処理して描き、閉じたあとも入力できる。ダイアログの間、ウィンドウは無効になる。
#[test]
fn the_terminal_keeps_working_while_the_settings_dialog_is_open() {
    let server = start_settings_test(ONE_PROFILE);
    open_settings(&server);
    assert_eq!(server.state()["window_enabled"], false);

    server.send_line("echo during-%ERRORLEVEL%-dialog");
    let state = server.wait_text("during-0-dialog");
    let (col, row) = find_cell(&state, "during-0-dialog");
    assert_cell_is_drawn(&server, &state, col, row);

    // 裏のタブの出力も届く。
    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    server.wait_text(CMD_PROMPT);
    server.post("/e2e/tab/select/0", b"");
    server.wait_text("during-0-dialog");

    press_cancel(&server);
    let state = server.state();
    assert_eq!(state["window_enabled"], true, "{state:#}");
    check_terminal_still_works(&server, "after-settings");
}

/// ダイアログの間に最後のシェルが終わると、ダイアログごとウィンドウが閉じる。
#[test]
fn a_shell_ending_while_the_settings_dialog_is_open_closes_the_window() {
    let mut server = start_settings_test(ONE_PROFILE);
    open_settings(&server);
    server.send_line("exit");
    server.wait_exit();
}

/// 開いている間に別のプログラムが書き換えた設定は、キャンセルで閉じたあとに読み直す。
#[test]
fn a_file_changed_while_the_dialog_is_open_is_read_after_closing() {
    let server = start_settings_test(&format!("{ONE_PROFILE}[window]\npadding = 4\n"));
    let first_x = number(&server.state()["terminal_x"]);
    open_settings(&server);
    std::fs::write(
        config_path(&server),
        format!("{ONE_PROFILE}[window]\npadding = 10\n"),
    )
    .expect("failed to write the settings");
    let unchanged = Duration::from_millis(600);
    server.assert_holds_for(unchanged, "the padding before closing", |state| {
        number(&state["terminal_x"]) == first_x
    });
    press_cancel(&server);
    let expected = first_x + 6;
    server.wait_until("the padding of the file", |state| {
        number(&state["terminal_x"]) == expected
    });
}

/// 開いている間に別のプログラムが書き換えた設定は、保存したときも閉じたあとに反映する。ダイアログで変えた項目も残る。
#[test]
fn a_file_changed_while_the_dialog_is_open_is_read_after_saving() {
    let server = start_settings_test(&format!("{ONE_PROFILE}[window]\npadding = 4\n"));
    let first_x = number(&server.state()["terminal_x"]);

    // ダイアログで何も変えずに「OK」を押す。
    open_settings(&server);
    std::fs::write(
        config_path(&server),
        format!("{ONE_PROFILE}[window]\npadding = 10\n"),
    )
    .expect("failed to write the settings");
    press_ok(&server);
    server.wait_until("the padding of the file", |state| {
        number(&state["terminal_x"]) == first_x + 6
    });

    // ダイアログでほかの項目を変えて「OK」を押す。
    let bar = number(&server.state()["scroll_bar"]["width"]);
    open_settings(&server);
    std::fs::write(
        config_path(&server),
        format!("{ONE_PROFILE}[window]\npadding = 12\n"),
    )
    .expect("failed to write the settings");
    set_text(&server, ID_SCROLL_BAR_WIDTH, &(bar + 5).to_string());
    press_ok(&server);
    server.wait_until("the padding of the file and the width", |state| {
        number(&state["terminal_x"]) == first_x + 8
            && number(&state["scroll_bar"]["width"]) == bar + 5
    });
    let saved = saved_config(&server);
    assert_eq!(saved.window.padding, 12);
    assert_eq!(u64::from(saved.scroll_bar.width), bar + 5);

    // 「適用」のあとは、開いたままで反映せず、「キャンセル」で閉じたあとに反映する。
    open_settings(&server);
    press_apply(&server);
    std::fs::write(
        config_path(&server),
        format!("{ONE_PROFILE}[window]\npadding = 14\n"),
    )
    .expect("failed to write the settings");
    server.assert_holds_for(Duration::from_millis(600), "the padding", |state| {
        number(&state["terminal_x"]) == first_x + 8
    });
    press_cancel(&server);
    server.wait_until("the padding of the file", |state| {
        number(&state["terminal_x"]) == first_x + 10
    });
}

/// 設定ファイルの警告を、続けて出さない時間の窓。
const NO_REPEATED_WARNING: Duration = Duration::from_millis(600);

/// メッセージボックスが開かないことを確認する。
#[track_caller]
fn assert_no_warning_opens(server: &Server) {
    server.assert_holds_for(NO_REPEATED_WARNING, "no warning", |state| {
        state["dialogs"].as_array().map_or(0, Vec::len) == 0
    });
}

/// 同じ警告は、保存しても別のプログラムが書き換えても続けて出さない。中身が変わったら出し、直ったあとにまた
/// 同じ警告になれば出す。
#[test]
fn the_same_settings_warning_is_not_shown_again() {
    let first = format!("unknown_one = 1\n{ONE_PROFILE}");
    let server = Server::start(&first, &[]);
    assert!(wait_message_box(&server).contains("unknown_one"));
    close_message_box(&server, 0);
    server.wait_text(CMD_PROMPT);

    // 「OK」のたびに出さない。項目を書いて行がずれても、中身は同じ。
    open_settings(&server);
    press_ok(&server);
    assert_no_warning_opens(&server);
    open_settings(&server);
    set_text(&server, ID_PADDING, "9");
    press_ok(&server);
    assert_no_warning_opens(&server);
    assert_eq!(saved_config(&server).window.padding, 9);
    open_settings(&server);
    press_apply(&server);
    press_cancel(&server);
    assert_no_warning_opens(&server);

    // 別のプログラムが書き換えても、警告が同じなら出さない。
    let padded = number(&server.state()["terminal_x"]) + 1;
    std::fs::write(
        config_path(&server),
        format!("# a comment\n\nunknown_one = 1\n{ONE_PROFILE}[window]\npadding = 10\n"),
    )
    .expect("failed to write the settings");
    server.wait_until("the changed padding", |state| {
        number(&state["terminal_x"]) == padded
    });
    assert_no_warning_opens(&server);

    // 中身が変わったら出す。
    std::fs::write(
        config_path(&server),
        format!("unknown_two = 1\n{ONE_PROFILE}"),
    )
    .expect("failed to write the settings");
    assert!(wait_message_box(&server).contains("unknown_two"));
    close_message_box(&server, 0);

    // ダイアログの間に中身が変わったら、閉じたあとに出す。
    open_settings(&server);
    std::fs::write(
        config_path(&server),
        format!("unknown_three = 1\n{ONE_PROFILE}"),
    )
    .expect("failed to write the settings");
    click(&server, ID_OK);
    assert!(wait_message_box(&server).contains("unknown_three"));
    close_message_box(&server, 0);

    // 直ったあとに同じ警告になれば出す。
    std::fs::write(config_path(&server), ONE_PROFILE).expect("failed to write the settings");
    let width = number(&server.state()["scroll_bar"]["width"]);
    open_settings(&server);
    set_text(&server, ID_SCROLL_BAR_WIDTH, &(width + 1).to_string());
    press_ok(&server);
    server.wait_until("the changed width", |state| {
        number(&state["scroll_bar"]["width"]) == width + 1
    });
    std::fs::write(
        config_path(&server),
        format!("unknown_three = 1\n{ONE_PROFILE}"),
    )
    .expect("failed to write the settings");
    assert!(wait_message_box(&server).contains("unknown_three"));
    close_message_box(&server, 0);
}

/// 読めない設定ファイルは、起動時にメッセージで知らせて既定の設定で動く。
#[test]
fn a_settings_file_that_cannot_be_read_is_reported_at_startup() {
    let server = Server::start_with_setup("", &CMD_COMMAND, |dir| {
        let path = dir.join("config.toml");
        std::fs::remove_file(&path).expect("failed to remove the settings");
        std::fs::create_dir(&path).expect("failed to create the folder");
    });
    let text = wait_message_box(&server);
    assert!(text.contains("config.toml"), "{text}");
    close_message_box(&server, 0);
    server.wait_text(CMD_PROMPT);
    check_terminal_still_works(&server, "after-the-message");
}

/// `config.toml` を、`share` で共有して開いたまま `body` を実行する。
fn with_settings_file_open<T>(server: &Server, share: u32, body: impl FnOnce() -> T) -> T {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(share)
        .open(config_path(server))
        .expect("failed to open the settings");
    let result = body();
    drop(file);
    result
}

/// メッセージが開き、設定ダイアログは開いたまま、ファイルと、一時ファイルが残らないことを確認して閉じる。
#[track_caller]
fn assert_save_failed(server: &Server) {
    let text = wait_message_box(server);
    assert!(text.contains("config.toml"), "{text}");
    close_message_box(server, 1);
    wait_top_dialog(server, SETTINGS_DIALOG_CLASS);
    let leftovers: Vec<_> = std::fs::read_dir(server.dir())
        .expect("failed to list the settings folder")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// 読めない・書けないファイルへの保存はメッセージを出し、ダイアログは開いたままで、やり直せる。
#[test]
fn saving_to_a_file_that_cannot_be_read_or_written_shows_a_message_and_can_be_retried() {
    /// 共有モード: 共有しない（読めない）。
    const SHARE_NONE: u32 = 0;
    /// 共有モード: 読むことだけ共有する（読めるが、置き換えられない）。
    const SHARE_READ: u32 = 1;
    let original = format!("{ONE_PROFILE}[window]\npadding = 4\n");
    let server = start_settings_test(&original);
    open_settings(&server);
    set_text(&server, ID_PADDING, "10");

    for share in [SHARE_NONE, SHARE_READ] {
        with_settings_file_open(&server, share, || {
            click(&server, ID_APPLY);
            assert_save_failed(&server);
        });
        assert_eq!(config_text(&server), original, "{share}");
        // 端末には編集中の値を見せたまま。
        assert_eq!(control_text(&server, ID_PADDING), "10");
    }

    // 読み取り専用のファイルは置き換えられない。
    let path = config_path(&server);
    let set_read_only = |read_only: bool| {
        let mut permissions = std::fs::metadata(&path)
            .expect("failed to read the attributes")
            .permissions();
        permissions.set_readonly(read_only);
        std::fs::set_permissions(&path, permissions).expect("failed to set the attributes");
    };
    set_read_only(true);
    click(&server, ID_APPLY);
    assert_save_failed(&server);
    set_read_only(false);
    assert_eq!(config_text(&server), original);

    // 開けるようになれば入る。
    press_apply(&server);
    assert_eq!(saved_config(&server).window.padding, 10);

    // TOML として読めないときも同じ。閉じたあとの読み直しでも知らせる。
    std::fs::write(config_path(&server), "[font\n").expect("failed to write the settings");
    set_text(&server, ID_PADDING, "12");
    click(&server, ID_APPLY);
    assert_save_failed(&server);
    assert_eq!(config_text(&server), "[font\n");
    click(&server, ID_CANCEL);
    wait_message_box(&server);
    close_message_box(&server, 0);
    std::fs::write(config_path(&server), &original).expect("failed to write the settings");
    let expected = number(&server.state()["terminal_x"]) - 6;
    server.wait_until("the padding of the repaired file", |state| {
        number(&state["terminal_x"]) == expected
    });
}

/// 開いている間に消された設定ファイルは、ひな形に変えた項目を書いて作る。
#[test]
fn saving_after_the_file_was_removed_creates_it_from_the_template() {
    let server = start_settings_test(&format!("{ONE_PROFILE}[window]\npadding = 4\n"));
    open_settings(&server);
    std::fs::remove_file(config_path(&server)).expect("failed to remove the settings");
    set_text(&server, ID_PADDING, "10");
    press_ok(&server);
    let saved = saved_config(&server);
    assert_eq!(saved.window.padding, 10);
    assert!(config_text(&server).contains("[colors.tab_bar]"));
}
