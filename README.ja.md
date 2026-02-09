<h1 align="center">safe-rm</h1>

<p align="center">
  <strong>Git対応のファイル保護機能を持つAIエージェント向けセキュア削除CLI</strong>
</p>

<p align="center">
  <a href="https://github.com/owayo/safe-rm/actions/workflows/ci.yml">
    <img alt="CI" src="https://github.com/owayo/safe-rm/actions/workflows/ci.yml/badge.svg?branch=main">
  </a>
  <a href="https://github.com/owayo/safe-rm/releases/latest">
    <img alt="Version" src="https://img.shields.io/github/v/release/owayo/safe-rm">
  </a>
  <a href="LICENSE">
    <img alt="License" src="https://img.shields.io/github/license/owayo/safe-rm">
  </a>
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>

---

## 概要

`safe-rm` は、AIエージェントがコミットされていない作業やプロジェクト外のファイルを誤って削除することを防ぐCLIツールです。**Git対応のアクセス制御**を実施し、プロジェクトディレクトリ（Gitリポジトリルート）内のクリーンまたは無視されたファイルのみ削除を許可します。

## 機能

- **パス境界チェック**: プロジェクトディレクトリ外のファイル削除をブロック
- **Gitステータス保護**: 変更済み、ステージング済み、未追跡ファイルの削除を防止
- **ディレクトリトラバーサル防止**: `../` によるエスケープ試行をブロック
- **無視ファイルの許可**: `.gitignore` で指定されたファイル（ビルド成果物など）の削除を許可
- **許可パス設定**: 指定ディレクトリの安全チェックをバイパス（ディレクトリごとの再帰設定）
- **非Gitサポート**: 非Gitディレクトリでも安全に動作
- **ドライランモード**: 実際に削除せずに削除対象をプレビュー

## 要件

- **OS**: macOS, Linux
- **Rust**: 1.70以上（ソースからビルドする場合）

## インストール

### ソースからビルド

```bash
cargo install --path .
```

### バイナリダウンロード

