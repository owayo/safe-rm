//! safe-rm のエラー型
//!
//! SafeRmError および関連型を定義し、全エラー状態を処理する。

use std::fmt;
use std::path::{Path, PathBuf};

/// Git 追跡ファイルのステータス（エラーメッセージ用の前方宣言）
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileStatus {
    /// HEAD と一致（削除許可）
    Clean,
    /// .gitignore 対象（削除許可）
    Ignored,
    /// 変更あり・未ステージ（削除禁止）
    Modified,
    /// git add 済み（削除禁止）
    Staged,
    /// 未追跡（削除禁止）
    Untracked,
    /// Git 管理外
    NotInRepo,
}

impl FileStatus {
    /// ステータスが削除許可かどうかを判定
    ///
    /// Clean / Ignored / NotInRepo は安全に削除可能。
    /// Modified / Staged / Untracked は未コミットの変更があるため削除不可。
    pub fn is_deletable(self) -> bool {
        matches!(self, Self::Clean | Self::Ignored | Self::NotInRepo)
    }
}

impl fmt::Display for FileStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clean => write!(f, "Clean"),
            Self::Ignored => write!(f, "Ignored"),
            Self::Modified => write!(f, "Modified"),
            Self::Staged => write!(f, "Staged"),
            Self::Untracked => write!(f, "Untracked"),
            Self::NotInRepo => write!(f, "NotInRepo"),
        }
    }
}

/// git-lock サブコマンドが index.lock の削除を拒否した理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitLockRefusal {
    /// 設定で無効になっている（`[git_locks] enabled = false`、または設定を読めず strict に倒れた）
    Disabled,
    /// `--assume-quiescent` が無い
    QuiescenceNotAsserted,
    /// 通常の削除のオプションや PATH と併用された
    ConflictingOptions,
    /// 検査を実装していない OS
    UnsupportedPlatform { os: &'static str },
    /// `GIT_INDEX_FILE` が設定されている
    IndexFileOverridden,
    /// `--repo` をリポジトリとして開けない
    RepositoryOpenFailed { message: String },
    /// bare リポジトリ
    BareRepository,
    /// 通常ファイルでない
    NotRegularFile { kind: &'static str },
    /// 空でない
    NotEmpty { size: u64 },
    /// リンク数が 1 でない
    UnexpectedLinkCount { nlink: u64 },
    /// 所有者が実効ユーザーでない
    NotOwned { owner: u32, euid: u32 },
    /// 最終更新から最小経過時間がたっていない
    TooRecent { age_secs: u64, min_age_secs: u64 },
    /// 最終更新時刻が未来
    ModifiedInFuture { ahead_secs: u64 },
    /// ロックを開いているプロセスがある
    OpenByProcess { pids: Vec<u32> },
    /// 開いているプロセスを調べきれない
    InspectionIncomplete { reason: String },
    /// 検査の途中でロックが変わった
    ChangedDuringInspection,
}

impl fmt::Display for GitLockRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(
                f,
                "git-lock は設定で無効です（[git_locks] enabled = false、または設定ファイルを読めず strict モードに倒れた状態）。"
            ),
            Self::QuiescenceNotAsserted => write!(
                f,
                "--assume-quiescent が必要です。これは、そのリポジトリで git の処理が動いておらず、削除が終わるまで新しい処理も gitdir を書き換えるほかの操作も始めないことを、呼び出し側が保証する指定です。git の処理が残っていないことを確かめてから付けてください。"
            ),
            Self::ConflictingOptions => write!(
                f,
                "git-lock は -r / -f / -n / --allow-nested-git / PATH と併用できません。ドライランは safe-rm git-lock --repo PATH --assume-quiescent -n のように git-lock の後に -n を付けてください。"
            ),
            Self::UnsupportedPlatform { os } => write!(
                f,
                "この OS（{}）では git-lock を使えません（対応は Linux と macOS）。",
                os
            ),
            Self::IndexFileOverridden => write!(
                f,
                "GIT_INDEX_FILE が設定されています。代わりの index を使う指定は対象外です（gitdir 直下の index.lock が、その index のロックとは限りません）。"
            ),
            Self::RepositoryOpenFailed { message } => write!(
                f,
                "リポジトリとして開けません: {}\n--repo には worktree のルートか gitdir を指定してください（親ディレクトリは探索しません）。",
                message
            ),
            Self::BareRepository => write!(f, "bare リポジトリは対象外です。"),
            Self::NotRegularFile { kind } => {
                write!(f, "index.lock が通常ファイルではありません（{}）。", kind)
            }
            Self::NotEmpty { size } => write!(
                f,
                "index.lock が空ではありません（{} バイト）。書きかけの index を含み得るため削除しません。",
                size
            ),
            Self::UnexpectedLinkCount { nlink } => write!(
                f,
                "index.lock のリンク数が {} です（1 のときだけ削除します）。",
                nlink
            ),
            Self::NotOwned { owner, euid } => write!(
                f,
                "index.lock の所有者（uid {}）が実行ユーザー（uid {}）と違います。",
                owner, euid
            ),
            Self::TooRecent {
                age_secs,
                min_age_secs,
            } => write!(
                f,
                "index.lock の最終更新から {} 秒しかたっていません（必要なのは {} 秒）。git の処理が終わっていることを確かめ、{} 秒以上待ってから再実行してください。",
                age_secs,
                min_age_secs,
                min_age_secs.saturating_sub(*age_secs)
            ),
            Self::ModifiedInFuture { ahead_secs } => write!(
                f,
                "index.lock の最終更新時刻が現在より {} 秒先です。システムの時刻を確かめてください。",
                ahead_secs
            ),
            Self::OpenByProcess { pids } => write!(
                f,
                "index.lock を開いているプロセスがあります（PID: {}）。そのプロセスが終わるのを待ってください。",
                pids.iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::InspectionIncomplete { reason } => write!(
                f,
                "index.lock を開いているプロセスを調べきれませんでした（{}）。安全のため削除しません。",
                reason
            ),
            Self::ChangedDuringInspection => write!(
                f,
                "検査の途中で index.lock が変わりました。git の処理が動いている可能性があるため削除しません。"
            ),
        }
    }
}

