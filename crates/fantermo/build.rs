//! fantermo の実行ファイルのリンクの指定。

/// メインスレッドのスタックの予約量（バイト）。Windows の既定は 1 MB。
///
/// 窓の大きさを変えている間は、Windows の処理の上にウィンドウプロシージャが何段も積まれて呼ばれる。
/// その中で libghostty-vt の処理を呼んでも足りるように、余裕を持たせる。
const MAIN_THREAD_STACK_BYTES: u32 = 8 * 1024 * 1024;

fn main() {
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "msvc" {
        println!("cargo:rustc-link-arg-bins=/STACK:{MAIN_THREAD_STACK_BYTES}");
    }
}
