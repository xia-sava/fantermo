//! 更新を適用する、fantermo の外で動くスクリプト。
//!
//! fantermo が動いている間は実行ファイルを置き換えられないので、スクリプトに引き渡してから fantermo を
//! 閉じる。スクリプトは fantermo の終了を待ち、msiexec で MSI を無人で入れ、入れ終えたら fantermo を
//! 起動し直す。結果はログのフォルダの `update-apply.log` に書く（そのとき fantermo は動いていないので、
//! 成否はこのファイルだけに残る）。

use std::{
    io,
    os::windows::io::AsRawHandle,
    path::{Path, PathBuf},
};

use windows::{
    Win32::{
        Foundation::HANDLE,
        System::Threading::{
            PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        },
    },
    core::PWSTR,
};

use crate::{app_files::same_path, process};

/// スクリプトのファイルの名前。MSI と同じフォルダに書く。
pub const SCRIPT_NAME: &str = "apply-update.ps1";
/// 結果を書くファイルの名前。ログのフォルダに置く。
pub const RESULT_LOG_NAME: &str = "update-apply.log";
/// msiexec の詳しいログのファイルの名前。ログのフォルダに置き、更新のたびに書き直す。
pub const MSIEXEC_LOG_NAME: &str = "update-msiexec.log";
/// スクリプトが、待つプロセスの終了を待つ上限（秒）。過ぎたら待たずに入れる。
pub const WAIT_SECONDS: u32 = 300;

/// msiexec の終了コードを読めなかったときの、スクリプトの終了コード。
const UNKNOWN_EXIT_CODE: i32 = -1;

/// `%SystemRoot%` からの msiexec の場所。PATH から探さない。
const MSIEXEC_UNDER_SYSTEM_ROOT: &str = r"System32\msiexec.exe";

/// 実行ファイルの名前を読む領域の長さ（UTF-16 の数）。
const IMAGE_NAME_CAPACITY: usize = 32 * 1024;

/// UTF-8 の BOM。Windows PowerShell 5 は BOM のないスクリプトをシステムの文字コードで読む。
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// スクリプトに渡すもの。
pub struct Plan<'a> {
    /// 入れる MSI。
    pub msi: &'a Path,
    /// 入れる MSI の SHA-256（16 進）。スクリプトが入れる直前に照合する。
    pub msi_sha256: &'a str,
    /// 入れ終えたら起動し直す実行ファイル。
    pub exe: &'a Path,
    /// 結果を書くファイル。
    pub result_log: &'a Path,
    /// msiexec の詳しいログ。
    pub msiexec_log: &'a Path,
    /// 入れ終えたら消すフォルダ（MSI とスクリプトを置いた一時フォルダ）。
    pub work_dir: &'a Path,
    /// 終わるのを待つプロセスの ID。
    pub wait_pids: &'a [u32],
    /// 待つ上限（秒）。
    pub wait_seconds: u32,
}

/// 更新を適用する PowerShell のスクリプト。
///
/// 待つプロセスがすべて終わる（か上限を過ぎる）のを待ち、MSI の SHA-256 を照合してから、
/// `msiexec /i <MSI> /qn /norestart` で入れる。照合できないか合わなければ、入れずに終わる。
/// 終了コードが 0（成功）か 3010（成功。再起動で反映するものがある）なら一時フォルダを消して
/// 起動し直し、ほかなら起動し直さずに終わる。どちらも結果のファイルに書く。終了コードを読めなければ、
/// その旨を書いて `UNKNOWN_EXIT_CODE` で終わる。
pub fn script(plan: &Plan) -> String {
    let pids = plan
        .wait_pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let log = literal(&plan.result_log.to_string_lossy());
    let msi = plan.msi.to_string_lossy();
    // Start-Process は引数の配列を空白でつなぐだけなので、空白を含みうるパスは " で囲んで渡す。
    let msi_argument = literal(&format!("\"{msi}\""));
    let msiexec_log_argument = literal(&format!("\"{}\"", plan.msiexec_log.to_string_lossy()));
    let msiexec_log = literal(&plan.msiexec_log.to_string_lossy());
    let exe = literal(&plan.exe.to_string_lossy());
    let work_dir = literal(&plan.work_dir.to_string_lossy());
    let msiexec_under_root = literal(MSIEXEC_UNDER_SYSTEM_ROOT);
    let msi = literal(&msi);
    let msi_sha256 = literal(plan.msi_sha256);
    let wait_seconds = plan.wait_seconds;
    let unknown_exit_code = UNKNOWN_EXIT_CODE;
    format!(
        r#"$ErrorActionPreference = 'Stop'
$log = {log}
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $log) | Out-Null
function Write-Result([string]$text) {{
    Add-Content -LiteralPath $log -Encoding UTF8 -Value "$(Get-Date -Format s) $text"
}}
$deadline = (Get-Date).AddSeconds({wait_seconds})
foreach ($id in @({pids})) {{
    $left = [Math]::Max(1, [int][Math]::Ceiling(($deadline - (Get-Date)).TotalSeconds))
    Wait-Process -Id $id -Timeout $left -ErrorAction SilentlyContinue
}}
$msiexec = Join-Path $env:SystemRoot {msiexec_under_root}
try {{
    $actual = (Get-FileHash -LiteralPath {msi} -Algorithm SHA256).Hash
}} catch {{
    Write-Result "could not read the SHA-256 of the installer: $_"
    exit 1
}}
if ($actual -ine {msi_sha256}) {{
    Write-Result ("the SHA-256 of the installer is {{0}}, not {{1}}" -f $actual, {msi_sha256})
    exit 1
}}
Write-Result ("installing {{0}}" -f {msi})
try {{
    $installer = Start-Process -FilePath $msiexec -ArgumentList @('/i', {msi_argument}, '/qn', '/norestart', '/l*v', {msiexec_log_argument}) -Wait -PassThru
    $code = $installer.ExitCode
}} catch {{
    Write-Result "msiexec could not be started: $_"
    exit 1
}}
if ($null -eq $code) {{
    Write-Result ("msiexec finished but its exit code could not be read (see {{0}})" -f {msiexec_log})
    exit {unknown_exit_code}
}}
if ($code -ne 0 -and $code -ne 3010) {{
    Write-Result ("msiexec failed with exit code {{0}} (see {{1}})" -f $code, {msiexec_log})
    exit $code
}}
Write-Result "msiexec succeeded with exit code $code"
try {{
    Remove-Item -LiteralPath {work_dir} -Recurse -Force
}} catch {{
    Write-Result "could not remove the downloaded files: $_"
}}
try {{
    Start-Process -FilePath {exe}
}} catch {{
    Write-Result "could not start fantermo: $_"
    exit 1
}}
"#
    )
}

