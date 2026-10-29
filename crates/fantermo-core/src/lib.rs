//! fantermo core — UI に依存しない層。
//!
//! libghostty-vt の上に載せた端末の状態、バックエンドの trait、設定、窓の状態、背景画像の選び方と収め方、ブリッジ（シェルとの間を中継する補助プログラム）とのプロトコルを置く。

pub mod backend;
pub mod background;
pub mod config;
pub mod config_edit;
pub mod frame;
pub mod keymap;
pub mod link;
pub mod state;
pub mod vt;
