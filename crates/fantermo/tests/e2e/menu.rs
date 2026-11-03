use crate::common::CMD_COMMAND;
use crate::common::CMD_PROMPT;
use crate::common::Server;
use crate::ui::{
    BUTTON_CANCEL, BUTTON_OK, TERMINAL_BACKGROUND_SETTING, accelerator, area_center,
    assert_cell_is_drawn, button_click, cell_left, cell_right, check_terminal_still_works,
    click_main_menu, close_menu, echo_lines, find_cell, highlight_position, left_click, left_drag,
    menu_position, on_blank_of_tab_bar, on_scroll_bar, on_tab, open_menu, read_menu,
    right_click_terminal, scroll_bar_middle, select_item, select_position, selection, tab_count,
    wait_menu, wait_tabs, wait_text_dialog,
};
use serde_json::Value;

/// コンテキストメニューの並び。形は [`menu_shape`]。
const CONTEXT_MENU: &str = "C- P | A | T W";
const CONTEXT_MENU_WITH_SELECTION: &str = "C P | A | T W";
/// ≡ のメニューの並び。
const MAIN_MENU: &str = "T W | C- P A | S F B | X";
const MAIN_MENU_WITH_SELECTION: &str = "T W | C P A | S F B | X";

/// メニューの並びの短い形。項目はアクセラレータ（なければ `.`）で、押せなければ `-`、印があれば `*` を続け、区切りは
/// `|`。項目の文字の言い回しによらず、順番と押せるかを比べる。
fn menu_shape(items: &[Value]) -> String {
    let shapes: Vec<String> = items
        .iter()
        .map(|item| {
            if item["separator"] == true {
                return "|".to_owned();
            }
            let mut shape = accelerator(item).unwrap_or('.').to_string();
            if item["enabled"] != true {
                shape.push('-');
            }
            if item["checked"] == true {
                shape.push('*');
            }
            shape
        })
        .collect();
    shapes.join(" ")
}

/// 項目の表示の、キーの組み合わせの前の部分から、アクセラレータの印を除いたもの。
fn item_name(item: &Value) -> String {
    let text = item["text"].as_str().unwrap_or_default();
    let name = text.split('\t').next().unwrap_or_default();
    let mut plain = String::new();
    let mut chars = name.chars();
    while let Some(c) = chars.next() {
        if c == '&' {
            plain.extend(chars.next());
        } else {
            plain.push(c);
        }
    }
    plain
}

/// 項目に添えたキーの組み合わせ。
fn item_shortcut(item: &Value) -> Option<&str> {
    item["text"]
        .as_str()?
        .split_once('\t')
        .map(|(_, keys)| keys)
}

/// `accelerator_of` のアクセラレータの項目。
#[track_caller]
fn menu_item(items: &[Value], accelerator_of: char) -> &Value {
    &items[menu_position(items, accelerator_of)]
}

/// メニューのループへキーを送る。
fn press_menu_key(server: &Server, key: &str) {
    server.post(&format!("/e2e/menu/key/{key}"), b"");
}

/// どの開き方でもコンテキストメニューの並びが同じで、コピーだけが選択範囲に従う。スクロールバーの上の右クリックでは
/// 開かない。
#[test]
fn the_context_menu_has_the_same_items_wherever_it_opens_and_copy_follows_the_selection() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = echo_lines(&server, "alpha", "other");
    let blank_of_tab_bar = on_blank_of_tab_bar(&state);
    for steps in [
        right_click_terminal(&state),
        button_click("right", on_tab(&state, 0), ""),
        button_click("right", blank_of_tab_bar, ""),
    ] {
        let items = open_menu(&server, &steps);
        assert_eq!(menu_shape(&items), CONTEXT_MENU, "{steps}: {items:#?}");
        close_menu(&server);
    }
    for key in ["context_menu", "shift+f10"] {
        server.press(key);
        wait_menu(&server);
        let items = read_menu(&server);
        assert_eq!(menu_shape(&items), CONTEXT_MENU, "{key}: {items:#?}");
        close_menu(&server);
    }

    // 選択範囲があるときは、コピーが押せる。
    let (col, row) = find_cell(&state, "alpha-0-end");
    server.mouse(&left_drag(
        cell_left(&state, col, row),
        cell_right(&state, col + 4, row),
        "",
    ));
    let items = open_menu(&server, &right_click_terminal(&state));
    assert_eq!(
        menu_shape(&items),
        CONTEXT_MENU_WITH_SELECTION,
        "{items:#?}"
    );
    close_menu(&server);
    assert_eq!(selection(&server.state()), Some("alpha"));

    server.mouse(&button_click(
        "right",
        on_scroll_bar(&state, scroll_bar_middle(&state)),
        "",
    ));
    let after = server.state();
    assert_eq!(
        after["dialogs"].as_array().map(Vec::len),
        Some(0),
        "{after:#}"
    );
}

