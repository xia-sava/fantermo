---
name: e2e-verify
description: fantermo の e2e テストと、制御の口（feature e2e）での確認のしかた。実際の fantermo を別デスクトップで起動して、HTTP で入力・タブ・ウィンドウの大きさを操作し、画面の文字と絵を読む。実行のしかた、テストの書き方、Cygwin と WSL のブリッジの e2e、落とし穴。「e2e を走らせて」「e2e を足して」「制御の口で確認して」と言われたときに使う。
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

ブリッジ単体（パイプで接続してプロトコルで話す）は `CYGWIN_ROOT='C:\app\cygwin' ./tools/bridge.sh test all`。Linux 向けは WSL の中で実行する。

大きなものは手元でだけ流す。`FANTERMO_BRIDGE_BULK_BYTES=2000000`（大きな入力。既定 200 KB）、`FANTERMO_BRIDGE_LINE_EDITOR_BYTES=100000`（zsh の行編集に流す改行のない行。指定したときだけ実行する）。WSL の中で実行するものへ渡すには `WSLENV` に名前を足す。

OS のクリップボードを実際に変えるテスト（`os_clipboard` を含む名前）は `FANTERMO_E2E_OS_CLIPBOARD=1` があるときだけ実行する。CI では付ける。手元では付けない（利用者のクリップボードの文字列のほかの形式が消える）。

## 要求の一覧

| 要求 | すること |
|---|---|
| `GET /e2e/state` | JSON: `pid` `cols` `rows` `client_width` `client_height` `terminal_x` `terminal_y` `cell_width` `cell_height` `active` `tabs`（`title` `profile` `cols` `rows` `bell`）`profiles`（名前）`dialogs`（`class` `title` `controls`）`images`（画面に出ている画像の数）`window_title` `version` `text`（スクロールバックを含む全体の文字） |
| `GET /e2e/frame` | いまの画面の PNG |
| `POST /e2e/input` | 本文のバイト列をそのままバックエンドへ |
| `POST /e2e/text` | 本文の文字を `WM_CHAR` と同じ経路で（制御文字は送らない。Enter は `key`） |
| `POST /e2e/key/<組み合わせ>` | `enter`、`ctrl+shift+t` など。キーの割り当てと符号化を通す |
| `POST /e2e/tab/new`、`/e2e/tab/new/<プロファイルの位置>`、`/e2e/tab/close`、`/e2e/tab/select/<位置>` | タブの操作 |
| `POST /e2e/window/size/<桁数>/<行数>` | ウィンドウの大きさを変えて、そのあとの状態を返す |
| `POST /e2e/paste` | 本文の文字を、クリップボードから読んだものとして貼り付ける（`paste.confirm` と `paste.confirm_size` に従う） |
| `POST /e2e/dialog/button/<ID>` | 開いているダイアログのボタンを押す（`TaskDialogIndirect` は `TDM_CLICK_BUTTON`、ほかは `WM_COMMAND`。メッセージボックスは押せない） |
| `GET /e2e/clipboard`、`POST /e2e/clipboard` | クリップボードの文字列を読む（JSON の `text`）・置き換える |
| `POST /e2e/quit` | ウィンドウを閉じる |

## テストの書き方

`Server::start(設定の TOML, fantermo の引数)` か `Server::start_cmd()`・`start_powershell()` で起動し、`type_line`・`press`・`resize`・`post` で操作して、`wait_text`・`wait_until` で待つ。`sleep` で待たない。`Server` が一時フォルダ（`FANTERMO_CONFIG`・`FANTERMO_STATE`・ログ・ポート）、同時に起動する数の上限、Drop での停止と片づけを受け持つので、テストは並列で衝突しない。失敗すると fantermo のログを出す。打った行に出ない文字列を待つ（`echo marker-%ERRORLEVEL%-end` で `marker-0-end`）。

