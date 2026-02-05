//! safe-rm: Safe file deletion tool for AI agents
//!
//! This tool provides Git-aware access control for file deletion,
//! allowing AI agents to safely delete only clean or ignored files.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use safe_rm::cli::{CliArgs, Commands};
use safe_rm::config::Config;
use safe_rm::error::{FileStatus, SafeRmError};
use safe_rm::git_checker::GitChecker;
use safe_rm::init;
use safe_rm::path_checker::PathChecker;

fn main() -> ExitCode {
    let args = CliArgs::parse_args();

    // Handle subcommands
    if let Some(Commands::Init) = args.command {
        match init::run_init() {
            Ok(()) => return ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("safe-rm: {}", e);
                return ExitCode::FAILURE;
            }
        }
    }

    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("safe-rm: {}", e);
            e.exit_code().into()
        }
    }
}

/// Main execution logic
fn run(args: CliArgs) -> Result<(), SafeRmError> {
    // Load user configuration
    let config = Config::load();

    // Get current working directory
    let cwd = std::env::current_dir().map_err(SafeRmError::IoError)?;

    // Open Git repository if available
    let git_checker = GitChecker::open(&cwd);

    // Use Git repository root as project boundary (not just cwd)
    // This allows absolute paths within the same repo to work correctly
    // e.g., running from frontend/ but deleting backend/file.txt
    let project_root = git_checker
        .as_ref()
        .and_then(|checker| checker.workdir())
        .unwrap_or_else(|| cwd.clone());

    // Pre-fetch all Git statuses at once (batch optimization)
    // This reduces N API calls to 1 for N files
    let status_cache: HashMap<String, FileStatus> = git_checker
        .as_ref()
        .map(|checker| checker.get_all_statuses())
        .unwrap_or_default();

    let mut success_count = 0;
    let mut error_count = 0;
    let mut max_exit_code: u8 = 0;
    let mut last_error: Option<SafeRmError> = None;

    for path in &args.paths {
        match process_path(
            path,
            &project_root,
            &cwd,
            &git_checker,
            &status_cache,
            &args,
            &config,
        ) {
            Ok(deleted) => {
                if deleted {
                    success_count += 1;
                }
            }
            Err(e) => {
                eprintln!("safe-rm: {}: {}", path.display(), e);
                let exit_code = e.exit_code();
                if exit_code > max_exit_code {
                    max_exit_code = exit_code;
                    last_error = Some(e);
                } else if last_error.is_none() {
                    last_error = Some(e);
                }
                error_count += 1;
            }
        }
    }

    if error_count > 0 {
        // Return the error with highest exit code (security blocks take precedence)
        if max_exit_code == 2 {
            // Return the security error directly
            Err(last_error.unwrap())
        } else {
            Err(SafeRmError::PartialFailure {
                success: success_count,
                failed: error_count,
            })
        }
    } else {
        Ok(())
    }
}

/// Process a single path for deletion
fn process_path(
    path: &Path,
    project_root: &Path,
    cwd: &Path,
    git_checker: &Option<GitChecker>,
    status_cache: &HashMap<String, FileStatus>,
    args: &CliArgs,
    config: &Config,
) -> Result<bool, SafeRmError> {
    // Resolve path to absolute (relative paths are resolved from cwd, not git root)
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };

    // Check if path is in allowed_paths (bypass containment and Git checks)
    if config.is_path_allowed(&abs_path) {
        // Check if path exists
        if !abs_path.exists() {
            if args.force {
                return Ok(false);
            } else {
                return Err(SafeRmError::NotFound(abs_path));
            }
        }

        // Check if it's a directory without -r flag
        if abs_path.is_dir() && !args.recursive {
            return Err(SafeRmError::IsDirectory(abs_path));
        }

        // Perform deletion (or dry-run) — skip containment and Git checks
        if args.dry_run {
            println!("would remove: {} (allowed by config)", path.display());
            Ok(true)
        } else {
            delete_path(&abs_path, args.recursive)?;
            println!("removed: {} (allowed by config)", path.display());
            Ok(true)
        }
    } else {
        // Standard safety checks

        // Verify path is within project FIRST (security check takes precedence)
        // This prevents information disclosure about file existence outside project
        // project_root is the git repo root (or cwd if no git repo)
        // cwd is used as the base for resolving relative paths
        let canonical_path = PathChecker::verify_containment_with_base(project_root, cwd, path)?;

        // Check if path exists
        if !abs_path.exists() {
            if args.force {
                // --force: ignore nonexistent files
                return Ok(false);
            } else {
                return Err(SafeRmError::NotFound(abs_path));
            }
        }

        // Check if it's a directory without -r flag
        if abs_path.is_dir() && !args.recursive {
            return Err(SafeRmError::IsDirectory(abs_path));
        }

        // Check Git status using pre-fetched cache (batch optimization)
        // Skip if allow_project_deletion is enabled (containment already verified above)
        if !config.allow_project_deletion {
            if let Some(ref checker) = git_checker {
                checker.check_path_with_cache(&canonical_path, status_cache)?;
            }
        }

        // Perform deletion (or dry-run)
        if args.dry_run {
            println!("would remove: {}", path.display());
            Ok(true)
        } else {
            delete_path(&abs_path, args.recursive)?;
            println!("removed: {}", path.display());
            Ok(true)
        }
    }
}

/// Delete a file or directory
fn delete_path(path: &Path, recursive: bool) -> Result<(), SafeRmError> {
    if path.is_dir() {
        if recursive {
            fs::remove_dir_all(path).map_err(SafeRmError::IoError)?;
        } else {
            fs::remove_dir(path).map_err(SafeRmError::IoError)?;
        }
    } else {
        fs::remove_file(path).map_err(SafeRmError::IoError)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_project_compiles() {
        // Basic smoke test to verify the project compiles correctly
    }

    #[test]
    fn test_version_available() {
        let version = env!("CARGO_PKG_VERSION");
        assert!(!version.is_empty());
        // Version is defined in Cargo.toml, just verify it's a valid semver format
        assert!(version.contains('.'), "Version should be in semver format");
    }
}
