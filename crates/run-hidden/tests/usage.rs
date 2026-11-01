//! run-hidden の引数のテスト。デスクトップは作らない。
//!
//! このテストがあることで、`cargo test` は run-hidden の実行ファイルも作る。fantermo の e2e テストがそれを使う。

use std::process::Command;

/// 引数が正しくないときの終了コード。
const EXIT_USAGE: i32 = 2;

#[test]
fn missing_program_is_a_usage_error() {
    let status = Command::new(env!("CARGO_BIN_EXE_run-hidden"))
        .arg("--seconds")
        .arg("1")
        .output()
        .expect("failed to start run-hidden")
        .status;
    assert_eq!(status.code(), Some(EXIT_USAGE));
}
