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

OS のファイルを選ぶダイアログで実際に選ぶテスト（`the_file_dialogs_add_a_background_image_and_a_profile_icon`）は `FANTERMO_E2E_FILE_DIALOG=1` があるときだけ実行する。CI では付ける。手元では付けない（OS がダイアログの履歴 `LastVisitedPidlMRU` に一時フォルダを残す）。

## 要求の一覧

| 要求 | すること |
|---|---|
| `GET /e2e/state` | JSON: `pid` `hwnd`（同じデスクトップからしか使えない）`dpi`（ウィンドウが使っている DPI）`monitor_dpi`（`GetDpiForWindow`。OS が決める）`font_size_px` `ui_font_size_px` `cols` `rows` `client_width` `client_height` `terminal_x` `terminal_y` `cell_width` `cell_height` `active` `tabs`（`title` `profile` `cols` `rows` `bell`）`profiles`（名前）`dialogs`（`class` `title` `controls`）`images`（画面に出ている画像の数）`window`（`state.toml` と同じ `left` `top` `width` `height` `maximized`）`background`（背景画像を敷く範囲の `x` `y` `width` `height` と、表示しているタブが選んだ画像の名前 `image`）`selection`（選択範囲の文字列。なければ `null`）`mouse_tracking`（アプリがマウスを求めているか）`scroll`（`total` `offset` `len` と、ホイール 1 ノッチの行数 `lines_per_notch`）`scroll_bar`（`x` `y` `width` `height`、つまみ `thumb` の `top` `length`（なければ `null`）、押している間だけ真の `pressed`）`tab_bar`（`menu` `new_tab` `profiles` の範囲と、描いているタブ `tabs` の `index` `area` `close`）`window_enabled`（ウィンドウが入力を受けるか。ダイアログの間は偽）`window_title` `version` `text`（スクロールバックを含む全体の文字） |
| `GET /e2e/frame` | いまの画面の PNG |
| `POST /e2e/input` | 本文のバイト列をそのままバックエンドへ |
| `POST /e2e/text` | 本文の文字を `WM_CHAR` と同じ経路で（制御文字は送らない。Enter は `key`） |
| `POST /e2e/key/<組み合わせ>` | `enter`、`ctrl+shift+t` など。キーの割り当てと符号化を通す |
| `POST /e2e/tab/new`、`/e2e/tab/new/<プロファイルの位置>`、`/e2e/tab/close`、`/e2e/tab/select/<位置>` | タブの操作 |
| `POST /e2e/window/size/<桁数>/<行数>` | ウィンドウの大きさを変えて、そのあとの状態を返す |
| `POST /e2e/window/lower` | ウィンドウを同じデスクトップの Z オーダーのいちばん下へ移す（別デスクトップのスレッドには前面に出す権利がなく、いちばん上へは移せない） |
| `POST /e2e/window/block/<ミリ秒>` | 答えを返してから、ウィンドウのスレッドをその時間だけ止める（応答しないウィンドウの代わり） |
| `POST /e2e/paste` | 本文の文字を、クリップボードから読んだものとして貼り付ける（`paste.confirm` と `paste.confirm_size` に従う） |
| `POST /e2e/dialog/button/<ID>` | 開いているダイアログのボタンを押す（`TaskDialogIndirect` は `TDM_CLICK_BUTTON`、ほかは `WM_COMMAND`。メッセージボックスは押せない） |
| `GET /e2e/dialog/controls` | いちばん上のダイアログ（無効なものとメニューを除く）の JSON: `class` `title` `dpi` `left` `top` `width` `height`（枠を含む、画面のピクセル）`client_width` `client_height` `work_area`（`left` `top` `right` `bottom`）`focus`（コントロールの ID）`controls`（子孫のすべて。`id` `class` `text` `visible` `enabled` `x` `y` `width` `height`（クライアント座標）、ボタンは `checked`、コンボボックスと一覧は `selected`（なければ `null`）と `items`）。開いていなければ 400 |
| `POST /e2e/dialog/text/<ID>` | 入力欄の文字を本文にする（`WM_SETTEXT`。`EN_CHANGE` が届く） |
| `POST /e2e/dialog/select/<ID>/<位置>` | コンボボックスか一覧のその項目を選ぶ（`CBN_SELCHANGE`、`LBN_SELCHANGE`）。設定ダイアログのページの切り替えはページの一覧（ID 100）でこれ |
| `POST /e2e/dialog/radio/<ID>` | ラジオボタンを選ぶ（同じ並びのほかを外して `BN_CLICKED`） |
| `POST /e2e/dialog/click/<ID>` | ボタンを押す（`WM_COMMAND` の `BN_CLICKED`）。答えを先に返す。メッセージボックス、色を選ぶダイアログ、ファイルを選ぶダイアログのボタンも押せる |
| `POST /e2e/dialog/spin/<ID>/<回数>` | 上下の矢印を押す（`UDN_DELTAPOS`。ID は矢印のもの。正が上） |
| `POST /e2e/dialog/key/<組み合わせ>` | ダイアログのループへキーを積む（フォーカスのあるコントロール宛ての `WM_KEYDOWN`）。キーを読み取るダイアログ、Esc、Enter |
| `POST /e2e/dialog/close` | ダイアログの × を押す（`WM_CLOSE`を積む）。OK だけのメッセージボックスも閉じる |
| `GET /e2e/clipboard`、`POST /e2e/clipboard` | クリップボードの文字列を読む（JSON の `text`）・置き換える |
| `POST /e2e/message/<名前>`、`/e2e/message/<名前>/<wParam>` | ウィンドウにメッセージを `SendMessageW` で送り、戻り値を JSON の `result` で返す。名前は `queryendsession`、`endsession`、`capturechanged`（`WM_` を除いた小文字。`lParam` はメッセージごとに決まっている）。`dpichanged/<DPI>` は `<wParam>` の位置に DPI を書く（1〜576。推奨の矩形はウィンドウの側で、Windows と同じく左上を動かさずにクライアント領域を DPI の比で拡大・縮小して作る。枠はいまの大きさのまま）。`getdpiscaledsize/<DPI>` は `WM_GETDPISCALEDSIZE` を送り、答えに `width` `height`（書かれなければ 0）が入る |
| `POST /e2e/mouse` | 本文の各行のマウスの操作を、続けて `SendMessageW` で送る。行の形は `<操作> <x> <y> [押しているもの…]`。操作は `move`、`left-down`、`left-up`、`middle-down`、`middle-up`、`right-down`、`right-up`、`wheel:<回転量>`（1 ノッチが 120、奥が正）。座標はクライアント座標（ウィンドウの外を指す負の値も書ける）。押しているものは `left` `middle` `right` `shift` `ctrl` `alt` |
| `GET /e2e/menu` | JSON: 開いているメニューの `items`（`text`（`&` とキーの組み合わせの前のタブを含む）`separator` `enabled` `checked` `highlighted` `submenu` `icon`）。開いていなければ 400 |
| `POST /e2e/menu/highlight/<位置>` | メニューのループへ上下の矢印のキーを送って、その項目へ移る。区切りと並びの外は 400 |
| `POST /e2e/menu/select/<位置>` | 矢印のキーでその項目へ移り、Enter を送って選ぶ。区切りと並びの外は 400 |
| `POST /e2e/menu/key/<キー>` | メニューのループへキーを送る。`escape` か、英数字一文字（アクセラレータ） |
| `POST /e2e/menu/close` | 開いているメニューを閉じる（`EndMenu`）。開いていなければ 400 |
| `POST /e2e/quit` | ウィンドウを閉じる |

