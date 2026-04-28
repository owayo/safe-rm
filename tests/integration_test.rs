//! safe-rm CLI の統合テスト
//!
//! 実際のファイルシステム操作で CLI の挙動を検証する。

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
    #[cfg(unix)]
    fn test_force_nonexistent_absolute_path_via_repo_symlink_alias() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let alias_root = TempDir::new().unwrap();
        let repo_alias = alias_root.path().join("repo-link");
        std::os::unix::fs::symlink(&repo_path, &repo_alias).unwrap();

        // 同一リポジトリの symlink alias 経由でも、-f なら未作成パスは成功すべき
        let nonexistent = repo_alias.join("missing.txt");
        let nonexistent_arg = nonexistent.to_string_lossy().to_string();
        let (exit_code, _, stderr) = run_safe_rm(&["-f", nonexistent_arg.as_str()], &repo_path);

        assert_eq!(
            exit_code, 0,
            "Force deletion for missing path via repo alias should succeed. stderr: {}",
            stderr
        );
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
    fn test_single_path_error_is_reported_once() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // 単一パスの失敗では、同じエラー本文を重複出力しない
        commit_file(&repo_path, "modified.txt", "original");
        fs::write(repo_path.join("modified.txt"), "modified content").unwrap();

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["modified.txt"], &repo_path, Some(config.path()));

        assert_eq!(exit_code, 2, "Modified file deletion should be blocked");
        assert_eq!(
            stderr.matches("safe-rm:").count(),
            1,
            "単一パス失敗時の stderr は 1 回だけ出力されるべき: {}",
            stderr
        );
        assert_eq!(
            stderr
                .matches("未コミットの変更があるファイルは削除できません。")
                .count(),
            1,
            "同じエラー本文が重複してはいけない: {}",
            stderr
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
    fn test_directory_with_ignored_and_untracked_file_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        commit_file(&repo_path, ".gitignore", "*.log\n");

        let subdir = repo_path.join("logs");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("app.log"), "ignored").unwrap();
        fs::write(subdir.join("untracked.txt"), "untracked").unwrap();

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["-r", "logs"], &repo_path, Some(config.path()));

        assert_eq!(
            exit_code, 2,
            "ignored ファイルが混在しても未追跡ファイルを含むディレクトリはブロックされるべき"
        );
        assert!(
            stderr.contains("Untracked") || stderr.contains("未追跡"),
            "未追跡ステータスのエラーが必要: {}",
            stderr
        );
        assert!(
            subdir.exists(),
            "未追跡ファイルを含むディレクトリは削除されてはならない"
        );
    }

    #[test]
    fn test_git_metadata_directory_blocked_in_strict_mode() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();
        commit_file(&repo_path, "tracked.txt", "tracked");

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["-r", ".git"], &repo_path, Some(config.path()));

        assert_eq!(exit_code, 2, ".git の削除は常にブロックされるべき");
        assert!(
            stderr.contains("Git 管理メタデータ"),
            "Git 管理メタデータの保護エラーが必要: {}",
            stderr
        );
        assert!(
            repo_path.join(".git").exists(),
            ".git ディレクトリは削除されてはならない"
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

    #[test]
    fn test_no_arguments_returns_error() {
        // 引数なしで実行した場合、clap が required=true のためエラー終了すること
        let temp_dir = TempDir::new().unwrap();
        let project_path = temp_dir.path();

        let (exit_code, _, stderr) = run_safe_rm(&[], project_path);

        assert_ne!(exit_code, 0, "引数なしの実行はエラーで終了すべき");
        assert!(
            !stderr.is_empty(),
            "エラーメッセージが出力されるべき: {}",
            stderr
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

    #[test]
    fn test_allowed_paths_recursive_directory_deletion() {
        let project_dir = create_test_repo();
        let project_path = project_dir.path().canonicalize().unwrap();

        // 許可ディレクトリとファイルを作成
        let allowed_dir = TempDir::new().unwrap();
        let allowed_path = allowed_dir.path().canonicalize().unwrap();
        let subdir = allowed_path.join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("file.txt"), "content").unwrap();

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

        // -r で許可パス内のディレクトリを削除
        let (exit_code, stdout, stderr) = run_safe_rm_with_config(
            &["-r", subdir.to_str().unwrap()],
            &project_path,
            Some(config.path()),
        );

        assert_eq!(
            exit_code, 0,
            "Directory in allowed_paths should be deletable with -r. stderr: {}",
            stderr
        );
        assert!(
            stdout.contains("allowed by config"),
            "Should show allowed by config: {}",
            stdout
        );
        assert!(!subdir.exists(), "Directory should be deleted");
    }
}

// =============================================================================
// init サブコマンドのテスト
// =============================================================================

mod init_tests {
    use super::*;

    #[test]
    fn test_init_creates_config_file() {
        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().join("safe-rm");
        let config_path = config_dir.join("config.toml");

        // SAFE_RM_CONFIG で一時パスを指定して init を実行
        let binary = get_binary_path();
        let output = Command::new(&binary)
            .args(["init"])
            .env("SAFE_RM_CONFIG", &config_path)
            .output()
            .expect("Failed to execute safe-rm init");

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert_eq!(exit_code, 0, "init should succeed");
        assert!(config_path.exists(), "Config file should be created");
        assert!(
            stdout.contains("Created config file"),
            "Should show creation message: {}",
            stdout
        );

        // 作成されたファイルが有効なTOMLであることを確認
        let content = fs::read_to_string(&config_path).unwrap();
        assert!(
            content.contains("allowed_paths"),
            "Config should contain allowed_paths"
        );
    }

    #[test]
    fn test_init_does_not_overwrite_existing() {
        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().join("safe-rm");
        fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.toml");

        // 既存のファイルを作成
        fs::write(&config_path, "# existing config\n").unwrap();

        let binary = get_binary_path();
        let output = Command::new(&binary)
            .args(["init"])
            .env("SAFE_RM_CONFIG", &config_path)
            .output()
            .expect("Failed to execute safe-rm init");

        let exit_code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(exit_code, 0, "init should succeed even with existing file");
        assert!(
            stderr.contains("already exists"),
            "Should warn about existing file: {}",
            stderr
        );

        // 内容が変わっていないことを確認
        let content = fs::read_to_string(&config_path).unwrap();
        assert_eq!(
            content, "# existing config\n",
            "Existing config should not be overwritten"
        );
    }
}

// =============================================================================
// allowed_paths の非再帰設定テスト
// =============================================================================

mod allowed_paths_non_recursive_tests {
    use super::*;

    #[test]
    fn test_non_recursive_allows_direct_child() {
        let project_dir = create_test_repo();
        let project_path = project_dir.path().canonicalize().unwrap();

        // 許可ディレクトリを作成
        let allowed_dir = TempDir::new().unwrap();
        let allowed_path = allowed_dir.path().canonicalize().unwrap();
        let direct_file = allowed_path.join("direct.txt");
        fs::write(&direct_file, "direct child").unwrap();

        // 非再帰設定
        let config = tempfile::NamedTempFile::new().unwrap();
        let config_content = format!(
            r#"
[[allowed_paths]]
path = "{}"
recursive = false
"#,
            allowed_path.display()
        );
        fs::write(config.path(), config_content).unwrap();

        // 直接の子ファイルは削除可能
        let (exit_code, stdout, stderr) = run_safe_rm_with_config(
            &[direct_file.to_str().unwrap()],
            &project_path,
            Some(config.path()),
        );

        assert_eq!(
            exit_code, 0,
            "Direct child should be deletable with non-recursive. stderr: {}",
            stderr
        );
        assert!(
            stdout.contains("allowed by config"),
            "Should show allowed by config: {}",
            stdout
        );
        assert!(!direct_file.exists(), "File should be deleted");
    }

    #[test]
    fn test_non_recursive_blocks_nested_child() {
        let project_dir = create_test_repo();
        let project_path = project_dir.path().canonicalize().unwrap();

        // 許可ディレクトリとネストされたファイルを作成
        let allowed_dir = TempDir::new().unwrap();
        let allowed_path = allowed_dir.path().canonicalize().unwrap();
        let nested_dir = allowed_path.join("sub");
        fs::create_dir(&nested_dir).unwrap();
        let nested_file = nested_dir.join("nested.txt");
        fs::write(&nested_file, "nested child").unwrap();

        // 非再帰設定
        let config = tempfile::NamedTempFile::new().unwrap();
        let config_content = format!(
            r#"
[[allowed_paths]]
path = "{}"
recursive = false
"#,
            allowed_path.display()
        );
        fs::write(config.path(), config_content).unwrap();

        // ネストされたファイルはブロックされる（プロジェクト外のため）
        let (exit_code, _, _) = run_safe_rm_with_config(
            &[nested_file.to_str().unwrap()],
            &project_path,
            Some(config.path()),
        );

        assert_eq!(
            exit_code, 2,
            "Nested child should be blocked with non-recursive"
        );
        assert!(nested_file.exists(), "Nested file should NOT be deleted");
    }
}

// =============================================================================
// dry-run モードの追加テスト
// =============================================================================

mod dry_run_tests {
    use super::*;

    #[test]
    fn test_dry_run_with_directory() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ディレクトリを作成
        let subdir = repo_path.join("drydir");
        fs::create_dir(&subdir).unwrap();
        commit_file(&repo_path, "drydir/file.txt", "content");

        let (exit_code, stdout, _) = run_safe_rm(&["-rn", "drydir"], &repo_path);

        assert_eq!(exit_code, 0, "Dry run with directory should succeed");
        assert!(
            stdout.contains("would remove:"),
            "Should show would remove: {}",
            stdout
        );
        assert!(
            repo_path.join("drydir").exists(),
            "Directory should NOT be deleted in dry run"
        );
    }

    #[test]
    fn test_dry_run_with_allowed_paths() {
        let project_dir = create_test_repo();
        let project_path = project_dir.path().canonicalize().unwrap();

        // 許可ディレクトリを作成
        let allowed_dir = TempDir::new().unwrap();
        let allowed_path = allowed_dir.path().canonicalize().unwrap();
        let file = allowed_path.join("test.txt");
        fs::write(&file, "content").unwrap();

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

        let (exit_code, stdout, _) = run_safe_rm_with_config(
            &["-n", file.to_str().unwrap()],
            &project_path,
            Some(config.path()),
        );

        assert_eq!(exit_code, 0, "Dry run with allowed paths should succeed");
        assert!(
            stdout.contains("would remove:") && stdout.contains("allowed by config"),
            "Should show would remove with config annotation: {}",
            stdout
        );
        assert!(file.exists(), "File should NOT be deleted in dry run");
    }

    #[test]
    fn test_dry_run_with_multiple_files() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "dry1.txt", "content1");
        commit_file(&repo_path, "dry2.txt", "content2");

        let (exit_code, stdout, _) = run_safe_rm(&["-n", "dry1.txt", "dry2.txt"], &repo_path);

        assert_eq!(exit_code, 0, "Dry run with multiple files should succeed");
        assert!(stdout.contains("dry1.txt"), "Should mention dry1.txt");
        assert!(stdout.contains("dry2.txt"), "Should mention dry2.txt");
        assert!(
            repo_path.join("dry1.txt").exists(),
            "dry1.txt should NOT be deleted"
        );
        assert!(
            repo_path.join("dry2.txt").exists(),
            "dry2.txt should NOT be deleted"
        );
    }

    #[test]
    fn test_dry_run_strict_mode_dirty_file_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        // 変更済みファイルを用意
        commit_file(&repo_path, "dirty.txt", "original");
        fs::write(repo_path.join("dirty.txt"), "modified").unwrap();

        // --dry-run でも strict モードの dirty ファイルはブロック
        let (exit_code, stdout, stderr) =
            run_safe_rm_with_config(&["-n", "dirty.txt"], &repo_path, Some(config.path()));

        assert_eq!(
            exit_code, 2,
            "Dry-run with dirty file in strict mode should return exit 2. stderr: {}",
            stderr
        );
        assert!(
            !stdout.contains("would remove"),
            "Should NOT show 'would remove' for blocked file"
        );
        assert!(
            repo_path.join("dirty.txt").exists(),
            "File should NOT be deleted"
        );
    }

    #[test]
    fn test_dry_run_strict_mode_clean_file_shows_would_remove() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        commit_file(&repo_path, "clean.txt", "content");

        let (exit_code, stdout, _) =
            run_safe_rm_with_config(&["-n", "clean.txt"], &repo_path, Some(config.path()));

        assert_eq!(
            exit_code, 0,
            "Dry-run with clean file in strict mode should succeed"
        );
        assert!(
            stdout.contains("would remove:"),
            "Should show 'would remove' for clean file"
        );
        assert!(
            repo_path.join("clean.txt").exists(),
            "File should NOT be deleted in dry-run"
        );
    }
}

