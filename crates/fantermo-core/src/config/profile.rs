//! プロファイル（新しいタブで起動するもの）。接続方式、最上位の `shell` に書いたコマンドラインの
//! 解釈、Cygwin のフォルダと名前だけのコマンドの探索。

use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use super::{DEFAULT_COMMAND_LINE, DEFAULT_PROFILE_NAME, name_of, named_values, value_named};

/// プロファイルの並び `profiles` のうち、名前が `default` のものの位置。`None` か、その名前のものが
/// なければ先頭。
pub fn default_profile_index(profiles: &[Profile], default: Option<&str>) -> usize {
    default
        .and_then(|name| profile_position(profiles, name))
        .unwrap_or(0)
}

/// プロファイルの並び `profiles` のうち、名前が `name` のものの位置。名前は完全に一致するものを探す（設定
/// ダイアログの重複の判定と同じ）。
pub fn profile_position(profiles: &[Profile], name: &str) -> Option<usize> {
    profiles.iter().position(|profile| profile.name == name)
}

/// `[[profiles]]` のテーブルのキー。
pub struct ProfileKeys {
    pub name: &'static str,
    pub command: &'static str,
    pub backend: &'static str,
    pub cygwin_root: &'static str,
    pub distribution: &'static str,
    pub icon: &'static str,
}

/// `[[profiles]]` のテーブルのキー。読む処理と書き戻す処理が使う。
pub const PROFILE_KEYS: ProfileKeys = ProfileKeys {
    name: "name",
    command: "command",
    backend: "backend",
    cygwin_root: "cygwin_root",
    distribution: "distribution",
    icon: "icon",
};

/// プロファイルが決まりに合わないところ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// 名前が空（空白だけ）。
    EmptyName,
    /// ConPTY のプロファイルのコマンドラインが空（空白だけ）。
    EmptyCommand,
    /// 名前が、前に並ぶプロファイルと同じ。
    DuplicateName,
}

/// プロファイルの名前 `name` が使えるか。空（空白だけ）なら [`ProfileError::EmptyName`]。
pub(super) fn check_profile_name(name: &str) -> Result<(), ProfileError> {
    if name.trim().is_empty() {
        return Err(ProfileError::EmptyName);
    }
    Ok(())
}

/// 接続方式 `backend` のプロファイルのコマンドライン `command` が使えるか。ブリッジでは空でもよく
/// （相手のログインシェル）、ConPTY で空（空白だけ）なら [`ProfileError::EmptyCommand`]。
pub(super) fn check_profile_command(
    command: &str,
    backend: BackendKind,
) -> Result<(), ProfileError> {
    if command.trim().is_empty() && !backend.is_bridge() {
        return Err(ProfileError::EmptyCommand);
    }
    Ok(())
}

/// 新しいタブで起動するもの（`[[profiles]]` のテーブル一つ）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    /// タブバーの一覧に見せる名前。空ではない。
    pub name: String,
    /// 起動するコマンドライン。ConPTY では空ではない。ブリッジ（`cygwin`・`wsl`）では Cygwin・WSL の
    /// 側のコマンドラインで、空なら相手のログインシェル。
    pub command: String,
    pub backend: BackendKind,
    /// `backend = "cygwin"` の Cygwin のフォルダ。`None` なら [`find_cygwin_root`] で探す。ほかの種類では
    /// `None`。
    pub cygwin_root: Option<String>,
    /// `backend = "wsl"` のディストリビューションの名前。`None` なら既定のディストリビューション。
    /// ほかの種類では `None`。
    pub distribution: Option<String>,
    /// タブに出すアイコンの画像ファイル。書いたままの文字列で、相対パスは設定ファイルのフォルダから。
    /// `None` なら自動で作る。
    pub icon: Option<String>,
}

impl Profile {
    /// 名前が `name`、コマンドラインが `command` の、`backend` で接続するプロファイル。
    pub fn new(name: &str, command: &str, backend: BackendKind) -> Self {
        Self {
            name: name.to_owned(),
            command: command.to_owned(),
            backend,
            cygwin_root: None,
            distribution: None,
            icon: None,
        }
    }

