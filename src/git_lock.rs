//! 異常終了で残った Git の `index.lock` を削除する `git-lock` サブコマンド
//!
//! 通常の削除経路は Git 管理メタデータを例外なく保護する。ここでは、呼び出し側が
//! `--assume-quiescent` で静止状態（そのリポジトリで git の処理が動いておらず、削除が終わるまで
//! 新しい処理も gitdir を書き換えるほかの操作も始めない）を明示したときに限り、`--repo` から
//! 開いた gitdir の直下にある空の `index.lock` 1 つだけを、追加の検査の下で削除する。
//!
//! 検査は、ロックが残骸であることの証明ではない。git はファイルを閉じたままロックを持ち続ける
//! ことがある（`git commit -a` のエディタ待ちなど）。そのため時刻とプロセスの検査は拒否の材料に
//! とどめ、git が動いていないことの保証は呼び出し側に毎回明示させる。`allowed_paths` と
//! `allow_project_deletion` は、この削除の許可に使わない。

mod open_handles;

pub use open_handles::ScanCoverage;

use crate::config::GitLocksConfig;
use crate::error::{GitLockRefusal, SafeRmError};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 削除の対象にするロックの名前。gitdir の直下だけを見る。
pub const INDEX_LOCK_FILE_NAME: &str = "index.lock";

/// 削除の検査を実装している OS
const PLATFORM_SUPPORTED: bool = cfg!(any(target_os = "linux", target_os = "macos"));

/// git-lock サブコマンドへの指定
#[derive(Debug, Clone, Copy)]
pub struct GitLockRequest<'a> {
    /// `--repo`: worktree のルートか gitdir（親ディレクトリは探索しない）
    pub repo: &'a Path,
    /// `--assume-quiescent`
    pub assume_quiescent: bool,
    /// `-n` / `--dry-run`
    pub dry_run: bool,
}

/// git-lock サブコマンドの結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLockOutcome {
    /// canonical な gitdir に `index.lock` を足したパス
    pub lock_path: PathBuf,
    /// true なら削除した。false ならドライランで、すべての検査を通った
    pub removed: bool,
    /// 開いているプロセスの検査で対象外にしたもの
    pub coverage: ScanCoverage,
}

/// `--repo` から開いた gitdir の直下にある空の `index.lock` を、検査を通ったときだけ削除する。
///
/// 安全条件による拒否は [`SafeRmError::GitLockRefused`]（exit 2）、ロックが無いときは
/// [`SafeRmError::NotFound`]、OS の操作の失敗は [`SafeRmError::IoError`]（いずれも exit 1）を返す。
pub fn remove_index_lock(
    request: &GitLockRequest<'_>,
    policy: &GitLocksConfig,
) -> Result<GitLockOutcome, SafeRmError> {
    let refuse = |reason| SafeRmError::GitLockRefused {
        target: request.repo.to_path_buf(),
        reason,
    };
    if !policy.enabled {
        return Err(refuse(GitLockRefusal::Disabled));
    }
    if !request.assume_quiescent {
        return Err(refuse(GitLockRefusal::QuiescenceNotAsserted));
    }
    if !PLATFORM_SUPPORTED {
        return Err(refuse(GitLockRefusal::UnsupportedPlatform {
            os: std::env::consts::OS,
        }));
    }
    // 代わりの index を使う指定では、gitdir 直下の index.lock がその index のロックとは限らない
    // （git のフックの中でも設定されている）。空文字でも、設定されていれば対象外にする。
    if std::env::var_os("GIT_INDEX_FILE").is_some() {
        return Err(refuse(GitLockRefusal::IndexFileOverridden));
    }
    let gitdir = resolve_gitdir(request.repo)?;
    platform::remove_index_lock_in(request.repo, &gitdir, policy, request.dry_run)
}

