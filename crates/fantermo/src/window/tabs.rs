//! タブ。端末を開く・閉じる・切り替える、別のプロセスから渡された起動の指定、出力と子プロセスの終了の
//! 知らせ、タイトルとベル。

use std::path::Path;

use fantermo_core::{
    backend::ExitCode,
    config::{self, Profile},
};
use windows::{
    Win32::{
        Foundation::HWND,
        UI::{
            Input::KeyboardAndMouse::IsWindowEnabled,
            WindowsAndMessaging::{
                FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx, GetForegroundWindow,
                GetQueueStatus, KillTimer, QS_INPUT, QS_PAINT, SetTimer, SetWindowTextW,
                USER_TIMER_MINIMUM,
            },
        },
    },
    core::HSTRING,
};

use super::{
    App, OUTPUT_TIMER_ID, WINDOW_TITLE,
    layout::grid_size,
    proc::{Deferred, in_menu},
    wake_poster,
};
use crate::{
    clipboard_prompt,
    command_line::TabRequest,
    instance::Forwarded,
    notice::Topic,
    session::{Session, SessionId, Startup},
    tab_icon::TabIcon,
    version,
};

/// 新しい端末で起動するもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Launch {
    /// fantermo の引数の指定があればそれ、なければ既定のプロファイル。「+」・キー・メニューの「新しいタブ」。
    Default,
    /// 一覧から選んだ、プロファイルの並びの中の位置がこの値のプロファイル。
    Profile(usize),
}

/// 端末を開くときに起動するもの。`launch` が [`Launch::Default`] なら `request` に従う。
/// - 名前の合うプロファイルがあればそのプロファイル。コマンドもあれば、そのプロファイルの `command` の代わりに
///   そのコマンドラインを同じ接続先で起動する。
/// - なければ、コマンドがあればそのコマンドラインを既定のバックエンドで、それもなければ既定のプロファイル
///   （`profiles` の `default` 番目）のもの。
///
/// 一覧から選んだプロファイルなら `request` があってもそのプロファイルのもの。並びの外なら先頭のもの。
/// プロファイルの相対パスのアイコンは `config_dir` から辿る。
pub(super) fn startup_for(
    request: &TabRequest,
    profiles: &[Profile],
    default: usize,
    launch: Launch,
    config_dir: Option<&Path>,
) -> Startup {
    let mut command = None;
    let index = match launch {
        Launch::Profile(index) => index,
        Launch::Default => {
            let named = request
                .profile
                .as_deref()
                .and_then(|name| config::profile_position(profiles, name));
            match (named, request.command.as_deref()) {
                (Some(index), requested) => {
                    command = requested;
                    index
                }
                (None, Some(requested)) => return Startup::conpty(requested),
                (None, None) => default,
            }
        }
    };
    let index = if index < profiles.len() { index } else { 0 };
    let Some(profile) = profiles.get(index) else {
        return Startup::conpty(config::DEFAULT_COMMAND_LINE);
    };
    let mut startup =
        Startup::from_profile(profile, TabIcon::for_profile(profiles, index, config_dir));
    if let Some(command) = command {
        command.clone_into(&mut startup.command_line);
    }
    startup
}

/// タブの操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TabCommand {
    /// 新しい端末を開いて表示する。
    New,
    /// 表示している端末を閉じる。
    Close,
    /// 右隣の端末を表示する。右端なら左端へ。
    Next,
    /// 左隣の端末を表示する。左端なら右端へ。
    Previous,
}

impl App {
    /// タブの操作をする。最後の端末を閉じたときは、ウィンドウを閉じるよう返す。
    pub(super) fn run_tab_command(&mut self, command: TabCommand) -> Option<Deferred> {
        let count = self.sessions.len();
        match command {
            TabCommand::New => self.open_session(Launch::Default),
            TabCommand::Close => {
                if self.close_session(self.active) {
                    return Some(Deferred::CloseWindow);
                }
            }
            TabCommand::Next => self.activate((self.active + 1) % count),
            TabCommand::Previous => self.activate((self.active + count - 1) % count),
        }
        None
    }

    /// `launch` の新しい端末を開き、タブの並びの最後に足して表示する。起動できなければ警告を残して
    /// 何もしない。
    pub(super) fn open_session(&mut self, launch: Launch) {
        let startup = self.startup_of(&self.arguments, launch);
        // 失敗は open_startup が残す。
        _ = self.open_startup(&startup);
    }

