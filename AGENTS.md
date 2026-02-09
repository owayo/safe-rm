# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

safe-rm は AI エージェント（Claude Code等）向けの安全なファイル削除プロキシ。Git状態を確認し、未コミットのファイル削除をブロックする Rust CLI ツール。

## Commands

```bash
make build              # デバッグビルド
make release            # リリースビルド
make install            # /usr/local/bin にインストール
make test               # 全テスト実行
make test-unit          # ユニットテストのみ (cargo test --lib)
make test-integration   # 統合テストのみ (cargo test --test integration_test)
make fmt                # コードフォーマット
make check              # clippy (-D warnings) + cargo check

# 単一テスト実行
cargo test <test_name>
cargo test --test integration_test <test_name>
```

## Architecture

### 実行フロー

```
CLI引数パース → Config読込 → Git repo検出 → [Git status一括取得] → パス毎に処理 → 削除/ブロック
```

### モジュール構成

| モジュール | 責務 |
|---|---|
| `main.rs` | エントリポイント。削除フロー全体のオーケストレーション、複数パスのバッチ処理 |
| `cli.rs` | clap derive による引数定義 (`-r`, `-f`, `-n`, `init` サブコマンド) |
| `config.rs` | `~/.config/safe-rm/config.toml` の読込。`allowed_paths` と `allow_project_deletion` の管理 |
| `error.rs` | `SafeRmError` enum（終了コード: 0=成功, 1=操作エラー, 2=セキュリティブロック）、`FileStatus` enum |
| `path_checker.rs` | パス正規化、プロジェクトルート内包含検証、シンボリックリンク解決、ディレクトリトラバーサル防止 |
| `git_checker.rs` | Git リポジトリ検出、ファイルステータス判定 (Clean/Modified/Staged/Untracked/Ignored/NotInRepo) |
| `init.rs` | `safe-rm init` によるデフォルト設定ファイル生成 |

### セキュリティモデル

1. **パス包含検証** (常時有効): プロジェクトルート外への削除をブロック
2. **Git保護** (`allow_project_deletion = false` 時): Modified/Staged/Untracked ファイルの削除をブロック
3. **allowed_paths**: 設定ファイルで指定したパスは全チェックをバイパス
4. **Fail-Closed**: ディレクトリ読取エラー時は削除をブロック（無視しない）

### パフォーマンス最適化

- `allow_project_deletion = false` 時のみ Git status を一括事前取得（バッチ最適化）
- Config の `allowed_paths` はロード時にパスを事前解決（canonicalize）
- `symlink_metadata()` で1回のsyscallで存在確認とメタ情報取得を統合

### テスト構成

- **ユニットテスト**: 各モジュール内の `#[cfg(test)]` ブロック（パス検証、Git状態、Config解析等）
- **統合テスト**: `tests/integration_test.rs` - 実際のGitリポジトリを tempfile で作成してE2Eテスト

### バージョン体系

YY.M.NNN 形式（例: 26.2.100）。リリースは GitHub Actions の workflow_dispatch で実行。

## Dependencies

| Crate | 用途 |
|---|---|
| `clap` | CLI引数パース (derive) |
| `shlex` | シェルコマンドトークン化 |
| `path-clean` | パス正規化 |
| `git2` | Git操作 (vendored) |
| `anyhow` | エラーハンドリング |
| `serde` + `toml` | 設定ファイルパース |
| `dirs` | ホームディレクトリ検出 |

### Dev Dependencies

| Crate | 用途 |
|---|---|
| `assert_cmd` | CLIテストヘルパー |
| `predicates` | テストアサーション |
| `tempfile` | テスト用一時ディレクトリ |