/// `--repo` を探索なし・環境変数なしで開き、canonical な gitdir を返す。
///
/// 親ディレクトリの探索（`Repository::discover`）も、`GIT_DIR` などの環境変数も使わない。
/// 削除を許すのは明示した場所から確実に開けたリポジトリだけで、開けないときに構造マーカーで
/// 代用しない。linked worktree と submodule では、それぞれに固有の gitdir になる。
fn resolve_gitdir(repo: &Path) -> Result<PathBuf, SafeRmError> {
    let refuse = |reason| SafeRmError::GitLockRefused {
        target: repo.to_path_buf(),
        reason,
    };
    if repo.as_os_str().is_empty() {
        return Err(refuse(GitLockRefusal::RepositoryOpenFailed {
            message: "--repo が空です".to_string(),
        }));
    }
    let repository = git2::Repository::open_ext(
        repo,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::iter::empty::<&std::ffi::OsStr>(),
    )
    .map_err(|error| {
        refuse(GitLockRefusal::RepositoryOpenFailed {
            message: error.message().to_string(),
        })
    })?;
    if repository.is_bare() {
        return Err(refuse(GitLockRefusal::BareRepository));
    }
    repository
        .path()
        .canonicalize()
        .map_err(SafeRmError::IoError)
}

/// stat の `st_mode` のファイルの種類（Linux と macOS で共通の値）
const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;
const S_IFIFO: u32 = 0o010_000;
const S_IFSOCK: u32 = 0o140_000;
const S_IFCHR: u32 = 0o020_000;
const S_IFBLK: u32 = 0o060_000;

/// ファイルの種類
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Regular,
    Directory,
    Symlink,
    Fifo,
    Socket,
    CharDevice,
    BlockDevice,
    Other,
}

impl FileKind {
    fn from_mode(mode: u32) -> Self {
        match mode & S_IFMT {
            S_IFREG => Self::Regular,
            S_IFDIR => Self::Directory,
            S_IFLNK => Self::Symlink,
            S_IFIFO => Self::Fifo,
            S_IFSOCK => Self::Socket,
            S_IFCHR => Self::CharDevice,
            S_IFBLK => Self::BlockDevice,
            _ => Self::Other,
        }
    }

    /// 拒否のメッセージに出す名前
    fn describe(self) -> &'static str {
        match self {
            Self::Regular => "通常ファイル",
            Self::Directory => "ディレクトリ",
            Self::Symlink => "シンボリックリンク",
            Self::Fifo => "FIFO",
            Self::Socket => "ソケット",
            Self::CharDevice => "キャラクタデバイス",
            Self::BlockDevice => "ブロックデバイス",
            Self::Other => "不明な種類のファイル",
        }
    }
}

/// 検査したときのロックの属性。削除の直前に同じかどうかを確かめ直す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LockSnapshot {
    dev: u64,
    ino: u64,
    mode: u32,
    nlink: u64,
    uid: u32,
    size: u64,
    /// 最終更新時刻（秒, ナノ秒）
    mtime: (i64, i64),
    /// 状態の変更時刻（秒, ナノ秒）
    ctime: (i64, i64),
}

impl LockSnapshot {
    fn kind(&self) -> FileKind {
        FileKind::from_mode(self.mode)
    }

    /// 最終更新時刻。表せない値なら None
    fn modified(&self) -> Option<SystemTime> {
        let (seconds, nanos) = self.mtime;
        let nanos = u32::try_from(nanos)
            .ok()
            .filter(|nanos| *nanos < 1_000_000_000)?;
        if seconds >= 0 {
            SystemTime::UNIX_EPOCH.checked_add(Duration::new(seconds.unsigned_abs(), nanos))
        } else {
            SystemTime::UNIX_EPOCH
                .checked_sub(Duration::from_secs(seconds.unsigned_abs()))?
                .checked_add(Duration::from_nanos(u64::from(nanos)))
        }
    }
}

