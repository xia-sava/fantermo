//! アプリの中からの更新。
//!
//! 更新できるのは、インストーラ（MSI）で `%LOCALAPPDATA%\Programs\fantermo` に入れたリリースのビルド
//! だけ。zip の版と開発中のビルドでは「fantermo について」の「更新を確認」を押せない。
//!
//! 「更新を確認」を押すと、GitHub の `latest` のリリースから更新の情報 `latest.json` と署名
//! `latest.json.sig` を取得し、埋め込んだ公開鍵で署名を確かめてから読み、版を比べる。新しい版を
//! 入れるときは、MSI を一時フォルダにダウンロードして SHA-256 を照合し、fantermo の外で走るスクリプト
//! （`apply`）に引き渡してから fantermo を閉じる。取得とダウンロードは窓を止めないよう別のスレッドで
//! 行い、終わったら窓にメッセージで知らせる。
//!
//! 取得先はアプリが組み立てる。`latest.json` から使うのは版と MSI の SHA-256 だけ。

mod apply;
mod http;
mod manifest;
pub mod prompt;
mod signature;

use std::{
    fmt, io,
    path::{Path, PathBuf},
    sync::mpsc::Sender,
    time::{SystemTime, UNIX_EPOCH},
};

use windows::Win32::UI::Shell::FOLDERID_UserProgramFiles;

pub use self::manifest::{Comparison, Release, Version};
use self::{
    http::HttpError, manifest::ManifestError, signature::SHA256_LEN, signature::VerifyError,
};
use crate::{known_folder, logging, version};

/// インストーラが fantermo を入れる、`%LOCALAPPDATA%\Programs` の下のフォルダ。
const INSTALL_DIR_NAME: &str = "fantermo";

/// 配布物を置くところ。`latest` のタグのリリースは、版を出すたびに中身を置き換える。
const RELEASE_HOST: &str = "github.com";
const RELEASE_PATH: &str = "/xia-sava/fantermo/releases/download/latest/";
/// 更新の情報、その署名、MSI のファイルの名前。
const MANIFEST_NAME: &str = "latest.json";
const SIGNATURE_NAME: &str = "latest.json.sig";
const MSI_NAME: &str = "fantermo.msi";

/// 受け取る大きさの上限（バイト）。
const MANIFEST_MAX_BYTES: usize = 64 * 1024;
const SIGNATURE_MAX_BYTES: usize = 1024;
const MSI_MAX_BYTES: usize = 200 * 1024 * 1024;

/// 署名を確かめる公開鍵（X.509 の SubjectPublicKeyInfo の DER）。署名する秘密鍵と対になる。
const PUBLIC_KEY: &[u8] = include_bytes!("../../../assets/manifest-public-key.der");

/// ダウンロードした MSI を置く一時フォルダの名前の前半。後ろにプロセスの番号と時刻を付ける。
const DOWNLOAD_DIR_PREFIX: &str = "fantermo-update";

/// この fantermo を更新できるか。インストーラで入れたリリースのビルドなら真。
pub fn can_update() -> bool {
    if !version::RELEASE {
        return false;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            tracing::warn!("failed to get the path of the executable: {e}");
            return false;
        }
    };
    let Some(programs) = known_folder::path(&FOLDERID_UserProgramFiles) else {
        return false;
    };
    is_installed_build(version::RELEASE, &exe, &programs)
}

/// リリースのビルド（`release`）で、実行ファイル `exe` がインストーラの入れる場所（`programs` の下の
/// `fantermo`）にあるか。Windows のパスなので、大文字と小文字は区別しない。
fn is_installed_build(release: bool, exe: &Path, programs: &Path) -> bool {
    let Some(dir) = exe.parent() else {
        return false;
    };
    let install_dir = programs.join(INSTALL_DIR_NAME);
    release && same_path(dir, &install_dir)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_lowercase()
    };
    normalize(a) == normalize(b)
}

/// いまの版。
pub fn current_version() -> Option<Version> {
    Version::parse(version::PACKAGE)
}

/// 別のスレッドでする仕事。
pub enum Work {
    /// 更新の情報を取得し、いまの版と比べる。
    Check,
    /// `Release` の MSI をダウンロードする。
    Download(Release),
}

/// 別のスレッドから窓へ渡す結果。
pub enum Event {
    Checked(Result<Comparison, CheckError>),
    Downloaded(Result<Downloaded, DownloadError>),
}

