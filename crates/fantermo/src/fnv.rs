//! FNV-1a のハッシュ。実行のたびにも Rust のバージョンでも値が変わらないので、色の選び方のように次の起動でも
//! 同じになってほしいものに使う。暗号には使わない。

/// 32 ビットの FNV-1a の初期値。
const OFFSET_BASIS_32: u32 = 0x811c_9dc5;

/// 32 ビットの FNV-1a の素数。
const PRIME_32: u32 = 0x0100_0193;

/// `bytes` の 32 ビットの FNV-1a。
pub fn hash32(bytes: &[u8]) -> u32 {
    bytes.iter().fold(OFFSET_BASIS_32, |hash, &byte| {
        (hash ^ u32::from(byte)).wrapping_mul(PRIME_32)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_values_match_the_published_ones() {
        // FNV の仕様の参照の値。
        assert_eq!(hash32(b""), 0x811c_9dc5);
        assert_eq!(hash32(b"a"), 0xe40c_292c);
        assert_eq!(hash32(b"foobar"), 0xbf9c_f968);
    }
}
