//! git-lock サブコマンドの統合テスト
//!
//! 別プロセスの safe-rm を実際の Git リポジトリに対して動かし、異常終了で残った
//! `index.lock` の削除と拒否、通常の削除での案内を検証する。

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

const LOCK: &str = "index.lock";

/// 異常終了で残ったロックの代わり（最終更新から 1 時間）
const STALE: Duration = Duration::from_secs(3600);

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

/// safe-rm を実行する。設定はテスト実行ユーザーの実設定に左右されないよう、未存在のパスを使う。
fn safe_rm(args: &[&str], cwd: &Path) -> Output {
    safe_rm_with(args, cwd, None, &[])
}

/// 設定ファイルと環境変数を指定して safe-rm を実行する。
fn safe_rm_with(args: &[&str], cwd: &Path, config: Option<&Path>, envs: &[(&str, &str)]) -> Output {
    let missing_config = cwd.join(".safe-rm-test-missing-config.toml");
    let mut command = Command::new(env!("CARGO_BIN_EXE_safe-rm"));
    command
        .args(args)
        .current_dir(cwd)
        .env("SAFE_RM_CONFIG", config.unwrap_or(&missing_config))
        // git のフックの中から動かされても、外の GIT_INDEX_FILE を引き継がない
        .env_remove("GIT_INDEX_FILE");
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().expect("safe-rm を起動できない");
    Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git を起動できない");
    assert!(
        output.status.success(),
        "git {:?} が失敗: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// コミットを 1 つ持つリポジトリを `dir` に作る。
fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "you@example.com"]);
    git(dir, &["config", "user.name", "you"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
}

/// 一時ディレクトリの中に `repo` を作り、(一時ディレクトリ, canonical な repo のパス) を返す。
fn create_repo() -> (TempDir, PathBuf) {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().canonicalize().unwrap().join("repo");
    init_repo(&repo);
    (temp, repo)
}

/// 最終更新を `now - age` にした空のロックを作る（開いたままにしない）。
fn write_lock(path: &Path, age: Duration) {
    File::create(path)
        .unwrap()
        .set_modified(SystemTime::now() - age)
        .unwrap();
}

fn config_file(dir: &Path, content: &str) -> PathBuf {
    let path = dir.join("config.toml");
    fs::write(&path, content).unwrap();
    path
}

fn removed_line(gitdir: &Path) -> String {
    format!(
        "removed: {} (git index lock; quiescence asserted)",
        gitdir.canonicalize().unwrap().join(LOCK).display()
    )
}

/// ロックを開いたまま待つ子プロセス。stdin が閉じるまで待ち（固定の寿命に頼らない）、drop で終了させる。
struct Holder(Child);

impl Holder {
    fn spawn(path: &Path) -> Self {
        let mut child = Command::new("sh")
            .args(["-c", "exec 3<\"$1\"; echo ready; exec cat >/dev/null", "sh"])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "ready");
        Self(child)
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn assert_refused(output: &Output, expected: &str) {
    assert_eq!(output.code, 2, "stderr: {}", output.stderr);
    assert!(
        output.stderr.contains("index.lock を削除できません。"),
        "stderr: {}",
        output.stderr
    );
    assert!(
        output.stderr.contains(expected),
        "{expected:?} が無い。stderr: {}",
        output.stderr
    );
    assert!(output.stdout.is_empty(), "stdout: {}", output.stdout);
}

/// この環境で、ロックを開いているプロセスの検査を最後までできるか（最初の 1 回だけ確かめる）。
///
/// fd を辿れない同じユーザーのプロセスがある環境（GitHub Actions の Linux ランナーなど）では、
/// git-lock は調べきれないとして拒否する。それが正しい動きなので、削除まで成功するはずの
/// テストは、その環境では拒否の理由とロックが残ることを確かめる。
fn scan_is_complete() -> bool {
    static COMPLETE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *COMPLETE.get_or_init(|| {
        let (_temp, repo) = create_repo();
        write_lock(&repo.join(".git").join(LOCK), STALE);
        let output = safe_rm(
            &["git-lock", "--repo", ".", "--assume-quiescent", "-n"],
            &repo,
        );
        if output.code == 0 {
            return true;
        }
        // 調べきれない理由が、権限で fd を辿れないこと（環境の制約）であることを確かめる
        assert_refused(&output, "開いているプロセスを調べきれませんでした");
        assert!(
            output.stderr.contains("Permission denied"),
            "stderr: {}",
            output.stderr
        );
        eprintln!(
            "note: この環境では検査を最後までできないので、削除の成功の代わりに拒否を確かめる: {}",
            output.stderr.trim()
        );
        false
    })
}

/// 削除まで成功するはずの実行を確かめる。検査を最後までできない環境では、調べきれないとして
/// 拒否し、ロックを残すことを確かめて false を返す（呼び出し側は以降の確認を省く）。
fn expect_success(output: &Output, lock: &Path) -> bool {
    if scan_is_complete() {
        assert_eq!(output.code, 0, "stderr: {}", output.stderr);
        true
    } else {
        assert_refused(output, "開いているプロセスを調べきれませんでした");
        assert!(lock.exists());
        false
    }
}

// =============================================================================
// 削除できる配置
// =============================================================================

mod removal_tests {
    use super::*;

    #[test]
    fn test_removes_stale_lock_and_git_can_resume() {
        let (_temp, repo) = create_repo();
        let lock = repo.join(".git").join(LOCK);
        write_lock(&lock, STALE);

        let output = safe_rm(&["git-lock", "--repo", ".", "--assume-quiescent"], &repo);

        if !expect_success(&output, &lock) {
            return;
        }
        assert_eq!(output.stdout.trim(), removed_line(&repo.join(".git")));
        assert!(!lock.exists());
        git(&repo, &["reset", "-q", "--hard"]);
    }

    #[test]
    fn test_dry_run_reports_and_keeps_lock() {
        let (_temp, repo) = create_repo();
        let lock = repo.join(".git").join(LOCK);
        write_lock(&lock, STALE);

        let output = safe_rm(
            &["git-lock", "--repo", ".", "--assume-quiescent", "-n"],
            &repo,
        );

        if !expect_success(&output, &lock) {
            return;
        }
        assert_eq!(
            output.stdout.trim(),
            format!(
                "would remove: {} (git index lock; quiescence asserted)",
                lock.display()
            )
        );
        assert!(lock.exists());
    }

    #[test]
    fn test_accepts_gitdir_as_repo() {
        let (_temp, repo) = create_repo();
        let lock = repo.join(".git").join(LOCK);
        write_lock(&lock, STALE);

        let output = safe_rm(&["git-lock", "--repo", ".git", "--assume-quiescent"], &repo);

        if !expect_success(&output, &lock) {
            return;
        }
        assert!(!lock.exists());
    }

    #[test]
    fn test_resolves_relative_repo_from_cwd() {
        let (_temp, repo) = create_repo();
        let lock = repo.join(".git").join(LOCK);
        write_lock(&lock, STALE);
        let subdir = repo.join("src");
        fs::create_dir(&subdir).unwrap();

        let output = safe_rm(&["git-lock", "--repo", "..", "--assume-quiescent"], &subdir);

        if !expect_success(&output, &lock) {
            return;
        }
        assert!(!lock.exists());
    }

    #[test]
    fn test_linked_worktree_removes_only_its_own_lock() {
        let (temp, repo) = create_repo();
        let worktree = temp.path().canonicalize().unwrap().join("wt");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "wt",
                worktree.to_str().unwrap(),
            ],
        );
        let main_lock = repo.join(".git").join(LOCK);
        let worktree_lock = repo.join(".git").join("worktrees").join("wt").join(LOCK);
        write_lock(&main_lock, STALE);
        write_lock(&worktree_lock, STALE);

        let output = safe_rm(
            &[
                "git-lock",
                "--repo",
                worktree.to_str().unwrap(),
                "--assume-quiescent",
            ],
            &repo,
        );

        if !expect_success(&output, &worktree_lock) {
            return;
        }
        assert_eq!(
            output.stdout.trim(),
            removed_line(&repo.join(".git").join("worktrees").join("wt"))
        );
        assert!(!worktree_lock.exists());
        assert!(main_lock.exists(), "本体の worktree のロックは残す");
    }

    #[test]
    fn test_submodule_lock_uses_its_own_gitdir() {
        let (temp, repo) = create_repo();
        let upstream = temp.path().canonicalize().unwrap().join("upstream");
        init_repo(&upstream);
        git(
            &repo,
            &["submodule", "add", "-q", upstream.to_str().unwrap(), "sub"],
        );
        let lock = repo.join(".git").join("modules").join("sub").join(LOCK);
        write_lock(&lock, STALE);

        let output = safe_rm(&["git-lock", "--repo", "sub", "--assume-quiescent"], &repo);

        if !expect_success(&output, &lock) {
            return;
        }
        assert_eq!(
            output.stdout.trim(),
            removed_line(&repo.join(".git").join("modules").join("sub"))
        );
        assert!(!lock.exists());
    }

    #[test]
    fn test_separate_gitdir_is_resolved_from_worktree() {
        let temp = TempDir::new().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let gitdir = base.join("gitdir");
        let work = base.join("work");
        git(
            &base,
            &[
                "init",
                "-q",
                "--separate-git-dir",
                gitdir.to_str().unwrap(),
                work.to_str().unwrap(),
            ],
        );
        let lock = gitdir.join(LOCK);
        write_lock(&lock, STALE);

        let output = safe_rm(&["git-lock", "--repo", "work", "--assume-quiescent"], &base);

        if !expect_success(&output, &lock) {
            return;
        }
        assert_eq!(output.stdout.trim(), removed_line(&gitdir));
        assert!(!lock.exists());
    }

    #[test]
    fn test_separate_gitdir_under_directory_owned_by_another_user() {
        // gitdir の親（/tmp は root の所有）が自分の所有でなくても、worktree から開けるなら削除できる。
        // gitdir から開き直すと、libgit2 が親を workdir と推定して所有者の検証で拒否してしまう。
        use std::os::unix::fs::MetadataExt;
        let shared = Path::new("/tmp");
        let temp = TempDir::new().unwrap();
        // root で動かしたときは /tmp も自分の所有なので、この配置を作れない
        if fs::metadata(shared).unwrap().uid() == fs::metadata(temp.path()).unwrap().uid() {
            return;
        }
        let gitdir_holder = tempfile::Builder::new()
            .prefix("safe-rm-gitdir-")
            .tempdir_in(shared)
            .unwrap();
        let gitdir = gitdir_holder.path().canonicalize().unwrap();
        let work = temp.path().canonicalize().unwrap().join("work");
        git(
            temp.path(),
            &[
                "init",
                "-q",
                "--separate-git-dir",
                gitdir.to_str().unwrap(),
                work.to_str().unwrap(),
            ],
        );
        let lock = gitdir.join(LOCK);
        write_lock(&lock, STALE);

        let output = safe_rm(&["git-lock", "--repo", ".", "--assume-quiescent"], &work);

        if !expect_success(&output, &lock) {
            return;
        }
        assert_eq!(output.stdout.trim(), removed_line(&gitdir));
        assert!(!lock.exists());
    }

    #[test]
    fn test_configured_minimum_age_is_used() {
        let (temp, repo) = create_repo();
        let lock = repo.join(".git").join(LOCK);
        write_lock(&lock, Duration::from_secs(120));
        let args = ["git-lock", "--repo", ".", "--assume-quiescent"];

        // 既定の 300 秒には足りない
        let output = safe_rm(&args, &repo);
        assert_refused(&output, "秒しかたっていません（必要なのは 300 秒）");
        assert!(lock.exists());

        let config = config_file(temp.path(), "[git_locks]\nmin_age_secs = 60\n");
        let output = safe_rm_with(&args, &repo, Some(&config), &[]);
        if !expect_success(&output, &lock) {
            return;
        }
        assert!(!lock.exists());
    }
}

