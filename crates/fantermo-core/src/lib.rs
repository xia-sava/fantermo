//! fantermo core — UI に依存しない層。
//!
//! libghostty-vt の上に載せた端末の状態、バックエンドの trait、設定、ウィンドウの状態、背景画像の選び方と表示方法を置く。

pub mod backend;
pub mod background;
pub mod config;
pub mod config_edit;
pub mod frame;
pub mod keymap;
pub mod link;
pub mod output_scan;
pub mod sixel;
pub mod state;
pub mod vt;
