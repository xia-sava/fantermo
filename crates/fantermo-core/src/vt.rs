//! libghostty-vt の端末を包む。fantermo の中で libghostty-vt を直接使うのはこのモジュールだけにする。

use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    rc::Rc,
    sync::OnceLock,
};

use libghostty_vt::{
    Terminal,
    fmt::{Format, Formatter, FormatterOptions},
    key,
    kitty::graphics::{self, ImageFormat, RustPngDecoder},
    mouse,
    paste::Options as PasteOptions,
    render::{
        CellIterator, ImageIterator, ImageLayer as VtImageLayer, ImageProtocol, RenderState,
        RowIterator, Snapshot,
    },
    screen::{CellWide, GridRef},
    selection::{
        FormatOptions as SelectionFormatOptions,
        gesture::{DragEvent, Geometry, Gesture, PressEvent, ReleaseEvent},
    },
    style::{Palette, PaletteIndex, RgbColor, Style, StyleColor, Underline as StyleUnderline},
    terminal::{
        ClipboardMime, ConformanceLevel, DeviceAttributeFeature, DeviceAttributes, DeviceType,
        Point, PointCoordinate, PointSpace, PrimaryDeviceAttributes, ScrollViewport,
        SecondaryDeviceAttributes, SizeReportSize,
    },
};
use std::time::Duration;

use crate::{
    backend::GridSize,
    config::TerminalColors,
    frame::{
        CellView, CursorView, Frame, ImageKey, ImageLayer, ImagePixels, ImageSource, ImageView,
        Rgb, Underline,
    },
    link::{self, LineCell, Link, LinkSpan},
    output_scan::{OutputEvent, OutputScanner},
    sixel::SixelImage,
};

pub use libghostty_vt::{
    Error as VtError,
    key::{Action as KeyAction, Key, Mods},
    mouse::{Action as MouseAction, Button as MouseButton},
};

/// ロックの状態を表す修飾キー。
const LOCK_MODS: Mods = Mods::CAPS_LOCK.union(Mods::NUM_LOCK);

/// アプリへ報告するマウスの操作一つ。
#[derive(Clone, Copy, Debug)]
pub struct MouseInput {
    pub action: MouseAction,
    /// 押した・離したボタン。動かしただけなら、押しているボタン（なければ `None`）。
    pub button: Option<MouseButton>,
    pub mods: Mods,
    /// ウィンドウの中の位置（ピクセル）。
    pub x: f32,
    pub y: f32,
    /// どれかのボタンを押している。ドラッグとただの移動を区別するのに使う。
    pub any_button_pressed: bool,
}

/// 貼り付けた文字列の種類。
const PLAIN_TEXT_MIME: &str = "text/plain";

/// 貼り付けの結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteOutcome {
    Pasted,
    /// 貼り付けるものがなかった。
    Empty,
    /// コマンドとして実行されうるので送らなかった。
    Unsafe,
}

/// `text` が、端末のモードに関わらずコマンドとして実行されえない。改行（LF、CR）と bracketed paste の
/// 終わりの印（`ESC [ 201 ~`）を含めば `false`。
pub fn is_safe_to_paste(text: &str) -> bool {
    libghostty_vt::paste::is_safe(text)
}

/// 端末の外で文字列をセルに並べるときの、書記素クラスタ一つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grapheme {
    pub text: String,
    /// 占めるセルの数。端末と同じ数え方で、曖昧幅は 1。幅のない文字（制御文字など）だけなら 0。
    pub width: u8,
}

/// `text` を書記素クラスタに分け、それぞれが占めるセルの数を添える（端末の mode 2027 と同じ数え方）。
pub fn split_graphemes(text: &str) -> Vec<Grapheme> {
    let chars: Vec<char> = text.chars().collect();
    let mut graphemes = Vec::new();
    let mut rest = chars.as_slice();
    while !rest.is_empty() {
        let (consumed, width) = libghostty_vt::unicode::grapheme_width(rest);
        // 空でない入力では少なくとも 1 文字を読むはずだが、読まなければ 1 文字ずつ進めて止まらないようにする。
        let consumed = consumed.clamp(1, rest.len());
        graphemes.push(Grapheme {
            text: rest[..consumed].iter().collect(),
            width,
        });
        rest = &rest[consumed..];
    }
    graphemes
}

/// マウスの位置。表示している範囲の中のセルと、ウィンドウの中のピクセル。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerPosition {
    pub col: u16,
    pub row: u16,
    pub x: f64,
    pub y: f64,
}

/// 押された・離されたキー一つ。
#[derive(Clone, Copy, Debug)]
pub struct KeyInput<'a> {
    pub key: Key,
    pub action: KeyAction,
    pub mods: Mods,
    /// `text` を作るのに使われた修飾キー（Shift で大文字になった、など）。
    pub consumed_mods: Mods,
    /// 修飾キーなしでこのキーが出す文字。なければ `'\0'`。
    pub unshifted: char,
    /// このキーで入力された文字。制御文字は入れないこと（Ctrl+C の U+0003 など）。
    pub text: Option<&'a str>,
}

/// 表示している範囲の、スクロールバックを含む全体の中での位置（行の数）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollPosition {
    /// スクロールバックと画面を合わせた全体の行数。
    pub total: usize,
    /// 表示している範囲の先頭の、全体の先頭からの行数。いちばん新しい行を表示しているときは
    /// `total - len`。
    pub offset: usize,
    /// 表示している範囲の行数。
    pub len: usize,
}

/// スクロールバックに残す行数。
const SCROLLBACK_LINES: usize = 10_000;

/// 薄く（SGR 2）した文字の色を、背景に重ねるときの不透明度。
const FAINT_OPACITY: f32 = 0.5;

/// XTVERSION の問い合わせに返す名前。
const XTVERSION_NAME: &str = "fantermo";

/// リンクを探すときに、指した行の前後へ折り返しをたどる行数の上限。
const MAX_LINK_LINE_ROWS: u32 = 64;

/// セルの文字（書記素クラスタ）を読むときの、コードポイントの数の初めの見込み。
const GRAPHEME_BUFFER_LEN: usize = 16;

/// OSC 8 の URI を読むときの、バイト数の初めの見込み。
const URI_BUFFER_LEN: usize = 256;

/// kitty graphics protocol の画像を保管するバイト数の上限（画面ごと）。Ghostty の既定の
/// `image-storage-limit` と同じ。
const IMAGE_STORAGE_LIMIT: u64 = 320 * 1000 * 1000;

/// カーソルを同じ桁のまま一行下へ動かす（IND）。下端ではスクロールする。
const INDEX: &[u8] = b"\x1bD";

/// kitty graphics protocol の Unicode placeholder の文字。画像は描画の状態が一覧に載せるので、文字としては描かない。
const KITTY_PLACEHOLDER: char = '\u{10EEEE}';

/// PNG のデコーダをプロセスに一度だけ入れる。
fn install_png_decoder() -> Result<(), VtError> {
    static INSTALLED: OnceLock<Result<(), VtError>> = OnceLock::new();
    *INSTALLED.get_or_init(|| {
        // SAFETY: 端末はどれも `Vt::new` で作り、最初の端末を作る前にここを通る。ここを通っている間、
        // ほかのスレッドの `Vt::new` は `get_or_init` で待つので、端末を使うスレッドはまだない。
        unsafe { graphics::set_png_decoder(Some(Box::new(RustPngDecoder::new()))) }
    })
}

/// 端末の状態。シェルの出力を `feed` で流し込み、シェルへの応答を `take_replies` で取り出す。
pub struct Vt {
    terminal: Terminal<'static, 'static>,
    /// 端末に流す前に出力を見て、libghostty-vt が扱わないものを取り出す。
    output_scanner: OutputScanner,
    /// 端末がシェルへ返す応答（カーソル位置の報告など）。コールバックの中で溜める。
    replies: Rc<RefCell<Vec<u8>>>,
    /// 大きさの問い合わせに答えるための、現在の大きさ。
    size: Rc<Cell<GridSize>>,
    /// `take_title_change` が最後に返したタイトル。
    last_title: Vec<u8>,
    /// ベル（BEL）を受けた。`take_bell` で下ろす。
    bell_rang: Rc<Cell<bool>>,
    /// 画面を取り出すときに使い回す、描画用の状態と行・セルの反復子。
    render_state: RenderState<'static>,
    row_iter: RowIterator<'static>,
    cell_iter: CellIterator<'static>,
    image_iter: ImageIterator<'static>,
    /// キーを変換するときに使い回す、変換器とキーの情報。
    key_encoder: key::Encoder<'static>,
    key_event: key::Event<'static>,
    /// アプリへマウスを報告するときに使い回す、変換器とマウスの情報。
    mouse_encoder: mouse::Encoder<'static>,
    mouse_event: mouse::Event<'static>,
    /// マウスでの選択。ダブルクリックやトリプルクリックの判定も持つ。
    selection_gesture: Gesture<'static>,
    selection_press: PressEvent<'static>,
    selection_drag: DragEvent<'static>,
    selection_release: ReleaseEvent<'static>,
    /// `frame` で下線を引く範囲（マウスを載せているリンク）。
    link_underline: Vec<LinkSpan>,
}

impl Vt {
    /// `terminfo_name` は XTGETTCAP の `TN` の問い合わせに返す名前で、子プロセスの `TERM` と揃える。
    pub fn new(size: GridSize, terminfo_name: &str) -> Result<Self, VtError> {
        let replies = Rc::new(RefCell::new(Vec::new()));
        let size_cell = Rc::new(Cell::new(size));
        let bell_rang = Rc::new(Cell::new(false));

        install_png_decoder()?;
        let mut terminal = Terminal::new(size.cols, size.rows)?;
        terminal
            .set_scrollback_max_lines(Some(SCROLLBACK_LINES))?
            .set_terminfo_name(terminfo_name)?
            .set_kitty_image_storage_limit(IMAGE_STORAGE_LIMIT)?;
        // 生成時の指定にはセルのピクセル寸法がないので、大きさを指定し直して渡す。
        terminal.resize(
            size.cols,
            size.rows,
            size.cell_width_px,
            size.cell_height_px,
        )?;

        terminal
            .on_pty_write({
                let replies = Rc::clone(&replies);
                move |_terminal, data| replies.borrow_mut().extend_from_slice(data)
            })?
            .on_size({
                let size = Rc::clone(&size_cell);
                move |_terminal| {
                    let size = size.get();
                    Some(SizeReportSize {
                        rows: size.rows,
                        columns: size.cols,
                        cell_width: size.cell_width_px,
                        cell_height: size.cell_height_px,
                    })
                }
            })?
            .on_device_attributes(|_terminal| Some(device_attributes()))?
            .on_xtversion(|_terminal| Some(XTVERSION_NAME))?
            .on_bell({
                let bell_rang = Rc::clone(&bell_rang);
                move |_terminal| bell_rang.set(true)
            })?;

        Ok(Self {
            terminal,
            output_scanner: OutputScanner::new(),
            replies,
            size: size_cell,
            last_title: Vec::new(),
            bell_rang,
            render_state: RenderState::new()?,
            row_iter: RowIterator::new()?,
            cell_iter: CellIterator::new()?,
            image_iter: ImageIterator::new()?,
            key_encoder: key::Encoder::new()?,
            key_event: key::Event::new()?,
            mouse_encoder: mouse::Encoder::new()?,
            mouse_event: mouse::Event::new()?,
            selection_gesture: Gesture::new()?,
            selection_press: PressEvent::new()?,
            selection_drag: DragEvent::new()?,
            selection_release: ReleaseEvent::new()?,
            link_underline: Vec::new(),
        })
    }

