//! 更新の情報（`latest.json`）とバージョン。
//!
//! `latest.json` は `tools/manifest.sh` が書く:
//! `{ "versionName": "0.0.2", "zip": { "url": "...", "sha256": "..." }, "msi": { "url": "...", "sha256": "..." } }`。
//! 使うのは `versionName` と `msi` の `sha256` だけで、配布物の取得先はアプリが組み立てる。

use std::fmt;

use serde::Deserialize;

use super::signature::SHA256_LEN;

/// バージョン（`MAJOR.MINOR.PATCH`）。数として比べる。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Version {
    /// `MAJOR.MINOR.PATCH`（それぞれ 10 進の数字だけ）を読む。ほかの形なら `None`。
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('.').map(|part| {
            (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
                .then(|| part.parse::<u32>().ok())
                .flatten()
        });
        let version = Self {
            major: parts.next()??,
            minor: parts.next()??,
            patch: parts.next()??,
        };
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// 配布しているバージョンの、MSI。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// MSI の SHA-256。
    pub msi_sha256: [u8; SHA256_LEN],
}

/// いまのバージョンと比べた結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Comparison {
    /// 配布しているバージョンがいまのバージョンより新しくない。
    UpToDate { latest: Version },
    /// 新しいバージョンがある。
    Newer(Release),
}

/// 更新の情報を読めなかったわけ。ログに残す。
#[derive(Debug)]
pub enum ManifestError {
    /// JSON として、または項目の形が読めない。
    Json(serde_json::Error),
    /// `versionName` が `MAJOR.MINOR.PATCH` でない。
    Version(String),
    /// `msi` の項目がない。
    NoMsi,
    /// `msi` の `sha256` が 64 桁の 16 進でない。
    Digest(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(e) => write!(f, "{e}"),
            Self::Version(text) => write!(f, "versionName {text:?} is not MAJOR.MINOR.PATCH"),
            Self::NoMsi => write!(f, "no msi entry"),
            Self::Digest(text) => write!(f, "msi.sha256 {text:?} is not a SHA-256 in hex"),
        }
    }
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(rename = "versionName")]
    version_name: String,
    msi: Option<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    sha256: String,
}

/// 署名を確かめた更新の情報 `bytes` を読み、いまのバージョン `current` と比べる。
///
/// 新しくないときは `msi` の項目がなくてもよい（MSI を配る前のバージョンの情報でも、最新かは分かる）。
pub fn compare(bytes: &[u8], current: Version) -> Result<Comparison, ManifestError> {
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(ManifestError::Json)?;
    let latest = Version::parse(&manifest.version_name)
        .ok_or_else(|| ManifestError::Version(manifest.version_name.clone()))?;
    if latest <= current {
        return Ok(Comparison::UpToDate { latest });
    }
    let msi = manifest.msi.ok_or(ManifestError::NoMsi)?;
    let msi_sha256 = parse_digest(&msi.sha256).ok_or(ManifestError::Digest(msi.sha256))?;
    Ok(Comparison::Newer(Release {
        version: latest,
        msi_sha256,
    }))
}

/// 64 桁の 16 進（大文字も読む）を SHA-256 のバイト列にする。
fn parse_digest(text: &str) -> Option<[u8; SHA256_LEN]> {
    let text = text.as_bytes();
    if text.len() != 2 * SHA256_LEN {
        return None;
    }
    let mut digest = [0; SHA256_LEN];
    for (byte, pair) in digest.iter_mut().zip(text.as_chunks::<2>().0) {
        let pair = std::str::from_utf8(pair).ok()?;
        if !pair.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &[u8] = include_bytes!("../../testdata/update/latest.json");

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(version("0.0.1") < version("0.0.2"));
        assert!(version("0.0.10") > version("0.0.9"));
        assert!(version("0.10.0") > version("0.9.99"));
        assert!(version("1.0.0") > version("0.99.99"));
        assert_eq!(version("0.0.1"), version("0.0.1"));
        assert_eq!(version("0.0.10").to_string(), "0.0.10");
    }

    #[test]
    fn only_three_numbers_are_versions() {
        for text in [
            "",
            "0.0",
            "0.0.1.2",
            "0.0.x",
            "v0.0.1",
            "0.0.1-rc1",
            "0..1",
            " 0.0.1",
            "+1.0.0",
        ] {
            assert_eq!(Version::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn newer_manifest_gives_the_msi_digest() {
        let Comparison::Newer(release) = compare(MANIFEST, version("0.0.1")).unwrap() else {
            panic!("expected a newer release");
        };
        assert_eq!(release.version, version("0.0.2"));
        assert_eq!(
            super::super::signature::hex(&release.msi_sha256),
            "8b25505cb8d6a75a790f7aee75930a62dc03dfabdab4d43acc40468485509f4b"
        );
    }

    #[test]
    fn same_or_older_manifest_is_up_to_date() {
        for current in ["0.0.2", "0.0.3", "1.0.0"] {
            assert_eq!(
                compare(MANIFEST, version(current)).unwrap(),
                Comparison::UpToDate {
                    latest: version("0.0.2")
                }
            );
        }
    }

    #[test]
    fn manifest_without_msi_is_enough_to_know_it_is_up_to_date() {
        let zip_only = br#"{ "versionName": "0.0.1", "zip": { "url": "u", "sha256": "s" } }"#;
        assert!(matches!(
            compare(zip_only, version("0.0.1")),
            Ok(Comparison::UpToDate { .. })
        ));
        let newer = br#"{ "versionName": "0.0.2", "zip": { "url": "u", "sha256": "s" } }"#;
        assert!(matches!(
            compare(newer, version("0.0.1")),
            Err(ManifestError::NoMsi)
        ));
    }

    #[test]
    fn malformed_manifests_are_rejected() {
        let current = version("0.0.1");
        assert!(matches!(
            compare(b"{", current),
            Err(ManifestError::Json(_))
        ));
        assert!(matches!(
            compare(b"{}", current),
            Err(ManifestError::Json(_))
        ));
        assert!(matches!(
            compare(br#"{ "versionName": "dev-abc" }"#, current),
            Err(ManifestError::Version(_))
        ));
        let short = br#"{ "versionName": "0.0.2", "msi": { "url": "u", "sha256": "8b25" } }"#;
        assert!(matches!(
            compare(short, current),
            Err(ManifestError::Digest(_))
        ));
        let no_digest = br#"{ "versionName": "0.0.2", "msi": { "url": "u" } }"#;
        assert!(matches!(
            compare(no_digest, current),
            Err(ManifestError::Json(_))
        ));
    }

    #[test]
    fn digests_are_64_hex_digits() {
        let upper = "8B25505CB8D6A75A790F7AEE75930A62DC03DFABDAB4D43ACC40468485509F4B";
        assert_eq!(parse_digest(upper).unwrap()[0], 0x8b);
        assert!(parse_digest(&upper[1..]).is_none());
        assert!(parse_digest(&format!("+{}", &upper[1..])).is_none());
        assert!(parse_digest(&format!("あ{}", &upper[3..])).is_none());
    }
}
