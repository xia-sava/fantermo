//! base64 への変換。

/// base64 の文字の並び。
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/// base64 の 4 文字にまとめるバイト数。
const GROUP_BYTES: usize = 3;

/// `data` を base64 にして `out` に足す。
pub fn push(out: &mut Vec<u8>, data: &[u8]) {
    for group in data.chunks(GROUP_BYTES) {
        let byte = |index: usize| u32::from(group.get(index).copied().unwrap_or(0));
        let bits = byte(0) << 16 | byte(1) << 8 | byte(2);
        for position in 0..=GROUP_BYTES {
            if position <= group.len() {
                let shift = 18 - 6 * position;
                out.push(ALPHABET[(bits >> shift) as usize & 0x3f]);
            } else {
                out.push(b'=');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_padding_rules() {
        for (data, encoded) in [
            (b"".as_slice(), ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (&[0xfb, 0xff], "+/8="),
        ] {
            let mut out = Vec::new();
            push(&mut out, data);
            assert_eq!(out, encoded.as_bytes());
        }
    }
}