    /// 表示している範囲の `col` 桁目・`row` 行目のセルにあるリンク。OSC 8 のハイパーリンクがあれば
    /// そちらを、なければ画面の文字から見つけた URL を返す。折り返した行はつなげて探す。
    pub fn link_at(&self, col: u16, row: u16) -> Result<Option<Link>, VtError> {
        let size = self.size.get();
        if col >= size.cols || row >= size.rows {
            return Ok(None);
        }
        let Some(viewport_top) =
            self.screen_row(Point::Viewport(PointCoordinate { x: 0, y: 0 }))?
        else {
            return Ok(None);
        };
        let screen_row = viewport_top + u32::from(row);
        let cells = self.logical_line(screen_row, size.cols)?;
        let Some(index) = cells.iter().position(|cell| {
            cell.screen_row == screen_row
                && (cell.col..cell.col + u16::from(cell.width)).contains(&col)
        }) else {
            return Ok(None);
        };
        let Some((uri, range)) = link::link_in_line(&cells, index) else {
            return Ok(None);
        };
        let spans = link::spans_of(&cells, range, viewport_top, size.rows);
        Ok(Some(Link { uri, spans }))
    }

    /// 既定の背景色・文字の色・16 色を決める。256 色の残りは libghostty-vt の標準の色のまま。カーソルの
    /// 色は決めず、文字の色で描く。アプリが OSC 10 / 11 / 4 で変えた色はそのまま残り、OSC 110 / 111 /
    /// 104 で戻すとこの色になる。OSC 10 / 11 / 4 の問い合わせにも、変えていなければこの色を答える。
    pub fn set_default_colors(&mut self, colors: &TerminalColors) -> Result<(), VtError> {
        let mut palette = Palette::default();
        for (index, color) in colors.ansi.iter().enumerate() {
            palette.0[index] = rgb_color(*color);
        }
        self.terminal
            .set_default_bg_color(Some(rgb_color(colors.background)))?
            .set_default_fg_color(Some(rgb_color(colors.foreground)))?
            .set_default_color_palette(Some(palette))?;
        Ok(())
    }

    /// `frame` で下線を引く範囲を決める。空なら引かない。
    pub fn set_link_underline(&mut self, spans: Vec<LinkSpan>) {
        self.link_underline = spans;
    }

    /// `point` のセルの、スクロールバックの先頭から数えた行。
    fn screen_row(&self, point: Point) -> Result<Option<u32>, VtError> {
        let grid_ref = self.terminal.grid_ref(point)?;
        Ok(self
            .terminal
            .point_from_grid_ref(&grid_ref, PointSpace::Screen)?
            .map(|point| point.y))
    }

    /// `screen_row` の行を含む論理行（折り返しでつながった行）のセルを、先頭から並べる。
    /// たどるのは前後それぞれ [`MAX_LINK_LINE_ROWS`] 行まで。
    fn logical_line(&self, screen_row: u32, cols: u16) -> Result<Vec<LineCell>, VtError> {
        let row_at = |y: u32| {
            self.terminal
                .grid_ref(Point::Screen(PointCoordinate { x: 0, y }))
                .and_then(|grid_ref| grid_ref.row())
        };
        let mut first = screen_row;
        while first > 0
            && screen_row - first < MAX_LINK_LINE_ROWS
            && row_at(first)?.is_wrap_continuation()?
        {
            first -= 1;
        }
        let mut cells = Vec::new();
        let mut y = first;
        loop {
            self.push_row_cells(y, cols, &mut cells)?;
            let wrapped = row_at(y)?.is_wrapped()?;
            if !wrapped || y.saturating_sub(screen_row) >= MAX_LINK_LINE_ROWS {
                break;
            }
            y += 1;
            // 最後の行が折り返しの印を持っていても、その先の行はない。
            if row_at(y).is_err() {
                break;
            }
        }
        Ok(cells)
    }

    /// スクロールバックの先頭から数えて `y` 行目のセルを `cells` に足す。全角の文字の右半分と、
    /// 行末に収まらなかった全角の文字の印は除く。
    fn push_row_cells(&self, y: u32, cols: u16, cells: &mut Vec<LineCell>) -> Result<(), VtError> {
        for x in 0..cols {
            let grid_ref = self
                .terminal
                .grid_ref(Point::Screen(PointCoordinate { x, y }))?;
            let cell = grid_ref.cell()?;
            let width = match cell.wide()? {
                CellWide::Wide => 2,
                CellWide::SpacerTail | CellWide::SpacerHead => continue,
                CellWide::Narrow => 1,
            };
            let uri = if cell.has_hyperlink()? {
                hyperlink_uri(&grid_ref)?
            } else {
                None
            };
            cells.push(LineCell {
                screen_row: y,
                col: x,
                width,
                text: graphemes(&grid_ref)?,
                uri,
            });
        }
        Ok(())
    }

    /// アプリがマウスの報告を求めている（vim の `mouse=a` など）。
    pub fn is_mouse_tracking(&self) -> Result<bool, VtError> {
        self.terminal.is_mouse_tracking()
    }

    /// マウスの操作を、アプリが求めている報告の形式（X10、SGR など）でバイト列に変える。
    /// アプリが求めていない種類の操作なら空を返す。
    pub fn encode_mouse(&mut self, input: &MouseInput) -> Result<Vec<u8>, VtError> {
        let size = self.size.get();
        self.mouse_event
            .set_action(input.action)
            .set_button(input.button)
            .set_mods(input.mods)
            .set_position(mouse::Position {
                x: input.x,
                y: input.y,
            });
        let mut encoded = Vec::new();
        self.mouse_encoder
            .set_options_from_terminal(&self.terminal)
            .set_size(mouse::EncoderSize {
                screen_width: u32::from(size.cols) * size.cell_width_px,
                screen_height: u32::from(size.rows) * size.cell_height_px,
                cell_width: size.cell_width_px,
                cell_height: size.cell_height_px,
                padding_top: 0,
                padding_bottom: 0,
                padding_right: 0,
                padding_left: 0,
            })
            .set_any_button_pressed(input.any_button_pressed)
            // 同じセルの中での移動は、繰り返して報告しない。
            .set_track_last_cell(true)
            .encode_to_vec(&self.mouse_event, &mut encoded)?;
        Ok(encoded)
    }

    /// 左ボタンを押した。続けて押す間隔が `repeat_interval` 以内なら、ダブルクリック（単語）や
    /// トリプルクリック（行）として選ぶ。`time` は同じ時計で測った、押した時刻。
    pub fn selection_press(
        &mut self,
        at: PointerPosition,
        time: Duration,
        repeat_interval: Duration,
    ) -> Result<(), VtError> {
        let size = self.size.get();
        let grid_ref = self.terminal.grid_ref(viewport_point(at))?;
        let selection = self
            .selection_press
            .set_repeat_interval(repeat_interval)?
            .set_repeat_distance(f64::from(size.cell_width_px))?
            .set_time(time)?
            .set_position(at.x, at.y)?
            .apply(&mut self.selection_gesture, &self.terminal, grid_ref)?;
        self.terminal.set_selection(selection.as_ref())?;
        Ok(())
    }

    /// 左ボタンを押したまま動かした。`rectangle` なら矩形で選ぶ。
    pub fn selection_drag(&mut self, at: PointerPosition, rectangle: bool) -> Result<(), VtError> {
        let size = self.size.get();
        let geometry = Geometry {
            columns: u32::from(size.cols),
            cell_width: size.cell_width_px,
            padding_left: 0,
            screen_height: u32::from(size.rows) * size.cell_height_px,
        };
        let grid_ref = self.terminal.grid_ref(viewport_point(at))?;
        let selection = self
            .selection_drag
            .set_rectangle(rectangle)?
            .set_position(at.x, at.y)?
            .apply(
                &mut self.selection_gesture,
                &self.terminal,
                grid_ref,
                geometry,
            )?;
        self.terminal.set_selection(selection.as_ref())?;
        Ok(())
    }

    /// 左ボタンを離した。
    pub fn selection_release(&mut self, at: PointerPosition) -> Result<(), VtError> {
        let grid_ref = self.terminal.grid_ref(viewport_point(at)).ok();
        self.selection_release
            .apply(&mut self.selection_gesture, &self.terminal, grid_ref)
    }

    /// 選択範囲の文字列。折り返した行はつなげ、行末の空白は除く。選んでいなければ `None`。
    pub fn selected_text(&self) -> Result<Option<String>, VtError> {
        let options = SelectionFormatOptions::new()
            .with_emit_format(Format::Plain)
            .with_unwrap(true)
            .with_trim(true);
        let text = self.terminal.format_selection_alloc(None, options)?;
        Ok(text
            .map(|text| String::from_utf8_lossy(&text).into_owned())
            .filter(|text| !text.is_empty()))
    }

    /// `text` を貼り付ける。シェルへ送るバイト列は `take_replies` で取り出す。
    ///
    /// 改行を含む文字列は、アプリが bracketed paste に対応していなければコマンドとして実行されうる
    /// ので、`allow_unsafe` でなければ送らずに [`PasteOutcome::Unsafe`] を返す。
    pub fn paste(&mut self, text: &str, allow_unsafe: bool) -> Result<PasteOutcome, VtError> {
        let mimes = [ClipboardMime::new(PLAIN_TEXT_MIME)];
        let options = PasteOptions::new().with_allow_unsafe(allow_unsafe);
        let read = |_mime: &str, out: &mut dyn std::io::Write| out.write_all(text.as_bytes());
        match self.terminal.paste(options, &mimes, read) {
            Ok(true) => Ok(PasteOutcome::Pasted),
            Ok(false) => Ok(PasteOutcome::Empty),
            Err(VtError::Rejected) => Ok(PasteOutcome::Unsafe),
            Err(e) => Err(e),
        }
    }

    /// スクロールバックも含めて、すべてを選ぶ。
    pub fn select_all(&mut self) -> Result<(), VtError> {
        let selection = self.terminal.select_all()?;
        self.terminal.set_selection(selection.as_ref())?;
        Ok(())
    }

    pub fn clear_selection(&mut self) -> Result<(), VtError> {
        self.terminal.set_selection(None)?;
        Ok(())
    }

    /// キーを、端末のいまのモード（カーソルキーのモード、kitty keyboard protocol など）に合わせて
    /// シェルへ送るバイト列に変える。送るものがなければ空を返す。
    pub fn encode_key(&mut self, input: &KeyInput) -> Result<Vec<u8>, VtError> {
        let mods = self.mods_to_report(input.mods)?;
        self.key_event
            .set_action(input.action)
            .set_key(input.key)
            .set_mods(mods)
            .set_consumed_mods(input.consumed_mods)
            .set_unshifted_codepoint(input.unshifted)
            .set_utf8(input.text);
        let mut encoded = Vec::new();
        self.key_encoder
            .set_options_from_terminal(&self.terminal)
            .encode_to_vec(&self.key_event, &mut encoded)?;
        Ok(encoded)
    }

    /// キーの変換器に渡す修飾キー。
    ///
    /// kitty keyboard protocol では、変換器は渡されたロックの状態（Caps Lock 64、Num Lock 128）を
    /// そのまま修飾キーの数に入れる。そのため Num Lock を点けているだけで Shift+Enter が `CSI 13;130u`
    /// になり、`CSI 13;2u` と比べるアプリには Shift+Enter と分からない。ロックの状態は、アプリが
    /// 「すべてのキーをエスケープコードで報告する」を求めたときだけ渡す（仕様はロックの状態をすべての
    /// キーで得る方法としてこれを挙げている）。kitty keyboard protocol を使っていないときは、テンキーの
    /// 変換が Num Lock を見るのでそのまま渡す。
    fn mods_to_report(&self, mods: Mods) -> Result<Mods, VtError> {
        let flags = self.terminal.kitty_keyboard_flags()?;
        if flags.is_empty() || flags.contains(key::KittyKeyFlags::REPORT_ALL) {
            return Ok(mods);
        }
        Ok(mods - LOCK_MODS)
    }

    /// シェルの出力を流し込む。
    pub fn feed(&mut self, data: &[u8]) {
        let terminal = &mut self.terminal;
        let size = self.size.get();
        self.output_scanner.scan(data, &mut |event| match event {
            OutputEvent::Bytes(bytes) => terminal.vt_write(bytes),
            OutputEvent::Sixel { image, scrolling } => {
                place_sixel(terminal, size, &image, scrolling);
            }
        });
    }

