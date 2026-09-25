<h1 align="center">safe-rm</h1>

<p align="center">
  Secure file deletion CLI for AI agents with Git-aware protection
</p>

<!-- standard:badges:start -->
<h3 align="center">Supported Platforms</h3>

<p align="center">
  <img src="https://img.shields.io/badge/Linux-FCC624?logo=linux&amp;logoColor=black" alt="Linux">
  <img src="https://img.shields.io/badge/macOS-000000?logo=apple&amp;logoColor=white" alt="macOS">
</p>

<p align="center">
  <a href="https://github.com/owayo/safe-rm/actions/workflows/ci.yml"><img src="https://github.com/owayo/safe-rm/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/owayo/safe-rm/releases/latest"><img src="https://img.shields.io/github/v/release/owayo/safe-rm" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/owayo/safe-rm" alt="License"></a>
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>
<!-- standard:badges:end -->

---

`safe-rm` is a CLI tool that prevents AI agents from accidentally deleting files outside the project or critical Git metadata. By default it enforces project containment and blocks Git administrative paths such as `.git`. In **strict mode** (`allow_project_deletion = false`), it also enforces Git-aware access control and blocks deletion of modified, staged, or untracked files.

It is meant to stand in for `rm` in AI agents such as Claude Code: a `PreToolUse` hook rejects `rm` and `rmdir` in Bash tool calls and tells the agent to use `safe-rm` instead (see [Claude Code Integration](#claude-code-integration)).

## Features

- **Path Containment**: Block deletion of files outside project directory
- **Strict-Mode Git Status Protection**: When `allow_project_deletion = false`, prevent deletion of modified, staged, or untracked files. The check uses the Git repository that actually owns each target — both the cwd-side checker and a discovery from the target itself are evaluated, and the deepest workdir that contains the target is selected. This blocks bypasses where (a) cwd is non-Git but the target lives inside a nested Git repository, (b) the cwd repository ignores a nested repository (e.g. via `.gitignore`) so its files would otherwise be treated as `Ignored`, and (c) cwd and the target belong to different Git repositories. If the owning repository redirects its working tree elsewhere (e.g. via `core.worktree`) so that its `workdir()` does not contain the target, the target is treated as `Modified` and blocked fail-closed instead of silently falling through to `NotInRepo`
- **Git Metadata Protection**: Always block `.git`, gitdir indirection files, bare-repository administrative paths, recursive deletes that would include current-repository Git metadata, and nested Git metadata found in the deletion target, any intermediate path component, or its subtree. This includes nested `.git` files/directories and bare repositories that have no `.git` component. Component matching is ASCII case-insensitive to also block `.GIT` bypass attempts on case-insensitive filesystems (e.g., macOS APFS). Intermediate symlinks that resolve to a `.git` directory or bare repository (for example `gitlink -> nested/.git` followed by deleting `gitlink/config`) are also blocked by canonicalizing only the parent directory while keeping the trailing component (so deleting the symlink itself is still permitted because it removes only the link)
- **Conflict-Aware Status Mapping**: Files with the `Status::CONFLICTED` flag are treated as `Modified` in strict mode, even when no other index/worktree flags are set, to prevent silent deletion of unresolved merge conflicts
- **Nested Dirty-File Protection**: Strict-mode checks catch files inside untracked directories, mixed ignored/untracked directories, and tracked modifications inside ignored directories instead of treating them as outside Git
- **Directory Traversal Prevention**: Block `../` escape attempts whenever the component being collapsed by `..` is not a real directory. Patterns such as `link/../victim` (symlink intermediate), `missing/../victim` (non-existent intermediate), and `file/../victim` (regular-file intermediate) all fail under the OS path resolver but would otherwise be silently normalized to `victim` by lexical path cleaning; safe-rm rejects them fail-closed
- **Dangling Intermediate Symlink Guard**: Block paths whose intermediate component is an unresolvable symlink (for example `dangling/child.txt`) before metadata lookup. A final dangling symlink itself remains deletable because removing it only removes the link entry
- **`.` / `..` Operand Rejection**: Reject operands whose trailing component is `.` or `..` (`.`, `./`, `..`, `../`, `sub/.`, `sub/..`, `/abs/path/..`), matching what POSIX `rm` does — it writes a diagnostic and does nothing with such an operand. Without this guard `safe-rm -r .` would delete the current directory itself and `safe-rm -r ..` its parent, making the proxy more destructive than the `rm` it replaces. The check runs before normalization, `allowed_paths` matching, and `-f`, so neither an allowed path nor force can bypass it. Ordinary dot-prefixed names such as `.hidden`, `...`, and `..foo` remain deletable. On Windows, drive-relative forms (`C:.`, `C:..`) are rejected as well
- **Empty-Operand Rejection**: An empty operand (`safe-rm ""`) resolves to the current directory because `cwd.join("")` equals `cwd`, so `-r ""` would delete the working directory itself. It is rejected up front as `No such file or directory` (exit 1), matching `rm`, and silently ignored under `-f`. Like `rm -f`, `safe-rm -f` with no operand at all succeeds without touching the config or the current directory
- **Root-Operand Rejection**: Reject operands that resolve to the root directory (`/`, `//`, or `.` when the working directory is `/`) with exit code 2, matching POSIX `rm` and GNU `rm`'s default `--preserve-root`. Without this guard, a working directory of `/` in a non-Git environment (such as a container running as root) makes the project root `/`, so containment passes and Git metadata protection does not cover `/` itself, letting `safe-rm -r /` descend into a full filesystem wipe. Like the `.` / `..` check, it runs before `allowed_paths` matching and `-f`
- **Trailing-Separator Operand Validation**: A trailing separator is a POSIX request that the operand be a directory, so `rm` refuses operands that do not resolve as one. Because lexical normalization strips it, `file.txt/` would otherwise delete `file.txt` itself and `danglink/` (a broken symlink) would delete the link entry. safe-rm resolves the raw operand first: non-directories return `Not a directory` and unresolvable paths return `No such file or directory` (both exit 1, silently ignored under `-f` exactly as `rm` does), while `ELOOP` and permission failures propagate as I/O errors even under `-f`. Symlinks to directories still resolve as directories, so `link/` keeps deleting only the link entry and leaves the target intact
- **Ignored File Passthrough**: Allow deletion of `.gitignore`d files (build artifacts, etc.). A file that matches a `.gitignore` pattern but is force-added (`git add -f`) and therefore tracked is still protected when it has uncommitted changes — status is resolved before the ignore check, so a tracked, dirty file is never misclassified as `Ignored`
- **Symlink-Safe Git Checks**: Directory symlinks are checked as links themselves (not traversed)
- **Non-UTF-8 Path Safety**: Git status cache is keyed by raw byte paths (`HashMap<Vec<u8>, FileStatus>`) using `entry.path_bytes()` so non-UTF-8 filenames are still registered. Without this, deletions of non-UTF-8 untracked files inside untracked directories could silently fall through to `NotInRepo` and become deletable in strict mode. `status_should_ignore()` errors are also fail-closed (treated as `Modified`) instead of being swallowed
- **Alias-Path Safety (Containment + allowed_paths + Strict Mode)**: Containment checks and `allowed_paths` matching canonicalize up to the nearest existing parent and re-append missing segments, while strict-mode Git checks canonicalize non-symlink paths and canonicalize only symlink parents (checking the link itself), blocking bypasses via alternate absolute aliases
- **Configurable Allowed Paths**: Bypass project containment and Git status checks for specified directories (per-directory recursive control), without requiring current-directory Git discovery to succeed
- **Non-Git Support**: Works safely in non-Git directories
- **Dry Run Mode**: Preview what would be deleted without actually deleting
- **Deterministic Error Output**: Single-path failures emit one stderr block, while batch runs emit one error per failed path without duplicating the same message

How the checks fit together (check order, safety layers, and what each mode can delete): [docs/architecture.md](docs/architecture.md)

## Installation

<!-- standard:install:start -->
### Cargo

Requires Rust 1.98 or later.

```bash
cargo install --git https://github.com/owayo/safe-rm --locked
```

### From GitHub Releases

Download the archive for your platform from [Releases](https://github.com/owayo/safe-rm/releases/latest), extract it, and put `safe-rm` on your `PATH`. Each release also includes `SHA256SUMS` for checking the downloads.

| Platform | Archive |
|---|---|
| Linux (x86_64) | `safe-rm-x86_64-unknown-linux-gnu.tar.gz` |
| macOS (Intel) | `safe-rm-x86_64-apple-darwin.tar.gz` |
| macOS (Apple Silicon) | `safe-rm-aarch64-apple-darwin.tar.gz` |

On macOS, if you downloaded the archive with a browser, remove the quarantine attribute before running it: `xattr -d com.apple.quarantine safe-rm`.

### From Source

Requires [mise](https://mise.jdx.dev/) (the Rust toolchain is pinned in `mise.toml`).

```bash
git clone https://github.com/owayo/safe-rm.git
cd safe-rm
make install
```

`make install` installs to `/usr/local/bin`. Set `INSTALL_PATH` to change it (for example `make install INSTALL_PATH="$HOME/.local/bin"`).
<!-- standard:install:end -->

Then add the hook and the `CLAUDE.md` rules from [Claude Code Integration](#claude-code-integration). `safe-rm init` creates the optional configuration file (see [Configuration](#configuration)).

## Usage

```bash
# Delete a single file
safe-rm file.txt

# Delete a directory recursively
safe-rm -r directory/

# Delete multiple files
safe-rm file1.txt file2.txt file3.txt

# Dry run (show what would be deleted)
safe-rm -n file.txt

# Force (ignore nonexistent files)
safe-rm -f nonexistent.txt

# Combine flags
safe-rm -rf build/
```

A blocked deletion exits with code 2 and prints the reason on stderr:

```bash
# Outside project
safe-rm /etc/passwd
# Exit 2: "プロジェクト外へのアクセスは禁止されています"

# `.` / `..` operands (POSIX rm refuses these too)
safe-rm -r .
# Exit 2: "末尾が '.' または '..' のパスは削除できません"
```

More allowed and blocked operations, including the strict-mode cases: [docs/usage.md](docs/usage.md)

All options, the `init` subcommand, and exit codes: [docs/cli-reference.md](docs/cli-reference.md)

## Configuration

`safe-rm` supports an optional configuration file at `~/.config/safe-rm/config.toml`. You can also specify a custom config path via the `SAFE_RM_CONFIG` environment variable. On Unix, the environment variable is read as a raw OS path so non-UTF-8 paths are preserved.

### Setup

```bash
# Generate a default config file
safe-rm init
# → Creates ~/.config/safe-rm/config.toml
# Defaults allow ~/.claude/skills and /tmp recursively
```

`safe-rm init` does not overwrite an existing config path entry. Existing files and existing symlinks, including dangling symlinks, are treated as already present, and the file is created with create-new semantics so a race cannot redirect the template into another path.

### Config File Format

```toml
# Allow deletion of any file within the current project without Git status checks.
# Containment check (cannot delete outside project) is still enforced.
# Default: true
allow_project_deletion = true

# Recursively allow all files/subdirectories under this path
# Tilde (~) is expanded to home directory
[[allowed_paths]]
path = "~/.claude/skills"
recursive = true

# Recursively allow files/subdirectories under /tmp
[[allowed_paths]]
path = "/tmp"
recursive = true

# Only allow direct children of this directory
# [[allowed_paths]]
# path = "/tmp/logs"
# recursive = false
```

### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `allow_project_deletion` | bool | `true` | If `true`, allow deletion of any file within the current project without Git status checks. Containment check is still enforced. |
| `path` | string | (required) | Directory path where deletion is permitted |
| `recursive` | bool | `false` | If `true`, all nested files/subdirectories are allowed. If `false`, only direct children. |

Unknown keys at the top level or inside `allowed_paths` are rejected. This makes misspelled security settings a parse error, which triggers the fail-closed strict-mode fallback instead of silently applying a permissive default. An older `safe-rm` reading a config written for a newer version therefore becomes more restrictive, not less.

How the settings behave (default and strict mode, `allowed_paths` and `recursive`, missing or broken config files) and an example: [docs/configuration.md](docs/configuration.md)

## Claude Code Integration

Configure Claude Code hooks to redirect `rm`/`rmdir` commands to `safe-rm`.

### Hook Configuration

Add to your Claude Code settings (e.g., `~/.claude/settings.json` or `.claude/settings.json`):

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "bash_command=\"$(jq -er '.tool_input.command | strings')\" || { echo '🚫 Could not inspect Bash command; blocking fail-closed.' >&2; exit 2; }; if printf '%s\\n' \"$bash_command\" | grep -qE '(^|[[:space:];|&(`])([^[:space:];|&()]+/)?rm(dir)?([[:space:];|&()<>]|$)'; then echo '🚫 Use safe-rm instead: safe-rm <file> (validates Git status and path containment).' >&2; exit 2; fi; exit 0"
          }
        ]
      }
    ]
  }
}
```

This hook:
1. Conservatively detects literal `rm`/`rmdir` command tokens anywhere in Bash tool calls via stdin JSON, including `cd dir && rm`, `echo ok; rm`, `command rm`, absolute paths such as `/bin/rm`, and command substitutions such as `$(rm file)`
2. Blocks with exit code 2 and shows guidance message to Claude
3. Claude then uses `safe-rm <file>` directly for safe deletion

This hook intentionally fails closed and can also reject a quoted or printed `rm` token. It is not a complete shell parser: dynamically constructed command names can evade textual inspection. Treat it as defense in depth, keep Bash permissions restricted, and require the agent to use `safe-rm` for deletion.

### CLAUDE.md Instructions

Add to your `CLAUDE.md`:

```markdown
## File Deletion Rules