## テストの書き方

`Server::start(設定の TOML, fantermo の引数)` か `Server::start_cmd()`・`start_powershell()` で起動し、`type_line`・`press`・`resize`・`post` で操作して、`wait_text`・`wait_until` で待つ。`sleep` で待たない。何かが起きないことを時間の窓で見るときは `assert_holds_for`（`wait_until` と同じ間隔で読む）。`Server` が一時フォルダ（`FANTERMO_CONFIG`・`FANTERMO_STATE`・ログ・ポート）、同時に起動する数の上限、Drop での停止と片づけを受け持つので、テストは並列で衝突しない。失敗すると fantermo のログを出す。打った行に出ない文字列を待つ（`echo marker-%ERRORLEVEL%-end` で `marker-0-end`）。

- 起動中の起動: `server.launch_again(引数)`・`launch_again_with(引数, &LaunchAgain { .. })` が、同じ別デスクトップで fantermo をもう一度起動する（`run-hidden --desktop`）。既存のウィンドウへ渡して終わるのは `wait_exit` が 0 で `opened_a_window` が偽（`assert_handed_over`）、ウィンドウを開くなら `wait_for_window` のあとで `Server` として操作する。ウィンドウが表示される前に起動すると見つからないので、先に `wait_text` で待つ。作業フォルダの既定は元の `Server` の一時フォルダ。足されたタブがそこにとどまるので、起動したものの一時フォルダにしない（消せなくなる）。fantermo のウィンドウへメッセージを送るのは `server.on_its_desktop` の中で（ウィンドウのハンドルは別のデスクトップからは無効で、`ERROR_INVALID_WINDOW_HANDLE` になる）。`state` の `hwnd` だけを使い、ウィンドウを探さない。
- 長い行: `type_line` は一文字ずつ送り、Cygwin の pty では一文字に 30 ms ほどかかる。長い行は `send_line`（`/e2e/input` でまとめて送る）で。
- 起動直後に閉じる確認もできる（`closing_a_tab_ends_its_child_processes_only` はプロセスができたらすぐ閉じる）。ConPTY では閉じたあと 5 秒の猶予ののちに残りを終わらせるので、残らないことは 60 秒の `wait_ended` で待つ。起動直後に閉じるテストと、新しいコンソールを作るテストは、利用者の画面に窓が出うるので、手元では利用者に声を掛けてから流す。
- 新しいコンソールを作るプロセス（cmd の `start ""`、`CREATE_NEW_CONSOLE`）をテストから起動しない。別デスクトップでも既定の端末（Windows Terminal）に渡って利用者の画面に出る。別のコンソールを持つプロセスが要るときは、PowerShell の `ProcessStartInfo` の `CreateNoWindow` で起動する。WMI など、別のプロセスに起動を頼む方法も別デスクトップの外に出るので使わない。
- 画像: ConPTY は APC と DCS を落とすので、ブリッジのタブで `printf` する。`images` を待ってから `picture()` で画素を読む。位置は `terminal_x` などから計算する（`cell_pixel`）。
- ダイアログ: 編集のダイアログや確認のダイアログの間も要求は処理される（ダイアログのループが届ける）。`wait_dialogs` で待ち、`press_dialog_button` で押す。
- 設定ダイアログ: `open_settings` で開き、`show_page` でページを移し、`set_text`・`select`・`choose_radio`・`spin`・`click` で操作する。`read_controls` と `control` で値を読み、値は文言ではなくコントロールの ID と `saved_config`（`config.toml` を `fantermo_core::config::parse` で読んだもの）で見る。
  - 操作は利用者の操作と同じ通知をウィンドウプロシージャへこの場で送る。`click` のほかは終わってから答えるので、続けて読めば結果が見える。`click` は答えを先に返してから送るが（押すと閉じるまで戻らないダイアログがあるため）、続く要求はそのあとに処理される。ファイルの中身を見る前に要求を一つ挟む（`press_apply`）か、`wait_dialogs(0)` で閉じるのを待つ。
  - 隠れているページのコントロール、押せないコントロール、無効なダイアログの操作は 400 で断る。操作の前に `show_page` する。テストは利用者のできることだけをする。
  - 項目を打つと 400 ms 待って反映するので、端末への反映は `wait_until`・`wait_pixels` で待つ。矢印、コンボボックス、ラジオボタン、ボタンはすぐ反映する。
  - 入れ子のダイアログは `wait_top_dialog`（`state` の `dialogs` はいちばん上が先頭）で待ち、同じ要求で操作する。メッセージボックスは `wait_message_box`。はい / いいえは `click`（6 / 7）、OK だけのものは `close_message_box`。色を選ぶダイアログは赤・緑・青の入力欄（706〜708）に書いて `click(1)`、ファイルを選ぶダイアログは `choose_file`（ファイル名の欄に書いて `click(1)`）。フォルダを選ぶダイアログは書いても移るだけで選べないので、やめるところだけを見る。
  - キーを読み取るダイアログは `capture_key`。修飾キーはダイアログのループがキーを読むときに読むので、`POST /e2e/dialog/key` は積んだあとダイアログが閉じるまでスレッドのキーボードの状態を保ち、閉じたあとの最初の要求で戻す（`release_kept_modifiers`）。
  - 未知のキーがある設定は起動時に警告のメッセージボックスが開く。閉じてから設定ダイアログを開く。
  - 読めないファイルは共有しないで開いて作る（`with_settings_file_open`）。読めるが置き換えられないのは読むことだけ共有する形と読み取り専用の属性。起動時に読めないファイルはフォルダで作る。
  - ダイアログは `FANTERMO_DPI` ではなくモニターの DPI で開く（手元は 192）。大きさはその DPI で測る（`assert_controls_fit`）。
