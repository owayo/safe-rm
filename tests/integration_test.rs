//! Integration tests for safe-rm CLI
//!
//! Tests the CLI interface with real file system operations.

use std::fs;
use std::process::Command;
use tempfile::TempDir;

/// テスト用の safe-rm バイナリのパスを取得
fn get_binary_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/target/debug/safe-rm", manifest_dir)
}

/// safe-rm を実行してステータスを取得
fn run_safe_rm(args: &[&str], cwd: &std::path::Path) -> (i32, String, String) {
    run_safe_rm_with_config(args, cwd, None)
}

/// safe-rm を実行してステータスを取得（カスタム設定ファイル指定可能）
fn run_safe_rm_with_config(
    args: &[&str],
    cwd: &std::path::Path,
    config_path: Option<&std::path::Path>,
) -> (i32, String, String) {
    let binary = get_binary_path();

    let mut cmd = Command::new(&binary);
    cmd.args(args).current_dir(cwd);

    if let Some(path) = config_path {
        cmd.env("SAFE_RM_CONFIG", path);
    }

    let output = cmd.output().expect("Failed to execute safe-rm");

    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    (exit_code, stdout, stderr)
}

/// テスト用の Git リポジトリを作成
fn create_test_repo() -> TempDir {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    Command::new("git")
        .args(["init"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    temp_dir
}

/// ファイルをコミット
fn commit_file(repo_path: &std::path::Path, filename: &str, content: &str) {
    let file_path = repo_path.join(filename);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&file_path, content).unwrap();

    Command::new("git")
        .args(["add", filename])
        .current_dir(repo_path)
        .output()
        .unwrap();

    Command::new("git")
        .args(["commit", "-m", &format!("Add {}", filename)])
        .current_dir(repo_path)
        .output()
        .unwrap();
}

// =============================================================================
// 許可フローのテスト
// =============================================================================

mod allow_flow_tests {
    use super::*;

    #[test]
    fn test_clean_file_deletion() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // Clean ファイルを作成
        commit_file(&repo_path, "clean.txt", "clean content");

        // 削除を実行
        let (exit_code, stdout, stderr) = run_safe_rm(&["clean.txt"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "Clean file deletion should succeed. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("clean.txt").exists(),
            "File should be deleted"
        );
    }

    #[test]
    fn test_ignored_file_deletion() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を作成してコミット
        fs::write(repo_path.join(".gitignore"), "ignored.txt\n").unwrap();
        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Ignored ファイルを作成
        fs::write(repo_path.join("ignored.txt"), "ignored content").unwrap();

        // 削除を実行
        let (exit_code, stdout, stderr) = run_safe_rm(&["ignored.txt"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "Ignored file deletion should succeed. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("ignored.txt").exists(),
            "File should be deleted"
        );
    }

    #[test]
    fn test_ignored_directory_deletion() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を作成してコミット
        fs::write(repo_path.join(".gitignore"), "build/\n").unwrap();
        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Ignored ディレクトリを作成
        let build_dir = repo_path.join("build");
        fs::create_dir(&build_dir).unwrap();
        fs::write(build_dir.join("output.o"), "binary").unwrap();

        // -r フラグ付きで削除を実行
        let (exit_code, stdout, stderr) = run_safe_rm(&["-r", "build"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "Ignored directory deletion should succeed. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("build").exists(),
            "Directory should be deleted"
        );
    }

    #[test]
    fn test_not_in_git_repo() {
        // 非 Git ディレクトリ
        let temp_dir = TempDir::new().unwrap();
        let project_path = temp_dir.path().canonicalize().unwrap();

        fs::write(project_path.join("file.txt"), "content").unwrap();

        let (exit_code, stdout, stderr) = run_safe_rm(&["file.txt"], &project_path);

        assert_eq!(
            exit_code, 0,
            "Non-git file deletion should succeed. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !project_path.join("file.txt").exists(),
            "File should be deleted"
        );
    }

    #[test]
    fn test_dry_run_mode() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // Clean ファイルを作成
        commit_file(&repo_path, "dryrun.txt", "dry run content");

        // --dry-run で実行
        let (exit_code, stdout, _) = run_safe_rm(&["--dry-run", "dryrun.txt"], &repo_path);

        assert_eq!(exit_code, 0, "Dry run should succeed");
        assert!(
            stdout.contains("would remove:"),
            "Should show what would be removed"
        );
        assert!(
            repo_path.join("dryrun.txt").exists(),
            "File should NOT be deleted in dry run"
        );
    }

    #[test]
    fn test_force_nonexistent_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 存在しないファイルに -f フラグ
        let (exit_code, _, _) = run_safe_rm(&["-f", "nonexistent.txt"], &repo_path);

        assert_eq!(exit_code, 0, "-f should ignore nonexistent files");
    }

    #[test]
    fn test_absolute_path_sibling_directory() {
        // 再現シナリオ: frontend/ から backend/file.txt をフルパスで削除
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // frontend/ と backend/ サブディレクトリを作成
        let frontend = repo_path.join("frontend");
        let backend = repo_path.join("backend");
        fs::create_dir(&frontend).unwrap();
        fs::create_dir(&backend).unwrap();

        // backend/file.txt を作成してコミット
        commit_file(&repo_path, "backend/file.txt", "backend content");

        // frontend/ から backend/file.txt のフルパスで削除
        let abs_path = backend.join("file.txt");
        let (exit_code, stdout, stderr) = run_safe_rm(&[abs_path.to_str().unwrap()], &frontend);

        assert_eq!(
            exit_code, 0,
            "Absolute path to sibling directory file should succeed. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(!abs_path.exists(), "File should be deleted");
    }

    #[test]
    fn test_absolute_path_within_same_repo() {
        // リポジトリルートのファイルをサブディレクトリからフルパスで削除
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // サブディレクトリを作成
        let subdir = repo_path.join("subdir");
        fs::create_dir(&subdir).unwrap();

        // ルートにファイルを作成してコミット
        commit_file(&repo_path, "root_file.txt", "root content");

        // subdir/ からルートのファイルをフルパスで削除
        let abs_path = repo_path.join("root_file.txt");
        let (exit_code, stdout, stderr) = run_safe_rm(&[abs_path.to_str().unwrap()], &subdir);

        assert_eq!(
            exit_code, 0,
            "Absolute path to repo root file should succeed. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(!abs_path.exists(), "File should be deleted");
    }

    #[test]
    fn test_multiple_files() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 複数の Clean ファイルを作成
        commit_file(&repo_path, "file1.txt", "content1");
        commit_file(&repo_path, "file2.txt", "content2");

        // 複数ファイルを削除
        let (exit_code, stdout, stderr) = run_safe_rm(&["file1.txt", "file2.txt"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "Multiple file deletion should succeed. stderr: {}",
            stderr
        );
        assert!(stdout.contains("file1.txt"), "Should mention file1");
        assert!(stdout.contains("file2.txt"), "Should mention file2");
        assert!(
            !repo_path.join("file1.txt").exists(),
            "file1 should be deleted"
        );
        assert!(
            !repo_path.join("file2.txt").exists(),
            "file2 should be deleted"
        );
    }
}

// =============================================================================
// ブロックフローのテスト（allow_project_deletion = false モード）
// =============================================================================

mod block_flow_tests {
    use super::*;

    /// allow_project_deletion = false の設定ファイルを作成
    fn create_strict_config() -> tempfile::NamedTempFile {
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();
        config
    }

    #[test]
    fn test_modified_file_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // ファイルをコミット後に変更
        commit_file(&repo_path, "modified.txt", "original");
        fs::write(repo_path.join("modified.txt"), "modified content").unwrap();

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["modified.txt"], &repo_path, Some(config.path()));

        assert_eq!(exit_code, 2, "Modified file deletion should be blocked");
        assert!(
            stderr.contains("Modified") || stderr.contains("変更"),
            "Error should mention modified status: {}",
            stderr
        );
        assert!(
            repo_path.join("modified.txt").exists(),
            "File should NOT be deleted"
        );
    }

    #[test]
    fn test_staged_file_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // 初期コミット
        commit_file(&repo_path, "init.txt", "init");

        // ファイルを作成してステージング
        fs::write(repo_path.join("staged.txt"), "staged content").unwrap();
        Command::new("git")
            .args(["add", "staged.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["staged.txt"], &repo_path, Some(config.path()));

        assert_eq!(exit_code, 2, "Staged file deletion should be blocked");
        assert!(
            stderr.contains("Staged") || stderr.contains("ステージング"),
            "Error should mention staged status: {}",
            stderr
        );
        assert!(
            repo_path.join("staged.txt").exists(),
            "File should NOT be deleted"
        );
    }

    #[test]
    fn test_untracked_file_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // 初期コミット
        commit_file(&repo_path, "init.txt", "init");

        // 未追跡ファイルを作成
        fs::write(repo_path.join("untracked.txt"), "untracked content").unwrap();

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["untracked.txt"], &repo_path, Some(config.path()));

        assert_eq!(exit_code, 2, "Untracked file deletion should be blocked");
        assert!(
            stderr.contains("Untracked") || stderr.contains("未追跡"),
            "Error should mention untracked status: {}",
            stderr
        );
        assert!(
            repo_path.join("untracked.txt").exists(),
            "File should NOT be deleted"
        );
    }

    #[test]
    fn test_directory_with_dirty_file_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // ディレクトリを作成
        let subdir = repo_path.join("subdir");
        fs::create_dir(&subdir).unwrap();

        // Clean ファイルをコミット
        commit_file(&repo_path, "subdir/clean.txt", "clean");

        // 未追跡ファイルを追加
        fs::write(subdir.join("untracked.txt"), "untracked").unwrap();

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["-r", "subdir"], &repo_path, Some(config.path()));

        assert_eq!(exit_code, 2, "Directory with dirty file should be blocked");
        assert!(!stderr.is_empty(), "Should have error message");
        assert!(
            repo_path.join("subdir").exists(),
            "Directory should NOT be deleted"
        );
    }

    #[test]
    fn test_outside_project_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let (exit_code, _, stderr) = run_safe_rm(&["/etc/passwd"], &repo_path);

        assert_eq!(exit_code, 2, "Outside project path should be blocked");
        assert!(
            stderr.contains("プロジェクト外") || stderr.contains("Outside"),
            "Error message should indicate outside project: {}",
            stderr
        );
    }

    #[test]
    fn test_traversal_attack_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let (exit_code, _, stderr) = run_safe_rm(&["../../../etc/passwd"], &repo_path);

        assert_eq!(exit_code, 2, "Traversal attack should be blocked");
        assert!(
            stderr.contains("プロジェクト外") || stderr.contains("Outside"),
            "Error message should indicate outside project: {}",
            stderr
        );
    }

    #[test]
    fn test_non_git_directory_outside_project_blocked() {
        // プロジェクト（Gitリポジトリ）を作成
        let project_dir = create_test_repo();
        let project_path = project_dir.path().canonicalize().unwrap();

        // プロジェクト外に非Gitディレクトリを作成
        let outside_dir = TempDir::new().unwrap();
        let outside_path = outside_dir.path().canonicalize().unwrap();
        let outside_file = outside_path.join("outside_file.txt");
        fs::write(&outside_file, "this is outside").unwrap();

        // 非Gitディレクトリ内のファイルを削除しようとする → ブロックされるべき
        let (exit_code, _, stderr) = run_safe_rm(&[outside_file.to_str().unwrap()], &project_path);

        assert_eq!(
            exit_code, 2,
            "Non-Git directory outside project should be blocked"
        );
        assert!(
            stderr.contains("プロジェクト外") || stderr.contains("Outside"),
            "Error message should indicate outside project: {}",
            stderr
        );
        assert!(
            outside_file.exists(),
            "File outside project should NOT be deleted"
        );
    }

    #[test]
    fn test_nonexistent_file_without_force() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let (exit_code, _, stderr) = run_safe_rm(&["nonexistent.txt"], &repo_path);

        assert_eq!(exit_code, 1, "Nonexistent file should return exit code 1");
        assert!(
            stderr.contains("No such file"),
            "Error should mention file not found: {}",
            stderr
        );
    }

    #[test]
    fn test_directory_without_recursive_flag() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ディレクトリを作成
        fs::create_dir(repo_path.join("testdir")).unwrap();

        let (exit_code, _, stderr) = run_safe_rm(&["testdir"], &repo_path);

        assert_eq!(exit_code, 1, "Directory without -r should fail");
        assert!(
            stderr.contains("Is a directory") || stderr.contains("-r"),
            "Error should mention directory requires -r: {}",
            stderr
        );
    }
}

