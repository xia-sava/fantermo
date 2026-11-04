//! e2e テストの制御の口のうち、ウィンドウに触らない側（feature `e2e` のときだけコンパイルする）。
//!
//! 環境変数 [`DIR_ENV`] があるときだけ、`127.0.0.1` の OS が割り当てたポートで小さな HTTP を受け、ポートの番号を
//! そのフォルダの [`PORT_FILE`] に書く。受けた要求はチャネルでウィンドウのスレッドへ渡し、答えを待って返す。
//! ウィンドウ側の処理は `window::e2e`。
//!
//! 一つの接続で要求を続けて受ける（keep-alive）。接続ごとにスレッドを使い、次の要求が [`STREAM_TIMEOUT`] のあいだ
//! 来なければ閉じる。[`PORT_FILE`] は 1 行目にポートの番号、2 行目に乱数のトークンを書く。要求には
//! [`TOKEN_HEADER`] でトークンを付ける。付いていない、合わない要求は 403 で断る。
//!
//! | 要求 | すること |
//! |---|---|
//! | `GET /e2e/state` | 状態の JSON |
//! | `GET /e2e/frame` | いまの画面の PNG |
//! | `POST /e2e/input` | 本文のバイト列をそのままバックエンドへ送る |
//! | `POST /e2e/text` | 本文の文字を、キーを伴わずに届く文字（`WM_CHAR`）と同じ経路で送る |
//! | `POST /e2e/key/<組み合わせ>` | `ctrl+shift+t` などを、キーを押したときと同じ経路で処理する |
//! | `POST /e2e/tab/new`、`/e2e/tab/new/<プロファイルの位置>`、`/e2e/tab/close`、`/e2e/tab/select/<位置>` | タブの操作 |
//! | `POST /e2e/window/size/<桁数>/<行数>` | 端末がその大きさになるようにウィンドウの大きさを変える |
//! | `POST /e2e/window/lower` | ウィンドウを同じデスクトップの Z オーダーのいちばん下へ移す |
//! | `POST /e2e/window/minimize`、`/e2e/window/restore` | ウィンドウを最小化する、元の大きさに戻す（`ShowWindow`。`SW_RESTORE` は最大化も解く）。そのあとの状態を返す |
//! | `POST /e2e/window/block/<ミリ秒>` | 答えを返してから、ウィンドウのスレッドをその時間だけ止める |
//! | `POST /e2e/paste` | 本文の文字を、クリップボードから読んだものとして貼り付ける |
//! | `POST /e2e/dialog/button/<ID>` | 開いているダイアログのそのボタンを押す |
//! | `GET /e2e/dialog/controls` | いちばん上のダイアログの大きさと、コントロールの値の JSON |
//! | `POST /e2e/dialog/text/<ID>` | 入力欄の文字を本文の文字にする（`EN_CHANGE` が届く） |
//! | `POST /e2e/dialog/select/<ID>/<位置>` | コンボボックスや一覧のその項目を選ぶ（`CBN_SELCHANGE`、`LBN_SELCHANGE` が届く） |
//! | `POST /e2e/dialog/radio/<ID>` | ラジオボタンを選ぶ（`BN_CLICKED` が届く） |
//! | `POST /e2e/dialog/click/<ID>` | ボタンを押す（`BN_CLICKED` が届く）。答えを先に返す |
//! | `POST /e2e/dialog/spin/<ID>/<回数>` | 上下の矢印を押す（`UDN_DELTAPOS` が届く） |
//! | `POST /e2e/dialog/key/<組み合わせ>` | ダイアログのループへキーを積む |
//! | `POST /e2e/dialog/close` | ダイアログの × を押す（`WM_CLOSE`） |
//! | `POST /e2e/dialog/dpichanged/<DPI>` | いちばん上のダイアログに `WM_DPICHANGED` を送り、そのあとのコントロールの JSON を返す |
//! | `GET /e2e/clipboard` | クリップボードの文字列の JSON |
//! | `POST /e2e/clipboard` | クリップボードを本文の文字で置き換える |
//! | `POST /e2e/message/<名前>`、`/e2e/message/<名前>/<wParam>` | ウィンドウにメッセージを送り、戻り値の JSON を返す（[`WindowMessage`]） |
//! | `POST /e2e/message/<名前>/<wParam>/in-borrow` | App を借りている間の処理の中からメッセージを送り、戻り値の JSON を返す（`queryendsession`、`endsession`、`close` だけ） |
//! | `POST /e2e/mouse` | 本文の各行のマウスの操作を、続けてウィンドウへ送る（[`MouseStep`]） |
//! | `GET /e2e/menu` | 開いているメニューの項目の JSON。サブメニューを開いていれば、そのサブメニューの項目（メニューへのキーも同じ） |
//! | `POST /e2e/menu/highlight/<位置>` | メニューのループへ上下の矢印のキーを送って、その項目へ移る |
//! | `POST /e2e/menu/select/<位置>` | 矢印のキーでその項目へ移り、Enter を送って選ぶ |
//! | `POST /e2e/menu/key/<キー>` | メニューのループへキーを送る（[`MenuKey`]） |
//! | `POST /e2e/menu/close` | 開いているメニューを閉じる |
//! | `POST /e2e/quit` | ウィンドウを閉じる |
//!
//! クリップボードは、環境変数 `FANTERMO_E2E_CLIPBOARD_FILE` があればそのファイル（テストの `Server` は既定でこれを
//! 渡す）、なければ OS のもの。

