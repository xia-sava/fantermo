#!/usr/bin/env bash
# ============================================================
#  ブリッジ（crates/fantermo-bridge）を Cygwin と Linux 向けにビルドする（git bash から）。
#
#  ./tools/bridge.sh build  <cygwin|linux|all> [cargo の引数…]   例: build all --release
#  ./tools/bridge.sh clippy <cygwin|linux|all> [cargo の引数…]
#  ./tools/bridge.sh test   <cygwin|linux|all> [cargo の引数…]
#  ./tools/bridge.sh fmt [--check]
#
#  出力: target/bridge/x86_64-pc-cygwin/<debug|release>/fantermo-bridge.exe（cygwin1.dll に依存する）
#        target/bridge/x86_64-unknown-linux-musl/<debug|release>/fantermo-bridge（静的にリンクした ELF）
#
#  cygwin: x86_64-pc-cygwin は tier 3 なので、nightly の -Zbuild-std で std からビルドする。
#    nightly は BRIDGE_NIGHTLY（既定 nightly-2026-10-30）。rust-src が要る:
#      rustup toolchain install nightly-2026-10-30 --profile minimal --component rust-src
#    clippy には同じ nightly の clippy も要る（--component clippy）。
#    リンカは Cygwin の x86_64-pc-cygwin-gcc。CYGWIN_ROOT（例 C:\app\cygwin）があればその bin のものを使い、
#    cargo を実行する間だけその bin を PATH の先頭に置く。なければ PATH から探す（Cygwin の bin が PATH に
#    要る）。Windows 形式のパスをそのまま渡してよい（Cygwin の gcc が読める）。
#    test は panic = "unwind" でビルドする（テストのハーネスが巻き戻しを使う）。
#  linux: x86_64-unknown-linux-musl。既定のツールチェインで、リンカは rust-lld。
#      rustup target add x86_64-unknown-linux-musl
#    Linux 以外で test を実行すると、テストは WSL（BRIDGE_WSL_DISTRIBUTION、既定 Ubuntu）の中で実行する。
# ============================================================
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"   # tools/
root="$(cd "$here/.." && pwd)"                          # リポジトリ root
cd "$root"

manifest="crates/fantermo-bridge/Cargo.toml"
nightly="${BRIDGE_NIGHTLY:-nightly-2026-10-30}"
export CARGO_TARGET_DIR="$root/target/bridge"

usage() {
  sed -n '5,8p' "${BASH_SOURCE[0]}" | sed 's/^#  //' >&2
  exit 2
}

[ $# -ge 1 ] || usage
subcommand="$1"
shift

if [ "$subcommand" = fmt ]; then
  exec cargo fmt --manifest-path "$manifest" "$@"
fi
case "$subcommand" in
  build | clippy | test) ;;
  *) usage ;;
esac
[ $# -ge 1 ] || usage
platform="$1"
shift

cygwin() {
  local path="$PATH"
  if [ -n "${CYGWIN_ROOT:-}" ]; then
    export CARGO_TARGET_X86_64_PC_CYGWIN_LINKER="$CYGWIN_ROOT\\bin\\x86_64-pc-cygwin-gcc.exe"
    # gcc が起動する collect2 や ld は Cygwin の bin の DLL を読むので、cargo の間だけ PATH の先頭に置く。
    path="$(cygpath -u "$CYGWIN_ROOT")/bin:$PATH"
  fi
  echo "[bridge.sh] $subcommand x86_64-pc-cygwin（$nightly）" >&2
  if [ "$subcommand" = test ]; then
    PATH="$path" CARGO_PROFILE_DEV_PANIC=unwind cargo "+$nightly" test --manifest-path "$manifest" --locked \
      --target x86_64-pc-cygwin -Zbuild-std "$@"
    return
  fi
  PATH="$path" cargo "+$nightly" "$subcommand" --manifest-path "$manifest" --locked \
    --target x86_64-pc-cygwin -Zbuild-std=std,panic_abort "$@"
}

linux() {
  export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
  echo "[bridge.sh] $subcommand x86_64-unknown-linux-musl" >&2
  if [ "$subcommand" = test ] && [ "$(uname -s)" != Linux ]; then
    test_linux_in_wsl "$@"
    return
  fi
  cargo "$subcommand" --manifest-path "$manifest" --locked \
    --target x86_64-unknown-linux-musl "$@"
}

# テストをビルドし、できた実行ファイルを WSL の中で一つずつ実行する。
test_linux_in_wsl() {
  local log executable status=0
  log="$(mktemp)"
  cargo test --manifest-path "$manifest" --locked --target x86_64-unknown-linux-musl --no-run "$@" 2>&1 | tee "$log" >&2
  while IFS= read -r executable; do
    echo "[bridge.sh] WSL で実行: $executable" >&2
    MSYS_NO_PATHCONV=1 wsl.exe -d "${BRIDGE_WSL_DISTRIBUTION:-Ubuntu}" --exec "/mnt$root/${executable//\\//}" < /dev/null || status=$?
  done < <(sed -n 's/^ *Executable .*(\(.*\))\r\{0,1\}$/\1/p' "$log")
  rm -f "$log"
  return "$status"
}

case "$platform" in
  cygwin) cygwin "$@" ;;
  linux) linux "$@" ;;
  all) cygwin "$@" && linux "$@" ;;
  *) usage ;;
esac