    /// 前に呼んだときからタイトル（OSC 0 / OSC 2 で設定されたもの）が変わっていれば、新しいタイトルを
    /// 返す。消されたときは空の文字列。UTF-8 でないバイトは U+FFFD に置き換える。
    ///
    /// 端末のリセット（RIS）はタイトルを消すが、タイトルの変化のコールバックを呼ばない。
    /// そのためコールバックではなく、前に返したタイトルと比べて決める。
    pub fn take_title_change(&mut self) -> Result<Option<String>, VtError> {
        let title = self.terminal.title()?;
        if title == self.last_title.as_slice() {
            return Ok(None);
        }
        self.last_title = title.to_vec();
        Ok(Some(String::from_utf8_lossy(&self.last_title).into_owned()))
    }

    /// 前に呼んだときからベル（BEL）を受けていれば `true`。何度受けても一度だけ返す。
    pub fn take_bell(&mut self) -> bool {
        self.bell_rang.replace(false)
    }

    /// 溜まっているシェルへの応答を取り出す。
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut *self.replies.borrow_mut())
    }

    pub fn resize(&mut self, size: GridSize) -> Result<(), VtError> {
        self.terminal.resize(
            size.cols,
            size.rows,
            size.cell_width_px,
            size.cell_height_px,
        )?;
        self.size.set(size);
        Ok(())
    }

    pub fn size(&self) -> GridSize {
        self.size.get()
    }

    /// 表示する範囲を `lines` 行動かす。負の値で過去（上）へ、正の値で新しい側（下）へ。
    pub fn scroll(&mut self, lines: isize) {
        self.terminal.scroll_viewport(ScrollViewport::Delta(lines));
    }

    /// 表示する範囲を、いちばん新しい行（カーソルのある画面）に戻す。
    pub fn scroll_to_bottom(&mut self) {
        self.terminal.scroll_viewport(ScrollViewport::Bottom);
    }

    /// 表示する範囲を、スクロールバックのいちばん古い行からにする。
    pub fn scroll_to_top(&mut self) {
        self.terminal.scroll_viewport(ScrollViewport::Top);
    }

    /// 表示する範囲の先頭を、全体の先頭から `row` 行目（[`ScrollPosition::offset`] と同じ数え方）に
    /// する。範囲を越える値は、いちばん新しい行までに収まる。
    pub fn scroll_to_row(&mut self, row: usize) {
        self.terminal.scroll_viewport(ScrollViewport::Row(row));
    }

    /// 表示している範囲の、スクロールバックを含む全体の中での位置。代替画面にはスクロールバックが
    /// ないので、全体が表示している範囲と同じになる。
    pub fn scroll_position(&self) -> Result<ScrollPosition, VtError> {
        let scrollbar = self.terminal.scrollbar()?;
        let to_usize = |value: u64| usize::try_from(value).unwrap_or(usize::MAX);
        Ok(ScrollPosition {
            total: to_usize(scrollbar.total),
            offset: to_usize(scrollbar.offset),
            len: to_usize(scrollbar.len),
        })
    }

    /// カーソルのある桁と行（0 始まり）。
    pub fn cursor_cell(&self) -> Result<(u16, u16), VtError> {
        Ok((self.terminal.cursor_x()?, self.terminal.cursor_y()?))
    }

    /// 描画のために画面の内容を取り出す。反転表示はここで前景色と背景色を入れ替え、薄くした文字は
    /// 前景色を背景色に寄せ、不可視の文字は文字と装飾を除いておく。点滅は印を付けて渡すだけで、
    /// 隠すのは描く側（[`Frame::hide_blinking_text`]）。[`Vt::set_link_underline`] で
    /// 決めた範囲には下線を付ける。
    pub fn frame(&mut self) -> Result<Frame, VtError> {
        let snapshot = self.render_state.update(&self.terminal)?;
        let colors = snapshot.colors()?;
        let mut cells = Vec::new();
        let mut text = String::new();

        let mut rows = self.row_iter.update(&snapshot)?;
        let mut row_index = 0u16;
        while let Some(row) = rows.next() {
            let mut row_cells = self.cell_iter.update(row)?;
            let mut col = 0u16;
            while let Some(cell) = row_cells.next() {
                let width = match cell.raw_cell()?.wide()? {
                    CellWide::Wide => 2,
                    CellWide::SpacerTail | CellWide::SpacerHead => {
                        col += 1;
                        continue;
                    }
                    CellWide::Narrow => 1,
                };
                cell.graphemes_utf8(&mut text)?;
                if text.starts_with(KITTY_PLACEHOLDER) {
                    text.clear();
                }
                let mut fg = cell.fg_color()?.unwrap_or(colors.foreground);
                let mut bg = cell.bg_color()?;
                let style = if cell.has_styling()? {
                    cell.style()?
                } else {
                    Style::default()
                };
                if style.inverse {
                    let swapped_bg = fg;
                    fg = bg.unwrap_or(colors.background);
                    bg = Some(swapped_bg);
                }
                // 選択範囲は、前景色と背景色を入れ替えて描く。
                if cell.is_selected()? {
                    let swapped_bg = fg;
                    fg = bg.unwrap_or(colors.background);
                    bg = Some(swapped_bg);
                }
                if style.faint {
                    fg = blend(fg, bg.unwrap_or(colors.background), FAINT_OPACITY);
                }
                let mut view = CellView {
                    col,
                    row: row_index,
                    text: text.clone(),
                    width,
                    fg: rgb(fg),
                    bg: bg.map(rgb),
                    bold: style.bold,
                    italic: style.italic,
                    underline: underline(style.underline),
                    underline_color: style_color(style.underline_color, &colors.palette),
                    strikethrough: style.strikethrough,
                    overline: style.overline,
                    blink: style.blink,
                };
                // 不可視の文字は、背景だけを描く。
                if style.invisible {
                    view.clear_text_and_decoration();
                    view.blink = false;
                }
                // マウスを載せているリンクには下線を引く。下線を持つ文字はその下線のまま描く。
                if view.underline == Underline::None
                    && self
                        .link_underline
                        .iter()
                        .any(|span| span.row == row_index && span.cols.contains(&col))
                {
                    view.underline = Underline::Single;
                }
                if !view.text.is_empty() || view.bg.is_some() || view.has_decoration() {
                    cells.push(view);
                }
                col += 1;
            }
            row_index += 1;
        }

        let cursor = match snapshot.cursor_viewport()? {
            Some(viewport) if snapshot.cursor_visible()? => {
                // 全角の文字の右半分にいるときは、その文字の左端に描く。
                let col = viewport.x.saturating_sub(u16::from(viewport.at_wide_tail));
                let width = cells
                    .iter()
                    .find(|cell| cell.col == col && cell.row == viewport.y)
                    .map_or(1, |cell| cell.width);
                Some(CursorView {
                    col,
                    row: viewport.y,
                    width,
                    color: rgb(colors.cursor.unwrap_or(colors.foreground)),
                })
            }
            _ => None,
        };
        let images = image_views(&mut self.image_iter, &snapshot)?;

        Ok(Frame {
            cols: snapshot.cols()?,
            rows: snapshot.rows()?,
            background: rgb(colors.background),
            foreground: rgb(colors.foreground),
            cells,
            cursor,
            images,
        })
    }

    /// `key` の画像のピクセル。保管にその画像がないか、`key` のあとでピクセルが変わった、または
    /// まだ届いていなければ `None`。
    pub fn image_pixels(&self, key: &ImageKey) -> Result<Option<ImagePixels<'_>>, VtError> {
        let graphics = self.terminal.kitty_graphics()?;
        let Some(image) = graphics.image(key.id) else {
            return Ok(None);
        };
        if image.generation()? != key.generation {
            return Ok(None);
        }
        let Some(data) = image.data()? else {
            return Ok(None);
        };
        Ok(rgba_pixels(
            image.format()?,
            data,
            image.width()?,
            image.height()?,
        ))
    }

    /// `key` の画像が、同じピクセルのまま表示している画面の保管にある。
    pub fn image_is_stored(&self, key: &ImageKey) -> Result<bool, VtError> {
        let graphics = self.terminal.kitty_graphics()?;
        let Some(image) = graphics.image(key.id) else {
            return Ok(false);
        };
        Ok(image.generation()? == key.generation)
    }

    /// 再生中の画像のアニメーションを進める。描き直す前に呼ぶ。`now` は同じ時計で測った時刻。次のコマ
    /// までの時間を返す。再生中のものがなければ `None`。
    pub fn tick_image_animations(&mut self, now: Duration) -> Result<Option<Duration>, VtError> {
        let now_ms = u64::try_from(now.as_millis()).unwrap_or(u64::MAX);
        let next_ms = self.terminal.kitty_graphics_animation_tick(now_ms)?;
        Ok(next_ms.map(|next_ms| Duration::from_millis(next_ms.saturating_sub(now_ms))))
    }

    /// 画面の内容をプレーンテキストで返す。行末の空白は除く。
    pub fn plain_text(&self) -> Result<String, VtError> {
        let options = FormatterOptions::new()
            .with_format(Format::Plain)
            .with_trim(true);
        let text = Formatter::new(&self.terminal, options)?.format_alloc(None)?;
        Ok(String::from_utf8_lossy(&text).into_owned())
    }
}

/// sixel の画像を kitty graphics protocol の転送にして端末に置く。位置の追跡、スクロール、消去、代替画面は
/// kitty graphics protocol の画像と同じになる。
///
/// 画像は文字より下に置き、あとから書いた文字が画像の上に見えるようにする。不透明な画像なら、覆うセルの
/// 文字を消して画像を見せる。
///
/// `scrolling` なら、カーソルの位置に置き、カーソルを画像の最後の行の同じ桁へ進める（xterm と同じ）。下端で
/// は画面がスクロールする。そうでなければ（DECSDM）、画面の左上に置いてカーソルを動かさない。
fn place_sixel(
    terminal: &mut Terminal<'static, 'static>,
    size: GridSize,
    image: &SixelImage,
    scrolling: bool,
) {
    let rows = image.height.div_ceil(size.cell_height_px.max(1));
    let cols = image.width.div_ceil(size.cell_width_px.max(1));
    let erase = image
        .is_opaque()
        .then(|| format!("\x1b[{cols}X").into_bytes());
    let erase = erase.as_deref().unwrap_or_default();

    if scrolling {
        image.write_kitty_transmission(&mut |apc| terminal.vt_write(apc));
        terminal.vt_write(erase);
        for _ in 1..rows {
            terminal.vt_write(INDEX);
            terminal.vt_write(erase);
        }
        return;
    }

    let cursor = terminal
        .cursor_x()
        .and_then(|col| Ok((col, terminal.cursor_y()?)));
    terminal.vt_write(b"\x1b[H");
    image.write_kitty_transmission(&mut |apc| terminal.vt_write(apc));
    for row in 0..rows.min(u32::from(size.rows)) {
        terminal.vt_write(format!("\x1b[{};1H", row + 1).as_bytes());
        terminal.vt_write(erase);
    }
    match cursor {
        Ok((col, row)) => terminal.vt_write(format!("\x1b[{};{}H", row + 1, col + 1).as_bytes()),
        Err(e) => tracing::warn!("failed to read the cursor before placing a sixel: {e}"),
    }
}

/// `snapshot` の表示している範囲の画像を、層ごとに下から描く順に並べる。同じ高さなら画像の番号の順
/// （Ghostty と同じ）。
fn image_views(
    images: &mut ImageIterator<'static>,
    snapshot: &Snapshot<'static, '_>,
) -> Result<Vec<ImageView>, VtError> {
    let mut views = Vec::new();
    let mut placements = images.update(snapshot)?;
    while let Some(placement) = placements.next() {
        // 新しいバージョンで増えた種類は、エラーとして返ることもある。読めるものだけを描く。
        let Ok(ImageProtocol::Kitty) = placement.protocol() else {
            continue;
        };
        views.push(ImageView {
            key: ImageKey {
                source: ImageSource::Kitty,
                id: placement.image_id()?,
                generation: placement.image_generation()?,
            },
            layer: match placement.layer()? {
                VtImageLayer::BelowBackground => ImageLayer::BelowBackground,
                VtImageLayer::BelowText => ImageLayer::BelowText,
                VtImageLayer::AboveText => ImageLayer::AboveText,
            },
            z: placement.z()?,
            col: placement.viewport_col()?,
            row: placement.viewport_row()?,
            offset_x: placement.cell_offset_x()?,
            offset_y: placement.cell_offset_y()?,
            width: placement.pixel_width()?,
            height: placement.pixel_height()?,
            source_x: placement.source_x()?,
            source_y: placement.source_y()?,
            source_width: placement.source_width()?,
            source_height: placement.source_height()?,
        });
    }
    views.sort_by_key(|view| (view.layer, view.z, view.key.id));
    Ok(views)
}