/// 削除の対象にしてよい空のロックかを、検査した属性から判定する。
///
/// 通常ファイル・サイズ 0・リンク数 1・所有者が実効ユーザー・最終更新から `min_age` 以上、を
/// すべて満たすときだけ通す。サイズ 0 に限るのは、中身のあるロックには書きかけの index が入って
/// いて、git が後で確定させる途中のこともあるため。
fn check_attributes(
    snapshot: &LockSnapshot,
    euid: u32,
    now: SystemTime,
    min_age: Duration,
) -> Result<(), GitLockRefusal> {
    let kind = snapshot.kind();
    if kind != FileKind::Regular {
        return Err(GitLockRefusal::NotRegularFile {
            kind: kind.describe(),
        });
    }
    if snapshot.size != 0 {
        return Err(GitLockRefusal::NotEmpty {
            size: snapshot.size,
        });
    }
    if snapshot.nlink != 1 {
        return Err(GitLockRefusal::UnexpectedLinkCount {
            nlink: snapshot.nlink,
        });
    }
    if snapshot.uid != euid {
        return Err(GitLockRefusal::NotOwned {
            owner: snapshot.uid,
            euid,
        });
    }
    let Some(modified) = snapshot.modified() else {
        return Err(GitLockRefusal::InspectionIncomplete {
            reason: "最終更新時刻を解釈できません".to_string(),
        });
    };
    match now.duration_since(modified) {
        Ok(age) if age < min_age => Err(GitLockRefusal::TooRecent {
            age_secs: age.as_secs(),
            min_age_secs: min_age.as_secs(),
        }),
        Ok(_) => Ok(()),
        Err(error) => {
            let ahead = error.duration();
            Err(GitLockRefusal::ModifiedInFuture {
                ahead_secs: ahead.as_secs() + u64::from(ahead.subsec_nanos() > 0),
            })
        }
    }
}

