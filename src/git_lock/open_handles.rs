//! ロックのファイルを開いているプロセスの検査
//!
//! 結果は 3 値（見つかった / 見つからなかった / 調べきれない）で返し、調べきれないときは
//! 呼び出し側が削除を拒否する。見つからなかったことは、ロックが使われていないことの証明では
//! ない。git はファイルを閉じたままロックを持ち続けることがあり、他ユーザーのプロセス、
//! 別の PID 名前空間（コンテナ）、別のホスト（ネットワークファイルシステム）も見えない。
//!
//! - Linux: `/proc/<pid>/fd` の各 fd を stat して、デバイス番号と inode で照合する
//! - macOS: libproc で各プロセスの vnode の fd を引き、デバイス番号と inode で照合する
//!
//! どちらも、実効ユーザーと同じ所有者のプロセスだけを調べる（実効ユーザーが root なら全部）。
//! 自分のプロセスは、検査のために開いたロックの fd 番号だけを除外する。

use std::os::fd::RawFd;
use std::time::{Duration, Instant};

/// 走査にかけてよい時間の上限。超えたら調べきれないとして拒否する
const SCAN_TIME_LIMIT: Duration = Duration::from_secs(10);

/// 照合するファイルの識別子（デバイス番号と inode）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileIdentity {
    pub dev: u64,
    pub ino: u64,
}

/// 走査の条件
#[derive(Debug, Clone, Copy)]
struct ScanTarget {
    /// 照合するファイル
    file: FileIdentity,
    /// 検査のために開いた fd を持つプロセス（このプロセス）の PID
    own_pid: u32,
    /// 検査のために開いた fd の番号。照合から外すのはこの fd だけ
    own_fd: RawFd,
    /// 実効ユーザー
    euid: u32,
    /// 実効グループ（Linux で、fd を辿れなかったプロセスの資格情報と比べて理由に添える）
    #[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
    egid: u32,
}

/// 検査の対象外にしたもの（他ユーザーのプロセスは常に対象外なので数えない）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanCoverage {
    /// fd の中身の取得を OS が拒んだプラットフォームバイナリ（macOS）の実行ファイル名
    pub excluded_platform_processes: Vec<String>,
    /// 中身を取得できなかった vnode の fd の数（macOS）
    pub unavailable_vnode_fds: usize,
}

impl ScanCoverage {
    /// 対象外にしたものがあれば、利用者に伝える 1 行の文を返す。
    pub fn note(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.excluded_platform_processes.is_empty() {
            parts.push(format!(
                "{} OS platform process(es) that deny inspection ({})",
                self.excluded_platform_processes.len(),
                self.excluded_platform_processes.join(", ")
            ));
        }
        if self.unavailable_vnode_fds > 0 {
            parts.push(format!(
                "{} file descriptor(s) whose vnode could not be read",
                self.unavailable_vnode_fds
            ));
        }
        if parts.is_empty() {
            return None;
        }
        Some(format!(
            "open-handle inspection did not cover {}; --assume-quiescent remains the guarantee",
            parts.join(" and ")
        ))
    }
}

/// 開いているプロセスの検査の結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OpenHandleEvidence {
    /// ロックを開いているプロセスが見つかった
    Found { pids: Vec<u32> },
    /// 調べた範囲では見つからなかった
    NoneObserved { coverage: ScanCoverage },
    /// 調べきれなかった
    Unknown { reason: String },
}

/// 走査の途中経過
#[derive(Debug, Default)]
struct Observation {
    holders: Vec<u32>,
    coverage: ScanCoverage,
    incomplete: Option<String>,
}

impl Observation {
    /// 調べきれなかったことを残す（最初の理由を伝える）。
    fn record_incomplete(&mut self, reason: String) {
        self.incomplete.get_or_insert(reason);
    }

    fn into_evidence(self) -> OpenHandleEvidence {
        if !self.holders.is_empty() {
            OpenHandleEvidence::Found { pids: self.holders }
        } else if let Some(reason) = self.incomplete {
            OpenHandleEvidence::Unknown { reason }
        } else {
            OpenHandleEvidence::NoneObserved {
                coverage: self.coverage,
            }
        }
    }
}

/// 走査の締め切り
struct Deadline(Instant);

impl Deadline {
    fn after(limit: Duration) -> Self {
        Self(Instant::now() + limit)
    }

    fn check(&self) -> Result<(), String> {
        if Instant::now() > self.0 {
            Err(format!(
                "{} 秒以内に検査を終えられませんでした",
                SCAN_TIME_LIMIT.as_secs()
            ))
        } else {
            Ok(())
        }
    }
}

/// `target` を開いているプロセスを探す。`own_fd` は、このプロセスが検査のために開いた fd。
pub(super) fn inspect(
    target: FileIdentity,
    own_fd: RawFd,
    euid: u32,
    egid: u32,
) -> OpenHandleEvidence {
    let scan = ScanTarget {
        file: target,
        own_pid: std::process::id(),
        own_fd,
        euid,
        egid,
    };
    run_scan(
        &Deadline::after(SCAN_TIME_LIMIT),
        |deadline, observation| platform::scan(&scan, deadline, observation),
    )
}

/// 走査を締め切り付きで動かし、結果を 3 値にまとめる。
///
/// 締め切りを過ぎてから終わった走査の結果は、削除の許可に使わない。
fn run_scan(
    deadline: &Deadline,
    scan: impl FnOnce(&Deadline, &mut Observation) -> Result<(), String>,
) -> OpenHandleEvidence {
    let mut observation = Observation::default();
    if let Err(reason) = scan(deadline, &mut observation).and_then(|()| deadline.check()) {
        observation.incomplete = Some(reason);
    }
    observation.into_evidence()
}

/// 実効ユーザーが調べるべきプロセスの所有者か（root はすべて調べる）
fn should_inspect_owner(owner: u32, euid: u32) -> bool {
    owner == euid || euid == 0
}

/// git の本体か、git が呼ぶ補助プログラム（`git-*`）の実行ファイル名か
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn is_git_program_name(name: &str) -> bool {
    name == "git" || name.starts_with("git-")
}

/// `/proc` の走査（Linux）。テストでは、作ったディレクトリの木を `/proc` の代わりに渡す。
#[cfg(any(target_os = "linux", all(test, unix)))]
mod procfs {
    use super::{Deadline, Observation, ScanTarget, should_inspect_owner};
    use std::io;
    use std::os::fd::RawFd;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    /// プロセスの fd を調べられなかった理由
    enum FdScanError {
        /// 権限が無い（EACCES / EPERM）。`/proc/<pid>/fd/<n>` を辿るには ptrace の読み取りの
        /// 検査を通る必要があり、所有者が同じでも資格情報（gid など）の違いや LSM で拒まれる
        Denied(String),
        /// それ以外
        Failed(String),
    }

