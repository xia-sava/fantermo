//! HLSL を実行時に `D3DCompile` でコンパイルする。

use windows::{
    Win32::Graphics::Direct3D::{
        Fxc::{D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile},
        ID3DBlob,
    },
    core::PCSTR,
};

/// セルを描くシェーダのソース。
pub const CELLS_SOURCE: &str = include_str!("cells.hlsl");
pub const CELLS_VERTEX_ENTRY: &str = "vs_main\0";
pub const CELLS_PIXEL_ENTRY: &str = "ps_main\0";

/// Direct3D 11 の機能レベル 10.0 から使えるシェーダモデル。
pub const VERTEX_TARGET: &str = "vs_4_0\0";
pub const PIXEL_TARGET: &str = "ps_4_0\0";

/// コンパイルの失敗。コンパイラの出したメッセージを持つ。
#[derive(Debug)]
pub struct CompileError(pub String);

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "shader compilation failed: {}", self.0)
    }
}

impl std::error::Error for CompileError {}

/// `source` の `entry` を `target` 向けにコンパイルする。`entry` と `target` は終端の NUL を含む。
pub fn compile(source: &str, entry: &str, target: &str) -> Result<ID3DBlob, CompileError> {
    debug_assert!(entry.ends_with('\0') && target.ends_with('\0'));
    let mut code = None;
    let mut errors = None;
    // SAFETY: source の領域は呼び出しの間は生きていて、entry と target は NUL で終わる。
    let result = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry.as_ptr()),
            PCSTR(target.as_ptr()),
            D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    match (result, code) {
        (Ok(()), Some(code)) => Ok(code),
        (result, _) => {
            let message = errors.as_ref().map(blob_text).unwrap_or_default();
            Err(CompileError(match result {
                Err(e) => format!("{e}: {message}"),
                Ok(()) => format!("no code produced: {message}"),
            }))
        }
    }
}

/// blob の中身をバイト列として見る。
pub fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: blob が持つ領域は、blob が生きている間は GetBufferSize バイト読める。
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()) }
}

fn blob_text(blob: &ID3DBlob) -> String {
    String::from_utf8_lossy(blob_bytes(blob))
        .trim_end_matches('\0')
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_shaders_compile() {
        compile(CELLS_SOURCE, CELLS_VERTEX_ENTRY, VERTEX_TARGET).unwrap();
        compile(CELLS_SOURCE, CELLS_PIXEL_ENTRY, PIXEL_TARGET).unwrap();
    }

    #[test]
    fn compile_errors_carry_the_message() {
        let error = compile("float4 broken(", CELLS_PIXEL_ENTRY, PIXEL_TARGET).unwrap_err();
        assert!(!error.0.is_empty());
    }
}
