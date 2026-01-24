//! CLI argument parser for safe-rm
//!
//! Provides type-safe argument parsing using clap derive.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// CLI arguments for safe-rm
#[derive(Parser, Debug)]
#[command(
    name = "safe-rm",
    version,
    about = "Safe file deletion tool for AI agents",
    long_about = "A CLI tool that provides Git-aware access control for file deletion.\n\
                  It allows deleting only clean or ignored files within the project directory,\n\
                  preventing accidental deletion of uncommitted work or files outside the project.",
    subcommand_negates_reqs = true
)]
pub struct CliArgs {
    /// Subcommand (e.g., init)
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Files or directories to delete
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<PathBuf>,

    /// Recursive deletion (remove directories and their contents)
    #[arg(short, long)]
    pub recursive: bool,

    /// Force deletion (ignore nonexistent files)
    #[arg(short, long)]
    pub force: bool,

    /// Dry run mode (show what would be deleted without actually deleting)
    #[arg(short = 'n', long)]
    pub dry_run: bool,
}

/// Subcommands
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Initialize configuration file (~/.config/safe-rm/config.toml)
    Init,
}

impl CliArgs {
    /// Parse CLI arguments from command line
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
        };
        assert!(matches!(args.command, Some(Commands::Init)));
    }
}