    /// プロファイルも `shell` も書いていないときのプロファイル。
    pub fn builtin() -> Self {
        Self::new(
            DEFAULT_PROFILE_NAME,
            DEFAULT_COMMAND_LINE,
            BackendKind::default(),
        )
    }

    /// 最上位の `shell` に書いたコマンドライン `command` のプロファイル。名前は実行ファイルの名前。
    /// 接続方式は [`Profile::from_shell_with`] で決め、Cygwin のフォルダかどうかは [`is_cygwin_root`]
    /// で見る。
    pub fn from_shell(command: &str) -> Self {
        Self::from_shell_with(command, is_cygwin_root)
    }

    /// 最上位の `shell` に書いたコマンドライン `command` のプロファイル。`is_cygwin_root` はフォルダが
    /// Cygwin のフォルダか。
    ///
    /// - 実行ファイルが絶対パスで、そのフォルダの一つ上が Cygwin のフォルダなら（`<Cygwin>\bin\zsh.exe`
    ///   など）、`backend = "cygwin"`。`cygwin_root` はそのフォルダ、`command` は実行ファイルを Cygwin の
    ///   パス（`/bin/zsh`）にして引数を続けたもの。
    /// - 実行ファイルが `wsl.exe` で、引数が `-d`（`--distribution`）とその名前、`-e`（`--exec`）か `--`
    ///   に続くコマンド、オプションでない最初の語から始まるコマンドだけなら、`backend = "wsl"`。
    ///   `distribution` は `-d` の名前、`command` はコマンドの部分（なければ空でログインシェル）。
    /// - どちらでもなければ ConPTY で、コマンドラインをそのまま使う。
    pub fn from_shell_with(command: &str, is_cygwin_root: impl Fn(&Path) -> bool) -> Self {
        let name = program_name(command);
        let (program, rest) = split_program(command);
        if let Some((root, cygwin_command)) = cygwin_shell(program, rest, is_cygwin_root) {
            return Self {
                cygwin_root: Some(root),
                ..Self::new(&name, &cygwin_command, BackendKind::Cygwin)
            };
        }
        if file_stem(program).eq_ignore_ascii_case(WSL_PROGRAM)
            && let Some((distribution, wsl_command)) = wsl_arguments(rest)
        {
            return Self {
                distribution,
                ..Self::new(&name, &wsl_command, BackendKind::Wsl)
            };
        }
        Self::new(&name, command, BackendKind::default())
    }

    /// 設定ファイルを読むときと同じ決まりでチェックする。`earlier` はこのプロファイルより前に並ぶ
    /// プロファイル。名前とコマンドライン（ブリッジでは空でよい）が空でなく、名前が `earlier` のどれとも
    /// 違えば `Ok`。そうでなければ、名前、コマンドライン、名前の重なりの順に見て最初に合わないところ。
    pub fn validate(&self, earlier: &[Profile]) -> Result<(), ProfileError> {
        check_profile_name(&self.name)?;
        check_profile_command(&self.command, self.backend)?;
        if profile_position(earlier, &self.name).is_some() {
            return Err(ProfileError::DuplicateName);
        }
        Ok(())
    }

    /// `command` の実行ファイル（`"` で囲んでいれば中身）。ConPTY のプロファイルで Windows のプログラムを指す。
    pub fn command_program(&self) -> &str {
        split_program(&self.command).0
    }

