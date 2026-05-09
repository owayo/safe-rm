//! safe-rm: AIエージェント向け安全なファイル削除ツール
//!
//! プロジェクト境界と Git 管理メタデータを保護するファイル削除ライブラリ。
//! 厳格モードでは Git 状態に基づき未コミット変更の削除もブロックする。

pub mod cli;
pub mod config;
pub mod error;
pub mod git_checker;
pub mod init;
pub mod path_checker;
