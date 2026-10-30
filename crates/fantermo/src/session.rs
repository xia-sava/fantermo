//! 端末一つ分の状態。端末エミュレーションと子プロセスのバックエンドに、選択・ホイールの余り・
//! リンクの下線・タイトル・背景画像に選んだファイルなど、端末ごとに持つ状態を添える。ウィンドウ・描画・
//! 設定は持たない（背景画像のテクスチャは描画が持つ）。

use std::io;

use fantermo_core::{
    backend::{Backend, ExitCode, GridSize, SpawnOptions, Wake},
    background::Candidate,
    config::{BackendKind, Profile, TerminalColors},
    link::Link,
    vt::Vt,
};

use crate::{
    bridge::{self, Bridge},
    conpty::ConPty,
    version,
};

/// 子プロセスの `TERM` と、端末が名乗る terminfo の名前。
const TERM: &str = "xterm-256color";
/// 子プロセスの `TERM_PROGRAM`。
const TERM_PROGRAM: &str = "fantermo";
/// 子プロセスへ渡す環境変数。
const CHILD_ENV: [(&str, &str); 3] = [
    ("TERM", TERM),
    ("TERM_PROGRAM", TERM_PROGRAM),
    ("TERM_PROGRAM_VERSION", version::PACKAGE),
];

/// 端末で起動するもの。バックエンドの種類と、子プロセスのコマンドライン（ブリッジなら相手の側の
/// コマンドラインで、空なら相手のログインシェル）、ブリッジのつなぐ先。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Startup {
    pub backend: BackendKind,
    pub command_line: String,
    /// `cygwin` の Cygwin のフォルダ。`None` なら環境変数 `PATH` とよくある場所から探す。
    pub cygwin_root: Option<String>,
    /// `wsl` のディストリビューション。`None` なら既定。
    pub distribution: Option<String>,
}

impl Startup {
    /// `command_line` を ConPTY で起動する。
    pub fn conpty(command_line: &str) -> Self {
        Self {
            backend: BackendKind::Conpty,
            command_line: command_line.to_owned(),
            cygwin_root: None,
            distribution: None,
        }
    }

    /// プロファイル `profile` のものを起動する。
    pub fn from_profile(profile: &Profile) -> Self {
        Self {
            backend: profile.backend,
            command_line: profile.command.clone(),
            cygwin_root: profile.cygwin_root.clone(),
            distribution: profile.distribution.clone(),
        }
    }
}

/// `startup` の種類のバックエンドを起動する。
fn spawn_backend(
    startup: &Startup,
    options: &SpawnOptions,
    wake: Wake,
) -> io::Result<Box<dyn Backend>> {
    match startup.backend {
        BackendKind::Conpty => Ok(Box::new(ConPty::spawn(options, wake)?)),
        BackendKind::Cygwin => {
            let target = bridge::Target::Cygwin {
                root: startup.cygwin_root.as_deref(),
            };
            Ok(Box::new(Bridge::spawn(target, options, wake)?))
        }
        BackendKind::Wsl => {
            let target = bridge::Target::Wsl {
                distribution: startup.distribution.as_deref(),
            };
            Ok(Box::new(Bridge::spawn(target, options, wake)?))
        }
    }
}

/// ウィンドウの中で端末を見分ける番号。出力や終了の知らせがどの端末からのものかを、この番号で区別する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionId(pub usize);

pub struct Session {
    id: SessionId,
    pub vt: Vt,
    backend: Box<dyn Backend>,
    /// アプリが付けたタイトル。消されたか、付けていなければ空。
    title: String,
    /// 高精度のホイールで、まだ 1 行に満たない回転量。
    pub wheel_remainder: i32,
    /// アプリへホイールを送るときの、まだ 1 ノッチに満たない回転量。
    pub wheel_report_remainder: i32,
    /// 左ボタンで選んでいる最中。
    pub selecting: bool,
    /// Ctrl を押している間の、マウスを載せているリンク。下線を引き、カーソルを手の形にする。
    pub hovered_link: Option<Link>,
    /// Ctrl を押しながら左ボタンを押したリンクの URI。同じリンクの上で離したら開く。
    pub pressed_link: Option<String>,
    /// 表示していない間にベルが鳴った。タブに印を付け、表示したら消す。
    pub bell_marked: bool,
    /// 背景画像に選んだファイルと、それを出した一覧の項目。敷かないなら `None`。
    pub background_image: Option<Candidate>,
}