    pub(super) fn scan(
        proc_root: &Path,
        target: &ScanTarget,
        deadline: &Deadline,
        observation: &mut Observation,
    ) -> Result<(), String> {
        let entries = std::fs::read_dir(proc_root)
            .map_err(|error| format!("{} を読めません: {error}", proc_root.display()))?;
        for entry in entries {
            deadline.check()?;
            let entry =
                entry.map_err(|error| format!("{} を読めません: {error}", proc_root.display()))?;
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            let process_dir = entry.path();
            // non-dumpable なプロセス（ssh-agent など）は /proc/<pid> が root の所有に見えるので、
            // 他ユーザーのプロセスと同じく対象外になる。
            let owner = match std::fs::metadata(&process_dir) {
                Ok(metadata) => metadata.uid(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("{}: {error}", process_dir.display())),
            };
            if !should_inspect_owner(owner, target.euid) {
                continue;
            }
            let skip_fd = (pid == target.own_pid).then_some(target.own_fd);
            let reason = match process_holds(&process_dir, target, skip_fd, deadline) {
                Ok(true) => {
                    observation.holders.push(pid);
                    continue;
                }
                Ok(false) => continue,
                Err(FdScanError::Denied(detail)) => {
                    denied_reason(&process_dir, pid, target, &detail)
                }
                Err(FdScanError::Failed(reason)) => reason,
            };
            // 締め切りなら走査をやめる。そうでなければ調べきれないことを残し、ロックを開いている
            // プロセスを探すために残りの走査は続ける（見つかれば、そちらを報告する）。
            deadline.check()?;
            observation.record_incomplete(reason);
        }
        Ok(())
    }

