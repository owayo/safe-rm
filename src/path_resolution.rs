//! safe-rm のパス解決
//!
//! 包含検証・allowed_paths 判定・Git 管理メタデータ保護が共通で使う canonicalize 処理。
//! いずれもセキュリティ境界の判定に直結するため、実装を 1 箇所にまとめて
//! モジュールごとの挙動の食い違いを防ぐ。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// 可能であれば canonicalize する。
///
/// 末尾が未作成で失敗した場合は、既存の親ディレクトリまで canonicalize してから
/// 未作成部分を再結合する（`/var` と `/private/var` のような別名の差を、未作成の
/// パスでも吸収するため）。どの祖先も解決できなければ入力をそのまま返す。
pub(crate) fn try_canonicalize(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    let mut current = path;
    let mut missing_segments: Vec<OsString> = Vec::new();

    while let Some(parent) = current.parent() {
        if let Some(name) = current.file_name() {
            missing_segments.push(name.to_os_string());
        }

        if let Ok(canonical_parent) = parent.canonicalize() {
            let mut rebuilt = canonical_parent;
            for segment in missing_segments.iter().rev() {
                rebuilt.push(segment);
            }
            return rebuilt;
        }

        current = parent;
    }

    path.to_path_buf()
}

/// 親ディレクトリのみ canonicalize し、末尾コンポーネントは解決せず保持する。
///
/// `remove_file` / `remove_dir` は末尾の symlink を辿らず、リンクエントリ自体を削除する。
/// 境界・許可・Git 管理メタデータの判定を「実際に削除されるエントリ」の位置で行うため、
/// 末尾は canonicalize しない。中間の symlink（エイリアス）は解決するので、中間 symlink
/// 経由の境界脱出や `.git` への到達は引き続き検出できる。
pub(crate) fn canonicalize_parent_keep_filename(path: &Path) -> PathBuf {
    let Some(file_name) = path.file_name() else {
        return try_canonicalize(path);
    };
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };
    try_canonicalize(parent).join(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_try_canonicalize_existing_path() {
        // 存在するパスはそのまま canonicalize される
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path().canonicalize().unwrap();

        assert_eq!(try_canonicalize(&dir_path), dir_path);
    }

    #[test]
    fn test_try_canonicalize_partial_existing() {
        // 既存ディレクトリの下に未作成のセグメントが続く場合は、既存部分だけを解決して再結合する
        let temp_dir = TempDir::new().unwrap();
        let canonical_tmp = temp_dir.path().canonicalize().unwrap();

        let missing_path = canonical_tmp.join("a").join("b").join("c").join("file.txt");
        let result = try_canonicalize(&missing_path);

        assert_eq!(result, missing_path);
        assert!(result.starts_with(&canonical_tmp));
    }

    #[cfg(unix)]
    #[test]
    fn test_try_canonicalize_resolves_alias_of_existing_prefix() {
        // 既存部分の別名（symlink）は未作成のパスでも実体へ解決される
        let temp_dir = TempDir::new().unwrap();
        let canonical_tmp = temp_dir.path().canonicalize().unwrap();
        let real_dir = canonical_tmp.join("real");
        std::fs::create_dir(&real_dir).unwrap();
        let alias = canonical_tmp.join("alias");
        std::os::unix::fs::symlink(&real_dir, &alias).unwrap();

        let result = try_canonicalize(&alias.join("missing.txt"));
        assert_eq!(result, real_dir.join("missing.txt"));
    }

    #[test]
    fn test_try_canonicalize_all_missing_segments() {
        // どの祖先も解決できない場合は入力をそのまま返す
        let path = Path::new("/nonexistent_root_xyz/a/b/file.txt");
        assert_eq!(try_canonicalize(path), path.to_path_buf());
    }

    #[cfg(unix)]
    #[test]
    fn test_canonicalize_parent_keep_filename_keeps_trailing_symlink() {
        // 末尾の symlink はリンク先へ解決せず、リンクエントリの位置を返す
        let temp_dir = TempDir::new().unwrap();
        let canonical_tmp = temp_dir.path().canonicalize().unwrap();
        let target = canonical_tmp.join("target.txt");
        std::fs::write(&target, "x").unwrap();
        let link = canonical_tmp.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert_eq!(canonicalize_parent_keep_filename(&link), link);
    }

    #[cfg(unix)]
    #[test]
    fn test_canonicalize_parent_keep_filename_resolves_intermediate_symlink() {
        // 中間の symlink は実体へ解決し、末尾の名前だけを保持する
        let temp_dir = TempDir::new().unwrap();
        let canonical_tmp = temp_dir.path().canonicalize().unwrap();
        let real_dir = canonical_tmp.join("real");
        std::fs::create_dir(&real_dir).unwrap();
        let alias = canonical_tmp.join("alias");
        std::os::unix::fs::symlink(&real_dir, &alias).unwrap();

        let result = canonicalize_parent_keep_filename(&alias.join("child.txt"));
        assert_eq!(result, real_dir.join("child.txt"));
    }

    #[test]
    fn test_canonicalize_parent_keep_filename_with_missing_parent() {
        // 親が未作成でも、既存の祖先まで解決して末尾の名前を付け直す
        let temp_dir = TempDir::new().unwrap();
        let canonical_tmp = temp_dir.path().canonicalize().unwrap();

        let path = canonical_tmp.join("missing_dir").join("file.txt");
        assert_eq!(canonicalize_parent_keep_filename(&path), path);
    }

    #[test]
    fn test_canonicalize_parent_keep_filename_root() {
        // 末尾の名前を持たないルートは、そのまま解決結果を返す
        let root = Path::new("/");
        assert_eq!(
            canonicalize_parent_keep_filename(root),
            try_canonicalize(root)
        );
    }
}
