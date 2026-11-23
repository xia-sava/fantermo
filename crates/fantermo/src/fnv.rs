//! FNV-1a のハッシュ。実行のたびにも Rust のバージョンでも値が変わらないので、色の選び方やファイルの名前のように
//! 次の起動でも同じになってほしいものに使う。暗号には使わない。

use std::ops::BitXor;

/// 32 ビットの FNV-1a の初期値。
const OFFSET_BASIS_32: u32 = 0x811c_9dc5;

/// 32 ビットの FNV-1a の素数。
const PRIME_32: u32 = 0x0100_0193;

/// 64 ビットの FNV-1a の初期値。
const OFFSET_BASIS_64: u64 = 0xcbf2_9ce4_8422_2325;

/// 64 ビットの FNV-1a の素数。
const PRIME_64: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a の値の型。桁あふれを捨てる掛け算ができる整数。
trait Word: Copy + BitXor<Output = Self> + From<u8> {
    fn wrapping_mul(self, other: Self) -> Self;
}

impl Word for u32 {
    fn wrapping_mul(self, other: Self) -> Self {
        u32::wrapping_mul(self, other)
    }
}

impl Word for u64 {
    fn wrapping_mul(self, other: Self) -> Self {
        u64::wrapping_mul(self, other)
    }
}

/// `bytes` の FNV-1a。初期値 `offset_basis` から始め、バイトごとに排他的論理和を取ってから `prime` を掛ける。
fn fnv1a<W: Word>(bytes: &[u8], offset_basis: W, prime: W) -> W {
    bytes.iter().fold(offset_basis, |hash, &byte| {
        (hash ^ W::from(byte)).wrapping_mul(prime)
    })
}

/// `bytes` の 32 ビットの FNV-1a。
pub fn hash32(bytes: &[u8]) -> u32 {
    fnv1a(bytes, OFFSET_BASIS_32, PRIME_32)
}

/// `bytes` の 64 ビットの FNV-1a。
pub fn hash64(bytes: &[u8]) -> u64 {
    fnv1a(bytes, OFFSET_BASIS_64, PRIME_64)
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
        assert_eq!(hash64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(hash64(b"foobar"), 0x8594_4171_f739_67e8);
    }
}
