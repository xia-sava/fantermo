//! 管理者として開く。
//!
//! 管理者の権限の fantermo は、別のプロセスとして `ShellExecuteExW` の `runas` で起動する。渡すのは引数と作業フォルダ
//! だけで、環境変数は渡らない（昇格したプロセスは利用者のトークンの環境で起動する）。引数は [`Options`] から組み立て
//! 直し、`--elevate` は入れない。昇格しないまま起動されたときに、起動したものがまた `runas` を求めて UAC の確認を
//! 繰り返さないため。
//!
//! このプロセスが管理者として動いているかは [`this_process`] で読む。feature `e2e` のときは、昇格の判定と起動を
//! `stand_in` で差し替え、`ShellExecuteExW` を呼ばない。

#[cfg(feature = "e2e")]
mod stand_in;

use std::{path::PathBuf, sync::OnceLock};

use windows::Win32::{Foundation::HWND, System::Threading::GetCurrentProcess};

use crate::{
    command_line::{END_OF_OPTIONS, NEW_WINDOW_OPTION, Options, PROFILE_OPTION},
    instance,
};

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
    /// 利用者が UAC の確認で断った。
    Cancelled,
    /// そのほかの失敗。
    Failed(windows::core::Error),
}

/// 起動に失敗したときに利用者に見せる文。
pub fn failure_message(error: &windows::core::Error) -> String {
    format!("管理者として起動できませんでした。\n\n{error}")
}

/// このプロセスが管理者として動いているか。読めなければ `None` で、警告を一度だけ残す。
///
/// `None` の扱いは呼ぶ側で決める。ほかのウィンドウへ起動を渡すときは渡さない側に、ほかでは管理者ではないとして扱う。
pub fn this_process() -> Option<bool> {
    #[cfg(feature = "e2e")]
    if stand_in::is_elevated() {
        return Some(true);
    }
    static ELEVATED: OnceLock<Option<bool>> = OnceLock::new();
    *ELEVATED.get_or_init(|| {
        // SAFETY: このプロセスの疑似ハンドルを得るだけ。
        let this = unsafe { GetCurrentProcess() };
        let elevated = instance::is_elevated(this);
        if elevated.is_none() {
            tracing::warn!("failed to tell whether this process is elevated");
        }
        elevated
    })
}

/// `options` を管理者として開く起動の指定にする。実行ファイルはこのプロセスのもの、作業フォルダはこのプロセスの
/// 作業フォルダ。
pub fn launch_request(options: &Options) -> Result<Launch, LaunchError> {
    let exe = std::env::current_exe().map_err(|e| LaunchError::Failed(e.into()))?;
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
/// 名前が `--` で始まるプロファイルと、名前が空のプロファイルは、`Options::parse` が名前のない `--profile` として
/// 読むので、同じものには戻らない。
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

    /// 配布物のビルドでは、e2e テストの環境変数を付けても昇格の判定は変わらない。`launch` は UAC の確認を出すので
    /// 呼ばない。
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
        let elevated = this_process();
        for (name, _) in VARIABLES {
            // SAFETY: 同上。
            unsafe { std::env::remove_var(name) };
        }
        // SAFETY: このプロセスの疑似ハンドルを得るだけ。
        let this = unsafe { GetCurrentProcess() };
        assert_eq!(elevated, instance::is_elevated(this));
    }
}
