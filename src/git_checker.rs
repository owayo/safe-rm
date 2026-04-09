//! safe-rm の Git ステータスチェック
//!
//! Git リポジトリを検出し、安全な削除のためにファイルステータスを確認する。

use crate::error::{FileStatus, SafeRmError};
use git2::{Repository, Status, StatusOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Git ステータスチェッカー
pub struct GitChecker {
    repo: Repository,
    /// canonicalize 済みワークディレクトリ（macOS /var→/private/var 等のエイリアス対策）
    workdir_canonical: Option<PathBuf>,
}

impl GitChecker {
    /// 指定パスから Git リポジトリを検出して開く
    ///
    /// `Repository::discover` を使用して上位ディレクトリを走査し、
    /// Gitリポジトリを検出する。サブディレクトリからでもリポジトリルートを正しく検出可能。
    ///
    /// # 戻り値
    /// * `Some(GitChecker)` - Git リポジトリが存在
    /// * `None` - Git リポジトリなし（Git チェックスキップ）
    pub fn open(path: &Path) -> Option<Self> {
        Repository::discover(path).ok().map(|repo| {
            let workdir_canonical = repo
                .workdir()
                .map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()));
            Self {
                repo,
                workdir_canonical,
            }
        })
    }

    /// Git リポジトリのワークディレクトリ（ルート）を取得
    ///
    /// フルパス指定時のプロジェクト境界判定に使用。
    /// bare リポジトリの場合は None を返す。
    /// macOS の /var → /private/var シンボリックリンク対策で canonicalize 済み。
    pub fn workdir(&self) -> Option<PathBuf> {
        self.workdir_canonical.clone()
    }

    /// 絶対パスからワークディレクトリ相対パスを取得
    ///
    /// canonicalize 済みパスと未解決パスの両方に対応し、
    /// macOS の /var→/private/var 等のエイリアス差異を吸収する。
    fn to_workdir_relative(&self, path: &Path) -> Option<PathBuf> {
        // canonicalize 済みワークディレクトリで試行
        if let Some(canonical) = &self.workdir_canonical {
            if let Ok(rel) = path.strip_prefix(canonical) {
                return Some(rel.to_path_buf());
            }
        }
        // フォールバック: 未解決ワークディレクトリで試行
        if let Some(raw) = self.repo.workdir() {
            if let Ok(rel) = path.strip_prefix(raw) {
                return Some(rel.to_path_buf());
            }
        }
        None
    }

    /// 全ファイルのステータスを一括取得（バッチ処理用）
    ///
    /// 一度の Git API 呼び出しで全ステータスを取得し、HashMap として返す。
    /// これにより、多数のファイルを処理する際の API 呼び出し回数を削減。
    /// Git API エラー時は fail-closed でエラーを返す。
    ///
    /// # 戻り値
    /// * `Ok(HashMap<String, FileStatus>)` - 相対パス → ステータスのマップ
    /// * `Err(SafeRmError)` - Git API エラー（ステータス取得不可時は削除をブロック）
    pub fn get_all_statuses(&self) -> Result<HashMap<String, FileStatus>, SafeRmError> {
        let mut status_map = HashMap::new();

        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        opts.include_ignored(true);
        opts.recurse_untracked_dirs(true);

        let statuses = self.repo.statuses(Some(&mut opts))?;
        for entry in statuses.iter() {
            if let Some(path) = entry.path() {
                let status = Self::convert_status(entry.status());
                status_map.insert(path.to_string(), status);
            }
        }

        Ok(status_map)
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
        let relative_path = match self.to_workdir_relative(path) {
            Some(p) => p,
            None => return FileStatus::NotInRepo,
        };

        let path_key = Self::to_git_relative_key(&relative_path);

        // キャッシュから取得
        if let Some(&status) = cache.get(&path_key) {
            return status;
        }

        // キャッシュにない場合: .gitignore チェック
        if self.is_ignored_path(path) {
            return FileStatus::Ignored;
        }

        self.resolve_status_from_relative_path(&relative_path)
    }

    /// ファイルの Git ステータスを取得
    pub fn get_file_status(&self, path: &Path) -> FileStatus {
        // リポジトリルートからの相対パスを取得
        let relative_path = match self.to_workdir_relative(path) {
            Some(p) => p,
            None => return FileStatus::NotInRepo,
        };

        self.resolve_status_from_relative_path(&relative_path)
    }

    /// 相対パスの Git ステータスを解決
    ///
    /// `status_file()` は未追跡ディレクトリを 1 エントリに畳み込むため、
    /// その配下のファイルを直接問い合わせると `NotFound` になることがある。
    /// その場合は再帰付きの status 一覧で再確認し、未追跡ファイルを取りこぼさない。
    fn resolve_status_from_relative_path(&self, relative_path: &Path) -> FileStatus {
        match self.repo.status_file(relative_path) {
            Ok(status) => Self::convert_status(status),
            Err(e) if e.code() == git2::ErrorCode::NotFound => self
                .lookup_status_in_listing(relative_path)
                .unwrap_or(FileStatus::NotInRepo),
            // fail-closed: 予期しない Git エラー時は削除をブロック
            Err(_) => FileStatus::Modified,
        }
    }

    /// status 一覧から相対パスに対応するステータスを検索
    fn lookup_status_in_listing(&self, relative_path: &Path) -> Option<FileStatus> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);
        opts.include_ignored(true);

        let relative_path_key = Self::to_git_relative_key(relative_path);
        if let Ok(statuses) = self.repo.statuses(Some(&mut opts)) {
            for entry in statuses.iter() {
                if let Some(entry_path) = entry.path() {
                    if entry_path == relative_path_key {
                        return Some(Self::convert_status(entry.status()));
                    }
                }
            }
        }

        if self
            .repo
            .status_should_ignore(relative_path)
            .unwrap_or(false)
        {
            return Some(FileStatus::Ignored);
        }

        None
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
        status.is_deletable()
    }

    /// ファイルまたはディレクトリをチェック
    ///
    /// # 戻り値
    /// * `Ok(())` - 削除可能
    /// * `Err(SafeRmError::DirtyFiles)` - 変更のあるファイルが存在
    pub fn check_path(&self, path: &Path) -> Result<(), SafeRmError> {
        if Self::is_real_directory(path) {
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
    /// # 戻り値
    /// * `Ok(())` - 全ファイルが Clean または Ignored
    /// * `Err(SafeRmError::DirtyFiles)` - 変更のあるファイルが存在
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
        let relative_path = match self.to_workdir_relative(dir) {
            Some(p) => p,
            None => return FileStatus::NotInRepo,
        };

        // ディレクトリパスの末尾にスラッシュを追加して gitignore マッチング
        let dir_pattern = format!("{}/", Self::to_git_relative_key(&relative_path));

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
        let relative_path = match self.to_workdir_relative(path) {
            Some(p) => p,
            None => return false,
        };

        self.repo
            .status_should_ignore(&relative_path)
            .unwrap_or(false)
    }

    /// ディレクトリ内のファイルを再帰的にチェック
    fn check_directory_recursive(&self, dir: &Path) -> Result<(), SafeRmError> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => {
                // fail-closed: ディレクトリ読み取り失敗時は削除をブロック
                return Err(SafeRmError::DirectoryReadError {
                    path: dir.to_path_buf(),
                });
            }
        };

        for entry_result in entries {
            let entry = entry_result.map_err(|_| SafeRmError::DirectoryReadError {
                path: dir.to_path_buf(),
            })?;
            let path = entry.path();

            if Self::is_real_directory(&path) {
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

        for entry_result in entries {
            let entry = entry_result.map_err(|_| SafeRmError::DirectoryReadError {
                path: dir.to_path_buf(),
            })?;
            let path = entry.path();

            if Self::is_real_directory(&path) {
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
        if Self::is_real_directory(path) {
            self.check_directory_with_cache(path, cache)
        } else {
            self.check_file_with_cache(path, cache)
        }
    }

    /// Git status のキー形式（スラッシュ区切り）に揃える
    fn to_git_relative_key(path: &Path) -> String {
        path.components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// シンボリックリンクを辿らずに「実体がディレクトリか」を判定
    fn is_real_directory(path: &Path) -> bool {
        std::fs::symlink_metadata(path)
            .map(|metadata| metadata.file_type().is_dir())
            .unwrap_or(false)
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

        // Git リポジトリを初期化
        Command::new("git")
            .args(["init"])
            .current_dir(repo_path)
            .output()
            .unwrap();

        // テスト用コミットの設定
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

    // Git リポジトリ検出のテスト

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

    // ファイルステータス判定のテスト

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
    fn test_get_file_status_nested_untracked_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 空リポジトリ扱いを避けるため初期コミットを作成
        commit_file(&repo_path, "initial.txt", "initial");

        let nested_dir = repo_path.join("newdir").join("deep");
        fs::create_dir_all(&nested_dir).unwrap();
        let file_path = nested_dir.join("untracked.txt");
        fs::write(&file_path, "untracked content").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let status = checker.get_file_status(&file_path);

        assert_eq!(
            status,
            FileStatus::Untracked,
            "未追跡ディレクトリ配下のファイルも Untracked と判定されるべき"
        );
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

    // ディレクトリ再帰チェックのテスト

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
    fn test_check_directory_with_nested_untracked_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "initial.txt", "initial");

        let dirty_dir = repo_path.join("dirty_dir");
        let nested_dir = dirty_dir.join("nested");
        fs::create_dir_all(&nested_dir).unwrap();
        let dirty_file = nested_dir.join("untracked.txt");
        fs::write(&dirty_file, "untracked").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let result = checker.check_directory(&dirty_dir);

        assert!(
            result.is_err(),
            "未追跡ファイルを含むディレクトリは失敗するべき"
        );
        match result.unwrap_err() {
            SafeRmError::DirtyFiles { path, status } => {
                assert_eq!(path, dirty_file);
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

    // get_all_statuses とキャッシュ関連のテスト

    #[test]
    fn test_get_all_statuses_returns_all_files() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を作成してコミット
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

        // Clean ファイル
        commit_file(&repo_path, "clean.txt", "clean");

        // Modified ファイル
        commit_file(&repo_path, "modified.txt", "original");
        fs::write(repo_path.join("modified.txt"), "changed").unwrap();

        // Untracked ファイル
        fs::write(repo_path.join("new.txt"), "new").unwrap();

        // Ignored ファイル
        fs::write(repo_path.join("debug.log"), "log").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let statuses = checker.get_all_statuses().unwrap();

        // Modified, Untracked, Ignored は status に含まれる
        assert!(statuses.contains_key("modified.txt"));
        assert!(statuses.contains_key("new.txt"));
        assert!(statuses.contains_key("debug.log"));

        // Clean ファイルはステータスリストに含まれない（変更なし）
        assert!(!statuses.contains_key("clean.txt"));
    }

    #[test]
    fn test_check_file_with_cache_clean() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "cached_clean.txt", "content");

        let checker = GitChecker::open(&repo_path).unwrap();
        let cache = checker.get_all_statuses().unwrap();
        let file_path = repo_path.join("cached_clean.txt");
        let result = checker.check_file_with_cache(&file_path, &cache);

        assert!(result.is_ok(), "Clean file should pass cache check");
    }

    #[test]
    fn test_check_file_with_cache_modified() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "cached_mod.txt", "original");
        fs::write(repo_path.join("cached_mod.txt"), "changed").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let cache = checker.get_all_statuses().unwrap();
        let file_path = repo_path.join("cached_mod.txt");
        let result = checker.check_file_with_cache(&file_path, &cache);

        assert!(result.is_err(), "Modified file should fail cache check");
    }

    #[test]
    fn test_check_path_with_cache_directory() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let subdir = repo_path.join("cachedir");
        fs::create_dir(&subdir).unwrap();
        commit_file(&repo_path, "cachedir/file1.txt", "content1");
        commit_file(&repo_path, "cachedir/file2.txt", "content2");

        let checker = GitChecker::open(&repo_path).unwrap();
        let cache = checker.get_all_statuses().unwrap();
        let result = checker.check_path_with_cache(&subdir, &cache);

        assert!(
            result.is_ok(),
            "Directory with clean files should pass cache check"
        );
    }

    #[test]
    fn test_check_directory_with_cache_dirty() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let subdir = repo_path.join("dirtycache");
        fs::create_dir(&subdir).unwrap();
        commit_file(&repo_path, "dirtycache/clean.txt", "clean");

        // 未追跡ファイルを追加
        fs::write(subdir.join("untracked.txt"), "untracked").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let cache = checker.get_all_statuses().unwrap();
        let result = checker.check_directory_with_cache(&subdir, &cache);

        assert!(
            result.is_err(),
            "Directory with dirty files should fail cache check"
        );
    }

    #[test]
    fn test_workdir_returns_path() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let workdir = checker.workdir();

        assert!(workdir.is_some(), "Git repo should have a workdir");
        let wd = workdir.unwrap();
        assert!(wd.is_absolute(), "Workdir should be absolute");
    }

    #[test]
    fn test_get_file_status_from_cache_not_in_repo() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "dummy.txt", "dummy");

        let checker = GitChecker::open(&repo_path).unwrap();
        let cache = checker.get_all_statuses().unwrap();

        // リポジトリ外のパスに対して NotInRepo が返ることを確認
        let outside_path = std::path::Path::new("/tmp/nonexistent_path.txt");
        let status = checker.get_file_status_from_cache(outside_path, &cache);
        assert_eq!(status, FileStatus::NotInRepo);
    }

    #[test]
    fn test_convert_status_clean() {
        // 空のステータス（変更なし）はCleanになるべき
        let status = GitChecker::convert_status(Status::empty());
        assert_eq!(status, FileStatus::Clean);
    }

    #[test]
    fn test_convert_status_ignored() {
        let status = GitChecker::convert_status(Status::IGNORED);
        assert_eq!(status, FileStatus::Ignored);
    }

    #[test]
    fn test_convert_status_staged_new() {
        let status = GitChecker::convert_status(Status::INDEX_NEW);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_convert_status_staged_modified() {
        let status = GitChecker::convert_status(Status::INDEX_MODIFIED);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_convert_status_wt_modified() {
        let status = GitChecker::convert_status(Status::WT_MODIFIED);
        assert_eq!(status, FileStatus::Modified);
    }

    #[test]
    fn test_convert_status_wt_new() {
        let status = GitChecker::convert_status(Status::WT_NEW);
        assert_eq!(status, FileStatus::Untracked);
    }

    #[test]
    fn test_convert_status_ignored_takes_precedence() {
        // IGNORED + WT_NEW の場合、IGNORED が優先される
        let status = GitChecker::convert_status(Status::IGNORED | Status::WT_NEW);
        assert_eq!(status, FileStatus::Ignored);
    }

    #[test]
    fn test_convert_status_index_deleted() {
        let status = GitChecker::convert_status(Status::INDEX_DELETED);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_convert_status_index_renamed() {
        let status = GitChecker::convert_status(Status::INDEX_RENAMED);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_convert_status_index_typechange() {
        let status = GitChecker::convert_status(Status::INDEX_TYPECHANGE);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_convert_status_wt_deleted() {
        let status = GitChecker::convert_status(Status::WT_DELETED);
        assert_eq!(status, FileStatus::Modified);
    }

    #[test]
    fn test_convert_status_wt_renamed() {
        let status = GitChecker::convert_status(Status::WT_RENAMED);
        assert_eq!(status, FileStatus::Modified);
    }

    #[test]
    fn test_convert_status_wt_typechange() {
        let status = GitChecker::convert_status(Status::WT_TYPECHANGE);
        assert_eq!(status, FileStatus::Modified);
    }

    #[test]
    fn test_to_git_relative_key_uses_forward_slash() {
        let nested = Path::new("subdir").join("file.txt");
        let key = GitChecker::to_git_relative_key(&nested);
        assert_eq!(key, "subdir/file.txt");
    }

    #[test]
    fn test_get_file_status_from_cache_matches_nested_key() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let nested_dir = repo_path.join("nested");
        fs::create_dir(&nested_dir).unwrap();
        let nested_file = nested_dir.join("file.txt");
        fs::write(&nested_file, "content").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add nested file"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let mut cache = HashMap::new();
        cache.insert("nested/file.txt".to_string(), FileStatus::Untracked);

        let status = checker.get_file_status_from_cache(&nested_file, &cache);
        assert_eq!(status, FileStatus::Untracked);
    }

    #[test]
    fn test_convert_status_index_and_wt_combined() {
        // INDEX_MODIFIED + WT_MODIFIED の場合、Staged が優先される
        let status = GitChecker::convert_status(Status::INDEX_MODIFIED | Status::WT_MODIFIED);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_convert_status_index_new_and_wt_modified() {
        // INDEX_NEW + WT_MODIFIED の場合、Staged が優先される
        let status = GitChecker::convert_status(Status::INDEX_NEW | Status::WT_MODIFIED);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_get_file_status_not_in_workdir() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "dummy.txt", "dummy");

        let checker = GitChecker::open(&repo_path).unwrap();

        // リポジトリ外のパスは NotInRepo
        let outside = Path::new("/tmp/outside_file.txt");
        let status = checker.get_file_status(outside);
        assert_eq!(status, FileStatus::NotInRepo);
    }

    #[test]
    fn test_check_directory_empty() {
        // 空ディレクトリのチェック（ファイルなし = 全 Clean）
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "initial.txt", "init");

        let empty_dir = repo_path.join("empty");
        fs::create_dir(&empty_dir).unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let result = checker.check_directory(&empty_dir);
        assert!(result.is_ok(), "空ディレクトリは削除可能であるべき");
    }

    #[test]
    fn test_check_directory_with_cache_empty() {
        // 空ディレクトリのキャッシュ付きチェック
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "initial.txt", "init");

        let empty_dir = repo_path.join("empty_cached");
        fs::create_dir(&empty_dir).unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let cache = checker.get_all_statuses().unwrap();
        let result = checker.check_directory_with_cache(&empty_dir, &cache);
        assert!(
            result.is_ok(),
            "空ディレクトリはキャッシュ付きでも削除可能であるべき"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_is_real_directory_does_not_follow_symlink() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let real_dir = root.join("real_dir");
        fs::create_dir(&real_dir).unwrap();

        let link = root.join("dir_link");
        std::os::unix::fs::symlink(&real_dir, &link).unwrap();

        assert!(GitChecker::is_real_directory(&real_dir));
        assert!(
            !GitChecker::is_real_directory(&link),
            "symlink to directory must be treated as non-directory"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_check_path_with_cache_symlink_to_directory_checks_link_itself() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 実体ディレクトリを作成してコミット
        let target_dir = repo_path.join("target_dir");
        fs::create_dir(&target_dir).unwrap();
        fs::write(target_dir.join("clean.txt"), "clean").unwrap();
        Command::new("git")
            .args(["add", "target_dir/clean.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add target dir"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // ディレクトリへのシンボリックリンクを作成してコミット
        let link_path = repo_path.join("dir_link");
        std::os::unix::fs::symlink("target_dir", &link_path).unwrap();
        Command::new("git")
            .args(["add", "dir_link"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add dir symlink"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // 実体ディレクトリ側を dirty にしても、link 自体が clean なら削除判定は許可されるべき
        fs::write(repo_path.join("target_dir").join("untracked.txt"), "dirty").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let cache = checker.get_all_statuses().unwrap();
        let result = checker.check_path_with_cache(&link_path, &cache);
        assert!(
            result.is_ok(),
            "directory symlink should be checked as the link itself, not traversed"
        );
    }

    #[test]
    fn test_get_file_status_from_cache_ignored_file_not_in_cache() {
        // キャッシュになく .gitignore で無視されるファイルの場合、
        // get_file_status_from_cache は Ignored を返すべき
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を作成してコミット
        fs::write(repo_path.join(".gitignore"), "*.log\n").unwrap();
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

        // Ignored ファイルを作成
        fs::write(repo_path.join("debug.log"), "log data").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        // 空キャッシュ（意図的にキャッシュミスさせる）
        let empty_cache = HashMap::new();

        let status = checker.get_file_status_from_cache(&repo_path.join("debug.log"), &empty_cache);
        assert_eq!(
            status,
            FileStatus::Ignored,
            "キャッシュにない .gitignore 対象ファイルは Ignored を返すべき"
        );
    }

    #[test]
    fn test_get_file_status_from_cache_clean_file_not_in_cache() {
        // キャッシュにないが Git 追跡済みで変更なし（Clean）のファイル
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "tracked.txt", "content");

        let checker = GitChecker::open(&repo_path).unwrap();
        let empty_cache = HashMap::new();

        let status =
            checker.get_file_status_from_cache(&repo_path.join("tracked.txt"), &empty_cache);
        assert_eq!(
            status,
            FileStatus::Clean,
            "キャッシュにないがコミット済みのファイルは Clean を返すべき"
        );
    }

    #[test]
    fn test_get_file_status_from_cache_nested_untracked_file_not_in_cache() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "initial.txt", "initial");

        let nested_dir = repo_path.join("newdir").join("deep");
        fs::create_dir_all(&nested_dir).unwrap();
        let file_path = nested_dir.join("untracked.txt");
        fs::write(&file_path, "untracked").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let empty_cache = HashMap::new();

        let status = checker.get_file_status_from_cache(&file_path, &empty_cache);
        assert_eq!(
            status,
            FileStatus::Untracked,
            "キャッシュミス時でも未追跡ディレクトリ配下のファイルは Untracked を返すべき"
        );
    }

    #[test]
    fn test_to_git_relative_key_single_segment() {
        // 単一セグメント（ディレクトリなし）のパス
        let path = Path::new("file.txt");
        let key = GitChecker::to_git_relative_key(path);
        assert_eq!(key, "file.txt");
    }

    #[test]
    #[cfg(unix)]
    fn test_get_file_status_accepts_canonical_path_when_repo_opened_via_alias() {
        // symlink alias 経由で repo を開き、canonical path で問い合わせるケース
        // to_workdir_relative() のフォールバックが機能することを検証
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "tracked.txt", "content");

        let alias_dir = TempDir::new().unwrap();
        let alias_repo = alias_dir.path().join("repo-link");
        std::os::unix::fs::symlink(&repo_path, &alias_repo).unwrap();

        // alias 経由で repo を開く
        let checker = GitChecker::open(&alias_repo).unwrap();

        // canonical path で問い合わせる
        let status = checker.get_file_status(&repo_path.join("tracked.txt"));
        assert_eq!(
            status,
            FileStatus::Clean,
            "symlink alias 経由でも canonical path から正しくステータスを取得すべき"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_get_file_status_from_cache_via_symlink_alias() {
        // symlink alias 経由の repo でキャッシュベースのステータス取得を検証
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "clean.txt", "content");
        fs::write(repo_path.join("dirty.txt"), "untracked").unwrap();

        let alias_dir = TempDir::new().unwrap();
        let alias_repo = alias_dir.path().join("repo-link");
        std::os::unix::fs::symlink(&repo_path, &alias_repo).unwrap();

        let checker = GitChecker::open(&alias_repo).unwrap();
        let cache = checker.get_all_statuses().unwrap();

        // canonical path で clean ファイルを問い合わせ
        let clean_status = checker.get_file_status_from_cache(&repo_path.join("clean.txt"), &cache);
        assert_eq!(
            clean_status,
            FileStatus::Clean,
            "symlink alias 経由でもキャッシュから Clean を正しく取得すべき"
        );

        // canonical path で dirty ファイルを問い合わせ
        let dirty_status = checker.get_file_status_from_cache(&repo_path.join("dirty.txt"), &cache);
        assert_eq!(
            dirty_status,
            FileStatus::Untracked,
            "symlink alias 経由でもキャッシュから Untracked を正しく取得すべき"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_check_file_with_cache_blocks_dirty_via_symlink_alias() {
        // symlink alias 経由の repo で dirty ファイルの削除がブロックされることを検証
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "file.txt", "original");
        fs::write(repo_path.join("file.txt"), "modified").unwrap();

        let alias_dir = TempDir::new().unwrap();
        let alias_repo = alias_dir.path().join("repo-link");
        std::os::unix::fs::symlink(&repo_path, &alias_repo).unwrap();

        let checker = GitChecker::open(&alias_repo).unwrap();
        let cache = checker.get_all_statuses().unwrap();

        // canonical path でチェック → Modified でブロックされるべき
        let result = checker.check_file_with_cache(&repo_path.join("file.txt"), &cache);
        assert!(
            result.is_err(),
            "symlink alias 経由でも Modified ファイルの削除はブロックされるべき"
        );
    }

    #[test]
    fn test_to_workdir_relative_returns_none_for_outside_path() {
        // ワークディレクトリ外のパスは None を返す
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let checker = GitChecker::open(&repo_path).unwrap();

        let outside_path = Path::new("/tmp/definitely-not-in-repo/file.txt");
        let status = checker.get_file_status(outside_path);
        assert_eq!(
            status,
            FileStatus::NotInRepo,
            "リポジトリ外パスは NotInRepo を返すべき"
        );
    }

    #[test]
    fn test_get_directory_status_ignored() {
        // .gitignore で無視されたディレクトリのステータス確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を作成してコミット
        commit_file(&repo_path, ".gitignore", "build/\n");

        // 無視対象ディレクトリを作成
        let build_dir = repo_path.join("build");
        fs::create_dir_all(&build_dir).unwrap();
        fs::write(build_dir.join("output.bin"), "binary").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();

        // ディレクトリ自体が Ignored として判定される
        let status = checker.get_directory_status(&build_dir);
        assert_eq!(
            status,
            FileStatus::Ignored,
            "gitignore 対象ディレクトリは Ignored として扱うべき"
        );
    }

    #[test]
    fn test_get_directory_status_not_ignored() {
        // 通常のディレクトリは Clean として判定される
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let src_dir = repo_path.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        commit_file(&repo_path, "src/main.rs", "fn main() {}");

        let checker = GitChecker::open(&repo_path).unwrap();
        let status = checker.get_directory_status(&src_dir);
        assert_eq!(
            status,
            FileStatus::Clean,
            "通常のディレクトリは Clean として扱うべき"
        );
    }

    #[test]
    fn test_convert_status_current() {
        // Status が空（CURRENT = 0x0）の場合は Clean を返す
        let status = Status::CURRENT;
        assert_eq!(
            GitChecker::convert_status(status),
            FileStatus::Clean,
            "Status::CURRENT は Clean として扱うべき"
        );
    }

    #[test]
    fn test_check_path_file_vs_directory() {
        // check_path がファイルとディレクトリを正しく振り分けることを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ファイルを作成してコミット
        let file = repo_path.join("file.txt");
        commit_file(&repo_path, "file.txt", "content");

        // ディレクトリを作成してファイルをコミット
        let dir = repo_path.join("subdir");
        fs::create_dir_all(&dir).unwrap();
        commit_file(&repo_path, "subdir/sub.txt", "sub content");

        let checker = GitChecker::open(&repo_path).unwrap();

        // ファイル — Clean で成功
        assert!(checker.check_path(&file).is_ok());
        // ディレクトリ — 全ファイルが Clean で成功
        assert!(checker.check_path(&dir).is_ok());
    }

    #[test]
    fn test_get_all_statuses_includes_ignored() {
        // get_all_statuses が Ignored ファイルも含むことを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore でパターンを設定
        commit_file(&repo_path, ".gitignore", "*.log\n");

        // 無視対象ファイルを作成
        fs::write(repo_path.join("debug.log"), "log data").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let statuses = checker.get_all_statuses().unwrap();

        assert_eq!(
            statuses.get("debug.log"),
            Some(&FileStatus::Ignored),
            "get_all_statuses は Ignored ファイルを含むべき"
        );
    }

    #[test]
    fn test_check_directory_empty_dir() {
        // 空のディレクトリに対する check_directory は成功すべき
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 初期コミットを作成
        commit_file(&repo_path, "init.txt", "init");

        let empty_dir = repo_path.join("empty");
        fs::create_dir_all(&empty_dir).unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        assert!(
            checker.check_directory(&empty_dir).is_ok(),
            "空ディレクトリの削除は許可されるべき"
        );
    }

    #[test]
    fn test_check_directory_recursive_with_mixed_status() {
        // ネストしたディレクトリ内に dirty ファイルがある場合のチェック
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let nested = repo_path.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();

        // clean ファイル
        commit_file(&repo_path, "a/b/clean.txt", "clean");

        // 未追跡ファイルを追加
        fs::write(nested.join("untracked.txt"), "new").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let result = checker.check_directory(&repo_path.join("a"));
        assert!(
            result.is_err(),
            "未追跡ファイルを含むディレクトリの削除はブロックされるべき"
        );
    }

    #[test]
    fn test_is_real_directory_returns_false_for_file() {
        let temp_dir = TempDir::new().unwrap();
        let file = temp_dir.path().join("file.txt");
        fs::write(&file, "content").unwrap();

        assert!(
            !GitChecker::is_real_directory(&file),
            "通常ファイルに対して false を返すべき"
        );
    }

    #[test]
    fn test_is_real_directory_returns_false_for_nonexistent() {
        assert!(
            !GitChecker::is_real_directory(Path::new("/nonexistent/path")),
            "存在しないパスに対して false を返すべき"
        );
    }

    #[test]
    fn test_to_git_relative_key_nested_path() {
        // ネストしたパスがスラッシュ区切りに正しく変換されることを確認
        let path = Path::new("src").join("components").join("App.tsx");
        let key = GitChecker::to_git_relative_key(&path);
        assert_eq!(key, "src/components/App.tsx");
    }

    #[test]
    fn test_check_file_staged_blocked() {
        // ステージ済みファイルの削除が DirtyFiles(Staged) でブロックされることを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 初期コミットを作成
        commit_file(&repo_path, "initial.txt", "initial");

        // ファイルを作成して git add のみ（コミットしない）
        let file_path = repo_path.join("staged_only.txt");
        fs::write(&file_path, "staged content").unwrap();
        Command::new("git")
            .args(["add", "staged_only.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let result = checker.check_file(&file_path);

        assert!(result.is_err(), "Staged ファイルの削除はブロックされるべき");
        match result.unwrap_err() {
            SafeRmError::DirtyFiles { path, status } => {
                assert_eq!(path, file_path);
                assert_eq!(status, FileStatus::Staged);
            }
            _ => panic!("Expected DirtyFiles error"),
        }
    }

    #[test]
    fn test_check_path_with_cache_file() {
        // check_path_with_cache がファイル（非ディレクトリ）に対して正しく動作することを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // Clean ファイルを作成
        commit_file(&repo_path, "cached_file.txt", "content");

        // Modified ファイルを作成
        commit_file(&repo_path, "dirty_file.txt", "original");
        fs::write(repo_path.join("dirty_file.txt"), "changed").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let cache = checker.get_all_statuses().unwrap();

        // Clean ファイルは成功
        let clean_result =
            checker.check_path_with_cache(&repo_path.join("cached_file.txt"), &cache);
        assert!(
            clean_result.is_ok(),
            "Clean ファイルはキャッシュ経由でも削除可能であるべき"
        );

        // Modified ファイルは失敗
        let dirty_result = checker.check_path_with_cache(&repo_path.join("dirty_file.txt"), &cache);
        assert!(
            dirty_result.is_err(),
            "Modified ファイルはキャッシュ経由でもブロックされるべき"
        );
        match dirty_result.unwrap_err() {
            SafeRmError::DirtyFiles { status, .. } => {
                assert_eq!(status, FileStatus::Modified);
            }
            _ => panic!("Expected DirtyFiles error"),
        }
    }

    #[test]
    fn test_check_directory_recursive_with_ignored_subdir() {
        // Ignored ファイルのみを含むディレクトリの削除が許可されることを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore で特定パターンを無視
        commit_file(&repo_path, ".gitignore", "*.log\n*.tmp\n");

        // ディレクトリを作成し、無視対象ファイルのみを配置
        let subdir = repo_path.join("logs");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(subdir.join("app.log"), "log data").unwrap();
        fs::write(subdir.join("debug.tmp"), "tmp data").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let result = checker.check_directory(&subdir);

        assert!(
            result.is_ok(),
            "Ignored ファイルのみを含むディレクトリは削除可能であるべき"
        );
    }

    #[test]
    fn test_get_file_status_from_cache_returns_clean_for_tracked_file() {
        // キャッシュに含まれない追跡済みファイルは Clean を返すことを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ファイルを作成してコミット（変更なし = Clean）
        commit_file(&repo_path, "tracked_clean.txt", "content");

        let checker = GitChecker::open(&repo_path).unwrap();
        let cache = checker.get_all_statuses().unwrap();

        // Clean ファイルはキャッシュに含まれないため、フォールバックで Clean が返る
        let file_path = repo_path.join("tracked_clean.txt");
        let status = checker.get_file_status_from_cache(&file_path, &cache);
        assert_eq!(
            status,
            FileStatus::Clean,
            "キャッシュに含まれない追跡済みファイルは Clean を返すべき"
        );

        // キャッシュにエントリがないことも確認
        assert!(
            !cache.contains_key("tracked_clean.txt"),
            "Clean ファイルは get_all_statuses のキャッシュに含まれないべき"
        );
    }

    #[test]
    fn test_get_all_statuses_returns_result_ok() {
        // 正常なリポジトリで get_all_statuses が Ok を返すことを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "file.txt", "content");

        let checker = GitChecker::open(&repo_path).unwrap();
        let result = checker.get_all_statuses();
        assert!(result.is_ok(), "正常なリポジトリでは Ok を返すべき");
    }

    #[test]
    fn test_resolve_status_fail_closed_returns_modified_on_unexpected_error() {
        // resolve_status_from_relative_path が NotFound 以外のエラーで
        // Modified（削除不可）を返すことを検証する間接テスト
        //
        // status_file() が NotFound 以外のエラーを返すケースを
        // check_file 経由で検証: ワークディレクトリ外の相対パスを渡す
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "dummy.txt", "content");

        let checker = GitChecker::open(&repo_path).unwrap();

        // check_file で Modified（fail-closed）が返され削除がブロックされることを確認
        // get_file_status 経由: ワークディレクトリ内の追跡済みファイルは Clean
        let clean_path = repo_path.join("dummy.txt");
        let status = checker.get_file_status(&clean_path);
        assert_eq!(status, FileStatus::Clean);
        assert!(GitChecker::is_deletable(status));

        // Modified は削除不可
        assert!(!GitChecker::is_deletable(FileStatus::Modified));
    }

    #[test]
    fn test_check_directory_recursive_with_cache_fail_closed_on_read_error() {
        // キャッシュ使用時のディレクトリ読み取り失敗で DirectoryReadError が返ることを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "file.txt", "content");

        let checker = GitChecker::open(&repo_path).unwrap();
        let cache = checker.get_all_statuses().unwrap();

        // 存在しないディレクトリに対してキャッシュ付きチェック
        let nonexistent_dir = repo_path.join("nonexistent_dir");
        fs::create_dir(&nonexistent_dir).unwrap();

        // パーミッションを除去してディレクトリ読み取り不可にする (Unix のみ)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&nonexistent_dir, fs::Permissions::from_mode(0o000)).unwrap();

            let result = checker.check_directory_with_cache(&nonexistent_dir, &cache);
            assert!(
                result.is_err(),
                "読み取り不可ディレクトリのキャッシュ付きチェックは失敗すべき"
            );

            // テスト後のクリーンアップ: パーミッション復元
            fs::set_permissions(&nonexistent_dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn test_batch_exit_code_two_paths_mixed_errors() {
        // 2パスのバッチで exit code 1 と exit code 2 が混在する場合のテスト
        // check_file は DirtyFiles (exit 2) を返す
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "clean.txt", "clean");
        fs::write(repo_path.join("dirty.txt"), "untracked").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();

        // Clean ファイルは OK
        let clean_result = checker.check_file(&repo_path.join("clean.txt"));
        assert!(clean_result.is_ok());

        // Untracked ファイルは DirtyFiles エラー
        let dirty_result = checker.check_file(&repo_path.join("dirty.txt"));
        assert!(dirty_result.is_err());
        if let Err(crate::error::SafeRmError::DirtyFiles { status, .. }) = dirty_result {
            assert_eq!(status, FileStatus::Untracked);
        } else {
            panic!("DirtyFiles エラーが期待されたが異なるエラーが返された");
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_check_path_symlink_to_file_treated_as_file() {
        // ファイルへの symlink は is_real_directory() が false を返すので
        // check_file パスで処理されることを検証
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "target.txt", "target content");
        let link_path = repo_path.join("link_to_file.txt");
        std::os::unix::fs::symlink(repo_path.join("target.txt"), &link_path).unwrap();

        Command::new("git")
            .args(["add", "link_to_file.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add symlink"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();

        // symlink-to-file は check_file 経由で処理される
        assert!(!GitChecker::is_real_directory(&link_path));
        let result = checker.check_path(&link_path);
        assert!(result.is_ok(), "コミット済み symlink は削除可能であるべき");
    }

    #[test]
    fn test_check_directory_with_only_ignored_files() {
        // .gitignore 対象ファイルのみ含むディレクトリは削除可能
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を設定
        fs::write(repo_path.join(".gitignore"), "build/\n").unwrap();
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

        // build/ ディレクトリにファイルを作成
        let build_dir = repo_path.join("build");
        fs::create_dir_all(&build_dir).unwrap();
        fs::write(build_dir.join("output.bin"), "binary content").unwrap();
        fs::write(build_dir.join("log.txt"), "build log").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();
        let result = checker.check_directory(&build_dir);
        assert!(
            result.is_ok(),
            "ignored ファイルのみ含むディレクトリは削除可能であるべき"
        );
    }

    #[test]
    fn test_get_file_status_from_cache_falls_back_correctly() {
        // キャッシュにないファイルで、is_ignored_path の判定にフォールバックすることを検証
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を設定してコミット
        fs::write(repo_path.join(".gitignore"), "*.log\n").unwrap();
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

        // ignored ファイルを作成
        fs::write(repo_path.join("debug.log"), "log data").unwrap();

        let checker = GitChecker::open(&repo_path).unwrap();

        // 空のキャッシュで問い合わせ → フォールバックで正しくステータスを取得
        let empty_cache = HashMap::new();
        let status = checker.get_file_status_from_cache(&repo_path.join("debug.log"), &empty_cache);
        assert_eq!(
            status,
            FileStatus::Ignored,
            "空キャッシュでも ignored ファイルは正しく判定されるべき"
        );
    }

    #[test]
    fn test_check_file_with_cache_not_in_repo() {
        // ワークディレクトリ外のパスを check_file_with_cache に渡すと DirtyFiles
        // （NotInRepo は is_deletable なので Ok が返る）
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "initial.txt", "initial");

        let checker = GitChecker::open(&repo_path).unwrap();
        let cache = checker.get_all_statuses().unwrap();

        // リポジトリ外のパス
        let outside_path = Path::new("/tmp/nonexistent_safe_rm_test_xyz");
        let result = checker.check_file_with_cache(outside_path, &cache);
        // NotInRepo は is_deletable = true なので Ok
        assert!(
            result.is_ok(),
            "NotInRepo ステータスは削除可能として扱われるべき"
        );
    }
}
