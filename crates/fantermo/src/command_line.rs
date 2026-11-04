//! fantermo のコマンドライン。
//!
//! オプションはプログラム名の直後でだけ読む。最初の認識できない語（`--foo` を含む）から後ろは、引用符も
//! そのままで子プロセスのコマンドラインにする。`--` はオプションの終わりで、その後ろがコマンドライン。

use fantermo_core::config::{Profile, profile_position};
use windows::Win32::System::Environment::GetCommandLineW;

/// プロファイルを指定するオプション。
pub const PROFILE_OPTION: &str = "--profile";
/// すでに動いているウィンドウがあっても新しいウィンドウを開くオプション。
pub const NEW_WINDOW_OPTION: &str = "--new-window";
/// 管理者として開くオプション。
pub const ELEVATE_OPTION: &str = "--elevate";
/// オプションの終わりを示す語。
pub const END_OF_OPTIONS: &str = "--";
/// オプションと同じ形の語の先頭。`--profile` の値としては受け取らない。
const OPTION_PREFIX: &str = "--";

/// タブで起動するものの指定。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabRequest {
    /// プロファイルの名前。
    pub profile: Option<String>,
    /// 子プロセスのコマンドライン。プロファイルがあれば、その `command` の代わりに同じ接続先で起動する。
    pub command: Option<String>,
}

/// コマンドラインを読んだ結果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub tab: TabRequest,
    /// 名前のない `--profile` があった。
    pub profile_without_name: bool,
    /// すでに動いているウィンドウがあっても新しいウィンドウを開く。
    pub new_window: bool,
    /// 管理者として開く。
    pub elevate: bool,
}

impl Options {
    /// このプロセスのコマンドラインを読む。
    pub fn of_process() -> Self {
        // SAFETY: プロセスが持つコマンドラインを指すポインタが返り、プロセスが終わるまで有効。
        match unsafe { GetCommandLineW().to_string() } {
            Ok(raw) => Self::parse(skip_program_name(&raw)),
            Err(e) => {
                tracing::warn!("failed to read the command line: {e}");
                Self::default()
            }
        }
    }

    /// プログラム名を除いたコマンドライン `arguments` を読む。
    pub fn parse(arguments: &str) -> Self {
        let mut options = Self::default();
        let mut rest = arguments.trim_start();
        while !rest.is_empty() {
            let (word, after_word) = next_word(rest);
            match word.as_str() {
                END_OF_OPTIONS => {
                    rest = after_word;
                    break;
                }
                NEW_WINDOW_OPTION => {
                    options.new_window = true;
                    rest = after_word;
                }
                ELEVATE_OPTION => {
                    options.elevate = true;
                    rest = after_word;
                }
                PROFILE_OPTION => {
                    let (value, after_value) = next_word(after_word);
                    if value.starts_with(OPTION_PREFIX) {
                        options.profile_without_name = true;
                        rest = after_word;
                        continue;
                    }
                    if value.is_empty() {
                        options.profile_without_name = true;
                    } else {
                        options.tab.profile = Some(value);
                    }
                    rest = after_value;
                }
                _ => break,
            }
        }
        let command = rest.trim_start();
        options.tab.command = (!command.is_empty()).then(|| command.to_owned());
        options
    }

    /// 利用者に知らせること。`profiles` は設定のプロファイル。なければ `None`。
    pub fn notice(&self, profiles: &[Profile]) -> Option<String> {
        let mut lines = Vec::new();
        if self.profile_without_name {
            lines.push(format!(
                "コマンドラインの {PROFILE_OPTION} にプロファイルの名前がありません。"
            ));
        }
        if let Some(name) = &self.tab.profile
            && profile_position(profiles, name).is_none()
        {
            let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
            lines.push(format!(
                "コマンドラインで指定したプロファイル「{name}」は設定にありません。\n設定にあるプロファイル: {}",
                names.join("、")
            ));
        }
        if lines.is_empty() {
            return None;
        }
        lines.push("プロファイルを指定しなかったものとして開きます。".to_owned());
        Some(lines.join("\n\n"))
    }
}

