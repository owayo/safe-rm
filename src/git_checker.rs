//! Git status checking for safe-rm
//!
//! Detects Git repositories and checks file status for safe deletion.

use crate::error::{FileStatus, SafeRmError};
use git2::{Repository, Status, StatusOptions};
use std::collections::HashMap;
use std::path::Path;

/// Git ステータスチェッカー
pub struct GitChecker {
    repo: Repository,
}

impl GitChecker {
    /// プロジェクトルートで Git リポジトリを開く
    ///
    /// # Returns
    /// * `Some(GitChecker)` - Git リポジトリが存在
    /// * `None` - Git リポジトリなし（Git チェックスキップ）
    pub fn open(project_root: &Path) -> Option<Self> {
        Repository::open(project_root)
            .ok()
            .map(|repo| Self { repo })
    }

    /// 全ファイルのステータスを一括取得（バッチ処理用）
    ///
    /// 一度の Git API 呼び出しで全ステータスを取得し、HashMap として返す。
    /// これにより、多数のファイルを処理する際の API 呼び出し回数を削減。
    ///
    /// # Returns
    /// * `HashMap<String, FileStatus>` - 相対パス → ステータスのマップ
    pub fn get_all_statuses(&self) -> HashMap<String, FileStatus> {
        let mut status_map = HashMap::new();

        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        opts.include_ignored(true);
        opts.recurse_untracked_dirs(true);

        if let Ok(statuses) = self.repo.statuses(Some(&mut opts)) {
            for entry in statuses.iter() {
                if let Some(path) = entry.path() {
                    let status = Self::convert_status(entry.status());
                    status_map.insert(path.to_string(), status);
                }
            }
        }

        status_map
    }

    /// キャッシュからファイルステータスを取得
    ///
    /// `get_all_statuses()` で事前取得したキャッシュを使用。
    /// キャッシュにない場合は Clean として扱う（Git 追跡済みで変更なし）。
    pub fn get_file_status_from_cache(
        &self,
        path: &Path,
        cache: &HashMap<String, FileStatus>,
    ) -> FileStatus {
        let workdir = match self.repo.workdir() {
            Some(dir) => dir,
            None => return FileStatus::NotInRepo,
        };

        let relative_path = match path.strip_prefix(workdir) {
            Ok(p) => p,
            Err(_) => return FileStatus::NotInRepo,
        };

        let path_str = relative_path.to_string_lossy().to_string();

        // キャッシュから取得
        if let Some(&status) = cache.get(&path_str) {
            return status;
        }

        // キャッシュにない場合: .gitignore チェック
        if self.is_ignored_path(path) {
            return FileStatus::Ignored;
        }

        // Git 追跡済みで変更がない（Clean）か、リポジトリ外
        // status_file で確認
        match self.repo.status_file(relative_path) {
            Ok(status) if status.is_empty() => FileStatus::Clean,
            Ok(status) => Self::convert_status(status),
            Err(_) => FileStatus::NotInRepo,
        }
    }

    /// ファイルの Git ステータスを取得
    pub fn get_file_status(&self, path: &Path) -> FileStatus {
        // リポジトリルートからの相対パスを取得
        let workdir = match self.repo.workdir() {
            Some(dir) => dir,
            None => return FileStatus::NotInRepo,
        };

        let relative_path = match path.strip_prefix(workdir) {
            Ok(p) => p,
            Err(_) => return FileStatus::NotInRepo,
        };

        // status_file を使用して直接ステータスを取得
        match self.repo.status_file(relative_path) {
            Ok(status) => Self::convert_status(status),
            Err(e) => {
                // ファイルが追跡されていない場合のエラーハンドリング
                if e.code() == git2::ErrorCode::NotFound {
                    // Git管理外のファイル（.gitignore にも含まれていない新規ファイル）
                    // この場合は statuses() API で確認する
                    let mut opts = StatusOptions::new();
                    opts.include_untracked(true);
                    opts.include_ignored(true);

                    if let Ok(statuses) = self.repo.statuses(Some(&mut opts)) {
                        for entry in statuses.iter() {
                            if let Some(entry_path) = entry.path() {
                                if entry_path == relative_path.to_string_lossy() {
                                    return Self::convert_status(entry.status());
                                }
                            }
                        }
                    }
                    // 見つからない場合は NotInRepo
                    FileStatus::NotInRepo
                } else {
                    FileStatus::NotInRepo
                }
            }
        }
    }

