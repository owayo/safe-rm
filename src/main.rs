//! safe-rm: AIエージェント向け安全なファイル削除ツール
//!
//! プロジェクト境界と Git 管理メタデータを保護するファイル削除プロキシ。
//! 厳格モードでは Git 状態に基づき未コミット変更の削除もブロックする。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use path_clean::PathClean;
use safe_rm::cli::{CliArgs, Commands};
use safe_rm::config::Config;
use safe_rm::error::{FileStatus, SafeRmError};
use safe_rm::git_checker::GitChecker;
use safe_rm::init;
use safe_rm::path_checker::PathChecker;

/// Git リポジトリ検出を必要になるまで遅延するための状態。
struct GitContext {
    checker: Option<GitChecker>,
    project_root: PathBuf,
    opened: bool,
    optional_open_failed: bool,
}

impl GitContext {
    fn new(cwd: &Path) -> Self {
        Self {
            checker: None,
            project_root: cwd.to_path_buf(),
            opened: false,
            optional_open_failed: false,
        }
    }

    /// allowed_paths の Git メタデータ保護を強化するため、可能なら Git 情報を取得する。
    /// ただし allowed_paths は Git チェックをバイパスする仕様なので、Git 検出失敗では止めない。
    fn try_open_optional(&mut self, cwd: &Path) {
        if self.opened || self.optional_open_failed {
            return;
        }

        match GitChecker::open(cwd) {
            Ok(checker) => self.set_checker(cwd, checker),
            Err(_) => {
                self.optional_open_failed = true;
            }
        }
    }

    /// 通常パスの包含検証や strict モードでは Git 情報が安全境界になるため、
    /// Git 検出エラーを fail-closed で呼び出し元へ返す。
    fn ensure_open_required(&mut self, cwd: &Path) -> Result<(), SafeRmError> {
        if self.opened {
            return Ok(());
        }

        let checker = GitChecker::open(cwd)?;
        self.set_checker(cwd, checker);
        Ok(())
    }

    fn set_checker(&mut self, cwd: &Path, checker: Option<GitChecker>) {
        self.project_root = checker
            .as_ref()
            .and_then(|checker| checker.workdir())
            .unwrap_or_else(|| cwd.to_path_buf());
        self.checker = checker;
        self.opened = true;
    }

    fn checker(&self) -> Option<&GitChecker> {
        self.checker.as_ref()
    }

