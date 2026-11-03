//! すでに動いている fantermo のウィンドウへ、起動の指定を `WM_COPYDATA` で渡す。
//!
//! 探すのは同じデスクトップのウィンドウだけで（`EnumWindows`）、Z オーダーの順、つまり最後に使ったものから
//! 順に渡す。受け取ったウィンドウはタブを足し、起動したプロセスは終わる。渡せるウィンドウがなければ、起動した
//! プロセスが新しいウィンドウを開く。次のウィンドウには渡さない。
//! - 実行ファイルか設定ファイルが違うウィンドウ（開発中のビルドと、インストールしたものなど）。受け取る側が断る。
//! - 管理者として動いているかが違うウィンドウ。管理者として起動したものが普通のタブになったり、その逆になったり
//!   しないよう、送る側が飛ばす。
//! - 応答しないウィンドウ。[`SEND_TIMEOUT`] まで待つ。
//!
//! 受け取った内容は外から来た入力として扱う（[`Forwarded::decode`]）。Windows の既定により、整合性レベルが低い
//! プロセスからは届かない。

use std::{
    ffi::{OsString, c_void},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::{HANDLE, HWND, LPARAM, TRUE, WPARAM},
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::{
            DataExchange::COPYDATASTRUCT,
            Threading::{GetCurrentProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION},
        },
        UI::WindowsAndMessaging::{
            AllowSetForegroundWindow, EnumWindows, GetClassNameW, GetWindowThreadProcessId,
            IsWindowVisible, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_COPYDATA,
        },
    },
    core::BOOL,
};

use crate::{
    app_files::same_path,
    command_line::{Options, TabRequest},
    process,
    window::WINDOW_CLASS,
};

/// `COPYDATASTRUCT` の `dwData`。fantermo の起動の指定であることと、形の版を示す。形を変えたら値を変える。
pub const LAUNCH_DATA_KIND: usize = 0x4654_4D01;

/// 受け取ったウィンドウがウィンドウプロシージャから返す値。
pub const ACCEPTED: isize = 1;

/// 本文の欄の数。順に、指定の誤りの印、実行ファイル、設定ファイル、作業フォルダ、プロファイルの名前、コマンドライン。
const FIELD_COUNT: usize = 6;
/// 欄の区切り（各欄の終わりに置く）。
const FIELD_END: u16 = 0;
/// 指定の誤りの印の値。
const FLAG_OFF: &[u16] = &[b'0' as u16];
const FLAG_ON: &[u16] = &[b'1' as u16];
/// 一つの欄の長さの上限（UTF-16 の単位。Windows のコマンドラインの上限）。
const MAX_FIELD_UNITS: usize = 32_767;
/// プロファイルの名前の長さの上限（UTF-16 の単位）。受け取った名前を警告の文に入れるので抑える。
const MAX_PROFILE_NAME_UNITS: usize = 1024;
/// 本文の長さの上限（UTF-16 の単位）。
const MAX_UNITS: usize = FIELD_COUNT * (MAX_FIELD_UNITS + 1);

/// すべてのウィンドウへ渡そうとするのにかける時間の上限。
const SEND_TIMEOUT: Duration = Duration::from_secs(3);

/// ウィンドウのクラスの名前の長さの上限（Windows の決まり）。
const MAX_CLASS_NAME: usize = 256;

/// 別のプロセスから渡された起動の指定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forwarded {
    /// 起動したプロセスの実行ファイル。
    pub exe: PathBuf,
    /// 起動したプロセスの設定ファイル。
    pub config: Option<PathBuf>,
    /// 起動したプロセスの作業フォルダ。絶対パス。
    pub working_folder: Option<PathBuf>,
    pub options: Options,
}

impl Forwarded {
    /// `COPYDATASTRUCT` の本文にする。
    fn encode(&self) -> Vec<u16> {
        let path_units = |path: Option<&Path>| -> Vec<u16> {
            path.map(|path| path.as_os_str().encode_wide().collect())
                .unwrap_or_default()
        };
        let text_units = |text: Option<&str>| -> Vec<u16> {
            text.map(|text| text.encode_utf16().collect())
                .unwrap_or_default()
        };
        let flag = if self.options.profile_without_name {
            FLAG_ON
        } else {
            FLAG_OFF
        };
        let fields = [
            flag.to_vec(),
            path_units(Some(&self.exe)),
            path_units(self.config.as_deref()),
            path_units(self.working_folder.as_deref()),
            text_units(self.options.tab.profile.as_deref()),
            text_units(self.options.tab.command.as_deref()),
        ];
        let mut units = Vec::new();
        for field in &fields {
            units.extend_from_slice(field);
            units.push(FIELD_END);
        }
        units
    }