/// 削除の直前に、保持している fd の側と名前の側の両方が、検査したときと同じかを確かめる。
fn verify_unchanged(
    inspected: &LockSnapshot,
    by_fd: &LockSnapshot,
    by_name: &LockSnapshot,
) -> Result<(), GitLockRefusal> {
    if by_fd == inspected && by_name == inspected {
        Ok(())
    } else {
        Err(GitLockRefusal::ChangedDuringInspection)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod platform {
    use super::open_handles::{self, FileIdentity, OpenHandleEvidence};
    use super::{
        FileKind, GitLockOutcome, INDEX_LOCK_FILE_NAME, LockSnapshot, check_attributes,
        resolve_gitdir, verify_unchanged,
    };
    use crate::config::GitLocksConfig;
    use crate::error::{GitLockRefusal, SafeRmError};
    use std::ffi::{CStr, CString};
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::mem::MaybeUninit;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    /// `repo` から解決した gitdir を固定し、その直下の `index.lock` を検査してから削除する。
    ///
    /// 名前の解決は gitdir のディレクトリの fd の中に閉じ込め、symlink は辿らない。ロックの fd を
    /// 削除まで保持し、検査から削除までの間に inode が再利用されないようにする。gitdir やロックを
    /// 差し替えと復元を繰り返して入れ替える操作、最後の再確認と `unlinkat` の間の差し替えは
    /// 防げないので、静止状態（gitdir を変更しないことを含む）は呼び出し側の保証に頼る。
    pub(super) fn remove_index_lock_in(
        repo: &Path,
        gitdir: &Path,
        policy: &GitLocksConfig,
        dry_run: bool,
    ) -> Result<GitLockOutcome, SafeRmError> {
        let lock_path = gitdir.join(INDEX_LOCK_FILE_NAME);
        let refuse = |reason| SafeRmError::GitLockRefused {
            target: lock_path.clone(),
            reason,
        };
        let name = CString::new(INDEX_LOCK_FILE_NAME).expect("固定のファイル名は NUL を含まない");
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(gitdir)
            .map_err(SafeRmError::IoError)?;

        // fd を開いた後に、最初と同じ `--repo` から解決し直して同じ gitdir になること、そのパスが
        // まだ fd と同じディレクトリを指していることを確かめる。解決と fd の取得の間に gitdir が
        // 別のディレクトリへ差し替わったら、ここで拒否する。gitdir から開き直さないのは、
        // core.worktree の無い外出しの gitdir で、libgit2 が親ディレクトリを workdir と推定して
        // 所有者の検証に使い、最初の解決と結果が変わるため。
        if resolve_gitdir(repo)? != gitdir || !same_directory(gitdir, &dir)? {
            return Err(refuse(GitLockRefusal::ChangedDuringInspection));
        }

        // 名前の側を先に調べ、通常ファイルでなければ開かずに拒否する（FIFO やデバイスを開かない）。
        let by_name = match stat_at(&dir, &name) {
            Ok(snapshot) => snapshot,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(SafeRmError::NotFound(lock_path));
            }
            Err(error) => return Err(SafeRmError::IoError(error)),
        };
        let kind = by_name.kind();
        if kind != FileKind::Regular {
            return Err(refuse(GitLockRefusal::NotRegularFile {
                kind: kind.describe(),
            }));
        }

        let lock = match open_at(&dir, &name) {
            Ok(lock) => lock,
            // 名前を調べてから開くまでの間に、消えたか symlink に差し替わった
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(libc::ELOOP) =>
            {
                return Err(refuse(GitLockRefusal::ChangedDuringInspection));
            }
            Err(error) => return Err(SafeRmError::IoError(error)),
        };
        let inspected = stat_fd(&lock).map_err(SafeRmError::IoError)?;
        if inspected != by_name {
            return Err(refuse(GitLockRefusal::ChangedDuringInspection));
        }
        let euid = effective_uid();
        check_attributes(
            &inspected,
            euid,
            SystemTime::now(),
            Duration::from_secs(policy.min_age_secs),
        )
        .map_err(refuse)?;

        // 走査側（/proc の stat と libproc）と同じ取り方で識別子を求める。
        let metadata = lock.metadata().map_err(SafeRmError::IoError)?;
        let identity = FileIdentity {
            dev: metadata.dev(),
            ino: metadata.ino(),
        };
        let coverage =
            match open_handles::inspect(identity, lock.as_raw_fd(), euid, effective_gid()) {
                OpenHandleEvidence::NoneObserved { coverage } => coverage,
                OpenHandleEvidence::Found { pids } => {
                    return Err(refuse(GitLockRefusal::OpenByProcess { pids }));
                }
                OpenHandleEvidence::Unknown { reason } => {
                    return Err(refuse(GitLockRefusal::InspectionIncomplete { reason }));
                }
            };

        if dry_run {
            return Ok(GitLockOutcome {
                lock_path,
                removed: false,
                coverage,
            });
        }

        let by_fd_now = stat_fd(&lock).map_err(SafeRmError::IoError)?;
        let by_name_now = match stat_at(&dir, &name) {
            Ok(snapshot) => snapshot,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(refuse(GitLockRefusal::ChangedDuringInspection));
            }
            Err(error) => return Err(SafeRmError::IoError(error)),
        };
        verify_unchanged(&inspected, &by_fd_now, &by_name_now).map_err(refuse)?;

        unlink_at(&dir, &name).map_err(SafeRmError::IoError)?;

        // 削除した名前が検査したファイルなら、保持している inode のリンク数は 0 になる。
        if stat_fd(&lock).map_err(SafeRmError::IoError)?.nlink != 0 {
            return Err(SafeRmError::GitLockReplaced { path: lock_path });
        }
        Ok(GitLockOutcome {
            lock_path,
            removed: true,
            coverage,
        })
    }

    /// パスと fd が同じディレクトリ（デバイス番号と inode）を指しているか
    fn same_directory(path: &Path, dir: &File) -> Result<bool, SafeRmError> {
        let by_path = std::fs::metadata(path).map_err(SafeRmError::IoError)?;
        let by_fd = dir.metadata().map_err(SafeRmError::IoError)?;
        Ok(by_path.dev() == by_fd.dev() && by_path.ino() == by_fd.ino())
    }

    fn effective_uid() -> u32 {
        // SAFETY: geteuid は前提条件なしに呼べる。
        unsafe { libc::geteuid() }
    }

    fn effective_gid() -> u32 {
        // SAFETY: getegid は前提条件なしに呼べる。
        unsafe { libc::getegid() }
    }

    /// Linux と macOS で型の違う stat のフィールドを、同じ表現にそろえる。
    #[allow(clippy::unnecessary_cast)]
    fn snapshot_from_stat(stat: &libc::stat) -> LockSnapshot {
        LockSnapshot {
            dev: stat.st_dev as u64,
            ino: stat.st_ino as u64,
            mode: stat.st_mode as u32,
            nlink: stat.st_nlink as u64,
            uid: stat.st_uid as u32,
            size: stat.st_size as u64,
            mtime: (stat.st_mtime as i64, stat.st_mtime_nsec as i64),
            ctime: (stat.st_ctime as i64, stat.st_ctime_nsec as i64),
        }
    }

    /// ディレクトリの fd 相対で、symlink を辿らずに stat する。
    fn stat_at(dir: &File, name: &CStr) -> io::Result<LockSnapshot> {
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: dir は開いたディレクトリの fd、name は NUL 終端、stat は書き込める領域。
        let result = unsafe {
            libc::fstatat(
                dir.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fstatat が成功したので stat は初期化済み。
        Ok(snapshot_from_stat(unsafe { stat.assume_init_ref() }))
    }

    fn stat_fd(file: &File) -> io::Result<LockSnapshot> {
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: file は開いた fd、stat は書き込める領域。
        let result = unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fstat が成功したので stat は初期化済み。
        Ok(snapshot_from_stat(unsafe { stat.assume_init_ref() }))
    }

    /// ディレクトリの fd 相対で、symlink を辿らず、FIFO でも待たずに読み取り用に開く。
    fn open_at(dir: &File, name: &CStr) -> io::Result<File> {
        // SAFETY: dir は開いたディレクトリの fd、name は NUL 終端。
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat が返した fd はこのプロセスが所有し、ほかで閉じない。
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn unlink_at(dir: &File, name: &CStr) -> io::Result<()> {
        // SAFETY: dir は開いたディレクトリの fd、name は NUL 終端。
        let result = unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::GitLockOutcome;
    use crate::config::GitLocksConfig;
    use crate::error::{GitLockRefusal, SafeRmError};
    use std::path::Path;

    pub(super) fn remove_index_lock_in(
        _repo: &Path,
        gitdir: &Path,
        _policy: &GitLocksConfig,
        _dry_run: bool,
    ) -> Result<GitLockOutcome, SafeRmError> {
        Err(SafeRmError::GitLockRefused {
            target: gitdir.join(super::INDEX_LOCK_FILE_NAME),
            reason: GitLockRefusal::UnsupportedPlatform {
                os: std::env::consts::OS,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EUID: u32 = 501;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000)
    }

    /// 削除してよい空のロック（最終更新から 1 時間）
    fn stale_lock() -> LockSnapshot {
        LockSnapshot {
            dev: 1,
            ino: 2,
            mode: S_IFREG | 0o644,
            nlink: 1,
            uid: EUID,
            size: 0,
            mtime: (2_000_000_000 - 3600, 0),
            ctime: (2_000_000_000 - 3600, 0),
        }
    }

    fn check(snapshot: &LockSnapshot) -> Result<(), GitLockRefusal> {
        check_attributes(snapshot, EUID, now(), Duration::from_secs(300))
    }

    #[test]
    fn test_check_attributes_accepts_stale_empty_lock() {
        assert_eq!(check(&stale_lock()), Ok(()));
    }

    #[test]
    fn test_check_attributes_rejects_non_regular_files() {
        for (mode, kind) in [
            (S_IFDIR, "ディレクトリ"),
            (S_IFLNK, "シンボリックリンク"),
            (S_IFIFO, "FIFO"),
            (S_IFSOCK, "ソケット"),
            (S_IFCHR, "キャラクタデバイス"),
            (S_IFBLK, "ブロックデバイス"),
        ] {
            let snapshot = LockSnapshot {
                mode: mode | 0o644,
                ..stale_lock()
            };
            assert_eq!(
                check(&snapshot),
                Err(GitLockRefusal::NotRegularFile { kind }),
                "mode {mode:o}"
            );
        }
    }

    #[test]
    fn test_check_attributes_rejects_non_empty_lock() {
        let snapshot = LockSnapshot {
            size: 12,
            ..stale_lock()
        };
        assert_eq!(check(&snapshot), Err(GitLockRefusal::NotEmpty { size: 12 }));
    }

    #[test]
    fn test_check_attributes_rejects_unexpected_link_counts() {
        for nlink in [0, 2] {
            let snapshot = LockSnapshot {
                nlink,
                ..stale_lock()
            };
            assert_eq!(
                check(&snapshot),
                Err(GitLockRefusal::UnexpectedLinkCount { nlink })
            );
        }
    }

    #[test]
    fn test_check_attributes_rejects_lock_owned_by_another_user() {
        let snapshot = LockSnapshot {
            uid: 0,
            ..stale_lock()
        };
        assert_eq!(
            check(&snapshot),
            Err(GitLockRefusal::NotOwned {
                owner: 0,
                euid: EUID
            })
        );
    }

    #[test]
    fn test_check_attributes_rejects_recent_lock_with_remaining_wait() {
        let snapshot = LockSnapshot {
            mtime: (2_000_000_000 - 18, 500_000_000),
            ..stale_lock()
        };
        assert_eq!(
            check(&snapshot),
            Err(GitLockRefusal::TooRecent {
                age_secs: 17,
                min_age_secs: 300
            })
        );
    }

    #[test]
    fn test_check_attributes_boundary_of_minimum_age() {
        let just_old_enough = LockSnapshot {
            mtime: (2_000_000_000 - 300, 0),
            ..stale_lock()
        };
        assert_eq!(check(&just_old_enough), Ok(()));

        let one_nanosecond_short = LockSnapshot {
            mtime: (2_000_000_000 - 300, 1),
            ..stale_lock()
        };
        assert_eq!(
            check(&one_nanosecond_short),
            Err(GitLockRefusal::TooRecent {
                age_secs: 299,
                min_age_secs: 300
            })
        );
    }

    #[test]
    fn test_check_attributes_rejects_future_modification_time() {
        let snapshot = LockSnapshot {
            mtime: (2_000_000_000 + 60, 1),
            ..stale_lock()
        };
        assert_eq!(
            check(&snapshot),
            Err(GitLockRefusal::ModifiedInFuture { ahead_secs: 61 })
        );
    }

    #[test]
    fn test_check_attributes_rejects_unrepresentable_modification_time() {
        let snapshot = LockSnapshot {
            mtime: (0, 1_000_000_000),
            ..stale_lock()
        };
        assert!(matches!(
            check(&snapshot),
            Err(GitLockRefusal::InspectionIncomplete { .. })
        ));
    }

    #[test]
    fn test_modified_handles_times_before_epoch() {
        let snapshot = LockSnapshot {
            mtime: (-10, 250_000_000),
            ..stale_lock()
        };
        assert_eq!(
            snapshot.modified(),
            Some(SystemTime::UNIX_EPOCH - Duration::from_millis(9_750))
        );
    }

    #[test]
    fn test_verify_unchanged_accepts_identical_snapshots() {
        let lock = stale_lock();
        assert_eq!(verify_unchanged(&lock, &lock, &lock), Ok(()));
    }

    #[test]
    fn test_verify_unchanged_rejects_any_difference() {
        let inspected = stale_lock();
        let changes = [
            LockSnapshot {
                ino: 3,
                ..inspected
            },
            LockSnapshot {
                dev: 9,
                ..inspected
            },
            LockSnapshot {
                size: 1,
                ..inspected
            },
            LockSnapshot {
                nlink: 2,
                ..inspected
            },
            LockSnapshot {
                mtime: (inspected.mtime.0 + 1, 0),
                ..inspected
            },
            LockSnapshot {
                ctime: (inspected.ctime.0, 1),
                ..inspected
            },
        ];
        for changed in changes {
            assert_eq!(
                verify_unchanged(&inspected, &changed, &inspected),
                Err(GitLockRefusal::ChangedDuringInspection),
                "fd 側: {changed:?}"
            );
            assert_eq!(
                verify_unchanged(&inspected, &inspected, &changed),
                Err(GitLockRefusal::ChangedDuringInspection),
                "名前側: {changed:?}"
            );
        }
    }

    #[test]
    fn test_file_kind_from_mode() {
        assert_eq!(FileKind::from_mode(S_IFREG | 0o600), FileKind::Regular);
        assert_eq!(FileKind::from_mode(S_IFDIR | 0o755), FileKind::Directory);
        assert_eq!(FileKind::from_mode(0o777), FileKind::Other);
    }

    fn enabled_policy() -> GitLocksConfig {
        GitLocksConfig::default()
    }

    fn request(repo: &Path) -> GitLockRequest<'_> {
        GitLockRequest {
            repo,
            assume_quiescent: true,
            dry_run: false,
        }
    }

    #[test]
    fn test_remove_index_lock_refuses_when_disabled() {
        let policy = GitLocksConfig {
            enabled: false,
            ..GitLocksConfig::default()
        };
        let error = remove_index_lock(&request(Path::new("/nonexistent")), &policy).unwrap_err();
        assert!(matches!(
            error,
            SafeRmError::GitLockRefused {
                reason: GitLockRefusal::Disabled,
                ..
            }
        ));
    }

    #[test]
    fn test_remove_index_lock_refuses_without_quiescence_assertion() {
        let request = GitLockRequest {
            assume_quiescent: false,
            ..request(Path::new("/nonexistent"))
        };
        let error = remove_index_lock(&request, &enabled_policy()).unwrap_err();
        assert!(matches!(
            error,
            SafeRmError::GitLockRefused {
                reason: GitLockRefusal::QuiescenceNotAsserted,
                ..
            }
        ));
    }

    #[test]
    fn test_resolve_gitdir_rejects_empty_repo_path() {
        let error = resolve_gitdir(Path::new("")).unwrap_err();
        assert!(matches!(
            error,
            SafeRmError::GitLockRefused {
                reason: GitLockRefusal::RepositoryOpenFailed { .. },
                ..
            }
        ));
    }

    #[test]
    fn test_resolve_gitdir_opens_worktree_root_and_gitdir() {
        let temp = tempfile::TempDir::new().unwrap();
        git2::Repository::init(temp.path()).unwrap();
        let expected = temp.path().join(".git").canonicalize().unwrap();

        assert_eq!(resolve_gitdir(temp.path()).unwrap(), expected);
        assert_eq!(resolve_gitdir(&temp.path().join(".git")).unwrap(), expected);
    }

    #[test]
    fn test_resolve_gitdir_does_not_search_parent_directories() {
        let temp = tempfile::TempDir::new().unwrap();
        git2::Repository::init(temp.path()).unwrap();
        let subdir = temp.path().join("src");
        std::fs::create_dir(&subdir).unwrap();

        let error = resolve_gitdir(&subdir).unwrap_err();
        assert!(matches!(
            error,
            SafeRmError::GitLockRefused {
                reason: GitLockRefusal::RepositoryOpenFailed { .. },
                ..
            }
        ));
    }

    #[test]
    fn test_resolve_gitdir_rejects_bare_repository() {
        let temp = tempfile::TempDir::new().unwrap();
        git2::Repository::init_bare(temp.path()).unwrap();

        let error = resolve_gitdir(temp.path()).unwrap_err();
        assert!(matches!(
            error,
            SafeRmError::GitLockRefused {
                reason: GitLockRefusal::BareRepository,
                ..
            }
        ));
    }

    /// 検査で拒否される経路だけをここで確かめる。
    ///
    /// 削除まで成功する経路は tests/git_lock_test.rs（別プロセスの safe-rm）で確かめる。
    /// テストのプロセスでは別のテストのスレッドが子プロセスを起動し、exec までの間だけ、子が
    /// 検査中のロックの fd の写しを持つことがあるため（検査はそれを正しく拒否の理由にする）。
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    mod removal {
        use super::*;
        use std::fs::{self, File};

        /// worktree と、最終更新を 1 時間前にした空の index.lock を作る。
        fn repo_with_stale_lock() -> (tempfile::TempDir, PathBuf) {
            let temp = tempfile::TempDir::new().unwrap();
            git2::Repository::init(temp.path()).unwrap();
            let lock = temp.path().join(".git").join(INDEX_LOCK_FILE_NAME);
            File::create(&lock)
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(3600))
                .unwrap();
            (temp, lock)
        }

        #[test]
        fn test_refuses_when_repo_no_longer_resolves_to_the_pinned_gitdir() {
            // fd を開いた後に --repo から解決し直した gitdir が、固定した gitdir と違えば拒否する
            let (temp_b, lock_b) = repo_with_stale_lock();
            let gitdir_b = temp_b.path().join(".git").canonicalize().unwrap();
            let temp_a = tempfile::TempDir::new().unwrap();
            git2::Repository::init(temp_a.path()).unwrap();

            let error =
                platform::remove_index_lock_in(temp_a.path(), &gitdir_b, &enabled_policy(), false)
                    .unwrap_err();

            assert!(
                matches!(
                    error,
                    SafeRmError::GitLockRefused {
                        reason: GitLockRefusal::ChangedDuringInspection,
                        ..
                    }
                ),
                "{error:?}"
            );
            assert!(lock_b.exists());
        }

        #[test]
        fn test_missing_lock_is_not_found() {
            let temp = tempfile::TempDir::new().unwrap();
            git2::Repository::init(temp.path()).unwrap();

            let error = remove_index_lock(&request(temp.path()), &enabled_policy()).unwrap_err();

            assert!(matches!(error, SafeRmError::NotFound(_)), "{error:?}");
            assert_eq!(error.exit_code(), 1);
        }

        #[test]
        fn test_refuses_recent_lock() {
            let temp = tempfile::TempDir::new().unwrap();
            git2::Repository::init(temp.path()).unwrap();
            let lock = temp.path().join(".git").join(INDEX_LOCK_FILE_NAME);
            File::create(&lock).unwrap();

            let error = remove_index_lock(&request(temp.path()), &enabled_policy()).unwrap_err();

            assert!(
                matches!(
                    error,
                    SafeRmError::GitLockRefused {
                        reason: GitLockRefusal::TooRecent {
                            min_age_secs: 300,
                            ..
                        },
                        ..
                    }
                ),
                "{error:?}"
            );
            assert!(lock.exists());
        }

        #[test]
        fn test_refuses_lock_newer_than_configured_minimum_age() {
            let temp = tempfile::TempDir::new().unwrap();
            git2::Repository::init(temp.path()).unwrap();
            let lock = temp.path().join(".git").join(INDEX_LOCK_FILE_NAME);
            File::create(&lock)
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(3600))
                .unwrap();
            let policy = GitLocksConfig {
                min_age_secs: 7200,
                ..GitLocksConfig::default()
            };

            let error = remove_index_lock(&request(temp.path()), &policy).unwrap_err();

            assert!(
                matches!(
                    error,
                    SafeRmError::GitLockRefused {
                        reason: GitLockRefusal::TooRecent {
                            min_age_secs: 7200,
                            ..
                        },
                        ..
                    }
                ),
                "{error:?}"
            );
            assert!(lock.exists());
        }

        #[test]
        fn test_refuses_symlink_lock_without_following_it() {
            let temp = tempfile::TempDir::new().unwrap();
            git2::Repository::init(temp.path()).unwrap();
            let target = temp.path().join("victim.txt");
            fs::write(&target, "").unwrap();
            let lock = temp.path().join(".git").join(INDEX_LOCK_FILE_NAME);
            std::os::unix::fs::symlink(&target, &lock).unwrap();

            let error = remove_index_lock(&request(temp.path()), &enabled_policy()).unwrap_err();

            assert!(matches!(
                error,
                SafeRmError::GitLockRefused {
                    reason: GitLockRefusal::NotRegularFile {
                        kind: "シンボリックリンク"
                    },
                    ..
                }
            ));
            assert!(fs::symlink_metadata(&lock).is_ok());
            assert!(target.exists());
        }

        #[test]
        fn test_refuses_lock_held_open_by_this_process() {
            let (temp, lock) = repo_with_stale_lock();
            let _held = File::open(&lock).unwrap();

            let error = remove_index_lock(&request(temp.path()), &enabled_policy()).unwrap_err();

            match error {
                SafeRmError::GitLockRefused {
                    reason: GitLockRefusal::OpenByProcess { pids },
                    ..
                } => assert!(pids.contains(&std::process::id()), "{pids:?}"),
                other => panic!("unexpected: {other:?}"),
            }
            assert!(lock.exists());
        }
    }
}
