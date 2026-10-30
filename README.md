# fantermo

Windows 用の端末アプリです．エスケープシーケンスの解釈と端末の状態管理に [libghostty-vt](https://github.com/ghostty-org/ghostty)（Ghostty の端末エミュレーションのライブラリ）を使い，ウィンドウ・描画・入力・IME を Windows 向けに作ります．

読み方は /fanˈtermo/（ファンテルモ）．

## 構成

Rust の workspace（edition 2024）で，4 つのクレートに分かれています．

- **`fantermo-core`** … UI に依存しない層．libghostty-vt の上に載せた端末の状態，バックエンドの trait，設定の読み書き，キーの割り当て，ウィンドウの状態，背景画像の選び方．
- **`fantermo`** … 実行ファイル．ウィンドウ（Win32），描画（Direct3D 11 + DirectWrite），キー入力，IME，ConPTY とブリッジのバックエンド，設定ダイアログ，更新の確認．
- **`fantermo-bridge-protocol`** … fantermo とブリッジの間でやり取りする単位の形．
- **`run-hidden`** … 開発用の道具．画面に出ないデスクトップでプログラムを起動し，決めた時間が過ぎたら終わらせる．

ブリッジ（**`fantermo-bridge`**）は，Cygwin と WSL の側で pty とシェルを持ち，fantermo との間を中継する補助プログラムです．Cygwin 向け（`fantermo-bridge.exe`）と Linux 向け（`fantermo-bridge`）にビルドするので workspace の外に置き，`./tools/bridge.sh` でビルドします．

libghostty-vt は [libghostty-rs](https://github.com/uzaaft/libghostty-rs) のバインディングを，git の依存としてコミットで固定して使います．

## インストール

[Releases](https://github.com/xia-sava/fantermo/releases) にバージョンごとのインストーラ（`fantermo-<バージョン>-x64.msi`）と zip（`fantermo-<バージョン>-x64.zip`）を置いています．

- インストーラ: 開いて「インストール」をクリックすると，現在のユーザーだけが使えるように `%LOCALAPPDATA%\Programs\fantermo\` にインストールします．管理者権限は不要です．完了の画面で「fantermo を起動する」を選んだままにすると，「完了」で起動します．次からはスタートメニューの「fantermo」から起動してください．新しいバージョンのインストーラを開くと，前のバージョンを置き換えます．アンインストールは「設定」の「アプリ」（「プログラムと機能」）からできます．
- zip: 好きな場所に展開して，中の `fantermo.exe` を起動してください．

どちらにも，`fantermo.exe` と同じフォルダにブリッジ（`fantermo-bridge.exe` と `fantermo-bridge`）が入っています．プロファイルで Cygwin や WSL にブリッジで接続するときに使うので，`fantermo.exe` と離さないでください．Cygwin 向けのブリッジは，インストールしてある Cygwin の `cygwin1.dll` などを使います．Cygwin のフォルダは，プロファイルの `cygwin_root` に書かなければ，環境変数 `PATH` と標準のインストール先（`C:\cygwin64`，`C:\cygwin`）から探します．Cygwin と WSL は，ConPTY よりブリッジで接続することを勧めます（設定ファイルの `[[profiles]]` に `backend = "cygwin"` か `backend = "wsl"` を書きます．ConPTY では，アプリの出力の一部が書き換えられたり，端末への問い合わせの一部に応答がなかったり，kitty graphics protocol と sixel の画像が表示されなかったり，ウィンドウの大きさを変えるとスクロールバックの行が欠けたりすることがあります）．

タブのタイトルの左には，プロファイルのアイコンを表示します．設定ファイルの `[[profiles]]` の `icon` に画像ファイル（PNG，ICO など）のパスを書くとその画像を，書かなければ，プロファイル名から決まる淡い色の正方形に絵を載せたアイコンを作って表示します．絵は，お使いの環境にあるアイコンを読み込みます（Cygwin は Cygwin のフォルダの `Cygwin.ico`，WSL は `wsl.exe`，ConPTY は `command` の実行ファイル）．読み込めなければ，fantermo に組み込みの絵（Cygwin の C，WSL のペンギン，ConPTY の `>_`）を表示します．同じ絵になるプロファイルが複数あるときは，プロファイル名の頭文字を添えます．

アプリは OSC 52 でクリップボードへ書き込めます（vim や tmux からのコピー）．読み取りは，設定ファイルの `[clipboard]` の `read` で決めます．既定の `"ask"` は確認のダイアログを出し（「許可」「拒否」「このタブでは今後も許可」），`"allow"` は確認せずに渡し，`"deny"` は何も返しません．読み取りは ConPTY では動作しないので，使うならブリッジで接続します．

設定ファイルとウィンドウの状態は `%APPDATA%\fantermo\` に，ログは `%LOCALAPPDATA%\fantermo\logs\` に置きます．どちらもアンインストールしても消さないので，要らなければ手で消してください．

タブバーのメニューのボタン（≡）の「fantermo について…」で，バージョンとビルドしたコミットを見られます．インストーラでインストールした fantermo は，ここの「更新を確認」で新しいバージョンを確認して更新できます（zip から展開したものでは，このボタンは押せません）．

## ビルド

Windows + Rust（edition 2024）+ MSVC ツールチェインと，Zig が必要です．Zig のバージョンは，固定している libghostty-vt が求めるもの（`tools/dev.bat` の `ZIG_REQUIRED`）に合わせます．

```sh
./tools/dev.sh build   # git-bash 用ラッパ（MSVC 環境込みで cargo を呼ぶ）
./tools/dev.sh run
./tools/dev.sh test
```

e2e テスト（実際の fantermo を画面に出ないデスクトップで起動して確認する）は `./tools/dev.sh test --features e2e` で実行します．出力先は `target/e2e/` です．

ビルドの途中で Zig が作る補助の exe は，cargo の出力先の下の 120 文字ほど深いパスに置かれます．出力先（既定はリポジトリ直下の `target/`）のパスが 130 文字ほどを超えると，Windows のパスの上限（260 文字）を超えてビルドが失敗します．

## ライセンス

[MIT](LICENSE)

fantermo に含まれる第三者のソフトウェアのライセンスは [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt) にまとめています．アプリの「fantermo について…」でも見られます．