/// メニューの要求は、閉じているときと項目でない位置を断る。断っても開いたまま。
#[test]
fn menu_requests_are_refused_without_a_menu_or_an_item() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);
    for (method, path) in [
        ("GET", "/e2e/menu"),
        ("POST", "/e2e/menu/highlight/0"),
        ("POST", "/e2e/menu/select/0"),
        ("POST", "/e2e/menu/key/escape"),
        ("POST", "/e2e/menu/close"),
    ] {
        let (status, _) = server.request(method, path, b"");
        assert_eq!(status, 400, "{path}");
    }

    let items = open_menu(&server, &click_main_menu(&state));
    let separator = items
        .iter()
        .position(|item| item["separator"] == true)
        .expect("no separator");
    for position in [separator, items.len()] {
        for action in ["highlight", "select"] {
            let path = format!("/e2e/menu/{action}/{position}");
            let (status, _) = server.request("POST", &path, b"");
            assert_eq!(status, 400, "{path}");
        }
    }
    wait_menu(&server);
    close_menu(&server);
}

/// ≡ のメニューの並び。項目に添えるキーは割り当てから決まる。
#[test]
fn the_main_menu_has_its_items_in_order_and_copy_follows_the_selection() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let state = echo_lines(&server, "alpha", "other");

    let items = open_menu(&server, &click_main_menu(&state));
    assert_eq!(menu_shape(&items), MAIN_MENU, "{items:#?}");
    assert!(
        items
            .iter()
            .all(|item| item["submenu"] == false && item["highlighted"] == false),
        "{items:#?}"
    );
    for (key, shortcut) in [
        ('T', Some("Ctrl+Shift+T")),
        ('W', Some("Ctrl+Shift+W")),
        ('C', Some("Ctrl+C")),
        ('P', Some("Shift+Insert")),
        ('A', None),
        ('S', None),
        ('X', None),
    ] {
        assert_eq!(
            item_shortcut(menu_item(&items, key)),
            shortcut,
            "{items:#?}"
        );
    }
    close_menu(&server);

    let (col, row) = find_cell(&state, "alpha-0-end");
    server.mouse(&left_drag(
        cell_left(&state, col, row),
        cell_right(&state, col + 4, row),
        "",
    ));
    let items = open_menu(&server, &click_main_menu(&state));
    assert_eq!(menu_shape(&items), MAIN_MENU_WITH_SELECTION, "{items:#?}");
    close_menu(&server);
}

