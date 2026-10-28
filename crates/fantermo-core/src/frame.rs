//! 描画のために取り出した画面の内容。libghostty-vt の型に依存しない。

/// 8 ビットの RGB。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// 下線の種類（SGR 4 と 4:0〜4:5）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    /// 波線。
    Curly,
    Dotted,
    Dashed,
}

/// 文字・背景色・装飾のどれかを持つセル。
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
    pub underline: Underline,
    /// 下線の色（SGR 58）。`None` なら文字と同じ色で描く。
    pub underline_color: Option<Rgb>,
    pub strikethrough: bool,
    pub overline: bool,
}

impl CellView {
    /// 下線・取り消し線・上線のどれかを持つ。
    pub fn has_decoration(&self) -> bool {
        self.underline != Underline::None || self.strikethrough || self.overline
    }
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
    /// 文字・背景色・装飾のどれかを持つセルだけを並べる。全角の文字の右半分は含めない。
    pub cells: Vec<CellView>,
    /// 表示中で、画面の中にあるときだけ持つ。
    pub cursor: Option<CursorView>,
}