    fn project_root(&self) -> &Path {
        &self.project_root
    }
}

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

    // Git リポジトリ検出は allowed_paths 以外の安全境界が必要になるまで遅延する。
    // allowed_paths は包含検証と Git ステータスチェックをバイパスする仕様なので、
    // cwd の `.git` が壊れていても許可パスの削除まで巻き込まない。
    let mut git_context = GitContext::new(&cwd);

    // Git ステータスは、厳格モードかつ allowed_paths 外の削除で初めて取得する。
    // allowed_paths は Git チェックをバイパスするため、現在のリポジトリに
    // Git API エラーがあっても allowed_paths の削除を巻き込まない。
    // キャッシュは「最後に使った repo workdir」をキーに持ち、同じ repo の連続
    // 削除では再利用しつつ、cwd と異なる repo の対象でも正しく status を引ける
    // ようにする（cwd 非 Git で対象側だけ Git の場合の strict バイパスを塞ぐ）。
    // キャッシュキーは非 UTF-8 パスにも対応するためバイト列で持つ。
    let mut status_cache: Option<(PathBuf, HashMap<Vec<u8>, FileStatus>)> = None;

    let mut success_count = 0;
    let mut error_count = 0;
    let mut max_exit_code: u8 = 0;
    let mut last_error: Option<SafeRmError> = None;
    let print_path_errors = args.paths.len() > 1;

    for path in &args.paths {
        match process_path(
            path,
            &cwd,
            &mut git_context,
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
    cwd: &Path,
    git_context: &mut GitContext,
    status_cache: &mut Option<(PathBuf, HashMap<Vec<u8>, FileStatus>)>,
    args: &CliArgs,
    config: &Config,
) -> Result<bool, SafeRmError> {
    // 絶対パスに変換（相対パスは cwd から解決、git root からではない）
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    PathChecker::reject_symlink_parent_traversal(cwd, path)?;

    // 字句的に `..` を解決した正規化パス。
    // OS の path resolution は symlink を辿った後で `..` を解決するため、
    // ユーザー入力をそのまま OS に渡すと `link/../victim` のような形で
    // プロジェクト境界を脱出される恐れがある。
    // 以降のメタデータ取得・削除・許可判定はすべて clean 済みパスで行う。
    let normalized_path = abs_path.clean();

    // allowed_paths 判定のために削除対象のディレクトリ性質を事前確認する。
    // 取得失敗（NotFound や I/O エラー）はここでは握りつぶし、後続の
    // fetch_target_metadata 側で正規のエラーパスに委ねる。
    // symlink-to-directory は symlink_metadata 上 is_dir = false となるため、
    // ディレクトリの再帰削除制約は実体ディレクトリのみに適用される
    // （リンクエントリ自体は単一エントリの削除として扱われる）。
    let target_is_dir = std::fs::symlink_metadata(&normalized_path)
        .ok()
        .map(|m| m.is_dir())
        .unwrap_or(false);

    // allowed_paths 内のパスか確認（包含検証と Git チェックをバイパス）
    // `recursive = false` エントリ配下のディレクトリへの `-r` は、直接の子の範囲を
    // 超えた削除になるため非再帰エントリのみマッチした場合はバイパスを許可しない
    // （`is_path_allowed_for_removal` が fail-closed で拒否する）。
    if config.is_path_allowed_for_removal(&normalized_path, target_is_dir, args.recursive) {
        git_context.try_open_optional(cwd);
        ensure_git_metadata_not_targeted(
            &normalized_path,
            path,
            args.recursive,
            git_context.checker(),
        )?;

        let Some(metadata) = fetch_target_metadata(&normalized_path, args.force, args.recursive)?
        else {
            return Ok(false);
        };

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

        git_context.ensure_open_required(cwd)?;

        // パスがプロジェクト内にあることを最初に検証（セキュリティチェック優先）
        // プロジェクト外のファイル存在情報の漏洩を防止
        let canonical_path =
            PathChecker::verify_containment_with_base(git_context.project_root(), cwd, path)?;

        ensure_git_metadata_not_targeted(
            &normalized_path,
            path,
            args.recursive,
            git_context.checker(),
        )?;

        let Some(metadata) = fetch_target_metadata(&normalized_path, args.force, args.recursive)?
        else {
            return Ok(false);
        };

        // 事前取得キャッシュを使用して Git ステータスをチェック（バッチ最適化）
        // allow_project_deletion 有効時はスキップ（包含検証は上記で完了）
        if !config.allow_project_deletion {
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

            // cwd の checker が対象を含まないとき、対象側で再 discover する。
            // 例えば cwd が非 Git でも、配下のサブディレクトリが Git リポジトリ
            // であれば、そちらの status で strict チェックを掛けないと
            // 未コミット変更を素通りで削除してしまう。
            let cwd_checker_covers_target = git_context
                .checker()
                .and_then(|c| c.workdir().map(|w| git_check_path.starts_with(&w)))
                .unwrap_or(false);

            // cwd の checker が対象を含まないときだけ、対象側で別途 discover する。
            // ライフタイム制約のため、所有を持つ Option<GitChecker> を outer に置き、
            // 参照として target_checker に渡す。
            let target_checker_owned: Option<GitChecker> = if cwd_checker_covers_target {
                None
            } else {
                // 対象がディレクトリならその場所から、ファイル/symlink なら親から discover。
                // 親が取れない場合は対象自身を起点にする。
                let discover_from: PathBuf =
                    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                        git_check_path.to_path_buf()
                    } else {
                        git_check_path
                            .parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| git_check_path.to_path_buf())
                    };
                GitChecker::open(&discover_from)?
            };
            let target_checker: Option<&GitChecker> = if cwd_checker_covers_target {
                git_context.checker()
            } else {
                target_checker_owned.as_ref()
            };

            if let Some(checker) = target_checker {
                // bare リポジトリは workdir が None。bare の管理ファイル直接削除は
                // Git 管理メタデータ保護で既にブロック済みなので、ここでは何もしない。
                if let Some(workdir) = checker.workdir() {
                    let cache_hit = status_cache
                        .as_ref()
                        .map(|(k, _)| k == &workdir)
                        .unwrap_or(false);
                    if !cache_hit {
                        *status_cache = Some((workdir.clone(), checker.get_all_statuses()?));
                    }
                    let cache = &status_cache
                        .as_ref()
                        .expect("status_cache must be initialized before strict Git check")
                        .1;
                    checker.check_path_with_cache(git_check_path, cache)?;
                }
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
    git_checker: Option<&GitChecker>,
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

/// 削除対象のメタデータを取得し、削除前の共通検証を行う。
///
/// allowed_paths 分岐と標準チェック分岐で完全に同一だった処理を共通化したもの。
/// 呼び出し側の安全チェック順序（包含検証・Git メタデータ保護）は変えず、
/// メタデータ取得とディレクトリ判定の重複だけを排除する。
///
/// # 戻り値
/// * `Ok(Some(metadata))` - 削除対象が存在し、削除を続行してよい
/// * `Ok(None)` - 対象が存在せず `-f` 指定のためスキップ（呼び出し側は `Ok(false)` を返す）
/// * `Err(SafeRmError::NotFound)` - 対象が存在せず `-f` 未指定
/// * `Err(SafeRmError::IsDirectory)` - ディレクトリだが `-r` 未指定
/// * `Err(SafeRmError::IoError)` - メタデータ取得時の I/O エラー
fn fetch_target_metadata(
    path: &Path,
    force: bool,
    recursive: bool,
) -> Result<Option<std::fs::Metadata>, SafeRmError> {
    // メタデータを1回の syscall で取得（exists() + is_dir() の代替）
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return if force {
                Ok(None)
            } else {
                Err(SafeRmError::NotFound(path.to_path_buf()))
            };
        }
        Err(e) => return Err(SafeRmError::IoError(e)),
    };

    // ディレクトリに -r フラグがない場合はエラー
    if metadata.is_dir() && !recursive {
        return Err(SafeRmError::IsDirectory(path.to_path_buf()));
    }

    Ok(Some(metadata))
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

    #[test]
    fn test_ensure_git_metadata_not_targeted_allows_plain_file() {
        // Git 管理メタデータに該当しない通常パスは許可される
        let tmp_dir = tempfile::tempdir().unwrap();
        let plain_file = tmp_dir.path().join("note.txt");
        std::fs::write(&plain_file, "content").unwrap();

        let result = super::ensure_git_metadata_not_targeted(&plain_file, &plain_file, false, None);

        assert!(
            result.is_ok(),
            "通常ファイルは Git 管理メタデータ判定を通すべき"
        );
    }

    #[test]
    fn test_ensure_git_metadata_not_targeted_blocks_dot_git_component() {
        // パスの末尾コンポーネントが `.git` の場合はブロック
        let tmp_dir = tempfile::tempdir().unwrap();
        let dot_git_path = tmp_dir.path().join(".git");
        std::fs::create_dir(&dot_git_path).unwrap();

        let result =
            super::ensure_git_metadata_not_targeted(&dot_git_path, &dot_git_path, false, None);

        assert!(
            matches!(result, Err(super::SafeRmError::ProtectedGitPath { .. })),
            "末尾が .git のパスは ProtectedGitPath で拒否されるべき: {:?}",
            result
        );
    }

    #[test]
    fn test_ensure_git_metadata_not_targeted_blocks_intermediate_dot_git() {
        // パスの中間コンポーネントが `.git` の場合もブロック
        // ファイルが存在しなくても、字句的に検出される
        let tmp_dir = tempfile::tempdir().unwrap();
        let inside_dot_git = tmp_dir.path().join(".git").join("config");

        let result =
            super::ensure_git_metadata_not_targeted(&inside_dot_git, &inside_dot_git, false, None);

        assert!(
            matches!(result, Err(super::SafeRmError::ProtectedGitPath { .. })),
            "中間に .git を含むパスは ProtectedGitPath で拒否されるべき: {:?}",
            result
        );
    }

    #[test]
    fn test_ensure_git_metadata_not_targeted_blocks_uppercase_dot_git_component() {
        // macOS APFS のような case-insensitive FS で `.GIT` 経由の
        // バイパスが起きないことを検証する
        let tmp_dir = tempfile::tempdir().unwrap();
        let inside_dot_git_upper = tmp_dir.path().join(".GIT").join("config");

        let result = super::ensure_git_metadata_not_targeted(
            &inside_dot_git_upper,
            &inside_dot_git_upper,
            false,
            None,
        );

        assert!(
            matches!(result, Err(super::SafeRmError::ProtectedGitPath { .. })),
            "大文字バリアントの .GIT 中間コンポーネントも拒否されるべき: {:?}",
            result
        );
    }

    #[test]
    fn test_fetch_target_metadata_returns_metadata_for_existing_file() {
        // 既存の通常ファイルはメタデータを返す
        let tmp_dir = tempfile::tempdir().unwrap();
        let file = tmp_dir.path().join("note.txt");
        std::fs::write(&file, "content").unwrap();

        let result = super::fetch_target_metadata(&file, false, false);
        assert!(
            matches!(result, Ok(Some(_))),
            "既存ファイルはメタデータを返すべき: {:?}",
            result
        );
    }

    #[test]
    fn test_fetch_target_metadata_missing_without_force_errors() {
        // 存在しないパスは force なしで NotFound エラー
        let tmp_dir = tempfile::tempdir().unwrap();
        let missing = tmp_dir.path().join("missing.txt");

        let result = super::fetch_target_metadata(&missing, false, false);
        assert!(
            matches!(result, Err(super::SafeRmError::NotFound(_))),
            "存在しないパスは force なしで NotFound を返すべき: {:?}",
            result
        );
    }

    #[test]
    fn test_fetch_target_metadata_missing_with_force_is_none() {
        // 存在しないパスは force ありで Ok(None)（スキップ対象）
        let tmp_dir = tempfile::tempdir().unwrap();
        let missing = tmp_dir.path().join("missing.txt");

        let result = super::fetch_target_metadata(&missing, true, false);
        assert!(
            matches!(result, Ok(None)),
            "force 指定時は存在しないパスを Ok(None) でスキップすべき: {:?}",
            result
        );
    }

    #[test]
    fn test_fetch_target_metadata_directory_without_recursive_errors() {
        // ディレクトリは recursive なしで IsDirectory エラー
        let tmp_dir = tempfile::tempdir().unwrap();
        let dir = tmp_dir.path().join("subdir");
        std::fs::create_dir(&dir).unwrap();

        let result = super::fetch_target_metadata(&dir, false, false);
        assert!(
            matches!(result, Err(super::SafeRmError::IsDirectory(_))),
            "ディレクトリは -r なしで IsDirectory を返すべき: {:?}",
            result
        );
    }

    #[test]
    fn test_fetch_target_metadata_directory_with_recursive_returns_metadata() {
        // ディレクトリは recursive ありでメタデータを返す
        let tmp_dir = tempfile::tempdir().unwrap();
        let dir = tmp_dir.path().join("subdir");
        std::fs::create_dir(&dir).unwrap();

        let metadata = super::fetch_target_metadata(&dir, false, true)
            .expect("ディレクトリは -r ありで Ok を返すべき")
            .expect("メタデータが存在するべき");
        assert!(
            metadata.is_dir(),
            "返されたメタデータはディレクトリを示すべき"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_fetch_target_metadata_symlink_to_dir_does_not_require_recursive() {
        // ディレクトリへの symlink は symlink_metadata 上はディレクトリでないため、
        // recursive なしでもメタデータを返す（リンクエントリ自体が削除対象）
        let tmp_dir = tempfile::tempdir().unwrap();
        let target_dir = tmp_dir.path().join("target_dir");
        std::fs::create_dir(&target_dir).unwrap();
        let link = tmp_dir.path().join("link_to_dir");
        std::os::unix::fs::symlink(&target_dir, &link).unwrap();

        let metadata = super::fetch_target_metadata(&link, false, false)
            .expect("symlink は -r なしで Ok を返すべき")
            .expect("メタデータが存在するべき");
        assert!(
            metadata.file_type().is_symlink(),
            "返されたメタデータは symlink を示すべき"
        );
    }
}