// =============================================================================
// シンボリックリンクのテスト
// =============================================================================

#[cfg(unix)]
mod symlink_tests {
    use super::*;

    #[test]
    fn test_symlink_inside_project_deletable() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ターゲットファイルを作成してコミット
        commit_file(&repo_path, "target.txt", "target content");

        // シンボリックリンクを作成
        let link_path = repo_path.join("link.txt");
        std::os::unix::fs::symlink(repo_path.join("target.txt"), &link_path).unwrap();

        // git add してコミット
        Command::new("git")
            .args(["add", "link.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add symlink"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let (exit_code, stdout, stderr) = run_safe_rm(&["link.txt"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "Symlink inside project should be deletable. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(!link_path.exists(), "Symlink should be deleted");
        // ターゲットはまだ存在する
        assert!(
            repo_path.join("target.txt").exists(),
            "Target should still exist"
        );
    }

    #[test]
    fn test_symlink_pointing_outside_project_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 初期コミット
        commit_file(&repo_path, "init.txt", "init");

        // プロジェクト外にファイルを作成
        let outside_dir = TempDir::new().unwrap();
        let outside_file = outside_dir.path().join("outside.txt");
        fs::write(&outside_file, "outside content").unwrap();

        // プロジェクト外を指すシンボリックリンクを作成
        let link_path = repo_path.join("evil_link.txt");
        std::os::unix::fs::symlink(&outside_file, &link_path).unwrap();

        let (exit_code, _, stderr) = run_safe_rm(&["evil_link.txt"], &repo_path);

        assert_eq!(
            exit_code, 2,
            "Symlink pointing outside should be blocked. stderr: {}",
            stderr
        );
        assert!(
            link_path.symlink_metadata().is_ok(),
            "Symlink should NOT be deleted"
        );
    }

    #[test]
    fn test_strict_mode_directory_symlink_does_not_traverse_target() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // strict モード設定
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        // ターゲットとなるディレクトリを作成してコミット
        commit_file(&repo_path, "restricted/file.txt", "content");

        // ディレクトリ symlink を作成してコミット
        let link_path = repo_path.join("dir_link");
        std::os::unix::fs::symlink("restricted", &link_path).unwrap();
        Command::new("git")
            .args(["add", "dir_link"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add directory symlink"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // リンク先を読めない状態にして、辿る実装だと失敗する状況を作る
        let restricted_dir = repo_path.join("restricted");
        let mut locked_perms = fs::metadata(&restricted_dir).unwrap().permissions();
        locked_perms.set_mode(0o000);
        fs::set_permissions(&restricted_dir, locked_perms).unwrap();

        let (exit_code, stdout, stderr) =
            run_safe_rm_with_config(&["dir_link"], &repo_path, Some(config.path()));

        // テンポラリ削除のため権限を戻す
        let mut restore_perms = fs::metadata(&restricted_dir).unwrap().permissions();
        restore_perms.set_mode(0o755);
        fs::set_permissions(&restricted_dir, restore_perms).unwrap();

        assert_eq!(
            exit_code, 0,
            "Directory symlink should be deletable in strict mode without traversing target. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(!link_path.exists(), "Symlink should be deleted");
        assert!(
            restricted_dir.exists(),
            "Target directory should remain untouched"
        );
    }

    #[test]
    fn test_strict_mode_absolute_path_via_repo_symlink_alias_blocks_dirty_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // strict モード設定
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        // 変更済みファイルを用意
        commit_file(&repo_path, "dirty.txt", "original");
        fs::write(repo_path.join("dirty.txt"), "modified").unwrap();

        // リポジトリへの別名 symlink を作成（絶対パス引数で利用）
        let alias_dir = TempDir::new().unwrap();
        let alias_repo = alias_dir.path().join("repo-link");
        std::os::unix::fs::symlink(&repo_path, &alias_repo).unwrap();
        let alias_dirty_path = alias_repo.join("dirty.txt");

        let (exit_code, _, stderr) = run_safe_rm_with_config(
            &[alias_dirty_path.to_str().unwrap()],
            &repo_path,
            Some(config.path()),
        );

        assert_eq!(
            exit_code, 2,
            "Dirty file accessed via symlink alias path must be blocked in strict mode. stderr: {}",
            stderr
        );
        assert!(
            stderr.contains("Status: Modified"),
            "Error should report dirty status. stderr: {}",
            stderr
        );
        assert!(
            repo_path.join("dirty.txt").exists(),
            "Dirty target file should remain"
        );
    }

    #[test]
    fn test_strict_mode_untracked_symlink_via_repo_alias_is_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // strict モード設定
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        // 初期コミットと symlink ターゲットを用意
        commit_file(&repo_path, "init.txt", "init");
        commit_file(&repo_path, "target.txt", "target");

        // 未追跡 symlink（strict モードではブロック対象）
        let link_path = repo_path.join("untracked_link");
        std::os::unix::fs::symlink("target.txt", &link_path).unwrap();

        // リポジトリ別名 symlink 経由で絶対パス指定
        let alias_dir = TempDir::new().unwrap();
        let alias_repo = alias_dir.path().join("repo-link");
        std::os::unix::fs::symlink(&repo_path, &alias_repo).unwrap();
        let alias_link_path = alias_repo.join("untracked_link");

        let (exit_code, _, stderr) = run_safe_rm_with_config(
            &[alias_link_path.to_str().unwrap()],
            &repo_path,
            Some(config.path()),
        );

        assert_eq!(
            exit_code, 2,
            "Untracked symlink via repo alias must be blocked in strict mode. stderr: {}",
            stderr
        );
        assert!(
            stderr.contains("Untracked") || stderr.contains("未追跡"),
            "Error should report untracked status. stderr: {}",
            stderr
        );
        assert!(
            link_path.symlink_metadata().is_ok(),
            "Untracked symlink should NOT be deleted"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_strict_mode_clean_file_deletable_when_cwd_is_repo_symlink_alias() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // strict モード設定
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        // Clean ファイルを用意
        commit_file(&repo_path, "clean.txt", "clean");

        // リポジトリの別名 symlink を作成し、その配下を cwd として実行
        let alias_dir = TempDir::new().unwrap();
        let alias_repo = alias_dir.path().join("repo-link");
        std::os::unix::fs::symlink(&repo_path, &alias_repo).unwrap();

        let (exit_code, stdout, stderr) =
            run_safe_rm_with_config(&["clean.txt"], &alias_repo, Some(config.path()));

        assert_eq!(
            exit_code, 0,
            "Clean file must remain deletable in strict mode when cwd is a repo symlink alias. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("clean.txt").exists(),
            "Clean target file should be deleted"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_strict_mode_dirty_file_blocked_when_cwd_is_repo_symlink_alias() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // strict モード設定
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        // 変更済みファイルを用意
        commit_file(&repo_path, "dirty.txt", "original");
        fs::write(repo_path.join("dirty.txt"), "modified").unwrap();

        // リポジトリの別名 symlink を作成し、その配下を cwd として実行
        let alias_dir = TempDir::new().unwrap();
        let alias_repo = alias_dir.path().join("repo-link");
        std::os::unix::fs::symlink(&repo_path, &alias_repo).unwrap();

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["dirty.txt"], &alias_repo, Some(config.path()));

        assert_eq!(
            exit_code, 2,
            "Dirty file must be blocked in strict mode even when cwd is a repo symlink alias. stderr: {}",
            stderr
        );
        assert!(
            stderr.contains("Status: Modified"),
            "Error should report modified status. stderr: {}",
            stderr
        );
        assert!(
            repo_path.join("dirty.txt").exists(),
            "Dirty target file should remain"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_strict_mode_symlink_to_clean_directory_with_recursive() {
        // strict モードで、clean なディレクトリを指すシンボリックリンクを
        // -r フラグ付きで削除した場合、リンク自体が削除されること
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // strict モード設定
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        // ターゲットディレクトリとその中のファイルを作成してコミット
        commit_file(&repo_path, "target_dir/file1.txt", "content1");
        commit_file(&repo_path, "target_dir/file2.txt", "content2");

        // ディレクトリを指すシンボリックリンクを作成してコミット
        let link_path = repo_path.join("dir_link");
        std::os::unix::fs::symlink("target_dir", &link_path).unwrap();
        Command::new("git")
            .args(["add", "dir_link"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add directory symlink"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // -r フラグ付きでシンボリックリンクを削除
        let (exit_code, stdout, stderr) =
            run_safe_rm_with_config(&["-r", "dir_link"], &repo_path, Some(config.path()));

        assert_eq!(
            exit_code, 0,
            "clean なディレクトリを指す symlink は -r 付きで削除可能であるべき. stderr: {}",
            stderr
        );
        assert!(
            stdout.contains("removed:"),
            "削除メッセージが出力されるべき"
        );
        assert!(
            !link_path.exists(),
            "シンボリックリンク自体が削除されるべき"
        );
        // ターゲットディレクトリは残っていること
        assert!(
            repo_path.join("target_dir").exists(),
            "ターゲットディレクトリは残っているべき"
        );
        assert!(
            repo_path.join("target_dir/file1.txt").exists(),
            "ターゲット内のファイルは残っているべき"
        );
    }
}

// =============================================================================
// バッチ処理の追加テスト
// =============================================================================

mod batch_tests {
    use super::*;

    /// allow_project_deletion = false の設定ファイルを作成
    fn create_strict_config() -> tempfile::NamedTempFile {
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();
        config
    }

    #[test]
    fn test_security_error_takes_precedence_over_operation_error() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 初期コミット
        commit_file(&repo_path, "init.txt", "init");

        // 存在しないファイル（exit 1）と プロジェクト外ファイル（exit 2）
        let (exit_code, _, _) = run_safe_rm(&["nonexistent.txt", "/etc/passwd"], &repo_path);

        assert_eq!(
            exit_code, 2,
            "Security error (exit 2) should take precedence over operation error (exit 1)"
        );
    }

    #[test]
    fn test_multiple_clean_files_in_strict_mode() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        commit_file(&repo_path, "a.txt", "a");
        commit_file(&repo_path, "b.txt", "b");
        commit_file(&repo_path, "c.txt", "c");

        let (exit_code, stdout, stderr) = run_safe_rm_with_config(
            &["a.txt", "b.txt", "c.txt"],
            &repo_path,
            Some(config.path()),
        );

        assert_eq!(
            exit_code, 0,
            "All clean files should be deletable in strict mode. stderr: {}",
            stderr
        );
        assert!(stdout.contains("a.txt"), "Should mention a.txt");
        assert!(stdout.contains("b.txt"), "Should mention b.txt");
        assert!(stdout.contains("c.txt"), "Should mention c.txt");
        assert!(!repo_path.join("a.txt").exists(), "a.txt should be deleted");
        assert!(!repo_path.join("b.txt").exists(), "b.txt should be deleted");
        assert!(!repo_path.join("c.txt").exists(), "c.txt should be deleted");
    }

    #[test]
    fn test_mix_of_clean_and_dirty_in_strict_mode() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // clean ファイルを作成
        commit_file(&repo_path, "clean1.txt", "clean1");
        commit_file(&repo_path, "clean2.txt", "clean2");

        // dirty ファイルを作成（modified）
        commit_file(&repo_path, "dirty.txt", "original");
        fs::write(repo_path.join("dirty.txt"), "modified").unwrap();

        let (exit_code, stdout, stderr) = run_safe_rm_with_config(
            &["clean1.txt", "dirty.txt", "clean2.txt"],
            &repo_path,
            Some(config.path()),
        );

        // 終了コード 2 を返す（セキュリティブロックを優先）
        assert_eq!(exit_code, 2, "Should exit with 2 due to dirty file");
        // clean ファイルは削除済みであるべき
        assert!(
            !repo_path.join("clean1.txt").exists(),
            "clean1.txt should be deleted"
        );
        assert!(
            !repo_path.join("clean2.txt").exists(),
            "clean2.txt should be deleted"
        );
        // dirty ファイルは削除されてはいけない
        assert!(
            repo_path.join("dirty.txt").exists(),
            "dirty.txt should NOT be deleted"
        );
        assert!(
            stderr.contains("dirty.txt"),
            "Error should mention dirty.txt"
        );
        assert!(
            stdout.contains("clean1.txt"),
            "Should mention clean1.txt removed"
        );
    }

    #[test]
    fn test_batch_partial_success_with_strict_mode() {
        // allow_project_deletion=false で複数パスをバッチ処理した際、
        // clean ファイルは削除され、dirty ファイルはブロックされること。
        // dirty ファイルのブロックはセキュリティエラー（終了コード 2）となるため、
        // 最終的な終了コードは 2 が返る（セキュリティブロック優先）。
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // clean ファイルを3つ作成
        commit_file(&repo_path, "ok1.txt", "ok1");
        commit_file(&repo_path, "ok2.txt", "ok2");
        commit_file(&repo_path, "ok3.txt", "ok3");

        // dirty ファイルを2つ作成（1つは modified、1つは untracked）
        commit_file(&repo_path, "modified.txt", "original");
        fs::write(repo_path.join("modified.txt"), "changed").unwrap();
        fs::write(repo_path.join("untracked.txt"), "new file").unwrap();

        let (exit_code, stdout, stderr) = run_safe_rm_with_config(
            &[
                "ok1.txt",
                "modified.txt",
                "ok2.txt",
                "untracked.txt",
                "ok3.txt",
            ],
            &repo_path,
            Some(config.path()),
        );

        // dirty ファイルがあるためセキュリティブロック（終了コード 2）
        assert_eq!(
            exit_code, 2,
            "dirty ファイルを含むバッチはセキュリティブロック（exit 2）で終了すべき. stderr: {}",
            stderr
        );

        // clean ファイルは削除されていること
        assert!(
            !repo_path.join("ok1.txt").exists(),
            "ok1.txt は削除されるべき"
        );
        assert!(
            !repo_path.join("ok2.txt").exists(),
            "ok2.txt は削除されるべき"
        );
        assert!(
            !repo_path.join("ok3.txt").exists(),
            "ok3.txt は削除されるべき"
        );

        // dirty ファイルはブロックされて残っていること
        assert!(
            repo_path.join("modified.txt").exists(),
            "modified.txt はブロックされて残るべき"
        );
        assert!(
            repo_path.join("untracked.txt").exists(),
            "untracked.txt はブロックされて残るべき"
        );

        // stderr に dirty ファイルのエラーが出力されていること
        assert!(
            stderr.contains("modified.txt"),
            "stderr に modified.txt のエラーが含まれるべき: {}",
            stderr
        );
        assert!(
            stderr.contains("untracked.txt"),
            "stderr に untracked.txt のエラーが含まれるべき: {}",
            stderr
        );

        // stdout に clean ファイルの削除メッセージが出力されていること
        assert!(
            stdout.contains("ok1.txt"),
            "stdout に ok1.txt の削除メッセージが含まれるべき"
        );
        assert!(
            stdout.contains("ok2.txt"),
            "stdout に ok2.txt の削除メッセージが含まれるべき"
        );
        assert!(
            stdout.contains("ok3.txt"),
            "stdout に ok3.txt の削除メッセージが含まれるべき"
        );
    }
}

// =============================================================================
// 部分失敗のテスト
// =============================================================================

mod partial_failure_tests {
    use super::*;

    #[test]
    fn test_partial_failure_all_not_found_returns_exit_1() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // リポジトリを有効化するため初期コミットを作成
        commit_file(&repo_path, "init.txt", "init");

        // 存在しない複数ファイルを -f なしで削除すると、すべて exit 1 で失敗する
        let (exit_code, _, stderr) = run_safe_rm(
            &["missing1.txt", "missing2.txt", "missing3.txt"],
            &repo_path,
        );

        assert_eq!(
            exit_code, 1,
            "All NotFound errors should result in exit code 1. stderr: {}",
            stderr
        );
        assert!(
            stderr.contains("file(s) removed"),
            "Should show PartialFailure message. stderr: {}",
            stderr
        );
    }

    #[test]
    fn test_partial_failure_mix_success_and_not_found() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 片方は存在して Clean
        commit_file(&repo_path, "exists.txt", "content");

        // 片方だけ存在する状態で実行
        let (exit_code, stdout, stderr) = run_safe_rm(&["exists.txt", "missing.txt"], &repo_path);

        assert_eq!(
            exit_code, 1,
            "Mix of success and NotFound should return exit 1. stderr: {}",
            stderr
        );
        assert!(
            stdout.contains("removed:"),
            "Should show success for existing file"
        );
        assert!(
            stderr.contains("No such file"),
            "Should report missing file. stderr: {}",
            stderr
        );
    }
}

// =============================================================================
// 未コミット symlink のテスト（厳格モード）
// =============================================================================

#[cfg(unix)]
mod dirty_symlink_tests {
    use super::*;

    #[test]
    fn test_strict_mode_untracked_symlink_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        // 初期コミット
        commit_file(&repo_path, "init.txt", "init");

        // ターゲットファイルを作成してコミット
        commit_file(&repo_path, "target.txt", "target");

        // 未追跡の symlink を作成（未コミット）
        let link_path = repo_path.join("untracked_link");
        std::os::unix::fs::symlink("target.txt", &link_path).unwrap();

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["untracked_link"], &repo_path, Some(config.path()));

        assert_eq!(
            exit_code, 2,
            "Untracked symlink should be blocked in strict mode. stderr: {}",
            stderr
        );
        assert!(
            link_path.symlink_metadata().is_ok(),
            "Untracked symlink should NOT be deleted"
        );
    }

