//! Error types for safe-rm
//!
//! Defines SafeRmError and related types for handling all error states.

use std::fmt;
use std::path::PathBuf;

/// File status for Git-tracked files (forward declaration for error messages)
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

/// safe-rm のエラー型
#[derive(Debug)]
pub enum SafeRmError {
    // ファイル操作エラー（Exit 1）
    /// ファイルが見つからない
    NotFound(PathBuf),
    /// ディレクトリに -r フラグなし
    IsDirectory(PathBuf),
    /// 部分的な失敗
    PartialFailure { success: usize, failed: usize },

    // ブロックエラー（Exit 2）
    /// シェル展開を含むパス（セキュリティリスク）
    ShellExpansionDetected { path: String, pattern: String },
    /// 危険なオプションの使用
    DangerousOption { option: String },
    /// ディレクトリ読み取り失敗（fail-closed）
    DirectoryReadError { path: PathBuf },
    /// プロジェクト外へのアクセス
    OutsideProject {
        path: PathBuf,
        project_root: PathBuf,
    },
    /// 未コミット変更のあるファイル
    DirtyFiles { path: PathBuf, status: FileStatus },

    // システムエラー（Exit 1）
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
            | Self::OutsideProject { .. }
            | Self::DirtyFiles { .. } => 2,
            // ファイル操作エラー
            Self::NotFound(_) | Self::IsDirectory(_) | Self::PartialFailure { .. } => 1,
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
            Self::PartialFailure { success, failed } => {
                format!("{} file(s) removed, {} failed", success, failed)
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
            SafeRmError::PartialFailure {
                success: 2,
                failed: 1
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
    fn test_user_message_partial_failure() {
        let err = SafeRmError::PartialFailure {
            success: 3,
            failed: 2,
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
        // Clean, Ignored, NotInRepo are deletable
        assert!(matches!(FileStatus::Clean, FileStatus::Clean));
        assert!(matches!(FileStatus::Ignored, FileStatus::Ignored));
        assert!(matches!(FileStatus::NotInRepo, FileStatus::NotInRepo));

        // Modified, Staged, Untracked are not deletable
        assert!(!matches!(FileStatus::Modified, FileStatus::Clean));
        assert!(!matches!(FileStatus::Staged, FileStatus::Clean));
        assert!(!matches!(FileStatus::Untracked, FileStatus::Clean));
    }

    // Security: セキュリティ関連エラーのテスト

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

    // --- IoError / GitError のテスト ---

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

    // --- std::error::Error の source() テスト ---

    #[test]
    fn test_source_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "io test");
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
}
