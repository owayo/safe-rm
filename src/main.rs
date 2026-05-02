//! safe-rm: AIエージェント向け安全なファイル削除ツール
//!
//! Git状態に基づくアクセス制御を備えたファイル削除プロキシ。
//! Clean または Ignored 状態のファイルのみ削除を許可する。

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use path_clean::PathClean;
use safe_rm::cli::{CliArgs, Commands};
use safe_rm::config::Config;
use safe_rm::error::{FileStatus, SafeRmError};
use safe_rm::git_checker::GitChecker;
use safe_rm::init;
use safe_rm::path_checker::PathChecker;

fn main() -> ExitCode {
    let args = CliArgs::parse_args();

    // サブコマンドの処理
    if let Some(Commands::Init) = args.command {
        match init::run_init() {
            Ok(()) => return ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("safe-rm: {}", e);
                return ExitCode::FAILURE;
            }
        }
    }

    // 単一パス失敗は実エラーを 1 回だけ表示する。
    // 複数パス失敗は run() 側の各パス出力を優先し、
    // 操作エラーのみ最後に集計メッセージを補足する。
    let path_count = args.paths.len();

    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if path_count == 1 || (path_count > 1 && e.exit_code() == 1) {
                eprintln!("safe-rm: {}", e);
            }
            e.exit_code().into()
        }
    }
}

