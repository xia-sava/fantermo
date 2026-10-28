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
    /// 点滅（SGR 5 と SGR 6）。
    pub blink: bool,
}

impl CellView {
    /// 下線・取り消し線・上線のどれかを持つ。
    pub fn has_decoration(&self) -> bool {
        self.underline != Underline::None || self.strikethrough || self.overline
    }

    /// 文字と装飾を除き、背景だけを残す。
    pub fn clear_text_and_decoration(&mut self) {
        self.text.clear();
        self.underline = Underline::None;
        self.strikethrough = false;
        self.overline = false;
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

impl Frame {
    /// 点滅させると見た目が変わるセルを持つ。点滅の属性を持っていても、空白だけで装飾もないセルは数えない。
    pub fn has_blink(&self) -> bool {
        self.cells
            .iter()
            .any(|cell| cell.blink && (!cell.text.trim().is_empty() || cell.has_decoration()))
    }

    /// 点滅の文字を隠す。点滅するセルの文字と装飾を除き、背景だけを残す。
    pub fn hide_blinking_text(&mut self) {
        for cell in self.cells.iter_mut().filter(|cell| cell.blink) {
            cell.clear_text_and_decoration();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgb = Rgb {
        r: 255,
        g: 255,
        b: 255,
    };
    const BLUE: Rgb = Rgb { r: 0, g: 0, b: 255 };

    fn cell(col: u16, text: &str, blink: bool) -> CellView {
        CellView {
            col,
            row: 0,
            text: text.to_owned(),
            width: 1,
            fg: WHITE,
            bg: Some(BLUE),
            bold: false,
            italic: false,
            underline: Underline::Curly,
            underline_color: None,
            strikethrough: true,
            overline: true,
            blink,
        }
    }

    fn frame(cells: Vec<CellView>) -> Frame {
        Frame {
            cols: 10,
            rows: 1,
            background: Rgb { r: 0, g: 0, b: 0 },
            foreground: WHITE,
            cells,
            cursor: None,
        }
    }

    #[test]
    fn hiding_blinking_text_keeps_the_background_and_other_cells() {
        let mut frame = frame(vec![cell(0, "a", true), cell(1, "b", false)]);
        assert!(frame.has_blink());
        frame.hide_blinking_text();
        let hidden = &frame.cells[0];
        assert_eq!(hidden.text, "");
        assert!(!hidden.has_decoration());
        assert_eq!(hidden.bg, Some(BLUE));
        assert_eq!(frame.cells[1], cell(1, "b", false));
    }

    #[test]
    fn frames_without_blinking_cells_have_no_blink() {
        assert!(!frame(vec![cell(0, "a", false)]).has_blink());
        assert!(!frame(Vec::new()).has_blink());
        // 空白だけで装飾もない点滅のセルは、隠しても見た目が変わらない。
        let mut space = CellView {
            underline: Underline::None,
            strikethrough: false,
            overline: false,
            ..cell(0, " ", true)
        };
        assert!(!frame(vec![space.clone()]).has_blink());
        space.underline = Underline::Single;
        assert!(frame(vec![space]).has_blink());
    }
}
