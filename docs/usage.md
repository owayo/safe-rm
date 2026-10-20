# Usage Examples

Examples of allowed and blocked deletions. The cases marked strict mode assume `allow_project_deletion = false`; in the default mode, modified and untracked files inside the project can be deleted. The basic flags are in the [README](../README.md#usage).

## Allowed Operations

```bash
# Clean file (committed, no changes)
safe-rm src/old_module.rs  # Exit 0

# Ignored file (in .gitignore)
safe-rm target/debug/app   # Exit 0
safe-rm -r node_modules    # Exit 0

# Non-Git directory
safe-rm temp_file.txt      # Exit 0

# Dry run
safe-rm -n file.txt        # Exit 0, shows "would remove: file.txt"

# If an ignored build directory contains vendored Git directories,
# name every reported metadata directory explicitly.
safe-rm -r --allow-nested-git target/vendor/.git target

# An empty .git/index.lock left behind by a crashed git process
# (run from the worktree root after confirming that no git process is running there)
safe-rm git-lock --repo . --assume-quiescent
# Exit 0, shows "removed: /path/to/repo/.git/index.lock (git index lock; quiescence asserted)"

# The gitdir itself also works (here a linked worktree's); -n only runs the checks
safe-rm git-lock --repo /path/to/repo/.git/worktrees/feature --assume-quiescent -n
# Exit 0, shows "would remove: /path/to/repo/.git/worktrees/feature/index.lock (git index lock; quiescence asserted)"
```

## Blocked Operations

```bash
# Modified file (strict mode)
safe-rm src/main.rs
# Exit 2: "未コミットの変更があるファイルは削除できません"

# Outside project
safe-rm /etc/passwd
# Exit 2: "プロジェクト外へのアクセスは禁止されています"

safe-rm ../../../etc/hosts
# Exit 2: "プロジェクト外へのアクセスは禁止されています"

# Untracked file (strict mode)
safe-rm new_feature.rs
# Exit 2: "未コミットの変更があるファイルは削除できません"

# `.` / `..` operands (POSIX rm refuses these too)
safe-rm -r .
# Exit 2: "末尾が '.' または '..' のパスは削除できません"
safe-rm -r sub/..
# Exit 2: "末尾が '.' または '..' のパスは削除できません"
# Name the directory explicitly instead: safe-rm -r sub

# Root operand (GNU rm refuses this too, via --preserve-root)
safe-rm -r /
# Exit 2: "ルートディレクトリは削除できません"

# Trailing separator on something that is not a directory
safe-rm file.txt/
# Exit 1: "cannot remove 'file.txt/': Not a directory"
safe-rm broken-link/
# Exit 1: "cannot remove 'broken-link/': No such file or directory"
# Drop the trailing slash to delete the entry itself: safe-rm file.txt

# A nested Git directory is protected by default, even under an ignored target.
safe-rm -r target
# Exit 2: lists the nested Git metadata paths and their count

# Normal deletion does not remove an index.lock either, even if .git is in allowed_paths
safe-rm .git/index.lock
# Exit 2: "Git 管理メタデータは削除できません"
# The message ends with the command to use:
#   safe-rm git-lock --repo /path/to/repo/.git --assume-quiescent

# git-lock without --assume-quiescent
safe-rm git-lock --repo .
# Exit 2: "--assume-quiescent が必要です"

# git-lock with top-level options (give -n after git-lock instead)
safe-rm -n git-lock --repo . --assume-quiescent
# Exit 2: "git-lock は -r / -f / -n / --allow-nested-git / PATH と併用できません"

# A subdirectory is not searched upward
safe-rm git-lock --repo src --assume-quiescent
# Exit 2: "リポジトリとして開けません"

# The lock was modified less than min_age_secs (default 300) seconds ago
safe-rm git-lock --repo . --assume-quiescent
# Exit 2: "index.lock の最終更新から 42 秒しかたっていません（必要なのは 300 秒）"

# The lock has content (it may hold an index that git has not finished writing)
safe-rm git-lock --repo . --assume-quiescent
# Exit 2: "index.lock が空ではありません（1024 バイト）"

# A process has the lock open
safe-rm git-lock --repo . --assume-quiescent
# Exit 2: "index.lock を開いているプロセスがあります（PID: 12345）"

# No index.lock in the gitdir
safe-rm git-lock --repo . --assume-quiescent
# Exit 1: "cannot remove '/path/to/repo/.git/index.lock': No such file or directory"
```
