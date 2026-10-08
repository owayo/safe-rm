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

## Exit Codes

| Code | Meaning | Examples |
|------|---------|----------|
| 0 | Success | File deleted, dry-run completed |
| 1 | Operation error | File not found, is directory without -r, trailing separator on a non-directory, I/O error, partial failure |
| 2 | Security block | Dirty file, outside project, `.` / `..` or root operand, directory read error (fail-closed) |
