//! 管理者として開く。
//!
//! 管理者の権限の fantermo は、別のプロセスとして `ShellExecuteExW` の `runas` で起動する。渡すのは引数と作業フォルダ
//! だけで、環境変数は渡らない（昇格したプロセスは利用者のトークンの環境で起動する）。引数は [`Options`] から組み立て
//! 直し、`--elevate` は入れない。昇格しないまま起動されたときに、起動したものがまた `runas` を求めて UAC の確認を
//! 繰り返さないため。
//!
//! このプロセスが管理者として動いているかは [`this_process`] で読み、動いているウィンドウへ起動を渡すときの比べる値は
//! [`this_process_for_forwarding`] で読む。feature `e2e` のときは、昇格の判定と起動を `stand_in` で差し替え、
//! `ShellExecuteExW` を呼ばない。
//!
//! feature `e2e` の [`this_process`] は差し替えの指定だけで決め、本物のトークンを見ない。テストを動かすプロセスが
//! 管理者でも、指定のない fantermo は管理者ではないものとして動く。[`this_process_for_forwarding`] は、相手のウィンドウを
//! 本物のトークンで読むので、指定がなければ自分も本物のトークンで比べる。

#[cfg(feature = "e2e")]
mod stand_in;

use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::PathBuf,
    sync::OnceLock,
};

use windows::Win32::{
    Foundation::{HANDLE, HWND},
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

use crate::command_line::{END_OF_OPTIONS, NEW_WINDOW_OPTION, Options, PROFILE_OPTION};

/// 管理者として起動するもの。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// 実行ファイル。
    pub exe: PathBuf,
    /// コマンドラインの引数。
    pub parameters: String,
    /// 作業フォルダ。なければ指定しない。
    pub directory: Option<PathBuf>,
}

/// 管理者として起動できなかったわけ。
#[derive(Debug)]
pub enum LaunchError {
    /// `ShellExecuteExW` が `ERROR_CANCELLED` を返した。利用者が UAC の確認で断ったときのほか、確認の時間切れなども
    /// 含む。
    Cancelled,
    /// そのほかの失敗。
    Failed(windows::core::Error),
}

/// 起動に失敗したときに利用者に見せる文。
pub fn failure_message(error: &windows::core::Error) -> String {
    format!("管理者として起動できませんでした。\n\n{error}")
}

/// このプロセスが管理者として動いているか（ウィンドウの印、メニュー、更新、`--elevate`）。読めなければ `None` で、
/// 警告を一度だけ残す。呼ぶ側は `None` を管理者ではないとして扱う。
pub fn this_process() -> Option<bool> {
    elevation_of(stand_in_for_this_process(), this_token)
}

/// 動いているウィンドウへ起動を渡すときに、相手と比べるこのプロセスの昇格。読めなければ `None` で、呼ぶ側は渡さない。
pub fn this_process_for_forwarding() -> Option<bool> {
    elevation_of(stand_in_for_forwarding(), this_token)
}

/// 差し替えの指定 `stand_in` があればそれで、なければトークン `token` で決める。
fn elevation_of(stand_in: Option<bool>, token: impl FnOnce() -> Option<bool>) -> Option<bool> {
    stand_in.map_or_else(token, Some)
}

/// [`this_process`] の差し替えの指定。指定がなければ管理者ではない。
#[cfg(feature = "e2e")]
fn stand_in_for_this_process() -> Option<bool> {
    Some(stand_in::is_elevated())
}

/// [`this_process_for_forwarding`] の差し替えの指定。管理者として扱うよう指定されたときだけ。
#[cfg(feature = "e2e")]
fn stand_in_for_forwarding() -> Option<bool> {
    stand_in::is_elevated().then_some(true)
}

/// 配布物のビルドは差し替えない。
#[cfg(not(feature = "e2e"))]
fn stand_in_for_this_process() -> Option<bool> {
    None
}

/// 配布物のビルドは差し替えない。
#[cfg(not(feature = "e2e"))]
fn stand_in_for_forwarding() -> Option<bool> {
    None
}

/// このプロセスのトークンが昇格しているか。一度だけ読む。
fn this_token() -> Option<bool> {
    static ELEVATED: OnceLock<Option<bool>> = OnceLock::new();
    *ELEVATED.get_or_init(|| {
        // SAFETY: このプロセスの疑似ハンドルを得るだけ。
        let this = unsafe { GetCurrentProcess() };
        let elevated = is_elevated(this);
        if elevated.is_none() {
            tracing::warn!("failed to tell whether this process is elevated");
        }
        elevated
    })
}

