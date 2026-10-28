//! 端末一つ分の状態。端末エミュレーションと子プロセスのバックエンドに、選択・ホイールの余り・
//! リンクの下線・タイトルなど、端末ごとに持つ状態を添える。窓・描画・設定は持たない。

use fantermo_core::{
    backend::{Backend, ExitCode, GridSize},
    link::Link,
    vt::Vt,
};

use crate::conpty::{ConPty, SpawnOptions, Wake};

/// 子プロセスの `TERM` と、端末が名乗る terminfo の名前。
const TERM: &str = "xterm-256color";

/// 窓の中で端末を見分ける番号。出力や終了の知らせがどの端末からのものかを、この番号で区別する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionId(pub usize);

pub struct Session {
    id: SessionId,
    pub vt: Vt,
    backend: ConPty,
    /// アプリが付けたタイトル。消されたか、付けていなければ空。
    title: String,
    /// 高精度のホイールで、まだ 1 行に満たない回転量。
    pub wheel_remainder: i32,
    /// アプリへホイールを送るときの、まだ 1 ノッチに満たない回転量。
    pub wheel_report_remainder: i32,
    /// 左ボタンで選んでいる最中。
    pub selecting: bool,
    /// Ctrl を押している間の、マウスの下のリンク。下線を引き、カーソルを手の形にする。
    pub hovered_link: Option<Link>,
    /// Ctrl を押しながら左ボタンを押したリンクの URI。同じリンクの上で離したら開く。
    pub pressed_link: Option<String>,
    /// 表示していない間にベルが鳴った。タブに印を付け、表示したら消す。
    pub bell_marked: bool,
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
    /// `size` の端末を作り、`command_line` を子プロセスとして起動する。`wake` は出力が届いたときと
    /// 子プロセスが終了したときに、別のスレッドから呼ばれる。
    pub fn spawn(
        id: SessionId,
        command_line: &str,
        size: GridSize,
        wake: Wake,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let vt = Vt::new(size, TERM)?;
        let backend = ConPty::spawn(
            &SpawnOptions {
                command_line,
                env: &[("TERM", TERM)],
                size,
            },
            wake,
        )?;
        Ok(Self {
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
        })
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    /// アプリが付けたタイトル。消されたか、付けていなければ空。
    pub fn title(&self) -> &str {
        &self.title
    }

    /// マウスの操作の途中の状態（選択のドラッグ、リンクの下線、押したリンク）を終える。表示から
    /// 外すときに使う。選んだ範囲は残す。
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

    /// 端末の大きさを `size` にする。変わらなければ何もせず `false`。子プロセスに伝えるのは、
    /// 桁数か行数が変わったときだけ（セルのピクセル寸法は ConPTY に渡らない）。
    pub fn resize(&mut self, size: GridSize) -> bool {
        let current = self.vt.size();
        if size == current {
            return false;
        }
        if let Err(e) = self.vt.resize(size) {
            tracing::warn!("failed to resize the terminal: {e}");
        }
        if (size.cols, size.rows) != (current.cols, current.rows)
            && let Err(e) = self.backend.resize(size)
        {
            tracing::warn!("failed to resize the pseudo console: {e}");
        }
        true
    }
}