use std::{
    io::{BufRead, BufReader, ErrorKind, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use fantermo_core::keymap::KeyCombo;
use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};

/// 制御の口を開くフォルダを指定する環境変数。ポートの番号と絵のファイルをここに書く。
const DIR_ENV: &str = "FANTERMO_E2E_DIR";

/// ポートの番号とトークンを書くファイル。
const PORT_FILE: &str = "port";

/// 要求に付けるトークンのヘッダー。
const TOKEN_HEADER: &str = "X-Fantermo-Token";

/// トークンのバイト数（乱数）。
const TOKEN_BYTES: usize = 16;

/// ウィンドウのスレッドの答えを待つ上限。
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// 接続を読み書きする時間の上限。
const STREAM_TIMEOUT: Duration = Duration::from_secs(10);

/// 受け取る本文の上限。
const MAX_BODY_LEN: usize = 16 << 20;

/// `POST /e2e/message/dpichanged/<DPI>` と `POST /e2e/dialog/dpichanged/<DPI>` で受ける DPI の上限。Windows の表示の倍率の上限（500%）より少し大きい値。
const MAX_DPI: usize = 576;

const STATUS_OK: u16 = 200;
const STATUS_BAD_REQUEST: u16 = 400;
const STATUS_FORBIDDEN: u16 = 403;
const STATUS_NOT_FOUND: u16 = 404;
const STATUS_INTERNAL_ERROR: u16 = 500;
const STATUS_UNAVAILABLE: u16 = 503;

/// `POST /e2e/message/<名前>` で送れるメッセージ。名前は `WM_` を除いた小文字。`lParam` はウィンドウのスレッドの側で
/// メッセージごとに決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowMessage {
    QueryEndSession,
    EndSession,
    /// ほかのウィンドウがマウスをキャプチャした。
    CaptureChanged,
    /// DPI が変わった。`<wParam>` の位置に新しい DPI を書く。推奨の矩形はウィンドウの側で、Windows と同じく
    /// いまの大きさを DPI の比で拡大・縮小して作る。
    DpiChanged,
    /// DPI が変わる前に、変わったあとのウィンドウの大きさを問う。`<wParam>` の位置に新しい DPI を書く。答えの JSON に
    /// 戻り値の `result` と、ウィンドウプロシージャが書いた `width` と `height`（書かなければ 0）が入る。ウィンドウは
    /// 変えない。
    GetDpiScaledSize,
    /// ウィンドウを閉じる（× と同じ `WM_CLOSE`）。
    Close,
}

impl WindowMessage {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "queryendsession" => Some(Self::QueryEndSession),
            "endsession" => Some(Self::EndSession),
            "capturechanged" => Some(Self::CaptureChanged),
            "dpichanged" => Some(Self::DpiChanged),
            "getdpiscaledsize" => Some(Self::GetDpiScaledSize),
            "close" => Some(Self::Close),
            _ => None,
        }
    }

    /// `<wParam>` の位置に DPI を書くメッセージか。
    fn takes_dpi(self) -> bool {
        matches!(self, Self::DpiChanged | Self::GetDpiScaledSize)
    }

    /// App を借りている間の処理の中から送れるメッセージか。この場でウィンドウプロシージャに入り直すので、借用が
    /// 重なったときの処理の確認に使う。DPI のものは送る側で App を読むので送れない。
    fn sendable_in_borrow(self) -> bool {
        matches!(self, Self::QueryEndSession | Self::EndSession | Self::Close)
    }
}

/// マウスの操作の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Move,
    LeftDown,
    LeftUp,
    MiddleDown,
    MiddleUp,
    RightDown,
    RightUp,
    /// ホイールの回転量（奥へ回すと正。1 ノッチが 120）。
    Wheel(i16),
}

impl MouseAction {
    fn parse(text: &str) -> Option<Self> {
        if let Some(delta) = text.strip_prefix("wheel:") {
            return delta.parse().ok().map(Self::Wheel);
        }
        match text {
            "move" => Some(Self::Move),
            "left-down" => Some(Self::LeftDown),
            "left-up" => Some(Self::LeftUp),
            "middle-down" => Some(Self::MiddleDown),
            "middle-up" => Some(Self::MiddleUp),
            "right-down" => Some(Self::RightDown),
            "right-up" => Some(Self::RightUp),
            _ => None,
        }
    }
}

/// 操作のあいだ押しているボタンと修飾キー。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseHeld {
    pub left: bool,
    pub middle: bool,
    pub right: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// `POST /e2e/mouse` の本文の一行。形は `<操作> <x> <y> [押しているもの…]`。
///
/// 操作は `move`、`left-down`、`left-up`、`middle-down`、`middle-up`、`right-down`、`right-up`、`wheel:<回転量>`。
/// `x` と `y` はクライアント領域の座標で、ウィンドウの外を指す負の値も書ける。押しているものは `left`、`middle`、
/// `right`、`shift`、`ctrl`、`alt`。押す・離す操作の当のボタンは、押した直後・離した直後の状態になるので書かなくてよい。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseStep {
    pub action: MouseAction,
    pub x: i16,
    pub y: i16,
    pub held: MouseHeld,
}

