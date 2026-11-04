# fantermo のコードの地図

機能を足したり不具合を直したりするときに，どこを読めばよいかをまとめたものです．細部は各モジュールの先頭のコメント（`//!`）と doc コメントにあります．ここではその場所を示し，同じ説明は繰り返しません．利用者から見た振る舞いとそう決めた理由は [design.md](design.md) にあります．

## 1. 全体像

### crate と依存の向き

```
fantermo ──────────────┬──> fantermo-core ──> libghostty-vt（git の依存）
  （実行ファイル）      └──> fantermo-bridge-protocol
fantermo-bridge ───────────> fantermo-bridge-protocol
  （workspace の外）
run-hidden（開発用の道具．ほかの crate に依存しない）
```

- **`crates/fantermo-core`** … UI に依存しない層．libghostty-vt を包む端末エミュレーション（`vt`），バックエンドの trait（`backend`），設定の型と解釈・書き戻し，キーの割り当て，リンク，ウィンドウの状態，背景画像の選び方．Win32 には依存しないので，単体のテストで多くを固定できます．
- **`crates/fantermo`** … 実行ファイル．ウィンドウ（Win32），描画（Direct3D 11 + DirectWrite），キー入力，IME，ConPTY とブリッジのバックエンド，ダイアログ，メニュー，更新の確認と適用．
- **`crates/fantermo-bridge-protocol`** … fantermo とブリッジの間で流す単位の形．依存は `std` だけで，`unsafe` を使いません（`#![forbid(unsafe_code)]`）．`fantermo-core` に置かないのは，`fantermo-core` が libghostty-vt（Zig）に依存していて，ブリッジのターゲットでビルドできないためです．
- **`crates/fantermo-bridge`** … ブリッジ．Cygwin と WSL の側で pty とシェルを持ち，標準入出力でプロトコルを話します．`x86_64-pc-cygwin` と `x86_64-unknown-linux-musl` 向けにだけビルドするので，workspace から `exclude` で外しています．自分の `Cargo.lock` を持ち，`[workspace.dependencies]` を使いません．`./tools/dev.sh fmt` と CI の `check` ジョブの fmt・clippy はブリッジを見ません（`./tools/bridge.sh` で見ます）．
- **`crates/run-hidden`** … 利用者の画面に出ない別デスクトップでプログラムを起動し，決めた時間が過ぎたら終わらせる開発用の道具．e2e テストの土台が使います．

fantermo は普通の Win32 アプリで，`cygwin1.dll` を読み込みません（ランタイムの混在，`fork` やシグナルの不整合，DLL のバージョンの不一致を避けるため）．Cygwin と WSL の pty はブリッジの側で持ちます．

### リポジトリのほかの場所

| 場所 | 役割 |
|---|---|
| `tools/dev.sh`・`tools/dev.bat` | ビルドのラッパ．`dev.bat` が vcvars64 を読み，Zig のバージョン（`ZIG_REQUIRED`）を確認して cargo を呼ぶ．引数に `--features e2e` があれば出力先を `target/e2e/` にする |
| `tools/bridge.sh` | ブリッジの build・clippy・test・fmt（4 節） |
| `tools/licenses.sh`・`tools/licenses/` | `THIRD-PARTY-NOTICES.txt` を作る．依存を変えたら作り直してコミットする．手順は先頭のコメント |
| `tools/manifest.sh` | 更新の情報 `latest.json` を作り，署名し，検証する．配布のワークフローが使う |
| `tools/icon.sh`・`tools/icon/` | `assets/icon/*.svg` から `fantermo.ico` を作る．図案を変えたときだけ実行する |
| `installer/` | MSI の定義（WiX）．`fantermo.wxs` と日本語の文 `fantermo.ja-JP.wxl` |
| `assets/` | アイコンと，`latest.json` の署名を検証する公開鍵 `manifest-public-key.der` |
| `.github/workflows/ci.yml` | main への push と PR の検証（7 節）．配布のワークフローからも `workflow_call` で呼ぶ |
| `.github/workflows/release.yml` | `vMAJOR.MINOR.PATCH` のタグで配布物（zip，MSI，署名した `latest.json`）を作って Release に載せる |
| `.github/actions/` | 道具を入れる action．`setup-toolchain`（fantermo）と `setup-bridge-toolchain`（ブリッジ） |
| `.cargo/config.toml` | git の依存を git コマンドで取得する．libghostty-vt を常に Zig の ReleaseSafe でビルドする（理由はファイルのコメント） |
| `.claude/skills/` | e2e の実行と書き方（`e2e-verify`），バージョンの上げ方と配布（`release`） |

## 2. モジュールの地図

### `fantermo-core`

| モジュール | 受け持つこと |
|---|---|
| `vt` | libghostty-vt の端末を包む．出力の流し込み（`feed`），応答（`take_replies`），キーとマウスの符号化，選択，貼り付け，スクロール，描画用の画面の取り出し（`frame`）．画像（kitty graphics protocol と sixel）は `vt/images.rs`，リンクの検出は `vt/links.rs`，テストは `vt/tests/` に分けてある．libghostty-vt を直接使うのはここだけで，ほかはここが再公開した型（`Key`，`Mods` など）を使う |
| `output_scan` | 出力を端末エミュレーションへ流す前に見て，sixel の DCS と OSC 52 の読み取りの依頼を取り出す（理由は先頭のコメント） |
| `sixel` | sixel のデコーダ．`vt` が kitty graphics protocol の画像にして端末に置く |
| `frame` | 描画のために取り出した画面．libghostty-vt の型に依存しない |
| `backend` | `Backend` trait，`SpawnOptions`，`GridSize`，`Wake` |
| `config` | 設定ファイルの各節の型と既定値，ひな形 `TEMPLATE`．数か名前一つだけの項目の一覧 `SCALARS` は，読む処理，書き戻す処理，設定ダイアログが使う．下の 3 つのモジュールの名前は `config::` の下に再公開する |
| `config::profile` | プロファイル（`Profile`，`BackendKind`），`shell` のコマンドラインの解釈，Cygwin のフォルダを探す処理（`find_cygwin_root`），名前だけのコマンドを `PATH` から探す処理（`find_program_on_path`），ブリッジの相手の側に渡すコマンドライン（`shell_command`）・`WSLENV`（`wslenv_with`）・パス（`wsl_path`）の形 |
| `config::colors` | 配色，端末・タブバー・スクロールバーの色のキー，`"#rrggbb"` の読み書き |
| `config::read` | 設定ファイルの中身の解釈（`parse`）．警告にエラーのある行を添える |
| `config::migrate` | 古い書き方を今の書き方に読み替える手順 `STEPS` |
| `config_edit` | 設定ダイアログで変えた項目だけを設定ファイルに書き戻す（`update`）．コメントと並びを残す．下の 3 つのモジュールに分けてある |
| `config_edit::document` | ほかのモジュールが使う，`toml_edit` の文書を書き換える共通の処理．文書の読み書き（`Document`），テーブルとその位置を探す処理，項目の行を書く（`set`）・既定値に戻す（`reset`）処理 |
| `config_edit::images` | 背景画像の一覧（`[[background.images]]`）の書き戻し |
| `config_edit::profiles` | プロファイル（`[[profiles]]`）の書き戻し |
| `keymap` | fantermo が受け持つキーの割り当て（`Action`，`KeyCombo`，`Keymap`）と名前の解釈 |
| `link` | OSC 8 と画面の文字の URL．開いてよいリンクかの判断 |
| `clipboard` | アプリからのクリップボードの読み書きの決まり（`clipboard.read` とタブごとの許可，確認を待つ `ReadPrompts`） |
| `background` | 背景画像の候補の展開，ランダムな選択，表示方法ごとの位置の計算 |
| `state` | ウィンドウの状態ファイル（`state.toml`）の型 |
| `base64` | base64 への変換 |

### `fantermo`

