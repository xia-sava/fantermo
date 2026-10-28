//! 描画のために取り出した画面の内容。libghostty-vt の型に依存しない。

/// 8 ビットの RGB。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// 文字か背景色を持つセル。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellView {
    pub col: u16,
    pub row: u16,
    /// セルの文字（書記素クラスタ）。空なら背景だけを描く。
    pub text: String,
    /// 占めるセルの数。全角の文字は 2。
    pub width: u8,
    pub fg: Rgb,
    /// 既定の背景色と違うときだけ持つ。
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
}

/// 画面に描くカーソル。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorView {
    pub col: u16,
    pub row: u16,
    /// カーソルが載っている文字の幅。
    pub width: u8,
    pub color: Rgb,
}

/// 1 フレーム分の画面。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub cols: u16,
    pub rows: u16,
    pub background: Rgb,
    pub foreground: Rgb,
    /// 文字か背景色を持つセルだけを並べる。全角の文字の右半分は含めない。
    pub cells: Vec<CellView>,
    /// 表示中で、画面の中にあるときだけ持つ。
    pub cursor: Option<CursorView>,
}