impl MouseStep {
    fn parse(line: &str) -> Result<Self, Response> {
        let malformed = || Response::bad_request(format!("malformed mouse step: {line}"));
        let mut words = line.split_ascii_whitespace();
        let action = words
            .next()
            .and_then(MouseAction::parse)
            .ok_or_else(malformed)?;
        let x = parse_number(words.next().ok_or_else(malformed)?)?;
        let y = parse_number(words.next().ok_or_else(malformed)?)?;
        let mut held = MouseHeld::default();
        for word in words {
            match word {
                "left" => held.left = true,
                "middle" => held.middle = true,
                "right" => held.right = true,
                "shift" => held.shift = true,
                "ctrl" => held.ctrl = true,
                "alt" => held.alt = true,
                _ => return Err(malformed()),
            }
        }
        Ok(Self { action, x, y, held })
    }
}

/// `POST /e2e/menu/key/<キー>` で送れるキー。名前は `escape` か、英数字一文字（アクセラレータ）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKey {
    Escape,
    /// 大文字の英字か数字。仮想キーの番号はこの文字のコードと同じ。
    Accelerator(char),
}

impl MenuKey {
    fn from_name(name: &str) -> Option<Self> {
        if name == "escape" {
            return Some(Self::Escape);
        }
        let mut chars = name.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_alphanumeric() => {
                Some(Self::Accelerator(c.to_ascii_uppercase()))
            }
            _ => None,
        }
    }
}

/// ウィンドウのスレッドでする操作。
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    State,
    Frame,
    Input(Vec<u8>),
    Text(String),
    /// キーの組み合わせ `combo`。`unshifted` はそのキーが修飾なしで打つ文字で、なければ NUL。
    Key {
        combo: KeyCombo,
        unshifted: char,
    },
    NewTab,
    /// プロファイルの並びの中のこの位置のプロファイルで、新しいタブを開く。
    NewProfileTab(usize),
    CloseTab,
    SelectTab(usize),
    ResizeWindow {
        cols: u16,
        rows: u16,
    },
    /// ウィンドウを同じデスクトップの Z オーダーのいちばん下へ移す。
    LowerWindow,
    /// ウィンドウを最小化する。
    MinimizeWindow,
    /// 最小化したウィンドウを元に戻す。
    RestoreWindow,
    /// ウィンドウのスレッドをこのミリ秒だけ止める。
    BlockWindow(u64),
    Paste(String),
    PressDialogButton(i32),
    DialogControls,
    SetDialogText {
        id: i32,
        text: String,
    },
    SelectDialogItem {
        id: i32,
        position: usize,
    },
    CheckDialogRadio(i32),
    ClickDialogButton(i32),
    StepDialogSpin {
        id: i32,
        delta: i32,
    },
    PressDialogKey(KeyCombo),
    CloseDialog,
    /// いちばん上のダイアログに、DPI がこの値に変わったことを知らせる。
    ChangeDialogDpi(u32),
    ReadClipboard,
    WriteClipboard(String),
    SendMessage {
        message: WindowMessage,
        wparam: usize,
    },
    /// App を借りている間の処理の中から送る（[`WindowMessage::sendable_in_borrow`]）。
    SendMessageInBorrow {
        message: WindowMessage,
        wparam: usize,
    },
    Mouse(Vec<MouseStep>),
    MenuItems,
    HighlightMenuItem(usize),
    SelectMenuItem(usize),
    MenuKey(MenuKey),
    CloseMenu,
    Quit,
}

/// 操作の結果。
#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    /// 答えたあとで接続を閉じる。要求の続きを読めないときに立てる。
    close: bool,
}

impl Response {
    pub fn json(value: &serde_json::Value) -> Self {
        Self {
            status: STATUS_OK,
            content_type: "application/json",
            body: value.to_string().into_bytes(),
            close: false,
        }
    }

    pub fn png(bytes: Vec<u8>) -> Self {
        Self {
            status: STATUS_OK,
            content_type: "image/png",
            body: bytes,
            close: false,
        }
    }

    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::text(STATUS_INTERNAL_ERROR, message)
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::text(STATUS_BAD_REQUEST, message)
    }

    fn text(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: message.into().into_bytes(),
            close: false,
        }
    }

    fn closing(mut self) -> Self {
        self.close = true;
        self
    }
}

/// ウィンドウのスレッドが処理を待っている要求と、答えの送り先。
pub struct Pending {
    pub request: Request,
    pub reply: mpsc::Sender<Response>,
}

/// ウィンドウのスレッドが持つ、制御の口の受け取り側。
pub struct Control {
    pub requests: mpsc::Receiver<Pending>,
    /// 絵のファイルを書くフォルダ。
    pub dir: PathBuf,
}

/// 環境変数 [`DIR_ENV`] があれば制御の口を開く。要求が届くたびに `wake` を呼ぶ。
pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Option<Control> {
    let dir = PathBuf::from(std::env::var_os(DIR_ENV)?);
    let (sender, requests) = mpsc::channel();
    let port = match open(&dir, sender, wake) {
        Ok(port) => port,
        Err(e) => {
            tracing::error!("failed to open the e2e control in {}: {e}", dir.display());
            return None;
        }
    };
    tracing::info!("e2e control is listening on 127.0.0.1:{port}");
    Some(Control { requests, dir })
}

