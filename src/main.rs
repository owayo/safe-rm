//! safe-rm: Safe file deletion tool for AI agents
//!
//! This tool provides Git-aware access control for file deletion,
//! allowing AI agents to safely delete only clean or ignored files.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use safe_rm::cli::CliArgs;
use safe_rm::error::SafeRmError;
use safe_rm::git_checker::GitChecker;
use safe_rm::path_checker::PathChecker;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("safe-rm: {}", e);
            e.exit_code().into()
        }
    }
}

/// Main execution logic
fn run() -> Result<(), SafeRmError> {
    let args = CliArgs::parse_args();

    // Get project root (current working directory)
    let project_root = std::env::current_dir().map_err(SafeRmError::IoError)?;

    // Open Git repository if available
    let git_checker = GitChecker::open(&project_root);

    let mut success_count = 0;
    let mut error_count = 0;
    let mut max_exit_code: u8 = 0;
    let mut last_error: Option<SafeRmError> = None;

    for path in &args.paths {
        match process_path(path, &project_root, &git_checker, &args) {
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
    git_checker: &Option<GitChecker>,
    args: &CliArgs,
) -> Result<bool, SafeRmError> {
    // Verify path is within project FIRST (security check takes precedence)
    // This prevents information disclosure about file existence outside project
    let canonical_path = PathChecker::verify_containment(project_root, path)?;

    // Resolve path to absolute
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };

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

    // Check Git status
    if let Some(ref checker) = git_checker {
        if abs_path.is_dir() {
            // For directories, check all files recursively
            check_directory_recursive(&abs_path, checker)?;
        } else {
            checker.check_path(&canonical_path)?;
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

/// Recursively check all files in a directory
fn check_directory_recursive(dir: &Path, checker: &GitChecker) -> Result<(), SafeRmError> {
    let entries = fs::read_dir(dir).map_err(|_| SafeRmError::DirectoryReadError {
        path: dir.to_path_buf(),
    })?;

    for entry in entries {
        let entry = entry.map_err(|_| SafeRmError::DirectoryReadError {
            path: dir.to_path_buf(),
        })?;
        let entry_path = entry.path();

        if entry_path.is_dir() {
            check_directory_recursive(&entry_path, checker)?;
        } else {
            checker.check_path(&entry_path)?;
        }
    }

    Ok(())
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
