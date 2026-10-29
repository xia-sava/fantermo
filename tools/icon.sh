#!/usr/bin/env bash
# ============================================================
#  アイコン assets/icon/fantermo.ico を作る (git-bash 側)
#  図案（assets/icon/*.svg）を変えたら作り直してコミットする。ビルドのたびには作らない。
#  例: ./tools/icon.sh
#  あわせて、各サイズを明るい地と暗い地に並べた見本を target/icon/sheet.png に書く。
#
#  使うもの:
#    - Microsoft Edge（ヘッドレスで SVG を 256 px の PNG に描く）。プロファイルは target/icon/ の下に
#      作り、利用者の Edge のものは使わない
#    - python3.12 と Pillow（tools/icon/icon.py。PNG を各サイズに縮め、ICO にまとめる）
#
#  サイズと図案:
#    - 16、20、24 px … fantermo-small.svg（地と画面を広げ、「%」の線を太くしたもの）
#    - 32、40、48、64、96、256 px … fantermo.svg
#    256 px は PNG、ほかは 32 ビットのビットマップで ICO に入れる。
# ============================================================
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"   # tools/
root="$(cd "$here/.." && pwd)"                          # リポジトリ root
cd "$root"

edge="/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"
work="target/icon"
mkdir -p "$work"

render() { # <svg> <png>
    MSYS_NO_PATHCONV=1 "$edge" --headless=new --disable-gpu --hide-scrollbars \
        --force-device-scale-factor=1 \
        --user-data-dir="$(cygpath -w "$root/$work/edge-profile")" \
        --default-background-color=00000000 --window-size=256,256 \
        --screenshot="$(cygpath -w "$root/$2")" \
        "file:///$(cygpath -m "$root/$1")" 2>/dev/null
    [[ -s "$2" ]] || { echo "failed to render $1" >&2; exit 1; }
}

render assets/icon/fantermo.svg "$work/large.png"
render assets/icon/fantermo-small.svg "$work/small.png"

python3.12 tools/icon/icon.py "$work" assets/icon/fantermo.ico "$work/sheet.png"
echo "wrote assets/icon/fantermo.ico and $work/sheet.png"