    /// ブリッジのプロファイルの `command` が Windows のプログラムを起動するなら、そのファイルの名前
    /// （フォルダを除き、書いたままの形）。相手の側から起動した Windows のプログラムは ConPTY を通るので、
    /// ブリッジで届くもの（画像など）が届かない。
    ///
    /// WSL では `wsl` と `.exe` で終わるものすべて。Cygwin ではプログラムの実行ファイルも `.exe` で
    /// 終わるので、[`CYGWIN_KNOWN_WINDOWS_PROGRAMS`] の名前だけ。最初の語だけを見る。
    pub fn windows_program_in_bridge(&self) -> Option<&str> {
        let program = file_name(bridge_program(&self.command));
        let stem = file_stem(program);
        let is_windows_program = match self.backend {
            BackendKind::Conpty => false,
            BackendKind::Wsl => stem.eq_ignore_ascii_case(WSL_PROGRAM) || stem != program,
            BackendKind::Cygwin => CYGWIN_KNOWN_WINDOWS_PROGRAMS
                .iter()
                .any(|known| stem.eq_ignore_ascii_case(known)),
        };
        is_windows_program.then_some(program)
    }
}

/// Cygwin のブリッジの `command` で、Windows のプログラムと分かる実行ファイルの名前（フォルダと `.exe`
/// を除く）。
const CYGWIN_KNOWN_WINDOWS_PROGRAMS: [&str; 4] = [WSL_PROGRAM, "cmd", "powershell", "pwsh"];

/// Cygwin・WSL の側のコマンドライン `command` の実行ファイル（`'` か `"` で囲んでいれば中身）。
fn bridge_program(command: &str) -> &str {
    let command = command.trim();
    match command.strip_prefix('\'') {
        Some(quoted) => quoted
            .split_once('\'')
            .map_or(quoted, |(program, _)| program),
        None => split_program(command).0,
    }
}

/// Cygwin のフォルダの `bin` にある DLL。
const CYGWIN_DLL: &str = "cygwin1.dll";
/// Cygwin のフォルダの `bin` にあるシェル。`cygwin1.dll` を同梱しているだけのアプリのフォルダと見分ける。
const CYGWIN_SHELL: &str = "sh.exe";
/// Cygwin のフォルダの下の、実行ファイルを置くフォルダの名前。
const CYGWIN_BIN: &str = "bin";

/// 環境変数 `PATH` になければ、この順に探す Cygwin のフォルダ。
pub const COMMON_CYGWIN_ROOTS: [&str; 2] = [r"C:\cygwin64", r"C:\cygwin"];

/// フォルダ `root` が Cygwin のフォルダだとしたときの、`cygwin1.dll` のパス。
pub fn cygwin_dll_path(root: &Path) -> PathBuf {
    root.join("bin").join(CYGWIN_DLL)
}

/// フォルダ `root` が Cygwin のフォルダか（`bin` に `cygwin1.dll` があるか）。
pub fn is_cygwin_root(root: &Path) -> bool {
    cygwin_dll_path(root).is_file()
}

/// `cygwin_root` を書いていないときに使う Cygwin のフォルダ。[`find_cygwin_root_with`] で、この
/// プロセスの環境変数 `PATH` と実際のファイルを見て探す。
pub fn find_cygwin_root() -> Option<PathBuf> {
    find_cygwin_root_with(env::var_os("PATH").as_deref(), Path::is_file)
}

/// 環境変数 `PATH` の値が `path_variable` のときの Cygwin のフォルダ。`is_file` はそのパスのファイルが
/// あるか。
///
/// `PATH` の絶対パスの項目を順に見て、フォルダの名前が `bin` で `cygwin1.dll` と `sh.exe` があるものの
/// 一つ上を返す。なければ [`COMMON_CYGWIN_ROOTS`] の順に、`bin` に `cygwin1.dll` があるもの。
pub fn find_cygwin_root_with(
    path_variable: Option<&OsStr>,
    is_file: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    path_variable
        .into_iter()
        .flat_map(env::split_paths)
        .find_map(|folder| cygwin_root_of_bin(&folder, &is_file))
        .or_else(|| {
            COMMON_CYGWIN_ROOTS
                .iter()
                .map(PathBuf::from)
                .find(|root| is_file(&cygwin_dll_path(root)))
        })
}