// =============================================================================
// エッジケースのテスト
// =============================================================================

// =============================================================================
// allow_project_deletion = true (デフォルトモード) のテスト
// =============================================================================

mod default_mode_tests {
    use super::*;

    #[test]
    fn test_default_mode_allows_modified_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ファイルをコミット後に変更
        commit_file(&repo_path, "modified.txt", "original");
        fs::write(repo_path.join("modified.txt"), "modified content").unwrap();

        // デフォルトモード（allow_project_deletion = true）では削除可能
        let (exit_code, stdout, stderr) = run_safe_rm(&["modified.txt"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "Modified file should be deletable in default mode. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("modified.txt").exists(),
            "File should be deleted"
        );
    }

    #[test]
    fn test_default_mode_allows_staged_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 初期コミット
        commit_file(&repo_path, "init.txt", "init");

        // ファイルを作成してステージング
        fs::write(repo_path.join("staged.txt"), "staged content").unwrap();
        Command::new("git")
            .args(["add", "staged.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // デフォルトモードでは削除可能
        let (exit_code, stdout, stderr) = run_safe_rm(&["staged.txt"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "Staged file should be deletable in default mode. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("staged.txt").exists(),
            "File should be deleted"
        );
    }

    #[test]
    fn test_default_mode_allows_untracked_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 初期コミット
        commit_file(&repo_path, "init.txt", "init");

        // 未追跡ファイルを作成
        fs::write(repo_path.join("untracked.txt"), "untracked content").unwrap();

        // デフォルトモードでは削除可能
        let (exit_code, stdout, stderr) = run_safe_rm(&["untracked.txt"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "Untracked file should be deletable in default mode. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("untracked.txt").exists(),
            "File should be deleted"
        );
    }

    #[test]
    fn test_default_mode_allows_dirty_directory_recursive() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ディレクトリを作成
        let subdir = repo_path.join("subdir");
        fs::create_dir(&subdir).unwrap();

        // Clean ファイルをコミット
        commit_file(&repo_path, "subdir/clean.txt", "clean");

        // 未追跡ファイルを追加
        fs::write(subdir.join("untracked.txt"), "untracked").unwrap();

        // デフォルトモードではダーティファイルを含むディレクトリも削除可能
        let (exit_code, stdout, stderr) = run_safe_rm(&["-r", "subdir"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "Directory with dirty files should be deletable in default mode. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("subdir").exists(),
            "Directory should be deleted"
        );
    }

    #[test]
    fn test_default_mode_still_blocks_outside_project() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // プロジェクト外へのアクセスはデフォルトモードでもブロックされる
        let (exit_code, _, stderr) = run_safe_rm(&["/etc/passwd"], &repo_path);

        assert_eq!(
            exit_code, 2,
            "Outside project path should be blocked even in default mode"
        );
        assert!(
            stderr.contains("プロジェクト外") || stderr.contains("Outside"),
            "Error message should indicate outside project: {}",
            stderr
        );
    }
}

// =============================================================================
// エッジケースのテスト
// =============================================================================

mod edge_case_tests {
    use super::*;