// =============================================================================
// 拒否（exit 2）
// =============================================================================

mod refusal_tests {
    use super::*;

    fn repo_with_stale_lock() -> (TempDir, PathBuf, PathBuf) {
        let (temp, repo) = create_repo();
        let lock = repo.join(".git").join(LOCK);
        write_lock(&lock, STALE);
        (temp, repo, lock)
    }

    #[test]
    fn test_refuses_without_assume_quiescent() {
        let (_temp, repo, lock) = repo_with_stale_lock();

        let output = safe_rm(&["git-lock", "--repo", "."], &repo);

        assert_refused(&output, "--assume-quiescent が必要です");
        assert!(lock.exists());
    }

    #[test]
    fn test_refuses_when_disabled_by_config() {
        let (temp, repo, lock) = repo_with_stale_lock();
        let config = config_file(temp.path(), "[git_locks]\nenabled = false\n");

        let output = safe_rm_with(
            &["git-lock", "--repo", ".", "--assume-quiescent"],
            &repo,
            Some(&config),
            &[],
        );

        assert_refused(&output, "git-lock は設定で無効です");
        assert!(lock.exists());
    }

    #[test]
    fn test_zero_minimum_age_is_a_config_error_that_disables_git_lock() {
        let (temp, repo, lock) = repo_with_stale_lock();
        let config = config_file(temp.path(), "[git_locks]\nmin_age_secs = 0\n");

        let output = safe_rm_with(
            &["git-lock", "--repo", ".", "--assume-quiescent"],
            &repo,
            Some(&config),
            &[],
        );

        assert_refused(&output, "git-lock は設定で無効です");
        assert!(
            output
                .stderr
                .contains("git_locks.min_age_secs must be at least 1")
        );
        assert!(output.stderr.contains("git_locks.enabled = false"));
        assert!(lock.exists());
    }

