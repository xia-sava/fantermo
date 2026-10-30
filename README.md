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

- インストーラ: 開いて「インストール」を押すと，使っているユーザーだけのために `%LOCALAPPDATA%\Programs\fantermo\` に入れます．管理者権限は要りません．完了の画面で「fantermo を起動する」を選んだままにすると，「完了」で起動します．次からはスタートメニューの「fantermo」から起動してください．新しいバージョンのインストーラを開くと，前のバージョンを置き換えます．アンインストールは「設定」の「アプリ」（「プログラムと機能」）からできます．
- zip: 好きな場所に展開して，中の `fantermo.exe` を起動してください．

どちらにも，`fantermo.exe` と同じフォルダにブリッジ（`fantermo-bridge.exe` と `fantermo-bridge`）が入っています．プロファイルで Cygwin や WSL にブリッジでつなぐときに使うので，`fantermo.exe` と離さないでください．Cygwin 向けのブリッジは，入れてある Cygwin の `cygwin1.dll` などを使います．Cygwin のフォルダは，プロファイルの `cygwin_root` に書かなければ，環境変数 `PATH` とよくある場所（`C:\cygwin64`，`C:\cygwin`）から探します．Cygwin と WSL は，ConPTY よりブリッジでつなぐことを勧めます（設定ファイルの `[[profiles]]` に `backend = "cygwin"` か `backend = "wsl"` を書きます．ConPTY を通すと，アプリの出力の一部が書き換えられ，端末への問い合わせの一部が届かず，kitty graphics protocol と sixel の画像も届かず，ウィンドウの大きさを変えるとスクロールバックの行が欠けることがあります）．

タブのタイトルの左には，プロファイルのアイコンを出します．設定ファイルの `[[profiles]]` の `icon` に画像ファイル（PNG，ICO など）のパスを書くとその画像を，書かなければ，つなぎ方の絵（Cygwin の C，WSL のペンギン，ConPTY の `>_`）とプロファイル名の頭文字のアイコンを作って出します．

設定ファイルとウィンドウの状態は `%APPDATA%\fantermo\` に，ログは `%LOCALAPPDATA%\fantermo\logs\` に置きます．どちらもアンインストールしても消さないので，要らなければ手で消してください．

タブバーのメニューのボタン（≡）の「fantermo について…」で，バージョンとビルドしたコミットを見られます．インストーラで入れた fantermo は，ここの「更新を確認」で新しいバージョンを確かめて更新できます（zip から展開したものでは，このボタンは押せません）．

## ビルド

Windows + Rust（edition 2024）+ MSVC ツールチェインと，Zig が必要です．Zig のバージョンは，固定している libghostty-vt が求めるもの（`tools/dev.bat` の `ZIG_REQUIRED`）に合わせます．

```sh
./tools/dev.sh build   # git-bash 用ラッパ（MSVC 環境込みで cargo を呼ぶ）
./tools/dev.sh run
./tools/dev.sh test
```

ビルドの途中で Zig が作る補助の exe は，cargo の出力先の下の 120 文字ほど深いパスに置かれます．出力先（既定はリポジトリ直下の `target/`）のパスが 130 文字ほどを超えると，Windows のパスの上限（260 文字）を超えてビルドが失敗します．

## ライセンス

[MIT](LICENSE)

fantermo に含まれる第三者のソフトウェアのライセンスは [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt) にまとめています．アプリの「fantermo について…」でも見られます．
