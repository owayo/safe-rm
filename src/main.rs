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
    // POSIX の rm は operand の basename が `.` / `..` のとき何も削除しない。
    // safe-rm でも同じ operand を拒否しないと `safe-rm -r .` で cwd 自体が、
    // `safe-rm -r ..` で親ディレクトリが消えてしまう。allowed_paths のバイパスや
    // `-f` に先んじて拒否するため、正規化・許可判定より前に評価する。
    PathChecker::reject_dot_or_dotdot_operand(path)?;

    // 絶対パスに変換（相対パスは cwd から解決、git root からではない）
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    PathChecker::reject_symlink_parent_traversal(cwd, path)?;
    PathChecker::reject_dangling_intermediate_symlink(cwd, path)?;

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
            let delete_result =
                delete_path_with_metadata(&normalized_path, args.recursive, &metadata);
            // 実削除でワークツリーが変化するため、後続パスの strict 判定が削除前の
            // 古い status を再利用しないよう status キャッシュを破棄する。allowed_paths
            // のファイルでも repo 内にあれば後続パスの status に影響し得る。remove_dir_all
            // は途中まで進んでから失敗し得るので `?` より前に無効化する。
            *status_cache = None;
            delete_result?;
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
            check_strict_git_status(
                path,
                &normalized_path,
                &canonical_path,
                &metadata,
                git_context,
                status_cache,
            )?;
        }

        // 削除実行（またはドライラン）
        if args.dry_run {
            println!("would remove: {}", path.display());
            Ok(true)
        } else {
            let delete_result =
                delete_path_with_metadata(&normalized_path, args.recursive, &metadata);
            // 実削除でワークツリーが変化するため、後続パスの strict 判定が削除前の
            // 古い status を再利用しないよう status キャッシュを破棄する。remove_dir_all
            // は途中まで進んでから失敗し得るので `?` より前に無効化する。
            *status_cache = None;
            delete_result?;
            println!("removed: {}", path.display());
            Ok(true)
        }
    }
}

/// 対象を管理する最も深い Git リポジトリで strict モードの削除可否を検査する。
fn check_strict_git_status(
    path: &Path,
    normalized_path: &Path,
    canonical_path: &Path,
    metadata: &std::fs::Metadata,
    git_context: &GitContext,
    status_cache: &mut Option<(PathBuf, HashMap<Vec<u8>, FileStatus>)>,
) -> Result<(), SafeRmError> {
    // シンボリックリンクの場合、親ディレクトリのみ canonicalize し
    // リンク名自体は保持。「リンク自体をチェック」するセマンティクスを
    // 維持しつつ、リポジトリエイリアスパスを解決する。
    let symlink_git_check_path: Option<PathBuf> = if metadata.file_type().is_symlink() {
        match (normalized_path.file_name(), normalized_path.parent()) {
            (Some(name), Some(parent)) => Some(
                parent
                    .canonicalize()
                    .map_err(SafeRmError::IoError)?
                    .join(name),
            ),
            _ => Some(normalized_path.to_path_buf()),
        }
    } else {
        None
    };
    let git_check_path = symlink_git_check_path.as_deref().unwrap_or(canonical_path);

    // 対象を含む repo のうち最も深い workdir を strict チェックに使う。
    // cwd の checker だけでは、cwd 側 repo の配下に nested repo がある場合に、
    // 外側 repo の Ignored / NotInRepo 判定で内側 repo の modified/staged を
    // 見落としてしまう。これを防ぐため、対象起点でも常に discover を試行する。
    let discover_from: PathBuf =
        if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
            git_check_path.to_path_buf()
        } else {
            git_check_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| git_check_path.to_path_buf())
        };
    // ライフタイム制約のため、所有を持つ Option<GitChecker> を外側に置く。
    let discovered_checker_owned: Option<GitChecker> = GitChecker::open(&discover_from)?;

    // core.worktree 等で論理ワークツリーがリダイレクトされた repo を検出する。
    // git2 の workdir() は core.worktree のリダイレクト先を返すため、対象起点で
    // discover した repo の workdir が git_check_path を含まないと、
    // to_workdir_relative が None → NotInRepo（削除可能）に落ちる fail-open が生じる。
    // git は実際この対象をダーティと報告するため、安全側で Modified 扱いにして
    // fail-closed でブロックする。bare リポジトリ（workdir None）は Git 管理
    // メタデータ保護で別途守られるため対象外。
    ensure_discovered_workdir_contains_target(
        path,
        git_check_path,
        discovered_checker_owned.as_ref(),
    )?;

    let target_checker = select_target_checker(
        git_check_path,
        git_context.checker(),
        discovered_checker_owned.as_ref(),
    );

    if let Some(checker) = target_checker {
        check_path_with_status_cache(checker, git_check_path, status_cache)?;
    }

    Ok(())
}

