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

`safe-rm` は、AIエージェントがプロジェクト外のファイルや重要な Git 管理メタデータを誤って削除することを防ぐ CLI ツールです。デフォルトではプロジェクト境界を強制し、`.git` などの Git 管理パスをブロックします。**厳格モード**（`allow_project_deletion = false`）ではさらに Git 対応のアクセス制御を有効にし、変更済み・ステージング済み・未追跡ファイルの削除も防止します。

## 機能

- **パス境界チェック**: プロジェクトディレクトリ外のファイル削除をブロック
- **厳格モードの Git ステータス保護**: `allow_project_deletion = false` のとき、変更済み・ステージング済み・未追跡ファイルの削除を防止。判定には削除対象を実際に所有する Git リポジトリを使用し、cwd 由来の checker と対象起点で discover した checker の両方を評価して、対象を含む最も深い workdir を持つ checker を選択する。これにより (a) cwd が非 Git でも対象が配下のネスト Git リポジトリ内にあるケース、(b) cwd 側 repo が `.gitignore` 等でネスト repo を除外していて本来 `Ignored` として素通りするケース、(c) cwd と対象が別の Git リポジトリに属するケースの三種類のバイパスをいずれも fail-closed で塞ぐ。さらに、対象を所有するリポジトリが `core.worktree` 等でワークツリーを別の場所へリダイレクトしていて `workdir()` が対象を含まない場合も、`NotInRepo` への素通りを防ぐため `Modified` 扱いで fail-closed にブロックする
- **Git 管理メタデータ保護**: `.git`、gitdir 参照ファイル、bare リポジトリの管理パス、現在のリポジトリの Git 管理メタデータを含む再帰削除に加え、削除対象自身・パスの任意の中間コンポーネント・再帰削除時に配下に存在する任意階層の Git 管理メタデータを常時ブロック。ネストした `.git` ファイル/ディレクトリだけでなく、`.git` コンポーネントを持たない bare リポジトリも検出。コンポーネント比較は ASCII case-insensitive で、macOS APFS などの大文字小文字を区別しないファイルシステムでの `.GIT` 経由バイパスも防止。中間 symlink が `.git` ディレクトリや bare リポジトリを指している場合（例: `gitlink -> nested/.git` のもとで `gitlink/config` を削除）も、親ディレクトリのみを canonicalize して末尾コンポーネントを保持する判定でブロック（symlink 自身の削除はリンクのみを消すため引き続き許可される）
- **コンフリクト対応のステータス判定**: `Status::CONFLICTED` フラグが立ったファイルは、他の index/worktree フラグが立っていない場合でも厳格モードで `Modified` として扱い、未解決のマージコンフリクトが黙って削除されることを防止
- **ネストしたダーティファイル保護**: 厳格モードでは未追跡ディレクトリ配下、ignored/未追跡が混在するディレクトリ配下、ignored ディレクトリ配下の tracked 変更済みファイルも Git 管理外扱いにせず、正しくブロック
- **ディレクトリトラバーサル防止**: `../` の直前成分が「実体として存在する通常ディレクトリ」でないパスは fail-closed で拒否。`link/../victim`（symlink 中間成分）、`missing/../victim`（存在しない中間成分）、`file/../victim`（通常ファイル中間成分）はいずれも OS の path resolution では失敗するが、字句正規化で `victim` に化けてしまう経路をブロック
- **dangling 中間 symlink ガード**: `dangling/child.txt` のように中間コンポーネントが解決不能な symlink のパスは、メタデータ取得前に fail-closed でブロック。末尾の dangling symlink 自体はリンクエントリだけを削除するため引き続き許可
- **`.` / `..` operand の拒否**: 末尾成分が `.` または `..` の operand（`.`・`./`・`..`・`../`・`sub/.`・`sub/..`・`/abs/path/..`）を拒否。POSIX の rm も同じ operand について診断メッセージを出すだけで一切処理しない。この防御がないと `safe-rm -r .` がカレントディレクトリ自体を、`safe-rm -r ..` が親ディレクトリを削除してしまい、置き換え対象の rm より危険側へ倒れる。判定は正規化・`allowed_paths` 判定・`-f` 処理より前に行うため、許可パスでも force でもバイパスできない。`.hidden`・`...`・`..foo` のような dot で始まる通常のファイル名は従来どおり削除可能。Windows では drive-relative 形式（`C:.` / `C:..`）も拒否
- **無視ファイルの許可**: `.gitignore` で指定されたファイル（ビルド成果物など）の削除を許可。ただし `.gitignore` に一致していても `git add -f` で強制追跡された（tracked な）ファイルは、未コミット変更があれば保護される — ignore 判定より先にステータスを解決するため、追跡済みの dirty ファイルが `Ignored` と誤判定されることはない
- **シンボリックリンク安全なGitチェック**: ディレクトリ symlink は辿らず、リンク自体として判定
- **非UTF-8パス対応**: Git ステータスキャッシュは生バイト列をキー (`HashMap<Vec<u8>, FileStatus>`) として持ち、`entry.path_bytes()` を直接使うため、非 UTF-8 名のファイルも正しく登録される。これがないと、未追跡ディレクトリ配下の非 UTF-8 未追跡ファイルが `NotInRepo` に落ちて厳格モードでも削除可能になってしまう。`status_should_ignore()` のエラーも握りつぶさずに `Modified` 相当として fail-closed でブロック
- **エイリアスパス耐性（包含検証 + allowed_paths + 厳格モード）**: 包含検証と `allowed_paths` 判定では「既存親ディレクトリまで canonicalize + 未作成部分を再結合」、厳格モードの Git チェックでは「非 symlink パスを canonicalize、symlink パスは親ディレクトリのみ canonicalize + リンク自体を判定」として、別名絶対パス経由のバイパスを防止
- **許可パス設定**: 指定ディレクトリのプロジェクト境界チェックと Git ステータスチェックをバイパス（ディレクトリごとの再帰設定）。現在ディレクトリの Git 検出に失敗しても、許可パス削除はそれだけではブロックされない
- **非Gitサポート**: 非Gitディレクトリでも安全に動作
- **ドライランモード**: 実際に削除せずに削除対象をプレビュー
- **決定的なエラー出力**: 単一パス失敗時は stderr を1回だけ出力し、複数パス実行時は失敗した各パスごとに1回ずつ出力