/// 押せないコピーで Enter を押すと、何も選ばずにメニューが閉じ、クリップボードは変わらない。メニューのコピーは
/// 選択範囲をクリップボードへ、貼り付けはクリップボードを端末へ送る（空なら何も送らない）。すべて選択は全体を選ぶ。
#[test]
fn copy_paste_and_select_all_in_the_menus_work() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);
    server.post("/e2e/clipboard", b"before-the-menu");
    for steps in [click_main_menu(&state), right_click_terminal(&state)] {
        let items = open_menu(&server, &steps);
        select_item(&server, &items, 'C');
        server.wait_dialogs(0);
        server.wait_clipboard("before-the-menu");
    }
    check_terminal_still_works(&server, "after-grayed");

    server.post("/e2e/clipboard", b"");
    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'P');
    server.wait_dialogs(0);
    check_terminal_still_works(&server, "after-empty-paste");

    let state = echo_lines(&server, "alpha", "other");
    let (col, row) = find_cell(&state, "alpha-0-end");
    let select = left_drag(
        cell_left(&state, col, row),
        cell_right(&state, col + 4, row),
        "",
    );

    server.mouse(&select);
    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'C');
    server.wait_clipboard("alpha");
    server.wait_dialogs(0);

    server.post("/e2e/clipboard", b"echo pasted-%ERRORLEVEL%-menu");
    let items = open_menu(&server, &right_click_terminal(&state));
    select_item(&server, &items, 'P');
    server.wait_dialogs(0);
    server.wait_text("echo pasted-");
    server.press("enter");
    server.wait_text("pasted-0-menu");

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'A');
    let selected = server.wait_until("everything selected", |state| {
        selection(state).is_some_and(|text| text.contains("other-0-end"))
    });
    assert!(
        selection(&selected).is_some_and(|text| text.contains("pasted-0-menu")),
        "{selected:#}"
    );

    // 選択したあとのコピーは、コンテキストメニューからも押せる。
    let items = open_menu(&server, &right_click_terminal(&state));
    assert_eq!(
        menu_shape(&items),
        CONTEXT_MENU_WITH_SELECTION,
        "{items:#?}"
    );
    select_item(&server, &items, 'C');
    server.wait_dialogs(0);
    let (status, body) = server.request("GET", "/e2e/clipboard", b"");
    assert_eq!(status, 200);
    let clipboard: Value = serde_json::from_slice(&body).expect("the answer is not JSON");
    assert!(
        clipboard["text"]
            .as_str()
            .is_some_and(|text| text.contains("other-0-end")),
        "{clipboard:#}"
    );

    // コンテキストメニューの「すべて選択」と、≡ の「貼り付け」。
    server.post("/e2e/clipboard", b"echo main-%ERRORLEVEL%-paste");
    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'P');
    server.wait_text("echo main-");
    assert_eq!(selection(&server.state()), None);
    server.press("enter");
    server.wait_text("main-0-paste");
    let items = open_menu(&server, &right_click_terminal(&state));
    assert_eq!(menu_shape(&items), CONTEXT_MENU, "{items:#?}");
    select_item(&server, &items, 'A');
    server.wait_until("everything selected", |state| {
        selection(state).is_some_and(|text| text.contains("main-0-paste"))
    });
    check_terminal_still_works(&server, "after-copy");
}

/// メニューを開いている間にシェルが終わってもタブは閉じ、メニューの操作は残ったタブに効く。最後のシェルが終わると、
/// メニューが開いていてもウィンドウは閉じ、シェルの終了コードで終わる。
#[test]
fn a_shell_ending_while_a_menu_is_open_closes_its_tab_and_then_the_window() {
    let mut server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    server.post("/e2e/tab/new", b"");
    let state = wait_tabs(&server, 2);

    let items = open_menu(&server, &click_main_menu(&state));
    server.post("/e2e/input", b"exit\r");
    let state = wait_tabs(&server, 1);
    wait_menu(&server);
    select_item(&server, &items, 'T');
    let reopened = wait_tabs(&server, 2);
    assert_eq!(reopened["active"], 1, "{reopened:#}");
    server.post("/e2e/tab/close", b"");
    wait_tabs(&server, 1);

    open_menu(&server, &click_main_menu(&state));
    server.post("/e2e/input", b"exit 7\r");
    assert_eq!(server.wait_exit(), 7);
}

/// メニューの貼り付けも、設定に従って編集のダイアログを通す。
#[test]
fn a_paste_from_the_menu_is_confirmed_like_any_other() {
    let server = Server::start("[paste]\nconfirm = \"multiline\"\n", &CMD_COMMAND);
    let state = server.wait_text(CMD_PROMPT);
    server.post("/e2e/clipboard", b"echo from-%ERRORLEVEL%-menu\n");

    let items = open_menu(&server, &right_click_terminal(&state));
    select_item(&server, &items, 'P');
    wait_text_dialog(&server);
    server.press_dialog_button(BUTTON_CANCEL);
    server.wait_dialogs(0);

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'P');
    wait_text_dialog(&server);
    server.press_dialog_button(BUTTON_OK);
    server.wait_text("from-0-menu");
}

/// メニューの「新しいタブ」「タブを閉じる」。どちらのメニューでも動く。≡ の「終了」は、タブがいくつあってもウィンドウを
/// 閉じる。
#[test]
fn tabs_open_and_close_from_the_menus() {
    let mut server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'T');
    let state = wait_tabs(&server, 2);
    assert_eq!(state["active"], 1, "{state:#}");

    let items = open_menu(&server, &right_click_terminal(&state));
    select_item(&server, &items, 'T');
    let state = wait_tabs(&server, 3);
    assert_eq!(state["active"], 2, "{state:#}");

    let items = open_menu(&server, &right_click_terminal(&state));
    select_item(&server, &items, 'W');
    let state = wait_tabs(&server, 2);
    assert_eq!(state["active"], 1, "{state:#}");

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'W');
    let state = wait_tabs(&server, 1);
    assert_eq!(state["active"], 0, "{state:#}");

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'T');
    let state = wait_tabs(&server, 2);

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'X');
    assert_eq!(server.wait_exit(), 0);
}