- 長い行: `type_line` は一文字ずつ送り、Cygwin の pty では一文字に 30 ms ほどかかる。長い行は `send_line`（`/e2e/input` でまとめて送る）で。
- 閉じる確認の前に、動かしたコマンドが動き出したのをタブごとに確認する。Cygwin では起動の途中で閉じるとコマンドが残ることがある。
- 画像: ConPTY は APC と DCS を落とすので、ブリッジのタブで `printf` する。`images` を待ってから `picture()` で画素を読む。位置は `terminal_x` などから計算する（`cell_pixel`）。
- ダイアログ: 編集のダイアログや確認のダイアログの間も要求は処理される（ダイアログのループが届ける）。`wait_dialogs` で待ち、`press_dialog_button` で押す。
- 子プロセス: `Server::start_watching_children` で起動すると、fantermo が終わったあと run-hidden が子孫の終わりを待ち（`--wait-children`）、`assert_no_children_remained` で残りが分かる。付けないと、run-hidden が Job を閉じて全部終わらせるので、残りがあっても見えない。タブを閉じる確認は `common::processes`（親子と作成時刻）で。Cygwin のプログラムは Windows の親子で辿れないので、`/proc` のコマンドラインと一意な `sleep` の引数で見る。
- クリップボード: `Server` は既定で `FANTERMO_E2E_CLIPBOARD_FILE`（一時フォルダのファイル）を渡し、fantermo はそのファイルをクリップボードの代わりに使う。OS のものを使うのは `Server::start_with_os_clipboard` だけで、`OsClipboard` が直列にする。
- ブリッジの単体のテストで中身を比べるときは、子プロセスに `/tmp` のファイルへ書かせて読む（テストと子プロセスは同じ環境で動く）。
- 読み取りの依頼のように答えがないことがあるものは、時間で待たず、依頼と同じ出力の後ろの印を待ってから、区切りの入力を送る（`start_clipboard_read`）。

## 落とし穴

1. **出力先は `target/e2e/`。** `tools/dev.bat` が引数の `--features e2e` を見て `CARGO_TARGET_DIR` を決める。feature の有無で同じ `target/debug/fantermo.exe` を取り合わないため。素の `cargo` や RustRover から `--features e2e` を付けると `target/debug/` に作るので、動かしている exe とぶつかる。
2. **`--features e2e` は引数のこの字面でないと `target/e2e/` にならない。** `--features=e2e` や `-F e2e` では `target/debug/` に作る。
3. **feature なしの `./tools/dev.sh test` は e2e を走らせず、fantermo.exe も作り直さない**（`[[test]]` の `required-features`）。e2e のテストの実行ファイルは feature があるときだけ。
4. **run-hidden は別の crate。** `crates/run-hidden/tests/` のテストがあるので workspace 全体の `test` で作られるが、`-p fantermo` や `--test e2e` だけでは作らない。なければテストがその旨を出して失敗する。`./tools/dev.sh build --features e2e` で作る。
5. **`target/e2e/` の fantermo からは `target/bridge/` のブリッジが見えない。** テストがビルド済みのものを `target/e2e/debug/` へ写す。ブリッジを変えたら `./tools/bridge.sh build` し直す（テストの実行ごとに写す）。git bash の `cp` で `fantermo-bridge` と `fantermo-bridge.exe` を同じフォルダに置かない（テストは Rust で写す）。
6. **ウィンドウの大きさを変えるのは借用の外で。** `SetWindowPos` はこの場で `WM_SIZE` を送るので、借用の中だと飛ばされる。答えを返してから閉じる、メニューを開くなど、自分でメッセージを回すものは `Deferred` で。
7. **`FANTERMO_STATE` と `FANTERMO_CONFIG` を付けないで起動しない。** 付けないと利用者の `%APPDATA%\fantermo` を読み書きする。手で起動するときも同じ。
8. **クリップボードは利用者と共通。** `run-hidden` の別デスクトップでも同じウィンドウステーションなので共通（新しいウィンドウステーションは普通の権限では作れない）。`/e2e/clipboard` と OSC 52 は `Server` の既定ではファイルへ向くので触らない。手で制御の口を使うときは `FANTERMO_E2E_CLIPBOARD_FILE` を付ける。
9. **ランナーで動くか。** `smoke` で始まるテストが別デスクトップと Direct3D の試し。失敗の見当は、ログの `run-hidden:`（デスクトップや起動の失敗）、`no hardware Direct3D 11 device`（WARP に切り替えた）、`failed to render`（描画）。