/// メイン実行ロジック
fn run(args: CliArgs) -> Result<(), SafeRmError> {
    // ユーザー設定の読み込み
    let config = Config::load();

    // カレントディレクトリの取得
    let cwd = std::env::current_dir().map_err(SafeRmError::IoError)?;

    // Git リポジトリを開く（存在する場合）
    let git_checker = GitChecker::open(&cwd);

    // Git リポジトリルートをプロジェクト境界として使用（cwd ではなく）
    // 例: frontend/ から実行して backend/file.txt を削除する場合にも正しく動作
    let project_root = git_checker
        .as_ref()
        .and_then(|checker| checker.workdir())
        .unwrap_or_else(|| cwd.clone());

    // Git ステータスは、厳格モードかつ allowed_paths 外の削除で初めて取得する。
    // allowed_paths は Git チェックをバイパスするため、現在のリポジトリに
    // Git API エラーがあっても allowed_paths の削除を巻き込まない。
    let mut status_cache: Option<HashMap<String, FileStatus>> = None;

    let mut success_count = 0;
    let mut error_count = 0;
    let mut max_exit_code: u8 = 0;
    let mut last_error: Option<SafeRmError> = None;
    let print_path_errors = args.paths.len() > 1;

    for path in &args.paths {
        match process_path(
            path,
            &project_root,
            &cwd,
            &git_checker,
            &mut status_cache,
            &args,
            &config,
        ) {
            Ok(deleted) => {
                if deleted {
                    success_count += 1;
                }
            }
            Err(e) => {
                if print_path_errors {
                    eprintln!("safe-rm: {}: {}", path.display(), e);
                }
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
        if args.paths.len() == 1 {
            return Err(last_error.unwrap());
        }

        // 最も高い終了コードのエラーを返す（セキュリティブロックが優先）
        if max_exit_code == 2 {
            // セキュリティエラーを直接返す
            Err(last_error.unwrap())
        } else {
            Err(SafeRmError::PartialFailure {
                success: success_count,
                failed: error_count,
                dry_run: args.dry_run,
            })
        }
    } else {
        Ok(())
    }
}

/// 単一パスの削除処理
fn process_path(
    path: &Path,
    project_root: &Path,
    cwd: &Path,
    git_checker: &Option<GitChecker>,
    status_cache: &mut Option<HashMap<String, FileStatus>>,
    args: &CliArgs,
    config: &Config,
) -> Result<bool, SafeRmError> {
    // 絶対パスに変換（相対パスは cwd から解決、git root からではない）
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    // 字句的に `..` を解決した正規化パス。
    // OS の path resolution は symlink を辿った後で `..` を解決するため、
    // ユーザー入力をそのまま OS に渡すと `link/../victim` のような形で
    // プロジェクト境界を脱出される恐れがある。
    // 以降のメタデータ取得・削除・許可判定はすべて clean 済みパスで行う。
    let normalized_path = abs_path.clean();

    // allowed_paths 内のパスか確認（包含検証と Git チェックをバイパス）
    if config.is_path_allowed(&normalized_path) {
        ensure_git_metadata_not_targeted(&normalized_path, path, args.recursive, git_checker)?;

        // メタデータを1回の syscall で取得（exists() + is_dir() の代替）
        let metadata = match std::fs::symlink_metadata(&normalized_path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if args.force {
                    return Ok(false);
                } else {
                    return Err(SafeRmError::NotFound(normalized_path));
                }
            }
            Err(e) => return Err(SafeRmError::IoError(e)),
        };

        // ディレクトリに -r フラグがない場合はエラー
        if metadata.is_dir() && !args.recursive {
            return Err(SafeRmError::IsDirectory(normalized_path));
        }

        // 削除実行（またはドライラン）— 包含検証と Git チェックをスキップ
        if args.dry_run {
            println!("would remove: {} (allowed by config)", path.display());
            Ok(true)
        } else {
            delete_path_with_metadata(&normalized_path, args.recursive, &metadata)?;
            println!("removed: {} (allowed by config)", path.display());
            Ok(true)
        }
    } else {
        // 標準安全チェック

        // パスがプロジェクト内にあることを最初に検証（セキュリティチェック優先）
        // プロジェクト外のファイル存在情報の漏洩を防止
        let canonical_path = PathChecker::verify_containment_with_base(project_root, cwd, path)?;

        ensure_git_metadata_not_targeted(&normalized_path, path, args.recursive, git_checker)?;

        // メタデータを1回の syscall で取得（exists() + is_dir() の代替）
        let metadata = match std::fs::symlink_metadata(&normalized_path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if args.force {
                    return Ok(false);
                } else {
                    return Err(SafeRmError::NotFound(normalized_path));
                }
            }
            Err(e) => return Err(SafeRmError::IoError(e)),
        };

        // ディレクトリに -r フラグがない場合はエラー
        if metadata.is_dir() && !args.recursive {
            return Err(SafeRmError::IsDirectory(normalized_path));
        }

        // 事前取得キャッシュを使用して Git ステータスをチェック（バッチ最適化）
        // allow_project_deletion 有効時はスキップ（包含検証は上記で完了）
        if !config.allow_project_deletion {
            if let Some(checker) = git_checker {
                // シンボリックリンクの場合、親ディレクトリのみ canonicalize し
                // リンク名自体は保持。「リンク自体をチェック」するセマンティクスを
                // 維持しつつ、リポジトリエイリアスパスを解決する。
                let symlink_git_check_path: Option<std::path::PathBuf> =
                    if metadata.file_type().is_symlink() {
                        Some(
                            normalized_path
                                .file_name()
                                .and_then(|name| {
                                    normalized_path
                                        .parent()
                                        .and_then(|parent| parent.canonicalize().ok())
                                        .map(|canonical_parent| canonical_parent.join(name))
                                })
                                .unwrap_or_else(|| normalized_path.clone()),
                        )
                    } else {
                        None
                    };
                let git_check_path = symlink_git_check_path.as_deref().unwrap_or(&canonical_path);
                if status_cache.is_none() {
                    *status_cache = Some(checker.get_all_statuses()?);
                }
                let cache = status_cache
                    .as_ref()
                    .expect("status_cache must be initialized before strict Git check");
                checker.check_path_with_cache(git_check_path, cache)?;
            }
        }

        // 削除実行（またはドライラン）
        if args.dry_run {
            println!("would remove: {}", path.display());
            Ok(true)
        } else {
            delete_path_with_metadata(&normalized_path, args.recursive, &metadata)?;
            println!("removed: {}", path.display());
            Ok(true)
        }
    }
}