    #[test]
    fn test_strict_mode_clean_symlink_allowed() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        // ターゲットと symlink を作成し、両方コミットする
        commit_file(&repo_path, "target.txt", "target content");
        let link_path = repo_path.join("clean_link");
        std::os::unix::fs::symlink("target.txt", &link_path).unwrap();
        Command::new("git")
            .args(["add", "clean_link"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add symlink"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let (exit_code, stdout, stderr) =
            run_safe_rm_with_config(&["clean_link"], &repo_path, Some(config.path()));

        assert_eq!(
            exit_code, 0,
            "Clean committed symlink should be deletable. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            link_path.symlink_metadata().is_err(),
            "Clean symlink should be deleted"
        );
        assert!(
            repo_path.join("target.txt").exists(),
            "Target file should remain"
        );
    }
}

// =============================================================================
// 特殊ファイル名のテスト
// =============================================================================

mod special_filename_tests {
    use super::*;

    #[test]
    fn test_file_with_spaces_in_name() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "file with spaces.txt", "content");

        let (exit_code, stdout, stderr) = run_safe_rm(&["file with spaces.txt"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "File with spaces should be deletable. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("file with spaces.txt").exists(),
            "File should be deleted"
        );
    }

    #[test]
    fn test_file_with_unicode_name() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "日本語ファイル.txt", "内容");

        let (exit_code, stdout, stderr) = run_safe_rm(&["日本語ファイル.txt"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "File with unicode name should be deletable. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join("日本語ファイル.txt").exists(),
            "File should be deleted"
        );
    }

    #[test]
    fn test_hidden_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, ".hidden_file", "hidden content");

        let (exit_code, stdout, stderr) = run_safe_rm(&[".hidden_file"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "Hidden file should be deletable. stderr: {}",
            stderr
        );
        assert!(stdout.contains("removed:"), "Should show removed message");
        assert!(
            !repo_path.join(".hidden_file").exists(),
            "Hidden file should be deleted"
        );
    }
}

