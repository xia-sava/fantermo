//! 端末一つ分の状態。端末エミュレーションと子プロセスのバックエンドに、選択・ホイールの余り・
//! リンクの下線・タイトル・背景画像に選んだファイルなど、端末ごとに持つ状態を添える。ウィンドウ・描画・
//! 設定は持たない（背景画像のテクスチャは描画が持つ）。

use std::{
    io,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

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
    tab_icon::TabIcon,
    version,
};

/// 子プロセスの `TERM` と、端末が名乗る terminfo の名前。
const TERM: &str = "xterm-256color";
/// 子プロセスの `TERM_PROGRAM`。
const TERM_PROGRAM: &str = "fantermo";
/// [`Session::on_output`] 一回で端末エミュレーションに流すバイト数の上限。残りは次のメッセージで流すので、
/// その間にキー入力・描画・タブの操作が処理される。1 MiB の流し込みは、文字だけで約 1.4 ms、数文字ごとに色を
/// 変える出力で約 27 ms、重ね塗りを繰り返す sixel で約 36 ms（リリースのビルド、120×40 の端末）。流す回数を
/// 増やさずに、ウィンドウのスレッドが止まる時間を数フレームまでに収める大きさ。ブリッジのデータの単位の上限
/// （1 MiB）と同じなので、一つの単位は一回で流せる。表示している端末の続きはたいていタイマーを通るので
/// （`App::continue_output`）、溜まった分は 1 MiB ごとに少なくともタイマーの刻みをかけて流れる。描画と入力を
/// 先にするための遅れで、意図したもの。
const MAX_OUTPUT_PER_CALL: usize = 1024 * 1024;

/// 子プロセスへ渡す環境変数。
const CHILD_ENV: [(&str, &str); 3] = [
    ("TERM", TERM),
    ("TERM_PROGRAM", TERM_PROGRAM),
    ("TERM_PROGRAM_VERSION", version::PACKAGE),
];

/// 端末で起動するもの。バックエンドの種類と、子プロセスのコマンドライン（ブリッジなら相手の側の
/// コマンドラインで、空なら相手のログインシェル）、ブリッジの接続先。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Startup {
    pub backend: BackendKind,
    pub command_line: String,
    /// `cygwin` の Cygwin のフォルダ。`None` なら環境変数 `PATH` と標準のインストール先から探す。
    pub cygwin_root: Option<String>,
    /// `wsl` のディストリビューション。`None` なら既定。
    pub distribution: Option<String>,
    /// タブに出すアイコン。
    pub tab_icon: TabIcon,
    /// プロファイルの名前。プロファイルなしで起動するなら `None`。
    pub profile_name: Option<String>,
    /// 作業フォルダ。`None` ならこのプロセスの作業フォルダ。
    pub working_folder: Option<PathBuf>,
}

impl Startup {
    /// プロファイルなしで `command_line` を ConPTY で起動する。
    pub fn conpty(command_line: &str) -> Self {
        Self {
            backend: BackendKind::Conpty,
            command_line: command_line.to_owned(),
            cygwin_root: None,
            distribution: None,
            tab_icon: TabIcon::without_profile(),
            profile_name: None,
            working_folder: None,
        }
    }

