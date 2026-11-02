//! e2e テストの制御の口のうち、ウィンドウに触らない側（feature `e2e` のときだけコンパイルする）。
//!
//! 環境変数 [`DIR_ENV`] があるときだけ、`127.0.0.1` の OS が割り当てたポートで小さな HTTP を受け、ポートの番号を
//! そのフォルダの [`PORT_FILE`] に書く。受けた要求はチャネルでウィンドウのスレッドへ渡し、答えを待って返す。
//! ウィンドウ側の処理は `window::e2e`。
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
//! | `POST /e2e/paste` | 本文の文字を、クリップボードから読んだものとして貼り付ける |
//! | `POST /e2e/dialog/button/<ID>` | 開いているダイアログのそのボタンを押す |
//! | `GET /e2e/clipboard` | OS のクリップボードの文字列の JSON |
//! | `POST /e2e/clipboard` | OS のクリップボードを本文の文字で置き換える |
//! | `POST /e2e/quit` | ウィンドウを閉じる |

use std::{
    io::{BufRead, BufReader, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

use fantermo_core::keymap::KeyCombo;

/// 制御の口を開くフォルダを指定する環境変数。ポートの番号と絵のファイルをここに書く。
const DIR_ENV: &str = "FANTERMO_E2E_DIR";

/// ポートの番号を書くファイル。
const PORT_FILE: &str = "port";

/// ウィンドウのスレッドの答えを待つ上限。
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// 接続を読み書きする時間の上限。
const STREAM_TIMEOUT: Duration = Duration::from_secs(10);

/// 受け取る本文の上限。
const MAX_BODY_LEN: usize = 16 << 20;

const STATUS_OK: u16 = 200;
const STATUS_BAD_REQUEST: u16 = 400;
const STATUS_FORBIDDEN: u16 = 403;
const STATUS_NOT_FOUND: u16 = 404;
const STATUS_INTERNAL_ERROR: u16 = 500;
const STATUS_UNAVAILABLE: u16 = 503;

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
    Paste(String),
    PressDialogButton(i32),
    ReadClipboard,
    WriteClipboard(String),
    Quit,
}

/// 操作の結果。
#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    pub fn json(value: &serde_json::Value) -> Self {
        Self {
            status: STATUS_OK,
            content_type: "application/json",
            body: value.to_string().into_bytes(),
        }
    }

    pub fn png(bytes: Vec<u8>) -> Self {
        Self {
            status: STATUS_OK,
            content_type: "image/png",
            body: bytes,
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
        }
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
pub fn start(wake: impl Fn() + Send + 'static) -> Option<Control> {
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
    wake: impl Fn() + Send + 'static,
) -> std::io::Result<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    std::fs::create_dir_all(dir)?;
    // 読む側が書きかけを読まないよう、書き終えてから名前を付ける。
    let partial = dir.join(format!("{PORT_FILE}.partial"));
    std::fs::write(&partial, port.to_string())?;
    std::fs::rename(&partial, dir.join(PORT_FILE))?;
    thread::Builder::new()
        .name("e2e-control".into())
        .spawn(move || serve(&listener, &sender, &wake))?;
    Ok(port)
}

fn serve(listener: &TcpListener, sender: &mpsc::Sender<Pending>, wake: &dyn Fn()) {
    for stream in listener.incoming() {
        let result = stream.and_then(|stream| handle_connection(stream, sender, wake));
        if let Err(e) = result {
            tracing::warn!("e2e control connection failed: {e}");
        }
    }
}

fn handle_connection(
    stream: TcpStream,
    sender: &mpsc::Sender<Pending>,
    wake: &dyn Fn(),
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(STREAM_TIMEOUT))?;
    stream.set_write_timeout(Some(STREAM_TIMEOUT))?;
    let mut reader = BufReader::new(stream);
    let response = match read_request(&mut reader)? {
        Ok(request) => run_on_window_thread(request, sender, wake),
        Err(response) => response,
    };
    write_response(reader.get_mut(), &response)
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

/// 接続から要求を一つ読む。読めても受け付けないものは、返す答えを `Err` に入れて返す。
fn read_request(reader: &mut impl BufRead) -> std::io::Result<Result<Request, Response>> {
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_ascii_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return Ok(Err(Response::bad_request("malformed request line")));
    };

    let mut content_length = 0;
    let mut from_browser = false;
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
    }
    if from_browser {
        return Ok(Err(Response::text(
            STATUS_FORBIDDEN,
            "requests from browsers are refused",
        )));
    }
    if content_length > MAX_BODY_LEN {
        return Ok(Err(Response::bad_request("the body is too long")));
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    Ok(parse_request(method, path, body))
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
        ("GET", ["e2e", "clipboard"]) => Ok(Request::ReadClipboard),
        ("POST", ["e2e", "clipboard"]) => parse_text(body).map(Request::WriteClipboard),
        ("POST", ["e2e", "tab", "close"]) => Ok(Request::CloseTab),
        ("POST", ["e2e", "tab", "select", index]) => Ok(Request::SelectTab(parse_number(index)?)),
        ("POST", ["e2e", "window", "size", cols, rows]) => Ok(Request::ResizeWindow {
            cols: parse_number(cols)?,
            rows: parse_number(rows)?,
        }),
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
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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

    fn parse(request: &str) -> Result<Request, Response> {
        read_request(&mut Cursor::new(request.as_bytes().to_vec())).expect("readable request")
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
}