    /// プロセスの fd のどれかが対象を指しているか。終了したプロセスは false。
    fn process_holds(
        process_dir: &Path,
        target: &ScanTarget,
        skip_fd: Option<RawFd>,
        deadline: &Deadline,
    ) -> Result<bool, FdScanError> {
        let fd_dir = process_dir.join("fd");
        let fds = match std::fs::read_dir(&fd_dir) {
            Ok(fds) => fds,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(scan_error(&fd_dir, &error)),
        };
        for fd in fds {
            deadline.check().map_err(FdScanError::Failed)?;
            let fd = match fd {
                Ok(fd) => fd,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(scan_error(&fd_dir, &error)),
            };
            if skip_fd.is_some()
                && fd
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<RawFd>().ok())
                    == skip_fd
            {
                continue;
            }
            // パス文字列ではなく、fd が指す実体のデバイス番号と inode で照合する。
            match std::fs::metadata(fd.path()) {
                Ok(metadata)
                    if metadata.dev() == target.file.dev && metadata.ino() == target.file.ino =>
                {
                    return Ok(true);
                }
                Ok(_) => {}
                // 調べている間に fd が閉じた
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(scan_error(&fd.path(), &error)),
            }
        }
        Ok(false)
    }

    /// 権限が無いことによる失敗（EACCES / EPERM）を、それ以外の失敗と分ける。
    fn scan_error(path: &Path, error: &io::Error) -> FdScanError {
        let detail = format!("{}: {error}", path.display());
        if error.kind() == io::ErrorKind::PermissionDenied {
            FdScanError::Denied(detail)
        } else {
            FdScanError::Failed(detail)
        }
    }

    /// 権限が無くて fd を調べられなかったプロセスについて、原因を追えるように、
    /// `/proc/<pid>/status`（他のプロセスのものも読める）の名前と資格情報を添えた理由を作る。
    ///
    /// 資格情報は診断にだけ使い、検査の対象外にする根拠にはしない。資格情報（gid など）の違う
    /// プロセスも、同じユーザーのリポジトリを操作でき、資格情報を変える前からロックを開いている
    /// こともあるため。
    fn denied_reason(process_dir: &Path, pid: u32, target: &ScanTarget, detail: &str) -> String {
        let credentials = std::fs::read_to_string(process_dir.join("status"))
            .ok()
            .as_deref()
            .and_then(ProcessCredentials::parse);
        match credentials {
            Some(credentials) => format!(
                "{detail}（PID {pid}、{}、uid {:?}、gid {:?}、ptrace の ID の照合は{}）",
                credentials.name,
                credentials.uids,
                credentials.gids,
                if credentials.ptrace_ids_match(target.euid, target.egid) {
                    "一致する"
                } else {
                    "一致しない"
                }
            ),
            None => format!("{detail}（PID {pid}）"),
        }
    }

    /// `/proc/<pid>/status` の名前と資格情報（実・実効・保存・ファイルシステムの uid と gid）
    #[derive(Debug, PartialEq, Eq)]
    struct ProcessCredentials {
        name: String,
        uids: [u32; 4],
        gids: [u32; 4],
    }

    impl ProcessCredentials {
        fn parse(status: &str) -> Option<Self> {
            let mut name = None;
            let mut uids = None;
            let mut gids = None;
            for line in status.lines() {
                if let Some(value) = line.strip_prefix("Name:") {
                    name = Some(value.trim().to_string());
                } else if let Some(value) = line.strip_prefix("Uid:") {
                    uids = parse_ids(value);
                } else if let Some(value) = line.strip_prefix("Gid:") {
                    gids = parse_ids(value);
                }
            }
            Some(Self {
                name: name?,
                uids: uids?,
                gids: gids?,
            })
        }

        /// ptrace の読み取りの検査のうち、ID の照合（呼び出し側の fsuid / fsgid と、相手の実・実効・
        /// 保存の uid / gid）が通るか。通っても、capability や LSM で拒まれることはある。
        fn ptrace_ids_match(&self, fsuid: u32, fsgid: u32) -> bool {
            self.uids[..3].iter().all(|&uid| uid == fsuid)
                && self.gids[..3].iter().all(|&gid| gid == fsgid)
        }
    }

    fn parse_ids(value: &str) -> Option<[u32; 4]> {
        let ids: Vec<u32> = value
            .split_whitespace()
            .map(|id| id.parse().ok())
            .collect::<Option<_>>()?;
        ids.try_into().ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_parse_process_credentials() {
            let status = "Name:\tRunner.Listener\nUmask:\t0022\nState:\tS (sleeping)\n\
                          Uid:\t1001\t1001\t1001\t1001\nGid:\t118\t118\t118\t1001\nGroups:\t4 118\n";
            assert_eq!(
                ProcessCredentials::parse(status),
                Some(ProcessCredentials {
                    name: "Runner.Listener".to_string(),
                    uids: [1001; 4],
                    gids: [118, 118, 118, 1001],
                })
            );
            // 欠けている・数が違う・数でないときは解釈できない
            assert_eq!(ProcessCredentials::parse("Name:\tx\nUid:\t1 1 1 1\n"), None);
            assert_eq!(
                ProcessCredentials::parse("Name:\tx\nUid:\t1 1 1\nGid:\t1 1 1 1\n"),
                None
            );
            assert_eq!(
                ProcessCredentials::parse("Name:\tx\nUid:\t1 1 1 a\nGid:\t1 1 1 1\n"),
                None
            );
        }

        #[test]
        fn test_ptrace_ids_match() {
            let credentials = |uids: [u32; 4], gids: [u32; 4]| ProcessCredentials {
                name: "worker".to_string(),
                uids,
                gids,
            };
            assert!(credentials([1001; 4], [118; 4]).ptrace_ids_match(1001, 118));
            // 相手の fsuid / fsgid（4 番目）は照合しない
            assert!(
                credentials([1001, 1001, 1001, 0], [118, 118, 118, 0]).ptrace_ids_match(1001, 118)
            );
            assert!(!credentials([1001; 4], [1001; 4]).ptrace_ids_match(1001, 118));
            assert!(!credentials([1001, 1001, 0, 1001], [118; 4]).ptrace_ids_match(1001, 118));
        }

        #[test]
        fn test_denied_reason_adds_process_name_and_credentials() {
            let temp = tempfile::TempDir::new().unwrap();
            std::fs::write(
                temp.path().join("status"),
                "Name:\tworker\nUid:\t1001\t1001\t1001\t1001\nGid:\t1001\t1001\t1001\t1001\n",
            )
            .unwrap();
            let target = ScanTarget {
                file: super::super::FileIdentity { dev: 1, ino: 2 },
                own_pid: 1,
                own_fd: 3,
                euid: 1001,
                egid: 118,
            };

            let reason =
                denied_reason(temp.path(), 42, &target, "/proc/42/fd/0: Permission denied");

            assert_eq!(
                reason,
                "/proc/42/fd/0: Permission denied（PID 42、worker、uid [1001, 1001, 1001, 1001]、gid [1001, 1001, 1001, 1001]、ptrace の ID の照合は一致しない）"
            );
            // status を読めなければ、PID だけを添える
            let missing = denied_reason(&temp.path().join("gone"), 43, &target, "d");
            assert_eq!(missing, "d（PID 43）");
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{Deadline, Observation, ScanTarget};
    use std::path::Path;

    pub(super) fn scan(
        target: &ScanTarget,
        deadline: &Deadline,
        observation: &mut Observation,
    ) -> Result<(), String> {
        super::procfs::scan(Path::new("/proc"), target, deadline, observation)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{Deadline, Observation, ScanTarget, is_git_program_name, should_inspect_owner};
    use std::ffi::{c_int, c_void};
    use std::io;
    use std::mem::{MaybeUninit, size_of};
    use std::os::fd::RawFd;
    use std::path::Path;

    /// `sys/proc_info.h` の `PROC_PIDFDVNODEINFO`（libc クレートに無い）
    const PROC_PIDFDVNODEINFO: c_int = 1;
    /// `sys/codesign.h` の `CS_OPS_STATUS`
    const CS_OPS_STATUS: u32 = 0;
    /// `sys/codesign.h` の `CS_PLATFORM_BINARY`（OS に含まれるプラットフォームバイナリ）
    const CS_PLATFORM_BINARY: u32 = 0x0400_0000;
    /// PID の一覧と fd の一覧を、バッファを広げて取り直す回数の上限
    const MAX_LIST_ATTEMPTS: usize = 8;

    /// `sys/proc_info.h` の `struct vnode_fdinfo`（libc クレートに無い）。
    /// 先頭の `struct proc_fileinfo`（24 バイト）は使わないので、配置だけを保つ。
    #[repr(C)]
    struct VnodeFdInfo {
        _file_info: [u64; 3],
        vnode: libc::vnode_info,
    }

    // C の sizeof(struct vnode_fdinfo) と同じ大きさであること（取得したバイト数とも照合する）
    const _: () = assert!(size_of::<VnodeFdInfo>() == 176);

    unsafe extern "C" {
        /// libSystem の `csops`（ヘッダは公開 SDK に無い）。`CS_OPS_STATUS` は他のプロセスにも使える。
        fn csops(
            pid: libc::pid_t,
            ops: u32,
            useraddr: *mut c_void,
            usersize: libc::size_t,
        ) -> c_int;
    }

    /// libproc の呼び出しの失敗
    #[derive(Debug, PartialEq, Eq)]
    enum Probe {
        /// プロセスが終了した（ESRCH）
        Gone,
        /// 権限が無い（EPERM）
        Denied,
        /// それ以外
        Failed(String),
    }

    /// vnode の fd の中身の取得の失敗
    #[derive(Debug, PartialEq, Eq)]
    enum VnodeProbe {
        /// fd が閉じた（EBADF）
        Closed,
        /// プロセスが終了した（ESRCH）
        Gone,
        /// vnode の中身を取得できない（ENOENT）
        Unavailable,
        /// 権限が無い（EPERM）
        Denied,
        /// それ以外
        Failed(String),
    }

    /// 一覧（PID や fd）の取得の 1 回分の結果
    #[derive(Debug, PartialEq, Eq)]
    enum ListStep {
        /// 取り切れた（要素の数）
        Complete(usize),
        /// バッファが埋まった。広げて取り直す
        Grow,
    }

    pub(super) fn scan(
        target: &ScanTarget,
        deadline: &Deadline,
        observation: &mut Observation,
    ) -> Result<(), String> {
        for pid in list_all_pids()? {
            deadline.check()?;
            // kernel_task はファイルを開かない
            if pid <= 0 {
                continue;
            }
            let skip_fd = (pid as u32 == target.own_pid).then_some(target.own_fd);
            if let Err(reason) = inspect_process(pid, target, skip_fd, deadline, observation) {
                // 締め切りなら走査をやめる。そうでなければ調べきれないことを残し、ロックを開いて
                // いるプロセスを探すために残りの走査は続ける（見つかれば、そちらを報告する）。
                deadline.check()?;
                observation.record_incomplete(reason);
            }
        }
        Ok(())
    }

    fn inspect_process(
        pid: libc::pid_t,
        target: &ScanTarget,
        skip_fd: Option<RawFd>,
        deadline: &Deadline,
        observation: &mut Observation,
    ) -> Result<(), String> {
        let owner = match short_bsd_info(pid) {
            Ok(info) => info.pbsi_uid,
            Err(Probe::Gone) => return Ok(()),
            Err(Probe::Denied) => return Err(format!("PID {pid} の所有者を取得できません")),
            Err(Probe::Failed(reason)) => {
                return Err(format!("PID {pid} の所有者を取得できません: {reason}"));
            }
        };
        if !should_inspect_owner(owner, target.euid) {
            return Ok(());
        }
        // 権限の無い fd を分類する前後で、同じプロセスかを照合するための識別子
        let started = match start_time(pid) {
            Ok(started) => started,
            Err(Probe::Gone) => return Ok(()),
            Err(Probe::Denied) => return Err(format!("PID {pid} の情報を取得できません")),
            Err(Probe::Failed(reason)) => {
                return Err(format!("PID {pid} の情報を取得できません: {reason}"));
            }
        };
        let fds = match list_fds(pid) {
            Ok(fds) => fds,
            Err(Probe::Gone) => return Ok(()),
            Err(Probe::Denied) => {
                return exclude_denied_process(pid, started, observation);
            }
            Err(Probe::Failed(reason)) => {
                return Err(format!("PID {pid} の fd の一覧を取得できません: {reason}"));
            }
        };

        let mut denied = false;
        for fd in fds {
            deadline.check()?;
            if fd.proc_fdtype != libc::PROX_FDTYPE_VNODE as u32 || Some(fd.proc_fd) == skip_fd {
                continue;
            }
            match vnode_identity(pid, fd.proc_fd) {
                Ok((dev, ino)) => {
                    // vinfo_stat の vst_dev は 32 ビット（stat の st_dev も macOS では 32 ビット）
                    if dev == target.file.dev as u32 && ino == target.file.ino {
                        observation.holders.push(pid as u32);
                        return Ok(());
                    }
                }
                Err(VnodeProbe::Closed) => {}
                Err(VnodeProbe::Gone) => return Ok(()),
                Err(VnodeProbe::Unavailable) => observation.coverage.unavailable_vnode_fds += 1,
                // 残りの fd は調べ続ける（一部だけ取得できるときにも検出できるように）
                Err(VnodeProbe::Denied) => denied = true,
                Err(VnodeProbe::Failed(reason)) => {
                    return Err(format!(
                        "PID {pid} の fd {} を調べられません: {reason}",
                        fd.proc_fd
                    ));
                }
            }
        }
        if denied {
            exclude_denied_process(pid, started, observation)?;
        }
        Ok(())
    }

    /// fd の中身の取得を拒まれたプロセスを、OS から取り直した情報で分類し、対象外にする。
    /// 分類の途中でプロセスが終了したら、fd も閉じているので何もしない。
    fn exclude_denied_process(
        pid: libc::pid_t,
        started: (u64, u64),
        observation: &mut Observation,
    ) -> Result<(), String> {
        let denied = || format!("PID {pid} の fd を調べる権限がありません");
        let flags = match code_signing_flags(pid) {
            Ok(flags) => flags,
            Err(Probe::Gone) => return Ok(()),
            Err(Probe::Denied | Probe::Failed(_)) => return Err(denied()),
        };
        let name = match executable_name(pid) {
            Ok(name) => name,
            Err(Probe::Gone) => return Ok(()),
            Err(Probe::Denied | Probe::Failed(_)) => return Err(denied()),
        };
        let started_again = match start_time(pid) {
            Ok(started_again) => started_again,
            Err(Probe::Gone) => return Ok(()),
            Err(Probe::Denied | Probe::Failed(_)) => return Err(denied()),
        };
        classify_denied_process(pid, flags, &name, started, started_again)?;
        observation.coverage.excluded_platform_processes.push(name);
        Ok(())
    }

    /// fd の中身の取得を拒まれたプロセスを、検査の対象外にしてよいかを判定する。
    ///
    /// OS が検査を禁じたプラットフォームバイナリ（実測では `/usr/libexec/routined` など
    /// 同じユーザーで動く一部の OS のデーモン）だけを対象外にする。git とその補助プログラムは、
    /// プラットフォームバイナリでも対象外にしない。分類の前後で起動時刻が違えば、PID が別の
    /// プロセスに再利用されたので調べきれないとする。
    fn classify_denied_process(
        pid: libc::pid_t,
        flags: u32,
        name: &str,
        started: (u64, u64),
        started_again: (u64, u64),
    ) -> Result<(), String> {
        if flags & CS_PLATFORM_BINARY == 0 {
            return Err(format!("PID {pid} の fd を調べる権限がありません"));
        }
        if is_git_program_name(name) {
            return Err(format!("PID {pid}（{name}）の fd を調べる権限がありません"));
        }
        if started != started_again {
            return Err(format!(
                "PID {pid} が検査の途中で別のプロセスに再利用されました"
            ));
        }
        Ok(())
    }

    fn clear_errno() {
        // SAFETY: __error() はこのスレッドの errno の場所を返す。
        unsafe { *libc::__error() = 0 };
    }

    fn last_errno() -> c_int {
        io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }

    fn probe_from_errno(errno: c_int) -> Probe {
        match errno {
            libc::ESRCH => Probe::Gone,
            libc::EPERM => Probe::Denied,
            0 => Probe::Failed("結果が空です".to_string()),
            errno => Probe::Failed(io::Error::from_raw_os_error(errno).to_string()),
        }
    }

    /// 固定長の構造体を取得した結果（書き込まれたバイト数と errno）を解釈する。
    /// libproc は失敗を 0 で返して errno を設定するので、0 は大きさの不一致ではなく失敗にする。
    fn decode_fixed_size(written: c_int, errno: c_int, expected: usize) -> Result<(), Probe> {
        if written <= 0 {
            return Err(probe_from_errno(errno));
        }
        if written as usize != expected {
            return Err(Probe::Failed(format!(
                "想定外の大きさの結果です（{written} バイト）"
            )));
        }
        Ok(())
    }

    /// vnode の fd の中身を取得した結果を解釈する。
    fn decode_vnode_result(
        written: c_int,
        errno: c_int,
        expected: usize,
    ) -> Result<(), VnodeProbe> {
        if written <= 0 {
            return Err(match errno {
                libc::EBADF => VnodeProbe::Closed,
                libc::ESRCH => VnodeProbe::Gone,
                libc::ENOENT => VnodeProbe::Unavailable,
                libc::EPERM => VnodeProbe::Denied,
                0 => VnodeProbe::Failed("結果が空です".to_string()),
                errno => VnodeProbe::Failed(io::Error::from_raw_os_error(errno).to_string()),
            });
        }
        if written as usize != expected {
            return Err(VnodeProbe::Failed(format!(
                "想定外の大きさの結果です（{written} バイト）"
            )));
        }
        Ok(())
    }

    /// 一覧を `capacity` 個分のバッファで取得した結果（書き込まれた量と errno）を解釈する。
    /// `entry_size` は、戻り値の 1 要素あたりの量（バイト数で返す API なら要素の大きさ）。
    fn decode_list_result(
        written: c_int,
        errno: c_int,
        entry_size: usize,
        capacity: usize,
    ) -> Result<ListStep, Probe> {
        if written < 0 || (written == 0 && errno != 0) {
            return Err(probe_from_errno(errno));
        }
        let written = written as usize;
        if !written.is_multiple_of(entry_size) {
            return Err(Probe::Failed(format!(
                "想定外の大きさの結果です（{written} バイト）"
            )));
        }
        let count = written / entry_size;
        if count < capacity {
            Ok(ListStep::Complete(count))
        } else {
            Ok(ListStep::Grow)
        }
    }

    /// 全 PID の一覧。バッファが埋まったら広げて取り直し、取り切れなければ失敗にする。
    fn list_all_pids() -> Result<Vec<libc::pid_t>, String> {
        let failed = |reason: String| format!("プロセスの一覧を取得できません: {reason}");
        clear_errno();
        // SAFETY: バッファ無しで呼ぶと、PID の数の見積もりだけを返す。
        let estimate = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if estimate <= 0 {
            return Err(failed(
                io::Error::from_raw_os_error(last_errno()).to_string(),
            ));
        }
        let mut capacity = estimate as usize + 64;
        for _ in 0..MAX_LIST_ATTEMPTS {
            let mut pids: Vec<libc::pid_t> = vec![0; capacity];
            clear_errno();
            // SAFETY: pids は capacity 個の pid_t を書ける。戻り値は PID の数。
            let count = unsafe {
                libc::proc_listallpids(
                    pids.as_mut_ptr().cast(),
                    (capacity * size_of::<libc::pid_t>()) as c_int,
                )
            };
            match decode_list_result(count, last_errno(), 1, capacity) {
                // このプロセス自身がいるので、空の一覧は失敗
                Ok(ListStep::Complete(0)) => return Err(failed("一覧が空です".to_string())),
                Ok(ListStep::Complete(count)) => {
                    pids.truncate(count);
                    return Ok(pids);
                }
                Ok(ListStep::Grow) => capacity *= 2,
                Err(Probe::Failed(reason)) => return Err(failed(reason)),
                Err(probe) => return Err(failed(format!("{probe:?}"))),
            }
        }
        Err("プロセスの一覧が取り切れません".to_string())
    }

    /// `proc_pidinfo` で固定長の構造体を取得する。
    fn pid_info<T>(pid: libc::pid_t, flavor: c_int) -> Result<T, Probe> {
        let mut info = MaybeUninit::<T>::zeroed();
        let size = size_of::<T>();
        clear_errno();
        // SAFETY: info は size バイトを書ける。libproc は失敗を 0 で返し errno を設定する。
        let written =
            unsafe { libc::proc_pidinfo(pid, flavor, 0, info.as_mut_ptr().cast(), size as c_int) };
        decode_fixed_size(written, last_errno(), size)?;
        // SAFETY: 整数だけの C の構造体で、ゼロ初期化の後に size バイトが書き込まれた。
        Ok(unsafe { info.assume_init() })
    }

    fn short_bsd_info(pid: libc::pid_t) -> Result<libc::proc_bsdshortinfo, Probe> {
        pid_info(pid, libc::PROC_PIDT_SHORTBSDINFO)
    }

    /// プロセスの起動時刻（秒, マイクロ秒）。PID の再利用を見分けるのに使う。
    fn start_time(pid: libc::pid_t) -> Result<(u64, u64), Probe> {
        let info: libc::proc_bsdinfo = pid_info(pid, libc::PROC_PIDTBSDINFO)?;
        Ok((info.pbi_start_tvsec, info.pbi_start_tvusec))
    }

    /// プロセスの fd の一覧。バッファが埋まったら広げて取り直し、取り切れなければ失敗にする。
    fn list_fds(pid: libc::pid_t) -> Result<Vec<libc::proc_fdinfo>, Probe> {
        let entry_size = size_of::<libc::proc_fdinfo>();
        clear_errno();
        // SAFETY: バッファ無しで呼ぶと、必要なバイト数の見積もり（余裕込み）だけを返す。
        let estimate =
            unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        if estimate <= 0 {
            return Err(probe_from_errno(last_errno()));
        }
        let mut capacity = estimate as usize / entry_size + 16;
        for _ in 0..MAX_LIST_ATTEMPTS {
            let empty = libc::proc_fdinfo {
                proc_fd: 0,
                proc_fdtype: 0,
            };
            let mut fds = vec![empty; capacity];
            clear_errno();
            // SAFETY: fds は capacity 個の proc_fdinfo を書ける。戻り値はバイト数。
            let written = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDLISTFDS,
                    0,
                    fds.as_mut_ptr().cast(),
                    (capacity * entry_size) as c_int,
                )
            };
            match decode_list_result(written, last_errno(), entry_size, capacity)? {
                ListStep::Complete(count) => {
                    fds.truncate(count);
                    return Ok(fds);
                }
                ListStep::Grow => capacity *= 2,
            }
        }
        Err(Probe::Failed("fd の一覧が取り切れません".to_string()))
    }

    /// vnode の fd が指すファイルのデバイス番号（32 ビット）と inode
    fn vnode_identity(pid: libc::pid_t, fd: c_int) -> Result<(u32, u64), VnodeProbe> {
        let mut info = MaybeUninit::<VnodeFdInfo>::zeroed();
        let size = size_of::<VnodeFdInfo>();
        clear_errno();
        // SAFETY: info は size バイトを書ける。libproc は失敗を 0 で返し errno を設定する。
        let written = unsafe {
            libc::proc_pidfdinfo(
                pid,
                fd,
                PROC_PIDFDVNODEINFO,
                info.as_mut_ptr().cast(),
                size as c_int,
            )
        };
        decode_vnode_result(written, last_errno(), size)?;
        // SAFETY: 整数だけの C の構造体で、ゼロ初期化の後に size バイトが書き込まれた。
        let info = unsafe { info.assume_init() };
        Ok((info.vnode.vi_stat.vst_dev, info.vnode.vi_stat.vst_ino))
    }

    /// プロセスのコード署名の状態（`CS_OPS_STATUS`）
    fn code_signing_flags(pid: libc::pid_t) -> Result<u32, Probe> {
        let mut flags: u32 = 0;
        clear_errno();
        // SAFETY: flags は 4 バイトを書ける。
        let result = unsafe {
            csops(
                pid,
                CS_OPS_STATUS,
                (&raw mut flags).cast(),
                size_of::<u32>(),
            )
        };
        if result != 0 {
            return Err(probe_from_errno(last_errno()));
        }
        Ok(flags)
    }

    /// プロセスの実行ファイルの名前（パスの末尾）
    fn executable_name(pid: libc::pid_t) -> Result<String, Probe> {
        let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        clear_errno();
        // SAFETY: buffer は PROC_PIDPATHINFO_MAXSIZE バイトを書ける。
        let length =
            unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        if length <= 0 {
            return Err(probe_from_errno(last_errno()));
        }
        buffer.truncate(length as usize);
        let path = String::from_utf8_lossy(&buffer).into_owned();
        Ok(Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or(path))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_decode_fixed_size() {
            assert_eq!(decode_fixed_size(64, 0, 64), Ok(()));
            assert_eq!(decode_fixed_size(0, libc::ESRCH, 64), Err(Probe::Gone));
            assert_eq!(decode_fixed_size(0, libc::EPERM, 64), Err(Probe::Denied));
            // 0 なのに errno が無いのは、成功ではなく失敗
            assert!(matches!(decode_fixed_size(0, 0, 64), Err(Probe::Failed(_))));
            assert!(matches!(
                decode_fixed_size(0, libc::EINVAL, 64),
                Err(Probe::Failed(_))
            ));
            // 短い結果を正常なデータとして読まない
            assert!(matches!(
                decode_fixed_size(32, 0, 64),
                Err(Probe::Failed(_))
            ));
        }

        #[test]
        fn test_decode_vnode_result() {
            let size = size_of::<VnodeFdInfo>();
            assert_eq!(decode_vnode_result(size as c_int, 0, size), Ok(()));
            assert_eq!(
                decode_vnode_result(0, libc::EBADF, size),
                Err(VnodeProbe::Closed)
            );
            assert_eq!(
                decode_vnode_result(0, libc::ESRCH, size),
                Err(VnodeProbe::Gone)
            );
            assert_eq!(
                decode_vnode_result(0, libc::ENOENT, size),
                Err(VnodeProbe::Unavailable)
            );
            assert_eq!(
                decode_vnode_result(0, libc::EPERM, size),
                Err(VnodeProbe::Denied)
            );
            assert!(matches!(
                decode_vnode_result(0, 0, size),
                Err(VnodeProbe::Failed(_))
            ));
            assert!(matches!(
                decode_vnode_result(0, libc::EIO, size),
                Err(VnodeProbe::Failed(_))
            ));
            assert!(matches!(
                decode_vnode_result(24, 0, size),
                Err(VnodeProbe::Failed(_))
            ));
        }

        #[test]
        fn test_decode_list_result() {
            assert_eq!(decode_list_result(24, 0, 8, 8), Ok(ListStep::Complete(3)));
            // fd がすべて閉じたプロセス
            assert_eq!(decode_list_result(0, 0, 8, 8), Ok(ListStep::Complete(0)));
            // バッファが埋まったら、取り切れたとはみなさない
            assert_eq!(decode_list_result(64, 0, 8, 8), Ok(ListStep::Grow));
            assert_eq!(decode_list_result(0, libc::ESRCH, 8, 8), Err(Probe::Gone));
            assert_eq!(decode_list_result(0, libc::EPERM, 8, 8), Err(Probe::Denied));
            assert!(matches!(
                decode_list_result(-1, libc::EINVAL, 8, 8),
                Err(Probe::Failed(_))
            ));
            assert!(matches!(
                decode_list_result(20, 0, 8, 8),
                Err(Probe::Failed(_))
            ));
        }

        #[test]
        fn test_classify_denied_process() {
            let started = (1_700_000_000, 1);
            assert_eq!(
                classify_denied_process(10, CS_PLATFORM_BINARY, "routined", started, started),
                Ok(())
            );
            // プラットフォームバイナリでなければ対象外にしない
            assert!(classify_denied_process(10, 0, "routined", started, started).is_err());
            // git とその補助プログラムは、プラットフォームバイナリでも対象外にしない
            for name in ["git", "git-remote-https"] {
                assert!(
                    classify_denied_process(10, CS_PLATFORM_BINARY, name, started, started)
                        .is_err(),
                    "{name}"
                );
            }
            // PID が再利用されていたら対象外にしない
            let reused = classify_denied_process(
                10,
                CS_PLATFORM_BINARY,
                "routined",
                started,
                (1_700_000_100, 0),
            );
            assert!(reused.unwrap_err().contains("再利用"));
        }

        #[test]
        fn test_vnode_identity_matches_fstat_for_own_fd() {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::MetadataExt;
            let temp = tempfile::TempDir::new().unwrap();
            let path = temp.path().join("index.lock");
            std::fs::File::create(&path).unwrap();
            let file = std::fs::File::open(&path).unwrap();
            let metadata = file.metadata().unwrap();

            let (dev, ino) = vnode_identity(std::process::id() as libc::pid_t, file.as_raw_fd())
                .unwrap_or_else(|probe| panic!("{probe:?}"));

            assert_eq!(dev, metadata.dev() as u32);
            assert_eq!(ino, metadata.ino());
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::{Deadline, Observation, ScanTarget};

    pub(super) fn scan(
        _target: &ScanTarget,
        _deadline: &Deadline,
        _observation: &mut Observation,
    ) -> Result<(), String> {
        Err("この OS ではプロセスを検査できません".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_is_none_without_exclusions() {
        assert_eq!(ScanCoverage::default().note(), None);
    }

    #[test]
    fn test_note_lists_excluded_platform_processes() {
        let coverage = ScanCoverage {
            excluded_platform_processes: vec!["routined".to_string(), "lsd".to_string()],
            unavailable_vnode_fds: 0,
        };
        let note = coverage.note().unwrap();
        assert!(note.contains("2 OS platform process(es)"), "{note}");
        assert!(note.contains("(routined, lsd)"), "{note}");
        assert!(
            note.contains("--assume-quiescent remains the guarantee"),
            "{note}"
        );
    }

    #[test]
    fn test_note_combines_platform_processes_and_unavailable_vnodes() {
        let coverage = ScanCoverage {
            excluded_platform_processes: vec!["routined".to_string()],
            unavailable_vnode_fds: 3,
        };
        let note = coverage.note().unwrap();
        assert!(
            note.contains(
                "1 OS platform process(es) that deny inspection (routined) and 3 file descriptor(s)"
            ),
            "{note}"
        );
    }

    #[test]
    fn test_should_inspect_owner() {
        assert!(should_inspect_owner(501, 501));
        assert!(!should_inspect_owner(0, 501));
        assert!(!should_inspect_owner(502, 501));
        // root は全プロセスを調べる
        assert!(should_inspect_owner(501, 0));
    }

    #[test]
    fn test_is_git_program_name() {
        assert!(is_git_program_name("git"));
        assert!(is_git_program_name("git-remote-https"));
        assert!(is_git_program_name("git-upload-pack"));
        assert!(!is_git_program_name("gitk"));
        assert!(!is_git_program_name("routined"));
        assert!(!is_git_program_name("lazygit"));
    }

    #[test]
    fn test_observation_prefers_holders_over_incomplete() {
        let observation = Observation {
            holders: vec![42],
            coverage: ScanCoverage::default(),
            incomplete: Some("途中で失敗".to_string()),
        };
        assert_eq!(
            observation.into_evidence(),
            OpenHandleEvidence::Found { pids: vec![42] }
        );
    }

    #[test]
    fn test_observation_reports_incomplete_without_holders() {
        let observation = Observation {
            holders: Vec::new(),
            coverage: ScanCoverage::default(),
            incomplete: Some("途中で失敗".to_string()),
        };
        assert_eq!(
            observation.into_evidence(),
            OpenHandleEvidence::Unknown {
                reason: "途中で失敗".to_string()
            }
        );
    }

    #[test]
    fn test_expired_deadline_reports_timeout() {
        let deadline = Deadline(Instant::now() - Duration::from_secs(1));
        assert!(
            deadline
                .check()
                .unwrap_err()
                .contains("秒以内に検査を終えられませんでした")
        );
    }

    #[test]
    fn test_scan_finishing_after_deadline_is_not_a_clean_result() {
        // 走査そのものは成功しても、締め切りを過ぎていれば調べきれないとする
        let deadline = Deadline(Instant::now() - Duration::from_secs(1));
        let evidence = run_scan(&deadline, |_, _| Ok(()));
        assert!(
            matches!(evidence, OpenHandleEvidence::Unknown { .. }),
            "{evidence:?}"
        );
    }

    #[test]
    fn test_scan_error_keeps_holders_found_before_it() {
        let deadline = Deadline::after(SCAN_TIME_LIMIT);
        let evidence = run_scan(&deadline, |_, observation| {
            observation.holders.push(7);
            Err("途中で失敗".to_string())
        });
        assert_eq!(evidence, OpenHandleEvidence::Found { pids: vec![7] });
    }

    /// `/proc` の代わりに作ったディレクトリの木で、走査の分岐を確かめる。
    #[cfg(unix)]
    mod procfs_tree {
        use super::super::*;
        use std::fs;
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
        use std::path::{Path, PathBuf};

        struct Tree {
            _temp: tempfile::TempDir,
            proc_root: PathBuf,
            lock: PathBuf,
            other: PathBuf,
        }

        fn make_tree() -> Tree {
            let temp = tempfile::TempDir::new().unwrap();
            let proc_root = temp.path().join("proc");
            fs::create_dir(&proc_root).unwrap();
            let lock = temp.path().join("index.lock");
            fs::write(&lock, "").unwrap();
            let other = temp.path().join("other");
            fs::write(&other, "").unwrap();
            Tree {
                _temp: temp,
                proc_root,
                lock,
                other,
            }
        }

        /// `<proc>/<pid>/fd/<n>` を、開いているファイルを指す symlink として作る。
        fn add_process(proc_root: &Path, pid: u32, fds: &[(RawFd, &Path)]) -> PathBuf {
            let fd_dir = proc_root.join(pid.to_string()).join("fd");
            fs::create_dir_all(&fd_dir).unwrap();
            for (fd, file) in fds {
                symlink(file, fd_dir.join(fd.to_string())).unwrap();
            }
            fd_dir
        }

        fn scan_tree(tree: &Tree, own_pid: u32, own_fd: RawFd) -> OpenHandleEvidence {
            let metadata = fs::metadata(&tree.lock).unwrap();
            let target = ScanTarget {
                file: FileIdentity {
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                },
                own_pid,
                own_fd,
                euid: metadata.uid(),
                egid: metadata.gid(),
            };
            run_scan(
                &Deadline::after(SCAN_TIME_LIMIT),
                |deadline, observation| {
                    procfs::scan(&tree.proc_root, &target, deadline, observation)
                },
            )
        }

        #[test]
        fn test_finds_process_holding_the_file() {
            let tree = make_tree();
            add_process(&tree.proc_root, 100, &[(3, &tree.other)]);
            add_process(&tree.proc_root, 200, &[(0, &tree.other), (5, &tree.lock)]);
            fs::create_dir(tree.proc_root.join("self-not-a-pid")).unwrap();

            assert_eq!(
                scan_tree(&tree, 1, 3),
                OpenHandleEvidence::Found { pids: vec![200] }
            );
        }

        #[test]
        fn test_skips_only_own_inspection_fd() {
            let tree = make_tree();
            add_process(&tree.proc_root, 300, &[(4, &tree.lock)]);
            assert_eq!(
                scan_tree(&tree, 300, 4),
                OpenHandleEvidence::NoneObserved {
                    coverage: ScanCoverage::default()
                }
            );

            // 同じプロセスの別の fd は見逃さない
            let tree = make_tree();
            add_process(&tree.proc_root, 300, &[(4, &tree.lock), (5, &tree.lock)]);
            assert_eq!(
                scan_tree(&tree, 300, 4),
                OpenHandleEvidence::Found { pids: vec![300] }
            );
        }

        #[test]
        fn test_closed_fd_and_exited_process_are_skipped() {
            let tree = make_tree();
            // 調べている間に閉じた fd（指す先が無い）
            add_process(&tree.proc_root, 400, &[(3, &tree.proc_root.join("gone"))]);
            // fd のディレクトリが無い（終了した）プロセス
            fs::create_dir(tree.proc_root.join("401")).unwrap();

            assert!(matches!(
                scan_tree(&tree, 1, 3),
                OpenHandleEvidence::NoneObserved { .. }
            ));
        }

        #[test]
        fn test_unreadable_fd_directory_is_inspection_incomplete() {
            // SAFETY: geteuid は前提条件なしに呼べる。
            if unsafe { libc::geteuid() } == 0 {
                // root は権限で読み取りを止められない
                return;
            }
            let tree = make_tree();
            let fd_dir = add_process(&tree.proc_root, 500, &[(3, &tree.other)]);
            fs::write(
                tree.proc_root.join("500").join("status"),
                "Name:\tworker\nUid:\t1\t1\t1\t1\nGid:\t2\t2\t2\t2\n",
            )
            .unwrap();
            fs::set_permissions(&fd_dir, fs::Permissions::from_mode(0o000)).unwrap();

            let evidence = scan_tree(&tree, 1, 3);

            fs::set_permissions(&fd_dir, fs::Permissions::from_mode(0o755)).unwrap();
            // 資格情報が違っても対象外にはせず、名前と資格情報を理由に添えて調べきれないとする
            match evidence {
                OpenHandleEvidence::Unknown { reason } => {
                    assert!(reason.contains("PID 500、worker"), "{reason}");
                    assert!(
                        reason.contains("ptrace の ID の照合は一致しない"),
                        "{reason}"
                    );
                }
                other => panic!("調べきれないとすべき: {other:?}"),
            }
        }

        #[test]
        fn test_holder_is_reported_even_after_an_uninspectable_process() {
            // SAFETY: geteuid は前提条件なしに呼べる。
            if unsafe { libc::geteuid() } == 0 {
                return;
            }
            let tree = make_tree();
            let fd_dir = add_process(&tree.proc_root, 500, &[(3, &tree.other)]);
            add_process(&tree.proc_root, 700, &[(9, &tree.lock)]);
            fs::set_permissions(&fd_dir, fs::Permissions::from_mode(0o000)).unwrap();

            let evidence = scan_tree(&tree, 1, 3);

            fs::set_permissions(&fd_dir, fs::Permissions::from_mode(0o755)).unwrap();
            // 調べきれないプロセスがあっても走査は続け、ロックを開いているプロセスを報告する
            assert_eq!(evidence, OpenHandleEvidence::Found { pids: vec![700] });
        }

        #[test]
        fn test_missing_proc_root_is_inspection_incomplete() {
            let tree = make_tree();
            fs::remove_dir(&tree.proc_root).unwrap();

            assert!(matches!(
                scan_tree(&tree, 1, 3),
                OpenHandleEvidence::Unknown { .. }
            ));
        }

        #[test]
        fn test_expired_deadline_stops_the_scan() {
            let tree = make_tree();
            add_process(&tree.proc_root, 600, &[(3, &tree.other)]);
            let metadata = fs::metadata(&tree.lock).unwrap();
            let target = ScanTarget {
                file: FileIdentity {
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                },
                own_pid: 1,
                own_fd: 3,
                euid: metadata.uid(),
                egid: metadata.gid(),
            };
            let deadline = Deadline(Instant::now() - Duration::from_secs(1));

            let evidence = run_scan(&deadline, |deadline, observation| {
                procfs::scan(&tree.proc_root, &target, deadline, observation)
            });

            assert!(
                matches!(evidence, OpenHandleEvidence::Unknown { .. }),
                "{evidence:?}"
            );
        }
    }

    /// テストのプロセスでは別のテストのスレッドが子プロセスを起動し、exec までの間だけ、子が
    /// このプロセスの fd の写しを持つことがある。そのため、見つかった PID の一覧は等号ではなく、
    /// 確かめたい PID を含むか（含まないか）で判定する。
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    mod scan {
        use super::super::*;
        use std::fs::File;
        use std::io::{BufRead, BufReader};
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;
        use std::process::{Command, Stdio};

        fn identity_of(file: &File) -> FileIdentity {
            let metadata = file.metadata().unwrap();
            FileIdentity {
                dev: metadata.dev(),
                ino: metadata.ino(),
            }
        }

        fn euid() -> u32 {
            // SAFETY: geteuid は前提条件なしに呼べる。
            unsafe { libc::geteuid() }
        }

        fn egid() -> u32 {
            // SAFETY: getegid は前提条件なしに呼べる。
            unsafe { libc::getegid() }
        }

        #[test]
        fn test_own_inspection_fd_is_not_reported() {
            let temp = tempfile::TempDir::new().unwrap();
            let path = temp.path().join("index.lock");
            File::create(&path).unwrap();
            let file = File::open(&path).unwrap();

            let evidence = inspect(identity_of(&file), file.as_raw_fd(), euid(), egid());

            match evidence {
                OpenHandleEvidence::Found { pids } => {
                    assert!(!pids.contains(&std::process::id()), "{pids:?}")
                }
                // fd を辿れないプロセスがある環境（CI のランナーなど）では、調べきれないとなる
                OpenHandleEvidence::NoneObserved { .. } | OpenHandleEvidence::Unknown { .. } => {}
            }
        }

        #[test]
        fn test_other_fd_of_own_process_is_reported() {
            let temp = tempfile::TempDir::new().unwrap();
            let path = temp.path().join("index.lock");
            File::create(&path).unwrap();
            let inspected = File::open(&path).unwrap();
            let _another = File::open(&path).unwrap();

            let evidence = inspect(
                identity_of(&inspected),
                inspected.as_raw_fd(),
                euid(),
                egid(),
            );

            match evidence {
                OpenHandleEvidence::Found { pids } => {
                    assert!(pids.contains(&std::process::id()), "{pids:?}")
                }
                other => panic!("見つからない: {other:?}"),
            }
        }

        #[test]
        fn test_child_process_holding_the_file_is_reported() {
            let temp = tempfile::TempDir::new().unwrap();
            let path = temp.path().join("index.lock");
            File::create(&path).unwrap();
            // 子は stdin が閉じるまでロックを開いたまま待つ（固定の寿命に頼らない）
            let mut child = Command::new("sh")
                .args(["-c", "exec 3<\"$1\"; echo ready; exec cat >/dev/null", "sh"])
                .arg(&path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut line = String::new();
            BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert_eq!(line.trim(), "ready");
            let inspected = File::open(&path).unwrap();

            let evidence = inspect(
                identity_of(&inspected),
                inspected.as_raw_fd(),
                euid(),
                egid(),
            );

            drop(child.stdin.take());
            child.wait().unwrap();
            match evidence {
                OpenHandleEvidence::Found { pids } => {
                    assert!(pids.contains(&child.id()), "{pids:?}")
                }
                other => panic!("見つからない: {other:?}"),
            }
        }
    }
}
