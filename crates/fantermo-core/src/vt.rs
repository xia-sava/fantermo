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
    log::{Level as LogLevel, Logger, set_logger},
    mouse,
    paste::Options as PasteOptions,
    render::{
        CellIterator, ImageIterator, ImageLayer as VtImageLayer, ImageProtocol, RenderState,
        RowIterator, Snapshot,
    },
    screen::Screen,
    screen::{CellWide, GridRef},
    selection::{
        FormatOptions as SelectionFormatOptions,
        gesture::{DragEvent, Geometry, Gesture, PressEvent, ReleaseEvent},
    },
    style::{Palette, PaletteIndex, RgbColor, Style, StyleColor, Underline as StyleUnderline},
    terminal::{
        ClipboardMime, ClipboardWrite, ClipboardWriteError, ConformanceLevel,
        DeviceAttributeFeature, DeviceAttributes, DeviceType, Mode, Point, PointCoordinate,
        PointSpace, PrimaryDeviceAttributes, ScrollViewport, SecondaryDeviceAttributes,
        SizeReportSize,
    },
};
use std::time::Duration;

use crate::{
    backend::GridSize,
    clipboard::{self, ReadRequest, WriteRequest},
    config::TerminalColors,
    frame::{
        CellView, CursorView, Frame, ImageKey, ImageLayer, ImagePixels, ImageView, Rgb, Underline,
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
    /// コマンドとして実行される可能性があるので送らなかった。
    Unsafe,
}

/// `text` が、端末のモードに関わらずコマンドとして実行されえない。改行（LF、CR）と bracketed paste の
/// 終わりの印（`ESC [ 201 ~`）を含めば `false`。
pub fn is_safe_to_paste(text: &str) -> bool {
    libghostty_vt::paste::is_safe(text)
}

/// クリップボードの文字列 `text` の改行 CRLF を LF にする。貼り付けと、アプリへ返すクリップボードの
/// 答えは、この形にしてから扱う。
pub fn normalize_paste(text: &str) -> Cow<'_, str> {
    if text.contains("\r\n") {
        Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(text)
    }
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
pub(crate) const MAX_LINK_LINE_ROWS: u32 = 64;

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

/// アプリからの書き込み `request` を、クリップボードへの書き込みにする。OSC 52 の選択先（`p`、`s` など）は
/// 見ない。Windows のクリップボードは一つなので、どれも同じところへ書く。
fn clipboard_write_request(
    request: &ClipboardWrite<'_>,
) -> Result<WriteRequest, ClipboardWriteError> {
    let mut contents = request.contents();
    if contents.len() == 0 {
        return Ok(WriteRequest::Clear);
    }
    let Some(text) = contents.find(|content| is_text_mime(content.mime)) else {
        tracing::warn!("ignoring a clipboard write without text");
        return Err(ClipboardWriteError::Unsupported);
    };
    clipboard::write_request(text.data).ok_or_else(|| {
        tracing::warn!(
            "ignoring a clipboard write of {} bytes: over the limit of {}",
            text.data.len(),
            clipboard::MAX_WRITE_BYTES
        );
        ClipboardWriteError::InvalidData
    })
}

/// `mime` が文字列の種類（`text/plain` か、`text/plain;charset=utf-8` のように添え書きの付いたもの）。
fn is_text_mime(mime: &[u8]) -> bool {
    let plain = PLAIN_TEXT_MIME.as_bytes();
    mime.strip_prefix(plain)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(b";"))
}

/// libghostty-vt のログを `tracing` に流す。シーケンスの解釈の警告はアプリの出力しだいでログを埋めるので、
/// クリップボードへの書き込みなどを受け持つ範囲の警告とエラーだけを流す。
struct VtLogger;

/// [`VtLogger`] が警告を流す、libghostty-vt のログの範囲。無効な base64 の OSC 52 を捨てたことなどが出る。
const LOGGED_WARNING_SCOPE: &str = "stream_terminal";

impl Logger for VtLogger {
    fn log(&self, level: LogLevel, scope: &str, message: &str) {
        match level {
            LogLevel::Error => tracing::warn!("libghostty-vt: {scope}: {message}"),
            LogLevel::Warning if scope == LOGGED_WARNING_SCOPE => {
                tracing::warn!("libghostty-vt: {message}");
            }
            _ => {}
        }
    }
}