fn open(
    dir: &Path,
    sender: mpsc::Sender<Pending>,
    wake: impl Fn() + Send + Sync + 'static,
) -> std::io::Result<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let token = new_token()?;
    std::fs::create_dir_all(dir)?;
    // 読む側が書きかけを読まないよう、書き終えてから名前を付ける。
    let partial = dir.join(format!("{PORT_FILE}.partial"));
    std::fs::write(&partial, format!("{port}\n{token}\n"))?;
    std::fs::rename(&partial, dir.join(PORT_FILE))?;
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
    thread::Builder::new()
        .name("e2e-control".into())
        .spawn(move || serve(&listener, &sender, &wake, &token))?;
    Ok(port)
}

/// 乱数のトークン（16 進）。
fn new_token() -> std::io::Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    // SAFETY: bytes は書き込める配列で、アルゴリズムのハンドルを渡さずシステムの乱数を使う。
    unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
        .ok()
        .map_err(std::io::Error::other)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn serve(
    listener: &TcpListener,
    sender: &mpsc::Sender<Pending>,
    wake: &Arc<dyn Fn() + Send + Sync>,
    token: &str,
) {
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(e) => {
                tracing::warn!("e2e control failed to accept a connection: {e}");
                continue;
            }
        };
        let (sender, wake, token) = (sender.clone(), Arc::clone(wake), token.to_owned());
        let spawned = thread::Builder::new()
            .name("e2e-connection".into())
            .spawn(move || {
                if let Err(e) = handle_connection(stream, &sender, &*wake, &token) {
                    tracing::warn!("e2e control connection failed: {e}");
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("e2e control failed to start a connection thread: {e}");
        }
    }
}

/// 一つの接続の要求を、接続が閉じるか答えで閉じると決まるまで続けて処理する。
fn handle_connection(
    stream: TcpStream,
    sender: &mpsc::Sender<Pending>,
    wake: &dyn Fn(),
    token: &str,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(STREAM_TIMEOUT))?;
    stream.set_write_timeout(Some(STREAM_TIMEOUT))?;
    let mut reader = BufReader::new(stream);
    loop {
        let Some(read) = read_request(&mut reader, token)? else {
            return Ok(());
        };
        let response = match read {
            Ok(request) => run_on_window_thread(request, sender, wake),
            Err(response) => response,
        };
        write_response(reader.get_mut(), &response)?;
        if response.close {
            return Ok(());
        }
    }
}

/// `request` をウィンドウのスレッドへ渡し、答えを待つ。
fn run_on_window_thread(
    request: Request,
    sender: &mpsc::Sender<Pending>,
    wake: &dyn Fn(),
) -> Response {
    let (reply, answer) = mpsc::channel();
    if sender.send(Pending { request, reply }).is_err() {
        return Response::text(STATUS_UNAVAILABLE, "the window is closed");
    }
    wake();
    answer.recv_timeout(REPLY_TIMEOUT).unwrap_or_else(|e| {
        Response::text(
            STATUS_UNAVAILABLE,
            format!("the window thread did not answer: {e}"),
        )
    })
}

/// 接続から要求を一つ読む。次の要求が来ないまま相手が閉じたか、[`STREAM_TIMEOUT`] が過ぎたときは `None`。読めても
/// 受け付けないものは、返す答えを `Err` に入れて返す。`token` は付いているべき [`TOKEN_HEADER`] の値。
fn read_request(
    reader: &mut impl BufRead,
    token: &str,
) -> std::io::Result<Option<Result<Request, Response>>> {
    let mut request_line = String::new();
    match reader.read_line(&mut request_line) {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            return Ok(None);
        }
        Err(e) => return Err(e),
    }
    let mut parts = request_line.split_ascii_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        // 続きの区切りが分からないので閉じる。
        return Ok(Some(Err(
            Response::bad_request("malformed request line").closing()
        )));
    };

    let mut content_length = 0;
    let mut from_browser = false;
    let mut token_matches = false;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap_or(usize::MAX);
        }
        // ブラウザの中のページは `Origin` を付けて送る。開いているページからシェルへ入力させない。
        from_browser |= name.eq_ignore_ascii_case("origin");
        if name.eq_ignore_ascii_case(TOKEN_HEADER) {
            token_matches |= value.trim() == token;
        }
    }
    if content_length > MAX_BODY_LEN {
        // 本文を読み捨てられないので閉じる。
        return Ok(Some(Err(
            Response::bad_request("the body is too long").closing()
        )));
    }
    // 断る要求も本文を読み切ってから答える。次の要求の頭が分からなくなる。
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    if from_browser {
        return Ok(Some(Err(Response::text(
            STATUS_FORBIDDEN,
            "requests from browsers are refused",
        ))));
    }
    if !token_matches {
        return Ok(Some(Err(Response::text(
            STATUS_FORBIDDEN,
            format!("the {TOKEN_HEADER} header is missing or wrong"),
        ))));
    }
    Ok(Some(parse_request(method, path, body)))
}

