//! e2e テストが、更新の取得先・公開鍵・いまのバージョン・スクリプトの起動の代わりに使うもの。
//!
//! feature `e2e` のときだけコンパイルする。環境変数で更新の流れを変えられるのはこのモジュールだけで、配布物には
//! 入らない。逆に、このモジュールを持つビルドは `127.0.0.1` のほかへ接続せず、スクリプトを起動しない。

use std::{ffi::OsString, io, path::Path};

use windows::{Win32::Foundation::E_FAIL, core::Error};

use super::http::{self, HttpError};

/// 更新の情報と配布物を返す HTTP の `127.0.0.1` のポート。あれば、インストーラで入れたものとして扱う（管理者として
/// 動いていなければ「更新を確認」を押せる）。
const PORT_ENV: &str = "FANTERMO_E2E_UPDATE_PORT";
/// 署名を検証する公開鍵（SubjectPublicKeyInfo の DER）を 16 進で。なければ埋め込みの公開鍵を使う。
const PUBLIC_KEY_ENV: &str = "FANTERMO_E2E_UPDATE_PUBLIC_KEY";
/// いまのバージョンとして扱う文字列。
const CURRENT_VERSION_ENV: &str = "FANTERMO_E2E_UPDATE_CURRENT_VERSION";
/// スクリプトを起動する代わりに、そのパスを書くファイル。なければ起動に失敗したことにする。
const HANDOFF_RECORD_ENV: &str = "FANTERMO_E2E_UPDATE_HANDOFF_RECORD";

/// 1 バイトを書く 16 進の桁数。
const HEX_DIGITS_PER_BYTE: usize = 2;
const HEX_RADIX: u32 = 16;

/// テストが取得先を指定しているか。
pub fn is_active() -> bool {
    std::env::var_os(PORT_ENV).is_some()
}

/// テストの HTTP から `path` を取得する。取得先の指定がなければ、接続せずに失敗する。
pub fn get(path: &str, max_bytes: usize) -> Result<Vec<u8>, HttpError> {
    let port = std::env::var(PORT_ENV)
        .ok()
        .and_then(|port| port.parse().ok())
        .ok_or_else(|| {
            HttpError::Request(Error::new(E_FAIL, "no port to fetch the update from"))
        })?;
    http::get_loopback(port, path, max_bytes)
}

/// テストが指定した公開鍵。読めなければ警告して、指定がないものとして扱う。
pub fn public_key() -> Option<Vec<u8>> {
    let text = std::env::var(PUBLIC_KEY_ENV).ok()?;
    let key = decode_hex(&text);
    if key.is_none() {
        tracing::warn!("{PUBLIC_KEY_ENV} is not hex");
    }
    key
}

/// テストが指定した、いまのバージョン。
pub fn current_version() -> Option<String> {
    std::env::var(CURRENT_VERSION_ENV).ok()
}

/// スクリプトを起動する代わりに、起動しようとしたことをファイルに残す。
pub fn launch(script: &Path) -> io::Result<()> {
    let record: OsString = std::env::var_os(HANDOFF_RECORD_ENV).ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "no file to record the hand-off in")
    })?;
    std::fs::write(record, script.to_string_lossy().as_bytes())
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u32> = text
        .chars()
        .map(|digit| digit.to_digit(HEX_RADIX))
        .collect::<Option<_>>()?;
    let (pairs, rest) = digits.as_chunks::<HEX_DIGITS_PER_BYTE>();
    rest.is_empty().then(|| {
        pairs
            .iter()
            .map(|&[high, low]| (high * HEX_RADIX + low) as u8)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_is_decoded_into_bytes() {
        assert_eq!(decode_hex("00ffA1"), Some(vec![0x00, 0xff, 0xa1]));
        assert_eq!(decode_hex(""), Some(vec![]));
        assert_eq!(decode_hex("0"), None);
        assert_eq!(decode_hex("0g"), None);
    }
}
