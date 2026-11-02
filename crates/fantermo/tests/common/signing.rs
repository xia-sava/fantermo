//! 更新の情報に署名する、テストの実行中に作る P-256 の鍵（Windows の CNG）。
//!
//! 公開鍵と署名の形は `tools/manifest.sh` と同じ。公開鍵は X.509 の SubjectPublicKeyInfo の DER、署名は
//! SHA256withECDSA の DER を base64 にした 1 行。鍵はメモリの中にしかなく、ファイルに書かない。

use windows::{
    Win32::Security::Cryptography::{
        BCRYPT_ALG_HANDLE, BCRYPT_ECCPUBLIC_BLOB, BCRYPT_ECDSA_P256_ALGORITHM, BCRYPT_FLAGS,
        BCRYPT_KEY_HANDLE, BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS, BCRYPT_SHA256_ALGORITHM,
        BCryptCloseAlgorithmProvider, BCryptDestroyKey, BCryptExportKey, BCryptFinalizeKeyPair,
        BCryptGenerateKeyPair, BCryptHash, BCryptOpenAlgorithmProvider, BCryptSignHash,
    },
    core::PCWSTR,
};

use super::base64;

/// P-256 の鍵の長さ（ビット）。
const KEY_BITS: u32 = 256;
/// P-256 の座標の長さ（バイト）。
const COORDINATE_LEN: usize = 32;
/// SHA-256 の長さ（バイト）。
const SHA256_LEN: usize = 32;
/// CNG の公開鍵の形で、X と Y の手前の `BCRYPT_ECCKEY_BLOB` の長さ。
const ECC_KEY_BLOB_HEADER_LEN: usize = 8;
/// P-256 の公開鍵の SubjectPublicKeyInfo の、点の手前まで。
const SPKI_P256_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];
/// 圧縮しない点の印。
const UNCOMPRESSED_POINT: u8 = 0x04;
/// DER の SEQUENCE と INTEGER の印。
const DER_SEQUENCE: u8 = 0x30;
const DER_INTEGER: u8 = 0x02;

/// テストの実行中に作った鍵。
pub struct SigningKey {
    algorithm: BCRYPT_ALG_HANDLE,
    key: BCRYPT_KEY_HANDLE,
}

// SAFETY: CNG のハンドルはスレッドに結び付かない。
unsafe impl Send for SigningKey {}
// SAFETY: 同上。署名と書き出しは鍵を変えない。
unsafe impl Sync for SigningKey {}

impl SigningKey {
    /// 新しい鍵を作る。
    pub fn generate() -> Self {
        let mut algorithm = BCRYPT_ALG_HANDLE::default();
        let mut key = BCRYPT_KEY_HANDLE::default();
        // SAFETY: 書き込み先は書き込める。名前は CNG の決めたもの。
        unsafe {
            BCryptOpenAlgorithmProvider(
                &mut algorithm,
                BCRYPT_ECDSA_P256_ALGORITHM,
                PCWSTR::null(),
                BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
            )
            .ok()
            .expect("cannot open ECDSA P-256");
            BCryptGenerateKeyPair(algorithm, &mut key, KEY_BITS, 0)
                .ok()
                .expect("cannot generate a key");
            BCryptFinalizeKeyPair(key, 0)
                .ok()
                .expect("cannot finish the key");
        }
        Self { algorithm, key }
    }

    /// 公開鍵（SubjectPublicKeyInfo の DER）。
    pub fn public_key(&self) -> Vec<u8> {
        let mut blob = [0u8; ECC_KEY_BLOB_HEADER_LEN + 2 * COORDINATE_LEN];
        let mut written = 0u32;
        // SAFETY: key は作った鍵で、blob は書き込める。
        unsafe {
            BCryptExportKey(
                self.key,
                None,
                BCRYPT_ECCPUBLIC_BLOB,
                Some(&mut blob),
                &mut written,
                0,
            )
        }
        .ok()
        .expect("cannot export the public key");
        assert_eq!(written as usize, blob.len());
        let mut spki = SPKI_P256_PREFIX.to_vec();
        spki.push(UNCOMPRESSED_POINT);
        spki.extend_from_slice(&blob[ECC_KEY_BLOB_HEADER_LEN..]);
        spki
    }

    /// `message` の署名（DER の署名を base64 にした 1 行。`tools/manifest.sh sign` と同じく改行を付けない）。
    pub fn sign(&self, message: &[u8]) -> String {
        let hash = sha256(message);
        let mut raw = [0u8; 2 * COORDINATE_LEN];
        let mut written = 0u32;
        // SAFETY: key は作った鍵で、hash と raw はこの呼び出しの間は生きている。ECDSA はパディングを使わない。
        unsafe {
            BCryptSignHash(
                self.key,
                None,
                &hash,
                Some(&mut raw),
                &mut written,
                BCRYPT_FLAGS(0),
            )
        }
        .ok()
        .expect("cannot sign");
        assert_eq!(written as usize, raw.len());
        let (r, s) = raw.split_at(COORDINATE_LEN);
        base64(&der_signature(r, s))
    }
}

impl Drop for SigningKey {
    fn drop(&mut self) {
        // SAFETY: generate で作ったハンドルで、ほかでは閉じない。
        unsafe {
            if BCryptDestroyKey(self.key).is_err() {
                eprintln!("failed to destroy a CNG key");
            }
            if BCryptCloseAlgorithmProvider(self.algorithm, 0).is_err() {
                eprintln!("failed to close a CNG algorithm");
            }
        }
    }
}

/// `data` の SHA-256。
pub fn sha256(data: &[u8]) -> [u8; SHA256_LEN] {
    let mut algorithm = BCRYPT_ALG_HANDLE::default();
    let mut hash = [0; SHA256_LEN];
    // SAFETY: 書き込み先は書き込める。名前は CNG の決めたもので、開いたハンドルは最後に閉じる。
    unsafe {
        BCryptOpenAlgorithmProvider(
            &mut algorithm,
            BCRYPT_SHA256_ALGORITHM,
            PCWSTR::null(),
            BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
        )
        .ok()
        .expect("cannot open SHA-256");
        BCryptHash(algorithm, None, data, &mut hash)
            .ok()
            .expect("cannot hash");
        if BCryptCloseAlgorithmProvider(algorithm, 0).is_err() {
            eprintln!("failed to close a CNG algorithm");
        }
    }
    hash
}

/// `data` の SHA-256 を小文字の 16 進で。`sha256sum` と同じ形。
pub fn sha256_hex(data: &[u8]) -> String {
    hex(&sha256(data))
}

/// `bytes` を小文字の 16 進にする。
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `r` と `s`（符号なしの大きな数）から DER の署名を作る。
pub fn der_signature(r: &[u8], s: &[u8]) -> Vec<u8> {
    let mut body = der_integer(r);
    body.extend(der_integer(s));
    let mut der = vec![
        DER_SEQUENCE,
        u8::try_from(body.len()).expect("a P-256 signature is short"),
    ];
    der.extend(body);
    der
}

/// 先頭の 0 を除き、最上位のビットが立っていれば 0 を一つ置いた DER の INTEGER。
fn der_integer(value: &[u8]) -> Vec<u8> {
    let start = value
        .iter()
        .position(|&byte| byte != 0)
        .unwrap_or(value.len());
    let mut digits = value[start..].to_vec();
    if digits.first().is_none_or(|first| first & 0x80 != 0) {
        digits.insert(0, 0);
    }
    let mut integer = vec![
        DER_INTEGER,
        u8::try_from(digits.len()).expect("a P-256 number is short"),
    ];
    integer.extend(digits);
    integer
}
