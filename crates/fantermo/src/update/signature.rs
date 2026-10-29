//! 更新の情報（`latest.json`）の署名の検証と、配布物の SHA-256。Windows の CNG を使う。
//!
//! 署名は ECDSA P-256（SHA256withECDSA）で、DER の署名を base64 にした 1 行。公開鍵は X.509 の
//! SubjectPublicKeyInfo の DER（`tools/manifest.sh` が使うものと同じ）。

use windows::{
    Win32::Security::Cryptography::{
        BCRYPT_ALG_HANDLE, BCRYPT_ECCPUBLIC_BLOB, BCRYPT_ECDSA_P256_ALGORITHM,
        BCRYPT_ECDSA_PUBLIC_P256_MAGIC, BCRYPT_FLAGS, BCRYPT_KEY_HANDLE,
        BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS, BCRYPT_SHA256_ALGORITHM,
        BCryptCloseAlgorithmProvider, BCryptDestroyKey, BCryptHash, BCryptImportKeyPair,
        BCryptOpenAlgorithmProvider, BCryptVerifySignature, CRYPT_STRING_BASE64,
        CryptStringToBinaryW,
    },
    core::PCWSTR,
};

/// P-256 の座標の長さ（バイト）。
const COORDINATE_LEN: usize = 32;
/// SHA-256 の長さ（バイト）。
pub const SHA256_LEN: usize = 32;

/// P-256 の公開鍵の SubjectPublicKeyInfo の、点の手前まで。アルゴリズムは id-ecPublicKey、曲線は
/// prime256v1、続く BIT STRING は 66 バイト（未使用のビット数 0 と、圧縮しない点 65 バイト）。
const SPKI_P256_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];
/// 圧縮しない点の印。後ろに X と Y が続く。
const UNCOMPRESSED_POINT: u8 = 0x04;

/// DER の SEQUENCE と INTEGER の印。
const DER_SEQUENCE: u8 = 0x30;
const DER_INTEGER: u8 = 0x02;
/// DER の長さを 1 バイトで書ける上限。P-256 の署名はこれに収まる。
const DER_SHORT_LENGTH_LIMIT: u8 = 0x80;

/// 署名を確かめられなかったわけ。利用者には区別して見せず、ログに残す。
#[derive(Debug)]
pub enum VerifyError {
    /// 公開鍵が P-256 の SubjectPublicKeyInfo として読めない。
    PublicKey,
    /// 署名が base64 か DER の署名として読めない。
    Encoding,
    /// CNG の呼び出しが失敗した。署名が合わないときもこれ。
    Cng(windows::core::Error),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PublicKey => write!(f, "the public key is not a P-256 SubjectPublicKeyInfo"),
            Self::Encoding => write!(f, "the signature is not a base64 DER ECDSA signature"),
            Self::Cng(e) => write!(f, "{e}"),
        }
    }
}

/// `message` の署名 `signature`（DER の署名を base64 にした文字列。前後の空白は無視する）を、公開鍵
/// `public_key`（SubjectPublicKeyInfo の DER）で確かめる。
pub fn verify(message: &[u8], signature: &str, public_key: &[u8]) -> Result<(), VerifyError> {
    let point = public_point(public_key).ok_or(VerifyError::PublicKey)?;
    let der = decode_base64(signature.trim()).ok_or(VerifyError::Encoding)?;
    let raw = raw_signature(&der).ok_or(VerifyError::Encoding)?;
    let hash = sha256(message).map_err(VerifyError::Cng)?;
    let algorithm = Algorithm::open(BCRYPT_ECDSA_P256_ALGORITHM).map_err(VerifyError::Cng)?;
    let key = algorithm
        .import_public_key(&public_key_blob(point))
        .map_err(VerifyError::Cng)?;
    // SAFETY: key は開いた鍵で、hash と raw はこの呼び出しの間は生きている。ECDSA は詰め物を使わない。
    unsafe { BCryptVerifySignature(key.0, None, &hash, &raw, BCRYPT_FLAGS(0)) }
        .ok()
        .map_err(VerifyError::Cng)
}

/// `data` の SHA-256。
pub fn sha256(data: &[u8]) -> windows::core::Result<[u8; SHA256_LEN]> {
    let algorithm = Algorithm::open(BCRYPT_SHA256_ALGORITHM)?;
    let mut hash = [0; SHA256_LEN];
    // SAFETY: algorithm は開いた SHA-256 で、data と hash はこの呼び出しの間は生きている。
    unsafe { BCryptHash(algorithm.0, None, data, &mut hash) }.ok()?;
    Ok(hash)
}