- 子プロセス: `Server::start_watching_children` で起動すると、fantermo が終わったあと run-hidden が子孫の終わりを待ち（`--wait-children`）、`assert_no_children_remained` で残りが分かる。付けないと、run-hidden が Job を閉じて全部終わらせるので、残りがあっても見えない。タブを閉じる確認は `common::processes`（親子と作成時刻）で。Cygwin のプログラムは Windows の親子で辿れないので、`/proc` のコマンドラインと一意な `sleep` の引数で見る。
- クリップボード: `Server` は既定で `FANTERMO_E2E_CLIPBOARD_FILE`（一時フォルダのファイル）を渡し、fantermo はそのファイルをクリップボードの代わりに使う。OS のものを使うのは `Server::start_with_os_clipboard` だけで、`OsClipboard` が直列にする。
- 起動の前に置くファイル: `Server::start_with_setup` の `prepare` が一時フォルダで呼ばれる。`state.toml` の初期値（`common::STATE_FILE`）はここで置く。閉じたあとの中身は `state_file()` で、`Server` を破棄する前に読む。
- 背景画像: `common::encode_png` で作った PNG を `prepare` で置き、設定には相対パスで書く（設定ファイルのフォルダから辿る）。候補が一枚なら選び方はランダムにならない。位置は状態の `background` の範囲から決め、文字のないところを見る。画素は `wait_pixels` で待ち、背景色を `[colors] background` で固定して比べる。合成の丸めとソフトウェアの描画の差の分、許容の幅を持たせる（`BACKGROUND_COLOR_TOLERANCE`）。
- ログオフ: `send_message` で `queryendsession`・`endsession` を送り、`state_file()` を読む。書かれたかどうかは、送る前にファイルを消して見る。OS がプロセスごと終わらせるところは `processes::terminate` で fantermo だけを終わらせて真似る。fantermo のほかのプロセスは OS に終わらされないので、残らないことの確認は実際のログオフより厳しい。
- マウス: `Server::mouse` に行を渡す（`e2e.rs` の `left_click`・`left_drag`・`button_click`・`wheel`・`step` で作る）。位置はセルなら `cell_center`・`cell_left`・`cell_right`、ほかは状態の `scroll_bar`・`tab_bar` から決める。文字の位置は `find_cell`。
  - 答えを先に返してから送る（メニューを開くと閉じるまで戻らないため）。続く要求はそのあとに処理されるので、次に読む状態には送った分が入っている。最後のタブの閉じるボタンでウィンドウが終わっても答えは返る。
  - ダブルクリックは `GetDoubleClickTime` の間隔の中で判定するので、一つの要求に続けて書く（別の要求にすると、負荷が高いと間に合わない）。逆に、同じ位置を間隔の中で押すとダブルクリックになるので、選び直すときは別の位置を挟む。つまみの外を押して離す 1 ページの送りも一つの要求に書く（250 ms で繰り返しが始まる）。押し続けるのを見るときは `left-down` だけ送り、`wait_until` で待つ。
  - 押していることは `wParam`（`MK_*`）に、修飾キーはそれに加えてウィンドウのスレッドのキーボードの状態（`SetKeyboardState`。`keyboard::current_mods` が読む）に、送る間だけ入れる。書いていない修飾キーは離した状態にする。別デスクトップのスレッドの `GetKeyState` にも利用者が本物のキーボードで押した修飾キーが見えることがあり、そのまま読むと Shift で報告が選択に変わるなど、テストが時々落ちる。
  - メニュー: `open_menu` で開いて項目を読み（`wait_menu` は `dialogs` の `class` が `#32768` になるのを待つ）、`close_menu` で閉じる。メニューを開いている間も要求は処理される。
    - 並びは `menu_shape` の短い形（`"C- P | A | T W"`。アクセラレータ、押せなければ `-`、印があれば `*`、区切りは `|`）で固定する。順番と押せるかは固定し、言い回しは比べない。文字を比べるのは設定から決まるもの（プロファイルの名前、添えるキー）だけ。
    - 選ぶのは `select_item`（アクセラレータで位置を探す）か `select_position`。先に `highlight_position` で移ったのを待ってから Enter を送るので、移れないときはそこで項目と状態を出して落ちる。答えは選んだ結果を待たないので、結果は状態で待つ。ダイアログを開く項目は `wait_dialog_of_class` で待つ。
    - 矢印のキーは押せない項目にも止まる。押せない項目で Enter を押すと、何も選ばずにメニューが閉じる。
    - アクセラレータの `POST /e2e/menu/key/<英数字>` は `TranslateMessage` の文字で届く。利用者が本物のキーボードで押した Ctrl がスレッドのキーボードの状態に見えると、選べないはず（起きたことはなく、見当）。矢印のキーと Enter は文字を通らない。
    - 「設定ファイルを開く」は既定のアプリを起動するので、選ばない。
  - マウスの報告: アプリ側は `start_report_reader`（`stty -icanon -echo -isig -icrnl` と `cat -v`）で読み、`reports_of` が操作ごとの受け取った分を返す。モードごとに `start_report_reader_in_new_tab`。ConPTY では PowerShell で `SetConsoleMode` に `ENABLE_VIRTUAL_TERMINAL_INPUT` を渡して読む（`mouse_reports_reach_an_application_through_conpty`）。
  - 別デスクトップでは本物のカーソルがウィンドウの上にないので、`TrackMouseEvent` の `WM_MOUSELEAVE` がすぐ届く。マウスを載せている間の絵は見られない。リンクの Ctrl+クリックは既定のブラウザを開くので通さない。