/// メニューのループへ送ったキーで動く。アクセラレータで選び、Esc で閉じる。
#[test]
fn the_keys_of_a_menu_choose_items_and_escape_closes_it() {
    let server = Server::start_cmd();
    let state = server.wait_text(CMD_PROMPT);

    open_menu(&server, &click_main_menu(&state));
    press_menu_key(&server, "escape");
    server.wait_dialogs(0);
    assert_eq!(tab_count(&server.state()), 1);

    open_menu(&server, &click_main_menu(&state));
    press_menu_key(&server, "t");
    let state = wait_tabs(&server, 2);

    // 矢印のキーは下へも上へも動き、区切りを飛ばし、押せない項目でも止まる。
    let items = open_menu(&server, &click_main_menu(&state));
    highlight_position(&server, menu_position(&items, 'X'));
    highlight_position(&server, menu_position(&items, 'C'));
    close_menu(&server);
}

/// メニューを開いている間もシェルの出力を読み、入力も届く。閉じたあとは入力と描画が続く。
#[test]
fn the_terminal_keeps_working_during_and_after_a_menu() {
    let server = Server::start(TERMINAL_BACKGROUND_SETTING, &CMD_COMMAND);
    let state = server.wait_text(CMD_PROMPT);
    for (index, steps) in [
        click_main_menu(&state),
        left_click(area_center(&state["tab_bar"]["profiles"])),
        right_click_terminal(&state),
    ]
    .into_iter()
    .enumerate()
    {
        open_menu(&server, &steps);
        let during = format!("during{index}");
        server.post(
            "/e2e/input",
            format!("echo {during}-%ERRORLEVEL%-end\r").as_bytes(),
        );
        let needle = format!("{during}-0-end");
        let state = server.wait_text(&needle);
        let (col, row) = find_cell(&state, &needle);
        assert_cell_is_drawn(&server, &state, col, row);
        wait_menu(&server);
        close_menu(&server);
        check_terminal_still_works(&server, &format!("after{index}"));
    }
}

/// プロファイルの一覧は設定の名前の順で、既定に印が付き、絵が付く。どのボタンからでも同じ。選ぶと、fantermo の引数が
/// あってもそのプロファイルで開く。
#[test]
fn the_profile_menu_lists_the_profiles_and_opens_the_chosen_one() {
    let config = r#"default_profile = "two"

[[profiles]]
name = "one"
command = "cmd.exe /k prompt one$G"

[[profiles]]
name = "two"
command = "cmd.exe /k prompt two$G"

[[profiles]]
name = "R&D"
command = "cmd.exe /k prompt three$G"
"#;
    let server = Server::start(config, &CMD_COMMAND);
    let state = server.wait_text(CMD_PROMPT);
    for steps in [
        left_click(area_center(&state["tab_bar"]["profiles"])),
        button_click("right", area_center(&state["tab_bar"]["profiles"]), ""),
        button_click("right", area_center(&state["tab_bar"]["new_tab"]), ""),
    ] {
        let items = open_menu(&server, &steps);
        assert_eq!(menu_shape(&items), "1 2* 3", "{steps}: {items:#?}");
        let names: Vec<String> = items.iter().map(item_name).collect();
        assert_eq!(names, ["1 one", "2 two", "3 R&D"], "{items:#?}");
        assert!(
            items
                .iter()
                .all(|item| item["icon"] == true && item_shortcut(item).is_none()),
            "{items:#?}"
        );
        close_menu(&server);
    }

    let items = open_menu(
        &server,
        &left_click(area_center(&state["tab_bar"]["profiles"])),
    );
    select_item(&server, &items, '3');
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    let state = server.wait_text("three>");
    assert_eq!(state["tabs"][1]["profile"], "R&D", "{state:#}");
    assert_eq!(state["active"], 1, "{state:#}");
}