    fn startup_of(&self, request: &TabRequest, launch: Launch) -> Startup {
        startup_for(
            request,
            &self.config.profiles,
            self.default_profile_index(),
            launch,
            self.config_path.as_deref().and_then(Path::parent),
        )
    }

    /// `startup` の新しい端末を開き、タブの並びの最後に足して表示する。開けなければ警告を残して、
    /// そのエラーを返す。
    fn open_startup(&mut self, startup: &Startup) -> Result<(), Box<dyn std::error::Error>> {
        let id = SessionId(self.next_session_id);
        // 起動に失敗しても番号を進める。失敗する前に動き出したバックエンドのスレッドが、この番号の知らせを
        // 積んでいることがあるため。
        self.next_session_id += 1;
        let size = grid_size(&self.font, self.terminal_area);
        let spawned = Session::spawn(
            id,
            startup,
            size,
            &self.config.colors.terminal_colors(),
            wake_poster(self.hwnd, id),
        );
        let mut session = match spawned {
            Ok(session) => session,
            Err(e) => {
                tracing::warn!(
                    "failed to open a terminal with {:?} ({}): {e}",
                    startup.command_line,
                    startup.backend.name()
                );
                return Err(e);
            }
        };
        session.background_image = self.choose_background_image();
        self.sessions.push(session);
        self.update_tab_icons();
        self.activate(self.sessions.len() - 1);
        Ok(())
    }

