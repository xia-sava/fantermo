use crate::common::CMD_COMMAND;
use crate::common::CMD_PROMPT;
use crate::common::Server;
use crate::common::release_host::Gate;
use crate::common::release_host::ReleaseHost;
use crate::common::release_host::Reply;
use crate::common::release_host::unused_port;
use crate::common::signing::SigningKey;
use crate::common::signing::hex;
use crate::common::signing::sha256_hex;
use crate::ui::{
    DIALOG_CLOSE_BUTTON, ID_CANCEL, ID_NO, ID_OK, ID_YES, SETTINGS_DIALOG_CLASS, TEXT_DIALOG_CLASS,
    check_terminal_still_works, click, click_main_menu, close_message_box, control_enabled,
    dialog_text, open_menu, open_settings, select_item, wait_dialog_of_class, wait_message_box,
    wait_message_box_with, wait_top_dialog,
};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// 「fantermo について」の「更新を確認」。
const ID_CHECK_UPDATE: i32 = 101;
/// fantermo が取得するパス。
const MANIFEST_PATH: &str = "/xia-sava/fantermo/releases/download/latest/latest.json";
const SIGNATURE_PATH: &str = "/xia-sava/fantermo/releases/download/latest/latest.json.sig";
const MSI_PATH: &str = "/xia-sava/fantermo/releases/download/latest/fantermo.msi";
/// 受け取る大きさの上限（`update.rs`）。
const MANIFEST_MAX_BYTES: usize = 64 * 1024;
const SIGNATURE_MAX_BYTES: usize = 1024;
const MSI_MAX_BYTES: usize = 200 * 1024 * 1024;
/// 配布物のダミー。
const DUMMY_MSI: &[u8] = b"not a real installer";
/// いまのバージョンとして扱うもの。比べる相手は桁の増える `0.0.9` と `0.0.11` など。
const CURRENT_VERSION: &str = "0.0.10";
const NEWER_VERSION: &str = "0.0.11";
/// 一時フォルダの中の、fantermo の `TMP` にするフォルダ。ダウンロードした配布物はここに置かれる。
const UPDATE_TEMP_FOLDER: &str = "tmp";
/// ダウンロードした配布物を置くフォルダの名前の前半。
const DOWNLOAD_FOLDER_PREFIX: &str = "fantermo-update-";
/// 一時フォルダの中の、スクリプトを起動する代わりにそのパスが書かれるファイル。
const HANDOFF_RECORD: &str = "handoff.txt";
/// メッセージボックスの文。`update.rs` の `CheckError` が作る文を写したもの（統合テストからは `update` を呼べない）。
/// 本体の文を変えたら、ここも直す。
const OFFER_TEXT: &str = "ダウンロードして更新しますか";
const UP_TO_DATE_TEXT: &str = "最新バージョンです";
const BAD_SIGNATURE_TEXT: &str = "更新情報の署名を確認できませんでした";
const UNREADABLE_MANIFEST_TEXT: &str = "更新情報を読み取れませんでした";
const CANNOT_FETCH_TEXT: &str = "更新情報を取得できませんでした";
const NETWORK_TEXT: &str = "ネットワークの接続を確認してください";
const CANNOT_DOWNLOAD_TEXT: &str = "新しいバージョンをダウンロードできませんでした";
const DIGEST_MISMATCH_TEXT: &str = "ダウンロードしたファイルが、更新情報と一致しませんでした";
const CANNOT_START_TEXT: &str = "更新を始められませんでした";
const OTHER_INSTANCES_TEXT: &str = "ほかに fantermo が";
const CONFIRM_APPLY_TEXT: &str = "すべてのタブを閉じて";
/// 何も起きないことを見る時間の窓。取得の要求は起動やダイアログを開く操作の直後に出るので、通信とスレッドの
/// 切り替えの遅れを見込んで数秒取る。
const NO_REQUEST_WINDOW: Duration = Duration::from_secs(3);

/// テストの配布元。HTTP と、署名する鍵。
struct ReleaseSource {
    host: ReleaseHost,
    key: SigningKey,
}

impl ReleaseSource {
    fn new() -> Self {
        Self {
            host: ReleaseHost::start(),
            key: SigningKey::generate(),
        }
    }

    /// いまのバージョンを `current` として fantermo を起動する。署名はこの配布元の鍵で検証する。
    fn start_fantermo(&self, current: &str) -> Server {
        start_update_test(self.host.port(), current, Some(&self.key.public_key()))
    }

