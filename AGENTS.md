# AGENTS.md

このファイルは AI エージェント（Claude Code 等）がこのリポジトリで作業する際のガイダンスを提供する。

## Project Overview

safe-rm は AI エージェント（Claude Code等）向けの安全なファイル削除プロキシ。常時プロジェクト境界と Git 管理メタデータ（`.git` 等）を保護し、`allow_project_deletion = false` の厳格モードでは未コミットのファイル削除もブロックする Rust CLI ツール。

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
CLI引数パース → Config読込 → Git repo検出 → [Git status一括取得] → [Git管理メタデータ保護] → パス毎に処理 → 削除/ブロック
```

### モジュール構成

| モジュール | 責務 |
|---|---|
| `main.rs` | エントリポイント。削除フロー全体のオーケストレーション、複数パスのバッチ処理 |
| `cli.rs` | clap derive による引数定義 (`-r`, `-f`, `-n`, `init` サブコマンド) |
| `config.rs` | `~/.config/safe-rm/config.toml` の読込。`allowed_paths` と `allow_project_deletion` の管理 |
| `error.rs` | `SafeRmError` enum（終了コード: 0=成功, 1=操作エラー, 2=セキュリティブロック）、`FileStatus` enum（`is_deletable()` メソッド付き）。`PartialFailure` には `dry_run` フラグがあり、ドライラン時は「would be removed」と表示する |
| `path_checker.rs` | パス正規化、プロジェクトルート内包含検証、シンボリックリンク解決、非存在パスでも既存親を canonicalize して別名パス差異を吸収、ディレクトリトラバーサル防止 |
| `git_checker.rs` | Git リポジトリ検出、ファイルステータス判定 (Clean/Modified/Staged/Untracked/Ignored/NotInRepo)、ディレクトリ再帰チェック（symlink非追従）、Git 管理メタデータを含む再帰削除の検出。`path_targets_or_contains_git_metadata()` で削除対象自体および再帰削除時の配下に含まれる任意階層の `.git` を検出し、ネストしたリポジトリのメタデータも保護する。ワークディレクトリは構造体に canonicalize 済みでキャッシュし、`to_workdir_relative()` で canonical/未解決両方のパスに対応。`status_file()` が未追跡ディレクトリを畳み込むケースでは、再帰付き status 一覧で再確認してネストした未追跡ファイルを取りこぼさない。ディレクトリの ignored 早期許可は「ディレクトリ自体が ignored」の場合だけで、配下の ignored ファイルは未追跡・変更済みの兄弟ファイルを隠さない。Git API エラー時は fail-closed で削除をブロック（`get_all_statuses` は `Result` を返し、`resolve_status_from_relative_path` は予期しないエラーで `Modified` を返し、`lookup_status_in_listing` も `statuses()` 失敗時に `Modified` を返す） |
| `init.rs` | `safe-rm init` によるデフォルト設定ファイル生成 |

### セキュリティモデル

1. **Git管理メタデータ保護** (常時有効): `.git`、gitdir 参照ファイル、bare リポジトリ管理パス、現在のリポジトリの Git 管理メタデータを含む再帰削除に加え、削除対象自身および再帰削除時に配下に存在する任意階層の `.git` ファイル/ディレクトリ（ネストしたリポジトリ含む）をブロック
2. **パス包含検証** (常時有効): プロジェクトルート外への削除をブロック
3. **Git保護** (`allow_project_deletion = false` 時): Modified/Staged/Untracked ファイルの削除をブロック
4. **allowed_paths**: 設定ファイルで指定したパスは包含チェックと Git ステータスチェックをバイパスするが、Git管理メタデータ保護はバイパスしない
5. **Fail-Closed**: ディレクトリ読取エラーおよび Git API エラー時は削除をブロック（無視しない）
6. **Ignored混在ディレクトリ保護**: 厳格モードではディレクトリ自体が ignored の場合だけ早期許可し、配下の ignored ファイルだけを理由にディレクトリ全体を許可しない
7. **Symlink安全性**: Gitチェック時のディレクトリ判定および任意階層 `.git` 検出は `symlink_metadata()` ベースで、ディレクトリsymlinkを辿らずリンク自体を評価
8. **シンボリックリンク経由の `..` 脱出防止**: メタデータ取得・削除・許可判定はすべて `path_clean` で字句的に `..` を解決した正規化パス（`normalized_path`）で行い、`link/../victim` のように OS の path resolution を悪用したプロジェクト境界の脱出をブロック
9. **エイリアスパス耐性**: パス包含検証と `allowed_paths` 判定では、非存在パスでも既存親ディレクトリまで canonicalize して未作成部分を再結合し、repo symlink 別名や `/var` と `/private/var` 差異を吸収。Git管理メタデータ保護と Gitチェックでは非symlinkパスを canonicalize して比較し、symlink パスは「親ディレクトリのみ canonicalize + リンク名維持」で照合することでバイパスを防止（repo symlink 別名を cwd にした場合も含む）

### パフォーマンス最適化

- `allow_project_deletion = false` 時のみ Git status を一括事前取得（バッチ最適化）
- `status_file()` の単体問い合わせで未追跡ディレクトリ配下を見落とすケースは、再帰付き status 一覧の再確認で補完
- Config の `allowed_paths` はロード時に既存親までパスを事前解決（canonicalize）
- `symlink_metadata()` で1回のsyscallで存在確認とメタ情報取得を統合
- Git ワークディレクトリは `open()` 時に1回 canonicalize してキャッシュ（毎回の syscall を回避）

### テスト構成

- **ユニットテスト**: 各モジュール内の `#[cfg(test)]` ブロック（パス検証、Git状態、Config解析、`SAFE_RM_CONFIG` の非 UTF-8 パス対応、symlink削除、I/Oエラー、Git API エラー時の fail-closed 検証、Git管理メタデータ判定、Git管理メタデータを含む再帰削除の判定、`path_targets_or_contains_git_metadata` による任意階層 `.git` 検出（ファイル/ディレクトリ自体・配下・深いネスト・symlink 非追従・存在しないパスを含む）、空リポジトリ対応、壊れた symlink 検出、複数ステータスの一括取得検証、キャッシュ使用時の ignored サブディレクトリチェック、ignored/未追跡混在ディレクトリのブロック、`PartialFailure { dry_run: true }` のドライラン文言検証等）
- **統合テスト**: `tests/integration_test.rs` - 実際のGitリポジトリを tempfile で作成してE2Eテスト。repo symlink 別名の cwd からの相対実行、単一失敗時の stderr 非重複、force フラグとダーティファイルの複合ケース、ネスト未追跡ディレクトリのブロック、ignored/未追跡混在ディレクトリのブロック、ドライラン+フォース複合、空ディレクトリ処理、バッチセキュリティエラー優先、設定の複合テスト（strict mode + allowed_paths、複数 allowed_paths エントリ）、strict mode + force フラグの複合テスト、相対パスの `..` コンポーネント検証、バッチ全ダーティの終了コード検証、allowed_paths ディレクトリ自体の削除挙動検証、allowed_paths でも現在のリポジトリの Git 管理メタデータ削除をバイパスできないこと、2パスバッチの終了コード優先度検証、symlink-to-directory の非再帰削除、Git index 破損時の fail-closed 検証、Git管理メタデータの常時ブロック、Git管理メタデータを含むリポジトリルート再帰削除のブロック、3パスバッチの終了コード優先度検証、ドライランのファイルシステム非変更保証、設定ファイルのエッジケース（空 allowed_paths 配列、存在しない allowed_paths ディレクトリ）、壊れた symlink のデフォルト/厳格モード対応、空リポジトリ厳格モード、バッチ force フラグ複合、allowed_paths ドライラン注釈、`link/../victim` 形式の symlink 経由 `..` 脱出ブロック（`-f` 併用も含む）、非 Git 親直下リポジトリの `.git` 直接削除ブロック、ネストリポジトリ全体の再帰削除と内部 `.git` 直接削除のブロック、ドライラン部分失敗の「would be removed」表記検証も含めて検証

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