    #[test]
    fn test_unknown_git_locks_key_is_a_config_error() {
        let (temp, repo, lock) = repo_with_stale_lock();
        let config = config_file(temp.path(), "[git_locks]\nmin_age_sec = 60\n");

        let output = safe_rm_with(
            &["git-lock", "--repo", ".", "--assume-quiescent"],
            &repo,
            Some(&config),
            &[],
        );

        assert_refused(&output, "git-lock は設定で無効です");
        assert!(lock.exists());
    }

    #[test]
    fn test_refuses_bare_repository() {
        let temp = TempDir::new().unwrap();
        let bare = temp.path().canonicalize().unwrap().join("bare.git");
        git(
            temp.path(),
            &["init", "-q", "--bare", bare.to_str().unwrap()],
        );
        let lock = bare.join(LOCK);
        write_lock(&lock, STALE);

        let output = safe_rm(
            &["git-lock", "--repo", "bare.git", "--assume-quiescent"],
            temp.path(),
        );

        assert_refused(&output, "bare リポジトリは対象外です");
        assert!(lock.exists());
    }

    #[test]
    fn test_refuses_when_git_index_file_is_set_even_if_empty() {
        let (_temp, repo, lock) = repo_with_stale_lock();

        let output = safe_rm_with(
            &["git-lock", "--repo", ".", "--assume-quiescent"],
            &repo,
            None,
            &[("GIT_INDEX_FILE", "")],
        );

        assert_refused(&output, "GIT_INDEX_FILE が設定されています");
        assert!(lock.exists());
    }