    /// 更新の情報 `manifest` と、この配布元の鍵の署名を置く。
    fn publish_manifest(&self, manifest: &[u8]) {
        self.host.set(MANIFEST_PATH, Reply::body(manifest));
        self.host
            .set(SIGNATURE_PATH, Reply::body(self.key.sign(manifest)));
    }

    /// バージョン `version` の配布物 `msi` を、更新の情報と署名とあわせて置く。
    fn publish(&self, version: &str, msi: &[u8]) {
        self.publish_manifest(manifest_for(version, &sha256_hex(msi)).as_bytes());
        self.host.set(MSI_PATH, Reply::body(msi));
    }
}

/// `tools/manifest.sh write` が書くのと同じ形の更新の情報。
fn manifest_for(version: &str, msi_sha256: &str) -> String {
    let base = "https://github.com/xia-sava/fantermo/releases/download/latest";
    let zip_sha256 = sha256_hex(b"zip");
    format!(
        "{{\n  \"versionName\": \"{version}\",\n  \"zip\": {{\n    \"url\": \"{base}/fantermo.zip\",\n    \
         \"sha256\": \"{zip_sha256}\"\n  }},\n  \"msi\": {{\n    \"url\": \"{base}/fantermo.msi\",\n    \
         \"sha256\": \"{msi_sha256}\"\n  }}\n}}\n"
    )
}

/// `127.0.0.1` の `port` から更新を取得する fantermo を起動する。`public_key` がなければ埋め込みの公開鍵で検証する。
#[track_caller]
fn start_update_test(port: u16, current: &str, public_key: Option<&[u8]>) -> Server {
    let server = Server::start_with_environment("", &CMD_COMMAND, |dir| {
        let temp = dir.join(UPDATE_TEMP_FOLDER);
        std::fs::create_dir(&temp).expect("failed to create the temporary folder");
        let mut environment = vec![
            ("TMP", temp.clone().into_os_string()),
            ("TEMP", temp.into_os_string()),
            ("FANTERMO_E2E_UPDATE_PORT", port.to_string().into()),
            ("FANTERMO_E2E_UPDATE_CURRENT_VERSION", current.into()),
            (
                "FANTERMO_E2E_UPDATE_HANDOFF_RECORD",
                dir.join(HANDOFF_RECORD).into_os_string(),
            ),
        ];
        if let Some(key) = public_key {
            environment.push(("FANTERMO_E2E_UPDATE_PUBLIC_KEY", hex(key).into()));
        }
        environment
    });
    server.wait_text(CMD_PROMPT);
    server
}

/// ≡ の「fantermo について…」を開く。
#[track_caller]
fn open_about(server: &Server) -> Value {
    let items = open_menu(server, &click_main_menu(&server.state()));
    select_item(server, &items, 'B');
    wait_dialog_of_class(server, TEXT_DIALOG_CLASS)
}

/// 「fantermo について…」を閉じる。
#[track_caller]
fn close_about(server: &Server) {
    click(server, DIALOG_CLOSE_BUTTON);
    server.wait_dialogs(0);
}

/// 「更新を確認」を押し、結果のメッセージボックスの文を返す。
#[track_caller]
fn check_for_update(server: &Server) -> String {
    click(server, ID_CHECK_UPDATE);
    let text = wait_message_box(server);
    let state = server.state();
    assert_eq!(state["dialogs"][0]["title"], "fantermo の更新", "{state:#}");
    text
}

/// 結果のメッセージボックスを閉じて、「更新を確認」を押せるようになるのを待つ。
#[track_caller]
fn dismiss_result(server: &Server) {
    close_message_box(server, 1);
    wait_check_enabled(server);
}

/// 新しいバージョンの知らせに「いいえ」と答えて、「更新を確認」を押せるようになるのを待つ。
#[track_caller]
fn decline_offer(server: &Server) {
    click(server, ID_NO);
    server.wait_dialogs(1);
    wait_check_enabled(server);
}

#[track_caller]
fn wait_check_enabled(server: &Server) {
    server.wait_until("the update button to be enabled", |_| {
        control_enabled(server, ID_CHECK_UPDATE)
    });
}

/// 新しいバージョンの知らせに「はい」と答える。「fantermo について…」が閉じてダウンロードに進む。
#[track_caller]
fn accept_offer(server: &Server, source: &ReleaseSource) {
    source.host.clear_requests();
    assert_contains(&check_for_update(server), OFFER_TEXT);
    click(server, ID_YES);
}