    /// git2 のステータスフラグから FileStatus への変換
    fn convert_status(status: Status) -> FileStatus {
        // Ignored チェック（最優先）
        if status.contains(Status::IGNORED) {
            return FileStatus::Ignored;
        }

        // Index 変更（Staged）
        if status.intersects(
            Status::INDEX_NEW
                | Status::INDEX_MODIFIED
                | Status::INDEX_DELETED
                | Status::INDEX_RENAMED
                | Status::INDEX_TYPECHANGE,
        ) {
            return FileStatus::Staged;
        }

        // Worktree 変更（Modified）
        if status.intersects(
            Status::WT_MODIFIED | Status::WT_DELETED | Status::WT_RENAMED | Status::WT_TYPECHANGE,
        ) {
            return FileStatus::Modified;
        }

        // 未追跡
        if status.contains(Status::WT_NEW) {
            return FileStatus::Untracked;
        }

        // 上記以外（稀なケース）は Clean として扱う
        FileStatus::Clean
    }

    /// ステータスが削除許可かどうかを判定
    pub fn is_deletable(status: FileStatus) -> bool {
        matches!(
            status,
            FileStatus::Clean | FileStatus::Ignored | FileStatus::NotInRepo
        )
    }

    /// ファイルまたはディレクトリをチェック
    ///
    /// # Returns
    /// * `Ok(())` - 削除可能
    /// * `Err(SafeRmError::DirtyFiles)` - Dirty ファイルが存在
    pub fn check_path(&self, path: &Path) -> Result<(), SafeRmError> {
        if path.is_dir() {
            self.check_directory(path)
        } else {
            self.check_file(path)
        }
    }

    /// 単一ファイルのチェック
    fn check_file(&self, path: &Path) -> Result<(), SafeRmError> {
        let status = self.get_file_status(path);
        if Self::is_deletable(status) {
            Ok(())
        } else {
            Err(SafeRmError::DirtyFiles {
                path: path.to_path_buf(),
                status,
            })
        }
    }

    /// ディレクトリ内のすべてのファイルをチェック
    ///
    /// # Returns
    /// * `Ok(())` - 全ファイルが Clean または Ignored
    /// * `Err(SafeRmError::DirtyFiles)` - Dirty ファイルが存在
    pub fn check_directory(&self, dir: &Path) -> Result<(), SafeRmError> {
        // まずディレクトリ自体が Ignored かチェック（早期許可）
        let dir_status = self.get_directory_status(dir);
        if dir_status == FileStatus::Ignored {
            return Ok(());
        }

        // ディレクトリ内のファイルを再帰的にチェック
        self.check_directory_recursive(dir)
    }

    /// ディレクトリ自体のステータスを取得
    fn get_directory_status(&self, dir: &Path) -> FileStatus {
        let workdir = match self.repo.workdir() {
            Some(d) => d,
            None => return FileStatus::NotInRepo,
        };

        let relative_path = match dir.strip_prefix(workdir) {
            Ok(p) => p,
            Err(_) => return FileStatus::NotInRepo,
        };

        // ディレクトリパスの末尾にスラッシュを追加して gitignore マッチング
        let dir_pattern = format!("{}/", relative_path.display());

        let mut opts = StatusOptions::new();
        opts.pathspec(&dir_pattern);
        opts.include_ignored(true);

        if let Ok(statuses) = self.repo.statuses(Some(&mut opts)) {
            for entry in statuses.iter() {
                if entry.status().contains(Status::IGNORED) {
                    return FileStatus::Ignored;
                }
            }
        }

        // ディレクトリが .gitignore にマッチするかを直接チェック
        if self.is_ignored_path(dir) {
            return FileStatus::Ignored;
        }

        FileStatus::Clean
    }

    /// パスが .gitignore に含まれるかチェック
    fn is_ignored_path(&self, path: &Path) -> bool {
        let workdir = match self.repo.workdir() {
            Some(d) => d,
            None => return false,
        };

        let relative_path = match path.strip_prefix(workdir) {
            Ok(p) => p,
            Err(_) => return false,
        };

        self.repo
            .status_should_ignore(relative_path)
            .unwrap_or(false)
    }

    /// ディレクトリ内のファイルを再帰的にチェック
    fn check_directory_recursive(&self, dir: &Path) -> Result<(), SafeRmError> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => {
                // fail-closed: ディレクトリ読み取り失敗は削除をブロック
                return Err(SafeRmError::DirectoryReadError {
                    path: dir.to_path_buf(),
                });
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();

            if path.is_dir() {
                // サブディレクトリは再帰的にチェック
                self.check_directory(&path)?;
            } else {
                // ファイルのステータスをチェック
                let status = self.get_file_status(&path);
                if !Self::is_deletable(status) {
                    return Err(SafeRmError::DirtyFiles { path, status });
                }
            }
        }