// =============================================================================
// 厳格モードでのネストされた未追跡ファイルのテスト
// =============================================================================

mod strict_mode_nested_tests {
    use super::*;

    /// allow_project_deletion = false の設定ファイルを作成
    fn create_strict_config() -> tempfile::NamedTempFile {
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();
        config
    }

    #[test]
    fn test_strict_mode_blocks_directory_with_nested_untracked_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // 2階層深いディレクトリにコミット済みファイルを作成
        commit_file(&repo_path, "parent/child/committed.txt", "committed");

        // 2階層深い場所に未追跡ファイルを追加
        fs::write(
            repo_path.join("parent/child/untracked_nested.txt"),
            "untracked nested content",
        )
        .unwrap();

        // 親ディレクトリの再帰削除を試みる → 未追跡ファイルがあるためブロックされるべき
        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["-r", "parent"], &repo_path, Some(config.path()));

        assert_eq!(
            exit_code, 2,
            "Directory with nested untracked file should be blocked. stderr: {}",
            stderr
        );
        assert!(!stderr.is_empty(), "エラーメッセージが出力されるべき");
        assert!(
            repo_path.join("parent").exists(),
            "ディレクトリは削除されていないべき"
        );
        assert!(
            repo_path.join("parent/child/untracked_nested.txt").exists(),
            "ネストされた未追跡ファイルは残っているべき"
        );
    }
}

// =============================================================================
// -f フラグのテスト
// =============================================================================

mod force_flag_tests {
    use super::*;

    /// allow_project_deletion = false の設定ファイルを作成
    fn create_strict_config() -> tempfile::NamedTempFile {
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();
        config
    }

    #[test]
    fn test_force_flag_ignores_nonexistent_multiple_files() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 初期コミット（空リポジトリ回避）
        commit_file(&repo_path, "init.txt", "init");

        // 複数の存在しないファイルに -f フラグ → すべて無視されて正常終了すべき
        let (exit_code, _, stderr) = run_safe_rm(
            &[
                "-f",
                "nonexistent1.txt",
                "nonexistent2.txt",
                "nonexistent3.txt",
            ],
            &repo_path,
        );

        assert_eq!(
            exit_code, 0,
            "-f フラグで複数の存在しないファイルは無視されるべき. stderr: {}",
            stderr
        );
    }

    #[test]
    fn test_force_flag_mixed_nonexistent_and_dirty() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // ファイルをコミット後に変更（dirty状態にする）
        commit_file(&repo_path, "dirty.txt", "original");
        fs::write(repo_path.join("dirty.txt"), "modified content").unwrap();

        // -f で存在しないファイルとdirtyファイルを混在させる
        // → 存在しないファイルは無視されるが、dirtyファイルはブロックされるべき
        let (exit_code, _, stderr) = run_safe_rm_with_config(
            &["-f", "nonexistent.txt", "dirty.txt"],
            &repo_path,
            Some(config.path()),
        );

        assert_eq!(
            exit_code, 2,
            "-f フラグでもdirtyファイルはブロックされるべき. stderr: {}",
            stderr
        );
        assert!(
            repo_path.join("dirty.txt").exists(),
            "dirtyファイルは削除されていないべき"
        );
    }
}

// =============================================================================
// ドライラン + フォースフラグの複合テスト
// =============================================================================

mod dry_run_force_tests {
    use super::*;