    #[test]
    fn test_refuses_subdirectory_without_searching_parents() {
        let (_temp, repo, lock) = repo_with_stale_lock();
        fs::create_dir(repo.join("src")).unwrap();

        let output = safe_rm(&["git-lock", "--repo", "src", "--assume-quiescent"], &repo);

        assert_refused(&output, "リポジトリとして開けません");
        assert!(output.stderr.contains("親ディレクトリは探索しません"));
        assert!(lock.exists());
    }

    #[test]
    fn test_refuses_non_git_directory() {
        let temp = TempDir::new().unwrap();

        let output = safe_rm(
            &["git-lock", "--repo", ".", "--assume-quiescent"],
            temp.path(),
        );

        assert_refused(&output, "リポジトリとして開けません");
    }

    #[test]
    fn test_refuses_non_empty_lock() {
        let (_temp, repo) = create_repo();
        let lock = repo.join(".git").join(LOCK);
        fs::write(&lock, "partial index").unwrap();
        File::options()
            .write(true)
            .open(&lock)
            .unwrap()
            .set_modified(SystemTime::now() - STALE)
            .unwrap();

        let output = safe_rm(&["git-lock", "--repo", ".", "--assume-quiescent"], &repo);

        assert_refused(&output, "index.lock が空ではありません（13 バイト）");
        assert_eq!(fs::read_to_string(&lock).unwrap(), "partial index");
    }