    /// 別のプロセスから渡された `forwarded` を受け取り、あとで開くよう積む。実行ファイルか設定ファイルが違えば
    /// 受け取らず、`false` を返す。
    pub(super) fn accept_launch(&mut self, forwarded: Forwarded) -> bool {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                tracing::warn!("failed to get the path of the executable: {e}");
                return false;
            }
        };
        if !forwarded.is_for(&exe, self.config_path.as_deref()) {
            tracing::info!(
                "refused a launch of {} with the settings {:?}",
                forwarded.exe.display(),
                forwarded.config
            );
            return false;
        }
        self.pending_launches.push_back(forwarded);
        true
    }

    /// 待っている起動の指定を開く。ダイアログやメニューの間は開かない（閉じたときの処理が表示しているタブに
    /// 働くので、その間に替えない）。閉じたあとに [`WM_APP_LAUNCH`](super::WM_APP_LAUNCH) で改めて呼ばれる。
    /// 知らせることがあれば返す。
    pub(super) fn open_pending_launches(&mut self) -> Option<Deferred> {
        // SAFETY: ウィンドウのハンドルを調べるだけ。
        if !unsafe { IsWindowEnabled(self.hwnd) }.as_bool() || in_menu() {
            return None;
        }
        let mut notices = Vec::new();
        while let Some(forwarded) = self.pending_launches.pop_front() {
            notices.extend(self.open_forwarded(&forwarded));
        }
        (!notices.is_empty()).then(|| Deferred::Warn {
            topic: Topic::CommandLine,
            text: notices.join("\n\n"),
        })
    }

    /// `forwarded` のタブを開く。知らせることがあれば返す。
    fn open_forwarded(&mut self, forwarded: &Forwarded) -> Option<String> {
        let mut startup = self.startup_of(&forwarded.options.tab, Launch::Default);
        startup.working_folder = forwarded.usable_working_folder().map(Path::to_owned);
        let mut notice = forwarded.options.notice(&self.config.profiles);
        if let Err(e) = self.open_startup(&startup) {
            let failed = format!(
                "「{}」を起動できませんでした。\n\n{e}",
                startup.command_line
            );
            notice = Some(match notice {
                Some(text) => format!("{text}\n\n{failed}"),
                None => failed,
            });
        }
        notice
    }

    /// `sessions` の `index` の端末を閉じる（子プロセスはバックエンドを破棄して終わらせる）。最後の
    /// 端末なら閉じずに `true` を返す（ウィンドウを閉じると一緒に閉じる）。
    pub(super) fn close_session(&mut self, index: usize) -> bool {
        if self.sessions.len() == 1 {
            return true;
        }
        let was_active = index == self.active;
        if was_active {
            self.leave_active();
        }
        let closed = self.sessions.remove(index);
        if self.clipboard_prompts.discard_tab(closed.id()) {
            clipboard_prompt::close();
        }
        self.output_continuations.retain(|id| *id != closed.id());
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.drop_terminal_images(closed.id().0);
        }
        drop(closed);
        self.drop_unused_background_images();
        // 閉じたタブだけが使っていたアイコンを捨てる。テクスチャは次に描くときに捨てる。
        self.update_tab_icons();
        self.active = active_after_close(self.active, index, self.sessions.len());
        if was_active {
            self.on_activated();
        }
        // タブバーのタブが減り、マウスを載せているものも変わりうる。
        self.update_tab_bar_hover();
        self.invalidate();
        false
    }

    /// `sessions` の `index` の端末を表示する。
    pub(super) fn activate(&mut self, index: usize) {
        if index == self.active {
            return;
        }
        self.leave_active();
        self.active = index;
        self.on_activated();
    }

    /// 表示している端末を表示から外す前に、マウスの操作の途中の状態を終える。
    fn leave_active(&mut self) {
        if self.session().selecting || self.scroll_bar_press.is_some() {
            self.end_scroll_bar_press();
            self.release_capture_pending = true;
        }
        self.session_mut().release_pointer();
    }

    /// 表示する端末が替わった。ウィンドウのタイトルとマウスを載せているリンクとタブバーのものを合わせ、
    /// 描き直す。
    fn on_activated(&mut self) {
        tracing::debug!(
            "showing terminal {:?} ({} of {})",
            self.session().id(),
            self.active + 1,
            self.sessions.len()
        );
        self.session_mut().bell_marked = false;
        self.update_window_title();
        self.refresh_hovered_link_now();
        // タブが入りきらないときは、表示するタブでタブの並びがずれる。
        self.update_tab_bar_hover();
        self.invalidate();
    }

    /// 既定のプロファイルの、`profiles` の中の位置。
    pub(super) fn default_profile_index(&self) -> usize {
        config::default_profile_index(
            &self.config.profiles,
            self.config.default_profile.as_deref(),
        )
    }

    /// 端末 `id` に出力が届いたか、子プロセスが終わった。出力を流し込み、タイトルとベルを反映する。
    /// 表示していない端末の出力も流し込む（描くのは表示している端末だけ）。一度に流す量には上限があり、
    /// 残れば [`continue_output`](Self::continue_output) で、続きを次のメッセージで流す。タイマーで続きを
    /// 待っている端末への知らせは流さずに戻る（続きはタイマーが流す）。子プロセスの終了は、出力を流しきって
    /// から扱う。ウィンドウを閉じるときは `true` を返す。
    pub(super) fn on_wake(&mut self, id: SessionId) -> bool {
        let Some(index) = self.sessions.iter().position(|session| session.id() == id) else {
            // 閉じた端末の知らせ。番号は使い回さないので、ほかの端末のものと取り違えない。
            tracing::debug!("ignoring a wake for an unknown terminal {id:?}");
            return false;
        };
        // タイマーに任せて戻るときも下ろす。タイマーが積む続きの知らせも同じ印を通るので、この知らせを受け取った
        // ことにしてから積ませる。
        self.sessions[index].wake_received();
        if self.output_continuations.contains(&id) {
            tracing::debug!("leaving a wake for terminal {id:?} to the output continuation timer");
            return false;
        }
        let events = self.sessions[index].on_output();
        self.on_clipboard_requests(index);
        let shown = index == self.active;
        if events.bell {
            self.ring_bell();
        }
        if shown {
            if events.title_changed {
                self.update_window_title();
            }
            // マウスを載せている文字が変わったかもしれない。
            self.refresh_hovered_link_now();
            self.invalidate();
        } else if events.title_changed || events.bell {
            // タブバーのタイトルとベルの印を描き直す。
            if events.bell {
                self.sessions[index].bell_marked = true;
            }
            self.invalidate();
        }
        if events.more_output {
            self.continue_output(index);
        }
        events
            .exited
            .is_some_and(|code| self.on_session_ended(index, code))
    }

    /// `sessions` の `index` の端末の流しきれなかった出力を、次のメッセージで続けて流す。積んだメッセージはキー入力と
    /// 描き直しより先に取り出されるので、それらが待っていればタイマーで流す（`WM_TIMER` はそれらのあとに届く）。
    /// 待っていなければ、すぐに [`WM_APP_WAKE`](super::WM_APP_WAKE) を積む（読むためのスレッドがもう積んでいれば、
    /// それで流す）。積めなければタイマーで流す。
    fn continue_output(&mut self, index: usize) {
        let session = &self.sessions[index];
        let id = session.id();
        // SAFETY: このスレッドのキューを調べるだけ。上位の 16 ビットが、いまキューにあるものの種類。
        let waiting = unsafe { GetQueueStatus(QS_INPUT | QS_PAINT) } >> 16;
        if waiting == 0 && session.wake() {
            return;
        }
        if !self.output_continuations.contains(&id) {
            self.output_continuations.push(id);
        }
        if start_output_timer(self.hwnd) {
            return;
        }
        // タイマーも動かせなければ、キー入力と描き直しより先になっても積む。
        let sessions = &self.sessions;
        for id in std::mem::take(&mut self.output_continuations) {
            if let Some(session) = sessions.iter().find(|session| session.id() == id) {
                session.wake();
            }
        }
    }

    /// 流しきれなかった出力の続きを流す。キー入力と描き直しはこのタイマーの前に処理されている。積めなかった
    /// 端末は残して、タイマーでもう一度試す。
    pub(super) fn on_output_timer(&mut self) {
        // SAFETY: self.hwnd は有効なウィンドウ。
        if let Err(e) = unsafe { KillTimer(Some(self.hwnd), OUTPUT_TIMER_ID) } {
            tracing::warn!("failed to stop the timer to continue the output: {e}");
        }
        let sessions = &self.sessions;
        // 閉じた端末は close_session が外すので、見つからなければ外すだけ。
        self.output_continuations.retain(|id| {
            sessions
                .iter()
                .find(|session| session.id() == *id)
                .is_some_and(|session| !session.wake())
        });
        if !self.output_continuations.is_empty() {
            start_output_timer(self.hwnd);
        }
    }

    /// `sessions` の `index` の端末の子プロセスが `code` で終わった。その端末を閉じる。最後の端末なら、
    /// それをウィンドウの終了コードにし、ウィンドウを閉じるよう `true` を返す。
    fn on_session_ended(&mut self, index: usize, code: ExitCode) -> bool {
        if !self.close_session(index) {
            return false;
        }
        self.exit = Some(code);
        true
    }

    /// ウィンドウのタイトルを、表示している端末のタイトルとバージョンから決める（`window_title`）。
    /// 変わったときだけウィンドウに設定する。
    fn update_window_title(&mut self) {
        let title = window_title(self.session().title(), &version::display(), self.elevated);
        if title == self.window_title {
            return;
        }
        // SAFETY: self.hwnd は有効なウィンドウ。文字列はこの呼び出しの間は生きている。
        if let Err(e) = unsafe { SetWindowTextW(self.hwnd, &HSTRING::from(&title)) } {
            tracing::warn!("failed to set the window title: {e}");
            return;
        }
        self.window_title = title;
    }

    /// ベル。ウィンドウが前面になければ、前面に来るまでタスクバーのボタンを点滅させる。音は鳴らさない。
    fn ring_bell(&self) {
        // SAFETY: 引数のない問い合わせ。
        if unsafe { GetForegroundWindow() } == self.hwnd {
            return;
        }
        let flash = FLASHWINFO {
            cbSize: size_of::<FLASHWINFO>() as u32,
            hwnd: self.hwnd,
            dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
            // FLASHW_TIMERNOFG で、回数ではなく前面に来るまで点滅を続ける。
            uCount: 0,
            // 0 でカーソルの点滅の間隔を使う。
            dwTimeout: 0,
        };
        // SAFETY: flash は初期化済みで、self.hwnd は有効なウィンドウ。戻り値は前の点滅の状態で、失敗では
        // ない。
        unsafe { _ = FlashWindowEx(&flash) };
    }
}