    /// allow_project_deletion = false の設定ファイルを作成
    fn create_strict_config() -> tempfile::NamedTempFile {
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();
        config
    }

    #[test]
    fn test_partial_failure() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // Clean ファイルを作成
        commit_file(&repo_path, "clean.txt", "clean");

        // 初期コミット
        commit_file(&repo_path, "init.txt", "init");

        // 未追跡ファイルを作成
        fs::write(repo_path.join("untracked.txt"), "untracked").unwrap();

        // clean.txt と untracked.txt を一緒に削除しようとする
        let (exit_code, stdout, stderr) = run_safe_rm_with_config(
            &["clean.txt", "untracked.txt"],
            &repo_path,
            Some(config.path()),
        );

        // clean.txt は削除成功、untracked.txt は失敗
        assert_ne!(exit_code, 0, "Should have partial failure");
        assert!(stdout.contains("clean.txt"), "clean.txt should be removed");
        assert!(
            !repo_path.join("clean.txt").exists(),
            "clean.txt should be deleted"
        );
        assert!(
            stderr.contains("untracked.txt"),
            "Error should mention untracked.txt"
        );
        assert!(
            repo_path.join("untracked.txt").exists(),
            "untracked.txt should NOT be deleted"
        );
    }

    #[test]
    fn test_help_flag() {
        let temp_dir = TempDir::new().unwrap();
        let project_path = temp_dir.path();

        let (exit_code, stdout, _) = run_safe_rm(&["--help"], project_path);

        assert_eq!(exit_code, 0, "--help should succeed");
        assert!(stdout.contains("safe-rm"), "Help should show program name");
        assert!(
            stdout.contains("--recursive"),
            "Help should mention --recursive"
        );
        assert!(stdout.contains("--force"), "Help should mention --force");
        assert!(
            stdout.contains("--dry-run"),
            "Help should mention --dry-run"
        );
    }