/// フォルダ `folder` が Cygwin の `bin` なら、Cygwin のフォルダ。
fn cygwin_root_of_bin(folder: &Path, is_file: &impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let is_bin = folder
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(CYGWIN_BIN));
    if !folder.is_absolute()
        || !is_bin
        || !is_file(&folder.join(CYGWIN_DLL))
        || !is_file(&folder.join(CYGWIN_SHELL))
    {
        return None;
    }
    folder.parent().map(Path::to_path_buf)
}

/// 実行ファイル `program` が、パスの区切り（`\`、`/`）もドライブ名も含まない名前だけのものか。空は含めない。
pub fn is_bare_program_name(program: &str) -> bool {
    !program.is_empty() && !program.contains(['\\', '/', ':'])
}

/// 名前だけの実行ファイル `program` を、環境変数 `PATH` の値 `path_variable` から探す。`is_file` はそのパスの
/// ファイルがあるか。
///
/// `PATH` の絶対パスの項目だけを前から順に見る。空・`.`・相対パスの項目は、作業フォルダで結果が変わるので見ない。
/// 名前に `.` がなければ、`CreateProcessW` と同じく `.exe` を付けて探す（`.` があれば拡張子があるものとし、
/// そのまま探す）。名前だけでなければ `None`。
pub fn find_program_on_path(
    program: &str,
    path_variable: Option<&OsStr>,
    is_file: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    const EXE: &str = ".exe";
    if !is_bare_program_name(program) {
        return None;
    }
    let file_name = if program.contains('.') {
        program.to_owned()
    } else {
        format!("{program}{EXE}")
    };
    path_variable
        .into_iter()
        .flat_map(env::split_paths)
        .filter(|folder| folder.is_absolute())
        .map(|folder| folder.join(&file_name))
        .find(|candidate| is_file(candidate))
}

/// WSL を起動する実行ファイルの名前（フォルダと `.exe` を除く）。
const WSL_PROGRAM: &str = "wsl";

/// コマンドライン `command` の実行ファイルの名前（フォルダと `.exe` を除く）。読めなければ `shell`。
pub(super) fn program_name(command: &str) -> String {
    let stem = file_stem(split_program(command).0);
    if stem.is_empty() {
        "shell".to_owned()
    } else {
        stem.to_owned()
    }
}

/// コマンドライン `command` を、実行ファイル（`"` で囲んでいれば中身）と、そのあとの引数（前後の空白を
/// 除いたもの）に分ける。
pub fn split_program(command: &str) -> (&str, &str) {
    let command = command.trim();
    match command.strip_prefix('"') {
        Some(quoted) => match quoted.split_once('"') {
            Some((program, rest)) => (program, rest.trim()),
            None => (quoted, ""),
        },
        None => match command.split_once(char::is_whitespace) {
            Some((program, rest)) => (program, rest.trim()),
            None => (command, ""),
        },
    }
}

/// パス `program` のファイルの名前。
fn file_name(program: &str) -> &str {
    program.rsplit(['\\', '/']).next().unwrap_or(program)
}

/// パス `program` のファイルの名前から `.exe` を除いたもの。
fn file_stem(program: &str) -> &str {
    const EXE: &str = ".exe";
    let file = file_name(program);
    match file.len().checked_sub(EXE.len()) {
        Some(end) if file.is_char_boundary(end) && file[end..].eq_ignore_ascii_case(EXE) => {
            &file[..end]
        }
        _ => file,
    }
}

/// 実行ファイル `program` が Cygwin のフォルダの下のものなら、そのフォルダと、Cygwin の側のコマンド
/// ライン（実行ファイルを Cygwin のパスにして引数 `arguments` を続けたもの）。
fn cygwin_shell(
    program: &str,
    arguments: &str,
    is_cygwin_root: impl Fn(&Path) -> bool,
) -> Option<(String, String)> {
    let path = Path::new(program);
    if !path.is_absolute() {
        return None;
    }
    let folder = path.parent()?;
    let root = folder.parent()?;
    let folder_name = folder.file_name()?.to_str()?;
    let stem = file_stem(program);
    if stem.is_empty() || !is_cygwin_root(root) {
        return None;
    }
    let mut command = format!("/{folder_name}/{stem}");
    if !arguments.is_empty() {
        command.push(' ');
        command.push_str(arguments);
    }
    Some((root.to_str()?.to_owned(), command))
}