[Releases](https://github.com/owayo/safe-rm/releases) から最新版をダウンロードしてください。

## 使い方

```bash
# 単一ファイルを削除
safe-rm file.txt

# ディレクトリを再帰的に削除
safe-rm -r directory/

# 複数ファイルを削除
safe-rm file1.txt file2.txt file3.txt

# ドライラン（削除対象を表示）
safe-rm -n file.txt

# 強制モード（存在しないファイルを無視）
safe-rm -f nonexistent.txt

# フラグを組み合わせ
safe-rm -rf build/
```

### オプション

| オプション | 説明 |
|------------|------|
| `-r, --recursive` | ディレクトリとその中身を削除 |
| `-f, --force` | 存在しないファイルを無視（エラーなし） |
| `-n, --dry-run` | 削除せずに削除対象を表示 |
| `-h, --help` | ヘルプを表示 |
| `-V, --version` | バージョンを表示 |

### サブコマンド

| サブコマンド | 説明 |
|------------|------|
| `init` | 設定ファイルを `~/.config/safe-rm/config.toml` に生成 |

## 設定

`safe-rm` は `~/.config/safe-rm/config.toml` にオプションの設定ファイルをサポートしています。`SAFE_RM_CONFIG` 環境変数でカスタムパスを指定することもできます。

### セットアップ

```bash
# デフォルト設定ファイルを生成
safe-rm init
# → ~/.config/safe-rm/config.toml を作成
```

### 設定ファイル形式

```toml
# プロジェクト内のすべてのファイルをGitステータスチェックなしで削除許可
# 境界チェック（プロジェクト外への削除不可）は引き続き有効
# デフォルト: true
allow_project_deletion = true

# このパス配下のすべてのファイル/サブディレクトリを再帰的に許可
# チルダ（~）はホームディレクトリに展開されます
[[allowed_paths]]
path = "~/.claude/skills"
recursive = true

# このディレクトリの直下のファイルのみ許可
[[allowed_paths]]
path = "/tmp/logs"
recursive = false
```

### フィールド

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `allow_project_deletion` | bool | `true` | `true`: プロジェクト内のすべてのファイルをGitステータスチェックなしで削除許可。境界チェックは引き続き有効。 |
| `path` | string | (必須) | 削除を許可するディレクトリパス |
| `recursive` | bool | `false` | `true`: ネストされたすべてのファイル/サブディレクトリを許可。`false`: 直下のファイルのみ。 |

### 動作

- **`allow_project_deletion = true`（デフォルト）**: プロジェクト内のすべてのファイルをGitステータスチェックなしで削除可能。作業プロジェクト内でファイルを自由に削除する必要があるAIエージェントに最適。
- **`allow_project_deletion = false`**: クリーン（コミット済み）または無視されたファイルのみ削除可能。未コミットの変更は保護。
- `allowed_paths` にマッチするパスは、プロジェクト境界チェックとGitステータスチェックの両方をバイパス
- `recursive` フラグでサブディレクトリの扱いを制御:
  - `recursive = true`: `/path/to/dir/sub/deep/file.txt` も許可
  - `recursive = false`: `/path/to/dir/file.txt`（直下のファイル）のみ許可
- 設定ファイルが存在しないか無効な場合、デフォルト動作（`allow_project_deletion = true`、許可パスなし）にフォールバック
- 設定で許可された削除には `(allowed by config)` の注釈が出力に表示

### 例

```bash
# 設定: allowed_paths = [{ path = "~/.claude/skills", recursive = true }]

# 現在のプロジェクト外でも動作:
safe-rm ~/.claude/skills/my-skill/rules.md
# removed: /Users/owa/.claude/skills/my-skill/rules.md (allowed by config)

safe-rm -r ~/.claude/skills/old-skill/
# removed: /Users/owa/.claude/skills/old-skill/ (allowed by config)
```

## アーキテクチャ

```mermaid
flowchart TB
    CLI[CLI引数] --> ConfigCheck{allowed_paths内?}
    ConfigCheck -->|Yes| Delete[ファイル削除]
    ConfigCheck -->|No| PathCheck[パスチェッカー]
    PathCheck --> ProjectCheck{allow_project_deletion?}
    ProjectCheck -->|true| Delete
    ProjectCheck -->|false| GitCheck[Gitチェッカー]
    GitCheck --> Result{クリーンまたは無視?}
    Result -->|Yes| Delete
    Result -->|No| Exit2[Exit 2 + stderr]
    Delete --> Exit0[Exit 0]
```

### 安全レイヤー

1. **パス境界チェック**: すべてのパスがプロジェクトディレクトリ（Gitリポジトリルート、Git外の場合はcwd）内に解決されることを確認（常に有効）
2. **Git保護**: `allow_project_deletion = false` の場合、ダーティファイル（変更済み/ステージング済み/未追跡）の削除をブロック
3. **再帰チェック**: ディレクトリの場合、含まれるすべてのファイルを検証

### ファイルシステムと削除可能スコープ

#### デフォルトモード (`allow_project_deletion = true`)

```mermaid
%%{init: {'theme': 'base', 'themeVariables': { 'lineColor': '#666666', 'primaryTextColor': '#000000', 'primaryBorderColor': '#666666' }}}%%
flowchart TB
    subgraph outside["プロジェクト外 🛡️ 常にブロック"]
        etc["/etc/passwd"]
        home["~/.bashrc"]
        other["../other-project/"]
    end

    subgraph allowed["設定で許可されたパス ✅"]
        skills["~/.claude/skills/**<br/>(設定により許可)"]
    end

    subgraph project["プロジェクトディレクトリ (git root) ✅ すべて削除可能"]
        modified["main.rs (変更済み)"]
        staged["new_feature.rs (ステージング済み)"]
        untracked["temp.txt (未追跡)"]
        clean["old_module.rs (クリーン)"]
        ignored["target/ (.gitignore)"]
    end

    style outside fill:#ffcccc,stroke:#cc0000,color:#000000
    style allowed fill:#ccffcc,stroke:#00cc00,color:#000000
    style project fill:#ccffcc,stroke:#00cc00,color:#000000
```

| ファイル | 削除可能 | 理由 |
|----------|----------|------|
| `old_module.rs` (クリーン) | ✅ はい | プロジェクト内 |
| `target/` (無視) | ✅ はい | プロジェクト内 |
| `main.rs` (変更済み) | ✅ はい | プロジェクト内 (allow_project_deletion=true) |
| `temp.txt` (未追跡) | ✅ はい | プロジェクト内 (allow_project_deletion=true) |
| `~/.claude/skills/foo` | ✅ はい | 設定により許可（recursive） |
| `/etc/passwd` | ❌ いいえ | プロジェクトディレクトリ外 |
| `../other-project/` | ❌ いいえ | パストラバーサルをブロック |

#### 厳格モード (`allow_project_deletion = false`)

```mermaid
%%{init: {'theme': 'base', 'themeVariables': { 'lineColor': '#666666', 'primaryTextColor': '#000000', 'primaryBorderColor': '#666666' }}}%%
flowchart TB
    subgraph outside["プロジェクト外 🛡️ 常にブロック"]
        etc["/etc/passwd"]
        home["~/.bashrc"]
        other["../other-project/"]
    end

    subgraph allowed["設定で許可されたパス ✅"]
        skills["~/.claude/skills/**<br/>(設定により許可)"]
    end

    subgraph project["プロジェクトディレクトリ (git root)"]
        subgraph dirty["未コミットの変更 🛡️"]
            modified["main.rs<br/>(変更済み)"]
            staged["new_feature.rs<br/>(ステージング済み)"]
            untracked["temp.txt<br/>(未追跡)"]
        end

        subgraph deletable["削除可能なファイル ✅"]
            clean["old_module.rs<br/>(クリーン/コミット済み)"]
            ignored["target/<br/>(.gitignore)"]
            nodemod["node_modules/<br/>(.gitignore)"]
        end
    end

    style outside fill:#ffcccc,stroke:#cc0000,color:#000000
    style allowed fill:#ccffcc,stroke:#00cc00,color:#000000
    style dirty fill:#ffcccc,stroke:#cc0000,color:#000000
    style deletable fill:#ccffcc,stroke:#00cc00,color:#000000
```

| ファイル | 削除可能 | 理由 |
|----------|----------|------|
| `old_module.rs` (クリーン) | ✅ はい | コミット済み、`git checkout` で復元可能 |
| `target/` (無視) | ✅ はい | `.gitignore` に記載、ビルド成果物 |
| `node_modules/` (無視) | ✅ はい | `.gitignore` に記載、依存関係 |
| `~/.claude/skills/foo` | ✅ はい | 設定により許可（recursive） |
| `main.rs` (変更済み) | ❌ いいえ | 未コミットの変更が失われる |
| `new_feature.rs` (ステージング済み) | ❌ いいえ | コミット待ちの内容が失われる |
| `temp.txt` (未追跡) | ❌ いいえ | Git履歴になく、復元不可能 |
| `/etc/passwd` | ❌ いいえ | プロジェクトディレクトリ外 |
| `../other-project/` | ❌ いいえ | パストラバーサルをブロック |

**重要ポイント**:
- プロジェクト外のファイルは**常にブロック**（設定に関係なく）
- **デフォルトモード (`allow_project_deletion = true`)**: プロジェクト内のすべてのファイルが削除可能（AIエージェントに最適）
- **厳格モード (`allow_project_deletion = false`)**: クリーン（コミット済み）または無視されたファイルのみ削除可能
- **設定で許可されたパス**は境界チェックとGitチェックの両方をバイパス（`~` 展開対応）

## 終了コード

| コード | 意味 | 例 |
|--------|------|-----|
| 0 | 成功 | ファイル削除、ドライラン完了 |
| 1 | 操作エラー | ファイルが見つからない、-rなしでディレクトリ、I/Oエラー、部分的失敗 |
| 2 | セキュリティブロック | ダーティファイル、プロジェクト外、ディレクトリ読み取りエラー（fail-closed） |

## Claude Code 統合

Claude Code のフックで `rm`/`rmdir` コマンドを `safe-rm` にリダイレクトします。

### フック設定

Claude Code の設定ファイル（例: `~/.claude/settings.json` または `.claude/settings.json`）に追加:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "jq -r '.tool_input.command // \"\"' | grep -qE '^rm(dir)?\\b' && { echo '🚫 Use safe-rm instead: safe-rm <file> (validates Git status and path containment). Only clean/ignored files in project allowed.' >&2; exit 2; }; exit 0"
          }
        ]
      }
    ]
  }
}
```

このフック:
1. stdin の JSON から `rm`/`rmdir` コマンドを検出
2. 終了コード2でブロックし、ガイダンスメッセージを Claude に表示
3. Claude は安全な削除のために `safe-rm <file>` を直接使用

### CLAUDE.md への記載

`CLAUDE.md` に追加:

```markdown
## ファイル削除ルール