    /// `WM_COPYDATA` で受け取ったものを読む。形が合わなければ、その理由を返す。
    pub fn decode(kind: usize, bytes: &[u8]) -> Result<Self, String> {
        if kind != LAUNCH_DATA_KIND {
            return Err(format!("unknown kind {kind:#x}"));
        }
        if bytes.is_empty() || !bytes.len().is_multiple_of(2) || bytes.len() / 2 > MAX_UNITS {
            return Err(format!("unexpected length of {} bytes", bytes.len()));
        }
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_le_bytes(pair))
            .collect();
        let Some(body) = units.strip_suffix(&[FIELD_END]) else {
            return Err("the last field is not terminated".to_owned());
        };
        let fields: Vec<&[u16]> = body.split(|&unit| unit == FIELD_END).collect();
        let [flag, exe, config, working_folder, profile, command] = fields[..] else {
            return Err(format!("{} fields", fields.len()));
        };
        if fields.iter().any(|field| field.len() > MAX_FIELD_UNITS) {
            return Err("a field is too long".to_owned());
        }
        if profile.len() > MAX_PROFILE_NAME_UNITS {
            return Err("the profile name is too long".to_owned());
        }
        let profile_without_name = match flag {
            FLAG_OFF => false,
            FLAG_ON => true,
            _ => return Err("unknown flag".to_owned()),
        };
        if exe.is_empty() {
            return Err("no executable".to_owned());
        }
        let working_folder = optional_path(working_folder);
        if working_folder
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err("the working folder is not absolute".to_owned());
        }
        Ok(Self {
            exe: PathBuf::from(OsString::from_wide(exe)),
            config: optional_path(config),
            working_folder,
            options: Options {
                tab: TabRequest {
                    profile: optional_text(profile)?,
                    command: optional_text(command)?,
                },
                profile_without_name,
                new_window: false,
            },
        })
    }

    /// `exe` の実行ファイルで設定ファイルが `config` のウィンドウが受け取るものか。
    pub fn is_for(&self, exe: &Path, config: Option<&Path>) -> bool {
        same_path(&self.exe, exe)
            && match (self.config.as_deref(), config) {
                (Some(left), Some(right)) => same_path(left, right),
                (None, None) => true,
                _ => false,
            }
    }

    /// 作業フォルダ。渡されたあとに消えていれば `None`。
    pub fn usable_working_folder(&self) -> Option<&Path> {
        let folder = self.working_folder.as_deref()?;
        if folder.is_dir() {
            return Some(folder);
        }
        tracing::warn!(
            "ignoring the working folder of a launch: {}",
            folder.display()
        );
        None
    }

    /// 受け取ったウィンドウがタブを足さないまま閉じるので、新しいウィンドウで開き直す。
    pub fn start_in_new_window(&self) {
        let mut command = Command::new(&self.exe);
        command.arg(crate::command_line::NEW_WINDOW_OPTION);
        if let Some(profile) = &self.options.tab.profile {
            command
                .arg(crate::command_line::PROFILE_OPTION)
                .arg(profile);
        }
        if let Some(line) = &self.options.tab.command {
            command
                .arg(crate::command_line::END_OF_OPTIONS)
                .raw_arg(line);
        }
        if let Some(folder) = self.usable_working_folder() {
            command.current_dir(folder);
        }
        if let Err(e) = command.spawn() {
            tracing::warn!("failed to start a new window for a forwarded launch: {e}");
        }
    }
}

fn optional_path(field: &[u16]) -> Option<PathBuf> {
    (!field.is_empty()).then(|| PathBuf::from(OsString::from_wide(field)))
}

fn optional_text(field: &[u16]) -> Result<Option<String>, String> {
    if field.is_empty() {
        return Ok(None);
    }
    String::from_utf16(field)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// `WM_COPYDATA` の `lparam` の中身を読む。読めなければ `None`。
///
/// # Safety
/// `lparam` は `WM_COPYDATA` で受け取ったものそのままで、ウィンドウプロシージャから戻るまでに呼ぶ。
pub unsafe fn received(lparam: LPARAM) -> Option<Forwarded> {
    // SAFETY: OS が受け取り側に用意した、このメッセージの間だけ有効な構造体。
    let Some(data) = (unsafe { (lparam.0 as *const COPYDATASTRUCT).as_ref() }) else {
        tracing::warn!("ignoring WM_COPYDATA without data");
        return None;
    };
    let length = data.cbData as usize;
    if length == 0 || length / 2 > MAX_UNITS || data.lpData.is_null() {
        tracing::warn!("ignoring WM_COPYDATA of {length} bytes");
        return None;
    }
    // SAFETY: OS が cbData バイトを写した領域で、このメッセージの間だけ有効。バイトとして読むので並びは問わない。
    let bytes = unsafe { std::slice::from_raw_parts(data.lpData.cast::<u8>(), length) };
    match Forwarded::decode(data.dwData, bytes) {
        Ok(forwarded) => Some(forwarded),
        Err(reason) => {
            tracing::warn!("ignoring a malformed WM_COPYDATA: {reason}");
            None
        }
    }
}

/// 同じデスクトップで動いている fantermo のウィンドウへ `options` を渡す。受け取られたら `true`。
pub fn forward(options: &Options, config: Option<&Path>) -> bool {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            tracing::warn!("failed to get the path of the executable: {e}");
            return false;
        }
    };
    let working_folder = match std::env::current_dir() {
        Ok(folder) => Some(folder),
        Err(e) => {
            tracing::warn!("failed to get the working folder: {e}");
            None
        }
    };
    let units = Forwarded {
        exe,
        config: config.map(Path::to_owned),
        working_folder,
        options: options.clone(),
    }
    .encode();
    let deadline = Instant::now() + SEND_TIMEOUT;
    for window in windows_of_this_desktop() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            tracing::info!("gave up handing the launch to a window");
            return false;
        }
        if send_to(window, &units, remaining) {
            return true;
        }
    }
    false
}