/// discover したリポジトリの論理ワークツリーが対象を含むことを検証する。
fn ensure_discovered_workdir_contains_target(
    original_path: &Path,
    git_check_path: &Path,
    discovered_checker: Option<&GitChecker>,
) -> Result<(), SafeRmError> {
    let Some(discovered) = discovered_checker else {
        return Ok(());
    };
    let Some(discovered_workdir) = discovered.workdir() else {
        return Ok(());
    };
    if git_check_path.starts_with(&discovered_workdir) {
        return Ok(());
    }

    Err(SafeRmError::DirtyFiles {
        path: original_path.to_path_buf(),
        status: FileStatus::Modified,
    })
}

/// 対象を管理する Git リポジトリのうち、最も深い workdir の checker を選ぶ。
fn select_target_checker<'a>(
    git_check_path: &Path,
    cwd_checker: Option<&'a GitChecker>,
    discovered_checker: Option<&'a GitChecker>,
) -> Option<&'a GitChecker> {
    // 選択ロジック:
    // - cwd checker と discovered が両方とも対象を含み、discovered の方が
    //   深い workdir なら discovered（nested repo 優先）
    // - 上記以外で cwd workdir が対象を含むなら cwd_checker
    // - それ以外で discovered があるなら discovered
    // - どちらも対象を含まない場合は None（strict チェック対象なし）
    match (cwd_checker, discovered_checker) {
        (Some(cwd_checker), Some(discovered)) => {
            match (cwd_checker.workdir(), discovered.workdir()) {
                (Some(cwd_workdir), Some(discovered_workdir))
                    if git_check_path.starts_with(&discovered_workdir)
                        && discovered_workdir.starts_with(&cwd_workdir)
                        && discovered_workdir != cwd_workdir =>
                {
                    Some(discovered)
                }
                (Some(cwd_workdir), _) if git_check_path.starts_with(&cwd_workdir) => {
                    Some(cwd_checker)
                }
                _ => Some(discovered),
            }
        }
        (Some(cwd_checker), None) => cwd_checker
            .workdir()
            .filter(|workdir| git_check_path.starts_with(workdir))
            .map(|_| cwd_checker),
        (None, Some(discovered)) => Some(discovered),
        (None, None) => None,
    }
}

