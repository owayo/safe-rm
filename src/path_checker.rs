//! safe-rm のパス検証
//!
//! パスの正規化とプロジェクト境界の包含検証を行う。

use crate::error::SafeRmError;
use path_clean::PathClean;
use std::path::{Path, PathBuf};

/// パス検証器
pub struct PathChecker;

impl PathChecker {
    /// パスがプロジェクトルート内にあることを検証
    ///
    /// # 引数
    /// * `project_root` - プロジェクト境界の絶対パス（Gitリポジトリルート等）
    /// * `target_path` - 検証対象のパス（相対または絶対）
    ///
    /// # 戻り値
    /// * `Ok(PathBuf)` - 正規化された絶対パス
    /// * `Err(SafeRmError::OutsideProject)` - プロジェクト外へのアクセス
    pub fn verify_containment(
        project_root: &Path,
        target_path: &Path,
    ) -> Result<PathBuf, SafeRmError> {
        Self::verify_containment_with_base(project_root, project_root, target_path)
    }

    /// パスがプロジェクトルート内にあることを検証（解決ベース指定）
    ///
    /// # 引数
    /// * `project_root` - プロジェクト境界の絶対パス（Gitリポジトリルート等）
    /// * `resolve_base` - 相対パスの解決基底（通常はカレントディレクトリ）
    /// * `target_path` - 検証対象のパス（相対または絶対）
    ///
    /// # 戻り値
    /// * `Ok(PathBuf)` - 正規化された絶対パス
    /// * `Err(SafeRmError::OutsideProject)` - プロジェクト外へのアクセス
    pub fn verify_containment_with_base(
        project_root: &Path,
        resolve_base: &Path,
        target_path: &Path,
    ) -> Result<PathBuf, SafeRmError> {
        // 1. パスを絶対パスに変換（相対パスは resolve_base から解決）
        let absolute_path = Self::to_absolute(resolve_base, target_path);

        // 2. 字句的に正規化（.. を解決）
        let cleaned_path = absolute_path.clean();

        // 3. 可能であればシンボリックリンクを解決
        //    末尾が未作成でも、既存の親ディレクトリまで解決してエイリアス差異を吸収する
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

    /// `..` の解決対象が「実体として存在する通常ディレクトリ」でないパスを拒否する。
    ///
    /// OS の path resolution は symlink を辿ってから `..` を解決し、対象が
    /// ディレクトリでなければ `ENOTDIR`、存在しなければ `ENOENT` で失敗するが、
    /// `path_clean` は字句的に `..` を畳み込むため、`link/../victim` や
    /// `missing/../victim`、`file/../victim` といったパスを許すと、OS では
    /// 到達できないはずの別ファイルを正規化後に削除してしまう。
    ///
    /// そのため `..` の直前成分は以下を必ず満たす必要がある:
    /// - 実体として存在する（メタデータ取得に成功）
    /// - 通常ディレクトリである（`is_dir() && !is_symlink()`）
    ///
    /// 上記を満たさない（symlink、通常ファイル、特殊ファイル、存在しない、
    /// メタデータ取得失敗）場合は `UnsafeTraversal` で拒否する。
    pub fn reject_symlink_parent_traversal(
        resolve_base: &Path,
        target_path: &Path,
    ) -> Result<(), SafeRmError> {
        let absolute_path = Self::to_absolute(resolve_base, target_path);
        let mut current = PathBuf::new();
        // 各成分が `..` で安全に解決できる「通常ディレクトリ」かを記録する。
        let mut component_is_traversable_dir = Vec::new();

        for component in absolute_path.components() {
            match component {
                std::path::Component::Prefix(prefix) => {
                    current.push(prefix.as_os_str());
                    component_is_traversable_dir.clear();
                }
                std::path::Component::RootDir => {
                    current.push(component.as_os_str());
                    component_is_traversable_dir.clear();
                }
                std::path::Component::CurDir => {}
                std::path::Component::Normal(name) => {
                    current.push(name);
                    // `..` で安全に解決するためには、対象成分が「実体として存在する
                    // 通常ディレクトリ」である必要がある。symlink、通常ファイル、
                    // 存在しない、特殊ファイル、メタデータ取得失敗は不可。
                    let is_traversable_dir = std::fs::symlink_metadata(&current)
                        .map(|metadata| {
                            let file_type = metadata.file_type();
                            file_type.is_dir() && !file_type.is_symlink()
                        })
                        .unwrap_or(false);
                    component_is_traversable_dir.push(is_traversable_dir);
                }
                std::path::Component::ParentDir => {
                    if let Some(was_traversable_dir) = component_is_traversable_dir.pop() {
                        if !was_traversable_dir {
                            return Err(SafeRmError::UnsafeTraversal {
                                path: target_path.to_path_buf(),
                            });
                        }
                        current.pop();
                    }
                    // stack 空のときの `..` は無視:
                    // - 絶対パスの `RootDir` 直後（Unix では `/..` は `/`）
                    // - 相対パスの先頭（基底ディレクトリの親を指すが、最終的な
                    //   包含検証で扱われる）
                }
            }
        }

        Ok(())
    }

    /// 相対パスを絶対パスに変換
    fn to_absolute(base: &Path, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        }
    }

