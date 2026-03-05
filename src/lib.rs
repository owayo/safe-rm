//! safe-rm: AIエージェント向け安全なファイル削除ツール
//!
//! Git状態に基づくアクセス制御を備えたファイル削除ライブラリ。
//! Clean または Ignored 状態のファイルのみ削除を許可する。

pub mod cli;
pub mod config;
pub mod error;
pub mod git_checker;
pub mod init;
pub mod path_checker;