/// 設定ファイルを変えると、次に開く一覧が変わる。一覧を開いている間に変わって位置が並びの外になったら、先頭のプロファイルで
/// 開く。「新しいタブ」は既定のプロファイルで開く。プロファイルを書いていなければ、既定のプロファイルが一つだけ並ぶ。
/// 10 個目からは数字のアクセラレータがなく、選ぶのは矢印のキーと Enter で届く。
#[test]
fn the_profile_menu_follows_the_settings_file() {
    let two_profiles = r#"default_profile = "first"

[[profiles]]
name = "first"
command = "cmd.exe /k prompt first$G"

[[profiles]]
name = "second"
command = "cmd.exe /k prompt second$G"
"#;
    let renamed = r#"default_profile = "added"

[[profiles]]
name = "first"
command = "cmd.exe /k prompt first$G"

[[profiles]]
name = "added"
command = "cmd.exe /k prompt added$G"

[[profiles]]
name = "second"
command = "cmd.exe /k prompt second$G"
"#;
    let one_profile = r#"[[profiles]]
name = "first"
command = "cmd.exe /k prompt first$G"
"#;
    let server = Server::start(two_profiles, &[]);
    let state = server.wait_text("first>");
    let list = left_click(area_center(&state["tab_bar"]["profiles"]));
    let path = server.dir().join("config.toml");

    let items = open_menu(&server, &list);
    assert_eq!(menu_shape(&items), "1* 2", "{items:#?}");
    close_menu(&server);

    std::fs::write(&path, renamed).expect("failed to write the settings");
    server.wait_until("three profiles", |state| {
        state["profiles"].as_array().map_or(0, Vec::len) == 3
    });
    let items = open_menu(&server, &list);
    assert_eq!(menu_shape(&items), "1 2* 3", "{items:#?}");
    let names: Vec<String> = items.iter().map(item_name).collect();
    assert_eq!(names, ["1 first", "2 added", "3 second"], "{items:#?}");
    close_menu(&server);

    let items = open_menu(&server, &click_main_menu(&state));
    select_item(&server, &items, 'T');
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    let after = server.wait_text("added>");
    assert_eq!(after["tabs"][1]["profile"], "added", "{after:#}");

    // タブが増えて、ボタンの位置が変わっている。
    let list = left_click(area_center(&after["tab_bar"]["profiles"]));
    let items = open_menu(&server, &list);
    assert_eq!(items.len(), 3, "{items:#?}");
    std::fs::write(&path, one_profile).expect("failed to write the settings");
    server.wait_until("one profile", |state| {
        state["profiles"].as_array().map_or(0, Vec::len) == 1
    });
    wait_menu(&server);
    select_position(&server, 2);
    server.wait_until("three tabs", |state| tab_count(state) == 3);
    let after = server.wait_text("first>");
    assert_eq!(after["tabs"][2]["profile"], "first", "{after:#}");

    // プロファイルを書いていなければ、既定のプロファイルが一つだけ並ぶ。
    std::fs::write(&path, "").expect("failed to write the settings");
    let state = server.wait_until("the default profile", |state| {
        state["profiles"].as_array().map(Vec::len) == Some(1) && state["profiles"][0] != "first"
    });
    let items = open_menu(
        &server,
        &left_click(area_center(&state["tab_bar"]["profiles"])),
    );
    assert_eq!(menu_shape(&items), "1*", "{items:#?}");
    assert_eq!(
        item_name(&items[0]),
        format!("1 {}", state["profiles"][0].as_str().expect("no profile")),
        "{items:#?}"
    );
    close_menu(&server);

    // 10 個目からは数字のアクセラレータがなく、矢印のキーと Enter で選べる。
    const COUNT: usize = 11;
    let mut many = String::new();
    for index in 1..=COUNT {
        many.push_str(&format!(
            "[[profiles]]\nname = \"p{index}\"\ncommand = \"cmd.exe /k prompt p{index}$G\"\n"
        ));
    }
    std::fs::write(&path, many).expect("failed to write the settings");
    let state = server.wait_until("eleven profiles", |state| {
        state["profiles"].as_array().map(Vec::len) == Some(COUNT)
    });
    let items = open_menu(
        &server,
        &left_click(area_center(&state["tab_bar"]["profiles"])),
    );
    assert_eq!(menu_shape(&items), "1* 2 3 4 5 6 7 8 9 . .", "{items:#?}");
    assert_eq!(item_name(&items[COUNT - 1]), format!("p{COUNT}"));
    select_position(&server, COUNT - 1);
    server.wait_until("four tabs", |state| tab_count(state) == 4);
    let state = server.wait_text("p11>");
    assert_eq!(state["tabs"][3]["profile"], "p11", "{state:#}");
}

// 設定ダイアログ。コントロールの ID は `settings_dialog/` と同じ。