    #[test]
    fn test_refuses_symlink_lock_and_keeps_target() {
        let (_temp, repo) = create_repo();
        let target = repo.join("victim.txt");
        fs::write(&target, "").unwrap();
        let lock = repo.join(".git").join(LOCK);
        std::os::unix::fs::symlink(&target, &lock).unwrap();

        let output = safe_rm(&["git-lock", "--repo", ".", "--assume-quiescent"], &repo);

        assert_refused(&output, "通常ファイルではありません（シンボリックリンク）");
        assert!(
            fs::symlink_metadata(&lock)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(target.exists());
    }

    #[test]
    fn test_refuses_directory_lock() {
        let (_temp, repo) = create_repo();
        let lock = repo.join(".git").join(LOCK);
        fs::create_dir(&lock).unwrap();

        let output = safe_rm(&["git-lock", "--repo", ".", "--assume-quiescent"], &repo);

        assert_refused(&output, "通常ファイルではありません（ディレクトリ）");
        assert!(lock.is_dir());
    }

    #[test]
    fn test_refuses_fifo_lock_without_opening_it() {
        let (_temp, repo) = create_repo();
        let lock = repo.join(".git").join(LOCK);
        let status = Command::new("mkfifo").arg(&lock).status().unwrap();
        assert!(status.success());

        let output = safe_rm(&["git-lock", "--repo", ".", "--assume-quiescent"], &repo);

        assert_refused(&output, "通常ファイルではありません（FIFO）");
        assert!(fs::symlink_metadata(&lock).is_ok());
    }

    #[test]
    fn test_refuses_hard_linked_lock() {
        let (_temp, repo, lock) = repo_with_stale_lock();
        let other = repo.join(".git").join("other-name");
        fs::hard_link(&lock, &other).unwrap();

        let output = safe_rm(&["git-lock", "--repo", ".", "--assume-quiescent"], &repo);

        assert_refused(&output, "index.lock のリンク数が 2 です");
        assert!(lock.exists());
        assert!(other.exists());
    }

    #[test]
    fn test_refuses_recent_lock_with_wait_hint() {
        let (_temp, repo) = create_repo();
        let lock = repo.join(".git").join(LOCK);
        write_lock(&lock, Duration::ZERO);

        let output = safe_rm(&["git-lock", "--repo", ".", "--assume-quiescent"], &repo);

        assert_refused(&output, "秒しかたっていません（必要なのは 300 秒）");
        assert!(output.stderr.contains("秒以上待ってから再実行してください"));
        assert!(lock.exists());
    }

    #[test]
    fn test_refuses_lock_modified_in_the_future() {
        let (_temp, repo) = create_repo();
        let lock = repo.join(".git").join(LOCK);
        File::create(&lock)
            .unwrap()
            .set_modified(SystemTime::now() + STALE)
            .unwrap();

        let output = safe_rm(&["git-lock", "--repo", ".", "--assume-quiescent"], &repo);

        assert_refused(&output, "最終更新時刻が現在より");
        assert!(lock.exists());
    }

    #[test]
    fn test_refuses_lock_held_open_by_another_process() {
        let (_temp, repo, lock) = repo_with_stale_lock();
        let holder = Holder::spawn(&lock);

        let output = safe_rm(&["git-lock", "--repo", ".", "--assume-quiescent"], &repo);

        assert_refused(&output, "index.lock を開いているプロセスがあります");
        assert!(
            output.stderr.contains(&holder.pid().to_string()),
            "PID {} が無い。stderr: {}",
            holder.pid(),
            output.stderr
        );
        assert!(lock.exists());
    }

    #[test]
    fn test_refuses_top_level_delete_options() {
        let (_temp, repo, lock) = repo_with_stale_lock();
        let cases: [&[&str]; 4] = [
            &["-r", "git-lock", "--repo", ".", "--assume-quiescent"],
            &["-f", "git-lock", "--repo", ".", "--assume-quiescent"],
            &["-n", "git-lock", "--repo", ".", "--assume-quiescent"],
            &[
                "-r",
                "--allow-nested-git",
                ".git",
                "git-lock",
                "--repo",
                ".",
                "--assume-quiescent",
            ],
        ];
        for args in cases {
            let output = safe_rm(args, &repo);
            assert_refused(&output, "併用できません");
            assert!(lock.exists(), "{args:?}");
        }
    }

    #[test]
    fn test_path_before_git_lock_is_not_treated_as_subcommand() {
        let (_temp, repo, lock) = repo_with_stale_lock();
        let file = repo.join("file.txt");
        fs::write(&file, "keep").unwrap();

        let output = safe_rm(
            &["file.txt", "git-lock", "--repo", ".", "--assume-quiescent"],
            &repo,
        );

        assert_eq!(output.code, 2, "stderr: {}", output.stderr);
        assert!(file.exists());
        assert!(lock.exists());
    }
}

// =============================================================================
// ロックが無い（exit 1）
// =============================================================================

#[test]
fn test_missing_lock_is_reported_like_rm() {
    let (_temp, repo) = create_repo();

    let output = safe_rm(&["git-lock", "--repo", ".", "--assume-quiescent"], &repo);

    assert_eq!(output.code, 1, "stderr: {}", output.stderr);
    assert!(
        output.stderr.contains("No such file or directory"),
        "stderr: {}",
        output.stderr
    );
}

// =============================================================================
// 通常の削除は例外にせず、git-lock を案内する
// =============================================================================

mod normal_deletion_tests {
    use super::*;

