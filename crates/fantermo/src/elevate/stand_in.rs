//! e2e テストが、昇格の判定と管理者としての起動の代わりに使うもの。
//!
//! feature `e2e` のときだけコンパイルする。環境変数で昇格の判定と起動を変えられるのはこのモジュールだけで、配布物には
//! 入らない。逆に、このモジュールを持つビルドは `ShellExecuteExW` の `runas` を呼ばず、UAC の確認を出さない。

use windows::{Win32::Foundation::E_FAIL, core::Error};

use super::{Launch, LaunchError};

/// `1` なら、このプロセスを管理者として動いているものとして扱う。
const ELEVATED_ENV: &str = "FANTERMO_E2E_ELEVATED";
/// 起動する代わりに、実行ファイル・引数・作業フォルダを 1 行ずつ書くファイル。なければ起動に失敗したことにする。
const RECORD_ENV: &str = "FANTERMO_E2E_ELEVATE_RECORD";
/// 起動の結果。`cancelled` なら UAC の確認で断ったことに、`failed` なら失敗したことにして、記録しない。
const RESULT_ENV: &str = "FANTERMO_E2E_ELEVATE_RESULT";

/// [`ELEVATED_ENV`] の、管理者として扱う値。
const ELEVATED_VALUE: &str = "1";

/// テストが、このプロセスを管理者として扱うよう指定しているか。
pub fn is_elevated() -> bool {
    std::env::var(ELEVATED_ENV).is_ok_and(|value| value == ELEVATED_VALUE)
}

/// テストが指定した結果を返す。指定がなければ、起動する代わりに起動の指定をファイルに残す。
pub fn launch(request: &Launch) -> Result<(), LaunchError> {
    if let Some(error) = requested_error(std::env::var(RESULT_ENV).ok().as_deref()) {
        return Err(error);
    }
    let record = std::env::var_os(RECORD_ENV).ok_or_else(|| {
        LaunchError::Failed(Error::new(
            E_FAIL,
            "no file to record the elevated launch in",
        ))
    })?;
    std::fs::write(record, record_text(request)).map_err(|e| LaunchError::Failed(e.into()))
}

/// [`RESULT_ENV`] の値 `result` が求める誤り。なければ `None`。知らない値は警告して、ないものとして扱う。
fn requested_error(result: Option<&str>) -> Option<LaunchError> {
    match result? {
        "cancelled" => Some(LaunchError::Cancelled),
        "failed" => Some(LaunchError::Failed(Error::new(
            E_FAIL,
            "the elevated launch failed as the test requested",
        ))),
        other => {
            tracing::warn!("ignoring {RESULT_ENV}={other:?}");
            None
        }
    }
}

/// 記録のファイルの中身。実行ファイル、引数、作業フォルダ（なければ空）を 1 行ずつ。
fn record_text(request: &Launch) -> String {
    let directory = request
        .directory
        .as_ref()
        .map(|folder| folder.to_string_lossy())
        .unwrap_or_default();
    format!(
        "{}\n{}\n{directory}\n",
        request.exe.to_string_lossy(),
        request.parameters
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn requested_results_become_errors() {
        assert!(requested_error(None).is_none());
        assert!(matches!(
            requested_error(Some("cancelled")),
            Some(LaunchError::Cancelled)
        ));
        assert!(matches!(
            requested_error(Some("failed")),
            Some(LaunchError::Failed(_))
        ));
        assert!(requested_error(Some("other")).is_none());
    }

    #[test]
    fn the_record_has_one_line_for_each_part() {
        let request = Launch {
            exe: PathBuf::from(r"C:\a\fantermo.exe"),
            parameters: r#"--profile "one""#.to_owned(),
            directory: Some(PathBuf::from(r"D:\作業")),
        };
        assert_eq!(
            record_text(&request),
            "C:\\a\\fantermo.exe\n--profile \"one\"\nD:\\作業\n"
        );
        let without_folder = Launch {
            directory: None,
            ..request
        };
        assert_eq!(
            record_text(&without_folder),
            "C:\\a\\fantermo.exe\n--profile \"one\"\n\n"
        );
    }
}