/// コマンドラインの先頭のプログラム名を飛ばす。プログラム名は引用符で囲まれていれば次の引用符まで、
/// そうでなければ最初の空白までで、中の `\` は特別扱いしない（`CommandLineToArgvW` と同じ）。
fn skip_program_name(command_line: &str) -> &str {
    if let Some(quoted) = command_line.strip_prefix('"') {
        return quoted.find('"').map_or("", |end| &quoted[end + 1..]);
    }
    command_line
        .find([' ', '\t'])
        .map_or("", |end| &command_line[end..])
}

/// `text` の先頭の語と、その後ろの残り。語の切り方は `CommandLineToArgvW` のプログラム名より後ろの決まりに
/// 従う（`"` で囲むと空白も語に入る。`\"` は `"` そのもの）。
fn next_word(text: &str) -> (String, &str) {
    let text = text.trim_start_matches([' ', '\t']);
    let mut word = String::new();
    let mut quoted = false;
    let mut backslashes = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        match c {
            '\\' => {
                backslashes += 1;
                continue;
            }
            '"' => {
                word.extend(std::iter::repeat_n('\\', backslashes / 2));
                if backslashes % 2 == 1 {
                    word.push('"');
                } else if quoted && chars.peek().is_some_and(|&(_, next)| next == '"') {
                    chars.next();
                    word.push('"');
                } else {
                    quoted = !quoted;
                }
            }
            ' ' | '\t' if !quoted => {
                word.extend(std::iter::repeat_n('\\', backslashes));
                return (word, &text[index..]);
            }
            _ => {
                word.extend(std::iter::repeat_n('\\', backslashes));
                word.push(c);
            }
        }
        backslashes = 0;
    }
    word.extend(std::iter::repeat_n('\\', backslashes));
    (word, "")
}

#[cfg(test)]
mod tests {
    use fantermo_core::config::BackendKind;

    use super::*;

    fn parsed(arguments: &str) -> (Option<String>, Option<String>, bool) {
        let options = Options::parse(arguments);
        (
            options.tab.profile,
            options.tab.command,
            options.profile_without_name,
        )
    }

    fn some(text: &str) -> Option<String> {
        Some(text.to_owned())
    }