    #[test]
    fn test_dry_run_force_nonexistent_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "init.txt", "init");

        // -n -f で存在しないファイル → エラーなしで正常終了すべき
        let (exit_code, stdout, _) = run_safe_rm(&["-n", "-f", "nonexistent.txt"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "ドライラン + フォースで非存在ファイルは正常終了すべき"
        );
        // 存在しないファイルは出力されない（フォースで無視）
        assert!(
            !stdout.contains("nonexistent.txt"),
            "フォースで無視されたファイルは出力されないべき"
        );
    }

    #[test]
    fn test_dry_run_force_mixed_existing_and_nonexistent() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "exists.txt", "content");

        let (exit_code, stdout, _) =
            run_safe_rm(&["-n", "-f", "exists.txt", "missing.txt"], &repo_path);

        assert_eq!(exit_code, 0, "ドライラン + フォースは正常終了すべき");
        assert!(
            stdout.contains("would remove: exists.txt"),
            "存在するファイルは 'would remove' と表示されるべき"
        );
        assert!(
            repo_path.join("exists.txt").exists(),
            "ドライランなのでファイルは残っているべき"
        );
    }
}

// =============================================================================
// 空ディレクトリのテスト
// =============================================================================

mod empty_directory_tests {
    use super::*;

    #[test]
    fn test_empty_directory_without_recursive_flag() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "init.txt", "init");

        // 空ディレクトリを作成
        let empty_dir = repo_path.join("empty_dir");
        fs::create_dir(&empty_dir).unwrap();

        // -r なしでディレクトリを削除 → エラーになるべき
        let (exit_code, _, stderr) = run_safe_rm(&["empty_dir"], &repo_path);

        assert_eq!(exit_code, 1, "ディレクトリは -r なしで削除できないべき");
        assert!(
            stderr.contains("Is a directory"),
            "stderr に 'Is a directory' が含まれるべき. stderr: {}",
            stderr
        );
        assert!(empty_dir.exists(), "ディレクトリは残っているべき");
    }

    #[test]
    fn test_empty_committed_directory_with_recursive_flag() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ディレクトリ内にファイルを作成してコミット後、ファイルを削除してコミット
        // （空ディレクトリが残る状態）
        commit_file(&repo_path, "dir_to_empty/placeholder.txt", "placeholder");
        fs::remove_file(repo_path.join("dir_to_empty/placeholder.txt")).unwrap();
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Remove placeholder"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let empty_dir = repo_path.join("dir_to_empty");
        // Git はディレクトリを追跡しないので、コミット後にディレクトリが残っている場合のみテスト
        if empty_dir.exists() {
            let (exit_code, stdout, _) = run_safe_rm(&["-r", "dir_to_empty"], &repo_path);
            assert_eq!(exit_code, 0, "空ディレクトリは -r で削除できるべき");
            assert!(
                stdout.contains("removed:"),
                "削除メッセージが出力されるべき"
            );
            assert!(!empty_dir.exists(), "ディレクトリが削除されているべき");
        }
    }
}

// =============================================================================
// デフォルトモードでのフォースフラグと外部パスのテスト
// =============================================================================

mod default_mode_security_tests {
    use super::*;

    #[test]
    fn test_default_mode_force_still_blocks_outside_project() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "init.txt", "init");

        // 外部の一時ファイルを作成
        let outside_dir = tempfile::tempdir().unwrap();
        let outside_file = outside_dir.path().join("secret.txt");
        fs::write(&outside_file, "secret").unwrap();

        // -f フラグでもプロジェクト外は削除不可
        let (exit_code, _, stderr) =
            run_safe_rm(&["-f", outside_file.to_str().unwrap()], &repo_path);

        assert_eq!(
            exit_code, 2,
            "フォースフラグでもプロジェクト外はブロックされるべき. stderr: {}",
            stderr
        );
        assert!(
            outside_file.exists(),
            "プロジェクト外のファイルは削除されていないべき"
        );
    }

    #[test]
    fn test_default_mode_recursive_force_still_blocks_outside_project() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "init.txt", "init");

        let outside_dir = tempfile::tempdir().unwrap();
        let outside_subdir = outside_dir.path().join("subdir");
        fs::create_dir(&outside_subdir).unwrap();
        fs::write(outside_subdir.join("file.txt"), "content").unwrap();

        // -rf でもプロジェクト外は削除不可
        let (exit_code, _, _) = run_safe_rm(&["-rf", outside_subdir.to_str().unwrap()], &repo_path);

        assert_eq!(
            exit_code, 2,
            "-rf でもプロジェクト外ディレクトリはブロックされるべき"
        );
        assert!(
            outside_subdir.exists(),
            "プロジェクト外のディレクトリは削除されていないべき"
        );
    }

    #[test]
    fn test_default_mode_still_blocks_git_metadata_directory() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "tracked.txt", "tracked");

        let (exit_code, _, stderr) = run_safe_rm(&["-r", ".git"], &repo_path);

        assert_eq!(
            exit_code, 2,
            "デフォルトモードでも .git の削除はブロックされるべき"
        );
        assert!(
            stderr.contains("Git 管理メタデータ"),
            "Git 管理メタデータの保護エラーが必要: {}",
            stderr
        );
        assert!(
            repo_path.join(".git").exists(),
            ".git ディレクトリは削除されてはならない"
        );
    }

    #[test]
    fn test_default_mode_blocks_project_root_recursive_deletion() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "tracked.txt", "tracked");

        let (exit_code, _, stderr) = run_safe_rm(&["-r", "."], &repo_path);

        assert_eq!(
            exit_code, 2,
            "リポジトリルートの再帰削除は .git を含むためブロックされるべき"
        );
        assert!(
            stderr.contains("Git 管理メタデータ"),
            "Git 管理メタデータの保護エラーが必要: {}",
            stderr
        );
        assert!(repo_path.exists(), "リポジトリルートは残っているべき");
        assert!(
            repo_path.join(".git").exists(),
            ".git ディレクトリは削除されてはならない"
        );
        assert!(
            repo_path.join("tracked.txt").exists(),
            "作業ツリーファイルも削除されてはならない"
        );
    }
}

// =============================================================================
// Strict モードでの Git 削除済みファイルのテスト
// =============================================================================

mod strict_mode_deleted_file_tests {
    use super::*;

    /// allow_project_deletion = false の設定ファイルを作成
    fn create_strict_config() -> tempfile::NamedTempFile {
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();
        config
    }

    #[test]
    fn test_strict_mode_git_deleted_file_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // ファイルを作成してコミット
        commit_file(&repo_path, "to_delete.txt", "content");

        // git rm でステージング（ファイルはワークツリーから削除済み）
        Command::new("git")
            .args(["rm", "to_delete.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // ファイルは既に存在しないので -f で NotFound をスキップ
        let (exit_code, _, _) =
            run_safe_rm_with_config(&["-f", "to_delete.txt"], &repo_path, Some(config.path()));

        // ファイルが存在しないため force で無視 → 正常終了
        assert_eq!(exit_code, 0, "存在しないファイルは force で無視されるべき");
    }

    #[test]
    fn test_strict_mode_staged_new_file_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let config = create_strict_config();

        // 初期コミット
        commit_file(&repo_path, "init.txt", "init");

        // 新規ファイルを作成して git add（Staged 状態）
        let staged_file = repo_path.join("staged_new.txt");
        fs::write(&staged_file, "new content").unwrap();
        Command::new("git")
            .args(["add", "staged_new.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["staged_new.txt"], &repo_path, Some(config.path()));

        assert_eq!(
            exit_code, 2,
            "ステージングされた新規ファイルはブロックされるべき. stderr: {}",
            stderr
        );
        assert!(
            staged_file.exists(),
            "ステージングされたファイルは削除されていないべき"
        );
    }
}

// =============================================================================
// バッチ操作の全セキュリティエラーテスト
// =============================================================================

mod batch_security_tests {
    use super::*;

    #[test]
    fn test_batch_all_outside_project_returns_exit_2() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "init.txt", "init");

        let outside_dir = tempfile::tempdir().unwrap();
        let file1 = outside_dir.path().join("a.txt");
        let file2 = outside_dir.path().join("b.txt");
        fs::write(&file1, "a").unwrap();
        fs::write(&file2, "b").unwrap();

        let (exit_code, _, _) = run_safe_rm(
            &[file1.to_str().unwrap(), file2.to_str().unwrap()],
            &repo_path,
        );

        assert_eq!(
            exit_code, 2,
            "すべてプロジェクト外のファイルはセキュリティエラー（exit 2）を返すべき"
        );
        assert!(
            file1.exists(),
            "プロジェクト外のファイルは削除されていないべき"
        );
        assert!(
            file2.exists(),
            "プロジェクト外のファイルは削除されていないべき"
        );
    }

    #[test]
    fn test_batch_mix_outside_and_clean_prioritizes_security() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "clean.txt", "content");

        let outside_dir = tempfile::tempdir().unwrap();
        let outside_file = outside_dir.path().join("outside.txt");
        fs::write(&outside_file, "outside").unwrap();

        let (exit_code, _, _) =
            run_safe_rm(&["clean.txt", outside_file.to_str().unwrap()], &repo_path);

        // セキュリティエラー（exit 2）が操作エラーより優先
        assert_eq!(exit_code, 2, "セキュリティエラーが優先されるべき");
        assert!(
            outside_file.exists(),
            "プロジェクト外のファイルは削除されていないべき"
        );
    }
}