/// ダウンロードのあとの確認まで進む。テストは並列に動くので、同じ実行ファイルのほかの fantermo の知らせが
/// 間に出ることがあり、そのときは続ける。
#[track_caller]
fn wait_confirm_apply(server: &Server) -> String {
    let state = server.wait_until("the prompt after the download", |state| {
        let text = dialog_text(&state["dialogs"][0]);
        text.contains(OTHER_INSTANCES_TEXT) || text.contains(CONFIRM_APPLY_TEXT)
    });
    if dialog_text(&state["dialogs"][0]).contains(OTHER_INSTANCES_TEXT) {
        click(server, ID_YES);
    }
    wait_message_box_with(server, CONFIRM_APPLY_TEXT)
}

#[track_caller]
fn assert_contains(text: &str, needle: &str) {
    assert!(text.contains(needle), "{needle:?} is not in {text:?}");
}

/// fantermo の `TMP` にある、ダウンロードした配布物のフォルダ。
fn download_folders(server: &Server) -> Vec<PathBuf> {
    std::fs::read_dir(server.dir().join(UPDATE_TEMP_FOLDER))
        .expect("failed to read the temporary folder")
        .map(|entry| entry.expect("failed to read a folder entry").path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(DOWNLOAD_FOLDER_PREFIX))
        })
        .collect()
}

/// ダウンロードした配布物が消えるのを待ち、スクリプトの起動を試みていないことを確認する。
#[track_caller]
fn assert_nothing_is_handed_off(server: &Server) {
    server.wait_until("the download to be removed", |_| {
        download_folders(server).is_empty()
    });
    assert!(
        !server.dir().join(HANDOFF_RECORD).exists(),
        "the update was handed off"
    );
}

/// 起動を試みたスクリプトのパスと、BOM を除いた中身。
#[track_caller]
fn handed_off_script(server: &Server) -> (PathBuf, String) {
    let path = std::fs::read_to_string(server.dir().join(HANDOFF_RECORD))
        .expect("the update was not handed off");
    let path = PathBuf::from(path);
    let script = std::fs::read_to_string(&path).expect("failed to read the script");
    (path, script.trim_start_matches('\u{feff}').to_owned())
}

/// インストーラで入れていない開発中のビルドでは「更新を確認」を押せず、そのわけを添える。
#[test]
fn the_update_button_is_grayed_unless_installed_by_the_installer() {
    let server = Server::start_cmd();
    server.wait_text(CMD_PROMPT);
    let dialog = open_about(&server);
    assert_contains(
        &dialog_text(&dialog),
        "インストーラでインストールした場合にのみ",
    );
    let version = server.state()["version"]
        .as_str()
        .expect("no version")
        .to_owned();
    assert_contains(&dialog_text(&dialog), &version);
    assert!(!control_enabled(&server, ID_CHECK_UPDATE));
    let (status, _) = server.request("POST", &format!("/e2e/dialog/click/{ID_CHECK_UPDATE}"), b"");
    assert_eq!(status, 400);
    close_about(&server);
    check_terminal_still_works(&server, "after-grayed");
}

/// 確認は押したときだけする。起動しても「fantermo について…」を開いても取得しない。起動時の確認はない。
/// 新しいバージョンは知らせ、いまのバージョンを添えて尋ねる。取得するのは更新の情報と署名だけで、断ればダウンロードしない。
#[test]
fn a_newer_version_is_offered_and_declining_changes_nothing() {
    let source = ReleaseSource::new();
    source.publish("0.0.10", DUMMY_MSI);
    let server = source.start_fantermo("0.0.9");
    let no_requests = |_: &Value| source.host.requests().is_empty();
    server.assert_holds_for(NO_REQUEST_WINDOW, "no request after the start", no_requests);
    open_about(&server);
    assert!(control_enabled(&server, ID_CHECK_UPDATE));
    server.assert_holds_for(NO_REQUEST_WINDOW, "no request with the dialog", no_requests);

    let text = check_for_update(&server);
    assert_contains(&text, "新しいバージョン 0.0.10 があります");
    assert_contains(&text, "現在のバージョンは 0.0.9");
    assert_contains(&text, OFFER_TEXT);
    assert_eq!(source.host.requests(), [MANIFEST_PATH, SIGNATURE_PATH]);

    decline_offer(&server);
    assert_eq!(source.host.requests(), [MANIFEST_PATH, SIGNATURE_PATH]);
    assert!(download_folders(&server).is_empty());
    assert_eq!(server.state()["dialogs"][0]["class"], TEXT_DIALOG_CLASS);

    close_about(&server);
    check_terminal_still_works(&server, "after-decline");
}

