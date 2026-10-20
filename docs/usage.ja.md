# 使用例

削除が許可される操作とブロックされる操作の例です。厳格モードと書いた例は `allow_project_deletion = false` を前提にしています。デフォルトモードでは、プロジェクト内の変更済みファイルや未追跡ファイルも削除できます。基本のフラグは [README](../README.ja.md#使い方) にあります。

## 許可される操作

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

# ignore 済みの成果物内に Git 管理ディレクトリがある場合は全件を明示する
safe-rm -r --allow-nested-git target/vendor/.git target

# 異常終了した git が残した空の .git/index.lock
# （worktree のルートで、git の処理が動いていないことを確かめてから実行する）
safe-rm git-lock --repo . --assume-quiescent
# Exit 0, "removed: /path/to/repo/.git/index.lock (git index lock; quiescence asserted)" を表示

# gitdir を直接指定してもよい（ここでは linked worktree の gitdir）。-n は検査だけを行う
safe-rm git-lock --repo /path/to/repo/.git/worktrees/feature --assume-quiescent -n
# Exit 0, "would remove: /path/to/repo/.git/worktrees/feature/index.lock (git index lock; quiescence asserted)" を表示
```

## ブロックされる操作

```bash
# 変更済みファイル（厳格モード）
safe-rm src/main.rs
# Exit 2: "未コミットの変更があるファイルは削除できません"

# プロジェクト外
safe-rm /etc/passwd
# Exit 2: "プロジェクト外へのアクセスは禁止されています"

safe-rm ../../../etc/hosts
# Exit 2: "プロジェクト外へのアクセスは禁止されています"

# 未追跡ファイル（厳格モード）
safe-rm new_feature.rs
# Exit 2: "未コミットの変更があるファイルは削除できません"

# `.` / `..` operand（POSIX の rm も拒否する形式）
safe-rm -r .
# Exit 2: "末尾が '.' または '..' のパスは削除できません"
safe-rm -r sub/..
# Exit 2: "末尾が '.' または '..' のパスは削除できません"
# 代わりにディレクトリ名を明示する: safe-rm -r sub

# ルート operand（GNU rm も --preserve-root で拒否する形式）
safe-rm -r /
# Exit 2: "ルートディレクトリは削除できません"

# ディレクトリではない対象への末尾セパレータ
safe-rm file.txt/
# Exit 1: "cannot remove 'file.txt/': Not a directory"
safe-rm broken-link/
# Exit 1: "cannot remove 'broken-link/': No such file or directory"
# エントリ自体を消すなら末尾スラッシュを外す: safe-rm file.txt

# ignore 済みの対象内でも Git 管理ディレクトリは既定で保護する
safe-rm -r target
# Exit 2: 検出した Git 管理エントリの件数とパスを表示

# 通常の削除では index.lock も消さない（allowed_paths に .git を書いても同じ）
safe-rm .git/index.lock
# Exit 2: "Git 管理メタデータは削除できません"
# メッセージの最後に、使うべきコマンドを示す:
#   safe-rm git-lock --repo /path/to/repo/.git --assume-quiescent

# --assume-quiescent の無い git-lock
safe-rm git-lock --repo .
# Exit 2: "--assume-quiescent が必要です"

# トップレベルのオプションと git-lock の併用（-n は git-lock の後に付ける）
safe-rm -n git-lock --repo . --assume-quiescent
# Exit 2: "git-lock は -r / -f / -n / --allow-nested-git / PATH と併用できません"

# サブディレクトリから親は探索しない
safe-rm git-lock --repo src --assume-quiescent
# Exit 2: "リポジトリとして開けません"

# 最終更新から min_age_secs（既定 300）秒たっていないロック
safe-rm git-lock --repo . --assume-quiescent
# Exit 2: "index.lock の最終更新から 42 秒しかたっていません（必要なのは 300 秒）"

# 中身のあるロック（git が書き終えていない index を含み得る）
safe-rm git-lock --repo . --assume-quiescent
# Exit 2: "index.lock が空ではありません（1024 バイト）"

# ロックを開いているプロセスがある
safe-rm git-lock --repo . --assume-quiescent
# Exit 2: "index.lock を開いているプロセスがあります（PID: 12345）"

# gitdir に index.lock が無い
safe-rm git-lock --repo . --assume-quiescent
# Exit 1: "cannot remove '/path/to/repo/.git/index.lock': No such file or directory"
```