// =============================================================================
// 設定ファイルの複合テスト
// =============================================================================

mod config_combination_tests {
    use super::*;

    #[test]
    fn test_allowed_paths_with_strict_mode_and_force() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "init.txt", "init");

        let allowed_dir = tempfile::tempdir().unwrap();
        let allowed_canonical = allowed_dir.path().canonicalize().unwrap();
        let allowed_file = allowed_canonical.join("test.txt");
        fs::write(&allowed_file, "allowed content").unwrap();

        // 厳格モード + allowed_paths の組み合わせ
        let config = tempfile::NamedTempFile::new().unwrap();
        let config_content = format!(
            "allow_project_deletion = false\n\n[[allowed_paths]]\npath = \"{}\"\nrecursive = true\n",
            allowed_canonical.to_string_lossy()
        );
        fs::write(config.path(), config_content).unwrap();

        let (exit_code, stdout, stderr) = run_safe_rm_with_config(
            &[allowed_file.to_str().unwrap()],
            &repo_path,
            Some(config.path()),
        );

        assert_eq!(
            exit_code, 0,
            "allowed_paths 内のファイルは strict モードでも削除可能であるべき. stderr: {}",
            stderr
        );
        assert!(
            stdout.contains("allowed by config"),
            "allowed_paths で許可されたことが表示されるべき"
        );
        assert!(!allowed_file.exists(), "ファイルが削除されているべき");
    }

    #[test]
    fn test_multiple_allowed_paths_entries() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "init.txt", "init");

        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let canonical_a = dir_a.path().canonicalize().unwrap();
        let canonical_b = dir_b.path().canonicalize().unwrap();

        let file_a = canonical_a.join("a.txt");
        let file_b = canonical_b.join("b.txt");
        fs::write(&file_a, "a").unwrap();
        fs::write(&file_b, "b").unwrap();

        let config = tempfile::NamedTempFile::new().unwrap();
        let config_content = format!(
            "[[allowed_paths]]\npath = \"{}\"\nrecursive = true\n\n[[allowed_paths]]\npath = \"{}\"\nrecursive = false\n",
            canonical_a.to_string_lossy(),
            canonical_b.to_string_lossy()
        );
        fs::write(config.path(), config_content).unwrap();

        // dir_a の子ファイル削除
        let (exit_a, _, _) =
            run_safe_rm_with_config(&[file_a.to_str().unwrap()], &repo_path, Some(config.path()));
        assert_eq!(exit_a, 0, "dir_a 内のファイルは削除可能であるべき");
        assert!(!file_a.exists(), "file_a が削除されているべき");

        // dir_b の直下のファイル削除（非再帰で直下は許可）
        let (exit_b, _, _) =
            run_safe_rm_with_config(&[file_b.to_str().unwrap()], &repo_path, Some(config.path()));
        assert_eq!(exit_b, 0, "dir_b 直下のファイルは削除可能であるべき");
        assert!(!file_b.exists(), "file_b が削除されているべき");
    }
}

// =============================================================================
// 厳格モード + force フラグの複合テスト
// =============================================================================

mod strict_force_tests {
    use super::*;

    /// force フラグはダーティファイルの Git チェックをバイパスしないことを検証
    #[test]
    fn test_strict_mode_force_still_blocks_dirty_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "file.txt", "original");

        // ファイルを変更してダーティにする
        fs::write(repo_path.join("file.txt"), "modified").unwrap();

        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        // -f フラグをつけてもダーティファイルはブロックされるべき
        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["-f", "file.txt"], &repo_path, Some(config.path()));
        assert_eq!(
            exit_code, 2,
            "force フラグでもダーティファイルはブロックされるべき"
        );
        assert!(
            stderr.contains("Modified") || stderr.contains("未コミット"),
            "ダーティステータスが報告されるべき: {}",
            stderr
        );
        assert!(
            repo_path.join("file.txt").exists(),
            "ファイルが残っているべき"
        );
    }

    /// force フラグ + 厳格モードで存在しないファイルは無視されることを検証
    #[test]
    fn test_strict_mode_force_nonexistent_is_silent() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "init.txt", "init");

        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        let (exit_code, _, _) =
            run_safe_rm_with_config(&["-f", "nonexistent.txt"], &repo_path, Some(config.path()));
        assert_eq!(exit_code, 0, "force + 存在しないファイルは成功すべき");
    }

    /// force + recursive + 厳格モードでダーティなディレクトリ内容がブロックされることを検証
    #[test]
    fn test_strict_mode_force_recursive_blocks_dirty_directory() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "dir/clean.txt", "clean");

        // ディレクトリ内に未追跡ファイルを追加
        fs::write(repo_path.join("dir").join("untracked.txt"), "new").unwrap();

        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        let (exit_code, _, _) =
            run_safe_rm_with_config(&["-rf", "dir"], &repo_path, Some(config.path()));
        assert_eq!(
            exit_code, 2,
            "force + recursive でもダーティなディレクトリはブロックされるべき"
        );
        assert!(
            repo_path.join("dir").exists(),
            "ディレクトリが残っているべき"
        );
    }
}

// =============================================================================
// 相対パスの .. コンポーネントを含む統合テスト
// =============================================================================

mod relative_path_tests {
    use super::*;

    /// 相対パスに .. を含むがプロジェクト内に解決されるケース
    #[test]
    fn test_dotdot_resolves_inside_project() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "sub/file.txt", "content");

        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        // sub/ ディレクトリから ../sub/file.txt を指定（プロジェクト内に解決される）
        let sub_dir = repo_path.join("sub");
        let (exit_code, stdout, _) =
            run_safe_rm_with_config(&["../sub/file.txt"], &sub_dir, Some(config.path()));
        assert_eq!(
            exit_code, 0,
            "../sub/file.txt はプロジェクト内に解決されるべき"
        );
        assert!(
            stdout.contains("removed"),
            "削除完了メッセージが表示されるべき"
        );
        assert!(
            !repo_path.join("sub").join("file.txt").exists(),
            "ファイルが削除されているべき"
        );
    }

    /// 相対パスに .. を含みプロジェクト外に出るケース
    #[test]
    fn test_dotdot_resolves_outside_project_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "init.txt", "init");

        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        // リポジトリルートから ../../etc/passwd を指定
        let (exit_code, _, _) =
            run_safe_rm_with_config(&["../../etc/passwd"], &repo_path, Some(config.path()));
        assert_eq!(exit_code, 2, "プロジェクト外へのパスはブロックされるべき");
    }
}

// =============================================================================
// 厳格モードでバッチ全ダーティのテスト
// =============================================================================

mod batch_all_dirty_tests {
    use super::*;

    /// バッチ内の全ファイルがダーティな場合に終了コード 2 が返ることを検証
    #[test]
    fn test_batch_all_dirty_returns_exit_2() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "a.txt", "a");
        commit_file(&repo_path, "b.txt", "b");

        // 両方のファイルを変更
        fs::write(repo_path.join("a.txt"), "modified_a").unwrap();
        fs::write(repo_path.join("b.txt"), "modified_b").unwrap();

        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["a.txt", "b.txt"], &repo_path, Some(config.path()));
        assert_eq!(exit_code, 2, "全ダーティの場合は終了コード 2 であるべき");
        assert!(
            stderr.contains("a.txt") && stderr.contains("b.txt"),
            "両方のエラーが報告されるべき: {}",
            stderr
        );
    }

    /// バッチ内に未追跡ファイルと変更ファイルが混在する場合
    #[test]
    fn test_batch_mixed_dirty_types() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "tracked.txt", "content");

        // tracked.txt を変更、untracked.txt を新規作成
        fs::write(repo_path.join("tracked.txt"), "modified").unwrap();
        fs::write(repo_path.join("untracked.txt"), "new").unwrap();

        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        let (exit_code, _, stderr) = run_safe_rm_with_config(
            &["tracked.txt", "untracked.txt"],
            &repo_path,
            Some(config.path()),
        );
        assert_eq!(exit_code, 2, "ダーティファイルが含まれる場合は終了コード 2");
        assert!(
            stderr.contains("tracked.txt"),
            "Modified ファイルのエラーが報告されるべき: {}",
            stderr
        );
    }
}

// =============================================================================
// allowed_paths でディレクトリ自体の操作テスト
// =============================================================================

mod allowed_paths_directory_self_tests {
    use super::*;