## 要件

- **OS**: macOS, Linux
- **Rust**: 1.87以上（ソースからビルドする場合）

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

`safe-rm` は `~/.config/safe-rm/config.toml` にオプションの設定ファイルをサポートしています。`SAFE_RM_CONFIG` 環境変数でカスタムパスを指定することもできます。Unix ではこの環境変数を生の OS パスとして読み取るため、非 UTF-8 パスも保持されます。

### セットアップ

```bash
# デフォルト設定ファイルを生成
safe-rm init
# → ~/.config/safe-rm/config.toml を作成
# デフォルトでは ~/.claude/skills と /tmp を再帰的に許可
```

`safe-rm init` は既存の設定ファイルパスを上書きしません。既存ファイルだけでなく、dangling symlink を含む既存 symlink も「既に存在する設定エントリ」として扱い、さらに新規作成専用の書き込みを使うため、競合時にもテンプレートが別パスへ書き込まれません。

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

# /tmp 配下を再帰的に許可
[[allowed_paths]]
path = "/tmp"
recursive = true

# このディレクトリの直下のファイルのみ許可
# [[allowed_paths]]
# path = "/tmp/logs"
# recursive = false
```

### フィールド

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `allow_project_deletion` | bool | `true` | `true`: プロジェクト内のすべてのファイルをGitステータスチェックなしで削除許可。境界チェックは引き続き有効。 |
| `path` | string | (必須) | 削除を許可するディレクトリパス |
| `recursive` | bool | `false` | `true`: ネストされたすべてのファイル/サブディレクトリを許可。`false`: 直下のファイルのみ。 |

### 動作

- **`allow_project_deletion = true`（デフォルト）**: プロジェクト内の作業ツリーファイルは Git ステータスチェックなしで削除可能。`.git` などの Git 管理パスと、ネストしたリポジトリを含む任意リポジトリの Git 管理メタデータを含む再帰削除は引き続きブロック。
- **`allow_project_deletion = false`**: クリーン（コミット済み）または無視された作業ツリーファイルのみ削除可能。ignored な親ディレクトリ配下にある場合でも未コミットの変更は保護され、Git 管理パスも引き続きブロック。`allowed_paths` にマッチするパスは、現在のリポジトリの index を読めない場合や cwd の Git 管理情報を開けない場合でも Git ステータスチェックをバイパス。
- `allowed_paths` にマッチするパスは、プロジェクト境界チェックと Git ステータスチェックをバイパスするが、任意リポジトリの Git 管理メタデータ保護はバイパスできない。現在リポジトリの Git 管理メタデータは Git 検出に成功した場合に追加で確認し、cwd の Git 検出失敗だけでは許可パス削除を止めない。中間 symlink が `.git` や bare リポジトリ管理領域へ解決される場合は引き続きブロックし、symlink 自身の削除はリンクだけを消すため許可する。未作成パスでも既存親ディレクトリまで canonicalize して別名パス差異を吸収
- `recursive` フラグでサブディレクトリの扱いを制御:
  - `recursive = true`: `/path/to/dir/sub/deep/file.txt` も許可。`safe-rm -r /path/to/dir/sub` は allowed バイパス経由で配下を再帰削除する
  - `recursive = false`: `/path/to/dir/file.txt`（直下のファイル）のみ許可。直下のサブディレクトリも `-r` で削除できる場合があるが、その場合は allowed バイパスではなく**標準分岐（プロジェクト境界検証 + 厳格モードでは Git ステータス検査）に落ちる**。非再帰エントリ配下のディレクトリへの `-r` は意図しない子孫まで削除しうるため、allowed バイパスとしては明示的に拒否し、必ず標準チェックを経由させる
- 設定ファイルが**本当に存在しない**場合は permissive デフォルト（`allow_project_deletion = true`、許可パスなし）にフォールバック。設定ファイルの場所自体を決定できない場合、または設定ファイルが**存在するが読み込み/パースに失敗**した場合 — 設定パス（最終・中間いずれのコンポーネント）が **dangling symlink**（`read_to_string`・`symlink_metadata` ともに `NotFound` を返すが、設定が置かれた意図が壊れている状態）のケースを含む — は、利用者が意図した strict 設定が不明な設定位置・構文エラー・symlink 切れで無効化されないよう、fail-closed で strict モード（`allow_project_deletion = false`、許可パスなし）にフォールバック。symlink が解決でき、その先のファイルが未作成なだけの場合は本当に存在しないものとして扱い（permissive）、未設定環境を過剰に strict 化しない
- `safe-rm init` は設定ファイルパス自体も保護する。`~/.config/safe-rm/config.toml` が dangling symlink の場合も既存エントリとして扱ってリンク先を辿らないため、生成テンプレートが symlink のリンク先へ書き込まれない。
- 設定で許可された削除には `(allowed by config)` の注釈が出力に表示

### 例

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

## アーキテクチャ

```mermaid
flowchart TB
    CLI[CLI引数] --> DotCheck{末尾成分が . または ..?}
    DotCheck -->|Yes| Exit2[Exit 2 + stderr]
    DotCheck -->|No| ConfigCheck{allowed_paths内?}
    ConfigCheck -->|Yes| AllowedGitMetaCheck{Git 管理パス?}
    AllowedGitMetaCheck -->|Yes| Exit2
    AllowedGitMetaCheck -->|No| Delete[ファイル削除]
    ConfigCheck -->|No| GitOpen[必要時 Git リポジトリ検出]
    GitOpen --> PathCheck[パスチェッカー]
    PathCheck --> GitMetaCheck{Git 管理パス?}
    GitMetaCheck -->|Yes| Exit2
    GitMetaCheck -->|No| ProjectCheck{allow_project_deletion?}
    ProjectCheck -->|true| Delete
    ProjectCheck -->|false| GitCheck[Gitチェッカー]
    GitCheck --> Result{クリーンまたは無視?}
    Result -->|Yes| Delete
    Result -->|No| Exit2
    Delete --> Exit0[Exit 0]
