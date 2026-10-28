//! fantermo core — UI に依存しない層。
//!
//! libghostty-vt の上に載せた端末の状態、バックエンドの trait、設定、ブリッジ（シェルとの間を中継する補助プログラム）とのプロトコルを置く。

pub mod backend;
pub mod config;
pub mod frame;
pub mod vt;
