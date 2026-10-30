#!/usr/bin/env bash
# ============================================================
#  第三者のライセンスの表示 THIRD-PARTY-NOTICES.txt を作る (git-bash 側)
#  依存を変えたら作り直してコミットする。cargo-about（`cargo about`）が要る。
#  例: ./tools/licenses.sh
#
#  中身は 5 つをつなげたもの:
#    1. 前書き（このスクリプトの中）
#    2. tools/licenses/zig-notices.txt … libghostty-vt（Zig）に静的にリンクされるもの。手で確かめて書いた
#    3. cargo-about の出力 … 実行ファイル fantermo に入る Rust の crate
#       （設定は tools/licenses/about.toml、テンプレートは tools/licenses/about.hbs）
#    4. tools/licenses/bridge-notices.txt … ブリッジに静的にリンクされる Rust の crate 以外のもの。手で確かめて書いた
#    5. cargo-about の出力 … ブリッジ（crates/fantermo-bridge）に入る Rust の crate
#       （設定は tools/licenses/about-bridge.toml、テンプレートは 3 と同じ）
#
#  bridge-notices.txt を確かめたもの（Rust 1.99.1、Cygwin の gcc-core 14.4.0）:
#    - Linux 向け（x86_64-unknown-linux-musl）は、Rust の self-contained の libc.a（musl 1.2.5。Rust の
#      src/ci/docker/scripts/musl-toolchain.sh の版）・libunwind.a・crt の .o を静的にリンクする。
#      musl の文は https://git.musl-libc.org/cgit/musl/plain/COPYRIGHT?h=v1.2.5 。libunwind と crtbeginS.o・
#      crtendS.o は LLVM（Apache-2.0 WITH LLVM-exception。バイナリに表示を求めない）なので載せない
#    - Cygwin 向け（x86_64-pc-cygwin）は、crt0.o と libcygwin.a（Cygwin の Linking Exception）、libgcc.a・
#      crtbegin.o・crtend.o（GCC Runtime Library Exception）、w32api の取り込みライブラリ、default-manifest.o
#      （windows-default-manifest。表示を残す条件の許諾）を静的にリンクする。表示を求めるのは
#      default-manifest.o だけ。cygwin1.dll と cyggcc_s-seh-1.dll は動的にリンクし、同梱しない
#    - リンクされるものは、リンカに --trace を渡して確かめた（Cygwin は -C link-arg=-Wl,--trace、
#      Linux は -C link-arg=--trace）。Rust のツールチェインや Cygwin の gcc を上げたら確かめ直す
#
#  zig-notices.txt を確かめたバージョン（libghostty-rs 9ede35b8ea94a55ed3608408d67e102883129f1e、Zig 0.17.0）:
#    - Ghostty 521cf2c5d30bd48028ca61546c64e7913764073d（libghostty-vt-sys の build.rs の GHOSTTY_COMMIT）
#    - uucode 0.2.0、ea62149739404a73c202b48a33bf6dd2af4bd9b0（Ghostty の build.zig.zon）。
#      licenses/ の下の文は tarball に入っていないので、GitHub の同じコミットから取った
#    - simdutf 9.0.0（Ghostty の pkg/simdutf/vendor/simdutf.h の SIMDUTF_VERSION。pkg/simdutf の
#      build.zig.zon のバージョンとは違う）。vendor にライセンスの文がないので、GitHub の v9.0.0 の LICENSE-MIT を取った
#    - Highway 66486a10623fa0d72fe91260f96c892e41aceb06（pkg/highway の build.zig.zon）。ソースの各ファイルの
#      見出しが Apache-2.0 なので、同じコミットの LICENSE（Apache-2.0）の文を載せる
#    - Wuffs 7411f488fe2e2c205c3d3b3d28638b7356522930（pkg/wuffs の build.zig.zon）。release/c/wuffs-v0.4.c は
#      Apache-2.0 OR MIT
#    - X11 の rgb.txt（Ghostty の src/terminal/res/rgb.txt。src/terminal/x11_color.zig が埋め込む）。
#      文は https://gitlab.freedesktop.org/xorg/app/rgb の COPYING
#    - Zig 0.17.0 の LICENSE（標準ライブラリと compiler_rt。translate-c の LICENSE も同じ文）
#
#  リンクされるものは、静的ライブラリの中のオブジェクトで確かめた:
#    zig ar t target/debug/build/libghostty-vt-sys-*/out/ghostty-install/lib/ghostty-vt-static.lib
#  の結果は次の 12 個:
#    ghostty-vt-static_zcu.obj vt.obj base64.obj index_of.obj codepoint_width.obj  … Ghostty と Zig の標準ライブラリ、uucode
#    simdutf.obj                                                                     … simdutf
#    highway_zcu.obj abort.obj targets.obj per_target.obj                            … Highway
#    wuffs-v0.4.obj                                                                  … Wuffs
#    compiler_rt.obj                                                                 … Zig
#  Ghostty の build.zig.zon のほかの依存（freetype、harfbuzz、oniguruma、zlib など）は libghostty-vt の
#  ビルドでは使わない。zig-pkg/ に取ってくる zlib、pixels（Wuffs の試験の画像）、iTerm2 の配色、aro、
#  translate-c の本体はオブジェクトとして入らない。
#
#  libghostty-vt の rev を上げたら:
#    1. ./tools/dev.sh build のあと、上の zig ar t の結果を比べる。増えたオブジェクトがあれば、どの依存の
#       ものかを Ghostty の src/build/GhosttyZig.zig と SharedDeps.zig から辿る
#    2. build.zig.zon（Ghostty と pkg/ の下）の依存のコミットとバージョン、Ghostty の LICENSE が変わって
#       いれば、zig-notices.txt の文とこのコメントのバージョンを直す
#    3. このスクリプトを実行し直す。about.toml の checksum が合わなければ、文を確かめて checksum を直す
# ============================================================
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"   # tools/
root="$(cd "$here/.." && pwd)"                          # リポジトリ root
cd "$root"