```

### 安全レイヤー

1. **Git 管理メタデータ保護**: `.git`、gitdir 参照ファイル、bare リポジトリの管理パス、および Git 管理メタデータを含む再帰削除は、設定に関係なく常に削除をブロック。bare リポジトリは構造マーカー（`HEAD` エントリ・`objects/`・`refs/`）で検出するため、`config` が壊れて `Repository::open_bare()` が失敗する bare リポジトリも fail-closed で保護する。再帰的なメタデータ探索でディレクトリエントリを読み取れない場合も fail-closed でブロック。
2. **パス境界チェック**: `allowed_paths` 以外のすべてのパスがプロジェクトディレクトリ（Gitリポジトリルート、Git外の場合はcwd）内に解決されることを、再帰的なメタデータ探索より先に確認。存在しない削除対象でも、既存の親ディレクトリまで canonicalize して別名パス差異（repo symlink 別名、`/var` と `/private/var` など）を吸収。削除対象エントリ（末尾 symlink を辿らない位置）と末尾まで解決した実体の両方がプロジェクト内であることを要求するため、プロジェクト外の symlink がプロジェクト内を指すケース（エントリが外）と、プロジェクト内の symlink がプロジェクト外を指すケース（実体が外）の双方をブロックする。同じ2位置判定は `allowed_paths` のマッチにも適用される。
3. **Git保護**: `allow_project_deletion = false` の場合、ダーティファイル（変更済み/ステージング済み/未追跡）の削除をブロック。未追跡ディレクトリの深い階層にあるファイルも対象。ステータス判定には cwd 由来の checker だけでなく、削除対象起点でも `GitChecker::open()` を試行し、対象を含む repo のうち最も深い workdir を持つ checker を採用する。cwd 非 Git で対象側だけ Git のケース、cwd の outer repo が nested repo を `.gitignore` で除外しているケース、cwd と別 Git の対象を指定するケースの三種のクロスリポジトリ削除でも、対象側 repo の status で fail-closed に判定する
4. **再帰チェック**: 実ディレクトリの場合、含まれるすべてのファイルを検証。ignored な子孫ファイルは削除可能だが、ignored な親ディレクトリが tracked な変更済み/ステージング済みファイルや未追跡の兄弟ファイルを隠すことはない
5. **Fail-Closed**: ディレクトリ走査中のエラー（エントリ列挙エラーを含む）、対象の metadata 種別判定エラー、`allowed_paths` 外の削除経路で必要になる `Repository::discover()` 由来の Git API エラー（壊れた `.git` / 権限不足 / I/O エラー等）、worktree の canonicalize 失敗、`NotFound` 判定時の祖先メタデータ確認エラー、解決不能な中間 symlink、および設定位置の決定失敗・設定ファイル読込/パースエラー時は削除をブロックまたは strict モードへフォールバック。`NotFound` は `.git`/bare リポジトリの祖先がなく、祖先確認自体にも成功した場合のみ非 Git として扱う。`allowed_paths` は Git 管理メタデータ保護を維持しつつ、現在ディレクトリの Git 検出成功は必須にしない
6. **不正な親ディレクトリ参照ガード**: `..` の直前成分が「実体として存在する通常ディレクトリ」でないパス（symlink・通常ファイル・存在しない・読み取り不能・特殊ファイル）は削除前に fail-closed で拒否。`link/../victim`・`missing/../victim`・`file/../victim` が字句正規化で別ファイルへすり替わって削除される経路をブロック。dangling 中間 symlink 配下のパスも削除前に拒否し、末尾の dangling symlink エントリ自体の削除は許可する
7. **エイリアスパス対策**: 包含検証と `allowed_paths` 判定では既存親ディレクトリまで canonicalize して未作成部分を再結合し、Gitチェックでは非symlinkパスを canonicalize 比較し、symlink パスは「親ディレクトリのみ canonicalize + リンク自体を判定」することで、repo symlink 別名や `/var` と `/private/var` の差異による回避を防止

8. **`.` / `..` operand の拒否**: 末尾成分が `.` または `..` の operand は、正規化・`allowed_paths` 判定・`-f` 処理より前に exit 2 でブロックする。POSIX の rm も `.` / `..` ディレクトリの削除を拒否するため、これに揃えて「置き換え前の rm より危険」な状態を作らない。削除したいディレクトリは名前で明示する（`safe-rm -r sub`）。`.hidden` や `...` のような dot 始まりのファイル名は影響を受けない

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

    subgraph project["プロジェクトディレクトリ (git root) ✅ 作業ツリーファイルは削除可能"]
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
| `.git/` | ❌ いいえ | Git 管理メタデータは常時保護 |
| `./` または repo root に `-r` | ❌ いいえ | 再帰削除が Git 管理メタデータを含む |
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
| `.git/` | ❌ いいえ | Git 管理メタデータは常時保護 |
| `./` または repo root に `-r` | ❌ いいえ | 再帰削除が Git 管理メタデータを含む |
| `main.rs` (変更済み) | ❌ いいえ | 未コミットの変更が失われる |
| `new_feature.rs` (ステージング済み) | ❌ いいえ | コミット待ちの内容が失われる |
| `temp.txt` (未追跡) | ❌ いいえ | Git履歴になく、復元不可能 |
| `/etc/passwd` | ❌ いいえ | プロジェクトディレクトリ外 |
| `../other-project/` | ❌ いいえ | パストラバーサルをブロック |

**重要ポイント**:
- プロジェクト外のファイルは**常にブロック**（設定に関係なく）
- `.git` などの Git 管理メタデータは**常にブロック**（設定に関係なく）。現在のリポジトリルートの再帰削除も対象
- **デフォルトモード (`allow_project_deletion = true`)**: プロジェクト内の作業ツリーファイルは削除可能（AIエージェントに最適）
- **厳格モード (`allow_project_deletion = false`)**: クリーン（コミット済み）または無視された作業ツリーファイルのみ削除可能。ディレクトリ削除では、ディスク上に存在しない未コミットの削除（配下の `git rm` 済み=staged ファイルや worktree から削除済みの tracked ファイル）も拒否する。これらは `read_dir` ではなく Git ステータスキャッシュで検出する
- **設定で許可されたパス**は境界チェックと Git ステータスチェックをバイパスするが、Git 管理メタデータ保護はバイパスしない（`~` 展開対応）

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
            "command": "bash_command=\"$(jq -er '.tool_input.command | strings')\" || { echo '🚫 Bash コマンドを検査できないため fail-closed でブロックします。' >&2; exit 2; }; if printf '%s\\n' \"$bash_command\" | grep -qE '(^|[[:space:];|&(`])([^[:space:];|&()]+/)?rm(dir)?([[:space:];|&()<>]|$)'; then echo '🚫 safe-rm を使用してください: safe-rm <file>（Git 状態とパス包含を検証）' >&2; exit 2; fi; exit 0"
          }
        ]
      }
    ]
  }
}
```

このフック:
1. stdin の JSON から Bash コマンド全体を読み、`cd dir && rm`、`echo ok; rm`、`command rm`、`/bin/rm`、`$(rm file)` を含むリテラルな `rm`/`rmdir` コマンドトークンを保守的に検出
2. 終了コード2でブロックし、ガイダンスメッセージを Claude に表示
3. Claude は安全な削除のために `safe-rm <file>` を直接使用

このフックは fail-closed を優先するため、引用または表示するだけの `rm` 文字列も拒否する場合があります。また完全なシェルパーサーではなく、動的に組み立てたコマンド名はテキスト検査を回避できます。多層防御として扱い、Bash 権限を制限したうえで、削除には `safe-rm` を必須としてください。

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

**注意**: カレントディレクトリが Git リポジトリでなくても strict チェックは無効になりません。`safe-rm` は削除対象からもリポジトリを検出し、対象を含む最も深い worktree で判定します。対象自体が Git リポジトリに属さない場合のみ Git ステータスチェックをスキップし、リポジトリ検出や worktree 解決のエラーは fail-closed でブロックします。

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

# `.` / `..` operand（POSIX の rm も拒否する形式）
safe-rm -r .
# Exit 2: "末尾が '.' または '..' のパスは削除できません"
safe-rm -r sub/..
# Exit 2: "末尾が '.' または '..' のパスは削除できません"
# 代わりにディレクトリ名を明示する: safe-rm -r sub
```