/// `work` を別のスレッドで始める。終わったら結果を `events` に送り、`notify` で窓に知らせる。
/// スレッドを作れなければ偽を返す。
pub fn start(work: Work, events: Sender<Event>, notify: impl Fn() + Send + 'static) -> bool {
    let spawned = std::thread::Builder::new()
        .name("update".into())
        .spawn(move || {
            let event = match work {
                Work::Check => Event::Checked(check()),
                Work::Download(release) => Event::Downloaded(download(&release)),
            };
            if events.send(event).is_err() {
                tracing::debug!("the window is gone before the update result arrived");
                return;
            }
            notify();
        });
    if let Err(e) = spawned {
        tracing::warn!("failed to start the update thread: {e}");
        return false;
    }
    true
}

/// 更新を確認できなかったわけ。
#[derive(Debug)]
pub enum CheckError {
    /// 更新の情報を取得できなかった。
    Fetch(HttpError),
    /// 署名を取得できなかったか、確かめられなかった。
    Signature(String),
    /// 更新の情報を読めなかった。
    Manifest(ManifestError),
    /// いまの版が `MAJOR.MINOR.PATCH` でない。
    CurrentVersion,
}

impl CheckError {
    /// 利用者に見せる文。内訳はログに残す。
    pub fn message(&self) -> String {
        match self {
            Self::Fetch(HttpError::Status(status)) => {
                format!("更新の情報を取得できませんでした（HTTP {status}）。")
            }
            Self::Fetch(_) => {
                "更新の情報を取得できませんでした。ネットワークにつながっているか確かめてください。"
                    .to_owned()
            }
            Self::Signature(_) => "配布物の署名を確認できませんでした。".to_owned(),
            Self::Manifest(_) => "更新の情報を読めませんでした。".to_owned(),
            Self::CurrentVersion => "いまの版を読めませんでした。".to_owned(),
        }
    }
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fetch(e) => write!(f, "failed to fetch {MANIFEST_NAME}: {e}"),
            Self::Signature(e) => write!(f, "failed to verify {MANIFEST_NAME}: {e}"),
            Self::Manifest(e) => write!(f, "failed to read {MANIFEST_NAME}: {e}"),
            Self::CurrentVersion => write!(
                f,
                "the version {:?} is not MAJOR.MINOR.PATCH",
                version::PACKAGE
            ),
        }
    }
}

/// 更新の情報と署名を取得し、署名を確かめてから、いまの版と比べる。時間がかかるので、窓のスレッド
/// からは呼ばない。
fn check() -> Result<Comparison, CheckError> {
    let current = current_version().ok_or(CheckError::CurrentVersion)?;
    let manifest = http::get(
        RELEASE_HOST,
        &release_path(MANIFEST_NAME),
        MANIFEST_MAX_BYTES,
    )
    .map_err(CheckError::Fetch)?;
    let signature = http::get(
        RELEASE_HOST,
        &release_path(SIGNATURE_NAME),
        SIGNATURE_MAX_BYTES,
    )
    .map_err(|e| CheckError::Signature(format!("failed to fetch {SIGNATURE_NAME}: {e}")))?;
    let comparison = compare_signed(&manifest, &signature, PUBLIC_KEY, current)?;
    tracing::info!("checked for an update: {comparison:?}");
    Ok(comparison)
}

/// 署名 `signature` を公開鍵 `public_key` で確かめてから、更新の情報 `manifest` を読み、いまの版
/// `current` と比べる。署名を確かめられなければ、更新の情報は読まない。
fn compare_signed(
    manifest: &[u8],
    signature: &[u8],
    public_key: &[u8],
    current: Version,
) -> Result<Comparison, CheckError> {
    let signature = std::str::from_utf8(signature)
        .map_err(|e| CheckError::Signature(format!("the signature is not text: {e}")))?;
    signature::verify(manifest, signature, public_key)
        .map_err(|e: VerifyError| CheckError::Signature(e.to_string()))?;
    manifest::compare(manifest, current).map_err(CheckError::Manifest)
}

fn release_path(name: &str) -> String {
    format!("{RELEASE_PATH}{name}")
}

/// ダウンロードできなかったわけ。
#[derive(Debug)]
pub enum DownloadError {
    /// MSI を取得できなかった。
    Fetch(HttpError),
    /// 取得した MSI の SHA-256 が更新の情報と合わない。
    Digest,
    /// 一時フォルダに書けなかった。
    Save(io::Error),
}

impl DownloadError {
    /// 利用者に見せる文。内訳はログに残す。
    pub fn message(&self) -> String {
        match self {
            Self::Fetch(HttpError::Status(status)) => {
                format!("新しい版をダウンロードできませんでした（HTTP {status}）。")
            }
            Self::Fetch(_) => {
                "新しい版をダウンロードできませんでした。ネットワークにつながっているか確かめてください。"
                    .to_owned()
            }
            Self::Digest => "ダウンロードした配布物が、更新の情報と合いませんでした。".to_owned(),
            Self::Save(_) => "ダウンロードした配布物を保存できませんでした。".to_owned(),
        }
    }
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fetch(e) => write!(f, "failed to download {MSI_NAME}: {e}"),
            Self::Digest => write!(
                f,
                "the SHA-256 of {MSI_NAME} does not match {MANIFEST_NAME}"
            ),
            Self::Save(e) => write!(f, "failed to save {MSI_NAME}: {e}"),
        }
    }
}