| モジュール | 受け持つこと |
|---|---|
| `main` | 起動の順番（3 節の「起動」） |
| `command_line` | コマンドライン（`--elevate`，`--new-window`，`--profile`，`--`）の解釈 |
| `instance` | 動いている fantermo のウィンドウへ起動の指定を `WM_COPYDATA` で渡す側と受け取る側 |
| `elevate` | 管理者の権限の fantermo の起動（`ShellExecuteExW` の `runas`）．渡す引数の組み立て，このプロセスが管理者として動いているかの判定（`this_process`）．feature `e2e` のときは `elevate/stand_in` で差し替える |
| `app_files` | `%APPDATA%\fantermo` のファイルの場所（環境変数での差し替え），一時ファイルを経由した置き換え書き込み，パスの比較（`same_path`） |
| `config_file` | 設定ファイルの場所，読み込み，ひな形の作成，書き戻し（`save`），フォルダの監視 |
| `window_state` | `state.toml` の場所と読み書き，ウィンドウの位置・大きさ・最大化との受け渡し |
| `window` | 端末のウィンドウ．Win32 のメッセージを受け，端末・バックエンド・描画をつなぐ（下の節） |
| `window/e2e` | e2e テストの制御の口のうち，ウィンドウのスレッドで動く側（feature `e2e`） |
| `e2e_server` | 制御の口のうち HTTP を受ける側（feature `e2e`）．要求の一覧は先頭のコメント |
| `session` | 端末一つ分の状態（`Session`）．`Vt` とバックエンド，タブごとの状態．バックエンドを作るのは `spawn_backend` の一か所．出力と終了の知らせを端末ごとに一つにする印（`WakeMessage`） |
| `conpty` | ConPTY のバックエンド |
| `bridge` | ブリッジのバックエンド |
| `output_budget` | ConPTY とブリッジの読むためのスレッドが渡して，まだ受け取られていない出力のバイト数の上限，`Wake` のまとめ方，子プロセスが終わったあとの最後の知らせの積み直し（`sent_last`）（3 節の「バックエンドの出力から描画まで」） |
| `process` | プロセスのハンドルを開く・待つ・終了コードを読む・終わらせる処理，動いているプロセスの一覧，子プロセスの出力のパイプを閉じるまで読む処理．`conpty`，`bridge`，`instance`，`update` が使う |
| `keyboard` | Win32 のキーのメッセージを端末のキーの情報に直す |
| `ime` | IME の変換中のウィンドウと候補のウィンドウをカーソルの位置に出させる |
| `renderer` | Direct3D 11 でウィンドウに描く．スワップチェーン，背景画像と端末の中の画像とタブのアイコンのテクスチャ |
| `pipeline`・`cells.hlsl`・`shader` | 四角形のインスタンスを描くパイプライン，そのシェーダ，実行時のコンパイル（`D3DCompile`） |
| `atlas` | グリフアトラス（2048×2048）．DirectWrite で組んだ文字を Direct2D で描く |
| `text_placement` | 組んだ文字の送り幅とインクの範囲（`text_layout`），アトラスの場所への置き方（`place_text`）．`atlas` と `ui_font` が使う |
| `d2d_shape` | `boxdraw::Shape` を Direct2D の描画先に描く．`atlas` と `tab_icon` が使う |
| `font`・`ui_font` | 端末のフォントを探してセルの寸法を測る．タブバーのタイトルに使う Windows の UI のフォント |
| `boxdraw`・`boxdraw/` | 罫線，ブロック要素，点字，Powerline の記号，レガシーコンピューティング記号の一部を形として組み立てる |
| `decoration` | 下線・取り消し線・上線をセルの中の形として組み立てる |
| `paint` | セルの格子によらずピクセルの位置で描くもの（`Paint`）．タブバーとスクロールバーが使う |
| `tab_bar`・`scroll_bar` | 並べ方，当たり判定，色（`Palette`），描くものの組み立て．ウィンドウと描画に依存しない関数にしてある．`tab_bar` は寸法とタイトルの省略を `tab_bar.rs`，並べ方と当たり判定を `tab_bar/layout.rs`，色と描くものの組み立てを `tab_bar/paints.rs` に置く |
| `tab_icon`・`tab_icon/` | タブのタイトルの左のアイコン．`tab_icon.rs` がプロファイルからアイコンを決め，`tab_icon/draw.rs` の `Generator` がピクセルを作る（画像を読み，読めなければ絵と頭文字を描く．Direct3D のデバイスを使わない）．開いているタブのピクセルは `Pixels` に持ち，`App::update_tab_icons` がタブを開いたとき・閉じたとき，DPI か UI のフォントの書体が変わったときに揃える |
| `image_decoder` | 画像のファイルと実行ファイルのアイコンと Windows のストックアイコンを WIC で読む（背景画像，タブのアイコン，管理者の印の盾）．読んだ画像 `Decoded` とその重ね合わせ，Direct3D 11 のテクスチャの一辺の上限．アイコンのコマの選び方と正方形への収め方，盾の大きさと置き場所（`overlay_shield`） |
| `background`・`image_cache` | 背景画像の候補を実際のファイルシステムから探す（`DiskFiles`）．端末の中の画像のテクスチャの入れ物 |
| `context_menu` | コンテキストメニュー，≡ のメニュー，プロファイルの一覧（`MenuCommand`，`Menu`） |
| `gdi_image` | 読んだ画像 `Decoded` から GDI のビットマップとアイコンを作る．`context_menu` と `app_icon` が使う |
| `settings_dialog`・`key_capture` | 設定ダイアログ．キーの割り当てで押したキーを読み取るダイアログ．`settings_dialog` は，ダイアログ全体（`State`，`dialog_proc`，ページの型が使う `Page` と `DialogContext`）を `settings_dialog/mod.rs`，ページを `appearance.rs`・`background.rs`・`behavior.rs`・`profiles.rs`・`keys.rs`・`colors.rs`，コントロールを作る部品と種類ごとの部品を `widgets.rs` に置く |
| `text_dialog`・`paste_editor`・`about` | 入力欄のダイアログと，その上に作った貼り付けの編集と「fantermo について」 |
| `modal` | 自分で作ったダイアログを開いている間のメッセージのループ |
| `dialog` | 自分で作るダイアログ（設定ダイアログ，入力欄のダイアログ，キーを読み取るダイアログ）に共通の処理．クラスの登録，状態の結び付け（`attach_state`・`state_of`・`on_destroy`），96 DPI の寸法の拡大（`scale`），置き場所，フォント（`OwnedFont`）．メッセージボックスを開く `message_box` も置く |
| `notice`・`clipboard_prompt` | 警告のメッセージボックス（話題 `Topic` ごとに一つ）．クリップボードの読み取りの確認 |
| `clipboard`・`link_opener`・`file_dialog`・`known_folder` | Windows のクリップボード，リンクと設定ファイルを開く処理，ファイルを選ぶダイアログ，既知のフォルダ |
| `update`・`update/` | 更新の確認と適用（3 節の「更新の確認と適用」） |
| `version`・`app_icon`・`logging`・`dump` | バージョンとコミット，アイコン（管理者のときは盾を重ねる），ログの出し先，`FANTERMO_DUMP` での画面の書き出し |

`build.rs` は，メインスレッドのスタックを 8 MB にし，Common Controls 6 のマニフェストとアイコンのリソースを埋め込み，コミットの短い SHA とリリースのビルドか（`FANTERMO_RELEASE=1`）を埋め込みます．

### `window` の中

`App`（ウィンドウ一つ分の状態）は `window/mod.rs` に置き，そのメソッドは話題ごとのファイルの `impl App` に分けてあります．ほかのファイルから呼ぶものだけに `pub(super)` を付けます．