/// 大きさ `width` × `height` の `format` のピクセルを RGBA にする。バイト数が足りない、または
/// RGBA にできない形式なら `None`。
fn rgba_pixels(
    format: ImageFormat,
    data: &[u8],
    width: u32,
    height: u32,
) -> Option<ImagePixels<'_>> {
    let pixels = (width as usize).checked_mul(height as usize)?;
    let channels = match format {
        ImageFormat::Gray => 1,
        ImageFormat::GrayAlpha => 2,
        ImageFormat::Rgb => 3,
        ImageFormat::Rgba => 4,
        _ => return None,
    };
    let data = data.get(..pixels.checked_mul(channels)?)?;
    let rgba = match format {
        ImageFormat::Rgba => Cow::Borrowed(data),
        _ => Cow::Owned(
            data.chunks_exact(channels)
                .flat_map(|pixel| match *pixel {
                    [gray] => [gray, gray, gray, u8::MAX],
                    [gray, alpha] => [gray, gray, gray, alpha],
                    [r, g, b] => [r, g, b, u8::MAX],
                    _ => unreachable!("channels is 1, 2 or 3 here"),
                })
                .collect(),
        ),
    };
    Some(ImagePixels {
        width,
        height,
        rgba,
    })
}

/// `grid_ref` のセルの文字（書記素クラスタ）。空のセルは空の文字列。
fn graphemes(grid_ref: &GridRef) -> Result<String, VtError> {
    let mut buf = vec!['\0'; GRAPHEME_BUFFER_LEN];
    let len = match grid_ref.graphemes(&mut buf) {
        Err(VtError::OutOfSpace { required }) => {
            buf.resize(required, '\0');
            grid_ref.graphemes(&mut buf)?
        }
        result => result?,
    };
    Ok(buf[..len].iter().collect())
}

/// `grid_ref` のセルの、OSC 8 のハイパーリンクの URI。なければ `None`。UTF-8 でないバイトは
/// U+FFFD に置き換える。
fn hyperlink_uri(grid_ref: &GridRef) -> Result<Option<String>, VtError> {
    let mut buf = vec![0u8; URI_BUFFER_LEN];
    let len = match grid_ref.hyperlink_uri(&mut buf) {
        Err(VtError::OutOfSpace { required }) => {
            buf.resize(required, 0);
            grid_ref.hyperlink_uri(&mut buf)?
        }
        result => result?,
    };
    Ok((len > 0).then(|| String::from_utf8_lossy(&buf[..len]).into_owned()))
}

fn viewport_point(at: PointerPosition) -> Point {
    Point::Viewport(PointCoordinate {
        x: at.col,
        y: u32::from(at.row),
    })
}