    #[test]
    fn test_version_flag() {
        let temp_dir = TempDir::new().unwrap();
        let project_path = temp_dir.path();

        let (exit_code, stdout, _) = run_safe_rm(&["--version"], project_path);

        assert_eq!(exit_code, 0, "--version should succeed");
        assert!(
            stdout.contains("safe-rm"),
            "Version should show program name"
        );
    }
}

// =============================================================================
// strict モードでの許可フローテスト
// =============================================================================

mod strict_mode_allow_tests {
    use super::*;

    /// allow_project_deletion = false の設定ファイルを作成
    fn create_strict_config() -> tempfile::NamedTempFile {
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();
        config
    }

    #[test]
    fn test_strict_mode_allows_clean_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // Clean ファイルを作成
        commit_file(&repo_path, "clean.txt", "clean content");

        // strictモードでもcleanファイルは削除可能
        let (exit_code, stdout, stderr) =
            run_safe_rm_with_config(&["clean.txt"], &repo_path, Some(config.path()));

        assert_eq!(
            exit_code, 0,
            "Clean file should be deletable in strict mode. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("clean.txt").exists(),
            "File should be deleted"
        );
    }

    #[test]
    fn test_strict_mode_allows_ignored_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // .gitignore を作成してコミット
        fs::write(repo_path.join(".gitignore"), "ignored.txt\n").unwrap();
        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Ignored ファイルを作成
        fs::write(repo_path.join("ignored.txt"), "ignored content").unwrap();

        // strictモードでもignoredファイルは削除可能
        let (exit_code, stdout, stderr) =
            run_safe_rm_with_config(&["ignored.txt"], &repo_path, Some(config.path()));

        assert_eq!(
            exit_code, 0,
            "Ignored file should be deletable in strict mode. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("ignored.txt").exists(),
            "File should be deleted"
        );
    }

    #[test]
    fn test_strict_mode_allows_ignored_directory_recursive() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // .gitignore を作成してコミット
        fs::write(repo_path.join(".gitignore"), "build/\n").unwrap();
        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Ignored ディレクトリを作成
        let build_dir = repo_path.join("build");
        fs::create_dir(&build_dir).unwrap();
        fs::write(build_dir.join("output.o"), "binary").unwrap();

        // strictモードでもignoredディレクトリは-rで削除可能
        let (exit_code, stdout, stderr) =
            run_safe_rm_with_config(&["-r", "build"], &repo_path, Some(config.path()));

        assert_eq!(
            exit_code, 0,
            "Ignored directory should be deletable in strict mode. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("build").exists(),
            "Directory should be deleted"
        );
    }

    #[test]
    fn test_strict_mode_allows_clean_directory_recursive() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // ディレクトリとcleanファイルを作成
        let subdir = repo_path.join("subdir");
        fs::create_dir(&subdir).unwrap();
        commit_file(&repo_path, "subdir/clean1.txt", "clean1");
        commit_file(&repo_path, "subdir/clean2.txt", "clean2");

        // strictモードでもすべてcleanなディレクトリは削除可能
        let (exit_code, stdout, stderr) =
            run_safe_rm_with_config(&["-r", "subdir"], &repo_path, Some(config.path()));

        assert_eq!(
            exit_code, 0,
            "Directory with only clean files should be deletable in strict mode. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("subdir").exists(),
            "Directory should be deleted"
        );
    }
}