| ファイル | 受け持つこと |
|---|---|
| `window/mod.rs` | メッセージの定数（`WM_APP_WAKE`（出力と子プロセスの終了），`WM_APP_CONFIG_CHANGED`，`WM_APP_UPDATE`，`WM_APP_CLIPBOARD_PROMPT`，`WM_APP_E2E`，`WM_APP_LAUNCH`，`WM_APP_DEVICE_REMOVED`）とタイマーの番号．`run` がウィンドウと `App` を作り，メッセージループを回します．`App` の構造体と `App::new`（端末の並び（`sessions` と `active`），フォント，`Renderer`，いま使っている設定の値，マウスとタブバーとスクロールバーの操作の途中の状態など）．`GWLP_USERDATA` が指す `WindowData`（`App` の `RefCell` と状態ファイルの場所）．ウィンドウにメッセージを積む `post`・`poster`・`wake_poster` |
| `window/proc.rs` | `window_proc`（メッセージごとの振り分けと，`App` を借りる `with_app`），借用の外ですること（`Deferred` と `run_deferred`，`destroy_window`，`bring_to_front`），借用の有無によらず状態ファイルを書く `save_window_state`，メニューを開いている間か（`in_menu`），借用が重なって飛ばしたメッセージの積み直し（`is_repostable`，`repost_skipped_messages`），`app_of` |
| `window/layout.rs` | 配置の関数（`Insets`，`terminal_area_in_client`，`scroll_bar_area_in_client`，`backdrop_area_in_client`，`grid_size`）．クライアント領域を，タブバー，端末を描く領域，余白，スクロールバー，背景画像を敷く領域に分けます．座標の変換と，大きさ（`on_resize`，`layout`，`resize_sessions`） |
| `window/dpi.rs` | DPI とフォント（`apply_dpi`，`apply_font`．フォントは `font::find_or_default` で探す），起動したときの DPI の変更（`change_dpi_keeping_window`，`change_dpi_keeping_grid`） |
| `window/keys.rs` | キー（`on_key`，`on_key_input`，`run_bound_action`），IME，端末へ送る入力（`send`） |
| `window/tabs.rs` | タブ（`run_tab_command`，`open_startup`，`close_session`，`activate`），起動中の起動（`accept_launch`，`open_pending_launches`），出力（`on_wake`，`continue_output`），タイトルとベル，最初の端末で起動するもの（`startup_for`） |
| `window/mouse.rs` | マウスのボタンと動きとホイール，選択，リンク，アプリへの報告 |
| `window/scroll_bar.rs`・`window/tab_bar.rs` | スクロールバーとタブバーの操作と，描くものの組み立て |
| `window/menu.rs` | メニューを出す要求と，選ばれた項目の働き（`run_menu_command`） |
| `window/clipboard.rs` | 貼り付けとコピーと選択，アプリからのクリップボードの読み書きの依頼と読み取りの確認 |
| `window/settings.rs` | 設定（`reload_config`，`apply_config`，`settings_request`），設定ダイアログ（`edit_settings`，`SettingsHost`） |
| `window/render.rs` | 描画（`render_to`，`on_device_lost`，`on_device_removed_message`，`recreate_renderer`），背景画像の選び方，点滅と画像のアニメーションのタイマー |
| `window/update.rs` | 「fantermo について」（`show_about`）と更新の続き（`continue_update`） |
| `window/e2e.rs` | e2e テストの制御の口のうち，ウィンドウのスレッドで動く側（feature `e2e`） |

ウィンドウプロシージャの中では `App` を `RefCell` で借ります．処理の途中で同じウィンドウへのメッセージがこの場で届くと借用が重なるので，借りられなければそのメッセージを飛ばします（`window_proc` の `with_app`）．飛ばした `WM_APP_WAKE`，`WM_APP_DEVICE_REMOVED`，`WM_CLOSE` は，借用を手放したところで積み直します（`is_repostable`，`repost_skipped_messages`）．

ダイアログ，メニュー，メッセージボックスのように自分でメッセージを回すものは，借用を手放してから開きます．借用を持ったまま開くと，その間に届いた出力や描き直しが飛ばされるためです．入力の処理は開くものを `Deferred` で返し，`window_proc` が借用を外して `run_deferred` で開きます．借用の外で呼ぶものは，`run_deferred` とそこから呼ぶ `show_about`・`continue_update`（`window/update.rs`），`run_menu_command`（`window/menu.rs`），`edit_settings`・`reload_settings`（`window/settings.rs`），`destroy_window`，ウィンドウを前面に出す `bring_to_front` です．`DestroyWindow` も同じ理由で借用の外で呼びます．`destroy_window` は `with_app` で `App` を借りられたとき（ほかに借用がないとき）だけ状態を書いて `DestroyWindow` を呼びます．開いている間に最後の端末が終わるとウィンドウが閉じて `App` が解放されるので，`with_app` は呼ばれるたびに `App` を読み直し，もうなければ何もしません．

`GWLP_USERDATA` が指すのは `WindowData`（`window/mod.rs`）で，`App` の `RefCell` と，借用が重なっていても読む値（状態ファイルの場所）を持ちます．`save_window_state` だけは借用の有無によらず呼べます（3 節の「ログオフ」）．ほかのスレッドから送られた `WM_CLOSE` が，借用の中の同期の呼び出し（貼り付けの `GetClipboardData` など）の間に届いたときは，閉じずに飛ばして積み直します．積み直したメッセージは借用の中では取り出されないので，借用を手放したあとで処理されます．

## 3. 主な流れ

### 起動

1. `main` がログを初期化し（`logging`），コマンドラインを読み（`command_line::Options::of_process`），DPI の対応を Per-Monitor V2 にし，COM を初期化します（WIC，ファイルを選ぶダイアログ，`ShellExecuteExW` に使う）．
2. `--elevate` があり，管理者として動いていなければ（`elevate::this_process`），`elevate::launch` で管理者の権限の fantermo を起動して終わります．動いているウィンドウへ渡す前に判断します．
3. `--new-window` がなければ `instance::forward` で動いているウィンドウへ渡します．渡せたらそこで終わります．渡さない条件（応答しない，管理者かどうかが違う，実行ファイルか設定ファイルが違う）は `instance` の先頭のコメントにあります．
4. `config_file::load` が設定ファイルを読みます．なければ `TEMPLATE` で作ります．古い書き方は `config::migrate` で読み替えて書き戻します．`window_state::load` が `state.toml` を読みます．場所は環境変数 `FANTERMO_CONFIG`・`FANTERMO_STATE` で差し替えられます（テストはこれを使います）．
5. `window::run` がシステムの DPI でフォントを測ってウィンドウを作り，覚えていた位置と大きさを見せる前に戻します（`window_state::restore_hidden`）．`App::new` が最初の端末（`Session::spawn`）を開き，設定ファイルの監視を始めます．
6. ウィンドウのモニターの DPI がシステムの DPI と違えば，DPI の変更と同じ処理で作り直します．そのあと `ShowWindow` し，設定ファイルの警告とコマンドラインの誤りを見せてから，メッセージループに入ります．

最初の端末で何を起動するかは `startup_for` が決めます（名前の合うプロファイル，コマンドライン，既定のプロファイルの順）．

受け取る側は `WM_COPYDATA` で受け取ったかどうかだけを答え（`App::accept_launch`），開くのは積んだ `WM_APP_LAUNCH` で行います．ダイアログやメニューを開いている間は開かず，閉じたとき（`WM_ENABLE`，`WM_EXITMENULOOP`）に開きます．積んだままウィンドウが閉じたら，`WM_DESTROY` で `--new-window` を付けて起動し直します．

### キーと IME の入力からバックエンドまで

1. `WM_KEYDOWN`・`WM_SYSKEYDOWN` で，`VK_PROCESSKEY`（IME が処理中）と，Alt を含む F4・Space は Windows に任せます．
2. `take_translated_chars` が，そのキーから `TranslateMessage` が積んだ `WM_CHAR` を取り出します．デッドキーなら何も送りません．
3. `App::on_key` が `keyboard` でキーの種類（スキャンコードから，物理的な位置），修飾キー，修飾なしの文字を決め，`on_key_input` に渡します．
4. `keymap.lookup` で fantermo の動作に割り当てたキーなら `run_bound_action` が行います．成り立たなければ（選んでいないときのコピーなど）シェルへ送ります．動作に使ったキーは離したときも送りません（`suppressed_release`）．
5. それ以外は `Vt::encode_key` が端末のモード（カーソルキーのモード，kitty keyboard protocol など）に合わせて符号化し，`App::send` がスクロールを最新の行に戻し，選択を外して `Session::write` からバックエンドへ書きます．

IME の確定した文字と，キーを伴わない `WM_CHAR` は `on_text_unit` が `Utf16Assembler` でサロゲートをつないで UTF-8 で送ります．変換中の文字列は IME 自身に描かせ，`WM_IME_STARTCOMPOSITION`・`WM_IME_COMPOSITION` で変換中のウィンドウと候補のウィンドウをカーソルの位置に出させます（`ime`）．キーの細部（Ctrl の組み合わせの制御文字を付けない理由など）は `keyboard.rs` のコメントにあります．

### バックエンドの出力から描画まで