/// 端末のタブに出すタイトル。アプリが付けたタイトル、付けていなければ既定のタイトル。
pub(super) fn tab_title(session: &Session) -> &str {
    match session.title() {
        "" => WINDOW_TITLE,
        title => title,
    }
}

/// 管理者として動いているウィンドウのタイトルの先頭に付ける印。
const ELEVATED_TITLE_PREFIX: &str = "管理者: ";

/// ウィンドウに出すタイトル。アプリが付けたタイトル `tab_title` があれば
/// `<tab_title> - fantermo <version>`、なければ（空なら）`fantermo <version>`。`elevated` なら
/// 先頭に [`ELEVATED_TITLE_PREFIX`] を付ける。
pub(super) fn window_title(tab_title: &str, version: &str, elevated: bool) -> String {
    let prefix = if elevated { ELEVATED_TITLE_PREFIX } else { "" };
    if tab_title.is_empty() {
        format!("{prefix}{WINDOW_TITLE} {version}")
    } else {
        format!("{prefix}{tab_title} - {WINDOW_TITLE} {version}")
    }
}

/// `closed` の位置の端末を閉じて `remaining` 個になったあとの、表示する端末の位置。表示していた
/// `active` を閉じたら、右隣（右端なら新しい右端）を表示する。
fn active_after_close(active: usize, closed: usize, remaining: usize) -> usize {
    if closed < active {
        active - 1
    } else if closed == active {
        closed.min(remaining.saturating_sub(1))
    } else {
        active
    }
}

