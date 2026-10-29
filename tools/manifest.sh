#!/usr/bin/env bash
# ============================================================
#  更新の情報 latest.json を作り、署名し、署名を確かめる
#  配布物を作るワークフロー（.github/workflows/release.yml）が使う。手元の git-bash でも動く。
#  jq、sha256sum、openssl、base64 が要る。
#
#  例:
#    ./tools/manifest.sh write 0.0.1 https://github.com/xia-sava/fantermo/releases/download/latest \
#        out/latest.json zip=out/fantermo.zip msi=out/fantermo.msi
#    ./tools/manifest.sh sign out/latest.json <秘密鍵の PEM> assets/manifest-public-key.der
#    ./tools/manifest.sh verify out/latest.json out/latest.json.sig assets/manifest-public-key.der
#
#  write  <版> <置き場の URL> <出力> <種類>=<配布物>...
#         版と、配布物ごとの URL（置き場の URL + 配布物のファイル名）と sha256 を書く:
#           { "versionName": "0.0.1", "zip": { "url": "...", "sha256": "..." },
#             "msi": { "url": "...", "sha256": "..." } }
#         アプリは msi の sha256 で、ダウンロードした MSI を確かめる。
#  sign   <latest.json> <秘密鍵> <公開鍵>
#         latest.json の生バイトに ECDSA P-256（SHA256withECDSA）で署名し、DER の署名を base64 にした
#         1 行を <latest.json>.sig に書く。書いたあと公開鍵で確かめる。秘密鍵は PKCS#8 の PEM、
#         公開鍵は X.509 SubjectPublicKeyInfo の DER。
#  verify <latest.json> <署名> <公開鍵>
#         署名を公開鍵で確かめる。合わなければ 0 以外で終わる。
# ============================================================
set -euo pipefail

usage() {
  sed -n '3,24p' "$0" >&2
  exit 2
}

write_manifest() {
  [ $# -ge 4 ] || usage
  local version=$1 base_url=$2 out=$3
  shift 3
  local manifest
  manifest=$(jq -n --arg version "$version" '{ versionName: $version }')
  local entry kind file sha
  for entry in "$@"; do
    kind=${entry%%=*}
    file=${entry#*=}
    if [ -z "$kind" ] || [ "$kind" = "$entry" ] || [ ! -f "$file" ]; then
      echo "配布物を <種類>=<ファイル> で渡すこと（受け取った値: $entry）" >&2
      exit 1
    fi
    sha=$(sha256sum "$file" | cut -d' ' -f1)
    manifest=$(printf '%s' "$manifest" | jq \
      --arg kind "$kind" --arg url "$base_url/$(basename "$file")" --arg sha "$sha" \
      '. + { ($kind): { url: $url, sha256: $sha } }')
  done
  printf '%s\n' "$manifest" > "$out"
}

verify_manifest() {
  [ $# -eq 3 ] || usage
  local manifest=$1 signature=$2 public_key=$3
  local work
  work=$(mktemp -d)
  # shellcheck disable=SC2064
  trap "rm -rf '$work'" RETURN
  openssl pkey -pubin -inform DER -in "$public_key" -out "$work/public-key.pem"
  base64 -d "$signature" > "$work/signature.der"
  openssl dgst -sha256 -verify "$work/public-key.pem" -signature "$work/signature.der" "$manifest"
}

sign_manifest() {
  [ $# -eq 3 ] || usage
  local manifest=$1 private_key=$2 public_key=$3
  openssl dgst -sha256 -sign "$private_key" "$manifest" | base64 -w0 > "$manifest.sig"
  # 埋め込む公開鍵と食い違う鍵で署名したまま公開すると、どのアプリも更新を受け取れなくなる。
  verify_manifest "$manifest" "$manifest.sig" "$public_key"
}

[ $# -ge 1 ] || usage
command=$1
shift
case "$command" in
  write) write_manifest "$@" ;;
  sign) sign_manifest "$@" ;;
  verify) verify_manifest "$@" ;;
  *) usage ;;
esac