/// `bytes` を小文字の 16 進にする。
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// P-256 の公開鍵の SubjectPublicKeyInfo の DER から、点の X と Y（並べて 64 バイト）を取り出す。
fn public_point(spki: &[u8]) -> Option<&[u8]> {
    let rest = spki.strip_prefix(&SPKI_P256_PREFIX)?;
    let point = rest.strip_prefix(&[UNCOMPRESSED_POINT])?;
    (point.len() == 2 * COORDINATE_LEN).then_some(point)
}

/// CNG の `BCRYPT_ECCPUBLIC_BLOB`: `BCRYPT_ECCKEY_BLOB`（印と座標の長さ）のあとに X と Y。
fn public_key_blob(point: &[u8]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(8 + point.len());
    blob.extend_from_slice(&BCRYPT_ECDSA_PUBLIC_P256_MAGIC.to_le_bytes());
    blob.extend_from_slice(&(COORDINATE_LEN as u32).to_le_bytes());
    blob.extend_from_slice(point);
    blob
}

/// DER の ECDSA の署名（`SEQUENCE { INTEGER r, INTEGER s }`）を、CNG の求める r と s を 32 バイトずつ
/// 並べた形にする。読めなければ `None`。
fn raw_signature(der: &[u8]) -> Option<[u8; 2 * COORDINATE_LEN]> {
    let [DER_SEQUENCE, length, body @ ..] = der else {
        return None;
    };
    if *length >= DER_SHORT_LENGTH_LIMIT || usize::from(*length) != body.len() {
        return None;
    }
    let (r, rest) = der_integer(body)?;
    let (s, rest) = der_integer(rest)?;
    if !rest.is_empty() {
        return None;
    }
    let mut raw = [0; 2 * COORDINATE_LEN];
    raw[COORDINATE_LEN - r.len()..COORDINATE_LEN].copy_from_slice(r);
    raw[2 * COORDINATE_LEN - s.len()..].copy_from_slice(s);
    Some(raw)
}

/// `input` の先頭の DER の正の INTEGER を読み、先頭の 0 のバイトを除いた値と、残りを返す。値は
/// 32 バイトまで。
fn der_integer(input: &[u8]) -> Option<(&[u8], &[u8])> {
    let [DER_INTEGER, length, rest @ ..] = input else {
        return None;
    };
    let length = usize::from(*length);
    if length == 0 || length >= usize::from(DER_SHORT_LENGTH_LIMIT) || rest.len() < length {
        return None;
    }
    let (value, rest) = rest.split_at(length);
    // 最上位のビットが立っていれば負の数。正の数でそのビットが立つときは、先頭に 0 を一つ置く。
    if value[0] & 0x80 != 0 {
        return None;
    }
    let start = value
        .iter()
        .position(|&byte| byte != 0)
        .unwrap_or(value.len());
    let value = &value[start..];
    (value.len() <= COORDINATE_LEN).then_some((value, rest))
}

/// base64 の文字列をバイト列にする。読めなければ `None`。
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return None;
    }
    let mut length = 0u32;
    // SAFETY: 書き出し先を渡さず、要る長さだけを問い合わせる。
    unsafe { CryptStringToBinaryW(&wide, CRYPT_STRING_BASE64, None, &mut length, None, None) }
        .ok()?;
    let mut bytes = vec![0u8; length as usize];
    // SAFETY: bytes は length バイトを書き込める。
    unsafe {
        CryptStringToBinaryW(
            &wide,
            CRYPT_STRING_BASE64,
            Some(bytes.as_mut_ptr()),
            &mut length,
            None,
            None,
        )
    }
    .ok()?;
    bytes.truncate(length as usize);
    Some(bytes)
}

/// 開いた CNG のアルゴリズム。落とすと閉じる。
struct Algorithm(BCRYPT_ALG_HANDLE);

impl Algorithm {
    fn open(id: PCWSTR) -> windows::core::Result<Self> {
        let mut handle = BCRYPT_ALG_HANDLE::default();
        // SAFETY: handle は書き込める。id は CNG の決めたアルゴリズムの名前。
        unsafe {
            BCryptOpenAlgorithmProvider(
                &mut handle,
                id,
                PCWSTR::null(),
                BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
            )
        }
        .ok()?;
        Ok(Self(handle))
    }

    fn import_public_key(&self, blob: &[u8]) -> windows::core::Result<Key> {
        let mut key = BCRYPT_KEY_HANDLE::default();
        // SAFETY: self は開いたアルゴリズムで、key は書き込め、blob はこの呼び出しの間は生きている。
        unsafe { BCryptImportKeyPair(self.0, None, BCRYPT_ECCPUBLIC_BLOB, &mut key, blob, 0) }
            .ok()?;
        Ok(Key(key))
    }
}