/// Git ステータスキャッシュを必要に応じて更新し、対象の削除可否を検査する。
fn check_path_with_status_cache(
    checker: &GitChecker,
    git_check_path: &Path,
    status_cache: &mut Option<(PathBuf, HashMap<Vec<u8>, FileStatus>)>,
) -> Result<(), SafeRmError> {
    // bare リポジトリは workdir が None。bare の管理ファイル直接削除は
    // Git 管理メタデータ保護で既にブロック済みなので、ここでは何もしない。
    let Some(workdir) = checker.workdir() else {
        return Ok(());
    };
    let cache_hit = status_cache
        .as_ref()
        .map(|(cache_workdir, _)| cache_workdir == &workdir)
        .unwrap_or(false);
    if !cache_hit {
        *status_cache = Some((workdir, checker.get_all_statuses()?));
    }
    let cache = &status_cache
        .as_ref()
        .expect("strict Git チェック前に status_cache が初期化されている必要がある")
        .1;
    checker.check_path_with_cache(git_check_path, cache)
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

    #[test]
    fn test_ensure_git_metadata_not_targeted_blocks_via_current_repo_checker() {
        // git_checker が Some で、削除対象が現在のリポジトリの `.git` を指している場合に
        // touches_git_metadata_path 経由でブロックされることを検証する。
        // None 引数のテストでは `path_has_dot_git_component` の経路のみがカバーされ、
        // checker 経由の保護経路がカバーされない。
        use safe_rm::git_checker::GitChecker;
        use std::process::Command;

        let tmp_dir = tempfile::tempdir().unwrap();
        let repo_path = tmp_dir.path().canonicalize().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = GitChecker::open(&repo_path)
            .expect("Git API エラーは想定外")
            .expect("Git リポジトリが存在すべき");

        // リポジトリのルートを再帰削除しようとすると、配下の `.git` が含まれるためブロック
        let result =
            super::ensure_git_metadata_not_targeted(&repo_path, &repo_path, true, Some(&checker));

        assert!(
            matches!(result, Err(super::SafeRmError::ProtectedGitPath { .. })),
            "現在の repo の `.git` を含む再帰削除は ProtectedGitPath で拒否されるべき: {:?}",
            result
        );
    }

    #[test]
    fn test_ensure_git_metadata_not_targeted_allows_clean_path_in_repo() {
        // git_checker が Some でも、`.git` メタデータに触れない通常パスは許可される。
        use safe_rm::git_checker::GitChecker;
        use std::process::Command;

        let tmp_dir = tempfile::tempdir().unwrap();
        let repo_path = tmp_dir.path().canonicalize().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = GitChecker::open(&repo_path)
            .expect("Git API エラーは想定外")
            .expect("Git リポジトリが存在すべき");

        // 通常ファイルの単体削除は Git 管理メタデータ保護を通過する
        let plain_file = repo_path.join("note.txt");
        std::fs::write(&plain_file, "content").unwrap();

        let result = super::ensure_git_metadata_not_targeted(
            &plain_file,
            &plain_file,
            false,
            Some(&checker),
        );

        assert!(
            result.is_ok(),
            "通常パスは現在の repo の checker 経由でも Git 管理メタデータ判定を通すべき: {:?}",
            result
        );
    }

    #[test]
    fn test_select_target_checker_prefers_deepest_containing_repository() {
        use safe_rm::git_checker::GitChecker;
        use std::process::Command;

        let temp_dir = tempfile::tempdir().unwrap();
        let outer_path = temp_dir.path().canonicalize().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(&outer_path)
            .output()
            .unwrap();

        let nested_path = outer_path.join("nested");
        std::fs::create_dir(&nested_path).unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(&nested_path)
            .output()
            .unwrap();

        let outer_checker = GitChecker::open(&outer_path)
            .expect("外側リポジトリの検出に成功すべき")
            .expect("外側リポジトリが存在すべき");
        let nested_checker = GitChecker::open(&nested_path)
            .expect("内側リポジトリの検出に成功すべき")
            .expect("内側リポジトリが存在すべき");

        let nested_target = nested_path.join("file.txt");
        let selected = super::select_target_checker(
            &nested_target,
            Some(&outer_checker),
            Some(&nested_checker),
        )
        .expect("対象を含むリポジトリが選択されるべき");
        assert!(
            std::ptr::eq(selected, &nested_checker),
            "対象を含む最も深い nested repo が優先されるべき"
        );

        let outer_target = outer_path.join("outer.txt");
        let selected = super::select_target_checker(&outer_target, Some(&outer_checker), None)
            .expect("cwd リポジトリが対象を含む場合は選択されるべき");
        assert!(
            std::ptr::eq(selected, &outer_checker),
            "discover 結果がない場合は対象を含む cwd repo が選択されるべき"
        );

        let outside_dir = tempfile::tempdir().unwrap();
        let outside_target = outside_dir.path().join("outside.txt");
        assert!(
            super::select_target_checker(&outside_target, Some(&outer_checker), None).is_none(),
            "対象を含まない cwd repo は選択されるべきでない"
        );
    }

    #[test]
    fn test_select_target_checker_falls_back_to_discovered_and_none() {
        use safe_rm::git_checker::GitChecker;
        use std::process::Command;

        let cwd_dir = tempfile::tempdir().unwrap();
        let cwd_path = cwd_dir.path().canonicalize().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(&cwd_path)
            .output()
            .unwrap();

        // cwd とは無関係な別リポジトリ（sibling）に削除対象がある構成。
        let other_dir = tempfile::tempdir().unwrap();
        let other_path = other_dir.path().canonicalize().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(&other_path)
            .output()
            .unwrap();

        let cwd_checker = GitChecker::open(&cwd_path)
            .expect("cwd リポジトリの検出に成功すべき")
            .expect("cwd リポジトリが存在すべき");
        let other_checker = GitChecker::open(&other_path)
            .expect("別リポジトリの検出に成功すべき")
            .expect("別リポジトリが存在すべき");

        let other_target = other_path.join("file.txt");

        // cwd 非 Git（checker なし）でも、対象側 repo が見つかればそれを使う。
        let selected = super::select_target_checker(&other_target, None, Some(&other_checker))
            .expect("cwd checker が無くても discover 結果を使うべき");
        assert!(
            std::ptr::eq(selected, &other_checker),
            "cwd checker が None のときは discover した repo を選ぶべき"
        );

        // cwd repo が対象を含まない場合も、対象側 repo を優先する。
        let selected =
            super::select_target_checker(&other_target, Some(&cwd_checker), Some(&other_checker))
                .expect("対象を含む repo が選択されるべき");
        assert!(
            std::ptr::eq(selected, &other_checker),
            "cwd repo が対象を含まないときは別 repo の checker を選ぶべき"
        );

        // どちらの checker も無ければ strict チェック対象なし。
        assert!(
            super::select_target_checker(&other_target, None, None).is_none(),
            "checker が 1 つも無ければ None を返すべき"
        );
    }

    #[test]
    fn test_ensure_discovered_workdir_contains_target() {
        use safe_rm::git_checker::GitChecker;
        use std::process::Command;

        let repo_dir = tempfile::tempdir().unwrap();
        let repo_path = repo_dir.path().canonicalize().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        let checker = GitChecker::open(&repo_path)
            .expect("リポジトリの検出に成功すべき")
            .expect("リポジトリが存在すべき");

        // discover 結果が無い場合は検査対象外。
        assert!(
            super::ensure_discovered_workdir_contains_target(
                std::path::Path::new("file.txt"),
                &repo_path.join("file.txt"),
                None,
            )
            .is_ok(),
            "discover 結果が無い場合は素通りすべき"
        );

        // workdir が対象を含むなら通す。
        assert!(
            super::ensure_discovered_workdir_contains_target(
                std::path::Path::new("file.txt"),
                &repo_path.join("file.txt"),
                Some(&checker),
            )
            .is_ok(),
            "workdir が対象を含む場合は通すべき"
        );

        // workdir が対象を含まない（core.worktree リダイレクト相当）なら fail-closed。
        let outside_dir = tempfile::tempdir().unwrap();
        let outside_target = outside_dir.path().join("outside.txt");
        let result = super::ensure_discovered_workdir_contains_target(
            std::path::Path::new("outside.txt"),
            &outside_target,
            Some(&checker),
        );
        assert!(
            matches!(
                result,
                Err(super::SafeRmError::DirtyFiles {
                    status: super::FileStatus::Modified,
                    ..
                })
            ),
            "workdir が対象を含まない場合は Modified 扱いでブロックすべき: {:?}",
            result
        );
    }

    #[test]
    fn test_check_path_with_status_cache_swaps_cache_between_repositories() {
        use safe_rm::git_checker::GitChecker;
        use std::process::Command;

        fn init_repo_with_untracked(dir: &std::path::Path, name: &str) {
            Command::new("git")
                .args(["init"])
                .current_dir(dir)
                .output()
                .unwrap();
            std::fs::write(dir.join(name), "content").unwrap();
        }

        let first_dir = tempfile::tempdir().unwrap();
        let first_path = first_dir.path().canonicalize().unwrap();
        init_repo_with_untracked(&first_path, "first.txt");

        let second_dir = tempfile::tempdir().unwrap();
        let second_path = second_dir.path().canonicalize().unwrap();
        init_repo_with_untracked(&second_path, "second.txt");

        let first_checker = GitChecker::open(&first_path).unwrap().unwrap();
        let second_checker = GitChecker::open(&second_path).unwrap().unwrap();

        let mut status_cache = None;

        // 1 つ目の repo で未追跡ファイルがブロックされ、キャッシュが張られる。
        let result = super::check_path_with_status_cache(
            &first_checker,
            &first_path.join("first.txt"),
            &mut status_cache,
        );
        assert!(
            matches!(result, Err(super::SafeRmError::DirtyFiles { .. })),
            "未追跡ファイルはブロックされるべき: {:?}",
            result
        );
        assert_eq!(
            status_cache.as_ref().map(|(workdir, _)| workdir.clone()),
            Some(first_path.clone()),
            "1 つ目の repo の workdir がキャッシュキーになるべき"
        );

        // 別 repo に切り替わったら、古いキャッシュを流用せず入れ替える。
        let result = super::check_path_with_status_cache(
            &second_checker,
            &second_path.join("second.txt"),
            &mut status_cache,
        );
        assert!(
            matches!(result, Err(super::SafeRmError::DirtyFiles { .. })),
            "別 repo の未追跡ファイルもブロックされるべき: {:?}",
            result
        );
        assert_eq!(
            status_cache.as_ref().map(|(workdir, _)| workdir.clone()),
            Some(second_path.clone()),
            "キャッシュキーが 2 つ目の repo の workdir に入れ替わるべき"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_check_strict_git_status_fails_closed_when_symlink_parent_disappears() {
        use std::io::ErrorKind;
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let parent = temp_dir.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        let symlink_path = parent.join("link");
        symlink("target", &symlink_path).unwrap();
        let metadata = std::fs::symlink_metadata(&symlink_path).unwrap();

        // metadata 取得後に親が消える競合状態でも、未解決パスへフォールバックしない。
        std::fs::remove_file(&symlink_path).unwrap();
        std::fs::remove_dir(&parent).unwrap();

        let git_context = super::GitContext::new(temp_dir.path());
        let mut status_cache = None;
        let result = super::check_strict_git_status(
            &symlink_path,
            &symlink_path,
            &symlink_path,
            &metadata,
            &git_context,
            &mut status_cache,
        );

        assert!(
            matches!(
                result,
                Err(super::SafeRmError::IoError(ref error))
                    if error.kind() == ErrorKind::NotFound
            ),
            "symlink 親の canonicalize 失敗は I/O エラーとして伝播すべき: {:?}",
            result
        );
    }
}