/// 一時フォルダにダウンロードし、SHA-256 を照合した MSI。引き渡さずに落とすと、フォルダごと消す。
pub struct Downloaded {
    pub version: Version,
    sha256: [u8; SHA256_LEN],
    dir: PathBuf,
    msi: PathBuf,
    handed_off: bool,
}

impl Drop for Downloaded {
    fn drop(&mut self) {
        if self.handed_off {
            return;
        }
        if let Err(e) = std::fs::remove_dir_all(&self.dir) {
            tracing::warn!("failed to remove {}: {e}", self.dir.display());
        }
    }
}

/// `release` の MSI をダウンロードして SHA-256 を照合し、取得のたびに作る一時フォルダに置く。
/// 時間がかかるので、窓のスレッドからは呼ばない。
fn download(release: &Release) -> Result<Downloaded, DownloadError> {
    let bytes = http::get(RELEASE_HOST, &release_path(MSI_NAME), MSI_MAX_BYTES)
        .map_err(DownloadError::Fetch)?;
    let digest = signature::sha256(&bytes).map_err(|e| DownloadError::Save(io::Error::other(e)))?;
    if digest != release.msi_sha256 {
        tracing::warn!(
            "the SHA-256 of {MSI_NAME} is {}, not {}",
            signature::hex(&digest),
            signature::hex(&release.msi_sha256)
        );
        return Err(DownloadError::Digest);
    }
    let dir = new_download_dir().map_err(DownloadError::Save)?;
    let downloaded = Downloaded {
        version: release.version,
        sha256: release.msi_sha256,
        msi: dir.join(MSI_NAME),
        dir,
        handed_off: false,
    };
    std::fs::write(&downloaded.msi, &bytes).map_err(DownloadError::Save)?;
    tracing::info!(
        "downloaded {} ({} bytes) to {}",
        release.version,
        bytes.len(),
        downloaded.msi.display()
    );
    Ok(downloaded)
}