/// メソッドとパスと本文から操作を決める。
fn parse_request(method: &str, path: &str, body: Vec<u8>) -> Result<Request, Response> {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method, segments.as_slice()) {
        ("GET", ["e2e", "state"]) => Ok(Request::State),
        ("GET", ["e2e", "frame"]) => Ok(Request::Frame),
        ("POST", ["e2e", "input"]) => Ok(Request::Input(body)),
        ("POST", ["e2e", "text"]) => parse_text(body).map(Request::Text),
        ("POST", ["e2e", "key", combo]) => parse_key(combo),
        ("POST", ["e2e", "tab", "new"]) => Ok(Request::NewTab),
        ("POST", ["e2e", "tab", "new", profile]) => {
            Ok(Request::NewProfileTab(parse_number(profile)?))
        }
        ("POST", ["e2e", "paste"]) => parse_text(body).map(Request::Paste),
        ("POST", ["e2e", "dialog", "button", id]) => {
            Ok(Request::PressDialogButton(parse_number(id)?))
        }
        ("GET", ["e2e", "dialog", "controls"]) => Ok(Request::DialogControls),
        ("POST", ["e2e", "dialog", "text", id]) => Ok(Request::SetDialogText {
            id: parse_number(id)?,
            text: parse_text(body)?,
        }),
        ("POST", ["e2e", "dialog", "select", id, position]) => Ok(Request::SelectDialogItem {
            id: parse_number(id)?,
            position: parse_number(position)?,
        }),
        ("POST", ["e2e", "dialog", "radio", id]) => {
            Ok(Request::CheckDialogRadio(parse_number(id)?))
        }
        ("POST", ["e2e", "dialog", "click", id]) => {
            Ok(Request::ClickDialogButton(parse_number(id)?))
        }
        ("POST", ["e2e", "dialog", "spin", id, delta]) => Ok(Request::StepDialogSpin {
            id: parse_number(id)?,
            delta: parse_number(delta)?,
        }),
        ("POST", ["e2e", "dialog", "key", combo]) => KeyCombo::parse(combo)
            .map(Request::PressDialogKey)
            .map_err(Response::bad_request),
        ("POST", ["e2e", "dialog", "close"]) => Ok(Request::CloseDialog),
        ("POST", ["e2e", "dialog", "dpichanged", dpi]) => {
            parse_dpi(dpi).map(Request::ChangeDialogDpi)
        }
        ("GET", ["e2e", "clipboard"]) => Ok(Request::ReadClipboard),
        ("POST", ["e2e", "clipboard"]) => parse_text(body).map(Request::WriteClipboard),
        ("POST", ["e2e", "tab", "close"]) => Ok(Request::CloseTab),
        ("POST", ["e2e", "tab", "select", index]) => Ok(Request::SelectTab(parse_number(index)?)),
        ("POST", ["e2e", "window", "size", cols, rows]) => Ok(Request::ResizeWindow {
            cols: parse_number(cols)?,
            rows: parse_number(rows)?,
        }),
        ("POST", ["e2e", "window", "lower"]) => Ok(Request::LowerWindow),
        ("POST", ["e2e", "window", "minimize"]) => Ok(Request::MinimizeWindow),
        ("POST", ["e2e", "window", "restore"]) => Ok(Request::RestoreWindow),
        ("POST", ["e2e", "window", "block", milliseconds]) => {
            Ok(Request::BlockWindow(parse_number(milliseconds)?))
        }
        ("POST", ["e2e", "message", name]) => parse_message(name, 0),
        ("POST", ["e2e", "message", name, wparam]) => parse_message(name, parse_number(wparam)?),
        ("POST", ["e2e", "message", name, wparam, "in-borrow"]) => {
            parse_message_in_borrow(name, parse_number(wparam)?)
        }
        ("POST", ["e2e", "mouse"]) => parse_mouse(body),
        ("GET", ["e2e", "menu"]) => Ok(Request::MenuItems),
        ("POST", ["e2e", "menu", "highlight", position]) => {
            Ok(Request::HighlightMenuItem(parse_number(position)?))
        }
        ("POST", ["e2e", "menu", "select", position]) => {
            Ok(Request::SelectMenuItem(parse_number(position)?))
        }
        ("POST", ["e2e", "menu", "key", name]) => MenuKey::from_name(name)
            .map(Request::MenuKey)
            .ok_or_else(|| Response::bad_request(format!("no such menu key: {name}"))),
        ("POST", ["e2e", "menu", "close"]) => Ok(Request::CloseMenu),
        ("POST", ["e2e", "quit"]) => Ok(Request::Quit),
        _ => Err(Response::text(
            STATUS_NOT_FOUND,
            format!("no such request: {method} {path}"),
        )),
    }
}

fn parse_text(body: Vec<u8>) -> Result<String, Response> {
    String::from_utf8(body).map_err(|e| Response::bad_request(e.to_string()))
}

fn parse_number<T: std::str::FromStr>(text: &str) -> Result<T, Response> {
    text.parse()
        .map_err(|_| Response::bad_request(format!("not a number: {text}")))
}

