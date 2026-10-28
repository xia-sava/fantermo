//! シェルとつなぐバックエンド（ConPTY やブリッジ）の共通の形。

use std::io;

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

/// シェルとの間の入出力を受け持つもの。
///
/// 出力は別のスレッドなどで読み出しておき、`drain_output` で溜まっている分だけを渡す。
/// 窓のスレッドから呼ばれるので、どの操作も待たずに戻ること。
pub trait Backend {
    /// 子プロセスへの入力を書き込む。
    fn write(&mut self, data: &[u8]) -> io::Result<()>;

    /// 端末の大きさが変わったことを子プロセスに伝える。
    fn resize(&mut self, size: GridSize) -> io::Result<()>;

    /// 読み出し済みの出力を、溜まっている分だけ `sink` に渡す。
    fn drain_output(&mut self, sink: &mut dyn FnMut(&[u8]));

    /// 子プロセスが終了していれば、その終了コードを返す。
    fn poll_exit(&mut self) -> Option<ExitCode>;
}