/// 更新の確認の結果を見せている間に最後のシェルが終わると、ウィンドウが閉じて落ちずに終わる。結果を閉じた
/// あとの処理は、ウィンドウがもうないので飛ばす。
#[test]
fn a_shell_ending_while_an_update_result_is_shown_closes_the_window() {
    let source = ReleaseSource::new();
    source.publish(CURRENT_VERSION, DUMMY_MSI);
    let mut server = source.start_fantermo(CURRENT_VERSION);
    open_about(&server);
    assert_contains(&check_for_update(&server), UP_TO_DATE_TEXT);
    server.send_line("exit");
    assert_eq!(server.wait_exit(), 0, "{}", server.log());
    let log = server.log();
    assert!(log.contains("the window is gone; skipped message"), "{log}");
}

/// 署名を検証できなければ、中身が新しいバージョンでも更新を出さず、配布物を取得せず、署名のせいだと分かる文を出す。
#[test]
fn a_manifest_that_does_not_verify_offers_nothing() {
    let source = ReleaseSource::new();
    let other_key = SigningKey::generate();
    let server = source.start_fantermo(CURRENT_VERSION);
    open_about(&server);
    let manifest = manifest_for(NEWER_VERSION, &sha256_hex(DUMMY_MSI));

    // 署名が合う形で同じ流れが更新を出すことを、先に確認する。
    source.publish_manifest(manifest.as_bytes());
    source.host.set(MSI_PATH, Reply::body(DUMMY_MSI));
    assert_contains(&check_for_update(&server), OFFER_TEXT);
    decline_offer(&server);

    // 署名を取得できなかった場合は、取得の失敗として知らせる。
    // (説明, 更新の情報, 署名)
    let fetch_failures: Vec<(&str, &str, Reply)> = vec![
        ("no signature", &manifest, Reply::Status(404)),
        (
            "a signature that is too large",
            &manifest,
            Reply::Zeros(SIGNATURE_MAX_BYTES + 1),
        ),
        ("the connection closes", &manifest, Reply::Close),
    ];
    // 取得できた署名が合わない場合は、署名の失敗として知らせる。
    let bad_signatures: Vec<(&str, &str, Reply)> = vec![(
        "signed with another key",
        &manifest,
        Reply::body(other_key.sign(manifest.as_bytes())),
    )];
    let cases = fetch_failures
        .into_iter()
        .map(|(description, body, signature)| (CANNOT_FETCH_TEXT, description, body, signature))
        .chain(
            bad_signatures
                .into_iter()
                .map(|(description, body, signature)| {
                    (BAD_SIGNATURE_TEXT, description, body, signature)
                }),
        );
    for (expected, description, body, signature) in cases {
        source.host.clear_requests();
        source.host.set(MANIFEST_PATH, Reply::body(body));
        source.host.set(SIGNATURE_PATH, signature);
        let text = check_for_update(&server);
        assert_contains(&text, expected);
        assert!(!text.contains(OFFER_TEXT), "{description}: {text}");
        assert!(
            !source.host.requests().iter().any(|path| path == MSI_PATH),
            "{description}"
        );
        dismiss_result(&server);
    }

    close_about(&server);
    assert!(download_folders(&server).is_empty());
    check_terminal_still_works(&server, "after-signatures");
}

/// 公開鍵を差し替えていなければ、テストの鍵の署名は埋め込みの公開鍵で通らない。
#[test]
fn the_embedded_key_does_not_accept_another_key() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let server = start_update_test(source.host.port(), CURRENT_VERSION, None);
    open_about(&server);
    assert_contains(&check_for_update(&server), BAD_SIGNATURE_TEXT);
    dismiss_result(&server);
    close_about(&server);
    check_terminal_still_works(&server, "after-embedded-key");
}