/// シェルにそのまま貼れるようにパスを引用する（安全な文字だけならそのまま）。
fn shell_quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    let is_plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+,:@%=".contains(c));
    if is_plain {
        text.into_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// safe-rm のエラー型
#[derive(Debug)]
pub enum SafeRmError {
    // ファイル操作エラー（Exit 1）
    /// ファイルが見つからない
    NotFound(PathBuf),
    /// ディレクトリに -r フラグなし
    IsDirectory(PathBuf),
    /// 末尾セパレータ付き operand が非ディレクトリを指している（OS の `ENOTDIR` 相当）
    NotADirectory(PathBuf),
    /// 部分的な失敗（dry_run=true の場合は実削除ではないため表示文言を変える）
    PartialFailure {
        success: usize,
        failed: usize,
        dry_run: bool,
    },

    // ブロックエラー（Exit 2）
    /// シェル展開を含むパス（セキュリティリスク）
    ShellExpansionDetected { path: String, pattern: String },
    /// 危険なオプションの使用
    DangerousOption { option: String },
    /// ディレクトリ読み取り失敗（fail-closed）
    DirectoryReadError { path: PathBuf },
    /// シンボリックリンク経由の親ディレクトリ参照
    UnsafeTraversal { path: PathBuf },
    /// 解決不能な中間シンボリックリンクを含むパス
    DanglingIntermediateSymlink { path: PathBuf, symlink: PathBuf },
    /// 末尾成分が `.` / `..` の operand（POSIX rm が拒否する形式）
    DotOrDotDotOperand { path: PathBuf },
    /// ルートディレクトリを指す operand（POSIX rm が拒否する形式）
    RootOperand { path: PathBuf },
    /// Git 管理メタデータへのアクセス
    ProtectedGitPath {
        path: PathBuf,
        metadata_paths: Vec<PathBuf>,
    },
    /// 通常の削除で gitdir 直下の index.lock を指定した（git-lock サブコマンドを案内する）
    ProtectedGitIndexLock { path: PathBuf, gitdir: PathBuf },
    /// git-lock サブコマンドが安全条件を満たさず index.lock の削除を拒否した
    GitLockRefused {
        target: PathBuf,
        reason: GitLockRefusal,
    },
    /// 指定された Git 管理エントリが削除対象に一致しない
    InvalidGitOptIn { path: PathBuf },
    /// プロジェクト外へのアクセス
    OutsideProject {
        path: PathBuf,
        project_root: PathBuf,
    },
    /// 未コミット変更のあるファイル
    DirtyFiles { path: PathBuf, status: FileStatus },

    // システムエラー（Exit 1）
    /// git-lock が削除した名前は、検査したファイルではなかった（検査と削除の間に差し替えられた）
    GitLockReplaced { path: PathBuf },
    /// I/O エラー
    IoError(std::io::Error),
    /// Git 操作エラー
    GitError(git2::Error),
}

impl SafeRmError {
    /// 終了コードを取得
    pub fn exit_code(&self) -> u8 {
        match self {
            // ブロック（安全のため削除を拒否）
            Self::ShellExpansionDetected { .. }
            | Self::DangerousOption { .. }
            | Self::DirectoryReadError { .. }
            | Self::UnsafeTraversal { .. }
            | Self::DanglingIntermediateSymlink { .. }
            | Self::DotOrDotDotOperand { .. }
            | Self::RootOperand { .. }
            | Self::ProtectedGitPath { .. }
            | Self::ProtectedGitIndexLock { .. }
            | Self::GitLockRefused { .. }
            | Self::InvalidGitOptIn { .. }
            | Self::OutsideProject { .. }
            | Self::DirtyFiles { .. } => 2,
            // ファイル操作エラー
            Self::NotFound(_)
            | Self::IsDirectory(_)
            | Self::NotADirectory(_)
            | Self::PartialFailure { .. } => 1,
            // その他のエラー
            _ => 1,
        }
    }

    /// AI と人間向けのエラーメッセージ
    pub fn user_message(&self) -> String {
        match self {
            Self::NotFound(path) => {
                format!(
                    "cannot remove '{}': No such file or directory",
                    path.display()
                )
            }
            Self::IsDirectory(path) => {
                format!(
                    "cannot remove '{}': Is a directory (use -r for recursive)",
                    path.display()
                )
            }
            Self::NotADirectory(path) => {
                format!("cannot remove '{}': Not a directory", path.display())
            }
            Self::PartialFailure {
                success,
                failed,
                dry_run,
            } => {
                let verb = if *dry_run {
                    "would be removed"
                } else {
                    "removed"
                };
                format!("{} file(s) {}, {} failed", success, verb, failed)
            }
            Self::ShellExpansionDetected { path, pattern } => {
                format!(
                    "シェル展開を含むパスは許可されていません。\nPath: {}\nPattern: {}\nシェル展開なしの絶対パスを使用してください。",
                    path, pattern
                )
            }
            Self::DangerousOption { option } => {
                format!(
                    "危険なオプションは許可されていません: {}\nファイルを直接指定してください。",
                    option
                )
            }
            Self::DirectoryReadError { path } => {
                format!(
                    "ディレクトリの読み取りに失敗しました（安全のため削除をブロック）。\nPath: {}",
                    path.display()
                )
            }
            Self::UnsafeTraversal { path } => {
                format!(
                    "`..` で消える成分が通常ディレクトリでないパスは許可されていません。\nPath: {}\nシンボリックリンク・ファイル・存在しない/読み取り不能な中間成分を含むパスは、字句正規化で別の削除対象に化ける可能性があるため、実体パスを直接指定してください。",
                    path.display()
                )
            }
            Self::DanglingIntermediateSymlink { path, symlink } => {
                format!(
                    "解決できない中間シンボリックリンクを含むパスは許可されていません。\nPath: {}\nSymlink: {}\nリンク先を作成するか、実体パスを直接指定してください。",
                    path.display(),
                    symlink.display()
                )
            }
            Self::DotOrDotDotOperand { path } => {
                format!(
                    "末尾が '.' または '..' のパスは削除できません。\nPath: {}\nPOSIX の rm も同じ operand を拒否します。削除したいディレクトリ名を明示的に指定してください。",
                    path.display()
                )
            }
            Self::RootOperand { path } => {
                format!(
                    "ルートディレクトリは削除できません。\nPath: {}\nPOSIX の rm も同じ operand を拒否します（GNU rm の --preserve-root 相当）。削除したい対象を個別に指定してください。",
                    path.display()
                )
            }
            Self::ProtectedGitPath {
                path,
                metadata_paths,
            } => {
                if metadata_paths.is_empty() {
                    format!(
                        "Git 管理メタデータは削除できません。\nPath: {}\n作業ツリーのファイルだけを指定してください。",
                        path.display()
                    )
                } else {
                    let entries = metadata_paths
                        .iter()
                        .map(|entry| format!("  {}", entry.display()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!(
                        "Git 管理メタデータは削除できません。\nPath: {}\n含まれる Git 管理エントリ ({} 件):\n{}\nignore 済みの単一ディレクトリなら、削除する各エントリを --allow-nested-git PATH で明示してください。",
                        path.display(),
                        metadata_paths.len(),
                        entries
                    )
                }
            }
            Self::ProtectedGitIndexLock { path, gitdir } => {
                format!(
                    "Git 管理メタデータは削除できません。\nPath: {}\n異常終了で残った index.lock なら、git の処理が動いていないことを確かめてから、次のコマンドで削除できます:\n  safe-rm git-lock --repo {} --assume-quiescent",
                    path.display(),
                    shell_quote(gitdir)
                )
            }
            Self::GitLockRefused { target, reason } => {
                format!(
                    "index.lock を削除できません。\nPath: {}\n{}",
                    target.display(),
                    reason
                )
            }
            Self::GitLockReplaced { path } => {
                format!(
                    "index.lock の名前を削除しましたが、検査したファイルではありませんでした（検査と削除の間に差し替えられた可能性があります）。\nPath: {}\nリポジトリの状態を確かめてください。",
                    path.display()
                )
            }
            Self::InvalidGitOptIn { path } => {
                format!(
                    "--allow-nested-git のパスが削除対象内の Git 管理ディレクトリと一致しないか、gitdir ファイル・symlink のため許可できません。\nPath: {}",
                    path.display()
                )
            }
            Self::OutsideProject { path, project_root } => {
                format!(
                    "プロジェクト外へのアクセスは禁止されています。\nPath: {}\nProject: {}",
                    path.display(),
                    project_root.display()
                )
            }
            Self::DirtyFiles { path, status } => {
                format!(
                    "未コミットの変更があるファイルは削除できません。\nPath: {}\nStatus: {}\n先にgit commitしてください。",
                    path.display(),
                    status
                )
            }
            Self::IoError(e) => format!("I/O error: {}", e),
            Self::GitError(e) => format!("Git error: {}", e),
        }
    }
}

impl fmt::Display for SafeRmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.user_message())
    }
}