## 開発

```bash
# ビルド
cargo build

# テスト実行
cargo test

# 最小サポート Rust バージョンを確認
cargo +1.87.0 check --locked --all-targets --all-features

# リリースビルド
cargo build --locked --release
```

CI は Rust 1.87.0 で最小サポートバージョンを検証し、テスト・lint・リリースビルドではコミット済みの `Cargo.lock` を強制使用する。ローカルの `make release` と `make install` でも同じロックファイルを強制する。リリースワークフローは workflow_dispatch 対象の同一コミットを全ターゲットでビルドし、すべて成功した後にだけリリースコミットとタグを push する。

### テストカバレッジ

- **ユニットテスト**: ライブラリ284件 + バイナリ27件のテストで全モジュールをカバー（CLI、config、error、path_checker、git_checker、init）。Git API エラー時の fail-closed 検証、Git 管理メタデータの再帰探索エラー時の fail-closed 検証、worktree の canonicalize エラーと対象 metadata 種別判定エラーの伝播、`.git` と `.GIT` メタデータディレクトリの再帰検出、`convert_status()` が `WT_UNREADABLE` と未知のステータスフラグを `Modified` に倒し空ビット（`CURRENT`）のときだけ `Clean` を返す検証、`check_path_with_cache()` がディスク上に存在しない未コミットの削除（配下の `git rm` 済み=staged ファイルや worktree から削除済みの tracked ファイル）を含むディレクトリをブロックし、prefix が重なる別ディレクトリ（`dir` vs `dir2`）では誤ブロックしない検証、strict モードで対象を含む最も深い Git workdir を選択する検証、壊れた `.git` や読み取り不可の探索対象で `GitChecker::open()` が `Err(GitError)` を返し permissive 経路へ抜けないこと（`.git` 痕跡がなく、祖先確認にも成功した場合のみ `Ok(None)`）、`Config::load_from_path()` のフェイルクローズ検証（`read_to_string()` の結果で分岐し、設定位置不明・その他の I/O エラー・TOML パース失敗・最終または中間コンポーネントが dangling symlink の設定パスは strict モードへフォールバック、本当に存在しない場合と解決可能な symlink 配下の未作成 config のみ permissive を維持）、`link/child/../../victim` のようなネストした symlink 経由の `..` 親ディレクトリ参照拒否、dangling 中間 symlink のブロックと末尾 dangling symlink 自身の削除許可、`FileStatus::is_deletable()` 検証、設定の前方互換性、Unix における `SAFE_RM_CONFIG` の非 UTF-8 パス対応、キャッシュフォールバック動作、空リポジトリ対応、壊れた symlink 検出、複数ステータスの一括取得、キャッシュ使用時の ignored サブディレクトリチェック、`Status::CONFLICTED` を `Modified` にマッピングする検証（単独フラグおよび他フラグとの組み合わせ）、`touches_git_metadata_path`/`is_git_metadata_path` が現在のリポジトリの `.git` を指す symlink 自身の削除を許可しつつ symlink 経由のアクセスはブロックする検証、中間 symlink 経由の `.git`/bare リポジトリバイパス検出（symlink 配下の管理ファイルはブロック、symlink 自身の削除は許可）、`config` 破損で `Repository::open_bare()` が失敗する bare リポジトリを構造マーカーで検出する検証（`HEAD` が未作成 ref を指す symlink のケースを含む）、および許可ディレクトリ内から許可範囲外を指す symlink を拒否する検証を含む
- **統合テスト**: 実際のGitリポジトリを使用した173件のテスト（許可/ブロックフロー、厳格モード、シンボリックリンク、repo symlink 別名の cwd からの相対実行を含むエイリアスパス対策、バッチ処理、ドライラン厳格モード、特殊ファイル名、forceフラグとダーティファイルの複合ケース、ネスト未追跡ディレクトリのブロック、ドライラン+フォース複合、空ディレクトリ処理、バッチセキュリティエラー優先、設定の複合テスト、strict mode + force フラグの複合テスト、`..` コンポーネントを含む相対パス検証、バッチ全ダーティの終了コード検証、allowed_paths ディレクトリ自体の削除挙動検証、2パスバッチの終了コード優先度検証、symlink-to-directory の非再帰削除、Git index 破損時の fail-closed 検証、設定ファイル読込/パースエラー時の strict モードフォールバック検証（読めない/壊れた config は permissive default に倒れず未追跡削除をブロックする）、3パスバッチの終了コード優先度検証、ドライランのファイルシステム非変更保証、設定ファイルのエッジケース、壊れた symlink のデフォルト/厳格モード対応、空リポジトリ厳格モード、バッチ force フラグ複合、allowed_paths ドライラン注釈、symlink 経由の `..` 親ディレクトリ参照拒否、dangling 中間 symlink のブロック、live 中間 symlink の包含ブロック、再帰削除時の `.GIT` メタデータブロック、中間 symlink で `.git`/bare リポジトリ配下を指す削除のブロック、プロジェクト外の symlink がプロジェクト内を指す場合の包含ブロック、`config` 破損で `Repository::open_bare()` が失敗する bare リポジトリでも HEAD 削除と全体再帰削除をブロックすること、厳格モードで配下に staged/worktree の削除を含むディレクトリの `-r` 削除をブロックすること（別ディレクトリの削除では clean なディレクトリの削除を阻害しない）、設定パスが最終・中間コンポーネントの dangling symlink のとき strict モードへフォールバックすること、allowed_paths 境界をまたぐ symlink の双方向（許可内から外を指すリンク、許可外から内を指すリンク）をいずれも削除せずブロックすること、`.` / `..` operand の拒否（非 Git ツリーで `-r .` がカレントディレクトリを消さないこと、Git リポジトリの孫ディレクトリからの `-r ..` が親を消さないこと、`-rf .` の force がバイパスにならないこと、`-r sub/..` で allowed_paths ディレクトリ全体を消せないこと、`.hidden` / `...` のような dot 始まりのファイル名は削除できること、`-r sub` のようなディレクトリ名の明示指定は従来どおり削除できること））

- **プロジェクト構成テスト**: 1件の回帰テストで、ローカルの `release` ターゲットが `cargo build --locked --release` を維持し、`make release` と `make install` が依存解決を暗黙に書き換えないことを検証

## コントリビューション

コントリビューションを歓迎します！お気軽にプルリクエストを送信してください。

## セキュリティ

セキュリティの脆弱性を発見した場合は、[GitHub Issues](https://github.com/owayo/safe-rm/issues) から報告してください。

## ライセンス

[MIT](LICENSE)