/// 更新の情報を取得できない・読めないときは、わけの分かる文を出す。落ちず、もう一度確認できる。
#[test]
fn failures_to_fetch_or_read_the_manifest_are_told() {
    let source = ReleaseSource::new();
    let server = source.start_fantermo(CURRENT_VERSION);
    open_about(&server);
    let manifest = manifest_for(NEWER_VERSION, &sha256_hex(DUMMY_MSI));
    source.host.set(
        SIGNATURE_PATH,
        Reply::body(source.key.sign(manifest.as_bytes())),
    );

    // 取得できない。署名は取得しない。
    let cases: Vec<(&str, Reply, &[&str])> = vec![
        ("404", Reply::Status(404), &[CANNOT_FETCH_TEXT, "HTTP 404"]),
        ("500", Reply::Status(500), &[CANNOT_FETCH_TEXT, "HTTP 500"]),
        ("closed", Reply::Close, &[CANNOT_FETCH_TEXT, NETWORK_TEXT]),
        (
            "too large",
            Reply::Zeros(MANIFEST_MAX_BYTES + 1),
            &[CANNOT_FETCH_TEXT],
        ),
    ];
    for (description, reply, expected) in cases {
        source.host.clear_requests();
        source.host.set(MANIFEST_PATH, reply);
        let text = check_for_update(&server);
        for needle in expected {
            assert!(text.contains(needle), "{description}: {text}");
        }
        assert!(
            !source
                .host
                .requests()
                .iter()
                .any(|path| path == SIGNATURE_PATH),
            "{description}"
        );
        dismiss_result(&server);
    }

    // 途中で切れた更新の情報は、署名が合わない。
    source.host.set(
        MANIFEST_PATH,
        Reply::Truncated {
            claimed: manifest.len(),
            body: Arc::from(&manifest.as_bytes()[..manifest.len() / 2]),
        },
    );
    assert_contains(&check_for_update(&server), BAD_SIGNATURE_TEXT);
    dismiss_result(&server);

    // 署名は合うが読めない。
    source.publish_manifest(br#"{ "versionName": "dev-abc" }"#);
    assert_contains(&check_for_update(&server), UNREADABLE_MANIFEST_TEXT);
    dismiss_result(&server);

    // 上限ちょうどまでは受け取る。新しくなければ MSI を取得しない。
    let mut largest = b"{ \"versionName\": \"0.0.10\" }".to_vec();
    largest.resize(MANIFEST_MAX_BYTES, b' ');
    source.publish_manifest(&largest);
    source.host.clear_requests();
    assert_contains(&check_for_update(&server), UP_TO_DATE_TEXT);
    assert!(!source.host.requests().iter().any(|path| path == MSI_PATH));
    dismiss_result(&server);

    close_about(&server);
    check_terminal_still_works(&server, "after-fetch-failures");
}

/// 接続できないときは、ネットワークを確認するよう知らせる。
#[test]
fn a_host_that_refuses_connections_is_told() {
    let server = start_update_test(unused_port(), CURRENT_VERSION, None);
    open_about(&server);
    let text = check_for_update(&server);
    assert_contains(&text, CANNOT_FETCH_TEXT);
    assert_contains(&text, NETWORK_TEXT);
    dismiss_result(&server);
    close_about(&server);
    check_terminal_still_works(&server, "after-refused");
}

/// 答えない相手は時間切れになる（受信の上限は 30 秒。実時間で待つので、このテストは遅い）。待っている間も端末は
/// 動く。
#[test]
fn a_silent_host_times_out_while_the_terminal_keeps_working() {
    let source = ReleaseSource::new();
    source.host.set(MANIFEST_PATH, Reply::Silent);
    let server = source.start_fantermo(CURRENT_VERSION);
    open_about(&server);
    click(&server, ID_CHECK_UPDATE);
    server.wait_until("the check to start", |_| !source.host.requests().is_empty());
    close_about(&server);
    check_terminal_still_works(&server, "while-waiting");
    let text = wait_message_box_with(&server, CANNOT_FETCH_TEXT);
    assert_contains(&text, NETWORK_TEXT);
    close_message_box(&server, 0);
    check_terminal_still_works(&server, "after-timeout");
}

/// 確認している間は押せない。「fantermo について…」を閉じても確認は続き、結果は端末のウィンドウの上に出る。
/// 開き直せば、結果はその上に出て、押せるように戻る。設定ダイアログの間に結果が届いても、閉じたあとにそのダイアログの
/// 間だけウィンドウを無効のままにする。ダウンロードの間に開いた「fantermo について…」の上にも確認を出し、取りやめれば
/// 押せるように戻る。
#[test]
fn a_result_arriving_over_other_dialogs_or_after_closing_the_about_dialog() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let manifest = manifest_for(NEWER_VERSION, &sha256_hex(DUMMY_MSI));
    let hold = |gate: &Arc<Gate>| Reply::After {
        gate: Arc::clone(gate),
        reply: Box::new(Reply::body(&manifest)),
    };
    let server = source.start_fantermo(CURRENT_VERSION);

    // 閉じてから結果が届く。
    let gate = Gate::new();
    source.host.set(MANIFEST_PATH, hold(&gate));
    open_about(&server);
    click(&server, ID_CHECK_UPDATE);
    server.wait_until("the check to start", |_| !source.host.requests().is_empty());
    assert!(!control_enabled(&server, ID_CHECK_UPDATE));
    close_about(&server);
    check_terminal_still_works(&server, "during-check");
    gate.open();
    wait_message_box_with(&server, OFFER_TEXT);
    assert_eq!(server.state()["dialogs"].as_array().map(Vec::len), Some(1));
    assert_eq!(server.state()["window_enabled"], false);
    click(&server, ID_NO);
    server.wait_dialogs(0);
    server.wait_until("the window to be enabled", |state| {
        state["window_enabled"] == true
    });
    check_terminal_still_works(&server, "after-result-on-window");

    // 開き直している間に結果が届く。
    let gate = Gate::new();
    source.host.clear_requests();
    source.host.set(MANIFEST_PATH, hold(&gate));
    open_about(&server);
    click(&server, ID_CHECK_UPDATE);
    server.wait_until("the check to start", |_| !source.host.requests().is_empty());
    close_about(&server);
    open_about(&server);
    assert!(!control_enabled(&server, ID_CHECK_UPDATE));
    gate.open();
    wait_message_box_with(&server, OFFER_TEXT);
    assert_eq!(server.state()["dialogs"].as_array().map(Vec::len), Some(2));
    decline_offer(&server);
    close_about(&server);
    check_terminal_still_works(&server, "after-reopen");

    // ほかのダイアログの間に結果が届いても、閉じたあとにそのダイアログの間だけウィンドウを無効のままにする。
    let gate = Gate::new();
    source.host.clear_requests();
    source.host.set(MANIFEST_PATH, hold(&gate));
    open_about(&server);
    click(&server, ID_CHECK_UPDATE);
    server.wait_until("the check to start", |_| !source.host.requests().is_empty());
    close_about(&server);
    open_settings(&server);
    gate.open();
    wait_message_box_with(&server, OFFER_TEXT);
    click(&server, ID_NO);
    wait_top_dialog(&server, SETTINGS_DIALOG_CLASS);
    server.wait_dialogs(1);
    let state = server.state();
    assert_eq!(state["window_enabled"], false, "{state:#}");
    click(&server, ID_CANCEL);
    server.wait_dialogs(0);
    server.wait_until("the window to be enabled", |state| {
        state["window_enabled"] == true
    });
    check_terminal_still_works(&server, "after-settings-and-result");

    // ダウンロードの間に開いた「fantermo について…」の上にも確認を出し、取りやめれば「更新を確認」を押せるように戻る。
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let gate = Gate::new();
    source.host.set(
        MSI_PATH,
        Reply::After {
            gate: Arc::clone(&gate),
            reply: Box::new(Reply::body(DUMMY_MSI)),
        },
    );
    open_about(&server);
    accept_offer(&server, &source);
    server.wait_until("the download to start", |_| {
        source.host.requests().iter().any(|path| path == MSI_PATH)
    });
    server.wait_dialogs(0);
    open_about(&server);
    assert!(!control_enabled(&server, ID_CHECK_UPDATE));
    gate.open();
    wait_confirm_apply(&server);
    click(&server, ID_CANCEL);
    server.wait_until("the about dialog to be on top", |state| {
        state["dialogs"][0]["class"] == TEXT_DIALOG_CLASS
    });
    wait_check_enabled(&server);
    assert_nothing_is_handed_off(&server);
    close_about(&server);
    check_terminal_still_works(&server, "after-download-over-about");
}

