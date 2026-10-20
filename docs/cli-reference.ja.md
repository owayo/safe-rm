# CLI リファレンス

`safe-rm` のオプション、サブコマンド、終了コードの一覧です。設定ファイルは [README](../README.ja.md#設定) と [configuration.ja.md](configuration.ja.md) で説明しています。

## オプション

| オプション | 説明 |
|------------|------|
| `-r, --recursive` | ディレクトリとその中身を削除 |
| `-f, --force` | 存在しないファイルを無視（エラーなし）。`rm -f` と同様に operand なしでの実行も許可する |
| `-n, --dry-run` | 削除せずに削除対象を表示 |
| `--allow-nested-git GIT_PATH` | `-r` と併用し、ignore 済みの単一ディレクトリ内にある Git 管理ディレクトリをパスで明示して許可。検出された各パスについて繰り返し指定する。gitdir ファイル、直接指定した管理パス、現在の repo の管理領域は保護する |
| `-h, --help` | ヘルプを表示 |
| `-V, --version` | バージョンを表示 |

## サブコマンド

| サブコマンド | 説明 |
|------------|------|
| `init` | 設定ファイルを `~/.config/safe-rm/config.toml` に生成 |
| `git-lock` | 異常終了した git が gitdir に残した空の `index.lock` を、静止状態の明示と追加の検査の下で削除。[git-lock](#git-lock) を参照 |

PATH より前に書いた `init` や `git-lock` は、サブコマンドとして解釈します。この名前のファイルを削除するときは、`safe-rm ./git-lock` のようにパスとして書いてください。

## git-lock

```bash
safe-rm git-lock --repo PATH --assume-quiescent [-n | --dry-run]
```

通常の削除では Git 管理メタデータを削除しません。`index.lock` を削除できるのは `git-lock` だけで、削除するのはすべての検査を通った空のロックに限ります。検査の内容と、検査から見えないものは [architecture.ja.md](architecture.ja.md#git-lock-サブコマンド) で説明しています。

| オプション | 説明 |
|------------|------|
| `--repo PATH` | 必須。worktree のルートか gitdir（`.git`、linked worktree の `.git/worktrees/<id>`、submodule の `.git/modules/<name>`）。親ディレクトリは探索せず、`GIT_DIR` などの環境変数も使わない。相対パスはカレントディレクトリを基準に解決する |
| `--assume-quiescent` | 毎回必須。そのリポジトリで git の処理が動いておらず、削除が終わるまで新しい処理も gitdir を書き換えるほかの操作も始めないことを、呼び出し側が保証する指定。safe-rm はこれを確かめきれず、設定ファイルで代わりに指定することもできない |
| `-n, --dry-run` | 削除せずに検査だけを行う。`git-lock` の後に付ける |

トップレベルのオプション（`-r`・`-f`・`-n`・`--allow-nested-git`）や PATH と一緒に指定すると、終了コード 2 で拒否します。`GIT_INDEX_FILE` が設定されているとき（空文字でも）も拒否します。代わりの index を使っていると、gitdir 直下の `index.lock` がその index のロックとは限らないためです。対応する OS は Linux と macOS です。

成功すると、ロックのパス（canonical な gitdir に `index.lock` を付けたもの）を表示します。

```text
removed: /path/to/repo/.git/index.lock (git index lock; quiescence asserted)
```

`-n` のときは、行頭が `would remove:` になります。macOS では、ロックを開いているプロセスの検査で対象外にしたものがあると、標準エラー出力にもう 1 行を出します。出るのは該当する部分だけです。

```text
safe-rm: note: open-handle inspection did not cover N OS platform process(es) that deny inspection (routined, lsd, ...) and M file descriptor(s) whose vnode could not be read; --assume-quiescent remains the guarantee
```

設定ファイルの `[git_locks]` で、`git-lock` を無効にしたり、ロックの最終更新から必要な経過時間（既定 300 秒）を変えたりできます。[configuration.ja.md](configuration.ja.md#git_locks) を参照してください。

## 終了コード

| コード | 意味 | 例 |
|--------|------|-----|
| 0 | 成功 | ファイル削除、ドライラン完了、`git-lock` がロックを削除した、`git-lock` のドライランがすべての検査を通った |
| 1 | 操作エラー | ファイルが見つからない、-rなしでディレクトリ、非ディレクトリへの末尾セパレータ、I/Oエラー、部分的失敗、`git-lock` で `index.lock` が無い、`git-lock` が削除した名前が検査したファイルではなかった |
| 2 | セキュリティブロック | ダーティファイル、プロジェクト外、`.` / `..` やルート operand、ディレクトリ読み取りエラー（fail-closed）、通常の削除で `.git/index.lock` などの Git 管理メタデータを指定、`git-lock` の拒否（`--assume-quiescent` が無い、設定で無効、ロックが空でない・新しすぎる・プロセスが開いている など） |
