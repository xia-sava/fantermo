# fantermo

Windows 用の端末アプリです．エスケープシーケンスの解釈と端末の状態管理に [libghostty-vt](https://github.com/ghostty-org/ghostty)（Ghostty の端末エミュレーションのライブラリ）を使い，窓・描画・入力・IME を Windows 向けに作ります．

名前はエスペラントの fantomo（幽霊）と terminalo（端末）から．

## 構成

Rust の workspace（edition 2024）で，2 つのクレートに分かれています．

- **`fantermo-core`** … UI に依存しない層．libghostty-vt の上に載せた端末の状態，バックエンドの trait，設定，ブリッジ（シェルとの間を中継する補助プログラム）とのプロトコル．
- **`fantermo`** … 実行ファイル．窓（Win32），描画（Direct3D 11 + DirectWrite），キー入力，IME，ConPTY．

libghostty-vt は [libghostty-rs](https://github.com/uzaaft/libghostty-rs) のバインディングを，git の依存としてコミットで固定して使います．

## インストール

[Releases](https://github.com/xia-sava/fantermo/releases) にバージョンごとのインストーラ（`fantermo-<バージョン>-x64.msi`）と zip（`fantermo-<バージョン>-x64.zip`）を置いています．

- インストーラ: 開くと，使っているユーザーだけのために `%LOCALAPPDATA%\Programs\fantermo\` に入れます．管理者権限は要りません．スタートメニューの「fantermo」から起動してください．新しいバージョンのインストーラを開くと，前のバージョンを置き換えます．アンインストールは「設定」の「アプリ」（「プログラムと機能」）からできます．
- zip: 好きな場所に展開して，中の `fantermo.exe` を起動してください．

設定ファイルと窓の状態は `%APPDATA%\fantermo\` に，ログは `%LOCALAPPDATA%\fantermo\logs\` に置きます．どちらもアンインストールしても消さないので，要らなければ手で消してください．

インストーラで入れたバージョンは，タブバーのメニューのボタン（≡）の「fantermo について…」の「更新を確認」で，新しいバージョンを確かめて更新できます．zip から展開したものでは押せません．

バージョンと，ビルドしたコミットは，タブバーのメニューのボタン（≡）の「fantermo について…」で見られます．

## ビルド

Windows + Rust（edition 2024）+ MSVC ツールチェインと，Zig が必要です．Zig のバージョンは，固定している libghostty-vt が求めるもの（`tools/dev.bat` の `ZIG_REQUIRED`）に合わせます．

```sh
./tools/dev.sh build   # git-bash 用ラッパ（MSVC 環境込みで cargo を呼ぶ）
./tools/dev.sh run
./tools/dev.sh test
```

ビルドの途中で Zig が作る補助の exe はパスが深くなるため，cargo の出力先を深い場所に移すと，Windows のパスの上限（260 文字）を超えてビルドが失敗します．出力先は既定のリポジトリ直下の `target/` のままにしてください．

## アイコン

アイコンの図案は `assets/icon/` の SVG です（`fantermo.svg` が 32 px 以上，`fantermo-small.svg` が 24 px 以下）．図案を変えたら `./tools/icon.sh` で `assets/icon/fantermo.ico` を作り直してください．SVG を描くのに Microsoft Edge を，ICO にまとめるのに Python の Pillow を使います．ICO はビルドのたびには作らず，作り直したものをコミットします．

## ライセンス

[MIT](LICENSE)

fantermo に含まれる第三者のソフトウェアのライセンスは [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt) にまとめています．このファイルと LICENSE は実行ファイルに埋め込んであり，タブバーのメニューのボタン（≡）の「fantermo について…」で見られます．Rust の crate は [cargo-about](https://github.com/EmbarkStudios/cargo-about) で集め，libghostty-vt に静的にリンクされる Zig 側のライブラリは手で確かめて `tools/licenses/zig-notices.txt` に書いています．依存を変えたら `./tools/licenses.sh` で作り直してください．