/// 確認やダウンロードの途中でウィンドウを閉じても、普通に終わり、何も起動しない。
#[test]
fn quitting_during_a_check_or_a_download_ends_normally() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let manifest = manifest_for(NEWER_VERSION, &sha256_hex(DUMMY_MSI));
    let check_gate = Gate::new();
    source.host.set(
        MANIFEST_PATH,
        Reply::After {
            gate: Arc::clone(&check_gate),
            reply: Box::new(Reply::body(&manifest)),
        },
    );
    let mut during_check = source.start_fantermo(CURRENT_VERSION);
    open_about(&during_check);
    click(&during_check, ID_CHECK_UPDATE);
    during_check.wait_until("the check to start", |_| !source.host.requests().is_empty());
    close_about(&during_check);
    during_check.post("/e2e/quit", b"");
    assert_eq!(during_check.wait_exit(), 0);
    check_gate.open();
    assert!(!during_check.dir().join(HANDOFF_RECORD).exists());
    drop(during_check);

    let download_gate = Gate::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    source.host.set(
        MSI_PATH,
        Reply::After {
            gate: Arc::clone(&download_gate),
            reply: Box::new(Reply::body(DUMMY_MSI)),
        },
    );
    let mut during_download = source.start_fantermo(CURRENT_VERSION);
    open_about(&during_download);
    accept_offer(&during_download, &source);
    during_download.wait_until("the download to start", |_| {
        source.host.requests().iter().any(|path| path == MSI_PATH)
    });
    during_download.wait_dialogs(0);
    during_download.post("/e2e/quit", b"");
    assert_eq!(during_download.wait_exit(), 0);
    download_gate.open();
    assert!(!during_download.dir().join(HANDOFF_RECORD).exists());
    assert!(download_folders(&during_download).is_empty());
}

