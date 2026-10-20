# CLI Reference

Command-line options, subcommands, and exit codes of `safe-rm`. The configuration file is covered in the [README](../README.md#configuration) and in [configuration.md](configuration.md).

## Options

| Option | Description |
|--------|-------------|
| `-r, --recursive` | Delete directories and their contents |
| `-f, --force` | Ignore nonexistent files (no error). Also allows running with no operand at all, like `rm -f` |
| `-n, --dry-run` | Show what would be deleted without deleting |
| `--allow-nested-git GIT_PATH` | With `-r`, explicitly allow one Git metadata directory found inside a single ignored target directory. Repeat for every reported path. Gitdir files, direct metadata targets, and the current repository's metadata remain protected |
| `-h, --help` | Show help message |
| `-V, --version` | Show version |

## Subcommands

| Subcommand | Description |
|------------|-------------|
| `init` | Generate config file at `~/.config/safe-rm/config.toml` |
| `git-lock` | Remove an empty `index.lock` that a crashed git process left in a gitdir, under an explicit quiescence assertion and additional checks. See [git-lock](#git-lock) |

`init` or `git-lock` given before any PATH is read as the subcommand. To delete a file with one of these names, write it as a path, for example `safe-rm ./git-lock`.

## git-lock

```bash
safe-rm git-lock --repo PATH --assume-quiescent [-n | --dry-run]
```

Normal deletion never removes Git metadata. `git-lock` is the only way to remove an `index.lock`, and it removes only an empty one that passes every check. The checks, and what they cannot see, are described in [architecture.md](architecture.md#the-git-lock-subcommand).

| Option | Description |
|--------|-------------|
| `--repo PATH` | Required. The worktree root or the gitdir: `.git`, a linked worktree's `.git/worktrees/<id>`, or a submodule's `.git/modules/<name>`. Parent directories are not searched, and environment variables such as `GIT_DIR` are not used. A relative path is resolved from the current directory |
| `--assume-quiescent` | Required on every run. States that no git process is running in that repository and that, until the removal finishes, neither a new git process nor any other change to the gitdir will start. safe-rm cannot verify this, and the config file cannot supply it |
| `-n, --dry-run` | Run every check without deleting. Give it after `git-lock` |

The top-level options (`-r`, `-f`, `-n`, `--allow-nested-git`) and PATH operands cannot be combined with `git-lock`; such a command is refused with exit code 2. `git-lock` is also refused while `GIT_INDEX_FILE` is set, even to an empty string, because the `index.lock` in the gitdir is then not necessarily the lock of the index in use. It runs on Linux and macOS only.

On success, `git-lock` prints the lock path, which is the canonical gitdir followed by `index.lock`:

```text
removed: /path/to/repo/.git/index.lock (git index lock; quiescence asserted)
```

With `-n`, the line starts with `would remove:` instead. On macOS, if the check for processes holding the lock open had to skip something, one more line goes to stderr. Only the parts that apply are printed:

```text
safe-rm: note: open-handle inspection did not cover N OS platform process(es) that deny inspection (routined, lsd, ...) and M file descriptor(s) whose vnode could not be read; --assume-quiescent remains the guarantee
```

`[git_locks]` in the config file can turn `git-lock` off or change how long the lock must stay unmodified (default 300 seconds). See [configuration.md](configuration.md#git_locks).

## Exit Codes

| Code | Meaning | Examples |
|------|---------|----------|
| 0 | Success | File deleted, dry-run completed, `git-lock` removed the lock or its dry run passed every check |
| 1 | Operation error | File not found, is directory without -r, trailing separator on a non-directory, I/O error, partial failure, no `index.lock` for `git-lock`, `git-lock` removed a name that turned out not to be the inspected file |
| 2 | Security block | Dirty file, outside project, `.` / `..` or root operand, directory read error (fail-closed), Git metadata such as `.git/index.lock` given to normal deletion, `git-lock` refused (no `--assume-quiescent`, turned off in the config, lock not empty, too recent, or open in a process) |
