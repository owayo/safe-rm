# AGENTS.md

このファイルは AI エージェント（Claude Code 等）がこのリポジトリで作業する際のガイダンスを提供する。

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
| `error.rs` | `SafeRmError` enum（終了コード: 0=成功, 1=操作エラー, 2=セキュリティブロック）、`FileStatus` enum（`is_deletable()` メソッド付き） |
| `path_checker.rs` | パス正規化、プロジェクトルート内包含検証、シンボリックリンク解決、非存在パスでも既存親を canonicalize して別名パス差異を吸収、ディレクトリトラバーサル防止 |
| `git_checker.rs` | Git リポジトリ検出、ファイルステータス判定 (Clean/Modified/Staged/Untracked/Ignored/NotInRepo)、ディレクトリ再帰チェック（symlink非追従）。ワークディレクトリは構造体に canonicalize 済みでキャッシュし、`to_workdir_relative()` で canonical/未解決両方のパスに対応。`status_file()` が未追跡ディレクトリを畳み込むケースでは、再帰付き status 一覧で再確認してネストした未追跡ファイルを取りこぼさない。Git API エラー時は fail-closed で削除をブロック（`get_all_statuses` は `Result` を返し、`resolve_status_from_relative_path` は予期しないエラーで `Modified` を返し、`lookup_status_in_listing` も `statuses()` 失敗時に `Modified` を返す） |
| `init.rs` | `safe-rm init` によるデフォルト設定ファイル生成 |

### セキュリティモデル

1. **パス包含検証** (常時有効): プロジェクトルート外への削除をブロック
2. **Git保護** (`allow_project_deletion = false` 時): Modified/Staged/Untracked ファイルの削除をブロック
3. **allowed_paths**: 設定ファイルで指定したパスは全チェックをバイパス
4. **Fail-Closed**: ディレクトリ読取エラーおよび Git API エラー時は削除をブロック（無視しない）
5. **Symlink安全性**: Gitチェック時のディレクトリ判定は `symlink_metadata()` ベースで、ディレクトリsymlinkを辿らずリンク自体を評価
6. **エイリアスパス耐性**: パス包含検証と `allowed_paths` 判定では、非存在パスでも既存親ディレクトリまで canonicalize して未作成部分を再結合し、repo symlink 別名や `/var` と `/private/var` 差異を吸収。Gitチェックでは非symlinkパスを canonicalize して比較し、symlink パスは「親ディレクトリのみ canonicalize + リンク名維持」で照合することでバイパスを防止（repo symlink 別名を cwd にした場合も含む）

### パフォーマンス最適化

- `allow_project_deletion = false` 時のみ Git status を一括事前取得（バッチ最適化）
- `status_file()` の単体問い合わせで未追跡ディレクトリ配下を見落とすケースは、再帰付き status 一覧の再確認で補完
- Config の `allowed_paths` はロード時に既存親までパスを事前解決（canonicalize）
- `symlink_metadata()` で1回のsyscallで存在確認とメタ情報取得を統合
- Git ワークディレクトリは `open()` 時に1回 canonicalize してキャッシュ（毎回の syscall を回避）

### テスト構成

- **ユニットテスト**: 各モジュール内の `#[cfg(test)]` ブロック（パス検証、Git状態、Config解析、symlink削除、I/Oエラー、Git API エラー時の fail-closed 検証等）
- **統合テスト**: `tests/integration_test.rs` - 実際のGitリポジトリを tempfile で作成してE2Eテスト。repo symlink 別名の cwd からの相対実行、単一失敗時の stderr 非重複、force フラグとダーティファイルの複合ケース、ネスト未追跡ディレクトリのブロック、ドライラン+フォース複合、空ディレクトリ処理、バッチセキュリティエラー優先、設定の複合テスト（strict mode + allowed_paths、複数 allowed_paths エントリ）、strict mode + force フラグの複合テスト、相対パスの `..` コンポーネント検証、バッチ全ダーティの終了コード検証、allowed_paths ディレクトリ自体の削除挙動検証、2パスバッチの終了コード優先度検証、symlink-to-directory の非再帰削除、Git index 破損時の fail-closed 検証、3パスバッチの終了コード優先度検証、ドライランのファイルシステム非変更保証、設定ファイルのエッジケース（空 allowed_paths 配列、存在しない allowed_paths ディレクトリ）も含めて検証

### バージョン体系

YY.M.NNN 形式（例: 26.2.100）。リリースは GitHub Actions の workflow_dispatch で実行。

## Dependencies

| Crate | 用途 |
|---|---|
| `clap` | CLI引数パース (derive) |
| `path-clean` | パス正規化 |
| `git2` | Git操作 (vendored) |
| `serde` + `toml` | 設定ファイルパース |
| `dirs` | ホームディレクトリ検出 |

### Dev Dependencies

| Crate | 用途 |
|---|---|
| `assert_cmd` | CLIテストヘルパー |
| `predicates` | テストアサーション |
| `tempfile` | テスト用一時ディレクトリ |