    /// recursive allowed_paths で許可ディレクトリ自体を -r で削除できることを検証
    #[test]
    fn test_recursive_allowed_path_directory_itself_deletable() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "init.txt", "init");

        let allowed_dir = tempfile::tempdir().unwrap();
        let canonical_allowed = allowed_dir.path().canonicalize().unwrap();
        fs::write(canonical_allowed.join("file.txt"), "data").unwrap();

        let config = tempfile::NamedTempFile::new().unwrap();
        let config_content = format!(
            "allow_project_deletion = false\n\n[[allowed_paths]]\npath = \"{}\"\nrecursive = true\n",
            canonical_allowed.to_string_lossy()
        );
        fs::write(config.path(), config_content).unwrap();

        let (exit_code, stdout, _) = run_safe_rm_with_config(
            &["-r", canonical_allowed.to_str().unwrap()],
            &repo_path,
            Some(config.path()),
        );
        assert_eq!(
            exit_code, 0,
            "recursive 許可パスのディレクトリ自体は削除可能であるべき"
        );
        assert!(
            stdout.contains("allowed by config"),
            "設定による許可メッセージが表示されるべき"
        );
        assert!(
            !canonical_allowed.exists(),
            "ディレクトリが削除されているべき"
        );
    }

    #[test]
    fn test_allowed_paths_cannot_bypass_project_git_metadata_directory() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "tracked.txt", "tracked");

        let config = tempfile::NamedTempFile::new().unwrap();
        let config_content = format!(
            "allow_project_deletion = false\n\n[[allowed_paths]]\npath = \"{}\"\nrecursive = true\n",
            repo_path.to_string_lossy()
        );
        fs::write(config.path(), config_content).unwrap();

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["-r", "."], &repo_path, Some(config.path()));

        assert_eq!(
            exit_code, 2,
            "allowed_paths でも Git 管理メタデータを含む削除はブロックされるべき"
        );
        assert!(
            stderr.contains("Git 管理メタデータ"),
            "Git 管理メタデータの保護エラーが必要: {}",
            stderr
        );
        assert!(repo_path.join(".git").exists(), ".git は残っているべき");
        assert!(
            repo_path.join("tracked.txt").exists(),
            "作業ツリーファイルも削除されてはならない"
        );
    }

    /// 非再帰 allowed_paths で許可ディレクトリ自体は削除不可であることを検証
    #[test]
    fn test_non_recursive_allowed_path_directory_itself_blocked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "init.txt", "init");

        let allowed_dir = tempfile::tempdir().unwrap();
        let canonical_allowed = allowed_dir.path().canonicalize().unwrap();

        let config = tempfile::NamedTempFile::new().unwrap();
        let config_content = format!(
            "allow_project_deletion = false\n\n[[allowed_paths]]\npath = \"{}\"\nrecursive = false\n",
            canonical_allowed.to_string_lossy()
        );
        fs::write(config.path(), config_content).unwrap();

        // 非再帰ではディレクトリ自体は直接の子ではないため allowed にマッチしない
        let (exit_code, _, _) = run_safe_rm_with_config(
            &["-r", canonical_allowed.to_str().unwrap()],
            &repo_path,
            Some(config.path()),
        );
        assert_eq!(
            exit_code, 2,
            "non-recursive 許可パスではディレクトリ自体の削除はブロックされるべき"
        );
        assert!(canonical_allowed.exists(), "ディレクトリが残っているべき");
    }
}

// =============================================================================
// 2パスバッチの終了コード優先度テスト
// =============================================================================

mod batch_two_paths_tests {
    use super::*;

    #[test]
    fn test_batch_two_paths_exit1_and_exit2_returns_exit2() {
        // 2パスバッチ: NotFound (exit 1) と OutsideProject (exit 2) の組み合わせ
        // セキュリティエラーが優先されて exit 2 を返すべき
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "dummy.txt", "content");

        // -f で NotFound をサイレントにしないため force なしで実行
        // パス1: 存在しないファイル (exit 1)
        // パス2: プロジェクト外 (exit 2)
        let (exit_code, _, _) = run_safe_rm(&["nonexistent.txt", "/etc/passwd"], &repo_path);
        assert_eq!(
            exit_code, 2,
            "セキュリティエラー (exit 2) が操作エラー (exit 1) より優先されるべき"
        );
    }

    #[test]
    fn test_batch_two_paths_both_exit2_returns_exit2() {
        // 2パスバッチ: 両方プロジェクト外 (exit 2)
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "dummy.txt", "content");

        let (exit_code, _, _) = run_safe_rm(&["/etc/passwd", "/etc/hosts"], &repo_path);
        assert_eq!(
            exit_code, 2,
            "両方セキュリティエラーの場合は exit 2 を返すべき"
        );
    }
}

// =============================================================================
// シンボリックリンク先がディレクトリの場合の非再帰削除テスト
// =============================================================================

#[cfg(unix)]
mod symlink_to_directory_no_recursive_tests {
    use super::*;

    #[test]
    fn test_symlink_to_directory_without_recursive_is_removed() {
        // ディレクトリへのシンボリックリンクは -r なしでも削除可能
        // (symlink_metadata().is_dir() は false なので -r チェックをパスする)
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "dummy.txt", "content");

        // ターゲットディレクトリとシンボリックリンクを作成
        let target_dir = repo_path.join("target_dir");
        fs::create_dir(&target_dir).unwrap();
        fs::write(target_dir.join("inner.txt"), "content").unwrap();

        // ターゲットディレクトリをコミット
        std::process::Command::new("git")
            .args(["add", "target_dir/inner.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "Add target_dir"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // シンボリックリンクを作成してコミット
        let link_path = repo_path.join("link_to_dir");
        std::os::unix::fs::symlink(&target_dir, &link_path).unwrap();
        std::process::Command::new("git")
            .args(["add", "link_to_dir"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "Add symlink"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // -r なしでシンボリックリンクを削除
        let (exit_code, stdout, _) = run_safe_rm(&["link_to_dir"], &repo_path);
        assert_eq!(
            exit_code, 0,
            "ディレクトリへのシンボリックリンクは -r なしで削除可能であるべき"
        );
        assert!(
            stdout.contains("removed"),
            "削除成功メッセージが出力されるべき"
        );
        assert!(
            link_path.symlink_metadata().is_err(),
            "シンボリックリンク自体が削除されているべき"
        );
        assert!(target_dir.exists(), "リンク先ディレクトリは残っているべき");
        assert!(
            target_dir.join("inner.txt").exists(),
            "リンク先ディレクトリ内のファイルも残っているべき"
        );
    }
}

// =============================================================================
// Git ステータス取得エラー時の fail-closed テスト
// =============================================================================

mod git_error_fail_closed_tests {
    use super::*;

    #[test]
    fn test_strict_mode_git_index_corruption_blocks_deletion() {
        // Git index ファイルを破損させて status API エラーを誘発し、
        // fail-closed で削除がブロックされることを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "clean.txt", "content");

        // strict モードの設定を作成
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(config.path(), "allow_project_deletion = false\n").unwrap();

        // .git/index を破損させて Git API エラーを誘発
        let index_path = repo_path.join(".git").join("index");
        fs::write(&index_path, b"CORRUPTED_INDEX_DATA").unwrap();

        // Clean ファイルの削除を試みる — Git エラーでブロックされるべき
        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["clean.txt"], &repo_path, Some(config.path()));

        assert_ne!(
            exit_code, 0,
            "Git index 破損時は削除がブロックされるべき (fail-closed)"
        );
        assert!(
            repo_path.join("clean.txt").exists(),
            "Git エラー時はファイルが残っているべき"
        );
        assert!(
            stderr.contains("Git error") || stderr.contains("error"),
            "Git エラーメッセージが表示されるべき: stderr='{}'",
            stderr
        );
    }
}

// =============================================================================
// バッチ処理の終了コード優先度テスト
// =============================================================================

mod batch_exit_code_priority_tests {
    use super::*;

    #[test]
    fn test_batch_three_paths_security_error_takes_highest_priority() {
        // 3パスバッチ: NotFound(1) + DirtyFiles(2) + Clean(0) → exit 2
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "clean.txt", "clean content");
        fs::write(repo_path.join("dirty.txt"), "untracked content").unwrap();

        let config_dir = tempfile::tempdir().unwrap();
        let config_path = config_dir.path().join("config.toml");
        fs::write(&config_path, "allow_project_deletion = false\n").unwrap();

        let (exit_code, stdout, _stderr) = run_safe_rm_with_config(
            &["clean.txt", "dirty.txt", "nonexistent.txt"],
            &repo_path,
            Some(&config_path),
        );

        assert_eq!(
            exit_code, 2,
            "セキュリティエラー（exit 2）が最優先されるべき"
        );
        // clean.txt は削除されている
        assert!(
            stdout.contains("removed: clean.txt"),
            "クリーンファイルは削除されるべき"
        );
    }

    #[test]
    fn test_batch_all_success_returns_zero() {
        // 全パスが成功する場合は exit 0
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "file1.txt", "content1");
        commit_file(&repo_path, "file2.txt", "content2");

        let (exit_code, stdout, stderr) = run_safe_rm(&["file1.txt", "file2.txt"], &repo_path);

        assert_eq!(exit_code, 0, "全ファイル削除成功時は exit 0");
        assert!(stdout.contains("removed: file1.txt"));
        assert!(stdout.contains("removed: file2.txt"));
        assert!(stderr.is_empty(), "エラー出力はない��き");
    }
}

// =============================================================================
// ドライランの詳細テスト
// =============================================================================

mod dry_run_detail_tests {
    use super::*;