// =============================================================================
// SAFE_RM_CONFIG 環境変数のテスト
// =============================================================================

mod env_config_tests {
    use super::*;

    #[test]
    fn test_env_config_nonexistent_path_fallback() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // Clean ファイルを作成
        commit_file(&repo_path, "clean.txt", "clean content");

        // 存在しない設定ファイルを指定 → デフォルト設定にフォールバック
        let (exit_code, stdout, _) = run_safe_rm_with_config(
            &["clean.txt"],
            &repo_path,
            Some(std::path::Path::new("/nonexistent/config.toml")),
        );

        assert_eq!(
            exit_code, 0,
            "Should fallback to default config and succeed"
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
    }

    #[test]
    fn test_env_config_invalid_toml_fallback() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // Clean ファイルを作成
        commit_file(&repo_path, "clean.txt", "clean content");

        // 無効なTOMLを含む設定ファイルを作成
        let invalid_config = tempfile::NamedTempFile::new().unwrap();
        fs::write(invalid_config.path(), "invalid[[[toml content").unwrap();

        // 無効な設定 → デフォルトにフォールバック
        let (exit_code, stdout, stderr) =
            run_safe_rm_with_config(&["clean.txt"], &repo_path, Some(invalid_config.path()));

        assert_eq!(
            exit_code, 0,
            "Should fallback to default config and succeed"
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            stderr.contains("warning") || stderr.contains("Warning"),
            "Should show warning about invalid config: {}",
            stderr
        );
    }

    #[test]
    fn test_env_config_applies_allowed_paths() {
        // プロジェクト外のディレクトリを作成
        let outside_dir = TempDir::new().unwrap();
        let outside_path = outside_dir.path().canonicalize().unwrap();
        let outside_file = outside_path.join("allowed_file.txt");
        fs::write(&outside_file, "content").unwrap();

        // allowed_pathsを含む設定ファイル
        let config = tempfile::NamedTempFile::new().unwrap();
        let config_content = format!(
            r#"
[[allowed_paths]]
path = "{}"
recursive = true
"#,
            outside_path.display()
        );
        fs::write(config.path(), config_content).unwrap();

        // プロジェクトを作成
        let project_dir = create_test_repo();
        let project_path = project_dir.path().canonicalize().unwrap();

        // allowed_pathsで許可されているので削除可能
        let (exit_code, stdout, stderr) = run_safe_rm_with_config(
            &[outside_file.to_str().unwrap()],
            &project_path,
            Some(config.path()),
        );

        assert_eq!(
            exit_code, 0,
            "File in allowed_paths should be deletable. stderr: {}",
            stderr
        );
        assert!(
            stdout.contains("removed:") && stdout.contains("allowed by config"),
            "Should show removed message with config annotation: {}",
            stdout
        );
        assert!(!outside_file.exists(), "File should be deleted");
    }
}