/// 配布物が更新の情報のハッシュと合わない、取得できないときは、スクリプトに引き渡さず、知らせる。
#[test]
fn a_download_that_does_not_match_or_fails_is_not_handed_off() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let server = source.start_fantermo(CURRENT_VERSION);
    let cases: Vec<(&str, Reply, &str)> = vec![
        (
            "another file",
            Reply::body(b"a replaced installer"),
            DIGEST_MISMATCH_TEXT,
        ),
        (
            "cut in the middle",
            Reply::Truncated {
                claimed: DUMMY_MSI.len(),
                body: Arc::from(&DUMMY_MSI[..DUMMY_MSI.len() / 2]),
            },
            DIGEST_MISMATCH_TEXT,
        ),
        ("404", Reply::Status(404), "HTTP 404"),
        ("500", Reply::Status(500), "HTTP 500"),
        ("closed", Reply::Close, CANNOT_DOWNLOAD_TEXT),
        (
            "too large",
            Reply::Zeros(MSI_MAX_BYTES + 1),
            CANNOT_DOWNLOAD_TEXT,
        ),
    ];
    for (description, reply, expected) in cases {
        source.host.set(MSI_PATH, reply);
        open_about(&server);
        accept_offer(&server, &source);
        let text = wait_message_box_with(&server, "更新できませんでした");
        assert!(text.contains(expected), "{description}: {text}");
        assert!(!text.contains(CONFIRM_APPLY_TEXT), "{description}: {text}");
        assert_eq!(
            source.host.requests().last().map(String::as_str),
            Some(MSI_PATH),
            "{description}"
        );
        close_message_box(&server, 0);
        assert_nothing_is_handed_off(&server);
    }
    check_terminal_still_works(&server, "after-bad-downloads");
}

/// ハッシュが合えば確認へ進み、「OK」でスクリプトに引き渡して閉じる。スクリプトは起動せず、中身を確認する。
#[test]
fn a_matching_download_is_handed_off_to_the_script_without_running_it() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let mut server = source.start_fantermo(CURRENT_VERSION);
    let pid = server.pid();
    open_about(&server);
    accept_offer(&server, &source);
    assert_contains(
        &wait_confirm_apply(&server),
        &format!("バージョン {NEWER_VERSION} に更新します"),
    );
    assert_eq!(
        source.host.requests(),
        [MANIFEST_PATH, SIGNATURE_PATH, MSI_PATH]
    );
    assert!(!server.dir().join(HANDOFF_RECORD).exists());

    click(&server, ID_OK);
    assert_eq!(server.wait_exit(), 0);
    assert!(server.state_file().exists(), "the state was not saved");

    let (script_path, script) = handed_off_script(&server);
    let folder = script_path.parent().expect("no folder");
    assert_eq!(
        folder.parent().expect("no temporary folder"),
        server.dir().join(UPDATE_TEMP_FOLDER)
    );
    let msi = folder.join("fantermo.msi");
    assert_eq!(std::fs::read(&msi).expect("no MSI"), DUMMY_MSI);
    assert_contains(&script, &format!("foreach ($id in @({pid}"));
    assert_contains(&script, r"System32\msiexec.exe");
    assert_contains(
        &script,
        &format!("@('/i', '\"{}\"', '/qn', '/norestart'", msi.display()),
    );
    assert_contains(
        &script,
        &format!(
            "Start-Process -FilePath '{}'",
            env!("CARGO_BIN_EXE_fantermo").replace('/', r"\")
        ),
    );
    assert!(!script.contains(r"AppData\Local\Programs"), "{script}");
}

