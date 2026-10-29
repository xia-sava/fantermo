//! fantermo の実行ファイルのリンクの指定とリソース。

/// メインスレッドのスタックの予約量（バイト）。Windows の既定は 1 MB。
///
/// 窓の大きさを変えている間は、Windows の処理の上にウィンドウプロシージャが何段も積まれて呼ばれる。
/// その中で libghostty-vt の処理を呼んでも足りるように、余裕を持たせる。
const MAIN_THREAD_STACK_BYTES: u32 = 8 * 1024 * 1024;

/// 実行ファイルに埋め込むマニフェスト。
const MANIFEST: &str = "fantermo.manifest";

/// 実行ファイルに埋め込むリソース（アイコン）の定義。
const RESOURCE_SCRIPT: &str = "fantermo.rc";

/// リソースの定義が読むアイコン。
const ICON: &str = "../../assets/icon/fantermo.ico";

fn main() {
    println!("cargo:rerun-if-changed={MANIFEST}");
    println!("cargo:rerun-if-changed={RESOURCE_SCRIPT}");
    println!("cargo:rerun-if-changed={ICON}");
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env != "msvc" {
        return;
    }
    println!("cargo:rustc-link-arg-bins=/STACK:{MAIN_THREAD_STACK_BYTES}");

    // マニフェストはリソースの定義に含めず、リンカに埋め込ませる。
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let manifest = std::path::Path::new(&manifest_dir).join(MANIFEST);
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );

    // rc.exe は PATH になくても、embed-resource がレジストリから Windows SDK を探して見つける。
    if let Err(e) =
        embed_resource::compile(RESOURCE_SCRIPT, embed_resource::NONE).manifest_required()
    {
        panic!("failed to compile {RESOURCE_SCRIPT}: {e}");
    }
}