impl std::error::Error for SafeRmError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::IoError(e) => Some(e),
            Self::GitError(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for SafeRmError {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err)
    }
}

impl From<git2::Error> for SafeRmError {
    fn from(err: git2::Error) -> Self {
        Self::GitError(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn test_exit_code_block_errors_return_2() {
        assert_eq!(
            SafeRmError::OutsideProject {
                path: PathBuf::from("/etc/passwd"),
                project_root: PathBuf::from("/home/user/project")
            }
            .exit_code(),
            2
        );
        assert_eq!(
            SafeRmError::DirtyFiles {
                path: PathBuf::from("./file.txt"),
                status: FileStatus::Modified
            }
            .exit_code(),
            2
        );
        assert_eq!(
            SafeRmError::ProtectedGitPath {
                path: PathBuf::from(".git"),
                metadata_paths: Vec::new(),
            }
            .exit_code(),
            2
        );
        assert_eq!(
            SafeRmError::UnsafeTraversal {
                path: PathBuf::from("link/../victim.txt")
            }
            .exit_code(),
            2
        );
        assert_eq!(
            SafeRmError::DanglingIntermediateSymlink {
                path: PathBuf::from("link/child.txt"),
                symlink: PathBuf::from("/project/link")
            }
            .exit_code(),
            2
        );
        assert_eq!(
            SafeRmError::DotOrDotDotOperand {
                path: PathBuf::from(".")
            }
            .exit_code(),
            2
        );
    }

    #[test]
    fn test_exit_code_file_errors_return_1() {
        assert_eq!(
            SafeRmError::NotFound(PathBuf::from("missing.txt")).exit_code(),
            1
        );
        assert_eq!(
            SafeRmError::IsDirectory(PathBuf::from("./dir")).exit_code(),
            1
        );
        assert_eq!(
            SafeRmError::NotADirectory(PathBuf::from("./file.txt/")).exit_code(),
            1
        );
        assert_eq!(
            SafeRmError::PartialFailure {
                success: 2,
                failed: 1,
                dry_run: false,
            }
            .exit_code(),
            1
        );
    }

    #[test]
    fn test_user_message_not_found() {
        let err = SafeRmError::NotFound(PathBuf::from("missing.txt"));
        let msg = err.user_message();
        assert!(msg.contains("missing.txt"));
        assert!(msg.contains("No such file"));
    }

    #[test]
    fn test_user_message_is_directory() {
        let err = SafeRmError::IsDirectory(PathBuf::from("./mydir"));
        let msg = err.user_message();
        assert!(msg.contains("mydir"));
        assert!(msg.contains("Is a directory"));
        assert!(msg.contains("-r"));
    }

    #[test]
    fn test_root_operand_exit_code_and_message() {
        let err = SafeRmError::RootOperand {
            path: PathBuf::from("/"),
        };
        assert_eq!(err.exit_code(), 2);
        let msg = err.user_message();
        assert!(msg.contains("ルートディレクトリ"));
        assert!(msg.contains("Path: /"));
    }

    #[test]
    fn test_user_message_not_a_directory() {
        // 末尾セパレータ付きの生 operand をそのまま見せて、rm と同じ診断に揃える。
        let err = SafeRmError::NotADirectory(PathBuf::from("./file.txt/"));
        let msg = err.user_message();
        assert!(msg.contains("./file.txt/"));
        assert!(msg.contains("Not a directory"));
    }

    #[test]
    fn test_user_message_partial_failure() {
        let err = SafeRmError::PartialFailure {
            success: 3,
            failed: 2,
            dry_run: false,
        };
        let msg = err.user_message();
        assert!(msg.contains("3 file(s) removed"));
        assert!(msg.contains("2 failed"));
    }

    #[test]
    fn test_user_message_outside_project() {
        let err = SafeRmError::OutsideProject {
            path: PathBuf::from("/etc/passwd"),
            project_root: PathBuf::from("/home/user/project"),
        };
        let msg = err.user_message();
        assert!(msg.contains("/etc/passwd"));
        assert!(msg.contains("/home/user/project"));
    }

    #[test]
    fn test_user_message_dirty_files() {
        let err = SafeRmError::DirtyFiles {
            path: PathBuf::from("./modified.txt"),
            status: FileStatus::Modified,
        };
        let msg = err.user_message();
        assert!(msg.contains("modified.txt"));
        assert!(msg.contains("Modified"));
        assert!(msg.contains("git commit"));
    }

    #[test]
    fn test_display_trait() {
        let err = SafeRmError::NotFound(PathBuf::from("test.txt"));
        let displayed = format!("{}", err);
        assert!(displayed.contains("test.txt"));
    }

    #[test]
    fn test_file_status_display() {
        assert_eq!(format!("{}", FileStatus::Clean), "Clean");
        assert_eq!(format!("{}", FileStatus::Modified), "Modified");
        assert_eq!(format!("{}", FileStatus::Staged), "Staged");
        assert_eq!(format!("{}", FileStatus::Untracked), "Untracked");
        assert_eq!(format!("{}", FileStatus::Ignored), "Ignored");
        assert_eq!(format!("{}", FileStatus::NotInRepo), "NotInRepo");
    }

    #[test]
    fn test_file_status_is_deletable() {
        // Clean / Ignored / NotInRepo は削除可能
        assert!(FileStatus::Clean.is_deletable());
        assert!(FileStatus::Ignored.is_deletable());
        assert!(FileStatus::NotInRepo.is_deletable());

        // Modified / Staged / Untracked は削除不可
        assert!(!FileStatus::Modified.is_deletable());
        assert!(!FileStatus::Staged.is_deletable());
        assert!(!FileStatus::Untracked.is_deletable());
    }

    // セキュリティ関連エラーのテスト

    #[test]
    fn test_exit_code_shell_expansion_returns_2() {
        assert_eq!(
            SafeRmError::ShellExpansionDetected {
                path: "~/file.txt".into(),
                pattern: "~".into()
            }
            .exit_code(),
            2
        );
    }

    #[test]
    fn test_exit_code_dangerous_option_returns_2() {
        assert_eq!(
            SafeRmError::DangerousOption {
                option: "--files0-from".into()
            }
            .exit_code(),
            2
        );
    }

    #[test]
    fn test_exit_code_directory_read_error_returns_2() {
        assert_eq!(
            SafeRmError::DirectoryReadError {
                path: PathBuf::from("/tmp/protected")
            }
            .exit_code(),
            2
        );
    }

    #[test]
    fn test_user_message_shell_expansion() {
        let err = SafeRmError::ShellExpansionDetected {
            path: "~/secret".into(),
            pattern: "~".into(),
        };
        let msg = err.user_message();
        assert!(msg.contains("~/secret"));
        assert!(msg.contains("~"));
        assert!(msg.contains("シェル展開"));
    }

    #[test]
    fn test_user_message_dangerous_option() {
        let err = SafeRmError::DangerousOption {
            option: "--files0-from".into(),
        };
        let msg = err.user_message();
        assert!(msg.contains("--files0-from"));
        assert!(msg.contains("危険なオプション"));
    }

    #[test]
    fn test_user_message_directory_read_error() {
        let err = SafeRmError::DirectoryReadError {
            path: PathBuf::from("/tmp/unreadable"),
        };
        let msg = err.user_message();
        assert!(msg.contains("/tmp/unreadable"));
        assert!(msg.contains("ディレクトリの読み取り"));
    }

    #[test]
    fn test_user_message_unsafe_traversal() {
        let err = SafeRmError::UnsafeTraversal {
            path: PathBuf::from("link/../victim.txt"),
        };
        let msg = err.user_message();
        // メッセージは「`..` で消える成分が通常ディレクトリでない」ケース全般を扱う
        // ため、symlink/ファイル/存在しない/読み取り不能の各バリアントを示唆する文言を確認する。
        assert!(
            msg.contains("通常ディレクトリ"),
            "通常ディレクトリ要件を案内すべき: {}",
            msg
        );
        assert!(
            msg.contains("シンボリックリンク") || msg.contains("実体パス"),
            "代表的なケースの説明が含まれるべき: {}",
            msg
        );
        assert!(msg.contains("link/../victim.txt"));
    }

    #[test]
    fn test_user_message_dangling_intermediate_symlink() {
        let err = SafeRmError::DanglingIntermediateSymlink {
            path: PathBuf::from("link/child.txt"),
            symlink: PathBuf::from("/project/link"),
        };
        let msg = err.user_message();
        assert!(msg.contains("解決できない中間シンボリックリンク"));
        assert!(msg.contains("link/child.txt"));
        assert!(msg.contains("/project/link"));
    }

    #[test]
    fn test_user_message_dot_or_dotdot_operand() {
        let err = SafeRmError::DotOrDotDotOperand {
            path: PathBuf::from("sub/.."),
        };
        let msg = err.user_message();
        assert!(msg.contains("末尾が '.' または '..'"), "msg: {}", msg);
        assert!(msg.contains("sub/.."), "msg: {}", msg);
    }

    #[test]
    fn test_user_message_protected_git_path() {
        let err = SafeRmError::ProtectedGitPath {
            path: PathBuf::from(".git"),
            metadata_paths: Vec::new(),
        };
        let msg = err.user_message();
        assert!(msg.contains(".git"));
        assert!(msg.contains("Git 管理メタデータ"));
    }

    #[test]
    fn test_protected_git_index_lock_guides_to_git_lock() {
        let err = SafeRmError::ProtectedGitIndexLock {
            path: PathBuf::from(".git/index.lock"),
            gitdir: PathBuf::from("/work/repo/.git"),
        };
        assert_eq!(err.exit_code(), 2);
        let msg = err.user_message();
        assert!(msg.contains("Git 管理メタデータは削除できません。"));
        assert!(msg.contains("Path: .git/index.lock"));
        assert!(msg.contains("safe-rm git-lock --repo /work/repo/.git --assume-quiescent"));
    }

    #[test]
    fn test_protected_git_index_lock_quotes_gitdir_for_the_shell() {
        let err = SafeRmError::ProtectedGitIndexLock {
            path: PathBuf::from(".git/index.lock"),
            gitdir: PathBuf::from("/work/my repo/it's/.git"),
        };
        assert!(
            err.user_message()
                .contains("--repo '/work/my repo/it'\\''s/.git' --assume-quiescent")
        );
    }

    #[test]
    fn test_shell_quote() {
        assert_eq!(shell_quote(Path::new("/a/b-c_d.e/.git")), "/a/b-c_d.e/.git");
        assert_eq!(shell_quote(Path::new("/a b")), "'/a b'");
        assert_eq!(shell_quote(Path::new("/a$b")), "'/a$b'");
        assert_eq!(shell_quote(Path::new("/日本語")), "'/日本語'");
        assert_eq!(shell_quote(Path::new("")), "''");
    }

    #[test]
    fn test_git_lock_refused_is_a_block_with_reason() {
        let err = SafeRmError::GitLockRefused {
            target: PathBuf::from("/work/repo/.git/index.lock"),
            reason: GitLockRefusal::TooRecent {
                age_secs: 18,
                min_age_secs: 300,
            },
        };
        assert_eq!(err.exit_code(), 2);
        let msg = err.user_message();
        assert!(
            msg.starts_with("index.lock を削除できません。\nPath: /work/repo/.git/index.lock\n")
        );
        assert!(msg.contains("最終更新から 18 秒しかたっていません（必要なのは 300 秒）"));
        assert!(msg.contains("282 秒以上待ってから再実行してください"));
    }

    #[test]
    fn test_git_lock_refusal_messages_explain_the_reason() {
        let cases = [
            (GitLockRefusal::Disabled, "enabled = false"),
            (GitLockRefusal::QuiescenceNotAsserted, "--assume-quiescent"),
            (GitLockRefusal::ConflictingOptions, "併用できません"),
            (
                GitLockRefusal::UnsupportedPlatform { os: "windows" },
                "この OS（windows）",
            ),
            (GitLockRefusal::IndexFileOverridden, "GIT_INDEX_FILE"),
            (
                GitLockRefusal::RepositoryOpenFailed {
                    message: "could not find repository".to_string(),
                },
                "could not find repository",
            ),
            (GitLockRefusal::BareRepository, "bare"),
            (
                GitLockRefusal::NotRegularFile { kind: "FIFO" },
                "通常ファイルではありません（FIFO）",
            ),
            (GitLockRefusal::NotEmpty { size: 42 }, "42 バイト"),
            (
                GitLockRefusal::UnexpectedLinkCount { nlink: 2 },
                "リンク数が 2",
            ),
            (
                GitLockRefusal::NotOwned {
                    owner: 0,
                    euid: 501,
                },
                "uid 0",
            ),
            (
                GitLockRefusal::ModifiedInFuture { ahead_secs: 61 },
                "61 秒先",
            ),
            (
                GitLockRefusal::OpenByProcess {
                    pids: vec![123, 456],
                },
                "PID: 123, 456",
            ),
            (
                GitLockRefusal::InspectionIncomplete {
                    reason: "/proc を読めません".to_string(),
                },
                "/proc を読めません",
            ),
            (GitLockRefusal::ChangedDuringInspection, "検査の途中で"),
        ];
        for (reason, expected) in cases {
            let message = reason.to_string();
            assert!(message.contains(expected), "{reason:?}: {message}");
        }
    }

    #[test]
    fn test_git_lock_replaced_is_an_operation_error() {
        let err = SafeRmError::GitLockReplaced {
            path: PathBuf::from("/work/repo/.git/index.lock"),
        };
        assert_eq!(err.exit_code(), 1);
        assert!(
            err.user_message()
                .contains("検査したファイルではありませんでした")
        );
    }

    // --- IoError / GitError 周りのテスト ---

    #[test]
    fn test_user_message_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied");
        let err = SafeRmError::IoError(io_err);
        let msg = err.user_message();
        assert!(msg.contains("I/O error"));
        assert!(msg.contains("permission denied"));
    }

    #[test]
    fn test_user_message_git_error() {
        let git_err = git2::Error::from_str("repository not found");
        let err = SafeRmError::GitError(git_err);
        let msg = err.user_message();
        assert!(msg.contains("Git error"));
        assert!(msg.contains("repository not found"));
    }

    #[test]
    fn test_exit_code_io_error_returns_1() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        assert_eq!(SafeRmError::IoError(io_err).exit_code(), 1);
    }