/// プロセス `process` のトークンが昇格しているか。読めなければ `None`。
pub fn is_elevated(process: HANDLE) -> Option<bool> {
    let mut token = HANDLE::default();
    // SAFETY: process は有効なハンドルで、token は書き込める。
    if let Err(e) = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } {
        tracing::debug!("cannot open the token of a process: {e}");
        return None;
    }
    // SAFETY: OpenProcessToken が成功したので、ハンドルは有効で、ほかに持ち主はいない。
    let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
    let mut elevation = TOKEN_ELEVATION::default();
    let mut written = 0;
    // SAFETY: elevation は指定した大きさの分だけ書き込める。
    let read = unsafe {
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenElevation,
            Some(std::ptr::from_mut(&mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut written,
        )
    };
    if let Err(e) = read {
        tracing::debug!("cannot read the token of a process: {e}");
        return None;
    }
    Some(elevation.TokenIsElevated != 0)
}

/// `options` を管理者として開く起動の指定にする。実行ファイルはこのプロセスのもの、作業フォルダはこのプロセスの
/// 作業フォルダ。
pub fn launch_request(options: &Options) -> Result<Launch, LaunchError> {
    let exe = std::env::current_exe().map_err(|e| {
        tracing::warn!("failed to get the path of the executable: {e}");
        LaunchError::Failed(e.into())
    })?;
    let directory = match std::env::current_dir() {
        Ok(folder) => Some(folder),
        Err(e) => {
            tracing::warn!("failed to get the working folder: {e}");
            None
        }
    };
    Ok(Launch {
        exe,
        parameters: parameters(options),
        directory,
    })
}

/// `options` を、`Options::parse` で同じものに読めるコマンドラインの引数にする。`--elevate` は入れない。
///
/// 名前が `--` で始まるプロファイルは、`Options::parse` が名前のない `--profile` として読むので、同じものには戻らない。
fn parameters(options: &Options) -> String {
    let mut words = Vec::new();
    if options.new_window {
        words.push(NEW_WINDOW_OPTION.to_owned());
    }
    // 名前のない `--profile` の後ろに名前のある `--profile` を置くと、両方が読まれる。
    if options.profile_without_name {
        words.push(PROFILE_OPTION.to_owned());
    }
    if let Some(name) = &options.tab.profile {
        words.push(PROFILE_OPTION.to_owned());
        words.push(quote_argument(name));
    }
    if let Some(command) = &options.tab.command {
        words.push(END_OF_OPTIONS.to_owned());
        words.push(command.clone());
    }
    words.join(" ")
}

