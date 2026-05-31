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
CLI引数パース → Config読込 → パス毎に処理 → allowed_paths判定 → [必要時 Git repo検出] → [Git管理メタデータ保護] → [必要時 Git status一括取得] → 削除/ブロック
```

### モジュール構成

| モジュール | 責務 |
|---|---|
| `main.rs` | エントリポイント。削除フロー全体のオーケストレーション、複数パスのバッチ処理。`GitContext` で Git リポジトリ検出を allowed_paths 以外の安全境界が必要になるまで遅延し、壊れた cwd の `.git` が許可パス削除を巻き込まないようにする |
| `cli.rs` | clap derive による引数定義 (`-r`, `-f`, `-n`, `init` サブコマンド) |
| `config.rs` | `~/.config/safe-rm/config.toml` の読込。`allowed_paths` と `allow_project_deletion` の管理。`Path::exists()` で fail-open に倒れないよう `read_to_string()` の結果で分岐する。`ErrorKind::NotFound` のみ permissive default を返し、その他の I/O エラー（権限不足等）と TOML パース失敗は fail-closed で `allow_project_deletion = false`（strict モード）にフォールバックする（`fail_closed_default()`）。利用者が意図した strict 設定が壊れた構文や stat 失敗だけで無効化されない |
| `error.rs` | `SafeRmError` enum（終了コード: 0=成功, 1=操作エラー, 2=セキュリティブロック）、`FileStatus` enum（`is_deletable()` メソッド付き）。`PartialFailure` には `dry_run` フラグがあり、ドライラン時は「would be removed」と表示する |
| `path_checker.rs` | パス正規化、プロジェクトルート内包含検証、シンボリックリンク解決、非存在パスでも既存親を canonicalize して別名パス差異を吸収、ディレクトリトラバーサル防止。`reject_symlink_parent_traversal()` は `..` の直前成分が「実体として存在する通常ディレクトリ」であることを必須とし、symlink/通常ファイル/特殊ファイル/存在しない/メタデータ取得失敗の場合は `UnsafeTraversal` で fail-closed に拒否する（`link/../victim`・`missing/../victim`・`file/../victim` の正規化すり替え対策） |
| `git_checker.rs` | Git リポジトリ検出、ファイルステータス判定 (Clean/Modified/Staged/Untracked/Ignored/NotInRepo)、ディレクトリ再帰チェック（symlink非追従）、Git 管理メタデータを含む再帰削除の検出。`GitChecker::open()` は `Result<Option<Self>, SafeRmError>` を返し、Git API エラー（壊れた `.git`、権限不足、I/O エラー等）は fail-closed で `Err` を伝播する。`Repository::discover()` が `NotFound` を返した場合でも `has_git_metadata_ancestor()` で `path` の祖先に `.git` 痕跡が残っていないか確認し、痕跡があれば「壊れたリポジトリ」とみなして `Err` に倒す。祖先確認そのものが権限/I/O エラーで信頼できない場合も fail-closed とし、`.git` 痕跡がなく確認にも成功した場合のみ非 Git 環境として `Ok(None)` を返す。`path_targets_or_contains_git_metadata()` / `try_path_targets_or_contains_git_metadata()` で削除対象自体・任意の中間コンポーネント・再帰削除時の配下に含まれる任意階層の Git 管理メタデータを検出し、ネストしたリポジトリのメタデータも保護する。`.git` ファイル/ディレクトリに加え、`.git` コンポーネントを持たない bare リポジトリのルートや配下の管理ファイルも検出する。中間 symlink 経由のバイパスを防ぐため、親ディレクトリのみ canonicalize して末尾コンポーネントは保持する `canonicalize_parent_keep_filename()` を併用する（symlink 自身の削除は許可しつつ、`gitlink/config` のように中間 symlink で `.git` を指すケースは確実にブロックする）。`touches_git_metadata_path()` と `is_git_metadata_path()` も同じ `canonicalize_parent_keep_filename()` を使い、`.git` を指す symlink 自身の削除（リンクのみが消え実体は残る）を許可する一方で、symlink 経由で `.git` 配下に到達するパスは引き続きブロックする。コンポーネント比較は ASCII case-insensitive で行い、macOS APFS のような大文字小文字を区別しないファイルシステムで `.GIT` 経由のバイパスも防ぐ。`convert_status()` は `Status::CONFLICTED` を Modified にマッピングして、CONFLICTED が単独で立つ稀なケースでも Clean に落として削除許可してしまう不具合を防ぐ。実削除前の検査ではディレクトリ読み取り・エントリ取得エラーを `DirectoryReadError` として返し、fail-closed でブロックする。ワークディレクトリは構造体に canonicalize 済みでキャッシュし、`to_workdir_relative()` で canonical/未解決両方のパスに対応。`status_file()` が未追跡ディレクトリを畳み込むケースでは、再帰付き status 一覧で再確認してネストした未追跡ファイルを取りこぼさない。ディレクトリ配下は常に再帰検査し、ignored ファイルは許可する一方で、ignored 親ディレクトリ配下の tracked 変更済み/ステージング済みファイルや未追跡ファイルはブロックする。Git API エラー時は fail-closed で削除をブロック（`get_all_statuses` は `Result` を返し、`resolve_status_from_relative_path` は予期しないエラーで `Modified` を返し、`lookup_status_in_listing` も `statuses()` 失敗時に `Modified` を返す）。ステータスキャッシュキーは `HashMap<Vec<u8>, FileStatus>` のバイト列で持ち、git2 0.21 で `Result<&str, git2::Error>` を返すようになった `StatusEntry::path()` の UTF-8 制限を避けて、`entry.path_bytes()` を直接キーに使う。`to_git_relative_key()` も `Vec<u8>` を返す（Unix では `OsStrExt::as_bytes()`、それ以外は `to_string_lossy()` フォールバック）。これにより非 UTF-8 ファイル名でもキャッシュに正しく登録され、`NotInRepo` に落ちて削除許可される fail-open 経路を塞ぐ。キャッシュミス時の `.gitignore` 判定も `ignored_status_from_relative_path()` で `Ok(true)→Ignored`/`Ok(false)→None`/`Err→Modified` に分岐し、`status_should_ignore()` の Git API エラーも fail-closed で削除をブロックする。`lookup_status_in_listing()` 末尾の `status_should_ignore()` 判定も同じく Err を `Modified` に倒す |
| `init.rs` | `safe-rm init` によるデフォルト設定ファイル生成（`~/.claude/skills` と `/tmp` を再帰許可） |

### セキュリティモデル

1. **Git管理メタデータ保護** (常時有効): `.git`、gitdir 参照ファイル、bare リポジトリ管理パス、現在のリポジトリの Git 管理メタデータを含む再帰削除に加え、削除対象自身・パスの任意の中間コンポーネント・再帰削除時に配下に存在する任意階層の Git 管理メタデータ（ネストした `.git` と bare リポジトリを含む）をブロック。コンポーネント比較は ASCII case-insensitive で、macOS APFS のような大文字小文字を区別しないファイルシステムでの `.GIT` 経由バイパスも防止
2. **パス包含検証** (常時有効): `allowed_paths` 以外では、再帰的な Git 管理メタデータ探索より先にプロジェクトルート外への削除をブロック
3. **Git保護** (`allow_project_deletion = false` 時): Modified/Staged/Untracked/コンフリクト中ファイルの削除をブロック（CONFLICTED フラグも Modified として扱う）
4. **allowed_paths**: 設定ファイルで指定したパスは包含チェックと Git ステータスチェックをバイパスするが、Git管理メタデータ保護はバイパスしない。現在のリポジトリ情報は可能なら使うが、cwd の `.git` が壊れていても許可パス削除は Git 検出失敗に巻き込まれない
5. **Fail-Closed**: ディレクトリ読取エラー、`allowed_paths` 外の通常経路で必要になる `Repository::discover()` 由来の Git API エラー（壊れた `.git`/権限不足/I/O 等。`NotFound` でも祖先に `.git` 痕跡が残っている、または祖先確認が権限/I/O エラーで完了できない場合は `has_git_metadata_ancestor()` が fail-closed に倒す）、設定ファイル読込/パースエラー時はすべて fail-closed。Git API エラーは `SafeRmError::GitError` として伝播し、設定ファイルが読み取れない/パースできない場合は `Config::fail_closed_default()`（`allow_project_deletion = false`）にフォールバックして利用者の strict 意図を守る。Config の存在判定は `Path::exists()` ではなく `read_to_string()` の `ErrorKind::NotFound` で行い、stat 失敗で fail-open に倒れない。`status_should_ignore()` の `Err` も `ignored_status_from_relative_path()` と `lookup_status_in_listing()` で `Modified` に倒し、Git API エラーを握りつぶして偶発的に削除許可されないようにする
6. **Ignored混在ディレクトリ保護**: 厳格モードでは ignored ディレクトリでも配下を再帰検査し、ignored ファイルは許可する一方で tracked 変更済み/ステージング済みファイルや未追跡ファイルをブロックする
7. **Symlink安全性**: Gitチェック時のディレクトリ判定および任意階層 `.git` 検出は `symlink_metadata()` ベースで、ディレクトリsymlinkを辿らずリンク自体を評価
8. **`..` 解決による別ファイルすり替え防止**: `..` の直前成分が「実体として存在する通常ディレクトリ」でないパス（symlink、通常ファイル、特殊ファイル、存在しない/読み取り不能な中間成分等）は削除前に拒否する。OS の path resolution と `path_clean` の字句正規化の差を利用して、`link/../victim`・`missing/../victim`・`file/../victim` のように本来到達できない別ファイルを削除する経路をブロックする。メタデータ取得・削除・許可判定は引き続き `path_clean` で字句的に `..` を解決した正規化パス（`normalized_path`）で行う
9. **エイリアスパス耐性**: パス包含検証と `allowed_paths` 判定では、非存在パスでも既存親ディレクトリまで canonicalize して未作成部分を再結合し、repo symlink 別名や `/var` と `/private/var` 差異を吸収。Git管理メタデータ保護と Gitチェックでは非symlinkパスを canonicalize して比較し、symlink パスは「親ディレクトリのみ canonicalize + リンク名維持」で照合することでバイパスを防止（repo symlink 別名を cwd にした場合も含む）
10. **中間 symlink 経由 `.git` バイパス防止**: `gitlink -> nested/.git` のような symlink を中間に挟んで `gitlink/config` を削除しようとしても、親ディレクトリを canonicalize して `.git` コンポーネントの存在を確認することで実体パスへの到達をブロックする。bare リポジトリへの中間 symlink 経由削除も同様に防止。symlink 自身の削除はリンクのみが消えて実体は残るため、リンク先が `.git` や bare repo であっても削除を許可する（過剰防御を回避）。`touches_git_metadata_path()` と `is_git_metadata_path()` も同じ方針で symlink 自身を許可し、現在のリポジトリの `.git` を指す symlink でも一貫した挙動になる
11. **非UTF-8パスの fail-closed**: `StatusEntry::path()` の UTF-8 制限を避け、キャッシュキーをバイト列 (`HashMap<Vec<u8>, FileStatus>`) で持つ。`get_all_statuses()` は `entry.path_bytes().to_vec()` でキーを生成し、`lookup_status_in_listing()` も `entry.path_bytes()` で比較する。非 UTF-8 ファイル名でもキャッシュに正しく登録され、`NotInRepo` （削除許可）に落ちる fail-open 経路が塞がれる。キャッシュミス時の ignore 判定も `ignored_status_from_relative_path()` で `Err→Modified` に倒し、`status_should_ignore()` の Git API エラーを握りつぶす経路を排除

### パフォーマンス最適化

- Git リポジトリ検出は allowed_paths 外の包含検証や strict Git チェックが必要になった時だけ必須化する。allowed_paths では可能な範囲で現在リポジトリの Git 管理メタデータ保護に使うが、Git 検出失敗だけでは許可パス削除をブロックしない
- `allow_project_deletion = false` かつ `allowed_paths` 外の削除が必要になった時だけ Git status を一括取得（バッチ最適化、Clean ファイルもキャッシュに含め、allowed_paths は Git status エラーに巻き込まない）
- `status_file()` の単体問い合わせで未追跡ディレクトリ配下を見落とすケースは、再帰付き status 一覧の再確認で補完
- Config の `allowed_paths` はロード時に既存親までパスを事前解決（canonicalize）
- `symlink_metadata()` で1回のsyscallで存在確認とメタ情報取得を統合
- Git ワークディレクトリは `open()` 時に1回 canonicalize してキャッシュ（毎回の syscall を回避）

### テスト構成

- **ユニットテスト**: 各モジュール内の `#[cfg(test)]` ブロック（パス検証、Git状態、Config解析、`SAFE_RM_CONFIG` の非 UTF-8 パス対応、symlink削除、symlink 経由の `..` 親ディレクトリ参照拒否、`reject_symlink_parent_traversal` による「`..` 直前成分が存在しない/通常ファイルである」パスの拒否、`link/child/../../victim` のように symlink 配下へ一度下ってから symlink 成分自体を消すパスの拒否、I/Oエラー、Git API エラー時の fail-closed 検証、壊れた `.git` や読み取り不可の探索対象で `GitChecker::open()` が `Err(GitError)` を返し permissive 経路へ抜けないこと、祖先に `.git` 痕跡がなく確認にも成功した場合のみ `Ok(None)` を返すこと、Config 読込/パースエラー時の strict モードフォールバック検証、Git管理メタデータ判定（`.git` を指す symlink 自身の削除は許可、symlink 経由のアクセスはブロックする挙動を `touches_git_metadata_path` / `is_git_metadata_path` 両方で検証）、Git管理メタデータを含む再帰削除の判定、`path_targets_or_contains_git_metadata` による任意階層 Git 管理メタデータ検出（ファイル/ディレクトリ自体・配下・深いネスト・bare リポジトリ・symlink 非追従・存在しないパス・再帰探索時のディレクトリ読み取りエラーを含む）、空リポジトリ対応、壊れた symlink 検出、複数ステータスの一括取得検証、キャッシュ使用時の ignored サブディレクトリチェック、ignored/未追跡混在ディレクトリのブロック、ignored ディレクトリ配下の tracked 変更済みファイルのブロック、`PartialFailure { dry_run: true }` のドライラン文言検証、`ensure_git_metadata_not_targeted()` 単体での `.git`/`.GIT` 中間コンポーネント検出と通常パスの許可検証、非 UTF-8 ファイル名がバイト列キーで `get_all_statuses()` キャッシュに登録されること（macOS APFS 等で非 UTF-8 名作成不可な FS では `fs::write` 失敗で安全にスキップ）、非 UTF-8 ネスト未追跡ファイルが `check_file_with_cache` と空キャッシュ経由 `get_file_status_from_cache` の両方で削除不可（`Untracked`/`Modified`）と判定される fail-closed 検証等）
- **統合テスト**: `tests/integration_test.rs` - 実際のGitリポジトリを tempfile で作成してE2Eテスト。通常の CLI 実行ヘルパーは `SAFE_RM_CONFIG` に未存在パスを明示して、テスト実行ユーザーの `~/.config/safe-rm/config.toml` による環境依存を避ける。repo symlink 別名の cwd からの相対実行、単一失敗時の stderr 非重複、CLI ヘルプがデフォルトモードと厳格モードを誤説明しないこと、force フラグとダーティファイルの複合ケース、ネスト未追跡ディレクトリのブロック、ignored/未追跡混在ディレクトリのブロック、ignored ディレクトリ配下の tracked 変更済みファイルのブロック、プロジェクト外パスで包含検証を優先すること、ドライラン+フォース複合、空ディレクトリ処理、バッチセキュリティエラー優先、設定の複合テスト（strict mode + allowed_paths、複数 allowed_paths エントリ、Git status エラー時と cwd の Git 検出失敗時の allowed_paths バイパス）、strict mode + force フラグの複合テスト、相対パスの `..` コンポーネント検証、バッチ全ダーティの終了コード検証、allowed_paths ディレクトリ自体の削除挙動検証、allowed_paths でも現在のリポジトリの Git 管理メタデータ削除をバイパスできないこと、allowed_paths 配下でも中間 symlink 経由の `.git` 配下削除をブロックし、`.git` を指す symlink 自身の削除は許可すること、2パスバッチの終了コード優先度検証、symlink-to-directory の非再帰削除、Git index 破損時の fail-closed 検証、Config 設定ファイルが読めない/壊れたときに permissive default ではなく strict モードにフォールバックして未追跡削除がブロックされること、Git管理メタデータの常時ブロック、Git管理メタデータを含むリポジトリルート再帰削除のブロック、3パスバッチの終了コード優先度検証、ドライランのファイルシステム非変更保証、設定ファイルのエッジケース（空 allowed_paths 配列、存在しない allowed_paths ディレクトリ）、壊れた symlink のデフォルト/厳格モード対応、空リポジトリ厳格モード、バッチ force フラグ複合、allowed_paths ドライラン注釈、`link/../victim` 形式の symlink 経由 `..` 脱出ブロックと正規化後の別ファイルを削除しないこと（`-f` 併用も含む）、`missing_dir/../victim.txt` 形式（存在しない中間成分）の正規化すり替え拒否、`regular_file/../victim.txt` 形式（通常ファイル中間成分）の正規化すり替え拒否、`-f` 併用時も中間成分が通常ディレクトリでない `..` を fail-closed で拒否すること、非 Git 親直下リポジトリの `.git` 直接削除ブロック、ネストリポジトリ全体の再帰削除と内部 `.git` 直接削除のブロック、bare リポジトリ全体と内部管理ファイル削除のブロック、`nested/.git/config` 形式の中間コンポーネント `.git` 経由メタデータ削除ブロック、`nested/.GIT/config` のような大文字バリアント（macOS APFS 等の case-insensitive FS 対策）のブロック、ドライラン部分失敗の「would be removed」表記検証も含めて検証

### 既知の限界

- **TOCTOU**: 事前の Git 管理メタデータ判定と `fs::remove_dir_all` による実削除の間にはレースウィンドウがある。安全性は AI エージェントが単一プロセスで完結する前提を想定しており、別プロセスが削除対象配下に `.git` や bare リポジトリを置く・rename するシナリオは想定外。同一サーバ上で複数の非協力プロセスが同時に同じパスを操作する環境では追加の防御が必要

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