    #[test]
    fn test_dry_run_does_not_modify_filesystem() {
        // ドライランでファイルが実際に削除されないことの厳密な検証
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "important.txt", "important data");
        let file_path = repo_path.join("important.txt");
        assert!(file_path.exists(), "テスト前提: ファイルが存在する");

        let (exit_code, stdout, _) = run_safe_rm(&["-n", "important.txt"], &repo_path);

        assert_eq!(exit_code, 0);
        assert!(stdout.contains("would remove"));
        assert!(file_path.exists(), "ドライラン後もファイルが残っているべき");
        // ファイル内容も変更されていない
        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "important data", "ファイル内容が変更されていない");
    }

    #[test]
    fn test_dry_run_batch_shows_all_results() {
        // バッチドライランで全パスの結果が表示される
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "a.txt", "a");
        commit_file(&repo_path, "b.txt", "b");
        commit_file(&repo_path, "c.txt", "c");

        let (exit_code, stdout, _) = run_safe_rm(&["-n", "a.txt", "b.txt", "c.txt"], &repo_path);

        assert_eq!(exit_code, 0);
        assert!(stdout.contains("would remove: a.txt"));
        assert!(stdout.contains("would remove: b.txt"));
        assert!(stdout.contains("would remove: c.txt"));
        // 全ファイルが残っている
        assert!(repo_path.join("a.txt").exists());
        assert!(repo_path.join("b.txt").exists());
        assert!(repo_path.join("c.txt").exists());
    }
}

// =============================================================================
// 設定ファイルのエッジケーステスト
// =============================================================================

// =============================================================================
// 壊れたシンボリックリンクのテスト
// =============================================================================

#[cfg(unix)]
mod broken_symlink_tests {
    use super::*;

    #[test]
    fn test_default_mode_broken_symlink_deletable() {
        // デフォルトモード（allow_project_deletion=true）では壊れた symlink も削除可能
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "initial.txt", "initial");

        let broken_link = repo_path.join("broken_link.txt");
        std::os::unix::fs::symlink("/nonexistent/target", &broken_link).unwrap();

        let (exit_code, stdout, _) = run_safe_rm(&["broken_link.txt"], &repo_path);

        assert_eq!(
            exit_code, 0,
            "デフォルトモードでは壊れた symlink も削除可能"
        );
        assert!(stdout.contains("removed: broken_link.txt"));
        assert!(
            broken_link.symlink_metadata().is_err(),
            "壊れた symlink が削除されているべき"
        );
    }

    #[test]
    fn test_strict_mode_broken_symlink_blocked() {
        // strict モード（allow_project_deletion=false）では壊れた symlink（未追跡）は削除不可
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "initial.txt", "initial");

        let broken_link = repo_path.join("broken_link.txt");
        std::os::unix::fs::symlink("/nonexistent/target", &broken_link).unwrap();

        let config_dir = tempfile::tempdir().unwrap();
        let config_path = config_dir.path().join("config.toml");
        fs::write(&config_path, "allow_project_deletion = false\n").unwrap();

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["broken_link.txt"], &repo_path, Some(&config_path));

        assert_eq!(
            exit_code, 2,
            "strict モードでは壊れた symlink（未追跡）の削除はブロックされるべき"
        );
        assert!(
            stderr.contains("Untracked"),
            "エラーメッセージに Untracked が含まれるべき"
        );
    }

    #[test]
    fn test_committed_broken_symlink_deletable_in_strict_mode() {
        // strict モードでもコミット済みの壊れた symlink は削除可能
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "initial.txt", "initial");

        // 壊れた symlink を作成してコミット
        let broken_link = repo_path.join("committed_broken.txt");
        std::os::unix::fs::symlink("/nonexistent/target", &broken_link).unwrap();
        std::process::Command::new("git")
            .args(["add", "committed_broken.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "Add broken symlink"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let config_dir = tempfile::tempdir().unwrap();
        let config_path = config_dir.path().join("config.toml");
        fs::write(&config_path, "allow_project_deletion = false\n").unwrap();

        let (exit_code, stdout, _) =
            run_safe_rm_with_config(&["committed_broken.txt"], &repo_path, Some(&config_path));

        assert_eq!(
            exit_code, 0,
            "コミット済みの壊れた symlink は strict モードでも削除可能"
        );
        assert!(stdout.contains("removed: committed_broken.txt"));
    }
}

// =============================================================================
// allowed_paths のドライランテスト
// =============================================================================

mod allowed_paths_dry_run_tests {
    use super::*;

    #[test]
    fn test_dry_run_with_allowed_paths_shows_config_message() {
        // allowed_paths 経由のドライランで "(allowed by config)" が表示される
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "initial.txt", "initial");

        let allowed_dir = repo_path.join("allowed");
        fs::create_dir_all(&allowed_dir).unwrap();
        fs::write(allowed_dir.join("file.txt"), "content").unwrap();

        let config_dir = tempfile::tempdir().unwrap();
        let config_path = config_dir.path().join("config.toml");
        fs::write(
            &config_path,
            format!(
                r#"allow_project_deletion = false

[[allowed_paths]]
path = "{}"
recursive = true
"#,
                allowed_dir.display()
            ),
        )
        .unwrap();

        let (exit_code, stdout, _) = run_safe_rm_with_config(
            &["-n", &format!("{}", allowed_dir.join("file.txt").display())],
            &repo_path,
            Some(&config_path),
        );

        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("allowed by config"),
            "allowed_paths 経由のドライランは '(allowed by config)' を表示すべき"
        );
    }
}

// =============================================================================
// 空リポジトリの strict モードテスト
// =============================================================================

mod empty_repo_tests {
    use super::*;

    #[test]
    fn test_strict_mode_empty_repo_untracked_file_blocked() {
        // 初期コミットなしの空リポジトリで、strict モードの未追跡ファイル削除がブロックされる
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // コミットなしで直接ファイルを作成
        fs::write(repo_path.join("new_file.txt"), "content").unwrap();

        let config_dir = tempfile::tempdir().unwrap();
        let config_path = config_dir.path().join("config.toml");
        fs::write(&config_path, "allow_project_deletion = false\n").unwrap();

        let (exit_code, _, stderr) =
            run_safe_rm_with_config(&["new_file.txt"], &repo_path, Some(&config_path));

        assert_eq!(
            exit_code, 2,
            "空リポジトリの strict モードでも未追跡ファイルはブロックされるべき"
        );
        assert!(stderr.contains("Untracked"));
    }
}

// =============================================================================
// バッチ処理の force フラグ複合テスト
// =============================================================================

mod batch_force_tests {
    use super::*;

    #[test]
    fn test_batch_force_all_nonexistent_returns_zero() {
        // --force で全ファイルが存在しない場合、成功（exit 0）
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "initial.txt", "initial");

        let (exit_code, stdout, stderr) = run_safe_rm(
            &["-f", "missing1.txt", "missing2.txt", "missing3.txt"],
            &repo_path,
        );

        assert_eq!(exit_code, 0, "全ファイル存在せず --force なら exit 0");
        assert!(stdout.is_empty(), "存在しないファイルの出力はないべき");
        assert!(stderr.is_empty(), "エラー出力もないべき");
    }

    #[test]
    fn test_batch_force_mix_existing_and_nonexistent() {
        // --force でexisting + nonexistent の混在
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "exists.txt", "content");

        let (exit_code, stdout, _) = run_safe_rm(&["-f", "exists.txt", "missing.txt"], &repo_path);

        assert_eq!(exit_code, 0, "混在しても --force で成功すべき");
        assert!(stdout.contains("removed: exists.txt"));
        assert!(!repo_path.join("exists.txt").exists());
    }
}

mod config_edge_case_tests {
    use super::*;

    #[test]
    fn test_config_with_empty_allowed_paths_array() {
        // allowed_paths が空配列の設定でも正常に動作
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "test.txt", "content");

        let config_dir = tempfile::tempdir().unwrap();
        let config_path = config_dir.path().join("config.toml");
        fs::write(
            &config_path,
            "allow_project_deletion = false\nallowed_paths = []\n",
        )
        .unwrap();

        let (exit_code, stdout, _) =
            run_safe_rm_with_config(&["test.txt"], &repo_path, Some(&config_path));

        assert_eq!(exit_code, 0);
        assert!(stdout.contains("removed: test.txt"));
    }

    #[test]
    fn test_config_with_nonexistent_allowed_path_does_not_crash() {
        // 存在しないディレクトリを allowed_paths に設定してもクラッシュしない
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "test.txt", "content");

        let config_dir = tempfile::tempdir().unwrap();
        let config_path = config_dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
allow_project_deletion = false

[[allowed_paths]]
path = "/nonexistent/path/that/does/not/exist"
recursive = true
"#,
        )
        .unwrap();

        let (exit_code, _, _) =
            run_safe_rm_with_config(&["test.txt"], &repo_path, Some(&config_path));

        // test.txt はクリーンなので削除可能
        assert_eq!(
            exit_code, 0,
            "存在しない allowed_paths でもクラッシュしない"
        );
    }
}
