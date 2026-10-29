//! HTTPS で取得する（WinHTTP の同期の呼び出し）。窓のスレッドからは呼ばない。

use std::{ffi::c_void, fmt};

use windows::{
    Win32::Networking::WinHttp::{
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen,
        WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
        WinHttpSendRequest, WinHttpSetTimeouts,
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::version;

/// HTTPS のポート。
const HTTPS_PORT: u16 = 443;
/// 成功の応答のステータスコード。リダイレクトは WinHTTP が追う。
const HTTP_OK: u32 = 200;
/// 名前の解決・接続・送信・受信のそれぞれを待つ上限（ミリ秒）。受信は 1 回の読み出しごと。
const RESOLVE_TIMEOUT_MS: i32 = 10_000;
const CONNECT_TIMEOUT_MS: i32 = 10_000;
const SEND_TIMEOUT_MS: i32 = 30_000;
const RECEIVE_TIMEOUT_MS: i32 = 30_000;
/// 1 回に読み出す大きさ。
const READ_CHUNK: usize = 64 * 1024;

/// 取得できなかったわけ。
#[derive(Debug)]
pub enum HttpError {
    /// WinHTTP の呼び出しが失敗した（名前の解決・接続・TLS・時間切れなど）。
    Request(windows::core::Error),
    /// 成功でない応答。ステータスコードを添える。
    Status(u32),
    /// 応答の本体が上限より大きい。
    TooLarge,
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(e) => write!(f, "{e}"),
            Self::Status(status) => write!(f, "HTTP {status}"),
            Self::TooLarge => write!(f, "the response is too large"),
        }
    }
}

/// `https://<host><path>` を GET し、本体を返す。本体が `max_bytes` を超えたら読むのをやめて失敗する。
pub fn get(host: &str, path: &str, max_bytes: usize) -> Result<Vec<u8>, HttpError> {
    let agent = HSTRING::from(format!("fantermo/{}", version::display()));
    // SAFETY: 文字列はこの呼び出しの間は生きている。プロキシは Windows の設定に従う。
    let session = Handle::new(unsafe {
        WinHttpOpen(
            &agent,
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        )
    })?;
    // SAFETY: session は開いたハンドル。
    unsafe {
        WinHttpSetTimeouts(
            session.0,
            RESOLVE_TIMEOUT_MS,
            CONNECT_TIMEOUT_MS,
            SEND_TIMEOUT_MS,
            RECEIVE_TIMEOUT_MS,
        )
    }
    .map_err(HttpError::Request)?;
    // SAFETY: session は開いたハンドルで、文字列はこの呼び出しの間は生きている。
    let connection =
        Handle::new(unsafe { WinHttpConnect(session.0, &HSTRING::from(host), HTTPS_PORT, 0) })?;
    // SAFETY: connection は開いたハンドルで、文字列はこの呼び出しの間は生きている。受け付ける種類は
    // 指定しない。
    let request = Handle::new(unsafe {
        WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        )
    })?;
    // SAFETY: request は開いたハンドル。足すヘッダーも本体もない。
    unsafe {
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(HttpError::Request)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(HttpError::Request)?;
    }
    let status = status_code(&request)?;
    if status != HTTP_OK {
        return Err(HttpError::Status(status));
    }
    read_body(&request, max_bytes)
}

fn status_code(request: &Handle) -> Result<u32, HttpError> {
    let mut status = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: request は応答を受けたハンドルで、status は size バイトを書き込める。
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(std::ptr::from_mut(&mut status).cast()),
            &mut size,
            std::ptr::null_mut(),
        )
    }
    .map_err(HttpError::Request)?;
    Ok(status)
}

fn read_body(request: &Handle, max_bytes: usize) -> Result<Vec<u8>, HttpError> {
    let mut body = Vec::new();
    let mut chunk = vec![0u8; READ_CHUNK];
    loop {
        let mut read = 0u32;
        // SAFETY: request は応答を受けたハンドルで、chunk は READ_CHUNK バイトを書き込める。
        unsafe {
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                READ_CHUNK as u32,
                &mut read,
            )
        }
        .map_err(HttpError::Request)?;
        if read == 0 {
            return Ok(body);
        }
        if body.len() + read as usize > max_bytes {
            return Err(HttpError::TooLarge);
        }
        body.extend_from_slice(&chunk[..read as usize]);
    }
}

/// WinHTTP のハンドル。破棄すると閉じる。
struct Handle(*mut c_void);

impl Handle {
    /// 開いた結果のハンドル。null なら失敗として、スレッドの最後のエラーを返す。
    fn new(handle: *mut c_void) -> Result<Self, HttpError> {
        if handle.is_null() {
            return Err(HttpError::Request(windows::core::Error::from_thread()));
        }
        Ok(Self(handle))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: 開いたハンドルで、ほかでは閉じない。開いた順の逆に破棄される。
        if let Err(e) = unsafe { WinHttpCloseHandle(self.0) } {
            tracing::warn!("failed to close a WinHTTP handle: {e}");
        }
    }
}