    #[test]
    fn test_normal_delete_refuses_index_lock_with_guidance() {
        let (_temp, repo) = create_repo();
        let lock = repo.join(".git").join(LOCK);
        write_lock(&lock, STALE);

        let output = safe_rm(&[".git/index.lock"], &repo);

        assert_eq!(output.code, 2, "stderr: {}", output.stderr);
        assert!(
            output
                .stderr
                .contains("Git 管理メタデータは削除できません。")
        );
        assert!(
            output.stderr.contains(&format!(
                "safe-rm git-lock --repo {} --assume-quiescent",
                repo.join(".git").display()
            )),
            "stderr: {}",
            output.stderr
        );
        assert!(lock.exists());
    }

    #[test]
    fn test_allowed_paths_does_not_permit_index_lock() {
        let (temp, repo) = create_repo();
        let lock = repo.join(".git").join(LOCK);
        write_lock(&lock, STALE);
        let config = config_file(
            temp.path(),
            &format!(
                "[[allowed_paths]]\npath = \"{}\"\nrecursive = true\n",
                repo.join(".git").display()
            ),
        );

        let output = safe_rm_with(&["-f", ".git/index.lock"], &repo, Some(&config), &[]);

        assert_eq!(output.code, 2, "stderr: {}", output.stderr);
        assert!(output.stderr.contains("safe-rm git-lock --repo"));
        assert!(lock.exists());
    }

    #[test]
    fn test_other_git_metadata_has_no_git_lock_guidance() {
        let (_temp, repo) = create_repo();
        // gitdir（HEAD を持つディレクトリ）の直下でない index.lock にも案内を出さない
        let refs_lock = repo.join(".git").join("refs").join("heads").join(LOCK);
        write_lock(&refs_lock, STALE);

        for target in [".git/HEAD", ".git/refs/heads/index.lock"] {
            let output = safe_rm(&[target], &repo);
            assert_eq!(output.code, 2, "{target}: {}", output.stderr);
            assert!(
                output
                    .stderr
                    .contains("Git 管理メタデータは削除できません。")
            );
            assert!(
                !output.stderr.contains("git-lock"),
                "{target}: {}",
                output.stderr
            );
        }
        assert!(repo.join(".git").join("HEAD").exists());
        assert!(refs_lock.exists());
    }
}