1. バックエンドの読むためのスレッドが出力を読み，チャネルに入れて `Wake` を呼びます．`Wake` は `WM_APP_WAKE` をその端末の `SessionId` を添えて積みます．端末の状態は `!Send` なので，ウィンドウのスレッドだけが持ちます．チャネルに入っていて，ウィンドウのスレッドがまだ受け取っていないバイト数には 64 MiB の上限を置きます（`output_budget`）．次の塊を足すと上限を超えるなら，読むためのスレッドは `drain_output` で受け取られるまで待ち，その間は読まないので，pty の側でアプリの出力が止まります．ウィンドウのスレッドが遅れても，メモリに溜まり続けません．破棄すると待っているスレッドを起こし，読むためのスレッドはそれ以上渡さず，`Wake` も呼びません．端末を閉じたときも，ウィンドウを壊すとき（`WM_NCDESTROY` で `App` を解放するとき）も，ウィンドウのハンドルを解放する前にバックエンドを破棄するので，ほかのウィンドウに使い回されたハンドルの値へ積むことはありません．`Wake` も `output_budget` でまとめ（ConPTY の子プロセスの終了の知らせも同じ），知らせたあとウィンドウのスレッドが `drain_output` で受け取り始めるまでは知らせ直しません．さらに `Wake` は端末ごとの印（`session::WakeMessage`）を通して積みます．印は積めたときに立て，`App::on_wake` が出力を受け取る前に下ろします．2 の続きの知らせも同じ印を通すので，積んでまだ処理されていない `WM_APP_WAKE` は端末ごとに高々一つで，大量の出力の間も知らせがキューを埋めません．借用が重なって `WM_APP_WAKE` を飛ばしたときは，借用を手放したところで積み直します（`repost_skipped_messages`．飛ばした知らせは `on_wake` を通らず印が立ったままなので，その端末の知らせはほかに積まれないため）．`WM_APP_WAKE` を積めなかったときは，端末ごとに一度だけ `tracing::warn!` に残します（二度目からと，ウィンドウが壊れたあとは debug）．飛ばした知らせの積み直しに失敗したときは，同じ知らせを続けて積めない間は一度だけ warn に残します．`Wake` は積めたかを返し，読むためのスレッドは積めなかった知らせを次の出力でまた積みます．子プロセスが終わったあとの最後の知らせ（ConPTY の終了，ブリッジの終わりの単位かエラーを受け取ったことと出力を閉じたこと）は次の出力がないので，積めるまで `WAKE_RETRY_INTERVAL` ごとに積み直します（`OutputBudget::sent_last`）．起動できなかったブリッジのエラーの文の知らせは，ウィンドウのスレッドから一度だけ積み，積めなければ積み直しません（ウィンドウのスレッドでは積めるまで待てないため）．
2. `App::on_wake` が `Session::on_output` を呼びます．`drain_output` で溜まった出力を `Vt::feed` に流し，端末の応答（`take_replies`）をバックエンドへ書き，タイトルの変化とベルと子プロセスの終了を返します．表示していない端末の出力も流し込みます．一回で流すのは `MAX_OUTPUT_PER_CALL`（1 MiB．値の根拠は定数のコメント）までで，読み取った塊は分けません．残りがあれば，`App::continue_output` が次のメッセージで続きを同じ順番のまま流します．積んだメッセージはキー入力と描き直しより先に取り出されるので，それらがキューにあれば（`GetQueueStatus`）`OUTPUT_TIMER_ID` のタイマーで，なければすぐに `WM_APP_WAKE` を積みます．積めなければタイマーで流します．タイマーで続きを待っている端末（`output_continuations`）に届いた `WM_APP_WAKE` は流さずに戻り，続きはタイマーに任せます（読むためのスレッドの知らせが続きを追い越して流し続けると，キー入力と描画が後回しになるため）．こうして，溜まった出力を流している間も，キー入力，描画，タブの切り替え，閉じる操作が処理されます．表示している端末は描き直しが待っていることが多いので，続きはたいていタイマーを通り，1 MiB ごとに少なくともタイマーの刻み（`USER_TIMER_MINIMUM` の 10 ms 以上）がかかります．流すのが遅れても，描画と入力を先にするためです．子プロセスの終了は，チャネルに溜まった分を流しきってから扱います（ConPTY の `poll_exit` は出力と関係なくプロセスの終了を見るので，先に扱うと最後の出力を捨てたままタブを閉じるため）．パイプと conhost に残っている分までは待ちません．
3. `Vt::feed` の中では `output_scan` が sixel と OSC 52 の読み取りの依頼を取り出し，残りのバイトを順番を変えずに libghostty-vt へ渡します．sixel は `sixel` でデコードし，kitty graphics protocol の画像として端末に置きます．
4. 表示している端末なら `invalidate` し，`WM_PAINT` で `render_to` が `Vt::frame` で画面を取り出し，タブバーとスクロールバーの `Paint` を組み，`Renderer::render` で描きます．
5. `Renderer` は背景画像，セルの背景，カーソル，グリフ，装飾の順に四角形のインスタンスを `DrawList` に並べ，`cells.hlsl` で描きます．描画命令は，テクスチャ（背景画像，端末の中の画像，タブのアイコン）が変わるところだけ分けます．アトラスがいっぱいになったら全部を捨てて組み直し，組み直しても入らないものは飛ばして入った分だけ描きます（design.md 7 節）．

### リサイズ

`WM_SIZE` で `App::on_resize` がスワップチェーンの大きさを変え，`layout` が各領域を計算し直し，`resize_sessions` がすべての端末（表示していないものも）を同じ桁数と行数にします．最小化（`SIZE_MINIMIZED`）の `WM_SIZE` では何もしません（大きさ 0 を端末へ伝えないため）．設定の読み直しと DPI の変更からの `fit_to_client` も，最小化中は何もしません．`Session::resize` は大きさが変わったときだけ `Vt::resize` とバックエンドの `resize` を呼びます．ConPTY とブリッジは，前に伝えた大きさと同じなら伝えません．

ウィンドウの大きさが基準です．フォント，スクロールバーの幅，余白を変えても，ウィンドウの位置と大きさは変えず，いまのクライアント領域で桁数と行数を決め直します．

### 設定の読み直し

1. `config_file::watch` のスレッドがフォルダの変化を見て `WM_APP_CONFIG_CHANGED` を積みます．ウィンドウは `CONFIG_RELOAD_DELAY` のタイマーで知らせをまとめ，止んでから一度だけ読み直します．
2. `App::reload_config` が `config_file::reload` で読みます．エラーのある項目は前の値のまま，TOML として読めなければ全体を前の値のままにします．警告は前と同じ中身（`notice_key`）なら続けて見せず，エラーがなくなれば `notice::withdraw` で取り下げます．
3. `App::apply_config` が値を入れ替えます．フォントが変われば，`apply_font_config` で測り直したフォントを `apply_font` に渡してアトラスを作り直し，フォント・スクロールバーの幅・余白が変われば `layout` します．プロファイルは次に開く端末から使います．
4. 設定ダイアログを開いている間は読まず（`settings_dialog_open`），閉じてから読みます．ダイアログの試し表示（`Host::preview`）も同じ `apply_config` を通ります．

### DPI の変更

`WM_DPICHANGED` で，`App::apply_dpi` が端末のフォントを測り直し，`apply_font` で UI のフォントを測り直してアトラスを作り直し，`update_tab_icons` でタブのアイコンをその DPI の一辺で作り直します．そのあと `window_proc` がウィンドウのアイコンを付け直し，推奨の矩形へ `SetWindowPos` します．変わったあとの大きさ（`WM_GETDPISCALEDSIZE`）は既定の処理に任せ，Windows が DPI の比で拡大・縮小します．そのあと届く `WM_SIZE` で桁数と行数が決まります．クライアント領域の大きさが変わらず `WM_SIZE` が届かないときのために，最後に `fit_to_client`（`on_resize`）を呼びます．最小化中は呼んでも何もせず，戻したときの `WM_SIZE` に任せます．`state.toml` は位置と大きさをピクセルのまま覚え，DPI は書きません．

タブバーとスクロールバーの寸法は DPI から決め（`paint::scale`），端末のフォントによりません．設定ダイアログと入力欄のダイアログは，開くときにモニターの DPI（`GetDpiForWindow`）を読み，開いている間に DPI が変われば（ダイアログに届く `WM_DPICHANGED`）推奨の矩形へ移り，フォントを作り直して並べ直します．端末のウィンドウに届く `WM_DPICHANGED` は，ダイアログへは伝えません．

### タブを閉じるとき，ウィンドウを閉じるとき

- タブ: `App::close_session` が `Session` を取り除いて破棄します．あわせて，そのタブの確認待ちのクリップボードの依頼，端末の中の画像のテクスチャ，使われなくなった背景画像とタブのアイコンを捨てます．最後のタブなら閉じずにウィンドウを閉じます．子プロセスが終わったとき（`on_session_ended`）も同じで，最後のタブならその終了コードがプロセスの終了コードになります．
- バックエンドの破棄: `ConPty` と `Bridge` の `Drop` は待たずに戻り，片づけを別のスレッドに任せます．ConPTY は書きかけの入力を捨て，疑似コンソールを閉じ，猶予のあと，そのコンソールに接続しているプロセスと，接続し終えないまま止まっているコンソールのプログラムを終わらせます（`conpty.rs` の先頭のコメントと `Drop`）．ブリッジは終了の依頼を送り，書きかけの入力を捨てて標準入力を閉じ，5 秒で終わらなければ `TerminateProcess` します．ブリッジの側は SIGHUP を送り，シェルが終わったあとは前面のプロセスグループへ送り直します（`fantermo-bridge` の `hang_up_wait`）．
- ウィンドウ: × と Alt+F4（`WM_CLOSE`），メニューの終了，最後のタブは，どれも `destroy_window` で `state.toml` を書いてから `DestroyWindow` します．`WM_NCDESTROY` で `App` を解放すると，すべての `Session` が破棄されます．`main` は `conpty::wait_for_closing_consoles` で疑似コンソールの片づけを上限まで待ってから終わります（プロセスが終わると閉じるスレッドも終わり，接続の途中だったプロセスが残るため）．