/// 同じデスクトップの見えている fantermo のウィンドウ。Z オーダーの順。
fn windows_of_this_desktop() -> Vec<HWND> {
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: windows はこの呼び出しの間は生きていて、コールバックはこの呼び出しの中でだけ呼ばれる。
    let listed = unsafe {
        EnumWindows(
            Some(collect_window),
            LPARAM(std::ptr::from_mut(&mut windows) as isize),
        )
    };
    // デスクトップにウィンドウが一つもないと、エラーの番号なしで失敗を返す。
    if let Err(e) = listed
        && e.code().is_err()
    {
        tracing::warn!("failed to list the windows: {e}");
    }
    windows
}

extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam は [`windows_of_this_desktop`] の `Vec<HWND>` を指し、列挙している間は生きている。
    let windows = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    let mut class = [0u16; MAX_CLASS_NAME];
    // SAFETY: class は書き込める。
    let length = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
    // SAFETY: WINDOW_CLASS は静的な終端付きの文字列。
    let ours = unsafe { WINDOW_CLASS.as_wide() };
    // SAFETY: ウィンドウの状態を読むだけ。
    if &class[..length] == ours && unsafe { IsWindowVisible(hwnd) }.as_bool() {
        windows.push(hwnd);
    }
    TRUE
}

/// `window` へ `units` を送る。受け取られたら `true`。
fn send_to(window: HWND, units: &[u16], timeout: Duration) -> bool {
    let mut pid = 0;
    // SAFETY: pid は書き込める。
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    if !same_elevation_as_this_process(pid) {
        tracing::info!("skipping the window of process {pid}: it may run with other rights");
        return false;
    }
    // 受け取ったウィンドウが前面に出られるようにする。このプロセスが前面に出す権利を持たなければ失敗し、
    // ウィンドウはタスクバーで知らせることになる。
    // SAFETY: プロセスの番号を渡すだけ。
    if let Err(e) = unsafe { AllowSetForegroundWindow(pid) } {
        tracing::debug!("cannot let process {pid} come to the front: {e}");
    }
    let data = COPYDATASTRUCT {
        dwData: LAUNCH_DATA_KIND,
        cbData: size_of_val(units) as u32,
        lpData: units.as_ptr().cast_mut().cast::<c_void>(),
    };
    let mut result = 0;
    // SAFETY: data と units はこの呼び出しの間は生きている。OS が相手のプロセスへ写す。
    let sent = unsafe {
        SendMessageTimeoutW(
            window,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(std::ptr::from_ref(&data) as isize),
            SMTO_ABORTIFHUNG,
            timeout.as_millis() as u32,
            Some(&mut result),
        )
    };
    if sent.0 == 0 {
        tracing::info!(
            "the window of process {pid} did not take the launch: {}",
            windows::core::Error::from_thread()
        );
        return false;
    }
    if result as isize != ACCEPTED {
        tracing::info!("the window of process {pid} refused the launch");
        return false;
    }
    tracing::info!("handed the launch to the window of process {pid}");
    true
}

/// プロセス `pid` が、管理者として動いているかどうかでこのプロセスと同じか。読めなければ `false`。
fn same_elevation_as_this_process(pid: u32) -> bool {
    // 読み取りの権限で開く。
    let process = match process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) {
        Ok(process) => process,
        Err(e) => {
            tracing::debug!("cannot open process {pid}: {e}");
            return false;
        }
    };
    // SAFETY: このプロセスの疑似ハンドルを得るだけ。
    let this = unsafe { GetCurrentProcess() };
    match (
        is_elevated(HANDLE(process.as_raw_handle())),
        is_elevated(this),
    ) {
        (Some(other), Some(this)) => other == this,
        _ => false,
    }
}