    #[test]
    fn test_exit_code_git_error_returns_1() {
        let git_err = git2::Error::from_str("test error");
        assert_eq!(SafeRmError::GitError(git_err).exit_code(), 1);
    }

    // --- std::error::Error の source() に関するテスト ---

    #[test]
    fn test_source_io_error() {
        let io_err = std::io::Error::other("io test");
        let err = SafeRmError::IoError(io_err);
        assert!(err.source().is_some(), "IoError should have a source");
    }

    #[test]
    fn test_source_git_error() {
        let git_err = git2::Error::from_str("git test");
        let err = SafeRmError::GitError(git_err);
        assert!(err.source().is_some(), "GitError should have a source");
    }

    #[test]
    fn test_source_not_found_is_none() {
        let err = SafeRmError::NotFound(PathBuf::from("test.txt"));
        assert!(err.source().is_none(), "NotFound should not have a source");
    }

    #[test]
    fn test_source_outside_project_is_none() {
        let err = SafeRmError::OutsideProject {
            path: PathBuf::from("/etc/passwd"),
            project_root: PathBuf::from("/project"),
        };
        assert!(
            err.source().is_none(),
            "OutsideProject should not have a source"
        );
    }

    // --- From トレイトのテスト ---

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err: SafeRmError = io_err.into();
        assert!(matches!(err, SafeRmError::IoError(_)));
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn test_from_git2_error() {
        let git_err = git2::Error::from_str("bad ref");
        let err: SafeRmError = git_err.into();
        assert!(matches!(err, SafeRmError::GitError(_)));
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn test_user_message_dirty_files_staged() {
        let err = SafeRmError::DirtyFiles {
            path: PathBuf::from("./staged.txt"),
            status: FileStatus::Staged,
        };
        let msg = err.user_message();
        assert!(msg.contains("staged.txt"));
        assert!(msg.contains("Staged"));
        assert!(msg.contains("git commit"));
    }

    #[test]
    fn test_user_message_dirty_files_untracked() {
        let err = SafeRmError::DirtyFiles {
            path: PathBuf::from("./new.txt"),
            status: FileStatus::Untracked,
        };
        let msg = err.user_message();
        assert!(msg.contains("new.txt"));
        assert!(msg.contains("Untracked"));
        assert!(msg.contains("git commit"));
    }

    #[test]
    fn test_file_status_not_in_repo_display() {
        // NotInRepo の Display 表示が正しいことを検証
        let status = FileStatus::NotInRepo;
        assert_eq!(format!("{}", status), "NotInRepo");
    }

    #[test]
    fn test_user_message_partial_failure_dry_run() {
        // dry_run=true の場合は "would be removed" 表記になること
        let err = SafeRmError::PartialFailure {
            success: 2,
            failed: 1,
            dry_run: true,
        };
        let msg = err.user_message();
        assert!(
            msg.contains("would be removed"),
            "ドライラン時は would be removed 表記にすべき: {}",
            msg
        );
        assert!(
            !msg.contains(", removed"),
            "removed は使わないべき: {}",
            msg
        );
    }

    #[test]
    fn test_exit_code_partial_failure_returns_1() {
        // PartialFailure の終了コードが 1 であることを検証
        let err = SafeRmError::PartialFailure {
            success: 5,
            failed: 3,
            dry_run: false,
        };
        assert_eq!(err.exit_code(), 1);
    }
}
