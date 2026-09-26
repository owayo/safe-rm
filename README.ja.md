<h1 align="center">safe-rm</h1>

<p align="center">
  Git対応のファイル保護機能を持つAIエージェント向けセキュア削除CLI
</p>

<!-- standard:badges:start -->
<h3 align="center">対応プラットフォーム</h3>

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

`safe-rm` は、AIエージェントがプロジェクト外のファイルや重要な Git 管理メタデータを誤って削除することを防ぐ CLI ツールです。デフォルトではプロジェクト境界を強制し、`.git` などの Git 管理パスをブロックします。**厳格モード**（`allow_project_deletion = false`）ではさらに Git 対応のアクセス制御を有効にし、変更済み・ステージング済み・未追跡ファイルの削除も防止します。

Claude Code などの AI エージェントで `rm` の代わりに使う想定です。`PreToolUse` フックが Bash ツールの `rm` / `rmdir` を拒否し、エージェントに `safe-rm` を使うよう伝えます (設定は [Claude Code 連携](#claude-code-連携) を参照)。

## 機能

- **パス境界チェック**: プロジェクトディレクトリ外のファイル削除をブロック
- **厳格モードの Git ステータス保護**: `allow_project_deletion = false` のとき、変更済み・ステージング済み・未追跡ファイルの削除を防止。判定には削除対象を実際に所有する Git リポジトリを使用し、cwd 由来の checker と対象起点で discover した checker の両方を評価して、対象を含む最も深い workdir を持つ checker を選択する。これにより (a) cwd が非 Git でも対象が配下のネスト Git リポジトリ内にあるケース、(b) cwd 側 repo が `.gitignore` 等でネスト repo を除外していて本来 `Ignored` として素通りするケース、(c) cwd と対象が別の Git リポジトリに属するケースの三種類のバイパスをいずれも fail-closed で塞ぐ。さらに、対象を所有するリポジトリが `core.worktree` 等でワークツリーを別の場所へリダイレクトしていて `workdir()` が対象を含まない場合も、`NotInRepo` への素通りを防ぐため `Modified` 扱いで fail-closed にブロックする
- **Git 管理メタデータ保護**: `.git`、gitdir 参照ファイル、bare リポジトリの管理パス、現在のリポジトリの Git 管理メタデータを含む再帰削除に加え、削除対象自身・パスの任意の中間コンポーネント・再帰削除時に配下に存在する任意階層の Git 管理メタデータを常時ブロック。ネストした `.git` ファイル/ディレクトリだけでなく、`.git` コンポーネントを持たない bare リポジトリも検出。コンポーネント比較は ASCII case-insensitive で、macOS APFS などの大文字小文字を区別しないファイルシステムでの `.GIT` 経由バイパスも防止。中間 symlink が `.git` ディレクトリや bare リポジトリを指している場合（例: `gitlink -> nested/.git` のもとで `gitlink/config` を削除）も、親ディレクトリのみを canonicalize して末尾コンポーネントを保持する判定でブロック（symlink 自身の削除はリンクのみを消すため引き続き許可される）
- **コンフリクト対応のステータス判定**: `Status::CONFLICTED` フラグが立ったファイルは、他の index/worktree フラグが立っていない場合でも厳格モードで `Modified` として扱い、未解決のマージコンフリクトが黙って削除されることを防止
- **ネストしたダーティファイル保護**: 厳格モードでは未追跡ディレクトリ配下、ignored/未追跡が混在するディレクトリ配下、ignored ディレクトリ配下の tracked 変更済みファイルも Git 管理外扱いにせず、正しくブロック
- **ディレクトリトラバーサル防止**: `../` の直前成分が「実体として存在する通常ディレクトリ」でないパスは fail-closed で拒否。`link/../victim`（symlink 中間成分）、`missing/../victim`（存在しない中間成分）、`file/../victim`（通常ファイル中間成分）はいずれも OS の path resolution では失敗するが、字句正規化で `victim` に化けてしまう経路をブロック
- **dangling 中間 symlink ガード**: `dangling/child.txt` のように中間コンポーネントが解決不能な symlink のパスは、メタデータ取得前に fail-closed でブロック。末尾の dangling symlink 自体はリンクエントリだけを削除するため引き続き許可
- **`.` / `..` operand の拒否**: 末尾成分が `.` または `..` の operand（`.`・`./`・`..`・`../`・`sub/.`・`sub/..`・`/abs/path/..`）を拒否。POSIX の rm も同じ operand について診断メッセージを出すだけで一切処理しない。この防御がないと `safe-rm -r .` がカレントディレクトリ自体を、`safe-rm -r ..` が親ディレクトリを削除してしまい、置き換え対象の rm より危険側へ倒れる。判定は正規化・`allowed_paths` 判定・`-f` 処理より前に行うため、許可パスでも force でもバイパスできない。`.hidden`・`...`・`..foo` のような dot で始まる通常のファイル名は従来どおり削除可能。Windows では drive-relative 形式（`C:.` / `C:..`）も拒否
- **空文字 operand の拒否**: 空文字 operand（`safe-rm ""`）は `cwd.join("")` がカレントディレクトリと等価になるため、`-r ""` でカレントディレクトリ自体が消える。rm と同じく `No such file or directory`（exit 1）として先頭で拒否し、`-f` のときは黙って無視する。`rm -f` と同様に、operand を 1 つも指定しない `safe-rm -f` は設定もカレントディレクトリも参照せずに成功する
- **ルート operand の拒否**: ルートディレクトリに解決される operand（`/`・`//`・cwd が `/` のときの `.`）を exit 2 で拒否する。POSIX の rm も、既定で有効な GNU rm の `--preserve-root` も同じ operand を処理しない。この拒否がないと、cwd が `/` の非 Git 環境（root 実行のコンテナ等）ではプロジェクトルートが `/` になって包含検証を通過し、Git 管理メタデータ保護も `/` 自体はカバーしないため、`safe-rm -r /` がファイルシステム全体の削除に入り得る。`.` / `..` の判定と同じく allowed_paths 判定や `-f` より前に評価する
- **末尾セパレータ付き operand の検証**: POSIX では末尾セパレータは「その対象がディレクトリであること」の要求なので、rm はディレクトリとして解決できない operand を削除しない。字句正規化で末尾セパレータが落ちるため、検査しないと `file.txt/` が `file.txt` 自体の削除に、`danglink/`（リンク切れ symlink）がリンクエントリの削除に化ける。正規化前に生 operand を解決し、非ディレクトリなら `Not a directory`、解決できなければ `No such file or directory`（いずれも exit 1、rm と同じく `-f` では黙って無視）を返す。`ELOOP`（循環 symlink）や権限不足は判断不能なので `-f` でも I/O エラーとして伝播させる。ディレクトリへの symlink は引き続きディレクトリとして解決されるため、`link/` は従来どおりリンクエントリのみを削除してリンク先を残す
- **無視ファイルの許可**: `.gitignore` で指定されたファイル（ビルド成果物など）の削除を許可。ただし `.gitignore` に一致していても `git add -f` で強制追跡された（tracked な）ファイルは、未コミット変更があれば保護される — ignore 判定より先にステータスを解決するため、追跡済みの dirty ファイルが `Ignored` と誤判定されることはない
- **シンボリックリンク安全なGitチェック**: ディレクトリ symlink は辿らず、リンク自体として判定
- **非UTF-8パス対応**: Git ステータスキャッシュは生バイト列をキー (`HashMap<Vec<u8>, FileStatus>`) として持ち、`entry.path_bytes()` を直接使うため、非 UTF-8 名のファイルも正しく登録される。これがないと、未追跡ディレクトリ配下の非 UTF-8 未追跡ファイルが `NotInRepo` に落ちて厳格モードでも削除可能になってしまう。`status_should_ignore()` のエラーも握りつぶさずに `Modified` 相当として fail-closed でブロック
- **エイリアスパス耐性（包含検証 + allowed_paths + 厳格モード）**: 包含検証と `allowed_paths` 判定では「既存親ディレクトリまで canonicalize + 未作成部分を再結合」、厳格モードの Git チェックでは「非 symlink パスを canonicalize、symlink パスは親ディレクトリのみ canonicalize + リンク自体を判定」として、別名絶対パス経由のバイパスを防止
- **許可パス設定**: 指定ディレクトリのプロジェクト境界チェックと Git ステータスチェックをバイパス（ディレクトリごとの再帰設定）。現在ディレクトリの Git 検出に失敗しても、許可パス削除はそれだけではブロックされない
- **非Gitサポート**: 非Gitディレクトリでも安全に動作
- **ドライランモード**: 実際に削除せずに削除対象をプレビュー
- **決定的なエラー出力**: 単一パス失敗時は stderr を1回だけ出力し、複数パス実行時は失敗した各パスごとに1回ずつ出力

検査の順序、安全レイヤー、モードごとに削除できる範囲: [docs/architecture.ja.md](docs/architecture.ja.md)

## インストール

<!-- standard:install:start -->
### Cargo

Rust 1.98 以上が必要です。

```bash
cargo install --git https://github.com/owayo/safe-rm --locked
```

### GitHub Releases から

[Releases](https://github.com/owayo/safe-rm/releases/latest) から自分の環境のアーカイブを取得して展開し、`safe-rm` を `PATH` の通った場所に置きます。各リリースには、取得したファイルを確かめるための `SHA256SUMS` も添付しています。

| プラットフォーム | ファイル |
|---|---|
| Linux (x86_64) | `safe-rm-x86_64-unknown-linux-gnu.tar.gz` |
| macOS (Intel) | `safe-rm-x86_64-apple-darwin.tar.gz` |
| macOS (Apple Silicon) | `safe-rm-aarch64-apple-darwin.tar.gz` |

macOS でブラウザから取得した場合は、実行の前に隔離属性を外します: `xattr -d com.apple.quarantine safe-rm`。

### ソースから

[mise](https://mise.jdx.dev/) が必要です (Rust のツールチェーンは `mise.toml` で固定しています)。

```bash
git clone https://github.com/owayo/safe-rm.git
cd safe-rm
make install
```

`make install` は `/usr/local/bin` に入れます。場所を変えるときは `INSTALL_PATH` を指定します (例: `make install INSTALL_PATH="$HOME/.local/bin"`)。
<!-- standard:install:end -->

インストールしたら、[Claude Code 連携](#claude-code-連携) のフックと `CLAUDE.md` のルールを追加してください。設定ファイル (任意) は `safe-rm init` で作れます ([設定](#設定) を参照)。

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

ブロックした削除は終了コード 2 で終わり、理由を標準エラー出力に表示します。

```bash
# プロジェクト外
safe-rm /etc/passwd
# Exit 2: "プロジェクト外へのアクセスは禁止されています"

# `.` / `..` operand（POSIX の rm も拒否する形式）
safe-rm -r .
# Exit 2: "末尾が '.' または '..' のパスは削除できません"
```

厳格モードの例を含む、許可される操作とブロックされる操作の例: [docs/usage.ja.md](docs/usage.ja.md)

すべてのオプション、`init` サブコマンド、終了コード: [docs/cli-reference.ja.md](docs/cli-reference.ja.md)

## 設定

`safe-rm` は `~/.config/safe-rm/config.toml` にオプションの設定ファイルをサポートしています。`SAFE_RM_CONFIG` 環境変数でカスタムパスを指定することもできます。ファイル名だけを含む相対パスは、`safe-rm init` と通常のコマンドのどちらでもカレントディレクトリを基準に解決されます。Unix ではこの環境変数を生の OS パスとして読み取るため、非 UTF-8 パスも保持されます。

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

トップレベルおよび `allowed_paths` 内の未知の設定キーは拒否されます。これにより、安全性に関わる設定名のタイプミスは解析エラーとなり、permissive な既定値を黙って採用せず、fail-closed の厳格モードへフォールバックします。新しいバージョン向けの設定を古い `safe-rm` が読んだ場合も、保護が弱くなるのではなく安全側へ倒れます。

各設定の働き (デフォルトモードと厳格モード、`allowed_paths` と `recursive`、設定ファイルが無いときや壊れているときの扱い) と設定例: [docs/configuration.ja.md](docs/configuration.ja.md)

## Claude Code 連携

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

## 開発

<!-- standard:dev:start -->
[mise](https://mise.jdx.dev/) が必要です。ツールの版は `mise.toml` で固定しています。

```bash
make setup   # ツールチェーン (mise) と依存を取得する
make ci      # CI と同じ検査 (書き換えない)
```

| コマンド | 説明 |
|---|---|
| `make setup` | ツールチェーン (mise) と依存を取得する |
| `make build` | デバッグ版をビルドする |
| `make release` | リリース版をビルドする |
| `make test` | テストを実行する |
| `make lint` | clippy を警告ゼロで通す |
| `make fmt` | コードを整形する (書き換える) |
| `make fmt-check` | 整形済みかを確かめる (書き換えない) |
| `make check` | 整形と静的検査 (書き換えない) |
| `make ci` | CI と同じ検査 (書き換えない) |
| `make install` | リリース版を INSTALL_PATH (既定 /usr/local/bin) に入れる |
| `make uninstall` | INSTALL_PATH から取り除く |
| `make clean` | ビルド成果物を消す |

`make` でターゲットの一覧を表示します。リリースは GitHub Actions で行います (**Actions → Release → Run workflow**)。
<!-- standard:dev:end -->

CI が確かめる内容、リリースの流れ、テストの範囲: [docs/development.ja.md](docs/development.ja.md)

## セキュリティ

セキュリティの脆弱性を発見した場合は、[GitHub Issues](https://github.com/owayo/safe-rm/issues) から報告してください。

## ライセンス

<!-- standard:license:start -->
[MIT](LICENSE)
<!-- standard:license:end -->