    /// 可能であれば canonicalize する。
    /// 末尾が未作成で失敗した場合は、既存の親ディレクトリまで canonicalize してから
    /// 未作成部分を再結合する。
    fn try_canonicalize(path: &Path) -> PathBuf {
        if let Ok(canonical) = path.canonicalize() {
            return canonical;
        }

        let mut current = path;
        let mut missing_segments: Vec<std::ffi::OsString> = Vec::new();

        while let Some(parent) = current.parent() {
            if let Some(name) = current.file_name() {
                missing_segments.push(name.to_os_string());
            }

            if let Ok(canonical_parent) = parent.canonicalize() {
                let mut rebuilt = canonical_parent;
                for segment in missing_segments.iter().rev() {
                    rebuilt.push(segment);
                }
                return rebuilt;
            }

            current = parent;
        }

        path.to_path_buf()
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

    // パス正規化処理のテスト

    #[test]
    fn test_verify_containment_relative_path() {
        let temp_dir = TempDir::new().unwrap();
        // 実パスに揃える（macOS の /var → /private 差異を吸収）
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

    // プロジェクト境界チェックのテスト

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
    #[cfg(unix)]
    fn test_reject_symlink_parent_traversal_blocks_removed_symlink_component() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();
        let outside_dir = TempDir::new().unwrap();
        let outside_sub = outside_dir.path().join("sub");
        fs::create_dir_all(&outside_sub).unwrap();

        std::os::unix::fs::symlink(&outside_sub, project_root.join("link")).unwrap();

        let result = PathChecker::reject_symlink_parent_traversal(
            &project_root,
            Path::new("link/../victim.txt"),
        );

        assert!(matches!(result, Err(SafeRmError::UnsafeTraversal { .. })));
    }

    #[test]
    fn test_reject_symlink_parent_traversal_blocks_missing_intermediate() {
        // `missing/../victim.txt` のように、`..` の直前成分が存在しないパスは
        // OS の path resolution では `ENOENT` で失敗する。
        // path_clean による字句正規化で別ファイル削除に化けないよう拒否する。
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();
        fs::write(project_root.join("victim.txt"), "victim").unwrap();

        let result = PathChecker::reject_symlink_parent_traversal(
            &project_root,
            Path::new("missing_dir/../victim.txt"),
        );

        assert!(
            matches!(result, Err(SafeRmError::UnsafeTraversal { .. })),
            "存在しない中間成分の `..` は UnsafeTraversal で拒否すべき"
        );
    }

    #[test]
    fn test_reject_symlink_parent_traversal_blocks_file_as_intermediate() {
        // `file/../victim.txt` のように、`..` の直前成分が通常ファイルのパスは
        // OS の path resolution では `ENOTDIR` で失敗する。
        // path_clean による字句正規化で別ファイル削除に化けないよう拒否する。
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();
        fs::write(project_root.join("file"), "regular file").unwrap();
        fs::write(project_root.join("victim.txt"), "victim").unwrap();

        let result = PathChecker::reject_symlink_parent_traversal(
            &project_root,
            Path::new("file/../victim.txt"),
        );

        assert!(
            matches!(result, Err(SafeRmError::UnsafeTraversal { .. })),
            "通常ファイルが中間成分の `..` は UnsafeTraversal で拒否すべき"
        );
    }

    #[test]
    fn test_reject_symlink_parent_traversal_allows_normal_parent() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();
        fs::create_dir(project_root.join("sub")).unwrap();

        let result = PathChecker::reject_symlink_parent_traversal(
            &project_root,
            Path::new("sub/../victim.txt"),
        );

        assert!(result.is_ok());
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

    // --- verify_containment_with_base のテスト ---

    #[test]
    fn test_verify_containment_with_base_different_from_root() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // サブディレクトリを作成してベースとする
        let subdir = project_root.join("frontend");
        fs::create_dir(&subdir).unwrap();

        // 別のサブディレクトリのファイルを作成
        let backend = project_root.join("backend");
        fs::create_dir(&backend).unwrap();
        let file = backend.join("app.rs");
        fs::write(&file, "fn main() {}").unwrap();

        // frontend/ をベースとして backend/app.rs (絶対パス) を検証
        let result = PathChecker::verify_containment_with_base(&project_root, &subdir, &file);
        assert!(
            result.is_ok(),
            "Absolute path within project should pass even with different base"
        );
    }

    #[test]
    fn test_verify_containment_with_base_relative_resolved_from_base() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // subdir/file.txt を作成
        let subdir = project_root.join("subdir");
        fs::create_dir(&subdir).unwrap();
        let file = subdir.join("file.txt");
        fs::write(&file, "content").unwrap();

