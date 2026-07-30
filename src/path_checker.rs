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

        // 3. プロジェクトルートも正規化
        let canonical_root = Self::try_canonicalize(&project_root.clean());

        // 4. 削除対象エントリ（末尾コンポーネント）の位置で境界判定する。
        //    `remove_file`/`remove_dir` は symlink を辿らず「リンクエントリ自体」を
        //    削除するため、実際に削除されるエントリがプロジェクト内にあることを要求する。
        //    末尾コンポーネントは canonicalize せず、中間 symlink（エイリアス）だけ解決する。
        //    これにより、プロジェクト外の symlink がプロジェクト内の実体を指していても、
        //    エントリ自体がプロジェクト外であれば確実にブロックできる（実体だけ見て
        //    通過させてしまう包含バイパスを塞ぐ）。
        let entry_path = Self::canonicalize_parent_keep_filename(&cleaned_path);
        if !Self::is_contained(&canonical_root, &entry_path) {
            return Err(SafeRmError::OutsideProject {
                path: target_path.to_path_buf(),
                project_root: project_root.to_path_buf(),
            });
        }

        // 5. 末尾まで解決した実体パスもプロジェクト内であることを要求する。
        //    プロジェクト内の symlink がプロジェクト外の実体を指すケースは、従来どおり
        //    安全側でブロックし続ける。未作成パスは既存の親まで解決される。
        let resolved_path = Self::try_canonicalize(&cleaned_path);
        if !Self::is_contained(&canonical_root, &resolved_path) {
            return Err(SafeRmError::OutsideProject {
                path: target_path.to_path_buf(),
                project_root: project_root.to_path_buf(),
            });
        }

        // 削除対象エントリの位置を返す（後続の Git ステータス判定もこの位置で行う）
        Ok(entry_path)
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

    /// 末尾成分が `.` または `..` の operand を拒否する。
    ///
    /// POSIX の rm は operand の basename が dot / dot-dot の場合、診断メッセージを
    /// 出して**その operand を一切処理しない**（GNU coreutils も
    /// `refusing to remove '.' or '..' directory` として skip する）。
    /// safe-rm がこれを実装しないと、`safe-rm -r .` がカレントディレクトリ自体を、
    /// `safe-rm -r ..` が親ディレクトリを実際に削除してしまい、置き換え対象である
    /// rm よりも危険側へ倒れる。削除したい対象はディレクトリ名で明示させる。
    ///
    /// allowed_paths のバイパスや `-f` より前に評価する必要があるため、正規化前の
    /// 生 operand に対して呼ぶ。
    pub fn reject_dot_or_dotdot_operand(target_path: &Path) -> Result<(), SafeRmError> {
        if Self::last_component_is_dot_or_dotdot(target_path) {
            return Err(SafeRmError::DotOrDotDotOperand {
                path: target_path.to_path_buf(),
            });
        }

        Ok(())
    }

    /// 末尾セパレータを除いた最後の成分が `.` / `..` かを判定する。
    ///
    /// `Path::components()` は `foo/.` の末尾 `.` を字句正規化で落として
    /// `Normal("foo")` にしてしまい、`foo/.`（basename は `.`）を検出できない。
    /// そのため生の文字列から末尾成分を切り出して比較する。
    /// 非 UTF-8 パスでも、`.` / `..` は ASCII なので `to_string_lossy()` の
    /// 置換文字（U+FFFD）と取り違えることはなく、判定は正確に行える。
    fn last_component_is_dot_or_dotdot(path: &Path) -> bool {
        let raw = path.as_os_str().to_string_lossy();
        // `./` や `foo/../` のような末尾スラッシュ付きも対象にする。
        let trimmed = raw.trim_end_matches(std::path::is_separator);
        if trimmed.is_empty() {
            // ルート（`/`）や空文字は末尾成分を持たないため対象外。
            return false;
        }

        let last_component = match trimmed.rfind(std::path::is_separator) {
            // セパレータは ASCII 1 バイトなので `+ 1` は必ず char 境界になる。
            Some(separator_index) => &trimmed[separator_index + 1..],
            None => trimmed,
        };

        if last_component == "." || last_component == ".." {
            return true;
        }

        // Windows の drive-relative 形式（`C:.` / `C:..`）はセパレータを含まないため
        // 上の比較に掛からないが、実質はドライブのカレント/親ディレクトリを指す
        // dot / dot-dot operand なので同様に拒否する。Unix では `C:.` が正当な
        // ファイル名になり得るため、この判定は Windows 限定にする。
        #[cfg(windows)]
        if Self::is_drive_relative_dot_component(last_component) {
            return true;
        }

        false
    }

    /// `C:.` / `C:..` のような Windows の drive-relative な dot / dot-dot 成分か判定する。
    ///
    /// 拒否に使うのは Windows のみだが、ロジックは全プラットフォームでコンパイル・
    /// テストできるようにしておく（Windows 環境でしか検証できない分岐を作らないため）。
    #[cfg_attr(not(windows), allow(dead_code))]
    fn is_drive_relative_dot_component(component: &str) -> bool {
        let Some(rest) = component.strip_prefix(|c: char| c.is_ascii_alphabetic()) else {
            return false;
        };
        let Some(rest) = rest.strip_prefix(':') else {
            return false;
        };

        rest == "." || rest == ".."
    }

    /// 中間コンポーネントに解決不能な symlink があるパスを拒否する。
    ///
    /// `dangling/child.txt` のようなパスは、検証時点では `NotFound` で止まるが、
    /// 検証後から実削除前までにリンク先が作成されると OS の path resolution が
    /// symlink の先へ進み、境界外のファイル削除に化け得る。末尾 symlink 自体の削除は
    /// リンクエントリだけを消す操作なので許可し、中間 symlink が解決できない場合だけ
    /// fail-closed でブロックする。
    pub fn reject_dangling_intermediate_symlink(
        resolve_base: &Path,
        target_path: &Path,
    ) -> Result<(), SafeRmError> {
        let absolute_path = Self::to_absolute(resolve_base, target_path).clean();
        let components: Vec<_> = absolute_path.components().collect();
        let last_normal_index = components
            .iter()
            .rposition(|component| matches!(component, std::path::Component::Normal(_)));

        let mut current = PathBuf::new();
        for (index, component) in components.iter().enumerate() {
            match component {
                std::path::Component::Prefix(prefix) => {
                    current.push(prefix.as_os_str());
                }
                std::path::Component::RootDir => {
                    current.push(component.as_os_str());
                }
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    current.pop();
                }
                std::path::Component::Normal(name) => {
                    current.push(name);
                    if Some(index) == last_normal_index {
                        continue;
                    }

                    let Ok(metadata) = std::fs::symlink_metadata(&current) else {
                        continue;
                    };
                    if metadata.file_type().is_symlink() && std::fs::metadata(&current).is_err() {
                        return Err(SafeRmError::DanglingIntermediateSymlink {
                            path: target_path.to_path_buf(),
                            symlink: current.clone(),
                        });
                    }
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

    /// 親ディレクトリのみ canonicalize し、末尾コンポーネントは解決せず保持する。
    ///
    /// 削除対象が symlink の場合、`remove_file`/`remove_dir` はリンクを辿らず
    /// リンクエントリ自体を削除する。境界判定を「実際に削除されるエントリ」の位置で
    /// 行うために、末尾コンポーネントは canonicalize しない。中間 symlink（エイリアス）は
    /// 解決して、中間 symlink 経由の境界脱出は引き続き防ぐ。
    fn canonicalize_parent_keep_filename(path: &Path) -> PathBuf {
        let Some(file_name) = path.file_name() else {
            return Self::try_canonicalize(path);
        };
        let Some(parent) = path.parent() else {
            return path.to_path_buf();
        };
        Self::try_canonicalize(parent).join(file_name)
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

        // プロジェクト内にある symlink でも、実体がプロジェクト外を指す場合は
        // 従来どおり安全側でブロックする（実体位置チェック）。
        let result = PathChecker::verify_containment(&project_root, &link_path);
        assert!(result.is_err());
    }

    #[test]
    #[cfg(unix)]
    fn test_verify_containment_outside_symlink_pointing_inside_is_blocked() {
        // プロジェクト外にある symlink が、プロジェクト内の実体を指すケース。
        // `remove_file` はリンクを辿らず symlink エントリ自体（プロジェクト外）を
        // 削除するため、実体が内側でも「削除されるエントリ」は境界外として
        // 確実にブロックする（実体だけ見て通過させてしまう包含バイパスの回帰防止）。
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();
        let inside_file = project_root.join("inside.txt");
        fs::write(&inside_file, "inside").unwrap();

        // プロジェクト外に、プロジェクト内を指す symlink を作成
        let outside_dir = TempDir::new().unwrap();
        let outside_link = outside_dir.path().join("link_into_project.txt");
        std::os::unix::fs::symlink(&inside_file, &outside_link).unwrap();

        let result = PathChecker::verify_containment(&project_root, &outside_link);
        assert!(
            result.is_err(),
            "プロジェクト外の symlink はリンク先がプロジェクト内でもブロックされるべき"
        );
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
    #[cfg(unix)]
    fn test_reject_symlink_parent_traversal_blocks_nested_removed_symlink_component() {
        // `link/child/../../victim.txt` は一度 symlink 配下へ下ってから
        // `..` で symlink 成分自体を消すため、字句正規化後の削除対象と
        // OS の path resolution が一致しない。
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();
        let outside_dir = TempDir::new().unwrap();
        fs::create_dir_all(outside_dir.path().join("child")).unwrap();

        std::os::unix::fs::symlink(outside_dir.path(), project_root.join("link")).unwrap();

        let result = PathChecker::reject_symlink_parent_traversal(
            &project_root,
            Path::new("link/child/../../victim.txt"),
        );

        assert!(
            matches!(result, Err(SafeRmError::UnsafeTraversal { .. })),
            "ネストした `..` で symlink 成分が消える経路は拒否すべき"
        );
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
    #[cfg(unix)]
    fn test_reject_dangling_intermediate_symlink_blocks_child_path() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();
        std::os::unix::fs::symlink(
            "/definitely/missing/safe-rm-target",
            project_root.join("link"),
        )
        .unwrap();

        let result = PathChecker::reject_dangling_intermediate_symlink(
            &project_root,
            Path::new("link/child.txt"),
        );

        assert!(
            matches!(result, Err(SafeRmError::DanglingIntermediateSymlink { .. })),
            "dangling 中間 symlink 配下の削除は fail-closed で拒否すべき"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_reject_dangling_intermediate_symlink_allows_final_symlink() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().canonicalize().unwrap();
        std::os::unix::fs::symlink(
            "/definitely/missing/safe-rm-target",
            project_root.join("link"),
        )
        .unwrap();

        let result =
            PathChecker::reject_dangling_intermediate_symlink(&project_root, Path::new("link"));

        assert!(
            result.is_ok(),
            "末尾の dangling symlink 自体の削除はリンクエントリだけを消すため許可する"
        );
    }

    #[test]
    fn test_reject_dot_or_dotdot_operand_blocks_dot_forms() {
        // POSIX の rm が operand ごと無視する形式はすべて拒否する。
        for operand in [
            ".",
            "./",
            "..",
            "../",
            "sub/.",
            "sub/..",
            "sub/./",
            "sub/../",
            "/tmp/sub/.",
            "/tmp/sub/..",
            "a/b/c/..",
        ] {
            let result = PathChecker::reject_dot_or_dotdot_operand(Path::new(operand));
            assert!(
                matches!(result, Err(SafeRmError::DotOrDotDotOperand { .. })),
                "末尾成分が dot/dot-dot の operand は拒否されるべき: {} -> {:?}",
                operand,
                result
            );
        }
    }

    #[test]
    fn test_reject_dot_or_dotdot_operand_allows_normal_paths() {
        // 末尾成分が dot/dot-dot でなければ、途中に `.` や `..` があっても
        // このチェックでは通す（包含検証と `..` トラバーサル検査に委ねる）。
        for operand in [
            "sub",
            "sub/deep.txt",
            "./sub/deep.txt",
            "../sibling/file.txt",
            "/tmp/sub/file.txt",
            "/",
            // dot で始まる/終わる通常のファイル名は削除できなければならない
            ".git",
            ".hidden",
            "...",
            "..foo",
            "foo..",
            "sub/...",
        ] {
            let result = PathChecker::reject_dot_or_dotdot_operand(Path::new(operand));
            assert!(
                result.is_ok(),
                "通常の operand は許可されるべき: {} -> {:?}",
                operand,
                result
            );
        }
    }

    #[test]
    fn test_is_drive_relative_dot_component() {
        // Windows の drive-relative な dot / dot-dot 形式だけを true にする。
        for component in ["C:.", "C:..", "z:.", "z:.."] {
            assert!(
                PathChecker::is_drive_relative_dot_component(component),
                "drive-relative な dot/dot-dot は検出されるべき: {}",
                component
            );
        }
        // ドライブ文字でない・区切りが `:` でない・`.`/`..` 以外は対象外。
        // 素の `.` / `..` は呼び出し元の比較で先に拾うため、ここでは false でよい。
        for component in ["C:", "C:foo", "C:...", "1:.", ":.", "Cx.", ".", ".."] {
            assert!(
                !PathChecker::is_drive_relative_dot_component(component),
                "drive-relative でない成分は検出されるべきでない: {}",
                component
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn test_reject_dot_or_dotdot_operand_blocks_drive_relative_forms() {
        for operand in ["C:.", "C:..", "z:.", "z:.."] {
            let result = PathChecker::reject_dot_or_dotdot_operand(Path::new(operand));
            assert!(
                matches!(result, Err(SafeRmError::DotOrDotDotOperand { .. })),
                "drive-relative な dot/dot-dot operand は拒否されるべき: {} -> {:?}",
                operand,
                result
            );
        }
    }

    #[test]
    fn test_reject_dot_or_dotdot_operand_error_carries_original_path() {
        let result = PathChecker::reject_dot_or_dotdot_operand(Path::new("sub/.."));

        match result {
            Err(SafeRmError::DotOrDotDotOperand { path }) => {
                assert_eq!(
                    path,
                    PathBuf::from("sub/.."),
                    "エラーには利用者が入力した生パスを保持すべき"
                );
            }
            other => panic!("DotOrDotDotOperand が返るべき: {:?}", other),
        }
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