/// PNG のデコーダとログの受け口をプロセスに一度だけ入れる。
fn install_process_hooks() -> Result<(), VtError> {
    static INSTALLED: OnceLock<Result<(), VtError>> = OnceLock::new();
    *INSTALLED.get_or_init(|| {
        // SAFETY: 端末はどれも `Vt::new` で作り、最初の端末を作る前にここを通る。ここを通っている間、
        // ほかのスレッドの `Vt::new` は `get_or_init` で待つので、端末を使うスレッドはまだない。
        unsafe {
            set_logger(Some(Box::new(VtLogger)))?;
            graphics::set_png_decoder(Some(Box::new(RustPngDecoder::new())))
        }
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
    /// アプリからのクリップボードへの書き込み。コールバックの中で溜める。
    clipboard_writes: Rc<RefCell<Vec<WriteRequest>>>,
    /// アプリからのクリップボードの読み取りの依頼。
    clipboard_reads: Vec<ReadRequest>,
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
        let clipboard_writes = Rc::new(RefCell::new(Vec::new()));

        install_process_hooks()?;
        let mut terminal = Terminal::new(size.cols, size.rows)?;
        terminal
            .set_scrollback_max_lines(Some(SCROLLBACK_LINES))?
            .set_terminfo_name(terminfo_name)?
            .set_kitty_image_storage_limit(IMAGE_STORAGE_LIMIT)?
            .set_clipboard_write_max_bytes(Some(clipboard::MAX_WRITE_BYTES))?;
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
            })?
            .on_clipboard_write({
                let clipboard_writes = Rc::clone(&clipboard_writes);
                move |_terminal, request| match clipboard_write_request(&request) {
                    Ok(write) => {
                        clipboard_writes.borrow_mut().push(write);
                        request.reply(Ok(()), false);
                    }
                    Err(error) => request.reply(Err(error), false),
                }
            })?;

        Ok(Self {
            terminal,
            output_scanner: OutputScanner::new(),
            replies,
            size: size_cell,
            last_title: Vec::new(),
            bell_rang,
            clipboard_writes,
            clipboard_reads: Vec::new(),
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

    /// ホイールをカーソルキーに変えて送るか。代替画面で、アプリがマウスを求めておらず、Alternate Scroll
    /// （DECSET 1007）を切っていないとき。
    pub fn wheel_sends_cursor_keys(&self) -> Result<bool, VtError> {
        Ok(self.terminal.active_screen()? == Screen::Alternate
            && !self.terminal.is_mouse_tracking()?
            && self.terminal.mode(Mode::ALT_SCROLL)?)
    }

    /// ホイールで `lines` 行動かす分のカーソルキーを、シェルへ送るバイト列に変える。正なら上、負なら下。
    /// キーの変換器を通すので、カーソルキーのモード（DECCKM）にも kitty keyboard protocol にも従う。
    /// 押して離した 1 回を 1 行とする（離したのを求めるアプリにだけ、離した分のバイト列ができる）。
    pub fn encode_wheel_as_cursor_keys(&mut self, lines: i32) -> Result<Vec<u8>, VtError> {
        let key = if lines > 0 {
            Key::ArrowUp
        } else {
            Key::ArrowDown
        };
        let input = |action| KeyInput {
            key,
            action,
            mods: Mods::empty(),
            consumed_mods: Mods::empty(),
            unshifted: '\0',
            text: None,
        };
        let mut encoded = self.encode_key(&input(KeyAction::Press))?;
        encoded.extend(self.encode_key(&input(KeyAction::Release))?);
        Ok(encoded.repeat(lines.unsigned_abs() as usize))
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
    /// 改行を含む文字列は、アプリが bracketed paste に対応していなければコマンドとして実行される可能性がある
    /// ので、`allow_unsafe` でなければ送らずに [`PasteOutcome::Unsafe`] を返す。
    pub fn paste(&mut self, text: &str, allow_unsafe: bool) -> Result<PasteOutcome, VtError> {
        let text = normalize_paste(text);
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
        let replies = &self.replies;
        let clipboard_reads = &mut self.clipboard_reads;
        let size = self.size.get();
        self.output_scanner.scan(data, &mut |event| match event {
            OutputEvent::Bytes(bytes) => terminal.vt_write(bytes),
            OutputEvent::Reply(reply) => replies.borrow_mut().extend_from_slice(&reply),
            OutputEvent::Sixel { image, scrolling } => {
                place_sixel(terminal, size, &image, scrolling);
            }
            OutputEvent::ClipboardRead(request) => clipboard_reads.push(request),
        });
    }

    /// アプリからのクリップボードへの書き込みを、来た順に取り出す（OSC 52、OSC 1337 Copy、OSC 5522）。
    pub fn take_clipboard_writes(&mut self) -> Vec<WriteRequest> {
        std::mem::take(&mut *self.clipboard_writes.borrow_mut())
    }

    /// アプリからのクリップボードの読み取りの依頼（OSC 52）を、来た順に取り出す。
    pub fn take_clipboard_reads(&mut self) -> Vec<ReadRequest> {
        std::mem::take(&mut self.clipboard_reads)
    }

    /// 前に呼んだときからタイトル（OSC 0 / OSC 2 で設定されたもの）が変わっていれば、新しいタイトルを
    /// 返す。消されたときは空の文字列。UTF-8 でないタイトルは、libghostty-vt が設定しない。
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
                    // 前景色を FAINT_OPACITY の割合で背景色に重ねる。
                    let background = rgb(bg.unwrap_or(colors.background));
                    fg = rgb_color(background.mix(rgb(fg), FAINT_OPACITY));
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
/// xterm と違い、文字を書いてもそのセルの画像は消えない（kitty graphics protocol の画像の配置は文字に
/// 反応しない）。空白で画像を消すアプリでは古い画像が残る。`CSI 2 J` では消える。
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
                DeviceAttributeFeature::SIXEL,
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
mod tests;