/// PowerShell の `'` で囲む文字列。中の `'` は二つ重ねる。
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// スクリプトを `dir` に書く。BOM を付けた UTF-8 で、利用者の名前などに日本語を含むパスも読める。
pub fn write_script(dir: &Path, script: &str) -> io::Result<PathBuf> {
    let path = dir.join(SCRIPT_NAME);
    let mut bytes = UTF8_BOM.to_vec();
    bytes.extend_from_slice(script.as_bytes());
    std::fs::write(&path, bytes)?;
    Ok(path)
}

#[cfg(not(feature = "e2e"))]
pub use self::launcher::launch;
#[cfg(feature = "e2e")]
pub use super::stand_in::launch;

/// スクリプトの起動。feature `e2e` のときはコンパイルしない。
#[cfg(not(feature = "e2e"))]
mod launcher {
    use std::{
        io,
        os::windows::process::CommandExt,
        path::Path,
        process::{Command, Stdio},
    };

    use windows::Win32::System::Threading::CREATE_NO_WINDOW;

    use crate::known_folder;

    /// システムのフォルダ（`System32`）からの PowerShell の場所。PATH から探さない。
    const POWERSHELL_UNDER_SYSTEM: &str = r"WindowsPowerShell\v1.0\powershell.exe";

    /// スクリプト `path` を、ウィンドウを出さない PowerShell で起動する。終わるのは待たない。
    pub fn launch(path: &Path) -> io::Result<()> {
        let powershell = known_folder::system()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the System folder is unknown"))?
            .join(POWERSHELL_UNDER_SYSTEM);
        Command::new(powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-WindowStyle",
                "Hidden",
                "-File",
            ])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .map(drop)
    }
}

/// 実行ファイルが `exe` と同じ、ほかのプロセスの ID。自分（`own`）は除く。
pub fn other_instances(exe: &Path, own: u32) -> Vec<u32> {
    let pids = match process::ids() {
        Ok(pids) => pids,
        Err(e) => {
            tracing::warn!("failed to list the processes: {e}");
            return Vec::new();
        }
    };
    pids.into_iter()
        .filter(|&pid| pid != own)
        .filter(|&pid| image_of(pid).is_some_and(|image| same_path(Path::new(&image), exe)))
        .collect()
}