### ログオフ

`WM_QUERYENDSESSION` と `WM_ENDSESSION` で `state.toml` を書きます．ウィンドウは壊さず，端末と子プロセスはほかのプロセスと一緒に終わらされるのに任せます．問われた時点でも書くのは，`ENDSESSION_CRITICAL` では `WM_ENDSESSION` のあとに時間がほとんどないためです（`window_proc` のコメント）．この二つは，`App` を借りている間の同期の呼び出し（貼り付けの `GetClipboardData` など）の中でも届き，そのときも書きます．`App` を借りずに書く決まりは `window/proc.rs` の `save_window_state` のコメントにあります．

### 更新の確認と適用

1. 「fantermo について」（`show_about`）の「更新を確認」は，`update::availability` が `Available` のとき（インストーラで `%LOCALAPPDATA%\Programs\fantermo` に入れたリリースのビルドで，管理者として動いていない）だけ押せます．
2. `update::start(Work::Check)` が別のスレッドで `latest.json` と署名を WinHTTP で取得し（`update/http`），埋め込みの公開鍵で ECDSA P-256 の署名を検証し（`update/signature`），バージョンを比べます（`update/manifest`）．結果は `update_events` に送り，`WM_APP_UPDATE` で知らせます．「fantermo について」を開いている間はそのループが先に受け取ります．
3. `update/prompt` が結果を見せて次を決めます．更新するなら `Work::Download` で MSI をダウンロードして SHA-256 を照合し，ほかの fantermo が動いていれば知らせて確認し，`update/apply` のスクリプト（PowerShell）に引き渡してからウィンドウを閉じます．スクリプトは fantermo の終了を待ち，MSI の SHA-256 を照合し直してから `msiexec` で入れ，fantermo を起動し直します．
4. feature `e2e` のビルドは，取得先・公開鍵・いまのバージョン・スクリプトの起動と，インストーラで入れたものかの判定を `update/stand_in` で差し替え，`127.0.0.1` のほかへ接続せず，スクリプトも起動しません．管理者の判定は差し替えず，`elevate::this_process`（`elevate/stand_in` で差し替えられる）を使います．

## 4. バックエンド

### `Backend` trait

`fantermo_core::backend::Backend` は `write`（入力），`resize`（大きさ），`drain_output`（溜まった出力を上限まで渡す．残したら `true`），`poll_exit`（終了コード）の 4 つです．どれもウィンドウのスレッドから呼ばれるので，待たずに戻ります．出力が届いたときと子プロセスが終わったときは，起動のときに受け取った `Wake` を呼びます．破棄すると子プロセスとの接続を閉じて後始末をします．種類はプロファイルの `backend`（`BackendKind`: `conpty`・`cygwin`・`wsl`）で，`session::spawn_backend` の一か所で作ります．子プロセスへ渡す環境変数（`TERM=xterm-256color`，`TERM_PROGRAM`，`TERM_PROGRAM_VERSION`）は `session.rs` の `CHILD_ENV` です．

テストでは偽のバックエンドで `Session` を試せます（`session.rs` のテスト）．

### ConPTY（`conpty.rs`）

疑似コンソールと 2 本の匿名パイプで接続します．入力は書くためのスレッドに任せ，ウィンドウのスレッドはチャネルに積むだけです．conhost は出力のパイプへの書き込みで止まるとコンソールのロックを持ったままになることがあり，入力のパイプが読まれなくなりうるので，ウィンドウのスレッドで書くと出力を受け取れないまま止まるおそれがあるためです（実際に止まるところは確認していません）．破棄すると，書くためのスレッドは書いている途中のものを書き終えたら残りを捨てて抜けます（ブリッジと同じ）．出力は読むためのスレッドで `ReadFile` を待ち，終了は子プロセスのハンドルを待つスレッドで知ります（ConPTY は子プロセスが終わっても出力のパイプに EOF を返さないため）．子プロセスは止めた状態で作ってタブの Job に入れてから動かします．Job は子孫を見分けるためだけに使い，Job で終わらせることはしません（別のコンソールを持つものやウィンドウのプログラムまで終わるため）．`STARTF_USESTDHANDLES` の理由は `spawn_process` のコメントにあります．

ConPTY は起動してすぐ mode 9001（win32-input-mode）を求めますが，libghostty-vt はこれを実装しておらず，普通の VT の入力で動きます．

### ブリッジ（`bridge.rs` と `crates/fantermo-bridge`）

fantermo は `std::process::Command` でブリッジを起動し（`CREATE_NO_WINDOW`，標準入出力と標準エラー出力はパイプ），開始の通知とリサイズを続けて送ります．書き込みは書くためのスレッドに任せるので，ブリッジが標準入力を読むのを止めていてもウィンドウのスレッドは止まりません．データは 1 MiB ずつに分けます．標準エラー出力はログへ流します．

- **Cygwin**: `<cygwin_root>\bin` を PATH の先頭に置き，`HOME` を消し，作業フォルダを `--cwd` と `CHERE_INVOKING=1` で渡します．利用者のコマンドラインは引数に入れず，環境変数 `FANTERMO_BRIDGE_COMMAND` に入れて `/bin/sh -c` で実行させます（理由は `COMMAND_VARIABLE` のコメント）．`cygwin_root` を書かなければ `config::find_cygwin_root` で探します．
- **WSL**: `<System32>\wsl.exe [-d <distribution>] --cd <作業フォルダ> --exec /mnt/<ドライブ>/…/fantermo-bridge [-- /bin/sh -c <command>]`．ブリッジの Linux のパスは自前で `/mnt/<ドライブ>/…` に直します（`wslpath` を呼ぶと WSL をもう一度起動して待つため）．`wsl.exe` は名前だけで探さず，`known_folder::system` が返すシステムのフォルダ（`System32`）の絶対パスで起動します．環境変数は `WSLENV` に名前を足して届けます．
- **共通**: 一つのコマンドなら `exec` を前に付けます（`shell_command`）．シェルを親として残さず，Ctrl+C のあとの終了コードがアプリのものになるようにするためです．`command` が空なら相手のログインシェルを起動します．
- **ブリッジの探し方**: fantermo の exe と同じフォルダ，なければ，開発中のビルド（`debug_assertions` が有効なビルド）では `<exe のフォルダ>\..\bridge\<ターゲット>\debug\`（`target/bridge/…`）．リリースのビルドは exe と同じフォルダだけを探します．
- **起動できないとき**: ブリッジや Cygwin が見つからない，起動できない，エラーで終わったときは，理由を `fantermo: …` の赤い文字で端末に出し，タブは開いたままにします．

ブリッジ（`crates/fantermo-bridge/src/main.rs`）は開始の通知を交換し，最初のリサイズの大きさで `forkpty` し，標準入力と pty の master を `poll` で見て中継します．pty への書き込みは別のスレッドで行い，書き終えていない入力が 64 MiB を超えたら標準入力を読むのを止めます．Cygwin の pty は大きな入力で詰まるので，Cygwin 向けだけ `InputPacer`（`cfg(target_os = "cygwin")`）で 4 KiB ずつ，pipe が空になってから書きます（理由は `InputPacer` のコメント）．終了コードは 0 正常，2 引数の誤り，3 プロトコルのエラー，4 子を起動できない，5 入出力のエラーです．

### プロトコル（`fantermo-bridge-protocol`）

一つの単位は `[種類 1 バイト][長さ 4 バイト（リトルエンディアン）][ペイロード]` で，ペイロードの上限は 1 MiB（`MAX_PAYLOAD_LEN`）です．種類の表は `lib.rs` の先頭のコメントにあります．流れは，互いに最初の単位として開始の通知を送り，fantermo がその次にリサイズを送り，ブリッジはそれを受け取ってから子プロセスを起動し，子プロセスが終わると子プロセスの終了の単位を送って終わる，というものです．

バージョンは整数一つ（`VERSION`）です．単位の形や意味を変えたら上げます．開始の通知のバージョンが合わなければ，ブリッジはエラーの単位を返して終わり（終了コード 3），fantermo はその文を端末に出します．バージョンの違う相手とは続けず，互換を保つ仕組みは持ちません（fantermo とブリッジは同じ配布物に入ります）．

### ブリッジのビルド

```sh
./tools/bridge.sh build <cygwin|linux|all> [--release]
./tools/bridge.sh clippy <cygwin|linux|all>
./tools/bridge.sh test <cygwin|linux|all>
./tools/bridge.sh fmt [--check]
```

- `x86_64-pc-cygwin` は tier 3 なので，nightly の `-Zbuild-std` で std からビルドします．nightly の日付は `tools/bridge.sh` の `BRIDGE_NIGHTLY` の既定値で，CI の道具もそこから読みます．リンカは Cygwin の `x86_64-pc-cygwin-gcc` で，`CYGWIN_ROOT` があればその `bin` のものを使います．`cygwin1.dll` と `cyggcc_s-seh-1.dll` は同梱せず，利用者の Cygwin のものを使います．
- `x86_64-unknown-linux-musl` は既定のツールチェインと `rust-lld` で，静的にリンクした ELF にします．Linux 以外で `test` を実行すると，テストは WSL の中で実行します．
- 出力は `target/bridge/<ターゲット>/<debug|release>/`．`panic = "abort"` です（test だけ `unwind`）．`rust-toolchain.toml` は置きません（musl のビルドまで nightly になるため）．

## 5. 技術の判断とその理由

### 言語とリポジトリ

- Rust の workspace（edition 2024）．crate は，分ける理由が出てから分けます．
- clippy と rustfmt を通し，workspace の lint に `undocumented_unsafe_blocks` を置きます．`unsafe` のブロックには `// SAFETY:` を書きます．
- 依存の版は `[workspace.dependencies]` にまとめ，windows-rs の機能は使う crate の `Cargo.toml` に足します．
- メインスレッドのスタックは 8 MB です（ウィンドウの大きさを変えている間，Windows の処理の上にウィンドウプロシージャが何段も積まれ，その中で libghostty-vt を呼ぶため）．同じ理由で，libghostty-vt は Cargo のプロファイルによらず Zig の ReleaseSafe でビルドします（Debug では関数ごとのスタックが大きくなる）．