/// Git 管理メタデータの削除対象化を検査する。
fn ensure_git_metadata_not_targeted(
    normalized_path: &Path,
    original_path: &Path,
    recursive: bool,
    git_checker: &Option<GitChecker>,
) -> Result<(), SafeRmError> {
    // 任意階層の `.git` や bare リポジトリ、および再帰削除時に配下へ含まれる
    // Git 管理メタデータを保護する（ネストしたリポジトリ対応）。
    if GitChecker::try_path_targets_or_contains_git_metadata(normalized_path, recursive)? {
        return Err(SafeRmError::ProtectedGitPath {
            path: original_path.to_path_buf(),
        });
    }

    // 現在のリポジトリの Git 管理メタデータも保護する。
    if let Some(checker) = git_checker {
        if checker.touches_git_metadata_path(normalized_path) {
            return Err(SafeRmError::ProtectedGitPath {
                path: original_path.to_path_buf(),
            });
        }
    }

    Ok(())
}

/// メタデータを使用してファイルまたはディレクトリを削除（追加 syscall を回避）
fn delete_path_with_metadata(
    path: &Path,
    recursive: bool,
    metadata: &std::fs::Metadata,
) -> Result<(), SafeRmError> {
    if metadata.is_dir() {
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
        // プロジェクトが正しくコンパイルされることを確認するスモークテスト
    }

    #[test]
    fn test_version_available() {
        let version = env!("CARGO_PKG_VERSION");
        assert!(!version.is_empty());
        // Cargo.toml のバージョンが有効な semver 形式であることを検証
        assert!(version.contains('.'), "Version should be in semver format");
    }

    #[test]
    fn test_delete_path_with_metadata_removes_file() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let file = tmp_dir.path().join("test.txt");
        std::fs::write(&file, "content").unwrap();

        let metadata = std::fs::symlink_metadata(&file).unwrap();
        let result = super::delete_path_with_metadata(&file, false, &metadata);
        assert!(result.is_ok());
        assert!(!file.exists(), "ファイルが削除されているべき");
    }

    #[test]
    fn test_delete_path_with_metadata_removes_directory() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let dir = tmp_dir.path().join("subdir");
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested").join("file.txt"), "content").unwrap();

        let metadata = std::fs::symlink_metadata(&dir).unwrap();
        let result = super::delete_path_with_metadata(&dir, true, &metadata);
        assert!(result.is_ok());
        assert!(!dir.exists(), "ディレクトリが再帰削除されているべき");
    }

    #[test]
    fn test_delete_path_with_metadata_non_recursive_empty_dir() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let dir = tmp_dir.path().join("empty");
        std::fs::create_dir_all(&dir).unwrap();

        let metadata = std::fs::symlink_metadata(&dir).unwrap();
        // recursive=false で空ディレクトリは remove_dir で削除可能
        let result = super::delete_path_with_metadata(&dir, false, &metadata);
        assert!(result.is_ok());
        assert!(!dir.exists(), "空ディレクトリが削除されているべき");
    }

    #[test]
    fn test_delete_path_with_metadata_non_recursive_nonempty_dir_fails() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let dir = tmp_dir.path().join("nonempty");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file.txt"), "content").unwrap();

        let metadata = std::fs::symlink_metadata(&dir).unwrap();
        // recursive=false で非空ディレクトリは remove_dir が失敗する
        let result = super::delete_path_with_metadata(&dir, false, &metadata);
        assert!(result.is_err(), "非空ディレクトリの非再帰削除は失敗すべき");
    }

    #[cfg(unix)]
    #[test]
    fn test_delete_path_with_metadata_symlink_file() {
        // シンボリックリンク自体が削除され、リンク先は残ることを検証
        let tmp_dir = tempfile::tempdir().unwrap();
        let target = tmp_dir.path().join("target.txt");
        std::fs::write(&target, "content").unwrap();

        let link = tmp_dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let metadata = std::fs::symlink_metadata(&link).unwrap();
        assert!(
            metadata.file_type().is_symlink(),
            "シンボリックリンクであるべき"
        );

        let result = super::delete_path_with_metadata(&link, false, &metadata);
        assert!(result.is_ok(), "シンボリックリンクの削除は成功すべき");
        assert!(!link.exists(), "シンボリックリンク自体が削除されているべき");
        assert!(target.exists(), "リンク先のファイルは残っているべき");
    }

    #[cfg(unix)]
    #[test]
    fn test_delete_path_with_metadata_symlink_dir() {
        // シンボリックリンク先がディレクトリの場合、リンク自体が削除されリンク先ディレクトリは残ることを検証
        let tmp_dir = tempfile::tempdir().unwrap();
        let target_dir = tmp_dir.path().join("target_dir");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(target_dir.join("inner.txt"), "content").unwrap();

        let link = tmp_dir.path().join("link_to_dir");
        std::os::unix::fs::symlink(&target_dir, &link).unwrap();

        let metadata = std::fs::symlink_metadata(&link).unwrap();
        assert!(
            metadata.file_type().is_symlink(),
            "シンボリックリンクであるべき"
        );

        // symlink_metadata で is_dir() は false（リンク自体はディレクトリではない）ため
        // remove_file パスで処理される
        let result = super::delete_path_with_metadata(&link, false, &metadata);
        assert!(
            result.is_ok(),
            "ディレクトリへのシンボリックリンクの削除は成功すべき"
        );
        assert!(
            link.symlink_metadata().is_err(),
            "シンボリックリンク自体が削除されているべき"
        );
        assert!(target_dir.exists(), "リンク先ディレクトリは残っているべき");
        assert!(
            target_dir.join("inner.txt").exists(),
            "リンク先ディレクトリ内のファイルも残っているべき"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_delete_path_with_metadata_recursive_dir_with_symlinks() {
        // recursive=true のときディレクトリ内のシンボリックリンクも含めて全削除されることを検証
        let tmp_dir = tempfile::tempdir().unwrap();

        // 削除対象ディレクトリの構築
        let dir = tmp_dir.path().join("parent");
        std::fs::create_dir_all(dir.join("child")).unwrap();
        std::fs::write(dir.join("child").join("file.txt"), "content").unwrap();

        // ディレクトリ外のリンク先（削除されないことを確認するため）
        let external_target = tmp_dir.path().join("external.txt");
        std::fs::write(&external_target, "external").unwrap();

        // ディレクトリ内にシンボリックリンクを作成
        let link_in_dir = dir.join("link_to_external.txt");
        std::os::unix::fs::symlink(&external_target, &link_in_dir).unwrap();

        let metadata = std::fs::symlink_metadata(&dir).unwrap();
        assert!(metadata.is_dir(), "ディレクトリであるべき");

        let result = super::delete_path_with_metadata(&dir, true, &metadata);
        assert!(result.is_ok(), "再帰削除は成功すべき");
        assert!(!dir.exists(), "ディレクトリ全体が削除されているべき");
        assert!(
            external_target.exists(),
            "シンボリックリンク先の外部ファイルは残っているべき"
        );
    }

    #[test]
    fn test_delete_path_with_metadata_io_error() {
        // 存在しないパスの削除は IoError を返すことを検証
        let tmp_dir = tempfile::tempdir().unwrap();
        let nonexistent = tmp_dir.path().join("nonexistent.txt");

        // 存在するファイルのメタデータを借用して、存在しないパスに適用
        let dummy_file = tmp_dir.path().join("dummy.txt");
        std::fs::write(&dummy_file, "dummy").unwrap();
        let metadata = std::fs::symlink_metadata(&dummy_file).unwrap();

        let result = super::delete_path_with_metadata(&nonexistent, false, &metadata);
        assert!(result.is_err(), "存在しないパスの削除は失敗すべき");
    }
}