// =============================================================================
// allowed_paths のエラーハンドリングテスト
// =============================================================================

mod allowed_paths_error_tests {
    use super::*;

    #[test]
    fn test_allowed_paths_nonexistent_with_force() {
        let project_dir = create_test_repo();
        let project_path = project_dir.path().canonicalize().unwrap();

        // 存在しないパスを許可する設定
        let allowed_dir = TempDir::new().unwrap();
        let allowed_path = allowed_dir.path().canonicalize().unwrap();
        let nonexistent_file = allowed_path.join("nonexistent.txt");

        let config = tempfile::NamedTempFile::new().unwrap();
        let config_content = format!(
            r#"
[[allowed_paths]]
path = "{}"
recursive = true
"#,
            allowed_path.display()
        );
        fs::write(config.path(), config_content).unwrap();

        // -f フラグ付きで存在しないファイルを削除 → 成功（無視）
        let (exit_code, _, _) = run_safe_rm_with_config(
            &["-f", nonexistent_file.to_str().unwrap()],
            &project_path,
            Some(config.path()),
        );

        assert_eq!(
            exit_code, 0,
            "-f flag should ignore nonexistent file in allowed_paths"
        );
    }

    #[test]
    fn test_allowed_paths_nonexistent_without_force() {
        let project_dir = create_test_repo();
        let project_path = project_dir.path().canonicalize().unwrap();

        // 存在しないパスを許可する設定
        let allowed_dir = TempDir::new().unwrap();
        let allowed_path = allowed_dir.path().canonicalize().unwrap();
        let nonexistent_file = allowed_path.join("nonexistent.txt");

        let config = tempfile::NamedTempFile::new().unwrap();
        let config_content = format!(
            r#"
[[allowed_paths]]
path = "{}"
recursive = true
"#,
            allowed_path.display()
        );
        fs::write(config.path(), config_content).unwrap();

        // -f なしで存在しないファイルを削除 → 失敗
        let (exit_code, _, stderr) = run_safe_rm_with_config(
            &[nonexistent_file.to_str().unwrap()],
            &project_path,
            Some(config.path()),
        );

        assert_eq!(exit_code, 1, "Should fail for nonexistent file without -f");
        assert!(
            stderr.contains("No such file"),
            "Error should mention file not found: {}",
            stderr
        );
    }