### libghostty-vt の取り込み

- [libghostty-rs](https://github.com/uzaaft/libghostty-rs) のバインディングを git の依存としてコミットで固定します．crates.io の版は固定している Zig のバージョンで通らず，submodule や vendor は上げる手間が重いためです．
- 上げるときは，`Cargo.toml` の `rev` を書き換え，`tools/dev.bat` の `ZIG_REQUIRED` を揃え，ビルドとテストを通します．libghostty-vt に静的にリンクされるものが変わっていないかを `tools/licenses.sh` の先頭のコメントの手順で見直し，`THIRD-PARTY-NOTICES.txt` を作り直します．
- 直接使うのは `fantermo-core` の `vt` だけです．振る舞いを当てにするところは，`crates/fantermo-core/tests/key_encoding.rs` と `vt/tests/` のテストで固定し，上げたときに壊れたところが分かるようにします．
- East Asian Ambiguous は 1 セルで固定します．libghostty-vt に幅を指定する API がなく，多くの TUI も 1 で数えるためです．全角の幅でデザインされたグリフは，既定では 1 セルに収まるように縮めて描きます（`font.wide_glyph` で変えられます）．

### ウィンドウまわり

windows-rs で Win32 を直接書きます．winsafe は標準のコントロールをほとんど使わないので利点が小さく，winit は IME とキー入力の細かいところに手が届きにくく，WinUI 3 は Rust から使うのが難しく重いためです．借用の扱いは 2 節の「`window` の中」のとおりです．

### 描画

Direct3D 11 にグリフアトラスを載せて描きます．Direct2D + DirectWrite で文字の並びごとに毎フレーム描画を頼むと CPU を多く使い，OpenGL は DirectWrite とのつなぎを自分で書くことになり，GDI は CPU で描いてカラー絵文字や合字が苦手なためです．

- グリフは DirectWrite で組み（ロケールは `ja-jp`），Direct2D でアトラス（BGRA，乗算済みアルファ）に白で描き，色はシェーダで付けます．文字の縁はグレースケールです．
- 斜体のようにインクがセルの左右へはみ出すグリフは，縮めずにはみ出す分の余白を持たせた場所に描き，隣のセルへ重ねて描きます．縮めるかどうかは送り幅で，余白はインクの範囲（`GetOverhangMetrics`）で決めます．
- 罫線などはフォントに頼らず形として組み立てます（`boxdraw.rs` の先頭のコメント）．装飾の位置と太さは DirectWrite のフォントの計測と，組んだ行のベースラインから決めます．
- シェーダは実行時に `D3DCompile` でコンパイルし，`cargo test` でコンパイルが通ることを確認します．
- 反転表示と選択範囲は `Vt::frame` の中で前景色と背景色を入れ替えておきます．薄い文字（SGR 2）は前景色を背景色に半分寄せ，不可視の文字（SGR 8）は背景だけを残します．
- スクロールバーも Direct3D で自分で描きます．Windows 11 の標準の描き方は太い場所に細いつまみで不自然なためです．幅は常に取っておき，スクロールバックの有無で桁数が変わらないようにします．
- ドライバの更新や再起動，GPU のリセットで Direct3D のデバイスを失っても描き続けます．描画，表示（`Present`），スワップチェーンの大きさの変更，グリフアトラスの作り直しが `DXGI_ERROR_DEVICE_REMOVED`，`DXGI_ERROR_DEVICE_RESET`，`DXGI_ERROR_DRIVER_INTERNAL_ERROR`，`D2DERR_RECREATE_TARGET` のどれかを返したら，`GetDeviceRemovedReason` をログに残し，`App` が `Renderer` を作り直します．
- 描くことのないウィンドウでも，デバイスを失ったらすぐに作り直します．`Renderer` は作るたびに `ID3D11Device4::RegisterDeviceRemovedEvent` でイベントを登録し，スレッドプールでそのイベントを待ちます．シグナルされたら，ウィンドウに `WM_APP_DEVICE_REMOVED` を積みます．ウィンドウは，今の `Renderer` のデバイスを失っているときだけ，描くときに気づいたときと同じ経路で作り直します．描くときにすでに気づいて作り直していれば，何もしません．`Renderer` を捨てるときは，待ちを解除してコールバックの終わりを待ち，登録を解除してからデバイスを手放します．`ID3D11Device4` を使えない環境では，描くときにだけ気づきます．
- 古いデバイスに属するもの（スワップチェーン，描画先，グリフアトラス，パイプライン，画像のテクスチャ）は `Renderer` ごと捨てます．一つのウィンドウに flip model のスワップチェーンは一つしか持てないので，古い `Renderer` を手放し，`ClearState` と `Flush` で解放を済ませてから新しいものを作ります．背景画像は各端末が選んだファイルから読み直します．読み直せなかった端末と，`Renderer` がない間に開いた端末には選び直します．端末の中の画像とタブのアイコンのテクスチャは描くときに作り直します．タブのアイコンは `App` が持つピクセル（`tab_icon::Pixels`）から作り，ファイルは読み直しません．
- 最初の作り直しはその場で行います．作れないか，作り直しても描けなければ，`RENDERER_RETRY_DELAY` のあとにもう一度試し，作り直しても描けないことが `MAX_RENDERER_RECREATIONS` 回続いたら，警告を残して描画をやめます．作り直しでハードウェアのデバイスを作れず WARP になったら，fantermo を起動し直すまでハードウェアに戻りません．

### キー入力の実装上の注意

- キーの種類はスキャンコードから決めます（libghostty-vt のキーの名前は物理的な位置）．修飾キーなしの文字は `MapVirtualKeyW` でキーボード配列から取ります．
- `WM_KEYDOWN` の中で，そのキーから `TranslateMessage` が積んだ `WM_CHAR` を取り出して同じキーの情報に入れます．
- Ctrl の組み合わせの制御文字を付けない理由，サロゲートの組み立て方は `keyboard.rs` のコメントにあります．
- Alt の組み合わせは `WM_SYSKEYDOWN` です．Alt を含む F4 と Space，`VK_PROCESSKEY` は Windows に任せます．
- kitty keyboard protocol で，アプリが「すべてのキーをエスケープコードで報告する」を求めていないときは，ロックの状態（Caps Lock，Num Lock）を修飾キーの数に入れません（`Vt::mods_to_report` のコメント）．

### ConPTY の実装上の注意

- `STARTF_USESTDHANDLES` を立てて標準ハンドルを空にします（`spawn_process` のコメント）．
- 出力は読むためのスレッドで，終了は子プロセスのハンドルを待つスレッドで知ります（4 節）．
- `TERM` などは子プロセスに渡す環境変数のブロックを作って渡し，自分のプロセスの環境は書き換えません（`environment_block`）．
- 疑似コンソールを閉じると，閉じる知らせは閉じた時点で接続しているプロセスにしか届きません．起動の途中だったプロセスは動き続け，conhost もそれを待って残ります．`ClosePseudoConsole` はすぐ戻るので，順番や猶予では避けられません．そのため，猶予のあとで，そのコンソールに接続しているプロセスと，接続し終えないまま止まっているコンソールのプログラムだけを終わらせます．接続のようすは `NtQueryInformationProcess` の `ProcessConsoleHostProcess` で読みます．Windows SDK のヘッダーにない値なので，読めなければ何もしません（`conpty.rs` の定数のコメント）．

### Cygwin と WSL の既定をブリッジにした理由

kitty keyboard protocol は ConPTY でもブリッジでも届きます．ブリッジの値打ちは次の点にあり，どれも ConPTY を通す限り fantermo の側では直せません．

- アプリの出力がそのまま届きます．ConPTY は OSC 52・OSC 8・OSC 133・OSC 7・`CSI ?2026` の順番や中身を変え，kitty graphics protocol と sixel の画像を落とします．
- 問い合わせに fantermo が答えます．ConPTY では conhost が自分のことを答えるか，落とします（OSC 10・11・4 の色，XTGETTCAP，kitty の画像の問い合わせ，DECRQM 2026 など）．
- リサイズで conhost が画面を描き直さないので，スクロールバックの行が欠けません．
- Cygwin では大きな出力が 30 倍ほど速く，tty が Cygwin のコンソールではなく本物の pty になります．
- OSC 52 の読み取りの依頼は ConPTY では fantermo に届きません．

ConPTY の利点は，補助のプログラムが要らないことです．そのため，`profiles` がないときに `shell` から作るプロファイルは，Cygwin の `bin` の下の実行ファイルなら `backend = "cygwin"`，`wsl.exe` なら `backend = "wsl"`，ほかは ConPTY にします（`Profile::from_shell`）．ファイルに書いた `[[profiles]]` は変えません．

### Cygwin の pty の大きな入力

Cygwin の pty は，master から子プロセスへの入力を一度に大きく書くと，中身が崩れたり，書き込みが返らなくなったりします．間を空けるだけでは足りず，pipe が空になるのを待つ必要があります．pipe の中身を知る手がかりは slave の `FIONREAD` だけです．速さの上限（50 KB/s ほど）は Cygwin の pty の処理の重さで決まり，外からは速くできません．書き方は `InputPacer` のコメントにあります．fantermo の側で分けて送る必要はなく，タブを閉じれば書き終えていない入力を捨てて終わります．

### 起動直後に閉じると残るプロセス

- ConPTY: 上の「ConPTY の実装上の注意」のとおりです．ウィンドウを閉じたときは，プロセスを終える前に片づけを待ちます（`wait_for_closing_consoles`）．
- Cygwin: 起動の途中でシェルが SIGHUP で終わると，シェルからの SIGHUP が子に届かず，前面のプロセスグループが残ることがあります．ブリッジは，シェルが終わったあとも前面のプロセスグループがなくなるまで SIGHUP を送り直します（全体で 2 秒まで）．前面だけに送るので，背景のジョブ（`nohup`，`disown`）には送りません．
- ブリッジのログは，fantermo が先に終わって標準エラー出力が閉じていても書くのを続けます．書けずに panic すると（`panic = "abort"`），SIGHUP を送らずに終わるためです．
- 別デスクトップで動いていても，新しいコンソールを作るプロセスは既定の端末に渡って利用者の画面に出ることがあります．起動直後に閉じる e2e は画面に出ることがあるものとして扱います（`.claude/skills/e2e-verify`）．

### run-hidden のデスクトップヒープ

run-hidden は利用者のセッションのウィンドウステーション（`WinSta0`）にデスクトップを作るので，そのデスクトップヒープを使います．`CreateDesktopExW` でヒープを 1024 KB（`DESKTOP_HEAP_KB`．既定は 20480 KB）にし，名前とヒープの大きさを標準エラー出力に出します．e2e の同時に起動する数の上限（`tests/common/mod.rs` の `MAX_RUNNING`）は 4 で，合わせて既定のデスクトップ一つ分より小さくなります．ヒープが足りないと，設定ダイアログなどが `0x80070008`（`ERROR_NOT_ENOUGH_MEMORY`）で開けなくなります．e2e の土台は，失敗したテストのログにこれがあればヒープの見当を添えます．

### 参考

- libghostty-vt のヘッダ: https://github.com/ghostty-org/ghostty/blob/main/include/ghostty/vt.h
- JetBrains の IDE の端末は libghostty を端末エミュレーションに使っています: https://blog.jetbrains.com/platform/2026/10/jetbrains-terminal-now-uses-ghostty/
- Windows 向けのフォーク: winghostty（Win32，OpenGL），WolftacDigital/ghostty-windows，InsipidPoint/ghostty-windows
- JVM から使うプラグインの例: https://github.com/NiHaiden/libghostty-jetbrains
- 曖昧幅の議論: ghostty Discussion #7204，cmux #9788，nanasess/home-manager #71
- ブリッジの前例: Cygterm，wslbridge / wslbridge2

## 6. よくある変更の手順

### 設定の項目を一つ足す

`[window] padding` が一通りそろった例です（`grep -rn padding crates/` でたどれます）．

1. `fantermo-core/src/config.rs`: 型にフィールドを足し，`Default` に既定値を入れます．既定値と範囲は `DEFAULT_*`・`*_RANGE` の定数にします．数か名前一つだけの項目なら，項目の一覧 `SCALARS` に 1 行（テーブル，キー，値の種類 `ScalarKind` と範囲，フィールドを読み書きする `get`・`set`）を足します．読む処理（`parse`）と書き戻す処理（`config_edit::update`）はこの一覧を上から順に処理し，設定ダイアログの入力欄から読む処理（`parse_input`）は項目を名前で引きます．`parse` は，型が違う・範囲外・知らない名前なら警告を残してその項目だけ前の値にします．名前で書く値は，その型に値と名前の表 `NAMES` を置き，`ALL`・`name`・`from_name` をそこから作ります．`TEMPLATE` に説明と `# キー = 既定値` の行を足します．
2. 一覧に載らない項目（`font.family`，`default_profile`，プロファイル，背景画像の一覧，色，キーの割り当て）は，読む処理と書き戻す処理をそれぞれ書きます．読む処理は，一覧の項目のあるテーブルなら `config/read.rs` の `read_table` に，ほかは `parse` に書きます（`parse` が `read_table` で回すのは，一覧の項目のあるテーブルだけです）．書き戻す処理は `fantermo-core/src/config_edit.rs` の `update` から呼びます（`default_profile`・`font.family`・色・キーの割り当ては `config_edit.rs` に，背景画像の一覧は `config_edit/images.rs` に，プロファイルは `config_edit/profiles.rs` にあります）．
3. `fantermo/src/window/mod.rs` と `window/settings.rs`: `App` はいま使っている設定を `Config` 一つ（`App::config`）で持つので，項目を足しても `App` の構造体と `App::new` は変わりません．使う側が `self.config.<項目>` を読み，反映に配置やアトラスの作り直しが要るかを `apply_config`（`settings.rs`）で決めます．
4. `fantermo/src/settings_dialog/`: 項目を置くページのファイル（`appearance.rs` など．`Page` を実装した `AppearancePage` などの型）に，コントロールの ID（`ID_*`），フィールド，`create` で作る，`fill` で値を入れる，`read` で `Config` に戻す（不正なら入力欄を返してフォーカスする），`on_command` で `WM_COMMAND` の知らせを受ける，を足します．ID はそのページの `ID_RANGE` の中の番号です．範囲がページの間で重ならないことと ID が範囲に入ることは単体のテストが見るので，ID を足したらそのページの `the_ids_are_within_the_range_of_the_page` にも足します．ページからダイアログ全体（端末への反映，最後に書いた設定，DPI など）へは `DialogContext` を通します．コントロールは種類ごとの部品（`settings_dialog/widgets.rs`）で作ります．
   - 数なら `NumberField`．読み方（`config::parse_input` に項目の名前を渡す）・見せ方・矢印での動かし方・読めないときの文を `NumberKind` の定数に書き，ページの `number_fields()` に足します．矢印での増減と矢印の配置は，すべてのページのこの一覧を回します．`number_fields()` に足せば，`on_command` の数の入力欄をまとめて受ける分岐が `EN_CHANGE` を受け，打ち終わるのを待って反映します．
   - ラジオボタンの組なら `RadioGroup`．選択肢と画面の名前の配列を渡します．`BN_CLICKED` で反映する処理は，`RadioGroup::owns` で ID を見て `on_command` に書きます．
   - 固定の選択肢のコンボボックスなら `ChoiceBox`．選択肢と画面の名前を渡します．`CBN_SELCHANGE` の処理は `on_command` に書きます．
   - 変えたときに反映のほかにも処理が要るものは，`on_command` の，数の入力欄やラジオボタンをまとめて受ける分岐より上に，その ID の分岐を書きます．
5. 書き方を改めるとき（キーの名前を変える，型を変える）は，`config::migrate` の `STEPS` の最後に手順を足します．今の書き方には何もしない手順にします．
6. テスト: `config/read.rs` の `template_shows_every_setting_with_its_default`・`all_items_are_read` と範囲のテスト（`SCALARS` の項目は，ひな形の項目，入力欄の範囲，既定値に戻す書き戻しのテストが一覧から数えます．`invalid_items_keep_the_previous_values_and_missing_ones_get_the_defaults` は設定の文字列に読めない値を 1 行足すまで落ちます．`all_items_are_read` と，`config/read.rs`・`config_edit.rs` それぞれの `customized()` は手で書いた値なので，足さなくても落ちませんが，既定値と違う値を足します．`customized()` に足すと，`settings_replace_the_commented_lines_of_the_template` の期待値にもその行の置き換えが要ります），`config_edit.rs` の書き戻しのテスト，設定ダイアログの e2e（`crates/fantermo/tests/e2e/settings.rs`）．利用者に見える項目なら README も．

### キーの割り当てを足す

1. `fantermo-core/src/keymap.rs`: `Action` に足し，`ACTION_NAMES`（設定ファイルに書く snake_case の名前）と `Action::ALL` に足します（どちらも配列の長さを書いています）．既定で割り当てるなら `DEFAULT_BINDINGS`（これも配列の長さを書いています）に足します．
2. `fantermo/src/window/keys.rs`: `run_bound_action` に動作を書きます．成り立たないときにシェルへ送るか，押し続けたときに繰り返すかもここで決めます．
3. `fantermo/src/settings_dialog/keys.rs`: `ACTION_LABELS` に画面の名前を足します．`Action::ALL` と同じ順に並べます（長さは `Action::ALL.len()` なので，足りないとビルドが通りません）．
4. メニューにも同じ項目があれば，`context_menu.rs` の `COMMANDS` の，その項目の 3 つ目の値に `Some(Action::…)` を書きます（メニューに添えるキーが割り当てから決まります）．
5. `TEMPLATE` の `[keys]` の説明（`template_lists_the_default_keys` が既定の割り当てとの一致を見ます）．

### メニューの項目を足す

1. `fantermo/src/context_menu.rs`: `MenuCommand` に足し，宣言と同じ順に `COMMANDS` へ 1 行（項目，アクセラレータ付きの名前，同じことをするキーの割り当ての動作）足します．項目の ID は `COMMANDS` の位置 + 1 で決まり，`PROFILE_ID_BASE`（100）未満に収めます（`ids_round_trip_and_avoid_zero` が見ます）．`Menu::items` で，どのメニューのどこに並べるかを決めます．押せないときがあれば `MenuRequest` に条件を足し，`enabled` で判定します．
2. `fantermo/src/window/menu.rs`: `run_menu_command` の `match` に動作を書きます．ダイアログを開くものは `Deferred` を返し，借用の外で開きます．
3. `context_menu.rs` の単体のテストを直します．`menu_items_keep_their_order_and_separators` は並びを書いているので足し，`every_command_is_in_the_main_menu` は項目をタブバーのメニューに入れることを求めます．押せない項目は `only_copy_and_the_settings_items_can_be_disabled` に足します．
4. e2e のメニューのテスト（`crates/fantermo/tests/e2e/menu.rs`）は並びを `menu_shape` の形で書いているので，並びが変わるメニューの分を直します．

### 制御の口の要求を足す

1. `fantermo/src/e2e_server.rs`: `Request` に足し，`parse_request` でパスと本文を読み，先頭のコメントの表に足します．
2. `fantermo/src/window/e2e.rs`: `run` の `match` に，借用の外で処理するもの（ダイアログ，メニュー，メッセージの送信など）を書きます．それ以外の要求は `App::run_e2e_request` が借用の中で処理します．状態の JSON に項目を足すときは `App::e2e_state` です．
3. `fantermo/tests/common/mod.rs`: テストから呼ぶ関数を足します．複数の話題のテストが使う補助は `fantermo/tests/e2e/ui.rs` です．
4. `.claude/skills/e2e-verify/SKILL.md` の「要求の一覧」に足します．

### ブリッジのプロトコルを変える

1. `crates/fantermo-bridge-protocol/src/lib.rs`: 単位の型，符号化と読み取り，先頭のコメントの表を変え，`VERSION` を上げます．
2. 両側を直します．fantermo は `crates/fantermo/src/bridge.rs`（`OutputReader` など），ブリッジは `crates/fantermo-bridge/src/main.rs`（`Relay`）．
3. テスト: プロトコルの crate の単体のテスト（`./tools/dev.sh test` で実行します），`crates/fantermo-bridge/tests/protocol.rs`（`./tools/bridge.sh test all`），Cygwin と WSL の e2e．
4. 開発中の fantermo は `target/bridge/` のブリッジを使うので，`./tools/bridge.sh build all` で作り直してから確認します．

### バージョンを上げる

`.claude/skills/release` の手順に従います．バージョンは workspace の `[workspace.package]` の `version` で，`Cargo.lock` も一緒にコミットします．`fantermo-bridge-protocol` と `fantermo-bridge` は自分の `version` を直に書いていて，workspace のバージョンを継ぎません（ブリッジの `Cargo.lock` を上げるたびに変えないため）．

## 7. テスト

- **単体のテスト**: `./tools/dev.sh test`．各モジュールの `#[cfg(test)]` のほか，`crates/fantermo-core/tests/key_encoding.rs`（libghostty-vt のキーの符号化を固定する）と `crates/run-hidden/tests/usage.rs`．ウィンドウや描画に依存しない計算（`tab_bar`，`scroll_bar`，`window/layout.rs` の配置の関数など）は関数に分けて単体でテストします．
- **e2e**: `./tools/dev.sh test --features e2e`．実際の fantermo を run-hidden で別デスクトップに起動し，制御の口で操作して画面の文字と絵を読みます．テストは `crates/fantermo/tests/e2e/`（話題ごとのファイルに分け，複数の話題が使う補助は `ui.rs`），土台は `crates/fantermo/tests/common/`．実行のしかた，書き方，環境変数，落とし穴は `.claude/skills/e2e-verify` にあります．
- **画面の書き出し**: `FANTERMO_DUMP=<ディレクトリ>` を付けると，fantermo が `screen.txt`（画面の文字と大きさ）と `frame.png`（描いたバックバッファ）を定期的に書き出します（`dump.rs`）．Direct3D のウィンドウはウィンドウの DC から撮れないので，画面を使わずに描画を見るときに使います．
- **ブリッジのテスト**: `./tools/bridge.sh test <cygwin|linux|all>`．`crates/fantermo-bridge/tests/protocol.rs` が，ブリッジをパイプで起動してプロトコルの単位で話します．
- **CI**（`.github/workflows/ci.yml`）:

| ジョブ | ランナー | すること |
|---|---|---|
| `check` | Windows | fmt，clippy（`-D warnings`），test |
| `bridge` | Windows | ブリッジの fmt，clippy，build（Cygwin と Linux） |
| `e2e-conpty` | Windows | feature `e2e` の build と clippy，`smoke`，e2e の全体（OS のクリップボードとファイルを選ぶダイアログを使うテストも） |
| `e2e-cygwin` | Windows | Cygwin のブリッジの単体のテストと，名前に `cygwin` を含む e2e |
| `bridge-linux` | Ubuntu | Linux 向けのブリッジの単体のテスト |

WSL の e2e は CI では実行しません．手元で `FANTERMO_E2E_WSL` を付けて実行します．
