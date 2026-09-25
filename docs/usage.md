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
```
