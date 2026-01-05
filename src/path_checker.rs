//! Path validation for safe-rm
//!
//! Normalizes paths and verifies project boundary containment.

use crate::error::SafeRmError;
use path_clean::PathClean;
use std::path::{Path, PathBuf};

/// パス検証器
pub struct PathChecker;

impl PathChecker {
    /// パスがプロジェクトルート内にあることを検証
    ///
    /// # Arguments
    /// * `project_root` - プロジェクトルートの絶対パス
    /// * `target_path` - 検証対象のパス（相対または絶対）
    ///
    /// # Returns
    /// * `Ok(PathBuf)` - 正規化された絶対パス
    /// * `Err(SafeRmError::OutsideProject)` - プロジェクト外へのアクセス
    pub fn verify_containment(
        project_root: &Path,
        target_path: &Path,
    ) -> Result<PathBuf, SafeRmError> {
        // 1. パスを絶対パスに変換
        let absolute_path = Self::to_absolute(project_root, target_path);

        // 2. 字句的に正規化（.. を解決）
        let cleaned_path = absolute_path.clean();

        // 3. 可能であればシンボリックリンクを解決
        let canonical_path = Self::try_canonicalize(&cleaned_path);

        // 4. プロジェクトルートも正規化
        let canonical_root = Self::try_canonicalize(&project_root.clean());

        // 5. 境界チェック
        if !Self::is_contained(&canonical_root, &canonical_path) {
            return Err(SafeRmError::OutsideProject {
                path: target_path.to_path_buf(),
                project_root: project_root.to_path_buf(),
            });
        }

        Ok(canonical_path)
    }

    /// 相対パスを絶対パスに変換
    fn to_absolute(base: &Path, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        }
    }

    /// 可能であれば canonicalize、失敗時は元のパスを返す
    fn try_canonicalize(path: &Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }

    /// パスがルート内に含まれているかチェック
    fn is_contained(root: &Path, path: &Path) -> bool {
        // パスがルートと同一か、ルートの子孫である
        path.starts_with(root)
    }

    /// ホームディレクトリへの参照をチェック
    #[allow(dead_code)]
    fn is_home_reference(path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        path_str.starts_with("~/") || path_str == "~"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Task 5.1: パス正規化処理のテスト

    #[test]
    fn test_verify_containment_relative_path() {
        let temp_dir = TempDir::new().unwrap();
        // canonicalize to get the real path (handles /private on macOS)
        let project_root = temp_dir.path().canonicalize().unwrap();

        // テスト用ファイルを作成
        let file_path = project_root.join("test.txt");
        fs::write(&file_path, "test").unwrap();

        let result = PathChecker::verify_containment(&project_root, Path::new("test.txt"));
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with(&project_root));
    }

    #[test]
    fn test_verify_containment_absolute_path_inside() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // テスト用ファイルを作成
        let file_path = project_root.join("test.txt");
        fs::write(&file_path, "test").unwrap();

        let result = PathChecker::verify_containment(&project_root, &file_path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_containment_nested_path() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // ネストしたディレクトリを作成
        let nested_dir = project_root.join("src").join("components");
        fs::create_dir_all(&nested_dir).unwrap();
        let file_path = nested_dir.join("test.tsx");
        fs::write(&file_path, "test").unwrap();

        let result =
            PathChecker::verify_containment(&project_root, Path::new("src/components/test.tsx"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_containment_with_dot_dot_inside() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // テスト用ファイルを作成
        let file_path = project_root.join("test.txt");
        fs::write(&file_path, "test").unwrap();

        // src/../test.txt はプロジェクト内
        let result = PathChecker::verify_containment(&project_root, Path::new("src/../test.txt"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_containment_nonexistent_file() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // 存在しないファイルでも、パス自体がプロジェクト内ならOK
        let result = PathChecker::verify_containment(&project_root, Path::new("nonexistent.txt"));
        assert!(result.is_ok());
    }

    // Task 5.2: プロジェクト境界チェックのテスト

    #[test]
    fn test_verify_containment_outside_project() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // プロジェクト外の絶対パス
        let result = PathChecker::verify_containment(&project_root, Path::new("/etc/passwd"));
        assert!(result.is_err());

        match result.unwrap_err() {
            SafeRmError::OutsideProject { .. } => (),
            _ => panic!("Expected OutsideProject error"),
        }
    }

    #[test]
    fn test_verify_containment_traversal_attack() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // ディレクトリトラバーサル攻撃
        let result =
            PathChecker::verify_containment(&project_root, Path::new("../../../etc/passwd"));
        assert!(result.is_err());

        match result.unwrap_err() {
            SafeRmError::OutsideProject { .. } => (),
            _ => panic!("Expected OutsideProject error"),
        }
    }

    #[test]
    fn test_verify_containment_deep_traversal() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // 深いディレクトリからのトラバーサル
        let result = PathChecker::verify_containment(
            &project_root,
            Path::new("a/b/c/d/e/../../../../../.."),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_containment_parent_directory() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // 単純な親ディレクトリ参照
        let result = PathChecker::verify_containment(&project_root, Path::new(".."));
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_containment_symlink_inside() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // ターゲットファイルを作成
        let target_file = project_root.join("target.txt");
        fs::write(&target_file, "test").unwrap();

        // プロジェクト内へのシンボリックリンクを作成
        let link_path = project_root.join("link.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target_file, &link_path).unwrap();

        #[cfg(unix)]
        {
            let result = PathChecker::verify_containment(&project_root, &link_path);
            assert!(result.is_ok());
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_verify_containment_symlink_outside() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // 外部ディレクトリを作成
        let outside_dir = TempDir::new().unwrap();
        let outside_file = outside_dir.path().join("outside.txt");
        fs::write(&outside_file, "outside").unwrap();

        // プロジェクト外へのシンボリックリンクを作成
        let link_path = project_root.join("evil_link.txt");
        std::os::unix::fs::symlink(&outside_file, &link_path).unwrap();

        let result = PathChecker::verify_containment(&project_root, &link_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_to_absolute_relative() {
        let base = Path::new("/project");
        let path = Path::new("src/main.rs");
        let result = PathChecker::to_absolute(base, path);
        assert_eq!(result, PathBuf::from("/project/src/main.rs"));
    }

    #[test]
    fn test_to_absolute_already_absolute() {
        let base = Path::new("/project");
        let path = Path::new("/etc/passwd");
        let result = PathChecker::to_absolute(base, path);
        assert_eq!(result, PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn test_is_contained_same_path() {
        let root = Path::new("/project");
        let path = Path::new("/project");
        assert!(PathChecker::is_contained(root, path));
    }

    #[test]
    fn test_is_contained_child_path() {
        let root = Path::new("/project");
        let path = Path::new("/project/src/main.rs");
        assert!(PathChecker::is_contained(root, path));
    }

    #[test]
    fn test_is_contained_outside_path() {
        let root = Path::new("/project");
        let path = Path::new("/other/file.txt");
        assert!(!PathChecker::is_contained(root, path));
    }

    #[test]
    fn test_is_contained_sibling_path() {
        let root = Path::new("/project");
        let path = Path::new("/project2/file.txt");
        assert!(!PathChecker::is_contained(root, path));
    }

    #[test]
    fn test_is_home_reference() {
        assert!(PathChecker::is_home_reference(Path::new("~")));
        assert!(PathChecker::is_home_reference(Path::new("~/")));
        assert!(PathChecker::is_home_reference(Path::new("~/Documents")));
        assert!(!PathChecker::is_home_reference(Path::new("/home/user")));
        assert!(!PathChecker::is_home_reference(Path::new("./file.txt")));
    }
}