    #[test]
    fn test_allowed_paths_directory_without_recursive() {
        let project_dir = create_test_repo();
        let project_path = project_dir.path().canonicalize().unwrap();

        // 許可されたパス内にディレクトリを作成
        let allowed_dir = TempDir::new().unwrap();
        let allowed_path = allowed_dir.path().canonicalize().unwrap();
        let subdir = allowed_path.join("subdir");
        fs::create_dir(&subdir).unwrap();

        let config = tempfile::NamedTempFile::new().unwrap();
        let config_content = format!(
            r#"
[[allowed_paths]]
path = "{}"
recursive = true
"#,
            allowed_path.display()
        );
        fs::write(config.path(), config_content).unwrap();

        // -r なしでディレクトリを削除 → 失敗
        let (exit_code, _, stderr) = run_safe_rm_with_config(
            &[subdir.to_str().unwrap()],
            &project_path,
            Some(config.path()),
        );

        assert_eq!(exit_code, 1, "Should fail for directory without -r");
        assert!(
            stderr.contains("Is a directory") || stderr.contains("-r"),
            "Error should mention directory requires -r: {}",
            stderr
        );
    }

    #[test]
    fn test_allowed_paths_strict_mode_bypasses_git_check() {
        let project_dir = create_test_repo();
        let project_path = project_dir.path().canonicalize().unwrap();

        // プロジェクト外にGitで追跡されていないファイルを作成
        let outside_dir = TempDir::new().unwrap();
        let outside_path = outside_dir.path().canonicalize().unwrap();
        let outside_file = outside_path.join("file.txt");
        fs::write(&outside_file, "content").unwrap();

        // strictモードでもallowed_pathsは許可される
        let config = tempfile::NamedTempFile::new().unwrap();
        let config_content = format!(
            r#"
allow_project_deletion = false

[[allowed_paths]]
path = "{}"
recursive = true
"#,
            outside_path.display()
        );
        fs::write(config.path(), config_content).unwrap();

        let (exit_code, stdout, stderr) = run_safe_rm_with_config(
            &[outside_file.to_str().unwrap()],
            &project_path,
            Some(config.path()),
        );

        assert_eq!(
            exit_code, 0,
            "allowed_paths should bypass both containment and Git checks in strict mode. stderr: {}",
            stderr
        );
        assert!(
            stdout.contains("allowed by config"),
            "Should show allowed by config: {}",
            stdout
        );
        assert!(!outside_file.exists(), "File should be deleted");
    }
}