/// 子プロセスの出力を流し込んだ結果。
#[derive(Clone, Copy, Debug, Default)]
pub struct OutputEvents {
    /// タイトルが変わった。
    pub title_changed: bool,
    /// ベルが鳴った。
    pub bell: bool,
    /// 子プロセスが終了した。その終了コード。
    pub exited: Option<ExitCode>,
}

impl Session {
    /// 既定の色が `colors` の `size` の端末を作り、`startup` のバックエンドでそのコマンドラインを
    /// 子プロセスとして起動する。`wake` は出力が届いたときと子プロセスが終了したときに、別のスレッド
    /// から呼ばれる。
    pub fn spawn(
        id: SessionId,
        startup: &Startup,
        size: GridSize,
        colors: &TerminalColors,
        wake: Wake,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let vt = new_vt(size, colors)?;
        let backend = spawn_backend(
            startup,
            &SpawnOptions {
                command_line: &startup.command_line,
                env: &CHILD_ENV,
                size,
            },
            wake,
        )?;
        Ok(Self::with_backend(id, vt, backend))
    }

    /// 起動済みの `backend` を `vt` の端末につなぐ。
    fn with_backend(id: SessionId, vt: Vt, backend: Box<dyn Backend>) -> Self {
        Self {
            id,
            vt,
            backend,
            title: String::new(),
            wheel_remainder: 0,
            wheel_report_remainder: 0,
            selecting: false,
            hovered_link: None,
            pressed_link: None,
            bell_marked: false,
            background_image: None,
        }
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    /// アプリが付けたタイトル。消されたか、付けていなければ空。
    pub fn title(&self) -> &str {
        &self.title
    }

    /// マウスの操作の途中の状態（選択のドラッグ、リンクの下線、押したリンク）を終える。表示から
    /// 外すときに使う。選択範囲は残す。
    pub fn release_pointer(&mut self) {
        self.selecting = false;
        self.pressed_link = None;
        if self.hovered_link.take().is_some() {
            self.vt.set_link_underline(Vec::new());
        }
    }

    /// 子プロセスへ書き込む。
    pub fn write(&mut self, data: &[u8]) {
        if let Err(e) = self.backend.write(data) {
            tracing::warn!("failed to write input to the child: {e}");
        }
    }

    /// 届いている出力を端末に流し込み、応答を返す。タイトルとベルの変化、子プロセスの終了を返す。
    pub fn on_output(&mut self) -> OutputEvents {
        let vt = &mut self.vt;
        self.backend.drain_output(&mut |data| vt.feed(data));
        let replies = self.vt.take_replies();
        if !replies.is_empty()
            && let Err(e) = self.backend.write(&replies)
        {
            tracing::warn!("failed to write replies to the child: {e}");
        }
        let title_changed = self.update_title();
        let bell = self.vt.take_bell();

        let exited = self.backend.poll_exit();
        if let Some(code) = exited {
            tracing::info!("child process exited with code {}", code.0);
        }
        OutputEvents {
            title_changed,
            bell,
            exited,
        }
    }

    /// アプリがタイトルを変えていれば、覚えて `true` を返す。
    fn update_title(&mut self) -> bool {
        match self.vt.take_title_change() {
            Ok(Some(title)) => {
                self.title = title;
                true
            }
            Ok(None) => false,
            Err(e) => {
                tracing::warn!("failed to read the title: {e}");
                false
            }
        }
    }

    /// 端末の大きさを `size` にする。変わらなければ何もせず `false`。変わればバックエンドにも伝える。
    pub fn resize(&mut self, size: GridSize) -> bool {
        if size == self.vt.size() {
            return false;
        }
        if let Err(e) = self.vt.resize(size) {
            tracing::warn!("failed to resize the terminal: {e}");
        }
        if let Err(e) = self.backend.resize(size) {
            tracing::warn!("failed to tell the child the new size: {e}");
        }
        true
    }
}

/// 既定の色が `colors` の `size` の端末を作る。
fn new_vt(size: GridSize, colors: &TerminalColors) -> Result<Vt, Box<dyn std::error::Error>> {
    let mut vt = Vt::new(size, TERM)?;
    vt.set_default_colors(colors)?;
    Ok(vt)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};