/// 確認で「キャンセル」か × を選ぶと、引き渡さず、ダウンロードしたものを消す。もう一度確認できる。
/// 確認の間に配布物が変わったときと、スクリプトを起動できないときも、引き渡さず知らせ、ダウンロードしたものを消す。
#[test]
fn cancelling_the_final_confirmation_removes_the_download() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let server = source.start_fantermo(CURRENT_VERSION);

    open_about(&server);
    accept_offer(&server, &source);
    wait_confirm_apply(&server);
    assert_eq!(download_folders(&server).len(), 1);
    click(&server, ID_CANCEL);
    server.wait_dialogs(0);
    assert_nothing_is_handed_off(&server);
    check_terminal_still_works(&server, "after-cancel");

    open_about(&server);
    accept_offer(&server, &source);
    wait_confirm_apply(&server);
    close_message_box(&server, 0);
    assert_nothing_is_handed_off(&server);
    check_terminal_still_works(&server, "after-close-button");

    // 確認している間に配布物が変わったら引き渡さず、知らせる。
    open_about(&server);
    accept_offer(&server, &source);
    wait_confirm_apply(&server);
    let folders = download_folders(&server);
    let msi = folders[0].join("fantermo.msi");
    std::fs::write(&msi, b"changed after the download").expect("failed to change the MSI");
    click(&server, ID_OK);
    wait_message_box_with(&server, CANNOT_START_TEXT);
    close_message_box(&server, 0);
    assert_nothing_is_handed_off(&server);
    check_terminal_still_works(&server, "after-changed-download");

    // スクリプトを起動できなければ、知らせて端末のまま続ける。書けないパスを作って、起動の失敗にする。
    open_about(&server);
    accept_offer(&server, &source);
    wait_confirm_apply(&server);
    std::fs::create_dir(server.dir().join(HANDOFF_RECORD)).expect("failed to create the folder");
    click(&server, ID_OK);
    wait_message_box_with(&server, CANNOT_START_TEXT);
    close_message_box(&server, 0);
    server.wait_until("the download to be removed", |_| {
        download_folders(&server).is_empty()
    });
    check_terminal_still_works(&server, "after-script-failure");
}

/// ほかの fantermo が動いていれば知らせる。断れば引き渡さず、続ければスクリプトはそのプロセスも待つ。
#[test]
fn other_instances_are_told_and_waited_for() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let other = Server::start_cmd();
    other.wait_text(CMD_PROMPT);
    let other_pid = other.pid();
    let mut server = source.start_fantermo(CURRENT_VERSION);

    open_about(&server);
    accept_offer(&server, &source);
    wait_message_box_with(&server, OTHER_INSTANCES_TEXT);
    click(&server, ID_NO);
    server.wait_dialogs(0);
    assert_nothing_is_handed_off(&server);
    check_terminal_still_works(&server, "after-declining-others");

    open_about(&server);
    accept_offer(&server, &source);
    wait_message_box_with(&server, OTHER_INSTANCES_TEXT);
    click(&server, ID_YES);
    wait_message_box_with(&server, CONFIRM_APPLY_TEXT);
    click(&server, ID_OK);
    assert_eq!(server.wait_exit(), 0);
    let (_, script) = handed_off_script(&server);
    let waited = script
        .lines()
        .find(|line| line.starts_with("foreach ($id in @("))
        .expect("the script waits for nothing");
    assert!(
        waited.contains(&other_pid.to_string()),
        "{other_pid} is not in {waited}"
    );
}

/// いまのバージョンが読めなければ、取得せずに知らせる。
#[test]
fn an_unreadable_current_version_is_told() {
    let source = ReleaseSource::new();
    source.publish(NEWER_VERSION, DUMMY_MSI);
    let server = source.start_fantermo("dev-abc");
    open_about(&server);
    assert_contains(
        &check_for_update(&server),
        "現在のバージョンを読み取れませんでした",
    );
    assert!(source.host.requests().is_empty());
    dismiss_result(&server);
    close_about(&server);
    check_terminal_still_works(&server, "after-unreadable-version");
}

// DPI の変更。モニターの DPI は変えられないので、`WM_DPICHANGED` への反応を見る。手元のモニター（192 DPI）でも
// ランナー（96 DPI）でも同じに通るよう、寸法は状態の値から求め、基準は 96 DPI で読んだ値にする。