fn rgb(color: RgbColor) -> Rgb {
    Rgb {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

fn rgb_color(color: Rgb) -> RgbColor {
    RgbColor {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

/// `color` を `opacity` の割合で `background` に重ねた色。
fn blend(color: RgbColor, background: RgbColor, opacity: f32) -> RgbColor {
    let mix = |front: u8, back: u8| {
        (f32::from(front) * opacity + f32::from(back) * (1.0 - opacity)).round() as u8
    };
    RgbColor {
        r: mix(color.r, background.r),
        g: mix(color.g, background.g),
        b: mix(color.b, background.b),
    }
}

/// スタイルの色を、パレットの番号ならパレットで引いて RGB にする。指定がなければ `None`。
fn style_color(color: StyleColor, palette: &[RgbColor; 256]) -> Option<Rgb> {
    match color {
        StyleColor::None => None,
        StyleColor::Palette(PaletteIndex(index)) => Some(rgb(palette[usize::from(index)])),
        StyleColor::Rgb(color) => Some(rgb(color)),
    }
}

fn underline(underline: StyleUnderline) -> Underline {
    match underline {
        StyleUnderline::None => Underline::None,
        StyleUnderline::Single => Underline::Single,
        StyleUnderline::Double => Underline::Double,
        StyleUnderline::Curly => Underline::Curly,
        StyleUnderline::Dotted => Underline::Dotted,
        StyleUnderline::Dashed => Underline::Dashed,
        // ここにない種類は一重として描く。
        _ => Underline::Single,
    }
}

/// DA1〜DA3 の問い合わせへの答え。VT220 相当として名乗る。
fn device_attributes() -> DeviceAttributes {
    DeviceAttributes {
        primary: PrimaryDeviceAttributes::new(
            ConformanceLevel::VT220,
            &[
                DeviceAttributeFeature::COLUMNS_132,
                DeviceAttributeFeature::SELECTIVE_ERASE,
                DeviceAttributeFeature::ANSI_COLOR,
            ],
        ),
        secondary: SecondaryDeviceAttributes {
            device_type: DeviceType::VT220,
            firmware_version: 1,
            rom_cartridge: 0,
        },
        tertiary: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: GridSize = GridSize {
        cols: 20,
        rows: 5,
        cell_width_px: 8,
        cell_height_px: 16,
    };

    #[test]
    fn graphemes_are_measured_like_the_terminal() {
        let widths = |text: &str| -> Vec<(String, u8)> {
            split_graphemes(text)
                .into_iter()
                .map(|grapheme| (grapheme.text, grapheme.width))
                .collect()
        };
        assert_eq!(
            widths("a端"),
            vec![("a".to_owned(), 1), ("端".to_owned(), 2)]
        );
        // 曖昧幅は 1。
        assert_eq!(widths("○"), vec![("○".to_owned(), 1)]);
        // 結合文字は前の文字と一つにまとまる。
        assert_eq!(widths("e\u{301}"), vec![("e\u{301}".to_owned(), 1)]);
        // 絵文字の ZWJ の並びは一つで 2 セル。
        assert_eq!(widths("👨\u{200d}👩"), vec![("👨\u{200d}👩".to_owned(), 2)]);
        assert_eq!(widths(""), vec![]);
    }

    fn new_vt() -> Vt {
        Vt::new(SIZE, "xterm-256color").unwrap()
    }

    /// 先頭のセルの文字の色。
    fn first_fg(vt: &mut Vt) -> Rgb {
        vt.frame().unwrap().cells[0].fg
    }

    #[test]
    fn dark_scheme_is_the_default_of_the_terminal() {
        use crate::config::{ANSI_COLOR_COUNT, DARK_COLORS};
        let mut vt = new_vt();
        let frame = vt.frame().unwrap();
        assert_eq!(frame.background, DARK_COLORS.background);
        assert_eq!(frame.foreground, DARK_COLORS.foreground);
        let palette = Palette::default();
        for index in 0..ANSI_COLOR_COUNT {
            assert_eq!(rgb(palette.0[index]), DARK_COLORS.ansi[index], "{index}");
        }
    }

    #[test]
    fn default_colors_replace_the_background_the_text_and_16_colors() {
        use crate::config::LIGHT_COLORS;
        let mut vt = new_vt();
        vt.set_default_colors(&LIGHT_COLORS).unwrap();
        vt.feed(b"a\x1b[31mb\x1b[93mc\x1b[38;5;16md\x1b[38;5;196me\x1b[38;5;255mf\x1b[0m\x1b[44m \x1b[7mg");
        let frame = vt.frame().unwrap();
        assert_eq!(frame.background, LIGHT_COLORS.background);
        assert_eq!(frame.foreground, LIGHT_COLORS.foreground);
        let fg = |col: u16| frame.cells.iter().find(|cell| cell.col == col).unwrap().fg;
        let bg = |col: u16| frame.cells.iter().find(|cell| cell.col == col).unwrap().bg;
        assert_eq!(fg(0), LIGHT_COLORS.foreground);
        assert_eq!(fg(1), LIGHT_COLORS.ansi[1]);
        assert_eq!(fg(2), LIGHT_COLORS.ansi[11]);
        // 256 色の残りは標準の色のまま。
        let standard = Palette::default();
        assert_eq!(fg(3), rgb(standard.0[16]));
        assert_eq!(fg(4), rgb(standard.0[196]));
        assert_eq!(fg(5), rgb(standard.0[255]));
        assert_eq!(bg(6), Some(LIGHT_COLORS.ansi[4]));
        // 反転（と選択範囲）は、配色の文字の色と背景色を入れ替える。
        assert_eq!(fg(7), LIGHT_COLORS.ansi[4]);
        let cursor = frame.cursor.unwrap();
        // カーソルは文字の色。
        assert_eq!(cursor.color, LIGHT_COLORS.foreground);
    }

    #[test]
    fn colors_set_by_the_application_outlive_the_default_colors() {
        use crate::config::{DARK_COLORS, LIGHT_COLORS};
        let mut vt = new_vt();
        vt.set_default_colors(&DARK_COLORS).unwrap();
        // OSC 11 で背景色、OSC 4 で赤を変える。
        vt.feed(b"\x1b]11;rgb:12/34/56\x1b\\\x1b]4;1;rgb:01/02/03\x1b\\\x1b[31mr\x1b[32mg");
        vt.set_default_colors(&LIGHT_COLORS).unwrap();
        let frame = vt.frame().unwrap();
        let changed = Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        };
        assert_eq!(frame.background, changed);
        assert_eq!(frame.foreground, LIGHT_COLORS.foreground);
        assert_eq!(first_fg(&mut vt), Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(vt.frame().unwrap().cells[1].fg, LIGHT_COLORS.ansi[2]);
        // OSC 111 と OSC 104 で戻すと、既定の色になる。
        vt.feed(b"\x1b]111\x1b\\\x1b]104\x1b\\");
        let frame = vt.frame().unwrap();
        assert_eq!(frame.background, LIGHT_COLORS.background);
        assert_eq!(frame.cells[0].fg, LIGHT_COLORS.ansi[1]);
    }

    #[test]
    fn color_queries_answer_the_default_colors() {
        use crate::config::LIGHT_COLORS;
        let mut vt = new_vt();
        vt.set_default_colors(&LIGHT_COLORS).unwrap();
        vt.take_replies();
        vt.feed(b"\x1b]11;?\x1b\\\x1b]10;?\x1b\\\x1b]4;3;?\x1b\\");
        let replies = String::from_utf8(vt.take_replies()).unwrap();
        assert_eq!(
            replies,
            "\x1b]11;rgb:ffff/ffff/ffff\x1b\\\x1b]10;rgb:3838/3a3a/4242\x1b\\\x1b]4;3;rgb:8a8a/6464/0000\x1b\\"
        );
    }

    #[test]
    fn feed_shows_text_on_screen() {
        let mut vt = new_vt();
        vt.feed("hello\r\n日本語 ○".as_bytes());
        assert_eq!(vt.plain_text().unwrap(), "hello\n日本語 ○");
    }

    #[test]
    fn long_line_wraps_at_cols() {
        let mut vt = new_vt();
        vt.feed(b"abcdefghijklmnopqrstuvwxyz");
        assert_eq!(vt.plain_text().unwrap(), "abcdefghijklmnopqrst\nuvwxyz");
    }

    #[test]
    fn cursor_position_report_is_queued_as_reply() {
        let mut vt = new_vt();
        vt.feed(b"ab\x1b[6n");
        assert_eq!(vt.take_replies(), b"\x1b[1;3R");
        assert!(vt.take_replies().is_empty());
    }

    #[test]
    fn osc_0_and_2_set_the_title() {
        let mut vt = new_vt();
        assert_eq!(vt.take_title_change().unwrap(), None);
        vt.feed("\x1b]0;日本語のタイトル\x07".as_bytes());
        assert_eq!(
            vt.take_title_change().unwrap().as_deref(),
            Some("日本語のタイトル")
        );
        // 一度取り出したら、次に設定されるまでは返さない。
        assert_eq!(vt.take_title_change().unwrap(), None);
        vt.feed(b"\x1b]2;vim\x1b\\");
        assert_eq!(vt.take_title_change().unwrap().as_deref(), Some("vim"));
    }

    #[test]
    fn only_the_last_title_is_returned() {
        let mut vt = new_vt();
        vt.feed(b"\x1b]2;first\x07\x1b]2;second\x07");
        assert_eq!(vt.take_title_change().unwrap().as_deref(), Some("second"));
    }

    #[test]
    fn empty_title_is_returned_as_cleared() {
        let mut vt = new_vt();
        vt.feed(b"\x1b]2;vim\x07");
        vt.take_title_change().unwrap();
        vt.feed(b"\x1b]2;\x07");
        assert_eq!(vt.take_title_change().unwrap().as_deref(), Some(""));
    }

    #[test]
    fn title_with_invalid_utf8_is_ignored() {
        let mut vt = new_vt();
        vt.feed(b"\x1b]2;vim\x07");
        vt.take_title_change().unwrap();
        vt.feed(b"\x1b]2;caf\xe9\x07");
        assert_eq!(vt.take_title_change().unwrap(), None);
    }

    #[test]
    fn reset_clears_the_title() {
        let mut vt = new_vt();
        vt.feed(b"\x1b]2;vim\x07");
        vt.take_title_change().unwrap();
        vt.feed(b"\x1bc");
        assert_eq!(vt.take_title_change().unwrap().as_deref(), Some(""));
    }

    #[test]
    fn setting_the_same_title_again_is_not_a_change() {
        let mut vt = new_vt();
        vt.feed(b"\x1b]2;vim\x07");
        vt.take_title_change().unwrap();
        vt.feed(b"\x1b]2;vim\x07");
        assert_eq!(vt.take_title_change().unwrap(), None);
    }

    #[test]
    fn bel_rings_the_bell_once() {
        let mut vt = new_vt();
        assert!(!vt.take_bell());
        vt.feed(b"a\x07b\x07");
        assert!(vt.take_bell());
        assert!(!vt.take_bell());
        // BEL は画面に何も出さない。
        assert_eq!(vt.plain_text().unwrap(), "ab");
    }

    #[test]
    fn bel_ending_an_osc_is_not_a_bell() {
        let mut vt = new_vt();
        vt.feed(b"\x1b]2;vim\x07");
        assert!(!vt.take_bell());
    }

    #[test]
    fn frame_lists_text_cells_with_widths() {
        let mut vt = new_vt();
        vt.feed("aあ○".as_bytes());
        let frame = vt.frame().unwrap();
        let cells: Vec<(u16, &str, u8)> = frame
            .cells
            .iter()
            .map(|cell| (cell.col, cell.text.as_str(), cell.width))
            .collect();
        // 曖昧幅の ○ は 1 セル。あ の右半分は並べない。
        assert_eq!(cells, [(0, "a", 1), (1, "あ", 2), (3, "○", 1)]);
        assert_eq!((frame.cols, frame.rows), (SIZE.cols, SIZE.rows));
    }

    #[test]
    fn frame_resolves_colors_and_inverse() {
        let mut vt = new_vt();
        // 前景を 24 ビットの赤、次の文字は反転。
        vt.feed(b"\x1b[38;2;255;0;0mr\x1b[0m\x1b[7mi\x1b[0m");
        let frame = vt.frame().unwrap();
        let red = Rgb { r: 255, g: 0, b: 0 };
        assert_eq!(frame.cells[0].fg, red);
        assert_eq!(frame.cells[0].bg, None);
        assert_eq!(frame.cells[1].fg, frame.background);
        assert_eq!(frame.cells[1].bg, Some(frame.foreground));
    }

    /// `sgr` の SGR を付けた `text` を書き、最初のセルを返す。
    fn styled_cell(sgr: &str, text: &str) -> CellView {
        let mut vt = new_vt();
        vt.feed(format!("\x1b[{sgr}m{text}\x1b[0m").as_bytes());
        vt.frame().unwrap().cells.remove(0)
    }

    #[test]
    fn frame_passes_underline_styles() {
        for (sgr, expected) in [
            ("4", Underline::Single),
            ("4:1", Underline::Single),
            ("21", Underline::Double),
            ("4:2", Underline::Double),
            ("4:3", Underline::Curly),
            ("4:4", Underline::Dotted),
            ("4:5", Underline::Dashed),
        ] {
            assert_eq!(styled_cell(sgr, "a").underline, expected, "SGR {sgr}");
        }
        assert_eq!(styled_cell("4;24", "a").underline, Underline::None);
        assert_eq!(styled_cell("4:3;4:0", "a").underline, Underline::None);
    }

    #[test]
    fn frame_keeps_decorated_blanks() {
        let mut vt = new_vt();
        // 下線を付けた空白は、文字がなくても下線を描くために並べる。
        vt.feed(b"a\x1b[4m  \x1b[0m b");
        let frame = vt.frame().unwrap();
        let underlined: Vec<(u16, Underline)> = frame
            .cells
            .iter()
            .map(|cell| (cell.col, cell.underline))
            .collect();
        assert_eq!(
            underlined,
            [
                (0, Underline::None),
                (1, Underline::Single),
                (2, Underline::Single),
                (3, Underline::None),
                (4, Underline::None),
            ]
        );
    }

    #[test]
    fn frame_resolves_the_underline_color() {
        let mut vt = new_vt();
        // 指定がなければ None（文字と同じ色）。
        assert_eq!(styled_cell("4", "a").underline_color, None);
        assert_eq!(
            styled_cell("4;58:2::10:20:30", "a").underline_color,
            Some(Rgb {
                r: 10,
                g: 20,
                b: 30
            })
        );
        assert_eq!(
            styled_cell("4;58;2;10;20;30", "a").underline_color,
            Some(Rgb {
                r: 10,
                g: 20,
                b: 30
            })
        );
        // パレットの番号はパレットで引く。
        vt.feed(b"\x1b[4;58;5;196ma");
        let palette_red = vt.frame().unwrap().cells[0].underline_color;
        vt.feed(b"\x1b[0m\r\n\x1b[38;5;196mb");
        let frame = vt.frame().unwrap();
        assert_eq!(palette_red, Some(frame.cells[1].fg));
        assert_eq!(styled_cell("4;58;2;10;20;30;59", "a").underline_color, None);
    }

    #[test]
    fn frame_passes_strikethrough_and_overline() {
        let cell = styled_cell("9", "a");
        assert!(cell.strikethrough && !cell.overline);
        let cell = styled_cell("53", "a");
        assert!(cell.overline && !cell.strikethrough);
        assert!(!styled_cell("9;29", "a").strikethrough);
        assert!(!styled_cell("53;55", "a").overline);
    }

    #[test]
    fn frame_passes_bold_and_italic() {
        let cell = styled_cell("1;3", "a");
        assert!(cell.bold && cell.italic);
        let cell = styled_cell("1;3;22;23", "a");
        assert!(!cell.bold && !cell.italic);
    }

    #[test]
    fn faint_text_is_blended_into_the_background() {
        let mut vt = new_vt();
        vt.feed(b"\x1b[2;38;2;200;100;0;48;2;0;0;100ma\x1b[0m\x1b[2mb");
        let frame = vt.frame().unwrap();
        assert_eq!(
            frame.cells[0].fg,
            Rgb {
                r: 100,
                g: 50,
                b: 50
            }
        );
        // 背景色がなければ既定の背景色に寄せる。
        let expected =
            |front: u8, back: u8| ((u16::from(front) + u16::from(back)) as f32 / 2.0).round() as u8;
        let (fg, bg) = (frame.foreground, frame.background);
        assert_eq!(
            frame.cells[1].fg,
            Rgb {
                r: expected(fg.r, bg.r),
                g: expected(fg.g, bg.g),
                b: expected(fg.b, bg.b)
            }
        );
    }

    #[test]
    fn frame_passes_blink() {
        // 速い点滅（SGR 6）も同じ点滅として渡る。
        assert!(styled_cell("5", "a").blink);
        assert!(styled_cell("6", "a").blink);
        assert!(!styled_cell("5;25", "a").blink);
        assert!(!styled_cell("1", "a").blink);
        // 不可視の文字は点滅させない。
        assert!(!styled_cell("5;8;48;2;1;2;3", "a").blink);
    }

    #[test]
    fn invisible_text_keeps_only_the_background() {
        let cell = styled_cell("8;4;9;53;48;2;1;2;3", "a");
        assert_eq!(cell.text, "");
        assert_eq!(cell.bg, Some(Rgb { r: 1, g: 2, b: 3 }));
        assert!(!cell.has_decoration());
        // 背景色も装飾もない不可視の文字は並べない。
        let mut vt = new_vt();
        vt.feed(b"\x1b[8ma");
        assert!(vt.frame().unwrap().cells.is_empty());
    }

    fn span(row: u16, cols: std::ops::Range<u16>) -> LinkSpan {
        LinkSpan { row, cols }
    }

    #[test]
    fn osc_8_hyperlinks_are_found_with_their_cells() {
        let mut vt = new_vt();
        vt.feed(b"go \x1b]8;;https://example.com/a\x1b\\click here\x1b]8;;\x1b\\ end");
        let link = vt.link_at(9, 0).unwrap().unwrap();
        assert_eq!(link.uri, "https://example.com/a");
        // 空白のセルも含めて、同じリンクのセルをまとめる。
        assert_eq!(link.spans, [span(0, 3..13)]);
        assert_eq!(vt.link_at(2, 0).unwrap(), None);
        assert_eq!(vt.link_at(14, 0).unwrap(), None);
        // 長い URI も読める。
        let long = format!("https://example.com/{}", "a".repeat(URI_BUFFER_LEN * 2));
        vt.feed(format!("\r\n\x1b]8;id=x;{long}\x07link\x1b]8;;\x07").as_bytes());
        assert_eq!(vt.link_at(0, 1).unwrap().unwrap().uri, long);
    }

    #[test]
    fn urls_in_text_are_found() {
        let mut vt = new_vt();
        vt.feed("見 https://a.jp/ だ".as_bytes());
        // 全角の文字は 2 セル。URL は 3 桁目から。
        let link = vt.link_at(8, 0).unwrap().unwrap();
        assert_eq!(link.uri, "https://a.jp/");
        assert_eq!(link.spans, [span(0, 3..16)]);
        assert_eq!(vt.link_at(2, 0).unwrap(), None);
        assert_eq!(vt.link_at(17, 0).unwrap(), None);
        // 画面の外は何もない。
        assert_eq!(vt.link_at(SIZE.cols, 0).unwrap(), None);
    }

    #[test]
    fn wrapped_urls_are_joined() {
        let mut vt = new_vt();
        // 20 桁で折り返す。
        vt.feed(b"see https://example.com/abcdefghijklmnop.\r\nnext https://x.example");
        let expected = "https://example.com/abcdefghijklmnop";
        for (col, row) in [(4, 0), (19, 0), (0, 1), (19, 1)] {
            let link = vt.link_at(col, row).unwrap().unwrap();
            assert_eq!(link.uri, expected, "({col}, {row})");
            assert_eq!(link.spans, [span(0, 4..20), span(1, 0..20)]);
        }
        // 終わりの「.」は 3 行目に折り返している。
        assert_eq!(vt.link_at(0, 2).unwrap(), None);
        // 改行で終わった行の次の行は別の行。
        let link = vt.link_at(6, 3).unwrap().unwrap();
        assert_eq!(link.uri, "https://x.example");
    }

    #[test]
    fn wrapped_urls_reach_into_the_scrollback() {
        let mut vt = new_vt();
        vt.feed(b"https://example.com/abcdefghijklmnopqrstuvwxyz");
        // URL は 3 行にわたる。5 行の改行で、表示している範囲の先頭は 4 行目になる。
        for _ in 0..SIZE.rows {
            vt.feed(b"\r\n");
        }
        vt.scroll(-2);
        // URL の 2 行目が表示している範囲の先頭にあり、1 行目は範囲の外。
        let link = vt.link_at(0, 0).unwrap().unwrap();
        assert_eq!(link.uri, "https://example.com/abcdefghijklmnopqrstuvwxyz");
        assert_eq!(link.spans, [span(0, 0..20), span(1, 0..6)]);
    }

    #[test]
    fn the_link_underline_is_drawn_on_its_cells() {
        let mut vt = new_vt();
        vt.feed(b"\x1b]8;;https://example.com\x1b\\a b\x1b]8;;\x1b\\ \x1b[4:3mc\x1b[0m");
        vt.set_link_underline(vec![span(0, 0..3), span(0, 4..5)]);
        let underlines: Vec<(u16, Underline)> = vt
            .frame()
            .unwrap()
            .cells
            .iter()
            .map(|cell| (cell.col, cell.underline))
            .collect();
        // リンクの中の空白にも引く。下線を持つ文字はその下線のまま。
        assert_eq!(
            underlines,
            [
                (0, Underline::Single),
                (1, Underline::Single),
                (2, Underline::Single),
                (3, Underline::None),
                (4, Underline::Curly),
            ]
        );
        vt.set_link_underline(Vec::new());
        assert!(
            vt.frame()
                .unwrap()
                .cells
                .iter()
                .all(|cell| cell.col == 4 || cell.underline == Underline::None)
        );
    }

    fn first_row_text(vt: &mut Vt) -> String {
        let frame = vt.frame().unwrap();
        frame
            .cells
            .iter()
            .filter(|cell| cell.row == 0)
            .map(|cell| cell.text.as_str())
            .collect()
    }

    #[test]
    fn scrolling_shows_older_lines_and_returns_to_the_bottom() {
        let mut vt = new_vt();
        for line in 0..20 {
            vt.feed(format!("line{line:02}\r\n").as_bytes());
        }
        // 5 行の画面の先頭は line16（最後の行は空のプロンプトの行）。
        assert_eq!(first_row_text(&mut vt), "line16");
        vt.scroll(-10);
        assert_eq!(first_row_text(&mut vt), "line06");
        // 過去を表示している間は、カーソルは画面の外。
        assert!(vt.frame().unwrap().cursor.is_none());
        vt.scroll_to_bottom();
        assert_eq!(first_row_text(&mut vt), "line16");
    }

    fn position(total: usize, offset: usize, len: usize) -> ScrollPosition {
        ScrollPosition { total, offset, len }
    }

    #[test]
    fn the_scroll_position_counts_rows_from_the_oldest_line() {
        let mut vt = new_vt();
        // スクロールバックがなければ、全体は画面だけ。
        assert_eq!(vt.scroll_position().unwrap(), position(5, 0, 5));
        for line in 0..20 {
            vt.feed(format!("line{line:02}\r\n").as_bytes());
        }
        // 21 行（最後は空のプロンプトの行）のうち、いちばん新しい 5 行を表示している。
        assert_eq!(vt.scroll_position().unwrap(), position(21, 16, 5));
        vt.scroll(-10);
        assert_eq!(vt.scroll_position().unwrap(), position(21, 6, 5));
        vt.scroll_to_top();
        assert_eq!(vt.scroll_position().unwrap(), position(21, 0, 5));
        assert_eq!(first_row_text(&mut vt), "line00");
        vt.scroll_to_bottom();
        assert_eq!(vt.scroll_position().unwrap(), position(21, 16, 5));
    }

    #[test]
    fn scrolling_to_a_row_uses_the_same_count_as_the_offset() {
        let mut vt = new_vt();
        for line in 0..20 {
            vt.feed(format!("line{line:02}\r\n").as_bytes());
        }
        vt.scroll_to_row(3);
        assert_eq!(vt.scroll_position().unwrap().offset, 3);
        assert_eq!(first_row_text(&mut vt), "line03");
        // いちばん新しい行より先は、いちばん新しい行までに収まる。
        vt.scroll_to_row(1000);
        assert_eq!(vt.scroll_position().unwrap(), position(21, 16, 5));
        assert_eq!(first_row_text(&mut vt), "line16");
    }

    #[test]
    fn the_alternate_screen_has_no_scrollback() {
        let mut vt = new_vt();
        for line in 0..20 {
            vt.feed(format!("line{line:02}\r\n").as_bytes());
        }
        vt.feed(b"\x1b[?1049h");
        assert_eq!(vt.scroll_position().unwrap(), position(5, 0, 5));
        vt.scroll(-3);
        assert_eq!(vt.scroll_position().unwrap(), position(5, 0, 5));
        // 代替画面を抜ければ、スクロールバックが戻る。
        vt.feed(b"\x1b[?1049l");
        assert_eq!(vt.scroll_position().unwrap(), position(21, 16, 5));
    }

    /// `col` 桁目・`row` 行目のセルの、左から `fraction` の割合の位置を指す。
    fn pointer_at(col: u16, row: u16, fraction: f64) -> PointerPosition {
        PointerPosition {
            col,
            row,
            x: (f64::from(col) + fraction) * f64::from(SIZE.cell_width_px),
            y: (f64::from(row) + 0.5) * f64::from(SIZE.cell_height_px),
        }
    }

    fn pointer(col: u16, row: u16) -> PointerPosition {
        pointer_at(col, row, 0.5)
    }

    const DOUBLE_CLICK: Duration = Duration::from_millis(500);

    #[test]
    fn dragging_selects_text_and_marks_cells() {
        let mut vt = new_vt();
        vt.feed(b"hello world");
        vt.selection_press(pointer_at(0, 0, 0.1), Duration::ZERO, DOUBLE_CLICK)
            .unwrap();
        // セルの半分を越えたところまで動かすと、そのセルまで選ばれる。
        let end = pointer_at(4, 0, 0.9);
        vt.selection_drag(end, false).unwrap();
        vt.selection_release(end).unwrap();
        assert_eq!(vt.selected_text().unwrap().as_deref(), Some("hello"));

        let frame = vt.frame().unwrap();
        // 選んだセルは色が入れ替わり、背景色を持つ。
        assert_eq!(frame.cells[0].bg, Some(frame.foreground));
        assert_eq!(frame.cells[6].bg, None);

        vt.clear_selection().unwrap();
        assert_eq!(vt.selected_text().unwrap(), None);
    }

    #[test]
    fn select_all_includes_the_scrollback() {
        let mut vt = new_vt();
        for line in 0..10 {
            vt.feed(format!("line{line}\r\n").as_bytes());
        }
        vt.select_all().unwrap();
        let text = vt.selected_text().unwrap().unwrap();
        assert!(text.starts_with("line0\n"));
        assert!(text.contains("line9"));
    }

    #[test]
    fn double_click_selects_a_word() {
        let mut vt = new_vt();
        vt.feed(b"hello world");
        let at = pointer(7, 0);
        vt.selection_press(at, Duration::ZERO, DOUBLE_CLICK)
            .unwrap();
        vt.selection_release(at).unwrap();
        vt.selection_press(at, Duration::from_millis(100), DOUBLE_CLICK)
            .unwrap();
        vt.selection_release(at).unwrap();
        assert_eq!(vt.selected_text().unwrap().as_deref(), Some("world"));
    }

    fn mouse(action: MouseAction, col: u16, row: u16) -> MouseInput {
        let at = pointer(col, row);
        MouseInput {
            action,
            button: Some(MouseButton::Left),
            mods: Mods::empty(),
            x: at.x as f32,
            y: at.y as f32,
            any_button_pressed: action == MouseAction::Press,
        }
    }

    #[test]
    fn mouse_is_not_reported_unless_requested() {
        let mut vt = new_vt();
        assert!(!vt.is_mouse_tracking().unwrap());
        let encoded = vt.encode_mouse(&mouse(MouseAction::Press, 2, 3)).unwrap();
        assert!(encoded.is_empty());
    }

    #[test]
    fn mouse_clicks_are_reported_in_sgr_format() {
        let mut vt = new_vt();
        // ボタンの押し離しを報告させ、形式は SGR にする。
        vt.feed(b"\x1b[?1000h\x1b[?1006h");
        assert!(vt.is_mouse_tracking().unwrap());
        let press = vt.encode_mouse(&mouse(MouseAction::Press, 2, 3)).unwrap();
        assert_eq!(press, b"\x1b[<0;3;4M");
        let release = vt.encode_mouse(&mouse(MouseAction::Release, 2, 3)).unwrap();
        assert_eq!(release, b"\x1b[<0;3;4m");
    }

    #[test]
    fn pasting_one_line_sends_it_as_is() {
        let mut vt = new_vt();
        assert_eq!(vt.paste("echo hi", false).unwrap(), PasteOutcome::Pasted);
        assert_eq!(vt.take_replies(), b"echo hi");
    }

    #[test]
    fn pasting_lines_without_bracketed_paste_needs_permission() {
        let mut vt = new_vt();
        assert_eq!(vt.paste("a\nb", false).unwrap(), PasteOutcome::Unsafe);
        assert!(vt.take_replies().is_empty());
        assert_eq!(vt.paste("a\nb", true).unwrap(), PasteOutcome::Pasted);
        // 改行は CR にして送る。
        assert_eq!(vt.take_replies(), b"a\rb");
    }

    #[test]
    fn pasting_lines_with_bracketed_paste_is_safe() {
        let mut vt = new_vt();
        vt.feed(b"\x1b[?2004h");
        assert_eq!(vt.paste("a\nb", false).unwrap(), PasteOutcome::Pasted);
        assert_eq!(vt.take_replies(), b"\x1b[200~a\nb\x1b[201~");
    }

    #[test]
    fn pasting_the_end_of_bracketed_paste_needs_permission() {
        let mut vt = new_vt();
        vt.feed(b"\x1b[?2004h");
        assert_eq!(
            vt.paste("a\x1b[201~b", false).unwrap(),
            PasteOutcome::Unsafe
        );
        assert!(vt.take_replies().is_empty());
        // 許しても、ESC は空白に変えて送るので、貼り付けの途中で終わらない。
        assert_eq!(vt.paste("a\x1b[201~b", true).unwrap(), PasteOutcome::Pasted);
        assert_eq!(vt.take_replies(), b"\x1b[200~a [201~b\x1b[201~");
    }

    #[test]
    fn control_characters_are_pasted_as_spaces() {
        let mut vt = new_vt();
        assert_eq!(vt.paste("a\x1bb\x7fc", true).unwrap(), PasteOutcome::Pasted);
        assert_eq!(vt.take_replies(), b"a b c");
        // CR は改行と同じく、そのまま送る。
        assert_eq!(vt.paste("a\rb\r\n", true).unwrap(), PasteOutcome::Pasted);
        assert_eq!(vt.take_replies(), b"a\rb\r\r");
    }

    #[test]
    fn safety_of_text_does_not_depend_on_the_mode() {
        assert!(is_safe_to_paste("echo hi"));
        assert!(!is_safe_to_paste("a\nb"));
        assert!(!is_safe_to_paste("a\rb"));
        assert!(!is_safe_to_paste("a\x1b[201~b"));
    }

    #[test]
    fn cursor_cell_follows_text() {
        let mut vt = new_vt();
        vt.feed("ab\r\nあ".as_bytes());
        assert_eq!(vt.cursor_cell().unwrap(), (2, 1));
    }

    #[test]
    fn frame_cursor_follows_text() {
        let mut vt = new_vt();
        vt.feed("ab\r\nあ".as_bytes());
        let cursor = vt.frame().unwrap().cursor.unwrap();
        assert_eq!((cursor.col, cursor.row, cursor.width), (2, 1, 1));
    }

    fn press(key: Key, mods: Mods, unshifted: char, text: Option<&str>) -> KeyInput<'_> {
        KeyInput {
            key,
            action: KeyAction::Press,
            mods,
            consumed_mods: Mods::empty(),
            unshifted,
            text,
        }
    }

    #[test]
    fn keys_encode_in_legacy_mode() {
        let mut vt = new_vt();
        let mut encode = |input| vt.encode_key(&input).unwrap();
        assert_eq!(encode(press(Key::A, Mods::empty(), 'a', Some("a"))), b"a");
        assert_eq!(encode(press(Key::Enter, Mods::empty(), '\0', None)), b"\r");
        assert_eq!(
            encode(press(Key::Backspace, Mods::empty(), '\0', None)),
            b"\x7f"
        );
        assert_eq!(
            encode(press(Key::ArrowUp, Mods::empty(), '\0', None)),
            b"\x1b[A"
        );
        assert_eq!(encode(press(Key::C, Mods::CTRL, 'c', None)), b"\x03");
        assert_eq!(encode(press(Key::A, Mods::ALT, 'a', Some("a"))), b"\x1ba");
    }

    #[test]
    fn application_cursor_mode_changes_arrow_keys() {
        let mut vt = new_vt();
        // DECCKM: カーソルキーをアプリケーションモードにする（vim などが使う）。
        vt.feed(b"\x1b[?1h");
        let encoded = vt
            .encode_key(&press(Key::ArrowUp, Mods::empty(), '\0', None))
            .unwrap();
        assert_eq!(encoded, b"\x1bOA");
    }

    /// kitty keyboard protocol の「エスケープコードの曖昧さをなくす」（フラグ 1）では、ロックの状態を
    /// 修飾キーの数に入れない。
    #[test]
    fn kitty_disambiguate_leaves_out_lock_mods() {
        let mut vt = new_vt();
        vt.feed(b"\x1b[>1u");
        assert_eq!(
            vt.terminal.kitty_keyboard_flags().unwrap(),
            key::KittyKeyFlags::DISAMBIGUATE
        );
        let mut encode = |input| vt.encode_key(&input).unwrap();
        let num_lock = Mods::NUM_LOCK;
        let caps_lock = Mods::CAPS_LOCK;
        assert_eq!(
            encode(press(Key::Enter, Mods::SHIFT | num_lock, '\0', None)),
            b"\x1b[13;2u"
        );
        assert_eq!(
            encode(press(Key::Enter, Mods::CTRL | num_lock, '\0', None)),
            b"\x1b[13;5u"
        );
        assert_eq!(
            encode(press(Key::I, Mods::CTRL | num_lock, 'i', None)),
            b"\x1b[105;5u"
        );
        assert_eq!(
            encode(press(Key::Enter, Mods::SHIFT | caps_lock, '\0', None)),
            b"\x1b[13;2u"
        );
        assert_eq!(
            encode(press(Key::I, Mods::CTRL | caps_lock | num_lock, 'i', None)),
            b"\x1b[105;5u"
        );
        assert_eq!(
            encode(press(Key::Enter, num_lock | caps_lock, '\0', None)),
            b"\r"
        );
        assert_eq!(encode(press(Key::A, caps_lock, 'a', Some("A"))), b"A");
    }

    /// 「すべてのキーをエスケープコードで報告する」（フラグ 8）では、ロックの状態も修飾キーの数に入れる。
    #[test]
    fn kitty_report_all_includes_lock_mods() {
        let mut vt = new_vt();
        vt.feed(b"\x1b[>8u");
        let mut encode = |input| vt.encode_key(&input).unwrap();
        assert_eq!(
            encode(press(Key::Enter, Mods::SHIFT | Mods::NUM_LOCK, '\0', None)),
            b"\x1b[13;130u"
        );
        assert_eq!(
            encode(press(Key::Enter, Mods::NUM_LOCK, '\0', None)),
            b"\x1b[13;129u"
        );
        assert_eq!(
            encode(press(Key::A, Mods::CAPS_LOCK, 'a', Some("A"))),
            b"\x1b[97;65u"
        );
    }

    /// kitty keyboard protocol をやめたあとは、ロックの状態をこれまでどおり渡す。
    #[test]
    fn lock_mods_pass_through_after_kitty_is_popped() {
        let mut vt = new_vt();
        vt.feed(b"\x1b[>1u\x1b[<u");
        assert!(vt.terminal.kitty_keyboard_flags().unwrap().is_empty());
        assert_eq!(
            vt.mods_to_report(Mods::SHIFT | Mods::NUM_LOCK).unwrap(),
            Mods::SHIFT | Mods::NUM_LOCK
        );
    }

    #[test]
    fn key_release_sends_nothing_in_legacy_mode() {
        let mut vt = new_vt();
        let release = KeyInput {
            action: KeyAction::Release,
            ..press(Key::A, Mods::empty(), 'a', None)
        };
        assert!(vt.encode_key(&release).unwrap().is_empty());
    }

    /// ウィンドウの大きさを変えている間は、Windows の処理の上にウィンドウプロシージャが積まれて呼ばれ、
    /// スタックの残りが少ない。そこで桁数を増やしても、スタックがあふれないこと。
    #[test]
    fn growing_columns_fits_in_a_small_stack() {
        const SMALL_STACK: usize = 256 * 1024;
        let worker = std::thread::Builder::new()
            .stack_size(SMALL_STACK)
            .spawn(|| {
                let mut vt = new_vt();
                for line in 0..30 {
                    vt.feed(format!("line {line}: 日本語と ascii\r\n").as_bytes());
                }
                vt.resize(GridSize {
                    cols: 200,
                    rows: 30,
                    ..SIZE
                })
                .unwrap();
            })
            .unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn resizing_through_many_sizes_keeps_working() {
        let mut vt = new_vt();
        for line in 0..30 {
            vt.feed(format!("line {line}: 日本語と ascii が混ざった長めの行です\r\n").as_bytes());
        }
        for (cols, rows) in [(1, 1), (3, 2), (80, 24), (2, 50), (200, 3), (20, 5)] {
            vt.resize(GridSize { cols, rows, ..SIZE }).unwrap();
            vt.frame().unwrap();
        }
    }

    #[test]
    fn size_report_follows_resize() {
        let mut vt = new_vt();
        let resized = GridSize {
            cols: 40,
            rows: 10,
            ..SIZE
        };
        vt.resize(resized).unwrap();
        assert_eq!(vt.size(), resized);
        // CSI 18 t: テキスト領域の大きさを文字数で問い合わせる。
        vt.feed(b"\x1b[18t");
        assert_eq!(vt.take_replies(), b"\x1b[8;10;40t");
    }

    /// 赤い 1×1 の PNG の base64。
    const RED_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAA\
                           DUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

    /// 3 バイトごとに 4 文字にする base64。テストの入力に使う。
    fn base64(data: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::new();
        for chunk in data.chunks(3) {
            let mut bytes = [0u8; 3];
            bytes[..chunk.len()].copy_from_slice(chunk);
            let digits = [
                bytes[0] >> 2,
                (bytes[0] & 0x03) << 4 | bytes[1] >> 4,
                (bytes[1] & 0x0f) << 2 | bytes[2] >> 6,
                bytes[2] & 0x3f,
            ];
            for (index, digit) in digits.into_iter().enumerate() {
                encoded.push(if index <= chunk.len() {
                    char::from(ALPHABET[usize::from(digit)])
                } else {
                    '='
                });
            }
        }
        encoded
    }

    fn graphics(vt: &mut Vt, control: &str, payload: &str) {
        vt.feed(format!("\x1b_G{control};{payload}\x1b\\").as_bytes());
    }

    #[test]
    fn a_png_image_is_placed_at_the_cursor_above_the_text() {
        let mut vt = new_vt();
        vt.feed(b"\x1b[2;3H");
        graphics(&mut vt, "a=T,f=100,i=7,q=2", RED_PNG);
        let frame = vt.frame().unwrap();
        let [view] = frame.images.as_slice() else {
            panic!("{:?}", frame.images);
        };
        assert_eq!(view.key.id, 7);
        assert_eq!(view.layer, ImageLayer::AboveText);
        assert_eq!((view.col, view.row), (2, 1));
        assert_eq!((view.width, view.height), (1, 1));
        let pixels = vt.image_pixels(&view.key).unwrap().unwrap();
        assert_eq!((pixels.width, pixels.height), (1, 1));
        assert_eq!(pixels.rgba.as_ref(), [255, 0, 0, 255]);
    }

    #[test]
    fn raw_rgb_is_converted_to_rgba_and_spans_rows() {
        let mut vt = new_vt();
        // 8×16 のセルで 10×20 ピクセルは 2 桁・2 行にまたがる。
        let (width, height) = (10u32, 20u32);
        let rgb: Vec<u8> = (0..width * height)
            .flat_map(|pixel| [(pixel % 256) as u8, 1, 2])
            .collect();
        let control = format!("a=T,f=24,s={width},v={height},i=2,q=2");
        graphics(&mut vt, &control, &base64(&rgb));
        let frame = vt.frame().unwrap();
        let view = frame.images[0];
        assert_eq!((view.width, view.height), (width, height));
        assert_eq!((view.source_width, view.source_height), (width, height));
        // 画像の下の行へカーソルが進む。
        assert_eq!(vt.cursor_cell().unwrap().1, 1);
        let pixels = vt.image_pixels(&view.key).unwrap().unwrap();
        assert_eq!(&pixels.rgba[..8], [0, 1, 2, 255, 1, 1, 2, 255]);
        assert_eq!(pixels.rgba.len(), (width * height * 4) as usize);
    }

    #[test]
    fn gray_pixels_are_converted_to_rgba() {
        assert_eq!(
            rgba_pixels(ImageFormat::Gray, &[9], 1, 1).unwrap().rgba,
            [9, 9, 9, 255].as_slice()
        );
        assert_eq!(
            rgba_pixels(ImageFormat::GrayAlpha, &[9, 3], 1, 1)
                .unwrap()
                .rgba,
            [9, 9, 9, 3].as_slice()
        );
        // バイト数が足りなければ読まない。
        assert_eq!(rgba_pixels(ImageFormat::Rgb, &[1, 2], 1, 1), None);
    }

    #[test]
    fn chunks_become_one_image() {
        let mut vt = new_vt();
        let rgba = base64(&[10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255]);
        let (first, rest) = rgba.split_at(4);
        graphics(&mut vt, "a=T,f=32,s=3,v=1,i=3,q=2,m=1", first);
        assert!(vt.frame().unwrap().images.is_empty());
        graphics(&mut vt, "m=0", rest);
        let frame = vt.frame().unwrap();
        let pixels = vt.image_pixels(&frame.images[0].key).unwrap().unwrap();
        assert_eq!(
            pixels.rgba.as_ref(),
            [10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255]
        );
    }

    #[test]
    fn deleting_removes_the_image_from_the_frame_and_the_storage() {
        let mut vt = new_vt();
        graphics(&mut vt, "a=T,f=100,i=4,q=2", RED_PNG);
        let key = vt.frame().unwrap().images[0].key;
        assert!(vt.image_is_stored(&key).unwrap());
        vt.feed(b"\x1b_Ga=d,d=I,i=4,q=2\x1b\\");
        assert!(vt.frame().unwrap().images.is_empty());
        assert!(!vt.image_is_stored(&key).unwrap());
        assert_eq!(vt.image_pixels(&key).unwrap(), None);
    }

    #[test]
    fn sending_again_changes_the_generation() {
        let mut vt = new_vt();
        graphics(&mut vt, "a=T,f=100,i=5,q=2", RED_PNG);
        let old = vt.frame().unwrap().images[0].key;
        graphics(&mut vt, "a=T,f=100,i=5,q=2", RED_PNG);
        let new = vt.frame().unwrap().images[0].key;
        assert_ne!(old.generation, new.generation);
        assert!(!vt.image_is_stored(&old).unwrap());
        assert!(vt.image_is_stored(&new).unwrap());
    }

    #[test]
    fn queries_are_answered() {
        let mut vt = new_vt();
        graphics(&mut vt, "i=31,s=1,v=1,a=q,t=d,f=24", "AAAA");
        assert_eq!(vt.take_replies(), b"\x1b_Gi=31;OK\x1b\\");
        // 確かめるだけで、保管も画面も変わらない。
        assert!(vt.frame().unwrap().images.is_empty());
    }

    #[test]
    fn files_cannot_be_read_as_images() {
        let mut vt = new_vt();
        let path = base64(br"C:\Windows\win.ini");
        graphics(&mut vt, "i=32,s=1,v=1,a=q,t=f,f=24", &path);
        let replies = vt.take_replies();
        let reply = String::from_utf8_lossy(&replies);
        assert!(reply.starts_with("\x1b_Gi=32;"), "{reply:?}");
        assert!(!reply.contains(";OK"), "{reply:?}");
    }

    #[test]
    fn images_scroll_with_the_text_and_into_the_scrollback() {
        let mut vt = new_vt();
        graphics(&mut vt, "a=T,f=100,i=6,q=2", RED_PNG);
        assert_eq!(vt.frame().unwrap().images[0].row, 0);
        vt.feed(b"\r\n\r\n\r\n\r\n\r\n");
        // 上へ流れて、表示している範囲から出た。
        let scrolled = vt.frame().unwrap().images;
        assert!(scrolled.is_empty(), "{scrolled:?}");
        vt.scroll_to_top();
        assert_eq!(vt.frame().unwrap().images[0].row, 0);
    }

    #[test]
    fn each_screen_has_its_own_images() {
        let mut vt = new_vt();
        graphics(&mut vt, "a=T,f=100,i=8,q=2", RED_PNG);
        let key = vt.frame().unwrap().images[0].key;
        vt.feed(b"\x1b[?1049h");
        assert!(vt.frame().unwrap().images.is_empty());
        assert!(!vt.image_is_stored(&key).unwrap());
        vt.feed(b"\x1b[?1049l");
        assert_eq!(vt.frame().unwrap().images[0].key, key);
        assert!(vt.image_is_stored(&key).unwrap());
    }

    #[test]
    fn layers_and_z_decide_the_order() {
        let mut vt = new_vt();
        let z_below_background = i32::MIN;
        for (id, z) in [(1, 5), (2, -1), (3, z_below_background), (4, 0), (5, 0)] {
            graphics(&mut vt, &format!("a=T,f=100,i={id},z={z},q=2"), RED_PNG);
        }
        let frame = vt.frame().unwrap();
        let order: Vec<(u32, ImageLayer)> = frame
            .images
            .iter()
            .map(|view| (view.key.id, view.layer))
            .collect();
        assert_eq!(
            order,
            [
                (3, ImageLayer::BelowBackground),
                (2, ImageLayer::BelowText),
                (4, ImageLayer::AboveText),
                (5, ImageLayer::AboveText),
                (1, ImageLayer::AboveText),
            ]
        );
    }

    #[test]
    fn placeholder_cells_show_the_image_instead_of_the_character() {
        let mut vt = new_vt();
        // 画像 3 の仮想の配置（2 桁・1 行）と、それを見せる 2 つのセル。
        graphics(&mut vt, "a=T,U=1,f=32,i=3,s=1,v=1,c=2,r=1,q=2", "AP8A/w==");
        vt.feed(
            "\x1b[38;5;3m\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}\u{0305}\u{030D}\x1b[39m".as_bytes(),
        );
        let frame = vt.frame().unwrap();
        let [view] = frame.images.as_slice() else {
            panic!("{:?}", frame.images);
        };
        assert_eq!(view.key.id, 3);
        assert_eq!(view.layer, ImageLayer::BelowText);
        assert_eq!((view.width, view.height), (16, 16));
        assert!(frame.cells.iter().all(|cell| cell.text.is_empty()));
    }

    #[test]
    fn animations_report_the_next_frame() {
        let mut vt = new_vt();
        graphics(&mut vt, "a=T,f=32,i=2,s=1,v=1,q=2", "/wAA/w==");
        assert_eq!(vt.tick_image_animations(Duration::ZERO).unwrap(), None);
        let first = vt.frame().unwrap().images[0].key;
        graphics(&mut vt, "a=f,f=32,i=2,s=1,v=1,z=40,q=2", "AAD//w==");
        vt.feed(b"\x1b_Ga=a,i=2,r=1,z=30,q=2\x1b\\\x1b_Ga=a,i=2,s=3,q=2\x1b\\");
        assert_eq!(
            vt.tick_image_animations(Duration::ZERO).unwrap(),
            Some(Duration::from_millis(30))
        );
        assert_eq!(
            vt.tick_image_animations(Duration::from_millis(30)).unwrap(),
            Some(Duration::from_millis(40))
        );
        let second = vt.frame().unwrap().images[0].key;
        assert_ne!(first.generation, second.generation);
        let pixels = vt.image_pixels(&second).unwrap().unwrap();
        assert_eq!(pixels.rgba.as_ref(), [0, 0, 255, 255]);
    }

    /// 大きさ `width` × `height` の、黒で埋めた sixel。
    fn black_sixel(width: u32, height: u32) -> Vec<u8> {
        format!("\x1bPq\"1;1;{width};{height}\x1b\\").into_bytes()
    }

    #[test]
    fn a_sixel_becomes_an_image_at_the_cursor_below_the_text() {
        let mut vt = new_vt();
        vt.feed(b"abc");
        vt.feed(b"\x1bP0;1q#1;2;100;0;0#1!2@\x1b\\");
        assert!(vt.take_replies().is_empty());
        let frame = vt.frame().unwrap();
        let [view] = frame.images.as_slice() else {
            panic!("{:?}", frame.images);
        };
        assert_eq!((view.col, view.row), (3, 0));
        assert_eq!((view.width, view.height), (2, 1));
        assert_eq!(view.layer, ImageLayer::BelowText);
        let pixels = vt.image_pixels(&view.key).unwrap().unwrap();
        assert_eq!((pixels.width, pixels.height), (2, 1));
        assert_eq!(pixels.rgba.as_ref(), [255, 0, 0, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn large_sixels_arrive_whole() {
        let mut vt = new_vt();
        vt.feed(b"\x1bPq#1;2;0;100;0#1!300~-!300~\x1b\\");
        let frame = vt.frame().unwrap();
        let pixels = vt.image_pixels(&frame.images[0].key).unwrap().unwrap();
        assert_eq!((pixels.width, pixels.height), (300, 12));
        let (rgba, _) = pixels.rgba.as_chunks::<4>();
        assert!(rgba.iter().all(|pixel| *pixel == [0, 255, 0, 255]));
    }

    #[test]
    fn the_cursor_stops_on_the_last_row_of_the_sixel_in_the_same_column() {
        let mut vt = new_vt();
        vt.feed(b"abc");
        // セルは 8×16。32 ピクセルは 2 行、33 ピクセルは 3 行。
        vt.feed(&black_sixel(16, 32));
        assert_eq!(vt.cursor_cell().unwrap(), (3, 1));
        vt.feed(b"\x1b[H");
        vt.feed(&black_sixel(16, 33));
        assert_eq!(vt.cursor_cell().unwrap(), (0, 2));
        vt.feed(b"\x1b[H");
        vt.feed(&black_sixel(1, 1));
        assert_eq!(vt.cursor_cell().unwrap(), (0, 0));
    }

    #[test]
    fn the_text_after_a_sixel_is_written_at_the_cursor() {
        let mut vt = new_vt();
        let mut data = black_sixel(16, 32);
        data.extend_from_slice(b"\r\nx");
        vt.feed(&data);
        assert_eq!(vt.cursor_cell().unwrap(), (1, 2));
        assert_eq!(vt.plain_text().unwrap().trim_start_matches('\n'), "x");
    }

    #[test]
    fn a_sixel_at_the_bottom_scrolls_the_screen() {
        let mut vt = new_vt();
        vt.feed(b"top\x1b[5;1H");
        vt.feed(&black_sixel(16, 48));
        assert_eq!(vt.cursor_cell().unwrap(), (0, 4));
        let frame = vt.frame().unwrap();
        assert_eq!(frame.images[0].row, 2);
        assert_eq!(vt.scroll_position().unwrap().total, 7);
        // 続けて流れると、スクロールバックに入って上へはみ出す。
        vt.feed(b"\r\n\r\n\r\n");
        assert_eq!(vt.frame().unwrap().images[0].row, -1);
        vt.scroll(-1);
        assert_eq!(vt.frame().unwrap().images[0].row, 0);
    }

    #[test]
    fn a_sixel_taller_than_the_screen_keeps_its_top_in_the_scrollback() {
        let mut vt = new_vt();
        vt.feed(&black_sixel(16, 16 * 8));
        assert_eq!(vt.cursor_cell().unwrap(), (0, 4));
        assert_eq!(vt.frame().unwrap().images[0].row, -3);
    }

    #[test]
    fn an_opaque_sixel_erases_the_text_it_covers_and_later_text_shows() {
        let mut vt = new_vt();
        vt.feed(b"abcdef\r\nghijkl\r\nmnopqr\x1b[H");
        vt.feed(&black_sixel(16, 32));
        assert_eq!(vt.plain_text().unwrap(), "  cdef\n  ijkl\nmnopqr");
        vt.feed(b"z");
        assert_eq!(vt.plain_text().unwrap(), "  cdef\nz ijkl\nmnopqr");
    }

    #[test]
    fn a_transparent_sixel_keeps_the_text_it_covers() {
        let mut vt = new_vt();
        vt.feed(b"abcdef\x1b[H");
        vt.feed(b"\x1bP0;1q\"1;1;16;16\x1b\\");
        assert_eq!(vt.plain_text().unwrap(), "abcdef");
    }

    #[test]
    fn clearing_the_screen_removes_a_sixel() {
        let mut vt = new_vt();
        vt.feed(&black_sixel(16, 16));
        let key = vt.frame().unwrap().images[0].key;
        vt.feed(b"\x1b[2J");
        assert!(vt.frame().unwrap().images.is_empty());
        assert!(!vt.image_is_stored(&key).unwrap());
    }

    #[test]
    fn each_screen_has_its_own_sixels() {
        let mut vt = new_vt();
        vt.feed(&black_sixel(16, 16));
        vt.feed(b"\x1b[?1049h");
        assert!(vt.frame().unwrap().images.is_empty());
        vt.feed(&black_sixel(8, 8));
        assert_eq!(vt.frame().unwrap().images[0].width, 8);
        vt.feed(b"\x1b[?1049l");
        assert_eq!(vt.frame().unwrap().images[0].width, 16);
    }

    #[test]
    fn with_the_sixel_display_mode_a_sixel_goes_to_the_top_left_and_the_cursor_stays() {
        let mut vt = new_vt();
        vt.feed(b"abcdef\x1b[3;4H\x1b[?80h");
        vt.feed(&black_sixel(16, 16 * 8));
        assert_eq!(vt.cursor_cell().unwrap(), (3, 2));
        let frame = vt.frame().unwrap();
        assert_eq!((frame.images[0].col, frame.images[0].row), (0, 0));
        assert_eq!(vt.scroll_position().unwrap().total, 5);
        assert_eq!(vt.plain_text().unwrap(), "  cdef");
    }

    #[test]
    fn sixels_can_arrive_in_pieces_across_feeds() {
        let mut vt = new_vt();
        for byte in black_sixel(16, 16) {
            vt.feed(&[byte]);
        }
        assert_eq!(vt.frame().unwrap().images.len(), 1);
    }

    #[test]
    fn sixels_do_not_take_the_numbers_of_kitty_images() {
        let mut vt = new_vt();
        graphics(&mut vt, "a=T,f=32,i=1,s=1,v=1,C=1,q=2", "/wAA/w==");
        vt.feed(&black_sixel(16, 16));
        graphics(&mut vt, "a=T,f=32,i=2,s=1,v=1,C=1,q=2", "/wAA/w==");
        vt.feed(&black_sixel(8, 8));
        let frame = vt.frame().unwrap();
        assert_eq!(frame.images.len(), 4);
        let mut ids: Vec<u32> = frame.images.iter().map(|view| view.key.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 4);
    }
}
