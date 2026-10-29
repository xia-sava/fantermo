---
name: release
description: fantermo の版を上げて配布する。Cargo の版を上げてコミットし、バージョンタグを打って GitHub Actions に配布物を作らせる手順、打つ前の確認、打ち直しが効かないことなど。「バージョンを上げて」「リリースして」「配布して」と言われたときに使う。
---

# 版を上げる

`vMAJOR.MINOR.PATCH` の形のタグを push すると、GitHub Actions（`.github/workflows/release.yml`）が
配布物を組んで公開する。main への push では検証（`.github/workflows/ci.yml` の fmt・clippy・test）しか
走らず、配布物は出ない。

版は **Cargo の版**（リポジトリ直下の `Cargo.toml` の `[workspace.package]` の `version`）で、各 crate は
これを継ぐ。**人が決める。** タグはこの版の先頭に `v` を付けたもので、食い違えばワークフローが止まる。
新しい版かどうかは版の名前（`MAJOR.MINOR.PATCH`）をそのまま比べる。

## 打つ前に確かめる

1. **作業ツリーがクリーンで、検証が通っていること。**

   ```
   ./tools/dev.sh fmt --check
   ./tools/dev.sh clippy --all-targets
   ./tools/dev.sh test
   ```

2. **main が push 済みで、CI が緑であること。**
   タグだけ先に push してもコミット自体は飛ぶが、ブランチ参照が古いまま残って追いにくくなる。

   ```
   gh run list --workflow CI --limit 3
   ```

3. **配布中の版を確認して、次の版を決める。**

   ```
   gh release view latest --json name,tagName
   ```

## 版を上げてコミットする

1. `Cargo.toml` の `[workspace.package]` の `version` を新しい版にする（例 `0.0.2`）。
2. `./tools/dev.sh build` で `Cargo.lock` の workspace の crate の版を書き換えさせる。ワークフローは
   `--locked` でビルドするので、`Cargo.lock` を上げ忘れると止まる。
3. `Cargo.toml` と `Cargo.lock` だけをコミットする（例「版を 0.0.2 にする」）。

## push してタグを打つ

```
git push origin main
git tag v0.0.2
git push origin v0.0.2
```

**main の push とタグの push は、どちらも外部へ公開する操作。それぞれ実行の直前に必ずユーザーへ
確認を取る。** 依頼済みであっても、押す瞬間に改めて一声かける。main の push の確認はタグの push に
及ばない。

## 確認する

```
gh run list --workflow Release --limit 3
```

`Release` ワークフローが走る。成功すると 2 つのリリースができている。

- **`v0.0.2`** … 版ごとの記録。配布物の実物（`fantermo-0.0.2-x64.msi`・`fantermo-0.0.2-x64.zip`）が
  残るので、後からどの版で何を配ったか辿れる
- **`latest`** … 更新の取得先（prerelease）。固定の名前の配布物（`fantermo.msi`・`fantermo.zip`）と、
  署名した `latest.json`・`latest.json.sig` を置き換える

`latest.json` の署名は secret `FANTERMO_SIGNING_KEY` の鍵で作り、リポジトリの
`assets/manifest-public-key.der` で確かめてから公開する。合わなければ `manifest` ジョブで止まる。
`latest.json` の `msi.sha256`・`zip.sha256` が latest に置く配布物と合わなければ `release` ジョブで止まる。
アプリの「更新を確認」は `latest.json` を読み、`latest/fantermo.msi` をダウンロードして `msi.sha256` で
確かめてから入れる。

## MSI

`build` ジョブが `installer/fantermo.wxs` を WiX で組む。WiX は `.config/dotnet-tools.json` で版を固定し
（7.0.1）、`dotnet tool restore` で入れる。WiX 7 は OSMF の EULA への同意がないと止まるので、
`wix eula accept wix7` で同意し、`wix build` にも `-acceptEula wix7` を付けている。組んだあと `wix msi validate` で ICE の検証を通す。
**MSI を組むのは GitHub Actions だけで、手元には WiX を入れない。**

- 中身は zip と同じ（`fantermo.exe`・`LICENSE`・`THIRD-PARTY-NOTICES.txt`・`README.md`）。ユーザー単位で
  `%LOCALAPPDATA%\Programs\fantermo\` に入れ、スタートメニューに「fantermo」を置く
- 製品の版は Cargo の版。版を上げた MSI は `MajorUpgrade` で古い版を外してから入る。製品コードは
  ビルドのたびに変わる
- **UpgradeCode（`C4FC87EE-5E35-4A23-8EF4-BF71B422CB1D`）は変えない。** 変えると、入れてある版を
  上書きできず二つ並び、アプリの中からの更新も届かなくなる
- 部品の GUID（`Component` の `Guid`）も変えない。変えるなら、部品を足すときに新しい GUID を作る
- WiX の版を上げるときは `.config/dotnet-tools.json` の `version` を変え、EULA の ID（`wix7`）が
  変わっていないかを確かめる

## 気をつけること

- **タグの打ち直しはしない。** 配布したものを直すときは patch を上げた新しい版を出す。同じ版の
  配布物を差し替えると、既に入れた人には新しい版として見えない。
- **版を下げる手段は無い。** 新しい版かどうかを版の名前で比べるので、下げた版は更新として届かない。
  問題が出たら戻すのではなく、直した版を出す。
- **接尾辞つきのタグは通らない**（`v1.0.0-rc1` 等）。MSI の版が major.minor.patch に限られるため、
  ワークフローが弾く。
- **`latest` の置き場は動かさない。** アプリは取得先を `latest` 固定で組み立てる。ここを動かすと
  配布済みのアプリが更新を見つけられなくなる。

## 配布せずに試したいとき

`Release` ワークフローを **workflow_dispatch で手動実行**すると、zip と MSI が成果物として残るだけで、
リリースには一切触れない。**成果物は 3 日で消えるので、その間に落とす。** 版は `dev-<短い SHA>` の
名前になり、中の fantermo はリリースのビルドとして扱わない（版は `0.0.2 (dev-<短い SHA>)` と見え、
「更新を確認」は押せない）。MSI の製品の版は Cargo の版のままなので、同じ版のリリースの MSI と
入れ替えて試せる。
