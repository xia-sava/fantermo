---
name: e2e-verify
description: fantermo の e2e テストと、制御の口（feature e2e）での確かめ方。実際の fantermo を別デスクトップで起動して、HTTP で入力・タブ・ウィンドウの大きさを操作し、画面の文字と絵を読む。実行のしかた、テストの書き方、Cygwin と WSL のブリッジの e2e、落とし穴。「e2e を走らせて」「e2e を足して」「制御の口で確かめて」と言われたときに使う。
---

# fantermo の e2e

開発用の feature `e2e`（既定で無効、配布物に入らない）を付けた fantermo は、環境変数 `FANTERMO_E2E_DIR` があると
`127.0.0.1` の OS が割り当てたポートで小さな HTTP を受ける。ポートの番号はそのフォルダの `port` に書く。
実装は `crates/fantermo/src/e2e_server.rs`（受ける側）と `crates/fantermo/src/window/e2e.rs`（ウィンドウのスレッド側）、
テストの土台は `crates/fantermo/tests/common/mod.rs`、テストは `crates/fantermo/tests/e2e.rs`。

## 実行

```sh
./tools/dev.sh test --features e2e                # 全部（単体のテストも実行する）
./tools/dev.sh test --features e2e --test e2e tabs   # 名前で絞る（run-hidden は前の回に作ったものを使う）
```

Cygwin と WSL のブリッジの e2e は環境変数があるときだけ実行し、なければ `SKIPPED:` と標準エラー出力に出す。

```sh
CYGWIN_ROOT='C:\app\cygwin' ./tools/bridge.sh build all
FANTERMO_E2E_CYGWIN_ROOT='C:\app\cygwin' FANTERMO_E2E_WSL=Ubuntu ./tools/dev.sh test --features e2e --test e2e
```

ブリッジ単体（パイプでつないでプロトコルで話す）は `CYGWIN_ROOT='C:\app\cygwin' ./tools/bridge.sh test all`。Linux 向けは WSL の中で実行する。

## 要求の一覧

| 要求 | すること |
|---|---|
| `GET /e2e/state` | JSON: `cols` `rows` `client_width` `client_height` `active` `tabs`（`title` `profile` `cols` `rows`）`window_title` `version` `text`（スクロールバックを含む全体の文字） |
| `GET /e2e/frame` | いまの画面の PNG |
| `POST /e2e/input` | 本文のバイト列をそのままバックエンドへ |
| `POST /e2e/text` | 本文の文字を `WM_CHAR` と同じ経路で（制御文字は送らない。Enter は `key`） |
| `POST /e2e/key/<組み合わせ>` | `enter`、`ctrl+shift+t` など。キーの割り当てと符号化を通す |
| `POST /e2e/tab/new`、`/e2e/tab/close`、`/e2e/tab/select/<位置>` | タブの操作 |
| `POST /e2e/window/size/<桁数>/<行数>` | ウィンドウの大きさを変えて、そのあとの状態を返す |
| `POST /e2e/quit` | ウィンドウを閉じる |

## テストの書き方

`Server::start(設定の TOML, fantermo の引数)` か `Server::start_cmd()`・`start_powershell()` で起動し、`type_line`・`press`・`resize`・`post` で操作して、`wait_text`・`wait_until` で待つ。`sleep` で待たない。`Server` が一時フォルダ（`FANTERMO_CONFIG`・`FANTERMO_STATE`・ログ・ポート）、同時に起動する数の上限、Drop での停止と片づけを受け持つので、テストは並列で衝突しない。失敗すると fantermo のログを出す。打った行に出ない文字列を待つ（`echo marker-%ERRORLEVEL%-end` で `marker-0-end`）。

## 落とし穴

1. **出力先は `target/e2e/`。** `tools/dev.bat` が引数の `--features e2e` を見て `CARGO_TARGET_DIR` を決める。feature の有無で同じ `target/debug/fantermo.exe` を取り合わないため。素の `cargo` や RustRover から `--features e2e` を付けると `target/debug/` に作るので、動かしている exe とぶつかる。
2. **`--features e2e` は引数のこの字面でないと `target/e2e/` にならない。** `--features=e2e` や `-F e2e` では `target/debug/` に作る。
3. **feature なしの `./tools/dev.sh test` は e2e を走らせず、fantermo.exe も作り直さない**（`[[test]]` の `required-features`）。e2e のテストの実行ファイルは feature があるときだけ。
4. **run-hidden は別の crate。** `crates/run-hidden/tests/` のテストがあるので workspace 全体の `test` で作られるが、`-p fantermo` や `--test e2e` だけでは作らない。なければテストがその旨を出して失敗する。`./tools/dev.sh build --features e2e` で作る。
5. **`target/e2e/` の fantermo からは `target/bridge/` のブリッジが見えない。** テストがビルド済みのものを `target/e2e/debug/` へ写す。ブリッジを変えたら `./tools/bridge.sh build` し直す（テストの実行ごとに写す）。git bash の `cp` で `fantermo-bridge` と `fantermo-bridge.exe` を同じフォルダに置かない（テストは Rust で写す）。
6. **ウィンドウの大きさを変えるのは借用の外で。** `SetWindowPos` はこの場で `WM_SIZE` を送るので、借用の中だと飛ばされる。答えを返してから閉じる、メニューを開くなど、自分でメッセージを回すものは `Deferred` で。
7. **`FANTERMO_STATE` と `FANTERMO_CONFIG` を付けないで起動しない。** 付けないと利用者の `%APPDATA%\fantermo` を読み書きする。手で起動するときも同じ。
8. **クリップボードは利用者と共通。** 土台にはクリップボードを触る要求がない。足すならテストを直列にして、利用者の中身を壊さない形にする。
9. **ランナーで動くか。** `smoke` で始まるテストが別デスクトップと Direct3D の試し。失敗の見当は、ログの `run-hidden:`（デスクトップや起動の失敗）、`no hardware Direct3D 11 device`（WARP に切り替えた）、`failed to render`（描画）。