/// プロセス `pid` の実行ファイルのパス。問い合わせられなければ `None`。
fn image_of(pid: u32) -> Option<String> {
    let process = process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
    let mut buffer = vec![0u16; IMAGE_NAME_CAPACITY];
    let mut length = buffer.len() as u32;
    // SAFETY: process は開いたハンドルで、buffer は length 個の u16 を書き込める。
    unsafe {
        QueryFullProcessImageNameW(
            HANDLE(process.as_raw_handle()),
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .ok()?;
    Some(String::from_utf16_lossy(&buffer[..length as usize]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MSI_SHA256: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn plan_script(wait_pids: &[u32]) -> String {
        script(&Plan {
            msi: Path::new(r"C:\Users\o'brien\AppData\Local\Temp\fantermo-update-1\fantermo.msi"),
            msi_sha256: MSI_SHA256,
            exe: Path::new(r"C:\Users\o'brien\AppData\Local\Programs\fantermo\fantermo.exe"),
            result_log: Path::new(r"C:\Users\o'brien\AppData\Local\fantermo\logs\update-apply.log"),
            msiexec_log: Path::new(
                r"C:\Users\o'brien\AppData\Local\fantermo\logs\update-msiexec.log",
            ),
            work_dir: Path::new(r"C:\Users\o'brien\AppData\Local\Temp\fantermo-update-1"),
            wait_pids,
            wait_seconds: 300,
        })
    }

    #[test]
    fn literals_double_single_quotes() {
        assert_eq!(literal("a"), "'a'");
        assert_eq!(literal("o'brien"), "'o''brien'");
        assert_eq!(literal(""), "''");
    }

    #[test]
    fn script_waits_for_every_process_then_installs_quietly() {
        let script = plan_script(&[1234, 5678]);
        assert!(script.contains("foreach ($id in @(1234, 5678))"));
        assert!(script.contains("AddSeconds(300)"));
        assert!(script.contains(r"Join-Path $env:SystemRoot 'System32\msiexec.exe'"));
        assert!(script.contains(
            r#"@('/i', '"C:\Users\o''brien\AppData\Local\Temp\fantermo-update-1\fantermo.msi"', '/qn', '/norestart', '/l*v', '"C:\Users\o''brien\AppData\Local\fantermo\logs\update-msiexec.log"')"#
        ));
        assert!(
            script.contains(
                r"$log = 'C:\Users\o''brien\AppData\Local\fantermo\logs\update-apply.log'"
            )
        );
        assert!(script.contains(
            r"Start-Process -FilePath 'C:\Users\o''brien\AppData\Local\Programs\fantermo\fantermo.exe'"
        ));
        assert!(script.contains(
            r"Remove-Item -LiteralPath 'C:\Users\o''brien\AppData\Local\Temp\fantermo-update-1' -Recurse"
        ));
        // 待ってから入れ、入れてから起動し直す。
        let waits = script.find("Wait-Process").unwrap();
        let installs = script.find("Start-Process -FilePath $msiexec").unwrap();
        let restarts = script.find("Start-Process -FilePath 'C:").unwrap();
        assert!(waits < installs && installs < restarts);
    }

    #[test]
    fn script_checks_the_installer_just_before_installing() {
        let script = plan_script(&[1]);
        assert!(script.contains(
            r"Get-FileHash -LiteralPath 'C:\Users\o''brien\AppData\Local\Temp\fantermo-update-1\fantermo.msi' -Algorithm SHA256"
        ));
        assert!(script.contains(&format!("if ($actual -ine '{MSI_SHA256}')")));
        let waits = script.find("Wait-Process").unwrap();
        let hashes = script.find("Get-FileHash").unwrap();
        let mismatch = script.find("if ($actual -ine").unwrap();
        let exit = script[mismatch..].find("exit 1").unwrap() + mismatch;
        let installs = script.find("Start-Process -FilePath $msiexec").unwrap();
        assert!(waits < hashes && hashes < mismatch && exit < installs);
    }

    #[test]
    fn script_treats_a_missing_exit_code_as_a_failure() {
        let script = plan_script(&[1]);
        assert!(script.contains("if ($null -eq $code) {"));
        assert!(script.contains("its exit code could not be read"));
        let missing = script.find("if ($null -eq $code)").unwrap();
        let exit = script[missing..].find("exit -1").unwrap() + missing;
        let check = script.find("if ($code -ne 0 -and $code -ne 3010)").unwrap();
        assert!(exit < check);
    }

    #[test]
    fn script_restarts_only_after_success() {
        let script = plan_script(&[1]);
        let check = script.find("if ($code -ne 0 -and $code -ne 3010)").unwrap();
        let exit = script[check..].find("exit $code").unwrap() + check;
        let restarts = script.find("Start-Process -FilePath 'C:").unwrap();
        assert!(exit < restarts);
    }

    #[test]
    fn script_is_written_with_a_bom() {
        let dir = std::env::temp_dir().join(format!("fantermo-apply-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = write_script(&dir, "Write-Output 'あ'\n").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        assert_eq!(path.file_name().unwrap(), SCRIPT_NAME);
        assert!(bytes.starts_with(UTF8_BOM));
        assert_eq!(&bytes[UTF8_BOM.len()..], "Write-Output 'あ'\n".as_bytes());
    }

    #[test]
    fn own_process_is_not_another_instance() {
        let exe = std::env::current_exe().unwrap();
        assert!(!other_instances(&exe, std::process::id()).contains(&std::process::id()));
    }
}
