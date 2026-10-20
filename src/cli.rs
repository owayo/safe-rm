//! safe-rm の CLI 引数パーサー
//!
//! clap derive による型安全な引数パースを提供する。

use clap::builder::{OsStringValueParser, TypedValueParser};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// safe-rm の CLI 引数
#[derive(Parser, Debug)]
#[command(
    name = "safe-rm",
    version,
    about = "Safe file deletion tool for AI agents",
    long_about = "A CLI tool that blocks deletion outside the project and protects Git metadata.\n\
                  By default, worktree files inside the project are allowed. When \
                  allow_project_deletion = false, modified, staged, and untracked files are also blocked.",
    subcommand_negates_reqs = true
)]
pub struct CliArgs {
    /// サブコマンド（例: init）
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// 削除対象のファイルまたはディレクトリ
    #[arg(
        required_unless_present = "force",
        value_name = "PATH",
        value_parser = OsStringValueParser::new().map(PathBuf::from)
    )]
    pub paths: Vec<PathBuf>,

    /// 再帰削除（ディレクトリとその内容を削除）
    #[arg(short, long)]
    pub recursive: bool,

    /// 強制削除（存在しないファイルを無視）
    #[arg(short, long)]
    pub force: bool,

    /// ドライランモード（実際には削除せず、削除対象を表示）
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// ignore 済みディレクトリ内で、削除を認める Git 管理エントリを正確に指定
    #[arg(long, value_name = "GIT_PATH", requires = "recursive")]
    pub allow_nested_git: Vec<PathBuf>,
}

/// サブコマンド
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// 設定ファイルを初期化（~/.config/safe-rm/config.toml）
    Init,
    /// 異常終了で残った gitdir 直下の空の index.lock を、静止状態の明示と追加の検査の下で削除
    GitLock(GitLockArgs),
}

/// git-lock サブコマンドの引数
#[derive(Args, Debug)]
pub struct GitLockArgs {
    /// worktree のルートか gitdir（親ディレクトリは探索しない）
    #[arg(
        long,
        value_name = "PATH",
        value_parser = OsStringValueParser::new().map(PathBuf::from)
    )]
    pub repo: PathBuf,

    /// そのリポジトリで git の処理が動いておらず、削除が終わるまで新しい処理も gitdir を書き換えるほかの操作も始めないことを保証する（必須）
    #[arg(long)]
    pub assume_quiescent: bool,

    /// ドライランモード（削除せず、検査を通るかを表示）
    #[arg(short = 'n', long)]
    pub dry_run: bool,
}

impl CliArgs {
    /// コマンドライン引数をパース
    pub fn parse_args() -> Self {
        Self::parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_args(paths: Vec<&str>, recursive: bool, force: bool, dry_run: bool) -> CliArgs {
        CliArgs {
            command: None,
            paths: paths.into_iter().map(PathBuf::from).collect(),
            recursive,
            force,
            dry_run,
            allow_nested_git: Vec::new(),
        }
    }

    #[test]
    fn test_cli_args_debug() {
        let args = make_args(vec!["file.txt"], false, false, false);
        let debug_str = format!("{:?}", args);
        assert!(debug_str.contains("CliArgs"));
        assert!(debug_str.contains("file.txt"));
    }

    #[test]
    fn test_cli_args_single_file() {
        let args = make_args(vec!["file.txt"], false, false, false);
        assert_eq!(args.paths.len(), 1);
        assert_eq!(args.paths[0], PathBuf::from("file.txt"));
        assert!(!args.recursive);
        assert!(!args.force);
        assert!(!args.dry_run);
    }

    #[test]
    fn test_cli_args_multiple_files() {
        let args = make_args(
            vec!["file1.txt", "file2.txt", "dir/file3.txt"],
            false,
            false,
            false,
        );
        assert_eq!(args.paths.len(), 3);
    }

    #[test]
    fn test_cli_args_empty_path() {
        let args = CliArgs::try_parse_from(["safe-rm", ""]).unwrap();
        assert_eq!(args.paths, vec![PathBuf::new()]);
    }

    #[test]
    fn test_cli_args_force_without_path() {
        let args = CliArgs::try_parse_from(["safe-rm", "-f"]).unwrap();
        assert!(args.force);
        assert!(args.paths.is_empty());
    }

    #[test]
    fn test_cli_args_without_force_or_path_is_rejected() {
        assert!(CliArgs::try_parse_from(["safe-rm"]).is_err());
    }

    #[test]
    fn test_cli_args_recursive_flag() {
        let args = make_args(vec!["dir"], true, false, false);
        assert!(args.recursive);
    }

    #[test]
    fn test_cli_args_force_flag() {
        let args = make_args(vec!["file.txt"], false, true, false);
        assert!(args.force);
    }

    #[test]
    fn test_cli_args_dry_run_flag() {
        let args = make_args(vec!["file.txt"], false, false, true);
        assert!(args.dry_run);
    }

    #[test]
    fn test_cli_args_all_flags() {
        let args = make_args(vec!["dir"], true, true, true);
        assert!(args.recursive);
        assert!(args.force);
        assert!(args.dry_run);
    }

    #[test]
    fn test_cli_args_init_subcommand() {
        let args = CliArgs {
            command: Some(Commands::Init),
            paths: vec![],
            recursive: false,
            force: false,
            dry_run: false,
            allow_nested_git: Vec::new(),
        };
        assert!(matches!(args.command, Some(Commands::Init)));
    }

    #[test]
    fn test_cli_args_git_lock_subcommand() {
        let args = CliArgs::try_parse_from([
            "safe-rm",
            "git-lock",
            "--repo",
            "repo/.git",
            "--assume-quiescent",
            "-n",
        ])
        .unwrap();
        let Some(Commands::GitLock(lock_args)) = args.command else {
            panic!("git-lock として解析されない: {:?}", args.command);
        };
        assert_eq!(lock_args.repo, PathBuf::from("repo/.git"));
        assert!(lock_args.assume_quiescent);
        assert!(lock_args.dry_run);
        // ドライランはサブコマンド側の -n で、トップレベルの -n とは別
        assert!(!args.dry_run);
        assert!(args.paths.is_empty());
    }

    #[test]
    fn test_cli_args_git_lock_without_assume_quiescent_is_parsed() {
        // 無いときは clap のエラーではなく、意味を説明する safe-rm の拒否にする
        let args = CliArgs::try_parse_from(["safe-rm", "git-lock", "--repo", "."]).unwrap();
        let Some(Commands::GitLock(lock_args)) = args.command else {
            panic!("git-lock として解析されない");
        };
        assert!(!lock_args.assume_quiescent);
    }

    #[test]
    fn test_cli_args_git_lock_requires_repo() {
        assert!(CliArgs::try_parse_from(["safe-rm", "git-lock", "--assume-quiescent"]).is_err());
    }

    #[test]
    fn test_cli_args_git_lock_keeps_top_level_flags_for_rejection() {
        let args = CliArgs::try_parse_from([
            "safe-rm",
            "-r",
            "git-lock",
            "--repo",
            ".",
            "--assume-quiescent",
        ])
        .unwrap();
        assert!(args.recursive);
        assert!(matches!(args.command, Some(Commands::GitLock(_))));
    }

    #[test]
    fn test_cli_args_dot_slash_git_lock_is_a_path() {
        let args = CliArgs::try_parse_from(["safe-rm", "./git-lock"]).unwrap();
        assert!(args.command.is_none());
        assert_eq!(args.paths, vec![PathBuf::from("./git-lock")]);
    }
}