/// `wsl.exe` の引数 `arguments` を、ディストリビューションの名前と WSL の側のコマンドラインに読む。
/// 読めない引数（ほかのオプション）があれば `None`。
fn wsl_arguments(arguments: &str) -> Option<(Option<String>, String)> {
    let mut distribution = None;
    let mut words = command_words(arguments).into_iter();
    while let Some((start, end)) = words.next() {
        let word = &arguments[start..end];
        match word {
            "-d" | "--distribution" => {
                let (start, end) = words.next()?;
                distribution = Some(arguments[start..end].replace('"', ""));
            }
            "-e" | "--exec" => {
                let command = arguments[end..].trim();
                return (!command.is_empty()).then(|| (distribution, command.to_owned()));
            }
            "--" => return Some((distribution, arguments[end..].trim().to_owned())),
            _ if word.starts_with('-') => return None,
            _ => return Some((distribution, arguments[start..].trim().to_owned())),
        }
    }
    Some((distribution, String::new()))
}

/// コマンドライン `text` の語の、始まりと終わりの位置。語は空白で区切り、`"` で囲んだところの空白は
/// 区切りにしない。
fn command_words(text: &str) -> Vec<(usize, usize)> {
    let mut words = Vec::new();
    let mut start = None;
    let mut quoted = false;
    for (index, c) in text.char_indices() {
        if c.is_whitespace() && !quoted {
            if let Some(start) = start.take() {
                words.push((start, index));
            }
            continue;
        }
        if c == '"' {
            quoted = !quoted;
        }
        start.get_or_insert(index);
    }
    if let Some(start) = start {
        words.push((start, text.len()));
    }
    words
}

/// プロファイルのシェルとの接続方式（`profiles[].backend`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BackendKind {
    /// Windows の疑似コンソール（ConPTY）。
    #[default]
    Conpty,
    /// Cygwin のブリッジ。
    Cygwin,
    /// WSL のブリッジ。
    Wsl,
}

impl BackendKind {
    /// 値と、設定ファイルに書く名前。
    const NAMES: [(BackendKind, &'static str); 3] = [
        (BackendKind::Conpty, "conpty"),
        (BackendKind::Cygwin, "cygwin"),
        (BackendKind::Wsl, "wsl"),
    ];

    /// 書ける値のすべて。
    pub const ALL: [BackendKind; 3] = named_values(&Self::NAMES);

    /// 設定ファイルに書く名前。
    pub fn name(self) -> &'static str {
        name_of(&Self::NAMES, self)
    }

    /// ブリッジで接続する。`command` を相手の側のコマンドラインとして渡し、空なら相手のログインシェル。
    pub fn is_bridge(self) -> bool {
        self != BackendKind::Conpty
    }

