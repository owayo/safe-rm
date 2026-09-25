# 設定の詳細

`~/.config/safe-rm/config.toml` の設定がどう働くかを説明します。設定ファイルの場所、`safe-rm init`、書式、各フィールドは [README](../README.ja.md#設定) にあります。

## 動作

- **`allow_project_deletion = true`（デフォルト）**: プロジェクト内の作業ツリーファイルは Git ステータスチェックなしで削除可能。`.git` などの Git 管理パスと、ネストしたリポジトリを含む任意リポジトリの Git 管理メタデータを含む再帰削除は引き続きブロック。
- **`allow_project_deletion = false`**: クリーン（コミット済み）または無視された作業ツリーファイルのみ削除可能。ignored な親ディレクトリ配下にある場合でも未コミットの変更は保護され、Git 管理パスも引き続きブロック。`allowed_paths` にマッチするパスは、現在のリポジトリの index を読めない場合や cwd の Git 管理情報を開けない場合でも Git ステータスチェックをバイパス。
- `allowed_paths` にマッチするパスは、プロジェクト境界チェックと Git ステータスチェックをバイパスするが、任意リポジトリの Git 管理メタデータ保護はバイパスできない。現在リポジトリの Git 管理メタデータは Git 検出に成功した場合に追加で確認し、cwd の Git 検出失敗だけでは許可パス削除を止めない。中間 symlink が `.git` や bare リポジトリ管理領域へ解決される場合は引き続きブロックし、symlink 自身の削除はリンクだけを消すため許可する。未作成パスでも既存親ディレクトリまで canonicalize して別名パス差異を吸収
- `recursive` フラグでサブディレクトリの扱いを制御:
  - `recursive = true`: `/path/to/dir/sub/deep/file.txt` も許可。`safe-rm -r /path/to/dir/sub` は allowed バイパス経由で配下を再帰削除する
  - `recursive = false`: `/path/to/dir/file.txt`（直下のファイル）のみ許可。直下のサブディレクトリも `-r` で削除できる場合があるが、その場合は allowed バイパスではなく**標準分岐（プロジェクト境界検証 + 厳格モードでは Git ステータス検査）に落ちる**。非再帰エントリ配下のディレクトリへの `-r` は意図しない子孫まで削除しうるため、allowed バイパスとしては明示的に拒否し、必ず標準チェックを経由させる
- 設定ファイルが**本当に存在しない**場合は permissive デフォルト（`allow_project_deletion = true`、許可パスなし）にフォールバック。設定ファイルの場所自体を決定できない場合、または設定ファイルが**存在するが読み込み/パースに失敗**した場合 — 設定パス（最終・中間いずれのコンポーネント）が **dangling symlink**（`read_to_string`・`symlink_metadata` ともに `NotFound` を返すが、設定が置かれた意図が壊れている状態）のケースを含む — は、利用者が意図した strict 設定が不明な設定位置・構文エラー・symlink 切れで無効化されないよう、fail-closed で strict モード（`allow_project_deletion = false`、許可パスなし）にフォールバック。symlink が解決でき、その先のファイルが未作成なだけの場合は本当に存在しないものとして扱い（permissive）、未設定環境を過剰に strict 化しない
- `safe-rm init` は設定ファイルパス自体も保護する。`~/.config/safe-rm/config.toml` が dangling symlink の場合も既存エントリとして扱ってリンク先を辿らないため、生成テンプレートが symlink のリンク先へ書き込まれない。
- 設定で許可された削除には `(allowed by config)` の注釈が出力に表示

## 例

```bash
# `safe-rm init` が生成するデフォルト設定:
# allowed_paths = [
#   { path = "~/.claude/skills", recursive = true },
#   { path = "/tmp", recursive = true },
# ]

# 現在のプロジェクト外でも動作:
safe-rm ~/.claude/skills/my-skill/rules.md
# removed: /Users/you/.claude/skills/my-skill/rules.md (allowed by config)

safe-rm -r ~/.claude/skills/old-skill/
# removed: /Users/you/.claude/skills/old-skill/ (allowed by config)
```