impl Drop for Algorithm {
    fn drop(&mut self) {
        // SAFETY: open で開いたハンドルで、ほかでは閉じない。
        let closed = unsafe { BCryptCloseAlgorithmProvider(self.0, 0) };
        if closed.is_err() {
            tracing::warn!("failed to close a CNG algorithm: {:#x}", closed.0);
        }
    }
}

/// 読み込んだ CNG の鍵。落とすと消す。
struct Key(BCRYPT_KEY_HANDLE);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: import_public_key で作ったハンドルで、ほかでは消さない。
        let destroyed = unsafe { BCryptDestroyKey(self.0) };
        if destroyed.is_err() {
            tracing::warn!("failed to destroy a CNG key: {:#x}", destroyed.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tools/manifest.sh` で使い捨ての鍵から作った、更新の情報と署名と公開鍵。
    const MANIFEST: &[u8] = include_bytes!("../../testdata/update/latest.json");
    const SIGNATURE: &str = include_str!("../../testdata/update/latest.json.sig");
    const TEST_PUBLIC_KEY: &[u8] = include_bytes!("../../testdata/update/public-key.der");
    /// アプリに埋め込む本物の公開鍵。
    const PUBLIC_KEY: &[u8] = include_bytes!("../../../../assets/manifest-public-key.der");

    #[test]
    fn signature_made_by_manifest_sh_verifies() {
        verify(MANIFEST, SIGNATURE, TEST_PUBLIC_KEY).unwrap();
        verify(MANIFEST, &format!("{SIGNATURE}\r\n"), TEST_PUBLIC_KEY).unwrap();
    }

    #[test]
    fn changed_manifest_does_not_verify() {
        let mut changed = MANIFEST.to_vec();
        let at = changed.iter().position(|&b| b == b'2').unwrap();
        changed[at] = b'3';
        assert!(matches!(
            verify(&changed, SIGNATURE, TEST_PUBLIC_KEY),
            Err(VerifyError::Cng(_))
        ));
    }

    #[test]
    fn other_key_does_not_verify() {
        assert!(matches!(
            verify(MANIFEST, SIGNATURE, PUBLIC_KEY),
            Err(VerifyError::Cng(_))
        ));
    }

    #[test]
    fn broken_signatures_and_keys_are_rejected() {
        assert!(matches!(
            verify(MANIFEST, "", TEST_PUBLIC_KEY),
            Err(VerifyError::Encoding)
        ));
        assert!(matches!(
            verify(MANIFEST, "not base64!", TEST_PUBLIC_KEY),
            Err(VerifyError::Encoding)
        ));
        assert!(matches!(
            verify(MANIFEST, SIGNATURE, &TEST_PUBLIC_KEY[1..]),
            Err(VerifyError::PublicKey)
        ));
    }

    #[test]
    fn embedded_public_key_is_a_p256_key() {
        assert_eq!(public_point(PUBLIC_KEY).unwrap().len(), 64);
        assert_eq!(public_point(TEST_PUBLIC_KEY).unwrap().len(), 64);
    }

    #[test]
    fn der_signatures_become_r_and_s() {
        // r は最上位のビットが立つので 0 を前に置いた 33 バイト、s は短い 2 バイト。
        let mut der = vec![0x30, 39, 0x02, 33, 0x00];
        der.extend([0x80; 32]);
        der.extend([0x02, 2, 0x01, 0x02]);
        let raw = raw_signature(&der).unwrap();
        assert_eq!(&raw[..32], &[0x80; 32]);
        assert_eq!(&raw[32..62], &[0; 30]);
        assert_eq!(&raw[62..], &[0x01, 0x02]);
    }

    #[test]
    fn malformed_der_signatures_are_rejected() {
        // 長さが合わない、余りがある、負の数、33 バイトを超える値。
        assert!(raw_signature(&[0x30, 7, 0x02, 1, 1, 0x02, 1, 1]).is_none());
        assert!(raw_signature(&[0x30, 7, 0x02, 1, 1, 0x02, 1, 1, 0]).is_none());
        assert!(raw_signature(&[0x30, 6, 0x02, 1, 0x80, 0x02, 1, 1]).is_none());
        let mut long = vec![0x30, 38, 0x02, 33, 0x01];
        long.extend([0; 32]);
        long.extend([0x02, 1, 1]);
        assert!(raw_signature(&long).is_none());
        assert!(raw_signature(&[]).is_none());
    }

    #[test]
    fn sha256_matches_the_known_digest() {
        assert_eq!(
            hex(&sha256(b"msi\n").unwrap()),
            "8b25505cb8d6a75a790f7aee75930a62dc03dfabdab4d43acc40468485509f4b"
        );
    }
}