    /// 設定ファイルに書く名前 `name` の値。
    pub fn from_name(name: &str) -> Option<Self> {
        value_named(&Self::NAMES, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_of_cygwin_uses_the_cygwin_bridge() {
        let cygwin = |root: &Path| root == Path::new(r"C:\app\cygwin");
        let profile = |command: &str| Profile::from_shell_with(command, cygwin);
        let bridge = |name: &str, command: &str| Profile {
            cygwin_root: Some(r"C:\app\cygwin".to_owned()),
            ..Profile::new(name, command, BackendKind::Cygwin)
        };
        assert_eq!(
            profile(r"C:\app\cygwin\bin\zsh.exe -l"),
            bridge("zsh", "/bin/zsh -l")
        );
        assert_eq!(
            profile(r#""C:\app\cygwin\bin\bash.EXE"  --login -i "#),
            bridge("bash", "/bin/bash --login -i")
        );
        // Cygwin のフォルダは書いたままの区切りで。
        assert_eq!(
            profile(r"C:/app/cygwin/bin/zsh"),
            Profile {
                cygwin_root: Some(r"C:/app/cygwin".to_owned()),
                ..bridge("zsh", "/bin/zsh")
            }
        );
        // bin の隣のフォルダでも、一つ上が Cygwin のフォルダならよい。
        assert_eq!(
            profile(r"C:\app\cygwin\sbin\tool.exe x"),
            bridge("tool", "/sbin/tool x")
        );
        // Cygwin のフォルダの下でないもの、フォルダのないもの、相対パスは ConPTY のまま。
        for command in [
            r"C:\cygwin64\bin\zsh.exe -l",
            r"C:\app\cygwin\bin\x\zsh.exe",
            "zsh -l",
            r"cygwin\bin\zsh.exe",
            r"C:\zsh.exe",
        ] {
            assert_eq!(
                profile(command),
                Profile::new(&program_name(command), command, BackendKind::Conpty),
                "{command}"
            );
        }
    }

    #[test]
    fn the_program_of_a_command_is_its_first_word() {
        let program = |command: &str| {
            Profile::new("a", command, BackendKind::Conpty)
                .command_program()
                .to_owned()
        };
        assert_eq!(program("powershell.exe -NoLogo"), "powershell.exe");
        assert_eq!(program("  cmd  "), "cmd");
        assert_eq!(
            program(r#""C:\Program Files\a\b.exe" -x"#),
            r"C:\Program Files\a\b.exe"
        );
    }

    #[test]
    fn windows_programs_are_found_in_bridge_commands() {
        let found = |backend: BackendKind, command: &str| {
            Profile::new("p", command, backend)
                .windows_program_in_bridge()
                .map(str::to_owned)
        };
        let wsl = |command: &str| found(BackendKind::Wsl, command);
        let cygwin = |command: &str| found(BackendKind::Cygwin, command);

        // wsl はどちらのブリッジでも。パスと大文字小文字を問わず、書いたままの名前を返す。
        for (command, program) in [
            ("wsl", "wsl"),
            ("wsl.exe", "wsl.exe"),
            ("WSL.EXE -d Ubuntu", "WSL.EXE"),
            (r"C:\Windows\System32\wsl.exe", "wsl.exe"),
            ("/mnt/c/Windows/System32/wsl.exe -d Ubuntu", "wsl.exe"),
            (r#"  "C:\Windows\System32\Wsl.exe" -d Ubuntu "#, "Wsl.exe"),
            (
                "'/cygdrive/c/Windows/System32/wsl.exe' -d Ubuntu",
                "wsl.exe",
            ),
        ] {
            assert_eq!(wsl(command).as_deref(), Some(program), "{command}");
            assert_eq!(cygwin(command).as_deref(), Some(program), "{command}");
        }

        // WSL では .exe で終わるものはすべて Windows のプログラム。
        for command in [
            "docker.exe run -it --rm otiai10/amesh",
            "cmd.exe",
            "'/mnt/c/Program Files/PowerShell/7/pwsh.EXE' -NoLogo",
        ] {
            assert!(wsl(command).is_some(), "{command}");
        }
        for command in [
            "",
            "   ",
            "zsh -l",
            "/usr/bin/docker run",
            "cmd",
            "a.exe.bak",
            "echo wsl.exe",
        ] {
            assert_eq!(wsl(command), None, "{command}");
        }

        // Cygwin では分かっている名前だけ。Cygwin のプログラムも .exe で終わる。
        for command in [
            "cmd.exe /c dir",
            "cmd",
            "powershell.exe -NoLogo",
            "PWSH.EXE",
        ] {
            assert!(cygwin(command).is_some(), "{command}");
        }
        for command in [
            "",
            "zsh.exe -l",
            "/bin/bash.exe",
            "docker.exe run",
            "zsh -l",
            "echo cmd.exe",
        ] {
            assert_eq!(cygwin(command), None, "{command}");
        }

        // ConPTY では wsl.exe が正しい。
        for command in ["wsl.exe", "cmd.exe", "powershell.exe -NoLogo"] {
            assert_eq!(found(BackendKind::Conpty, command), None, "{command}");
        }
    }

    /// `path_variable` を `PATH` の値、`files` をあるファイルの全部として Cygwin のフォルダを探す。
    fn find_cygwin(path_variable: Option<&str>, files: &[&str]) -> Option<PathBuf> {
        find_cygwin_root_with(path_variable.map(OsStr::new), |path| {
            files.iter().any(|file| Path::new(file) == path)
        })
    }

    #[test]
    fn cygwin_root_is_found_from_path_in_order() {
        let files = [
            r"C:\first\bin\cygwin1.dll",
            r"C:\first\bin\sh.exe",
            r"C:\second\bin\cygwin1.dll",
            r"C:\second\bin\sh.exe",
            r"C:\cygwin64\bin\cygwin1.dll",
        ];
        assert_eq!(
            find_cygwin(Some(r"C:\Windows;C:\second\bin;C:\first\bin"), &files),
            Some(PathBuf::from(r"C:\second"))
        );
        assert_eq!(
            find_cygwin(Some(r"C:\first\bin;C:\second\bin"), &files),
            Some(PathBuf::from(r"C:\first"))
        );
        // 区切りと大文字小文字は問わない。末尾の区切りがあっても一つ上を返す。
        let files = [r"C:\first\BIN\cygwin1.dll", r"C:\first\BIN\sh.exe"];
        assert_eq!(
            find_cygwin(Some(r"C:\first\BIN\"), &files),
            Some(PathBuf::from(r"C:\first"))
        );
        let files = [r"C:/first/bin/cygwin1.dll", r"C:/first/bin/sh.exe"];
        assert_eq!(
            find_cygwin(Some("C:/first/bin"), &files),
            Some(PathBuf::from(r"C:\first"))
        );
    }

    #[test]
    fn cygwin_root_skips_folders_that_are_not_cygwin_bin() {
        let files = [
            // sh.exe がない。
            r"C:\no_shell\bin\cygwin1.dll",
            // フォルダの名前が bin でない。
            r"C:\app\cygwin1.dll",
            r"C:\app\sh.exe",
            // cygwin1.dll がない。
            r"C:\msys\bin\sh.exe",
            // 相対パス。
            r"rel\bin\cygwin1.dll",
            r"rel\bin\sh.exe",
            // ここまでを飛ばして見つかるもの。
            r"C:\real\bin\cygwin1.dll",
            r"C:\real\bin\sh.exe",
        ];
        assert_eq!(
            find_cygwin(
                Some(r"C:\no_shell\bin;C:\app;C:\msys\bin;rel\bin;C:\real\bin"),
                &files
            ),
            Some(PathBuf::from(r"C:\real"))
        );
    }

    #[test]
    fn cygwin_root_reads_quoted_and_empty_path_entries() {
        let files = [
            r"C:\Program Files\cyg;win\bin\cygwin1.dll",
            r"C:\Program Files\cyg;win\bin\sh.exe",
        ];
        assert_eq!(
            find_cygwin(Some(r#";;"C:\Program Files\cyg;win\bin";"#), &files),
            Some(PathBuf::from(r"C:\Program Files\cyg;win"))
        );
        // 空の項目は Cygwin のフォルダにしない。
        assert_eq!(find_cygwin(Some(";;"), &["cygwin1.dll", "sh.exe"]), None);
    }

    /// `path_variable` を `PATH` の値、`files` をあるファイルの全部として `program` を探す。
    fn find_program(program: &str, path_variable: &str, files: &[&str]) -> Option<PathBuf> {
        find_program_on_path(program, Some(OsStr::new(path_variable)), |path| {
            files.iter().any(|file| Path::new(file) == path)
        })
    }

    #[test]
    fn bare_program_names_have_no_folder_and_no_drive() {
        for program in ["cmd", "cmd.exe", "my tool.exe", ".hidden"] {
            assert!(is_bare_program_name(program), "{program}");
        }
        for program in [
            "",
            r"C:\x\cmd.exe",
            r".\cmd.exe",
            "bin/cmd",
            "C:cmd.exe",
            r"\cmd",
        ] {
            assert!(!is_bare_program_name(program), "{program}");
        }
    }

    #[test]
    fn programs_are_found_on_path_in_order() {
        let files = [r"C:\a\tool.exe", r"C:\b\tool.exe", r"C:\b\other.exe"];
        assert_eq!(
            find_program("tool.exe", r"C:\a;C:\b", &files),
            Some(PathBuf::from(r"C:\a\tool.exe"))
        );
        assert_eq!(
            find_program("tool.exe", r"C:\b;C:\a", &files),
            Some(PathBuf::from(r"C:\b\tool.exe"))
        );
        assert_eq!(
            find_program("other.exe", r"C:\a;C:\b", &files),
            Some(PathBuf::from(r"C:\b\other.exe"))
        );
        assert_eq!(find_program("missing.exe", r"C:\a;C:\b", &files), None);
        assert_eq!(find_program("tool.exe", "", &files), None);
        assert_eq!(
            find_program_on_path("tool.exe", None, |_| true),
            None,
            "no PATH"
        );
    }

    #[test]
    fn programs_on_path_skip_relative_and_empty_entries() {
        let files = [
            r"tool.exe",
            r".\tool.exe",
            r"rel\tool.exe",
            r"\root\tool.exe",
            r"C:tool.exe",
            r"C:\real\tool.exe",
        ];
        assert_eq!(
            find_program("tool.exe", r";.;rel;\root;C:;C:\real", &files),
            Some(PathBuf::from(r"C:\real\tool.exe"))
        );
        assert_eq!(find_program("tool.exe", r";.;rel;\root;C:", &files), None);
    }

    #[test]
    fn programs_on_path_get_exe_only_without_an_extension() {
        let files = [
            r"C:\a\tool.exe",
            r"C:\a\tool",
            r"C:\a\script.bat",
            r"C:\a\.hidden",
            r"C:\a\.hidden.exe",
        ];
        assert_eq!(
            find_program("tool", r"C:\a", &files),
            Some(PathBuf::from(r"C:\a\tool.exe"))
        );
        // `.` で始まる名前も、`.` があるので拡張子があるものとする。
        assert_eq!(
            find_program(".hidden", r"C:\a", &files),
            Some(PathBuf::from(r"C:\a\.hidden"))
        );
        assert_eq!(
            find_program("script.bat", r"C:\a", &files),
            Some(PathBuf::from(r"C:\a\script.bat"))
        );
        assert_eq!(find_program("script", r"C:\a", &files), None);
    }

    #[test]
    fn programs_with_a_folder_are_not_searched_on_path() {
        let files = [r"C:\a\tool.exe", r"C:\a\sub\tool.exe"];
        for program in [r"sub\tool.exe", r"C:\a\tool.exe", "C:tool.exe", ""] {
            assert_eq!(find_program(program, r"C:\a", &files), None, "{program}");
        }
    }

    #[test]
    fn cygwin_root_falls_back_to_common_roots() {
        let both = [r"C:\cygwin\bin\cygwin1.dll", r"C:\cygwin64\bin\cygwin1.dll"];
        assert_eq!(
            find_cygwin(None, &both),
            Some(PathBuf::from(r"C:\cygwin64"))
        );
        assert_eq!(
            find_cygwin(Some(r"C:\Windows"), &both[..1]),
            Some(PathBuf::from(r"C:\cygwin"))
        );
        // PATH の項目と違い、標準のインストール先は sh.exe がなくてもよい。PATH で見つかればそちらが先。
        let files = [
            r"C:\cygwin64\bin\cygwin1.dll",
            r"C:\real\bin\cygwin1.dll",
            r"C:\real\bin\sh.exe",
        ];
        assert_eq!(
            find_cygwin(Some(r"C:\real\bin"), &files),
            Some(PathBuf::from(r"C:\real"))
        );
        assert_eq!(find_cygwin(None, &[]), None);
        assert_eq!(find_cygwin(Some(r"C:\Windows"), &[]), None);
    }
}