    /// プロファイル `profile` のものを起動する。タブには `tab_icon` を出す。
    pub fn from_profile(profile: &Profile, tab_icon: TabIcon) -> Self {
        Self {
            backend: profile.backend,
            command_line: profile.command.clone(),
            cygwin_root: profile.cygwin_root.clone(),
            distribution: profile.distribution.clone(),
            tab_icon,
            profile_name: Some(profile.name.clone()),
            working_folder: None,
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

/// 端末一つ分の、出力と子プロセスの終了の知らせ（ウィンドウの `WM_APP_WAKE`）。積んでまだ受け取られていない
/// 知らせを端末ごとに高々一つにする。バックエンドのスレッド（[`Wake`]）とウィンドウのスレッド（流しきれなかった
/// 出力の続き）の両方がこれを通して積む。
///
/// 印が立っている間は、その端末の知らせが一つ、ウィンドウのキューにあるか、借用が重なって飛ばして積み直しを
/// 待っている。受け取ったウィンドウのスレッドは、出力を受け取る前に [`received`](Self::received) で印を下ろす。
/// 下ろすまでに入った出力は、そのあとに受け取る。下ろしたあとに入った出力は、新しく積む知らせで受け取る。
///
/// 積むことと印を立てることは同じロックの中でする。積めなかったら印は立てないので、印が立っているのに
/// 積んでいない間はない（印を先に立てて失敗したら戻す形だと、その間に別のスレッドが印を見て積まずに戻り、
/// 知らせが失われる）。
///
/// 読むためのスレッドの知らせは、`OutputBudget` の印でもまとめる。二つは下ろす位置が違う。あちらはウィンドウの
/// スレッドが出力を受け取り始めたところで下ろし、受け取られるまで読むためのスレッドが知らせ直さないためのもの。
/// こちらは知らせを受け取ったところで下ろし、ウィンドウのスレッドが積む続きの知らせも含めて一つにするためのもの。
/// タイマーで続きを待っている端末への知らせは、受け取っても出力は受け取らないので、あちらは立ったままになり、
/// 読むためのスレッドは続きが流れるまで積まない。
pub struct WakeMessage {
    /// 積んで、まだウィンドウのスレッドが受け取っていない。
    posted: Mutex<bool>,
    /// 知らせを積む。積めたら `true`。待たずに戻る。
    post: Box<dyn Fn() -> bool + Send + Sync>,
}

impl WakeMessage {
    /// 知らせを `post` で積むもの。
    pub fn new(post: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            posted: Mutex::new(false),
            post: Box::new(post),
        }
    }

    /// 知らせを積んでいなければ積む。積んであるか、積めたら `true`。どのスレッドから呼んでもよい。
    pub fn post(&self) -> bool {
        let mut posted = self.lock();
        if !*posted {
            // 積むだけで待たないので、ロックを持ったまま呼んでよい。
            *posted = (self.post)();
        }
        *posted
    }

    /// ウィンドウのスレッドが知らせを受け取った。出力を受け取る前に呼ぶ。
    pub fn received(&self) {
        *self.lock() = false;
    }

    fn lock(&self) -> MutexGuard<'_, bool> {
        // 中身は bool の代入だけで、途中の状態はないので、ほかのスレッドが panic したあともそのまま使う。
        self.posted.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub struct Session {
    id: SessionId,
    pub vt: Vt,
    backend: Box<dyn Backend>,
    /// 出力と子プロセスの終了の知らせ。バックエンドにも渡している。
    wake: Arc<WakeMessage>,
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
    /// `hovered_link` を探したセル（桁、行）。同じセルの上の移動では探し直さない。画面の中身や
    /// 表示している範囲が変わったら捨てる。
    pub hovered_cell: Option<(u16, u16)>,
    /// Ctrl を押しながら左ボタンを押したリンクの URI。同じリンクの上で離したら開く。
    pub pressed_link: Option<String>,
    /// 表示していない間にベルが鳴った。タブに印を付け、表示したら消す。
    pub bell_marked: bool,
    /// 背景画像に選んだファイルと、それを出した一覧の項目。敷かないなら `None`。
    pub background_image: Option<Candidate>,
    /// タブに出すアイコン。開いたときのプロファイルで決まり、プロファイルを変えても消しても変わらない。
    pub tab_icon: TabIcon,
    /// 開いたときのプロファイルの名前。プロファイルなしで開いたなら `None`。
    pub profile_name: Option<String>,
    /// このタブのアプリには、クリップボードの読み取りを確認せずに許可する。利用者がダイアログで決める。
    pub clipboard_read_allowed: bool,
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
    /// 上限まで流して、まだ流していない出力が残っているかもしれない。
    pub more_output: bool,
}

impl Session {
    /// 既定の色が `colors` の `size` の端末を作り、`startup` のバックエンドでそのコマンドラインを
    /// 子プロセスとして起動する。`post` は出力と子プロセスの終了の知らせを積む。[`WakeMessage`] を通して、
    /// 別のスレッドとウィンドウのスレッドから呼ばれる。
    pub fn spawn(
        id: SessionId,
        startup: &Startup,
        size: GridSize,
        colors: &TerminalColors,
        post: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let vt = new_vt(size, colors)?;
        let wake = Arc::new(WakeMessage::new(post));
        let backend = spawn_backend(
            startup,
            &SpawnOptions {
                command_line: &startup.command_line,
                env: &CHILD_ENV,
                size,
                working_folder: startup.working_folder.as_deref(),
            },
            backend_wake(&wake),
        )?;
        let mut session = Self::with_backend(id, vt, backend, wake);
        session.tab_icon = startup.tab_icon.clone();
        session.profile_name = startup.profile_name.clone();
        Ok(session)
    }

    /// 起動済みの `backend` を `vt` の端末に接続する。`wake` はバックエンドに渡したもの。
    fn with_backend(
        id: SessionId,
        vt: Vt,
        backend: Box<dyn Backend>,
        wake: Arc<WakeMessage>,
    ) -> Self {
        Self {
            id,
            vt,
            backend,
            wake,
            title: String::new(),
            wheel_remainder: 0,
            wheel_report_remainder: 0,
            selecting: false,
            hovered_link: None,
            hovered_cell: None,
            pressed_link: None,
            bell_marked: false,
            background_image: None,
            tab_icon: TabIcon::without_profile(),
            profile_name: None,
            clipboard_read_allowed: false,
        }
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    /// 出力の知らせを積む（[`WakeMessage::post`]）。積んであるか、積めたら `true`。流しきれなかった出力の
    /// 続きを流すときに、ウィンドウのスレッドが呼ぶ。
    pub fn wake(&self) -> bool {
        self.wake.post()
    }

    /// 出力の知らせを受け取った（[`WakeMessage::received`]）。知らせを受け取ったら、出力を受け取るかどうかに
    /// かかわらず、まず呼ぶ。
    pub fn wake_received(&self) {
        self.wake.received();
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
        self.hovered_cell = None;
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

    /// 届いている出力を [`MAX_OUTPUT_PER_CALL`] まで端末に流し込み、応答を返す。タイトルとベルの変化、
    /// 子プロセスの終了、流しきれずに残った出力があるかを返す。残っていれば、子プロセスの終了は残りを流しきった
    /// あとの呼び出しで返す。
    pub fn on_output(&mut self) -> OutputEvents {
        let vt = &mut self.vt;
        let more_output = self
            .backend
            .drain_output(MAX_OUTPUT_PER_CALL, &mut |data| vt.feed(data));
        let replies = self.vt.take_replies();
        if !replies.is_empty()
            && let Err(e) = self.backend.write(&replies)
        {
            tracing::warn!("failed to write replies to the child: {e}");
        }
        let title_changed = self.update_title();
        let bell = self.vt.take_bell();

        let exited = if more_output {
            None
        } else {
            self.backend.poll_exit()
        };
        if let Some(code) = exited {
            tracing::info!("child process exited with code {}", code.0);
        }
        OutputEvents {
            title_changed,
            bell,
            exited,
            more_output,
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

/// バックエンドへ渡す [`Wake`]。`wake` を通して積む。
fn backend_wake(wake: &Arc<WakeMessage>) -> Wake {
    let wake = Arc::clone(wake);
    Arc::new(move || wake.post())
}

/// 既定の色が `colors` の `size` の端末を作る。
fn new_vt(size: GridSize, colors: &TerminalColors) -> Result<Vt, Box<dyn std::error::Error>> {
    let mut vt = Vt::new(size, TERM)?;
    vt.set_default_colors(colors)?;
    Ok(vt)
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        collections::VecDeque,
        rc::Rc,
        sync::atomic::{AtomicUsize, Ordering},
    };

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

        fn drain_output(&mut self, limit: usize, sink: &mut dyn FnMut(&[u8])) -> bool {
            let mut passed = 0usize;
            while passed < limit {
                let Some(chunk) = self.0.borrow_mut().output.pop_front() else {
                    return false;
                };
                sink(&chunk);
                passed += chunk.len();
            }
            true
        }

        fn poll_exit(&mut self) -> Option<ExitCode> {
            self.0.borrow().exit
        }
    }

    fn fake_session() -> (Session, Rc<RefCell<FakeState>>) {
        let (session, state, _) = fake_session_posting(|_| true);
        (session, state)
    }

    /// 知らせを積むたびに、それまでに積もうとした回数を `post` に渡し、その結果を積めたかとする端末。積もうとした
    /// 回数も返す。
    fn fake_session_posting(
        post: impl Fn(usize) -> bool + Send + Sync + 'static,
    ) -> (Session, Rc<RefCell<FakeState>>, Arc<AtomicUsize>) {
        let state = Rc::new(RefCell::new(FakeState::default()));
        let colors = ColorsConfig::default().terminal_colors();
        let vt = new_vt(SIZE, &colors).unwrap();
        let posts = Arc::new(AtomicUsize::new(0));
        let wake = Arc::new(WakeMessage::new({
            let posts = Arc::clone(&posts);
            move || post(posts.fetch_add(1, Ordering::SeqCst))
        }));
        let session = Session::with_backend(
            SessionId(0),
            vt,
            Box::new(FakeBackend(Rc::clone(&state))),
            wake,
        );
        (session, state, posts)
    }

    #[test]
    fn a_wake_is_posted_once_until_it_is_received() {
        let (session, _, posts) = fake_session_posting(|_| true);
        let backend_wake = backend_wake(&session.wake);
        assert!(backend_wake());
        // 積んである間は、バックエンドからもウィンドウのスレッドからも積まない。
        assert!(backend_wake());
        assert!(session.wake());
        assert_eq!(posts.load(Ordering::SeqCst), 1);

        session.wake_received();
        assert!(session.wake());
        assert!(backend_wake());
        assert_eq!(posts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_wake_that_could_not_be_posted_is_posted_again() {
        let (session, _, posts) = fake_session_posting(|tried| tried > 0);
        let backend_wake = backend_wake(&session.wake);
        assert!(!backend_wake());
        assert!(backend_wake());
        assert!(session.wake());
        assert_eq!(posts.load(Ordering::SeqCst), 2);
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
    fn output_over_the_limit_is_fed_over_calls_and_the_exit_comes_after_it() {
        let (mut session, state) = fake_session();
        {
            let mut state = state.borrow_mut();
            // 復帰だけなら画面は変わらないので、上限の分を流しても後ろの文字だけが見える。
            state.output.push_back(vec![b'\r'; MAX_OUTPUT_PER_CALL]);
            state.output.push_back(b"tail".to_vec());
            state.exit = Some(ExitCode(3));
        }
        let first = session.on_output();
        assert!(first.more_output);
        assert_eq!(first.exited, None);
        assert!(!session.vt.plain_text().unwrap().contains("tail"));

        let second = session.on_output();
        assert!(!second.more_output);
        assert_eq!(second.exited, Some(ExitCode(3)));
        assert!(session.vt.plain_text().unwrap().contains("tail"));
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