- DPI: `Server` は既定で `FANTERMO_DPI=96` を付ける。別の DPI で起動するのは `Server::start_with_environment`（返した環境変数が優先する）か `start_at_dpi`。起動のあとに変えるのは `change_dpi`（`POST /e2e/message/dpichanged/<DPI>`。この場で終わるので待たない）。
  - モニターの DPI で起動するのは、`start_with_environment` で `FANTERMO_DPI` を空にする（読めない値は無視する）。
  - モニターの DPI は変えられない。変わるのは `WM_DPICHANGED` に反応する部分（セル、フォント、余白、タブバー、スクロールバー、アイコン）だけで、ウィンドウの枠と、`GetDpiForWindow` を読むダイアログ（設定、文字のダイアログ）はモニターの DPI のまま。`monitor_dpi` と `dpi` の両方を見る。手元は 192、ランナーは 96 なので、`monitor_dpi` と同じ DPI のときだけ成り立つことは、それで分ける。
  - 寸法は固定せず、状態の値から求める（タブバーの高さは `background` の `y`、余白は `terminal_x`）。比べるときの基準は 96 DPI で読んだ値。フォントの測り直しは `at <DPI> DPI: cell` のログの行で分かる。
  - 192 DPI でも 1024×768 に収まるよう、先に `resize(DPI_TEST_GRID)` で小さくする。`change_dpi` が収まることを確認する。既定の 100×30 のままだと手元で通ってランナーで OS がウィンドウを切り詰める。
  - DPI を変えるとウィンドウは DPI の比で拡大・縮小し、桁数と行数はその中に入る数になる（セルの丸め方はフォントで違うので、変わる数は固定しない）。設定でフォント・スクロールバーの幅・余白を変えてもウィンドウは動かない。どちらも `assert_layout_fits_the_client`（桁数と行数がクライアント領域から求めた数）で見る。セルで割り切れない大きさは `start_uneven`（`state.toml` に `UNEVEN_WINDOW`）で起動する。
  - 描画は背景色のセルの四隅と外側（`assert_painted_cells_are_where_the_state_says`）で見る。文字の画素は、ヒンティングで DPI の比ちょうどにならないので、インクの範囲に許容の幅を持たせる。