        Ok(())
    }

    /// ディレクトリ内のファイルをキャッシュを使用して再帰的にチェック（高速版）
    ///
    /// `get_all_statuses()` で事前取得したキャッシュを使用することで、
    /// 多数のファイルを持つディレクトリの検証を高速化。
    pub fn check_directory_with_cache(
        &self,
        dir: &Path,
        cache: &HashMap<String, FileStatus>,
    ) -> Result<(), SafeRmError> {
        // まずディレクトリ自体が Ignored かチェック（早期許可）
        let dir_status = self.get_directory_status(dir);
        if dir_status == FileStatus::Ignored {
            return Ok(());
        }

        self.check_directory_recursive_with_cache(dir, cache)
    }

    /// キャッシュを使用した再帰的ディレクトリチェック
    fn check_directory_recursive_with_cache(
        &self,
        dir: &Path,
        cache: &HashMap<String, FileStatus>,
    ) -> Result<(), SafeRmError> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => {
                return Err(SafeRmError::DirectoryReadError {
                    path: dir.to_path_buf(),
                });
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();

            if path.is_dir() {
                // サブディレクトリも再帰的にチェック
                self.check_directory_with_cache(&path, cache)?;
            } else {
                // キャッシュからステータスを取得
                let status = self.get_file_status_from_cache(&path, cache);
                if !Self::is_deletable(status) {
                    return Err(SafeRmError::DirtyFiles { path, status });
                }
            }
        }

        Ok(())
    }

    /// 単一ファイルをキャッシュを使用してチェック
    pub fn check_file_with_cache(
        &self,
        path: &Path,
        cache: &HashMap<String, FileStatus>,
    ) -> Result<(), SafeRmError> {
        let status = self.get_file_status_from_cache(path, cache);
        if Self::is_deletable(status) {
            Ok(())
        } else {
            Err(SafeRmError::DirtyFiles {
                path: path.to_path_buf(),
                status,
            })
        }
    }

    /// ファイルまたはディレクトリをキャッシュを使用してチェック
    pub fn check_path_with_cache(
        &self,
        path: &Path,
        cache: &HashMap<String, FileStatus>,
    ) -> Result<(), SafeRmError> {
        if path.is_dir() {
            self.check_directory_with_cache(path, cache)
        } else {
            self.check_file_with_cache(path, cache)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    /// テスト用の Git リポジトリを作成
    fn create_test_repo() -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        // git init
        Command::new("git")
            .args(["init"])
            .current_dir(repo_path)
            .output()
            .unwrap();

        // git config for commits
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

    /// ファイルを作成してコミット
    fn commit_file(repo_path: &Path, filename: &str, content: &str) {
        let file_path = repo_path.join(filename);
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

    // Task 6.1: Gitリポジトリ検出のテスト

    #[test]
    fn test_open_git_repo() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let checker = GitChecker::open(&repo_path);
        assert!(checker.is_some());
    }

    #[test]
    fn test_open_non_git_directory() {
        let temp_dir = TempDir::new().unwrap();
        let checker = GitChecker::open(temp_dir.path());
        assert!(checker.is_none());
    }

    // Task 6.2: ファイルステータス判定のテスト

    #[test]
    fn test_get_file_status_clean() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ファイルを作成してコミット
        commit_file(&repo_path, "clean.txt", "clean content");

        let checker = GitChecker::open(&repo_path).unwrap();
        let file_path = repo_path.join("clean.txt");
        let status = checker.get_file_status(&file_path);

        assert_eq!(status, FileStatus::Clean);
    }

    #[test]
    fn test_get_file_status_modified() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ファイルを作成してコミット
        commit_file(&repo_path, "modified.txt", "original content");

        // ファイルを変更
        let file_path = repo_path.join("modified.txt");
        fs::write(&file_path, "modified content").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let status = checker.get_file_status(&file_path);

        assert_eq!(status, FileStatus::Modified);
    }

    #[test]
    fn test_get_file_status_staged() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ファイルを作成
        let file_path = repo_path.join("staged.txt");
        fs::write(&file_path, "staged content").unwrap();

        // git add（コミットせず）
        Command::new("git")
            .args(["add", "staged.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let status = checker.get_file_status(&file_path);

        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_get_file_status_untracked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 初期コミットを作成（空のリポジトリでないことを確認）
        commit_file(&repo_path, "initial.txt", "initial");

        // 未追跡ファイルを作成
        let file_path = repo_path.join("untracked.txt");
        fs::write(&file_path, "untracked content").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let status = checker.get_file_status(&file_path);

        assert_eq!(status, FileStatus::Untracked);
    }

    #[test]
    fn test_get_file_status_ignored() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を作成
        let gitignore_path = repo_path.join(".gitignore");
        fs::write(&gitignore_path, "*.log\n").unwrap();

        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "Add .gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // 無視されるファイルを作成
        let file_path = repo_path.join("debug.log");
        fs::write(&file_path, "log content").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let status = checker.get_file_status(&file_path);

        assert_eq!(status, FileStatus::Ignored);
    }

    #[test]
    fn test_is_deletable_clean() {
        assert!(GitChecker::is_deletable(FileStatus::Clean));
    }

    #[test]
    fn test_is_deletable_ignored() {
        assert!(GitChecker::is_deletable(FileStatus::Ignored));
    }

    #[test]
    fn test_is_deletable_not_in_repo() {
        assert!(GitChecker::is_deletable(FileStatus::NotInRepo));
    }

    #[test]
    fn test_is_not_deletable_modified() {
        assert!(!GitChecker::is_deletable(FileStatus::Modified));
    }

    #[test]
    fn test_is_not_deletable_staged() {
        assert!(!GitChecker::is_deletable(FileStatus::Staged));
    }

    #[test]
    fn test_is_not_deletable_untracked() {
        assert!(!GitChecker::is_deletable(FileStatus::Untracked));
    }

    // Task 6.3: ディレクトリ再帰チェックのテスト

    #[test]
    fn test_check_directory_all_clean() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ディレクトリを作成
        let subdir = repo_path.join("subdir");
        fs::create_dir(&subdir).unwrap();

        // クリーンなファイルを作成
        let file1 = subdir.join("file1.txt");
        let file2 = subdir.join("file2.txt");
        fs::write(&file1, "content1").unwrap();
        fs::write(&file2, "content2").unwrap();

        // コミット
        Command::new("git")
            .args(["add", "."])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "Add files"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let result = checker.check_directory(&subdir);

        assert!(result.is_ok());
    }

    #[test]
    fn test_check_directory_with_dirty_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ディレクトリを作成
        let subdir = repo_path.join("subdir");
        fs::create_dir(&subdir).unwrap();

        // ファイルを作成してコミット
        let file1 = subdir.join("file1.txt");
        fs::write(&file1, "content1").unwrap();

        Command::new("git")
            .args(["add", "."])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "Add file1"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // 未追跡ファイルを追加
        let file2 = subdir.join("untracked.txt");
        fs::write(&file2, "untracked").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let result = checker.check_directory(&subdir);

        assert!(result.is_err());
        match result.unwrap_err() {
            SafeRmError::DirtyFiles { status, .. } => {
                assert_eq!(status, FileStatus::Untracked);
            }
            _ => panic!("Expected DirtyFiles error"),
        }
    }

    #[test]
    fn test_check_directory_ignored() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を作成
        let gitignore_path = repo_path.join(".gitignore");
        fs::write(&gitignore_path, "build/\n").unwrap();

        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "Add .gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Ignored ディレクトリを作成
        let build_dir = repo_path.join("build");
        fs::create_dir(&build_dir).unwrap();

        // ディレクトリ内に任意のファイルを作成
        let artifact = build_dir.join("output.bin");
        fs::write(&artifact, "binary content").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let result = checker.check_directory(&build_dir);

        // Ignored ディレクトリは早期許可
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_directory_nested() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ネストしたディレクトリを作成
        let nested = repo_path.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();

        // クリーンなファイルを作成
        let file = nested.join("deep.txt");
        fs::write(&file, "deep content").unwrap();

        Command::new("git")
            .args(["add", "."])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "Add deep file"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let parent = repo_path.join("a");
        let result = checker.check_directory(&parent);

        assert!(result.is_ok());
    }

    #[test]
    fn test_check_file_clean() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "clean.txt", "clean");

        let checker = GitChecker::open(&repo_path).unwrap();
        let file_path = repo_path.join("clean.txt");
        let result = checker.check_path(&file_path);

        assert!(result.is_ok());
    }

    #[test]
    fn test_check_file_modified() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "file.txt", "original");

        let file_path = repo_path.join("file.txt");
        fs::write(&file_path, "modified").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let result = checker.check_path(&file_path);

        assert!(result.is_err());
        match result.unwrap_err() {
            SafeRmError::DirtyFiles { status, .. } => {
                assert_eq!(status, FileStatus::Modified);
            }
            _ => panic!("Expected DirtyFiles error"),
        }
    }
}
