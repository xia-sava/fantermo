#!/usr/bin/env bash
# ============================================================
#  第三者のライセンスの表示 THIRD-PARTY-NOTICES.txt を作る (git-bash 側)
#  依存を変えたら作り直してコミットする。cargo-about（`cargo about`）が要る。
#  例: ./tools/licenses.sh
#
#  中身は 3 つをつなげたもの:
#    1. 前書き（このスクリプトの中）
#    2. tools/licenses/zig-notices.txt … libghostty-vt（Zig）に静的にリンクされるもの。手で確かめて書いた
#    3. cargo-about の出力 … 実行ファイル fantermo に入る Rust の crate
#       （設定は tools/licenses/about.toml、テンプレートは tools/licenses/about.hbs）
#
#  zig-notices.txt を確かめた版（libghostty-rs 9ede35b8ea94a55ed3608408d67e102883129f1e、Zig 0.17.0）:
#    - Ghostty 521cf2c5d30bd48028ca61546c64e7913764073d（libghostty-vt-sys の build.rs の GHOSTTY_COMMIT）
#    - uucode 0.2.0、ea62149739404a73c202b48a33bf6dd2af4bd9b0（Ghostty の build.zig.zon）。
#      licenses/ の下の文は tarball に入っていないので、GitHub の同じコミットから取った
#    - simdutf 9.0.0（Ghostty の pkg/simdutf/vendor/simdutf.h の SIMDUTF_VERSION。pkg/simdutf の
#      build.zig.zon の版とは違う）。vendor にライセンスの文がないので、GitHub の v9.0.0 の LICENSE-MIT を取った
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
#    2. build.zig.zon（Ghostty と pkg/ の下）の依存のコミットと版、Ghostty の LICENSE が変わっていれば、
#       zig-notices.txt の文とこのコメントの版を直す
#    3. このスクリプトを流し直す。about.toml の checksum が合わなければ、文を確かめて checksum を直す
# ============================================================
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"   # tools/
root="$(cd "$here/.." && pwd)"                          # リポジトリ root
cd "$root"

out="THIRD-PARTY-NOTICES.txt"
rust="$(mktemp)"
trap 'rm -f "$rust"' EXIT

cargo about generate --fail --locked \
    --manifest-path crates/fantermo/Cargo.toml \
    --config tools/licenses/about.toml \
    --output-file "$rust" \
    tools/licenses/about.hbs

# crate の中のライセンスの文には CRLF のものや行末の空白を持つものがあるので、LF にそろえて行末を詰める。
normalize() { tr -d '\r' | sed -e 's/[[:space:]]*$//'; }

{
    cat <<'EOF'
fantermo が使っている第三者のソフトウェアと、そのライセンス

fantermo の実行ファイルには、fantermo 自身（MIT）のほかに、次のソフトウェアが含まれます。
二つ以上のライセンスから選べるものは、選んだほうの文を載せています。

EOF
    normalize < tools/licenses/zig-notices.txt
    printf '\n\n'
    normalize < "$rust"
} > "$out"

echo "wrote $out"