/// [`OUTPUT_TIMER_ID`] のタイマーを動かす。動かせなければ warn を残して `false` を返す。
pub(super) fn start_output_timer(hwnd: HWND) -> bool {
    // SAFETY: hwnd は有効なウィンドウ。同じ番号のタイマーは置き換わる。
    if unsafe { SetTimer(Some(hwnd), OUTPUT_TIMER_ID, USER_TIMER_MINIMUM, None) } != 0 {
        return true;
    }
    tracing::warn!(
        "failed to start the timer to continue the output: {}",
        windows::core::Error::from_thread()
    );
    false
}

#[cfg(test)]
mod tests {
    use fantermo_core::config::BackendKind;

    use super::*;

    /// コマンドだけを指定した [`TabRequest`]。
    fn command(line: Option<&str>) -> TabRequest {
        TabRequest {
            profile: None,
            command: line.map(str::to_owned),
        }
    }

    #[test]
    fn window_titles_end_with_the_version() {
        assert_eq!(
            window_title("vim", "0.0.1 (dev-abc1234)", false),
            "vim - fantermo 0.0.1 (dev-abc1234)"
        );
        assert_eq!(window_title("vim", "0.0.1", false), "vim - fantermo 0.0.1");
    }

    #[test]
    fn window_titles_without_a_tab_title_are_only_the_version() {
        assert_eq!(window_title("", "0.0.1", false), "fantermo 0.0.1");
        assert_eq!(
            window_title("", "0.0.1 (dev-abc1234)", false),
            "fantermo 0.0.1 (dev-abc1234)"
        );
    }

    #[test]
    fn elevated_window_titles_start_with_the_mark() {
        assert_eq!(
            window_title("vim", "0.0.1", true),
            "管理者: vim - fantermo 0.0.1"
        );
        assert_eq!(window_title("", "0.0.1", true), "管理者: fantermo 0.0.1");
    }

    #[test]
    fn closing_a_tab_shows_its_right_neighbour() {
        // 表示しているタブを閉じたら、右隣が繰り上がる。右端なら新しい右端。
        assert_eq!(active_after_close(1, 1, 3), 1);
        assert_eq!(active_after_close(2, 2, 2), 1);
        assert_eq!(active_after_close(0, 0, 1), 0);
        // 表示していないタブを閉じても、表示しているタブはそのまま。
        assert_eq!(active_after_close(2, 0, 3), 1);
        assert_eq!(active_after_close(0, 2, 2), 0);
    }