- `rm` や `rmdir` は使用禁止（安全のため制限されています）
- ファイル削除には `safe-rm <file>` または `safe-rm -r <directory>` を使用
- `safe-rm` はファイルが安全に削除可能か自動検証します（Git でコミット済みまたは無視されたファイル）
- `safe-rm` が失敗した場合、そのファイルは未コミットの変更があるか、プロジェクト外にあります

### 使用例
- ビルド成果物を削除: `safe-rm -r target/`
- 古いファイルを削除: `safe-rm old_module.rs`
- 削除対象をプレビュー: `safe-rm -n file.txt`
```

## Git ステータス判定マトリクス

### デフォルトモード (`allow_project_deletion = true`)

| ファイルステータス | 削除可能? | 理由 |
|-------------------|-----------|------|
| すべて（プロジェクト内） | はい | `allow_project_deletion = true` はGitチェックをスキップ |
| プロジェクト外 | いいえ | 設定に関わらず常にブロック |

### 厳格モード (`allow_project_deletion = false`)

| ファイルステータス | 削除可能? | 理由 |
|-------------------|-----------|------|
| Clean | はい | コミット済みで `git checkout` で復元可能 |
| Modified | いいえ | コミットされていない変更が失われる |
| Staged | いいえ | コミット待ちの内容が失われる |
| Untracked | いいえ | Git履歴になく、復元不可能 |
| Ignored | はい | ビルド成果物、ソース管理外 |
| プロジェクト外 | いいえ | Git状態に関わらず常にブロック |

**注意**: カレントディレクトリがGitリポジトリでない場合、Gitステータスチェックはスキップされ、プロジェクト内のすべてのファイルが削除可能になります。

## 使用例

### 許可される操作

```bash
# クリーンファイル（コミット済み、変更なし）
safe-rm src/old_module.rs  # Exit 0