fn parse_message(name: &str, wparam: usize) -> Result<Request, Response> {
    let message = WindowMessage::from_name(name)
        .ok_or_else(|| Response::bad_request(format!("no such message: {name}")))?;
    if message.takes_dpi() {
        check_dpi(wparam)?;
    }
    Ok(Request::SendMessage { message, wparam })
}

/// ダイアログに知らせる DPI。
fn parse_dpi(text: &str) -> Result<u32, Response> {
    check_dpi(parse_number(text)?)
}

/// 受ける DPI か（1 から [`MAX_DPI`] まで）。
fn check_dpi(dpi: usize) -> Result<u32, Response> {
    if !(1..=MAX_DPI).contains(&dpi) {
        return Err(Response::bad_request(format!(
            "a DPI from 1 to {MAX_DPI} is expected: {dpi}"
        )));
    }
    Ok(dpi as u32)
}

fn parse_message_in_borrow(name: &str, wparam: usize) -> Result<Request, Response> {
    let message = WindowMessage::from_name(name)
        .ok_or_else(|| Response::bad_request(format!("no such message: {name}")))?;
    if !message.sendable_in_borrow() {
        return Err(Response::bad_request(format!(
            "{name} cannot be sent while the App is borrowed"
        )));
    }
    Ok(Request::SendMessageInBorrow { message, wparam })
}

fn parse_mouse(body: Vec<u8>) -> Result<Request, Response> {
    parse_text(body)?
        .lines()
        .map(MouseStep::parse)
        .collect::<Result<_, _>>()
        .map(Request::Mouse)
}

fn parse_key(text: &str) -> Result<Request, Response> {
    let combo = KeyCombo::parse(text).map_err(Response::bad_request)?;
    // 一文字の名前のキーは、その文字を打つキー。
    let name = text.rsplit('+').next().unwrap_or_default();
    let mut chars = name.chars();
    let unshifted = match (chars.next(), chars.next()) {
        (Some(c), None) => c.to_ascii_lowercase(),
        _ => '\0',
    };
    Ok(Request::Key { combo, unshifted })
}