- Do NOT use `rm` or `rmdir`. These are restricted for safety.
- Use `safe-rm <file>` or `safe-rm -r <directory>` to delete files.
- `safe-rm` will automatically verify that the file is safe to delete (committed or ignored in Git).
- If `safe-rm` fails, the file likely has uncommitted changes or is outside the project.

### Examples
- Delete a build artifact: `safe-rm -r target/`
- Delete an old file: `safe-rm old_module.rs`
- Preview what would be deleted: `safe-rm -n file.txt`
```

## Development

<!-- standard:dev:start -->
Requires [mise](https://mise.jdx.dev/). Tool versions are pinned in `mise.toml`.

```bash
make setup   # Install the toolchain (mise) and dependencies
make ci      # Run the same checks as CI (no changes)
```

| Command | Description |
|---|---|
| `make setup` | Install the toolchain (mise) and dependencies |
| `make build` | Build a debug binary |
| `make release` | Build a release binary |
| `make test` | Run the tests |
| `make lint` | Run clippy with warnings as errors |
| `make fmt` | Format the code (rewrites files) |
| `make fmt-check` | Check the formatting (no changes) |
| `make check` | Run fmt-check and lint (no changes) |
| `make ci` | Run the same checks as CI (no changes) |
| `make install` | Install the release binary to INSTALL_PATH (default /usr/local/bin) |
| `make uninstall` | Remove the binary from INSTALL_PATH |
| `make clean` | Remove build artifacts |

Run `make` to list every target. Releases are published from GitHub Actions (**Actions → Release → Run workflow**).
<!-- standard:dev:end -->

What CI checks, how a release is made, and what the tests cover: [docs/development.md](docs/development.md)

## Security

If you discover a security vulnerability, please report it via [GitHub Issues](https://github.com/owayo/safe-rm/issues).

## License

<!-- standard:license:start -->
[MIT](LICENSE)
<!-- standard:license:end -->