    #[test]
    fn command_line_prefers_arguments_then_the_default_profile() {
        let profiles = [Profile::builtin(), Profile::from_shell("zsh -l")];
        let line = |arguments: Option<&str>, default, launch| {
            let startup = startup_for(&command(arguments), &profiles, default, launch, None);
            assert_eq!(startup.backend, BackendKind::Conpty);
            startup.command_line
        };
        assert_eq!(line(Some("cmd.exe"), 1, Launch::Default), "cmd.exe");
        assert_eq!(line(None, 1, Launch::Default), "zsh -l");
        assert_eq!(line(None, 0, Launch::Default), "powershell.exe -NoLogo");
        // 一覧から選んだプロファイルは、引数があってもそのプロファイル。
        assert_eq!(line(Some("cmd.exe"), 0, Launch::Profile(1)), "zsh -l");
        // 並びの外なら先頭。
        assert_eq!(line(None, 5, Launch::Default), "powershell.exe -NoLogo");
        assert_eq!(line(None, 0, Launch::Profile(7)), "powershell.exe -NoLogo");
        assert_eq!(
            startup_for(&TabRequest::default(), &[], 0, Launch::Default, None),
            Startup::conpty(config::DEFAULT_COMMAND_LINE)
        );
        // ブリッジのプロファイルは、接続先も渡す。
        let wsl = Profile {
            distribution: Some("Ubuntu".to_owned()),
            ..Profile::new("u", "", BackendKind::Wsl)
        };
        assert_eq!(
            startup_for(
                &command(Some("cmd.exe")),
                std::slice::from_ref(&wsl),
                0,
                Launch::Profile(0),
                None
            ),
            Startup {
                backend: BackendKind::Wsl,
                command_line: String::new(),
                cygwin_root: None,
                distribution: Some("Ubuntu".to_owned()),
                tab_icon: TabIcon::for_profiles(std::slice::from_ref(&wsl), None).remove(0),
                profile_name: Some(wsl.name.clone()),
                working_folder: None,
            }
        );
    }

    #[test]
    fn a_named_profile_is_opened_with_the_requested_command() {
        let wsl = Profile {
            distribution: Some("Ubuntu".to_owned()),
            ..Profile::new("u", "/bin/zsh", BackendKind::Wsl)
        };
        let profiles = [Profile::builtin(), wsl.clone()];
        let startup = |profile: &str, requested: Option<&str>, launch| {
            let request = TabRequest {
                profile: Some(profile.to_owned()),
                command: requested.map(str::to_owned),
            };
            startup_for(&request, &profiles, 0, launch, None)
        };

        let plain = startup("u", None, Launch::Default);
        assert_eq!(plain.command_line, "/bin/zsh");
        assert_eq!(plain.profile_name.as_deref(), Some("u"));

        let with_command = startup("u", Some("vim"), Launch::Default);
        assert_eq!(with_command.command_line, "vim");
        assert_eq!(with_command.backend, BackendKind::Wsl);
        assert_eq!(with_command.distribution.as_deref(), Some("Ubuntu"));
        assert_eq!(with_command.profile_name.as_deref(), Some("u"));
        assert_eq!(with_command.tab_icon, plain.tab_icon);

        // 一覧から選んだプロファイルは、指定があってもそのプロファイルの `command`。
        let chosen = startup("u", Some("vim"), Launch::Profile(0));
        assert_eq!(chosen.command_line, config::DEFAULT_COMMAND_LINE);

        // 名前の合うものがなければ、指定がなかったものとして決める。比較は完全一致。
        let unknown = startup("U", None, Launch::Default);
        assert_eq!(unknown.command_line, config::DEFAULT_COMMAND_LINE);
        assert_eq!(
            startup("U", Some("vim"), Launch::Default),
            Startup::conpty("vim")
        );
    }

    #[test]
    fn tabs_get_the_icon_of_their_profile() {
        let profiles = [Profile {
            icon: Some("zsh.png".to_owned()),
            ..Profile::new("zsh", "zsh -l", BackendKind::Cygwin)
        }];
        let folder = Path::new(r"C:\conf");
        let icon = |arguments: Option<&str>, launch| {
            startup_for(&command(arguments), &profiles, 0, launch, Some(folder)).tab_icon
        };
        let from_profile = TabIcon::for_profiles(&profiles, Some(folder)).remove(0);
        assert_eq!(icon(None, Launch::Default), from_profile);
        assert_eq!(from_profile.image, Some(folder.join("zsh.png")));
        // 引数のコマンドラインで開いたタブはプロファイルなし。一覧から選べばプロファイルのもの。
        assert_eq!(
            icon(Some("cmd.exe"), Launch::Default),
            TabIcon::without_profile()
        );
        assert_eq!(icon(Some("cmd.exe"), Launch::Profile(0)), from_profile);
    }
}