/// 一時フォルダの下に、まだない名前でフォルダを作る。
fn new_download_dir() -> io::Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "{DOWNLOAD_DIR_PREFIX}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// 引き渡せなかったわけ。
#[derive(Debug)]
pub enum ApplyError {
    /// ダウンロードしたあとに MSI が変わった。
    Digest,
    /// ファイルを読み書きできないか、スクリプトを起動できなかった。
    Io(io::Error),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Digest => write!(f, "{MSI_NAME} has changed after the download"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl Downloaded {
    /// ほかに同じ実行ファイルで動いている fantermo のプロセスの番号。
    pub fn other_instances(&self) -> Vec<u32> {
        match std::env::current_exe() {
            Ok(exe) => apply::other_instances(&exe, std::process::id()),
            Err(e) => {
                tracing::warn!("failed to get the path of the executable: {e}");
                Vec::new()
            }
        }
    }

    /// 更新を当てるスクリプトに引き渡す。スクリプトは、この fantermo と `others` のプロセスが終わるのを
    /// 待ってから MSI を入れるので、成功したら fantermo を閉じる。
    ///
    /// 照合してから引き渡すまでの間に MSI が差し替えられていないよう、引き渡す前にもう一度照合する。
    pub fn hand_off(mut self, others: &[u32]) -> Result<(), ApplyError> {
        let bytes = std::fs::read(&self.msi).map_err(ApplyError::Io)?;
        let digest = signature::sha256(&bytes).map_err(|e| ApplyError::Io(io::Error::other(e)))?;
        if digest != self.sha256 {
            return Err(ApplyError::Digest);
        }
        let exe = std::env::current_exe().map_err(ApplyError::Io)?;
        let log_dir = logging::log_dir().ok_or_else(|| {
            ApplyError::Io(io::Error::new(io::ErrorKind::NotFound, "no log folder"))
        })?;
        let mut wait_pids = vec![std::process::id()];
        wait_pids.extend_from_slice(others);
        let script = apply::script(&apply::Plan {
            msi: &self.msi,
            exe: &exe,
            result_log: &log_dir.join(apply::RESULT_LOG_NAME),
            msiexec_log: &log_dir.join(apply::MSIEXEC_LOG_NAME),
            work_dir: &self.dir,
            wait_pids: &wait_pids,
            wait_seconds: apply::WAIT_SECONDS,
        });
        let path = apply::write_script(&self.dir, &script).map_err(ApplyError::Io)?;
        apply::launch(&path).map_err(ApplyError::Io)?;
        tracing::info!(
            "handed the update to {} off to {}",
            self.version,
            path.display()
        );
        self.handed_off = true;
        Ok(())
    }
}

/// 待つ上限（分）。利用者に見せる。
fn wait_minutes() -> u32 {
    apply::WAIT_SECONDS / 60
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROGRAMS: &str = r"C:\Users\xia\AppData\Local\Programs";
    /// `tools/manifest.sh` で使い捨ての鍵から作った、更新の情報と署名と公開鍵。
    const MANIFEST: &[u8] = include_bytes!("../testdata/update/latest.json");
    const SIGNATURE: &[u8] = include_bytes!("../testdata/update/latest.json.sig");
    const TEST_PUBLIC_KEY: &[u8] = include_bytes!("../testdata/update/public-key.der");

    #[test]
    fn only_release_builds_in_the_install_dir_can_update() {
        let programs = Path::new(PROGRAMS);
        let installed = Path::new(r"C:\Users\xia\AppData\Local\Programs\fantermo\fantermo.exe");
        assert!(is_installed_build(true, installed, programs));
        assert!(!is_installed_build(false, installed, programs));
        let unzipped = Path::new(r"C:\Users\xia\Downloads\fantermo-0.0.1-x64\fantermo.exe");
        assert!(!is_installed_build(true, unzipped, programs));
        let nested = Path::new(r"C:\Users\xia\AppData\Local\Programs\fantermo\bin\fantermo.exe");
        assert!(!is_installed_build(true, nested, programs));
    }

    #[test]
    fn install_dir_ignores_case_and_trailing_separators() {
        let exe = Path::new(r"c:\users\XIA\appdata\local\programs\Fantermo\fantermo.exe");
        assert!(is_installed_build(true, exe, Path::new(PROGRAMS)));
        let programs = format!(r"{PROGRAMS}\");
        assert!(is_installed_build(true, exe, Path::new(&programs)));
    }

    #[test]
    fn the_current_version_can_be_compared() {
        assert!(current_version().is_some());
    }

    #[test]
    fn signed_manifest_is_compared_with_the_current_version() {
        let current = Version::parse("0.0.1").unwrap();
        let compared = compare_signed(MANIFEST, SIGNATURE, TEST_PUBLIC_KEY, current).unwrap();
        assert!(
            matches!(compared, Comparison::Newer(release) if release.version.to_string() == "0.0.2")
        );
    }

    #[test]
    fn manifest_is_not_read_unless_the_signature_verifies() {
        let current = Version::parse("0.0.1").unwrap();
        // 署名が合わなければ、中身が読めない更新の情報でも署名の誤りになる。
        assert!(matches!(
            compare_signed(b"{", SIGNATURE, TEST_PUBLIC_KEY, current),
            Err(CheckError::Signature(_))
        ));
        assert!(matches!(
            compare_signed(MANIFEST, SIGNATURE, PUBLIC_KEY, current),
            Err(CheckError::Signature(_))
        ));
        assert!(matches!(
            compare_signed(MANIFEST, b"\xff", TEST_PUBLIC_KEY, current),
            Err(CheckError::Signature(_))
        ));
        assert!(matches!(
            compare_signed(MANIFEST, b"", TEST_PUBLIC_KEY, current),
            Err(CheckError::Signature(_))
        ));
    }

    #[test]
    fn messages_tell_apart_fetching_verifying_and_reading() {
        assert_eq!(
            CheckError::Fetch(HttpError::Status(404)).message(),
            "更新の情報を取得できませんでした（HTTP 404）。"
        );
        assert_eq!(
            CheckError::Signature(String::new()).message(),
            "配布物の署名を確認できませんでした。"
        );
        assert_eq!(
            CheckError::Manifest(ManifestError::NoMsi).message(),
            "更新の情報を読めませんでした。"
        );
    }

    #[test]
    fn downloads_go_to_the_fixed_release() {
        assert_eq!(
            format!("https://{RELEASE_HOST}{}", release_path(MSI_NAME)),
            "https://github.com/xia-sava/fantermo/releases/download/latest/fantermo.msi"
        );
        assert_eq!(
            format!("https://{RELEASE_HOST}{}", release_path(MANIFEST_NAME)),
            "https://github.com/xia-sava/fantermo/releases/download/latest/latest.json"
        );
    }

    #[test]
    fn dropping_a_download_removes_its_folder() {
        let dir = new_download_dir().unwrap();
        std::fs::write(dir.join(MSI_NAME), b"msi").unwrap();
        drop(Downloaded {
            version: Version::parse("0.0.2").unwrap(),
            sha256: [0; SHA256_LEN],
            msi: dir.join(MSI_NAME),
            dir: dir.clone(),
            handed_off: false,
        });
        assert!(!dir.exists());
    }
}
