//! シェルと接続するバックエンド（ConPTY やブリッジ）の共通の形。

use std::{io, path::Path, sync::Arc};

/// 端末の大きさ。セルの数と、1 セルのピクセル寸法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
}

/// 子プロセスの終了コード。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitCode(pub u32);

/// 出力が届いたときと、子プロセスが終了したときに、別のスレッドから呼ばれる。
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// バックエンドを起動するときに渡すもの。どの種類のバックエンドにも共通。
pub struct SpawnOptions<'a> {
    /// 子プロセスのコマンドライン。
    pub command_line: &'a str,
    /// 子プロセスに渡す環境変数のうち、このプロセスの環境から変えるもの。
    pub env: &'a [(&'a str, &'a str)],
    /// 起動したときの端末の大きさ。
    pub size: GridSize,
    /// 子プロセスの作業フォルダ。`None` ならこのプロセスの作業フォルダ。
    pub working_folder: Option<&'a Path>,
}

/// シェルとの間の入出力を受け持つもの。
///
/// 出力は別のスレッドなどで読み取っておき、`drain_output` で溜まっている分（上限まで）を渡す。出力が届いた
/// ときと子プロセスが終了したときは、起動のときに受け取った [`Wake`] を呼ぶ。
/// ウィンドウのスレッドから呼ばれるので、どの操作も待たずに戻ること。
/// 破棄すると子プロセスとの接続を閉じ、読むためのスレッドなどの後始末をする。
pub trait Backend {
    /// 子プロセスへの入力を書き込む。
    fn write(&mut self, data: &[u8]) -> io::Result<()>;

    /// 端末の大きさが変わったことを子プロセスに伝える。桁数・行数・セルのピクセル寸法のどれかが
    /// 変わるたびに呼ばれる。どれを伝えるかはバックエンドが決める。
    fn resize(&mut self, size: GridSize) -> io::Result<()>;

    /// 読み取り済みの出力を、溜まっている分だけ、届いた順に `sink` に渡す。
    ///
    /// 渡したバイト数が `limit` に達したら、残りがあっても止めて `true` を返す。読み取った塊は分けずに渡すので、
    /// 渡すのは `limit` に最後の一塊を足した分まで。残りは次の呼び出しで続きから渡す。止めたあとに次の出力が
    /// 届かなければ `Wake` は呼ばれないので、呼び出し元が自分でもう一度呼ぶ。溜まっている分を渡しきったら
    /// `false` を返す。
    ///
    /// バックエンドは、まだ受け取られていない出力に上限を置き、上限に達したら受け取られるまで読むのを
    /// 待ってよい。そのあいだ子プロセスの出力は止まる。
    fn drain_output(&mut self, limit: usize, sink: &mut dyn FnMut(&[u8])) -> bool;

    /// 子プロセスが終了していれば、その終了コードを返す。[`drain_output`](Self::drain_output) が `true` を
    /// 返したあとは、まだ渡していない出力が残っているかもしれないので、`false` を返すまで渡しきってから呼ぶ
    /// （終了を先に扱うと、最後の出力を捨てる）。
    fn poll_exit(&mut self) -> Option<ExitCode>;
}