- メッセージを足す: `WindowMessage` に足し、`send_window_message` で `lParam` を決める。`SendMessageW` はこの場でウィンドウプロシージャを動かすので、借用の外（`window/e2e.rs` の `run`）から送る。借用の中だと状態を書く処理などが黙って飛ばされる。
- 更新の確認: `ReleaseSource`（`common/release_host.rs` の `ReleaseHost` と `common/signing.rs` の `SigningKey`）がテストの配布元で、`start_fantermo(いまのバージョン)` で起動する。`ReleaseHost` は `127.0.0.1` の HTTP で、`publish` で `latest.json`・署名・配布物のダミーを置き、`host.set` で答え（`Reply`。ステータス、切れた本体、大きな本体、答えない、`Gate` が開くまで止める）を変える。一つの fantermo で答えを変えて順に確認する。署名は `tools/manifest.sh` と同じ形（SHA256withECDSA の DER を base64 にした 1 行）で、鍵はテストの実行中に CNG で作り、ファイルに書かない。リポジトリの署名の鍵は使わない。
  - feature `e2e` のときだけ効く環境変数（`crates/fantermo/src/update/stand_in.rs`）: `FANTERMO_E2E_UPDATE_PORT`（`127.0.0.1` の取得先のポート。あれば「更新を確認」を押せる）、`FANTERMO_E2E_UPDATE_PUBLIC_KEY`（公開鍵の DER を 16 進で。なければ埋め込みの公開鍵）、`FANTERMO_E2E_UPDATE_CURRENT_VERSION`（いまのバージョン）、`FANTERMO_E2E_UPDATE_HANDOFF_RECORD`（スクリプトのパスを書くファイル）。feature `e2e` のビルドは HTTPS の取得とスクリプトの起動をコンパイルしないので、GitHub へ接続せず、PowerShell も `msiexec` も起動しない。「OK」を押しても、書いたスクリプトのパスを記録して閉じるだけ。中身は `handed_off_script` で読む。feature なしのビルドは逆に `stand_in` をコンパイルしない。
  - 配布物は fantermo の `TMP`（一時フォルダの `tmp`）に置かれるので、`download_folders` で見える。
  - テストは並列に動き、実行ファイルが同じなので、ダウンロードのあとに「ほかに fantermo が…」が出ることがある。`wait_confirm_apply` が続けて、最後の確認まで進む。数を固定しない。
  - 「更新を確認」を押して結果を閉じたあと、押せるようになるのは閉じたあとなので、`dismiss_result`・`decline_offer` で待ってから次を押す。
  - 署名や読み取りの文は `CheckError::message` の文で見る。読めない・ハッシュが合わない場合を確認するには、読めない更新の情報にもテストの鍵で署名する（`publish_manifest`）。署名しないと署名のエラーで止まる。
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
