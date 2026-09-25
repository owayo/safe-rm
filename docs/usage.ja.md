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
```