    #[test]
    fn program_name_is_skipped() {
        assert_eq!(skip_program_name("fantermo.exe"), "");
        assert_eq!(skip_program_name("fantermo.exe zsh -l"), " zsh -l");
        assert_eq!(
            skip_program_name(r#""C:\Program Files\fantermo.exe" "a b" c"#),
            r#" "a b" c"#
        );
    }

    #[test]
    fn nothing_means_no_profile_and_no_command() {
        assert_eq!(parsed(""), (None, None, false));
        assert_eq!(parsed("   "), (None, None, false));
    }

    #[test]
    fn commands_are_kept_as_written() {
        assert_eq!(
            parsed(r#" "C:\a b\zsh.exe" -c "echo  'x'"  "#),
            (None, some(r#""C:\a b\zsh.exe" -c "echo  'x'"  "#), false)
        );
    }

    #[test]
    fn the_profile_option_is_read_only_at_the_start() {
        assert_eq!(parsed("--profile one"), (some("one"), None, false));
        assert_eq!(
            parsed(r#"--profile "my profile" zsh -l"#),
            (some("my profile"), some("zsh -l"), false)
        );
        assert_eq!(
            parsed("--profile one vim --profile two"),
            (some("one"), some("vim --profile two"), false)
        );
        assert_eq!(
            parsed("vim --profile x"),
            (None, some("vim --profile x"), false)
        );
    }

    #[test]
    fn the_last_profile_option_wins() {
        assert_eq!(
            parsed("--profile one --profile two"),
            (some("two"), None, false)
        );
    }

    #[test]
    fn two_dashes_end_the_options() {
        assert_eq!(parsed("-- --profile x"), (None, some("--profile x"), false));
        assert_eq!(
            parsed("--profile one -- --foo"),
            (some("one"), some("--foo"), false)
        );
        assert_eq!(parsed("--"), (None, None, false));
    }

    #[test]
    fn unknown_options_start_the_command() {
        assert_eq!(parsed("--foo bar"), (None, some("--foo bar"), false));
        assert_eq!(
            parsed("--profile one --foo"),
            (some("one"), some("--foo"), false)
        );
        // `=` で続ける形は受け付けていない。
        assert_eq!(
            parsed("--profile=one"),
            (None, some("--profile=one"), false)
        );
    }

    #[test]
    fn a_profile_option_without_a_name_is_reported() {
        assert_eq!(parsed("--profile"), (None, None, true));
        assert_eq!(parsed("--profile   "), (None, None, true));
        assert_eq!(parsed(r#"--profile """#), (None, None, true));
        assert_eq!(parsed("--profile -- cmd"), (None, some("cmd"), true));
        assert_eq!(parsed("--profile --foo"), (None, some("--foo"), true));
    }

    #[test]
    fn the_new_window_option_is_read_at_the_start() {
        let options = Options::parse("--new-window --profile one zsh --new-window");
        assert!(options.new_window);
        assert_eq!(options.tab.profile.as_deref(), Some("one"));
        assert_eq!(options.tab.command.as_deref(), Some("zsh --new-window"));

        assert!(Options::parse("--profile one --new-window").new_window);
        assert!(!Options::parse("-- --new-window").new_window);
        assert!(!Options::parse("vim --new-window").new_window);
        // `--profile` の名前の代わりに書かれていれば、名前がないものとして続ける。
        let options = Options::parse("--profile --new-window");
        assert!(options.new_window);
        assert!(options.profile_without_name);
    }

    #[test]
    fn the_elevate_option_is_read_at_the_start() {
        let options = Options::parse("--elevate --new-window --profile one zsh --elevate");
        assert!(options.elevate);
        assert!(options.new_window);
        assert_eq!(options.tab.profile.as_deref(), Some("one"));
        assert_eq!(options.tab.command.as_deref(), Some("zsh --elevate"));

        assert!(Options::parse("--profile one --elevate").elevate);
        assert!(!Options::parse("").elevate);
        let after_dashes = Options::parse("-- --elevate");
        assert!(!after_dashes.elevate);
        assert_eq!(after_dashes.tab.command.as_deref(), Some("--elevate"));
        let in_command = Options::parse("vim --elevate");
        assert!(!in_command.elevate);
        assert_eq!(in_command.tab.command.as_deref(), Some("vim --elevate"));
        // `--profile` の名前の代わりに書かれていれば、名前がないものとして続ける。
        let options = Options::parse("--profile --elevate");
        assert!(options.elevate);
        assert!(options.profile_without_name);
        assert_eq!(options.tab.profile, None);
    }

    #[test]
    fn words_follow_the_quoting_of_the_command_line() {
        assert_eq!(next_word("a b"), ("a".to_owned(), " b"));
        assert_eq!(next_word(r#"  "a b"c d"#), ("a bc".to_owned(), " d"));
        assert_eq!(next_word(r#"a\"b c"#), (r#"a"b"#.to_owned(), " c"));
        assert_eq!(next_word(r#""a\\" b"#), (r"a\".to_owned(), " b"));
        assert_eq!(next_word(r"a\b\ c"), (r"a\b\".to_owned(), " c"));
        assert_eq!(next_word(r#""a""b" c"#), (r#"a"b"#.to_owned(), " c"));
        assert_eq!(next_word("日本語 x"), ("日本語".to_owned(), " x"));
        assert_eq!(next_word(r"a\"), (r"a\".to_owned(), ""));
    }

    #[test]
    fn notices_name_what_is_wrong() {
        let profiles = [
            Profile::new("one", "cmd.exe", BackendKind::Conpty),
            Profile::new("Two", "cmd.exe", BackendKind::Conpty),
        ];
        assert_eq!(Options::parse("").notice(&profiles), None);
        assert_eq!(Options::parse("--profile one").notice(&profiles), None);

        let unknown = Options::parse("--profile two").notice(&profiles);
        let unknown = unknown.expect("the names are compared exactly");
        assert!(unknown.contains("「two」"), "{unknown}");
        assert!(unknown.contains("one、Two"), "{unknown}");

        let without_name = Options::parse("--profile").notice(&profiles);
        assert!(without_name.is_some_and(|text| text.contains("--profile")));
    }
}