/// プロセス `process` が管理者として動いているか。読めなければ `None`。
pub(crate) fn is_elevated(process: HANDLE) -> Option<bool> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Forwarded {
        Forwarded {
            exe: PathBuf::from(r"C:\Program Files\fantermo\fantermo.exe"),
            config: Some(PathBuf::from(r"C:\conf\config.toml")),
            working_folder: Some(PathBuf::from(r"D:\作業")),
            options: Options {
                tab: TabRequest {
                    profile: Some("プロファイル".to_owned()),
                    command: Some(r#"cmd.exe /k "a  b""#.to_owned()),
                },
                profile_without_name: true,
                new_window: false,
            },
        }
    }

    fn bytes_of(units: &[u16]) -> Vec<u8> {
        units.iter().flat_map(|unit| unit.to_le_bytes()).collect()
    }

    fn decode_units(units: &[u16]) -> Result<Forwarded, String> {
        Forwarded::decode(LAUNCH_DATA_KIND, &bytes_of(units))
    }

    /// 欄を `fields` にした本文。
    fn units_of(fields: &[&str]) -> Vec<u16> {
        let mut units = Vec::new();
        for field in fields {
            units.extend(field.encode_utf16());
            units.push(FIELD_END);
        }
        units
    }

    #[test]
    fn what_is_encoded_is_decoded_again() {
        let forwarded = sample();
        assert_eq!(decode_units(&forwarded.encode()), Ok(forwarded));

        let bare = Forwarded {
            config: None,
            working_folder: None,
            options: Options::default(),
            ..sample()
        };
        assert_eq!(decode_units(&bare.encode()), Ok(bare));
    }

    #[test]
    fn other_kinds_are_refused() {
        let bytes = bytes_of(&sample().encode());
        assert!(Forwarded::decode(LAUNCH_DATA_KIND + 1, &bytes).is_err());
        assert!(Forwarded::decode(0, &bytes).is_err());
    }

    #[test]
    fn lengths_are_checked() {
        let bytes = bytes_of(&sample().encode());
        assert!(Forwarded::decode(LAUNCH_DATA_KIND, &[]).is_err());
        assert!(Forwarded::decode(LAUNCH_DATA_KIND, &bytes[..bytes.len() - 1]).is_err());
        assert!(Forwarded::decode(LAUNCH_DATA_KIND, &vec![b'a'; (MAX_UNITS + 1) * 2]).is_err());

        let long = "x".repeat(MAX_FIELD_UNITS + 1);
        assert!(decode_units(&units_of(&["0", "a.exe", "", "", "", &long])).is_err());
        let long_name = "x".repeat(MAX_PROFILE_NAME_UNITS + 1);
        assert!(decode_units(&units_of(&["0", "a.exe", "", "", &long_name, ""])).is_err());
    }

    #[test]
    fn the_shape_is_checked() {
        assert!(decode_units(&units_of(&["0", "a.exe", "", "", ""])).is_err());
        assert!(decode_units(&units_of(&["0", "a.exe", "", "", "", "", "more"])).is_err());
        assert!(decode_units(&units_of(&["2", "a.exe", "", "", "", ""])).is_err());
        assert!(decode_units(&units_of(&["", "a.exe", "", "", "", ""])).is_err());
        assert!(decode_units(&units_of(&["0", "", "", "", "", ""])).is_err());
        assert!(decode_units(&units_of(&["0", "a.exe", "", "relative", "", ""])).is_err());
        // 最後の欄が終わっていない。
        let mut unterminated = units_of(&["0", "a.exe", "", "", "", "cmd"]);
        unterminated.pop();
        assert!(decode_units(&unterminated).is_err());
        // 文字にならない UTF-16。
        let mut broken = units_of(&["0", "a.exe", "", "", ""]);
        broken.extend([0xD800, FIELD_END]);
        assert!(decode_units(&broken).is_err());

        assert!(decode_units(&units_of(&["0", "a.exe", "", "", "", ""])).is_ok());
    }

    #[test]
    fn windows_take_launches_of_their_own_executable_and_settings() {
        let forwarded = sample();
        let exe = Path::new(r"c:\program files\FANTERMO\fantermo.exe");
        let config = Path::new(r"C:\CONF\config.toml");
        assert!(forwarded.is_for(exe, Some(config)));
        assert!(!forwarded.is_for(Path::new(r"C:\other\fantermo.exe"), Some(config)));
        assert!(!forwarded.is_for(exe, Some(Path::new(r"C:\conf2\config.toml"))));
        assert!(!forwarded.is_for(exe, None));
        let without_settings = Forwarded {
            config: None,
            ..sample()
        };
        assert!(without_settings.is_for(exe, None));
        assert!(!without_settings.is_for(exe, Some(config)));
    }

    #[test]
    fn this_process_has_the_same_elevation_as_itself() {
        assert!(same_elevation_as_this_process(std::process::id()));
    }
}