# 無視されたファイル（.gitignoreに記載）
safe-rm target/debug/app   # Exit 0
safe-rm -r node_modules    # Exit 0

# 非Gitディレクトリ
safe-rm temp_file.txt      # Exit 0

# ドライラン
safe-rm -n file.txt # Exit 0, "would remove: file.txt" を表示
```

### ブロックされる操作

```bash
# 変更済みファイル
safe-rm src/main.rs
# Exit 2: "未コミットの変更があるファイルは削除できません"

# プロジェクト外
safe-rm /etc/passwd
# Exit 2: "プロジェクト外へのアクセスは禁止されています"

safe-rm ../../../etc/hosts
# Exit 2: "プロジェクト外へのアクセスは禁止されています"

# 未追跡ファイル
safe-rm new_feature.rs
# Exit 2: "未コミットの変更があるファイルは削除できません"
```

## 開発

```bash
# ビルド
cargo build

# テスト実行
cargo test

# リリースビルド
cargo build --release
```

### テストカバレッジ

- **ユニットテスト**: 全モジュールをカバーする130件以上のテスト（CLI、config、error、path_checker、git_checker、init）
- **統合テスト**: 実際のGitリポジトリを使用した50件以上のテスト（許可/ブロックフロー、厳格モード、シンボリックリンク、バッチ処理、特殊ファイル名）

## コントリビューション

コントリビューションを歓迎します！お気軽にプルリクエストを送信してください。

## セキュリティ

セキュリティの脆弱性を発見した場合は、[GitHub Issues](https://github.com/owayo/safe-rm/issues) から報告してください。

## ライセンス

[MIT](LICENSE)
