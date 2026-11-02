//! 更新の情報と配布物を返す、`127.0.0.1` の小さな HTTP。fantermo が取得した順番を覚える。

use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{Ipv4Addr, Shutdown, TcpListener, TcpStream},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

/// 要求を読み書きする上限。
const SOCKET_TIMEOUT: Duration = Duration::from_secs(90);
/// 大きな本体を一度に書く大きさ。
const ZEROS_CHUNK: usize = 64 * 1024;
/// 要求のヘッダーを読む上限。
const REQUEST_MAX_BYTES: usize = 16 * 1024;
const HEADER_END: &[u8] = b"\r\n\r\n";

/// 一つの要求への答え。
#[derive(Clone)]
pub enum Reply {
    /// 200 と本体。
    Body(Arc<[u8]>),
    /// 成功でないステータスと、短い本体。
    Status(u16),
    /// 200 と、`len` バイトの 0。読むのをやめられたら書くのをやめる。
    Zeros(usize),
    /// 答えずに接続を閉じる。
    Close,
    /// ヘッダーで `claimed` バイトと言って、`body` だけ送って閉じる。
    Truncated { claimed: usize, body: Arc<[u8]> },
    /// 接続を受けたまま、host を破棄するまで答えない。
    Silent,
    /// `gate` が開くまで待ってから `reply` を返す。
    After { gate: Arc<Gate>, reply: Box<Reply> },
}

impl Reply {
    pub fn body(bytes: impl AsRef<[u8]>) -> Self {
        Self::Body(Arc::from(bytes.as_ref()))
    }
}

/// テストが開くまで答えを止める。
#[derive(Default)]
pub struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    pub fn open(&self) {
        *self.open.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        while !*open {
            open = self.changed.wait(open).unwrap_or_else(|e| e.into_inner());
        }
    }
}

#[derive(Default)]
struct Shared {
    routes: Mutex<HashMap<String, Reply>>,
    requests: Mutex<Vec<String>>,
    /// 破棄したら開く。`Reply::Silent` がこれを待つ。
    closing: Gate,
}

/// 更新の情報と配布物を返す HTTP。破棄すると受け付けをやめる。
pub struct ReleaseHost {
    port: u16,
    shared: Arc<Shared>,
}

impl ReleaseHost {
    pub fn start() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("cannot listen");
        let port = listener.local_addr().expect("no local address").port();
        let shared = Arc::new(Shared::default());
        let accepting = Arc::clone(&shared);
        thread::spawn(move || accept_loop(&listener, &accepting));
        Self { port, shared }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// `path` への答えを `reply` にする。
    pub fn set(&self, path: &str, reply: Reply) {
        self.shared
            .routes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.to_owned(), reply);
    }

    /// `path` への答えをなくす（404 を返す）。
    pub fn remove(&self, path: &str) {
        self.shared
            .routes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(path);
    }

    /// 受けた要求のパスを順に返す。
    pub fn requests(&self) -> Vec<String> {
        self.shared
            .requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 受けた要求の記録を消す。
    pub fn clear_requests(&self) {
        self.shared
            .requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl Drop for ReleaseHost {
    fn drop(&mut self) {
        self.shared.closing.open();
        // accept を起こす。
        if let Err(e) = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port)) {
            eprintln!("failed to wake the release host: {e}");
        }
    }
}

fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) {
    for stream in listener.incoming() {
        if *shared
            .closing
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
        {
            return;
        }
        let stream = match stream {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!("the release host failed to accept: {e}");
                continue;
            }
        };
        let shared = Arc::clone(shared);
        thread::spawn(move || serve(stream, &shared));
    }
}

fn serve(mut stream: TcpStream, shared: &Shared) {
    if let Err(e) = stream
        .set_read_timeout(Some(SOCKET_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(SOCKET_TIMEOUT)))
    {
        eprintln!("the release host failed to set the timeouts: {e}");
        return;
    }
    let Some(path) = read_request_path(&mut stream) else {
        return;
    };
    shared
        .requests
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(path.clone());
    let reply = shared
        .routes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&path)
        .cloned()
        .unwrap_or(Reply::Status(404));
    // 相手が先に閉じるのは、大きすぎる応答を読むのをやめたときなど、確認している動きなので残さない。
    let _ = send(&mut stream, &reply, shared);
    let _ = stream.shutdown(Shutdown::Both);
}

fn read_request_path(stream: &mut TcpStream) -> Option<String> {
    let mut request = Vec::new();
    let mut chunk = [0u8; 1024];
    while !request.windows(HEADER_END.len()).any(|w| w == HEADER_END) {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 || request.len() > REQUEST_MAX_BYTES {
            return None;
        }
        request.extend_from_slice(&chunk[..read]);
    }
    let text = String::from_utf8_lossy(&request);
    text.split_ascii_whitespace().nth(1).map(str::to_owned)
}

fn send(stream: &mut TcpStream, reply: &Reply, shared: &Shared) -> std::io::Result<()> {
    match reply {
        Reply::Body(body) => {
            write_head(stream, 200, body.len())?;
            stream.write_all(body)
        }
        Reply::Status(status) => {
            let body = format!("status {status}");
            write_head(stream, *status, body.len())?;
            stream.write_all(body.as_bytes())
        }
        Reply::Zeros(len) => {
            write_head(stream, 200, *len)?;
            let chunk = vec![0u8; ZEROS_CHUNK];
            let mut left = *len;
            while left > 0 {
                let size = left.min(ZEROS_CHUNK);
                stream.write_all(&chunk[..size])?;
                left -= size;
            }
            Ok(())
        }
        Reply::Close => Ok(()),
        Reply::Truncated { claimed, body } => {
            write_head(stream, 200, *claimed)?;
            stream.write_all(body)
        }
        Reply::Silent => {
            shared.closing.wait();
            Ok(())
        }
        Reply::After { gate, reply } => {
            gate.wait();
            send(stream, reply, shared)
        }
    }
}

fn write_head(stream: &mut TcpStream, status: u16, length: usize) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} E2E\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
    )
}

/// 空いているポートの番号。受け付けている者はいない。
pub fn unused_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .expect("cannot find a free port")
        .port()
}
