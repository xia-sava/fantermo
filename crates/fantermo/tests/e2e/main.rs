//! 実際の fantermo を画面に出ないデスクトップで起動して、制御の口で確認する（feature `e2e` のときだけ実行する）。
//!
//! 実行は `./tools/dev.sh test --features e2e`。`smoke` で始まるテストは、その環境で別デスクトップと Direct3D が
//! 動くかだけを確認する。
//!
//! ブリッジのテストは環境変数があるときだけ実行し、なければスキップしたと標準エラー出力に出す。
//! - `FANTERMO_E2E_CYGWIN_ROOT`: Cygwin のフォルダ（`C:\cygwin64` など）。`./tools/bridge.sh build cygwin` で作っておく
//! - `FANTERMO_E2E_WSL`: WSL のディストリビューション（`Ubuntu` など）。`./tools/bridge.sh build linux` で作っておく
//!
//! クリップボードのテストは、一時フォルダのファイルを OS のクリップボードの代わりに使う。OS のクリップボードを
//! 実際に変えるテストは、環境変数 `FANTERMO_E2E_OS_CLIPBOARD` があるときだけ実行する（CI で付ける）。
//!
//! 設定ダイアログのテストのうち、OS のファイルを選ぶダイアログで実際に選ぶものは、環境変数 `FANTERMO_E2E_FILE_DIALOG` が
//! あるときだけ実行する（CI で付ける）。

#[path = "../common/mod.rs"]
mod common;

mod background;
mod basics;
mod clipboard;
mod command_line;
mod dpi;
mod images;
mod instances;
mod menu;
mod mouse;
mod paste;
mod processes;
mod session_end;
mod settings;
mod ui;
mod update;
mod window_state;