out="THIRD-PARTY-NOTICES.txt"
rust="$(mktemp)"
bridge_rust="$(mktemp)"
trap 'rm -f "$rust" "$bridge_rust"' EXIT

cargo about generate --fail --locked \
    --manifest-path crates/fantermo/Cargo.toml \
    --config tools/licenses/about.toml \
    --output-file "$rust" \
    tools/licenses/about.hbs

cargo about generate --fail --locked \
    --manifest-path crates/fantermo-bridge/Cargo.toml \
    --config tools/licenses/about-bridge.toml \
    --output-file "$bridge_rust" \
    tools/licenses/about.hbs

# crate の中のライセンスの文には CRLF のものや行末の空白を持つものがあるので、LF にそろえて行末を詰める。
normalize() { tr -d '\r' | sed -e 's/[[:space:]]*$//'; }

heading() {
    printf '%s\n%s\n%s\n' "$(printf '=%.0s' {1..80})" "$1" "$(printf '=%.0s' {1..80})"
}

{
    cat <<'EOF'
fantermo が使っている第三者のソフトウェアと、そのライセンス

fantermo の実行ファイルとブリッジ（fantermo-bridge.exe、fantermo-bridge）には、fantermo 自身（MIT）の
ほかに、次のソフトウェアが含まれます。
二つ以上のライセンスから選べるものは、選んだほうの文を載せています。

EOF
    normalize < tools/licenses/zig-notices.txt
    printf '\n\n'
    heading "Rust の crate"
    normalize < "$rust"
    # cargo-about の出力は空行で終わる。
    printf '\n'
    normalize < tools/licenses/bridge-notices.txt
    printf '\n\n'
    heading "ブリッジの Rust の crate"
    normalize < "$bridge_rust"
} > "$out"

echo "wrote $out"
