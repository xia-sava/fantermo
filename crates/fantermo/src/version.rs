//! fantermo の版と、ビルドしたコミット。
//!
//! 版は Cargo の版（workspace の `version`）。コミットとリリースのビルドかは `build.rs` が埋め込む。
//! 配布物を作るワークフローは `FANTERMO_RELEASE=1` を付けてビルドする。

/// Cargo の版（`MAJOR.MINOR.PATCH`）。
pub const PACKAGE: &str = env!("CARGO_PKG_VERSION");

/// ビルドしたコミットの短い SHA。git から取れなければ `unknown`。
pub const COMMIT: &str = env!("FANTERMO_COMMIT");

/// リリースのビルドか。
pub const RELEASE: bool = matches!(env!("FANTERMO_RELEASE_BUILD").as_bytes(), b"1");

/// 見せる版。リリースのビルドなら `0.0.1`、ほかは `0.0.1 (dev-<短い SHA>)`。
pub fn display() -> String {
    display_of(PACKAGE, COMMIT, RELEASE)
}

fn display_of(package: &str, commit: &str, release: bool) -> String {
    if release {
        package.to_owned()
    } else {
        format!("{package} (dev-{commit})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_builds_show_only_the_version() {
        assert_eq!(display_of("0.0.1", "abc1234", true), "0.0.1");
        assert_eq!(display_of("0.0.1", "abc1234", false), "0.0.1 (dev-abc1234)");
        assert_eq!(display_of("0.0.1", "unknown", false), "0.0.1 (dev-unknown)");
    }

    #[test]
    fn the_package_version_has_three_numbers() {
        let parts: Vec<_> = PACKAGE.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|part| part.parse::<u32>().is_ok()));
    }
}
