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

    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("safe-rm: {}", e);
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

    // Git ステータスを必要時のみ一括事前取得（パフォーマンス最適化）
    // allow_project_deletion 有効時はスキップ
    let status_cache: HashMap<String, FileStatus> = if !config.allow_project_deletion {
        git_checker
            .as_ref()
            .map(|checker| checker.get_all_statuses())
            .unwrap_or_default()
    } else {
        HashMap::new()
    };

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
        // 最も高い終了コードのエラーを返す（セキュリティブロックが優先）
        if max_exit_code == 2 {
            // セキュリティエラーを直接返す
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

/// 単一パスの削除処理
fn process_path(
    path: &Path,
    project_root: &Path,
    cwd: &Path,
    git_checker: &Option<GitChecker>,
    status_cache: &HashMap<String, FileStatus>,
    args: &CliArgs,
    config: &Config,
) -> Result<bool, SafeRmError> {
    // 絶対パスに変換（相対パスは cwd から解決、git root からではない）
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };

    // allowed_paths 内のパスか確認（包含検証と Git チェックをバイパス）
    if config.is_path_allowed(&abs_path) {
        // メタデータを1回の syscall で取得（exists() + is_dir() の代替）
        let metadata = match std::fs::symlink_metadata(&abs_path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if args.force {
                    return Ok(false);
                } else {
                    return Err(SafeRmError::NotFound(abs_path));
                }
            }
            Err(e) => return Err(SafeRmError::IoError(e)),
        };

        // ディレクトリに -r フラグがない場合はエラー
        if metadata.is_dir() && !args.recursive {
            return Err(SafeRmError::IsDirectory(abs_path));
        }

        // 削除実行（またはドライラン）— 包含検証と Git チェックをスキップ
        if args.dry_run {
            println!("would remove: {} (allowed by config)", path.display());
            Ok(true)
        } else {
            delete_path_with_metadata(&abs_path, args.recursive, &metadata)?;
            println!("removed: {} (allowed by config)", path.display());
            Ok(true)
        }
    } else {
        // 標準安全チェック

        // パスがプロジェクト内にあることを最初に検証（セキュリティチェック優先）
        // プロジェクト外のファイル存在情報の漏洩を防止
        let canonical_path = PathChecker::verify_containment_with_base(project_root, cwd, path)?;
        let normalized_path = abs_path.clean();

        // メタデータを1回の syscall で取得（exists() + is_dir() の代替）
        let metadata = match std::fs::symlink_metadata(&abs_path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if args.force {
                    return Ok(false);
                } else {
                    return Err(SafeRmError::NotFound(abs_path));
                }
            }
            Err(e) => return Err(SafeRmError::IoError(e)),
        };

        // ディレクトリに -r フラグがない場合はエラー
        if metadata.is_dir() && !args.recursive {
            return Err(SafeRmError::IsDirectory(abs_path));
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
                checker.check_path_with_cache(git_check_path, status_cache)?;
            }
        }

        // 削除実行（またはドライラン）
        if args.dry_run {
            println!("would remove: {}", path.display());
            Ok(true)
        } else {
            delete_path_with_metadata(&abs_path, args.recursive, &metadata)?;
            println!("removed: {}", path.display());
            Ok(true)
        }
    }
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
}
