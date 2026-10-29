//! fantermo の実行ファイルのリンクの指定とリソース。

/// メインスレッドのスタックの予約量（バイト）。Windows の既定は 1 MB。
///
/// ウィンドウの大きさを変えている間は、Windows の処理の上にウィンドウプロシージャが何段も積まれて呼ばれる。
/// その中で libghostty-vt の処理を呼んでも足りるように、余裕を持たせる。
const MAIN_THREAD_STACK_BYTES: u32 = 8 * 1024 * 1024;

/// 実行ファイルに埋め込むマニフェスト。
const MANIFEST: &str = "fantermo.manifest";

/// 実行ファイルに埋め込むリソース（アイコン）の定義。
const RESOURCE_SCRIPT: &str = "fantermo.rc";

/// リソースの定義が読むアイコン。
const ICON: &str = "../../assets/icon/fantermo.ico";

/// 1 ならリリースのビルドとしてバージョンを見せる環境変数。配布物を作るワークフローが付ける。
const RELEASE_ENV: &str = "FANTERMO_RELEASE";

/// コミットが分からないときの印。
const UNKNOWN_COMMIT: &str = "unknown";

fn main() {
    println!("cargo:rerun-if-changed={MANIFEST}");
    println!("cargo:rerun-if-changed={RESOURCE_SCRIPT}");
    println!("cargo:rerun-if-changed={ICON}");
    embed_build_info();
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

/// コミットの短い SHA を `FANTERMO_COMMIT` に、リリースのビルドかを `FANTERMO_RELEASE_BUILD`（`1` か
/// `0`）に入れて、実行ファイルから `env!` で読めるようにする。
fn embed_build_info() {
    println!("cargo:rerun-if-env-changed={RELEASE_ENV}");
    let release = std::env::var(RELEASE_ENV).is_ok_and(|value| value == "1");
    println!(
        "cargo:rustc-env=FANTERMO_RELEASE_BUILD={}",
        if release { "1" } else { "0" }
    );

    // コミットが変わったら作り直す。HEAD がブランチを指していれば、そのブランチの参照も見る。
    for path in git_paths() {
        println!("cargo:rerun-if-changed={path}");
    }
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| UNKNOWN_COMMIT.into());
    println!("cargo:rustc-env=FANTERMO_COMMIT={commit}");
}

/// コミットが変わると書き換わる git のファイル。無いファイルを渡すと毎回作り直すので、あるものだけ。
fn git_paths() -> Vec<String> {
    let git_path = |name: &str| git(&["rev-parse", "--path-format=absolute", "--git-path", name]);
    let mut names = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    names.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    names
        .iter()
        .filter_map(|name| git_path(name))
        .filter(|path| std::path::Path::new(path).exists())
        .collect()
}

/// git を実行して、成功すれば標準出力の 1 行目を返す。git が無い・リポジトリの外なら `None`。
fn git(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_owned())
}
