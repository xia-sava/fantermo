//! 管理者として開く。feature `e2e` のビルドは `ShellExecuteExW` の `runas` を呼ばず、起動の指定をファイルに書くか、
//! 環境変数で指定した結果を返す（`crates/fantermo/src/elevate/stand_in.rs`）。UAC の確認は出ない。

use crate::common::CMD_PROMPT;
use crate::common::LaunchAgain;
use crate::common::Server;
use crate::ui::{ONE_PROFILE, tab_count};
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

/// このプロセスを管理者として扱わせる環境変数。
const ELEVATED_ENV: &str = "FANTERMO_E2E_ELEVATED";
/// 起動する代わりに起動の指定を書くファイルの環境変数。
const RECORD_ENV: &str = "FANTERMO_E2E_ELEVATE_RECORD";
/// 起動の結果を決める環境変数。
const RESULT_ENV: &str = "FANTERMO_E2E_ELEVATE_RESULT";

/// 起動する代わりに書かれた、実行ファイル・引数・作業フォルダ。
#[derive(Debug)]
struct Recorded {
    exe: PathBuf,
    parameters: String,
    directory: PathBuf,
}

/// `path` に書かれた起動の指定を読む。
#[track_caller]
fn read_record(path: &Path) -> Recorded {
    let text = std::fs::read_to_string(path).expect("the elevated launch was not recorded");
    let lines: Vec<&str> = text.lines().collect();
    let [exe, parameters, directory] = lines[..] else {
        panic!("unexpected record: {text:?}");
    };
    Recorded {
        exe: PathBuf::from(exe),
        parameters: parameters.to_owned(),
        directory: PathBuf::from(directory),
    }
}

#[track_caller]
fn assert_same_file(left: &Path, right: &Path) {
    let canonical = |path: &Path| {
        std::fs::canonicalize(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    };
    assert_eq!(canonical(left), canonical(right));
}

/// 記録のファイルを `path` にする環境変数。
fn record_at(path: &Path) -> (&'static str, OsString) {
    (RECORD_ENV, path.as_os_str().to_owned())
}

/// `--elevate` は、動いているウィンドウへ渡さずに管理者の権限の fantermo を起動して終わる。渡す引数は `--elevate` を
/// 除いて組み立て直したもので、作業フォルダは起動したときの作業フォルダ。UAC の確認で断られたら、何も開かずに終わる。
/// すでに管理者なら、管理者としての起動はせず、管理者として動いていないウィンドウへも渡さずに自分のウィンドウを開く
/// （管理者として扱うのは起動した側だけで、ウィンドウの側は本物のトークンで読まれる）。
#[test]
fn the_elevate_option_starts_an_elevated_fantermo_and_ends() {
    let server = Server::start(ONE_PROFILE, &[]);
    server.wait_text(CMD_PROMPT);
    let record = server.dir().join("elevated.txt");

    let mut launched = server.launch_again_with(
        &["--elevate", "--profile", "one", "cmd.exe", "/k", "x"],
        &LaunchAgain {
            environment: vec![record_at(&record)],
            ..Default::default()
        },
    );
    assert_eq!(launched.wait_exit(), 0, "{}", launched.log());
    assert!(!launched.opened_a_window(), "{}", launched.log());
    let recorded = read_record(&record);
    assert_same_file(&recorded.exe, Path::new(env!("CARGO_BIN_EXE_fantermo")));
    assert_eq!(recorded.parameters, r#"--profile "one" -- cmd.exe /k x"#);
    assert_same_file(&recorded.directory, server.dir());
    assert_eq!(tab_count(&server.state()), 1);
    std::fs::remove_file(&record).expect("failed to remove the record");

    // UAC の確認で断ると、何も見せずに終わる。
    let mut cancelled = server.launch_again_with(
        &["--elevate"],
        &LaunchAgain {
            environment: vec![record_at(&record), (RESULT_ENV, "cancelled".into())],
            ..Default::default()
        },
    );
    assert_eq!(cancelled.wait_exit(), 1, "{}", cancelled.log());
    assert!(!cancelled.opened_a_window(), "{}", cancelled.log());
    assert!(!record.exists());
    assert_eq!(tab_count(&server.state()), 1);

    // すでに管理者なら、そのまま開く。管理者として動いていない元のウィンドウへは渡さない。
    let mut elevated = server.launch_again_with(
        &["--elevate", "--profile", "one"],
        &LaunchAgain {
            environment: vec![record_at(&record), (ELEVATED_ENV, "1".into())],
            ..Default::default()
        },
    );
    elevated.wait_for_window();
    let state = elevated.wait_text(CMD_PROMPT);
    assert_eq!(state["tabs"][0]["profile"], "one", "{state:#}");
    assert_ne!(state["pid"], server.state()["pid"], "{state:#}");
    assert!(!record.exists());
    assert_eq!(tab_count(&server.state()), 1);
    let log = elevated.log();
    assert!(log.contains("it may run with other rights"), "{log}");
}