fn write_response(stream: &mut TcpStream, response: &Response) -> std::io::Result<()> {
    let connection = if response.close {
        "close"
    } else {
        "keep-alive"
    };
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: {connection}\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    )?;
    stream.write_all(&response.body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use fantermo_core::vt::{Key, Mods};

    use super::*;

    const TEST_TOKEN: &str = "0123456789abcdef";

    /// 要求の先頭の行のあとに、正しいトークンのヘッダーを足して読む。
    fn parse(request: &str) -> Result<Request, Response> {
        let with_token =
            request.replacen("\r\n", &format!("\r\n{TOKEN_HEADER}: {TEST_TOKEN}\r\n"), 1);
        read_request(&mut Cursor::new(with_token.into_bytes()), TEST_TOKEN)
            .expect("readable request")
            .expect("a request")
    }

    #[test]
    fn body_is_read_by_its_length() {
        assert_eq!(
            parse("POST /e2e/input HTTP/1.1\r\nContent-Length: 2\r\n\r\nabcd"),
            Ok(Request::Input(b"ab".to_vec()))
        );
    }

    #[test]
    fn keys_carry_the_character_they_type() {
        assert_eq!(
            parse("POST /e2e/key/ctrl+c HTTP/1.1\r\n\r\n"),
            Ok(Request::Key {
                combo: KeyCombo::new(Mods::CTRL, Key::C),
                unshifted: 'c',
            })
        );
        assert_eq!(
            parse("POST /e2e/key/enter HTTP/1.1\r\n\r\n"),
            Ok(Request::Key {
                combo: KeyCombo::new(Mods::empty(), Key::Enter),
                unshifted: '\0',
            })
        );
    }

    #[test]
    fn tabs_and_window_sizes_take_numbers() {
        assert_eq!(
            parse("POST /e2e/tab/select/2 HTTP/1.1\r\n\r\n"),
            Ok(Request::SelectTab(2))
        );
        assert_eq!(
            parse("POST /e2e/window/size/80/24 HTTP/1.1\r\n\r\n"),
            Ok(Request::ResizeWindow { cols: 80, rows: 24 })
        );
        assert_eq!(
            parse("POST /e2e/tab/select/x HTTP/1.1\r\n\r\n")
                .unwrap_err()
                .status,
            STATUS_BAD_REQUEST
        );
    }

    #[test]
    fn profile_tabs_paste_dialogs_and_the_clipboard_are_requests() {
        assert_eq!(
            parse("POST /e2e/tab/new/1 HTTP/1.1\r\n\r\n"),
            Ok(Request::NewProfileTab(1))
        );
        assert_eq!(
            parse("POST /e2e/paste HTTP/1.1\r\nContent-Length: 3\r\n\r\na\nb"),
            Ok(Request::Paste("a\nb".to_owned()))
        );
        assert_eq!(
            parse("POST /e2e/dialog/button/1001 HTTP/1.1\r\n\r\n"),
            Ok(Request::PressDialogButton(1001))
        );
        assert_eq!(
            parse("GET /e2e/clipboard HTTP/1.1\r\n\r\n"),
            Ok(Request::ReadClipboard)
        );
        assert_eq!(
            parse("POST /e2e/clipboard HTTP/1.1\r\nContent-Length: 1\r\n\r\nx"),
            Ok(Request::WriteClipboard("x".to_owned()))
        );
    }

    #[test]
    fn window_messages_take_a_name_and_an_optional_wparam() {
        assert_eq!(
            parse("POST /e2e/message/queryendsession HTTP/1.1\r\n\r\n"),
            Ok(Request::SendMessage {
                message: WindowMessage::QueryEndSession,
                wparam: 0,
            })
        );
        assert_eq!(
            parse("POST /e2e/message/endsession/1 HTTP/1.1\r\n\r\n"),
            Ok(Request::SendMessage {
                message: WindowMessage::EndSession,
                wparam: 1,
            })
        );
        assert_eq!(
            parse("POST /e2e/message/dpichanged/144 HTTP/1.1\r\n\r\n"),
            Ok(Request::SendMessage {
                message: WindowMessage::DpiChanged,
                wparam: 144,
            })
        );
        assert_eq!(
            parse("POST /e2e/message/getdpiscaledsize/192 HTTP/1.1\r\n\r\n"),
            Ok(Request::SendMessage {
                message: WindowMessage::GetDpiScaledSize,
                wparam: 192,
            })
        );
        assert_eq!(
            parse("POST /e2e/message/close HTTP/1.1\r\n\r\n"),
            Ok(Request::SendMessage {
                message: WindowMessage::Close,
                wparam: 0,
            })
        );
        assert_eq!(
            parse("POST /e2e/message/endsession/1/in-borrow HTTP/1.1\r\n\r\n"),
            Ok(Request::SendMessageInBorrow {
                message: WindowMessage::EndSession,
                wparam: 1,
            })
        );
        for request in [
            "POST /e2e/message/destroy HTTP/1.1\r\n\r\n",
            "POST /e2e/message/endsession/x HTTP/1.1\r\n\r\n",
            "POST /e2e/message/queryendsession/in-borrow HTTP/1.1\r\n\r\n",
            "POST /e2e/message/dpichanged/144/in-borrow HTTP/1.1\r\n\r\n",
            "POST /e2e/message/capturechanged/0/in-borrow HTTP/1.1\r\n\r\n",
            "POST /e2e/message/dpichanged HTTP/1.1\r\n\r\n",
            "POST /e2e/message/dpichanged/0 HTTP/1.1\r\n\r\n",
            "POST /e2e/message/dpichanged/577 HTTP/1.1\r\n\r\n",
            "POST /e2e/message/getdpiscaledsize HTTP/1.1\r\n\r\n",
        ] {
            assert_eq!(parse(request).unwrap_err().status, STATUS_BAD_REQUEST);
        }
    }

    #[test]
    fn mouse_steps_are_read_line_by_line() {
        assert_eq!(
            parse(
                "POST /e2e/mouse HTTP/1.1\r\nContent-Length: 47\r\n\r\nleft-down 1 2\nmove -3 4 left alt\nwheel:-120 5 6"
            ),
            Ok(Request::Mouse(vec![
                MouseStep {
                    action: MouseAction::LeftDown,
                    x: 1,
                    y: 2,
                    held: MouseHeld::default(),
                },
                MouseStep {
                    action: MouseAction::Move,
                    x: -3,
                    y: 4,
                    held: MouseHeld {
                        left: true,
                        alt: true,
                        ..MouseHeld::default()
                    },
                },
                MouseStep {
                    action: MouseAction::Wheel(-120),
                    x: 5,
                    y: 6,
                    held: MouseHeld::default(),
                },
            ]))
        );
        for body in [
            "jump 1 2",
            "move 1",
            "move x 2",
            "move 1 2 meta",
            "wheel:x 1 2",
        ] {
            let request = format!(
                "POST /e2e/mouse HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            assert_eq!(parse(&request).unwrap_err().status, STATUS_BAD_REQUEST);
        }
    }

    #[test]
    fn menu_requests_take_a_position_or_a_key() {
        assert_eq!(
            parse("GET /e2e/menu HTTP/1.1\r\n\r\n"),
            Ok(Request::MenuItems)
        );
        assert_eq!(
            parse("POST /e2e/menu/highlight/3 HTTP/1.1\r\n\r\n"),
            Ok(Request::HighlightMenuItem(3))
        );
        assert_eq!(
            parse("POST /e2e/menu/select/3 HTTP/1.1\r\n\r\n"),
            Ok(Request::SelectMenuItem(3))
        );
        assert_eq!(
            parse("POST /e2e/menu/key/escape HTTP/1.1\r\n\r\n"),
            Ok(Request::MenuKey(MenuKey::Escape))
        );
        assert_eq!(
            parse("POST /e2e/menu/key/t HTTP/1.1\r\n\r\n"),
            Ok(Request::MenuKey(MenuKey::Accelerator('T')))
        );
        for request in [
            "POST /e2e/menu/select/x HTTP/1.1\r\n\r\n",
            "POST /e2e/menu/key/enter HTTP/1.1\r\n\r\n",
            "POST /e2e/menu/key/- HTTP/1.1\r\n\r\n",
        ] {
            assert_eq!(parse(request).unwrap_err().status, STATUS_BAD_REQUEST);
        }
    }

    #[test]
    fn dialog_control_requests_take_an_id_and_a_value() {
        assert_eq!(
            parse("GET /e2e/dialog/controls HTTP/1.1\r\n\r\n"),
            Ok(Request::DialogControls)
        );
        assert_eq!(
            parse("POST /e2e/dialog/text/201 HTTP/1.1\r\nContent-Length: 2\r\n\r\n14"),
            Ok(Request::SetDialogText {
                id: 201,
                text: "14".to_owned(),
            })
        );
        assert_eq!(
            parse("POST /e2e/dialog/select/100/3 HTTP/1.1\r\n\r\n"),
            Ok(Request::SelectDialogItem {
                id: 100,
                position: 3,
            })
        );
        assert_eq!(
            parse("POST /e2e/dialog/radio/211 HTTP/1.1\r\n\r\n"),
            Ok(Request::CheckDialogRadio(211))
        );
        assert_eq!(
            parse("POST /e2e/dialog/click/101 HTTP/1.1\r\n\r\n"),
            Ok(Request::ClickDialogButton(101))
        );
        assert_eq!(
            parse("POST /e2e/dialog/spin/202/-1 HTTP/1.1\r\n\r\n"),
            Ok(Request::StepDialogSpin { id: 202, delta: -1 })
        );
        assert_eq!(
            parse("POST /e2e/dialog/key/ctrl+n HTTP/1.1\r\n\r\n"),
            Ok(Request::PressDialogKey(KeyCombo::new(Mods::CTRL, Key::N)))
        );
        assert_eq!(
            parse("POST /e2e/dialog/close HTTP/1.1\r\n\r\n"),
            Ok(Request::CloseDialog)
        );
        assert_eq!(
            parse("POST /e2e/dialog/dpichanged/384 HTTP/1.1\r\n\r\n"),
            Ok(Request::ChangeDialogDpi(384))
        );
        for request in [
            "POST /e2e/dialog/text/x HTTP/1.1\r\n\r\n",
            "POST /e2e/dialog/select/100/x HTTP/1.1\r\n\r\n",
            "POST /e2e/dialog/spin/202/x HTTP/1.1\r\n\r\n",
            "POST /e2e/dialog/key/nothing HTTP/1.1\r\n\r\n",
            "POST /e2e/dialog/dpichanged/0 HTTP/1.1\r\n\r\n",
            "POST /e2e/dialog/dpichanged/577 HTTP/1.1\r\n\r\n",
            "POST /e2e/dialog/dpichanged/x HTTP/1.1\r\n\r\n",
        ] {
            assert_eq!(parse(request).unwrap_err().status, STATUS_BAD_REQUEST);
        }
    }

    #[test]
    fn unknown_paths_and_browsers_are_refused() {
        assert_eq!(
            parse("GET /e2e/input HTTP/1.1\r\n\r\n").unwrap_err().status,
            STATUS_NOT_FOUND
        );
        assert_eq!(
            parse("POST /e2e/input HTTP/1.1\r\nOrigin: http://example.com\r\n\r\n")
                .unwrap_err()
                .status,
            STATUS_FORBIDDEN
        );
    }

    #[test]
    fn requests_without_the_token_are_refused_and_the_connection_goes_on() {
        let first = "POST /e2e/input HTTP/1.1\r\nContent-Length: 2\r\n\r\nab";
        let wrong =
            "POST /e2e/input HTTP/1.1\r\nX-Fantermo-Token: wrong\r\nContent-Length: 1\r\n\r\nc";
        let right = "GET /e2e/state HTTP/1.1\r\nx-fantermo-token:  0123456789abcdef \r\n\r\n";
        let mut reader = Cursor::new(format!("{first}{wrong}{right}").into_bytes());
        let mut read = || read_request(&mut reader, TEST_TOKEN).expect("readable request");
        assert_eq!(read().unwrap().unwrap_err().status, STATUS_FORBIDDEN);
        assert_eq!(read().unwrap().unwrap_err().status, STATUS_FORBIDDEN);
        assert_eq!(read().unwrap(), Ok(Request::State));
        assert!(read().is_none());
    }

    #[test]
    fn requests_follow_one_another_on_a_connection() {
        let requests = format!(
            "POST /e2e/input HTTP/1.1\r\n{TOKEN_HEADER}: {TEST_TOKEN}\r\nContent-Length: 2\r\n\r\nab\
             GET /e2e/state HTTP/1.1\r\n{TOKEN_HEADER}: {TEST_TOKEN}\r\n\r\n"
        );
        let mut reader = Cursor::new(requests.into_bytes());
        let mut read = || read_request(&mut reader, TEST_TOKEN).expect("readable request");
        assert_eq!(read(), Some(Ok(Request::Input(b"ab".to_vec()))));
        assert_eq!(read(), Some(Ok(Request::State)));
        assert_eq!(read(), None);
    }

    #[test]
    fn a_body_that_is_too_long_closes_the_connection() {
        let request = format!(
            "POST /e2e/input HTTP/1.1\r\n{TOKEN_HEADER}: {TEST_TOKEN}\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_LEN + 1
        );
        let response = read_request(&mut Cursor::new(request.into_bytes()), TEST_TOKEN)
            .expect("readable request")
            .expect("a request")
            .unwrap_err();
        assert_eq!(response.status, STATUS_BAD_REQUEST);
        assert!(response.close);
    }
}