    use fantermo_core::config::ColorsConfig;

    use super::*;

    const SIZE: GridSize = GridSize {
        cols: 40,
        rows: 10,
        cell_width_px: 8,
        cell_height_px: 16,
    };

    #[test]
    fn children_get_the_terminal_name_and_the_package_version() {
        assert_eq!(
            CHILD_ENV,
            [
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "fantermo"),
                ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION")),
            ]
        );
    }

    /// 偽のバックエンドとテストが共に見る中身。
    #[derive(Default)]
    struct FakeState {
        /// 子プロセスへ書き込まれたもの。
        written: Vec<u8>,
        /// 伝えられた大きさ。
        resizes: Vec<GridSize>,
        /// 次の `drain_output` で渡す出力。
        output: VecDeque<Vec<u8>>,
        exit: Option<ExitCode>,
    }

    struct FakeBackend(Rc<RefCell<FakeState>>);

    impl Backend for FakeBackend {
        fn write(&mut self, data: &[u8]) -> io::Result<()> {
            self.0.borrow_mut().written.extend_from_slice(data);
            Ok(())
        }

        fn resize(&mut self, size: GridSize) -> io::Result<()> {
            self.0.borrow_mut().resizes.push(size);
            Ok(())
        }

        fn drain_output(&mut self, sink: &mut dyn FnMut(&[u8])) {
            while let Some(chunk) = self.0.borrow_mut().output.pop_front() {
                sink(&chunk);
            }
        }

        fn poll_exit(&mut self) -> Option<ExitCode> {
            self.0.borrow().exit
        }
    }

    fn fake_session() -> (Session, Rc<RefCell<FakeState>>) {
        let state = Rc::new(RefCell::new(FakeState::default()));
        let colors = ColorsConfig::default().terminal_colors();
        let vt = new_vt(SIZE, &colors).unwrap();
        let session =
            Session::with_backend(SessionId(0), vt, Box::new(FakeBackend(Rc::clone(&state))));
        (session, state)
    }

    #[test]
    fn output_reaches_the_terminal_and_replies_go_back() {
        let (mut session, state) = fake_session();
        state
            .borrow_mut()
            .output
            .extend([b"ab".to_vec(), b"\x1b[6n".to_vec()]);
        let events = session.on_output();
        assert!(session.vt.plain_text().unwrap().contains("ab"));
        assert_eq!(state.borrow().written, b"\x1b[1;3R");
        assert!(!events.title_changed);
        assert!(!events.bell);
        assert_eq!(events.exited, None);
    }

    #[test]
    fn title_and_bell_are_reported_once() {
        let (mut session, state) = fake_session();
        state
            .borrow_mut()
            .output
            .push_back(b"\x1b]0;fantermo-title\x07\x07".to_vec());
        let events = session.on_output();
        assert!(events.title_changed);
        assert!(events.bell);
        assert_eq!(session.title(), "fantermo-title");

        let events = session.on_output();
        assert!(!events.title_changed);
        assert!(!events.bell);
    }

    #[test]
    fn exit_is_reported() {
        let (mut session, state) = fake_session();
        state.borrow_mut().exit = Some(ExitCode(3));
        assert_eq!(session.on_output().exited, Some(ExitCode(3)));
    }

    #[test]
    fn writes_reach_the_backend() {
        let (mut session, state) = fake_session();
        session.write(b"echo hi\r");
        assert_eq!(state.borrow().written, b"echo hi\r");
    }

    #[test]
    fn every_size_change_reaches_the_backend() {
        let (mut session, state) = fake_session();
        assert!(!session.resize(SIZE));
        assert!(state.borrow().resizes.is_empty());

        let wider = GridSize { cols: 50, ..SIZE };
        assert!(session.resize(wider));
        // セルのピクセル寸法だけが変わっても伝える。伝えるかどうかはバックエンドが決める。
        let larger_cells = GridSize {
            cell_width_px: 10,
            ..wider
        };
        assert!(session.resize(larger_cells));
        assert_eq!(state.borrow().resizes, [wider, larger_cells]);
        assert_eq!(session.vt.size(), larger_cells);
    }
}