        // subdir をベースとして相対パス "file.txt" を検証
        let result = PathChecker::verify_containment_with_base(
            &project_root,
            &subdir,
            Path::new("file.txt"),
        );
        assert!(
            result.is_ok(),
            "Relative path resolved from subdir base should be within project"
        );
    }

    #[test]
    fn test_verify_containment_with_base_outside_via_relative() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        let subdir = project_root.join("deep").join("nested");
        fs::create_dir_all(&subdir).unwrap();

        // deep/nested/ から ../../../../etc/passwd を参照 → プロジェクト外
        let result = PathChecker::verify_containment_with_base(
            &project_root,
            &subdir,
            Path::new("../../../../etc/passwd"),
        );
        assert!(
            result.is_err(),
            "Traversal beyond project root should be blocked"
        );
    }

    #[test]
    fn test_verify_containment_project_root_itself() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // プロジェクトルート自身の削除判定は成功する
        // ルートは自身を starts_with するため包含チェックを通過する
        let result = PathChecker::verify_containment(&project_root, &project_root);
        assert!(
            result.is_ok(),
            "Project root itself should pass containment check"
        );
    }

    #[test]
    fn test_try_canonicalize_nonexistent_path() {
        // 存在しないパスでもフォールバックでパスが返る
        let path = Path::new("/nonexistent/path/to/file.txt");
        let result = PathChecker::try_canonicalize(path);
        assert_eq!(result, path.to_path_buf());
    }

    #[test]
    fn test_try_canonicalize_multiple_missing_segments() {
        // 既存の親ディレクトリから複数の未作成セグメントがある場合
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        let deep_nonexistent = project_root.join("a").join("b").join("c").join("file.txt");
        let result = PathChecker::try_canonicalize(&deep_nonexistent);

        // project_root は canonicalize 可能なので、そこから再結合される
        assert!(result.starts_with(&project_root));
        assert!(result.ends_with("a/b/c/file.txt") || result.ends_with("a\\b\\c\\file.txt"));
    }

    #[test]
    fn test_verify_containment_empty_path_component() {
        // 空のパスコンポーネントを含むケース
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        let file = project_root.join("test.txt");
        fs::write(&file, "content").unwrap();

        // "." はプロジェクトルート自体を指す
        let result = PathChecker::verify_containment(&project_root, Path::new("."));
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_containment_root_with_trailing_slash() {
        // project_root に末尾スラッシュがある場合でも正しく動作すること
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        // 末尾スラッシュ付きのパスを作成
        let root_with_slash = PathBuf::from(format!("{}/", project_root.display()));

        // テスト用ファイルを作成
        let file = project_root.join("file.txt");
        fs::write(&file, "content").unwrap();

        // 末尾スラッシュ付きルートでも包含チェックが通る
        let result = PathChecker::verify_containment(&root_with_slash, &file);
        assert!(
            result.is_ok(),
            "末尾スラッシュ付きの project_root でも包含チェックは成功すべき"
        );

        // プロジェクト外のパスは依然としてブロックされる
        let outside = PathChecker::verify_containment(&root_with_slash, Path::new("/etc/passwd"));
        assert!(
            outside.is_err(),
            "末尾スラッシュ付きでもプロジェクト外のパスはブロックすべき"
        );
    }

    #[test]
    fn test_is_contained_prefix_attack() {
        // "/project-evil" が "/project" の中に含まれないことを確認
        // starts_with はコンポーネント単位で比較するが、明示的にテスト
        let root = Path::new("/project");
        let evil_path = Path::new("/project-evil/file.txt");
        assert!(
            !PathChecker::is_contained(root, evil_path),
            "プレフィックス一致だがコンポーネントが異なるパスは含まれないべき"
        );

        // 類似のバリエーションも確認
        let evil_path2 = Path::new("/projectX/file.txt");
        assert!(
            !PathChecker::is_contained(root, evil_path2),
            "/projectX は /project の子ではない"
        );

        let evil_path3 = Path::new("/project.bak/file.txt");
        assert!(
            !PathChecker::is_contained(root, evil_path3),
            "/project.bak は /project の子ではない"
        );

        // 正当な子パスは通る
        let valid_child = Path::new("/project/src/main.rs");
        assert!(
            PathChecker::is_contained(root, valid_child),
            "/project/src/main.rs は /project の子である"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_verify_containment_with_base_nonexistent_absolute_path_via_symlink_alias() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();

        let alias_holder = TempDir::new().unwrap();
        let repo_alias = alias_holder.path().join("repo_alias");
        std::os::unix::fs::symlink(&project_root, &repo_alias).unwrap();

        // repo_alias/missing.txt は存在しないが、既存親(repo_alias)は実体に解決可能
        let nonexistent = repo_alias.join("missing.txt");
        let result =
            PathChecker::verify_containment_with_base(&project_root, &project_root, &nonexistent);

        assert!(
            result.is_ok(),
            "Symlink alias 経由の未作成パスでもプロジェクト内として扱うべき"
        );
        assert_eq!(result.unwrap(), project_root.join("missing.txt"));
    }
}