/// `text` を一つの語として `"` で囲む。`CommandLineToArgvW` の決まり（`command_line` の語の切り方）の逆で、`"` は
/// `\"` にし、`"` の直前と末尾の `\` は 2 つにする。
fn quote_argument(text: &str) -> String {
    let mut quoted = String::from('"');
    let mut backslashes = 0;
    for c in text.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(c);
                backslashes = 0;
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

/// `request` を管理者として起動する。UAC の確認が閉じるまで戻らない。`owner` は UAC の確認の持ち主のウィンドウ。
#[cfg(not(feature = "e2e"))]
pub fn launch(owner: Option<HWND>, request: &Launch) -> Result<(), LaunchError> {
    use windows::{
        Win32::{
            Foundation::ERROR_CANCELLED,
            UI::{
                Shell::{
                    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
                },
                WindowsAndMessaging::SW_SHOWNORMAL,
            },
        },
        core::{HSTRING, PCWSTR, w},
    };

    let exe = HSTRING::from(request.exe.as_os_str());
    let parameters = HSTRING::from(request.parameters.as_str());
    let directory = request.directory.as_deref().map(HSTRING::from);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // 起動を待ってから戻る。失敗は呼ぶ側が見せるので、OS のエラーのダイアログは出さない。
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        hwnd: owner.unwrap_or_default(),
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        lpDirectory: directory
            .as_ref()
            .map_or(PCWSTR::null(), |directory| PCWSTR(directory.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: info は初期化済みで、指す文字列はこの呼び出しの間は生きている。
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| {
        if e.code() == ERROR_CANCELLED.to_hresult() {
            LaunchError::Cancelled
        } else {
            LaunchError::Failed(e)
        }
    })
}

/// テストが指定したとおりに、起動を記録するか失敗する。UAC の確認は出さない。
#[cfg(feature = "e2e")]
pub fn launch(_owner: Option<HWND>, request: &Launch) -> Result<(), LaunchError> {
    stand_in::launch(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_line::TabRequest;

    fn options(profile: Option<&str>, command: Option<&str>) -> Options {
        Options {
            tab: TabRequest {
                profile: profile.map(str::to_owned),
                command: command.map(str::to_owned),
            },
            ..Options::default()
        }
    }

    #[test]
    fn arguments_are_quoted_as_one_word() {
        assert_eq!(quote_argument("one"), r#""one""#);
        assert_eq!(quote_argument("a b"), r#""a b""#);
        assert_eq!(quote_argument("R&D"), r#""R&D""#);
        assert_eq!(quote_argument(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote_argument(r"a\"), r#""a\\""#);
        assert_eq!(quote_argument(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(quote_argument(r"a\b"), r#""a\b""#);
        assert_eq!(quote_argument(""), r#""""#);
    }

    #[test]
    fn parameters_are_read_back_as_the_same_options() {
        let names = [
            "one",
            "a b",
            "R&D",
            r#"a"b"#,
            r"a\",
            r#"a\"b"#,
            r"C:\x\\",
            "日本語",
            "-x",
        ];
        let commands = [
            None,
            Some("cmd.exe"),
            Some(r#""C:\a b\zsh.exe" -c "echo  'x'"  "#),
            Some("--elevate --profile x"),
        ];
        let mut cases = Vec::new();
        for name in names {
            for command in commands {
                cases.push(options(Some(name), command));
            }
        }
        for command in commands {
            cases.push(options(None, command));
            cases.push(Options {
                profile_without_name: true,
                ..options(None, command)
            });
            cases.push(Options {
                profile_without_name: true,
                ..options(Some("one"), command)
            });
        }
        for case in cases {
            for new_window in [false, true] {
                let expected = Options {
                    new_window,
                    ..case.clone()
                };
                let elevated = Options {
                    elevate: true,
                    ..expected.clone()
                };
                let text = parameters(&elevated);
                assert_eq!(Options::parse(&text), expected, "{text}");
            }
        }
    }

    #[test]
    fn parameters_have_the_expected_shape() {
        assert_eq!(parameters(&Options::default()), "");
        assert_eq!(
            parameters(&options(Some("one"), Some("cmd.exe /k x"))),
            r#"--profile "one" -- cmd.exe /k x"#
        );
        let options = Options {
            new_window: true,
            elevate: true,
            profile_without_name: true,
            ..options(None, Some("vim"))
        };
        assert_eq!(parameters(&options), "--new-window --profile -- vim");
    }

    #[test]
    fn the_request_starts_this_executable_in_this_folder() {
        let request = launch_request(&options(Some("one"), None)).unwrap();
        assert_eq!(request.exe, std::env::current_exe().unwrap());
        assert_eq!(request.directory, Some(std::env::current_dir().unwrap()));
        assert_eq!(request.parameters, r#"--profile "one""#);
    }

    #[test]
    fn the_failure_message_names_the_error() {
        let error = windows::core::Error::from_hresult(windows::core::HRESULT(-1));
        let message = failure_message(&error);
        assert!(message.starts_with("管理者として起動できませんでした。\n\n"));
        assert!(message.ends_with(&error.to_string()));
    }

    #[test]
    fn a_stand_in_decides_without_reading_the_token() {
        let unread = || panic!("the token is read");
        assert_eq!(elevation_of(Some(true), unread), Some(true));
        assert_eq!(elevation_of(Some(false), unread), Some(false));
        assert_eq!(elevation_of(None, || Some(true)), Some(true));
        assert_eq!(elevation_of(None, || Some(false)), Some(false));
        assert_eq!(elevation_of(None, || None), None);
    }

    #[test]
    fn this_process_reads_its_own_token() {
        // SAFETY: このプロセスの疑似ハンドルを得るだけ。
        let this = unsafe { GetCurrentProcess() };
        assert!(is_elevated(this).is_some());
        assert_eq!(this_token(), is_elevated(this));
    }

    /// 配布物のビルドは差し替えの指定を持たず、どちらの判定もトークンで決める。e2e テストの環境変数を付けても変わらない。
    /// `launch` は UAC の確認を出すので呼ばない。
    #[cfg(not(feature = "e2e"))]
    #[test]
    fn the_e2e_variables_change_nothing_without_the_feature() {
        const VARIABLES: [(&str, &str); 3] = [
            ("FANTERMO_E2E_ELEVATED", "1"),
            ("FANTERMO_E2E_ELEVATE_RECORD", "record.txt"),
            ("FANTERMO_E2E_ELEVATE_RESULT", "failed"),
        ];
        for (name, value) in VARIABLES {
            // SAFETY: Windows の環境変数は OS が直列にする。ほかのテストはこの名前を読まない。
            unsafe { std::env::set_var(name, value) };
        }
        let stand_ins = (stand_in_for_this_process(), stand_in_for_forwarding());
        for (name, _) in VARIABLES {
            // SAFETY: 同上。
            unsafe { std::env::remove_var(name) };
        }
        assert_eq!(stand_ins, (None, None));
    }

    /// feature `e2e` のビルドは、指定がなければ、テストを動かすプロセスが管理者でも [`this_process`] を管理者では
    /// ないとし、起動を渡すときの比べる値は本物のトークンにする。ほかのテストが `FANTERMO_E2E_ELEVATED` を付けない
    /// ことを前提にする。
    #[cfg(feature = "e2e")]
    #[test]
    fn without_a_stand_in_the_e2e_build_is_not_elevated_but_forwards_by_the_token() {
        assert!(std::env::var_os("FANTERMO_E2E_ELEVATED").is_none());
        assert_eq!(stand_in_for_this_process(), Some(false));
        assert_eq!(this_process(), Some(false));
        assert_eq!(stand_in_for_forwarding(), None);
        assert_eq!(this_process_for_forwarding(), this_token());
    }
}
