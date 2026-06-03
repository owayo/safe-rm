//! safe-rm の設定管理
//!
//! `~/.config/safe-rm/config.toml` からユーザー設定を読み込む。
//! 指定ディレクトリの安全チェックをバイパスする allowed_paths をサポート。

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// 設定構造体
///
/// config.toml の例:
/// ```toml
/// # 現在のプロジェクト（Git リポジトリ）内なら、
/// # コミット済みや ignore 済みでなくても削除を許可する。
/// # ただし包含チェックは維持され、プロジェクト外は削除できない。
/// allow_project_deletion = true
///
/// [[allowed_paths]]
/// path = "/Users/owa/.claude/skills"
/// recursive = true
///
/// [[allowed_paths]]
/// path = "/tmp/logs"
/// recursive = false  # 直下の子のみ許可
/// ```
/// デフォルト値 true を返すヘルパー関数
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// true の場合、プロジェクト内の任意のファイルを Git ステータスチェックなしで削除可能。
    /// 包含検証は引き続き適用。デフォルト: true
    #[serde(default = "default_true")]
    pub allow_project_deletion: bool,

    /// 許可パスエントリのリスト
    #[serde(default)]
    pub allowed_paths: Vec<AllowedPathEntry>,

    /// 事前解決済み許可パス（パフォーマンスのためロード時に canonicalize 済み）
    #[serde(skip)]
    allowed_paths_resolved: Vec<AllowedPathResolved>,
}

/// 事前解決済み許可パスエントリ（高速検索のため canonicalize 済み）
#[derive(Debug, Clone)]
struct AllowedPathResolved {
    /// canonicalize 済みパス（失敗時は展開パスにフォールバック）
    canonical_path: PathBuf,
    /// true の場合、全ファイル/サブディレクトリを再帰的に許可
    recursive: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            allow_project_deletion: true,
            allowed_paths: Vec::new(),
            allowed_paths_resolved: Vec::new(),
        }
    }
}

impl Config {
    /// 設定ファイルの読込/解析に失敗した場合のフォールバック設定。
    ///
    /// 既に設定ファイルが置かれている環境で `allow_project_deletion = false`
    /// を意図していた可能性があるため、安全側の strict 設定に倒す。
    /// 設定ファイル自体が存在しない場合は通常の `default()` が使われる。
    fn fail_closed_default() -> Self {
        Self {
            allow_project_deletion: false,
            allowed_paths: Vec::new(),
            allowed_paths_resolved: Vec::new(),
        }
    }
}

/// ディレクトリごとの設定を持つ許可パスエントリ
#[derive(Debug, Clone, Deserialize)]
pub struct AllowedPathEntry {
    /// 削除を許可するディレクトリパス
    pub path: String,
    /// true の場合、全ファイル/サブディレクトリを再帰的に許可。
    /// false の場合、直下の子のみ許可。
    #[serde(default)]
    pub recursive: bool,
}

impl Config {
    /// 設定ファイルパスを取得: ~/.config/safe-rm/config.toml
    ///
    /// safe-kill 等の CLI ツールとの一貫性のため、全プラットフォームで
    /// XDG スタイルパス (~/.config/) を使用。
    ///
    /// SAFE_RM_CONFIG 環境変数が設定されている場合はそのパスを使用。
    /// Unix では非 UTF-8 の値もあり得るため、OsString として受け取る。
    pub fn config_path() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("SAFE_RM_CONFIG") {
            return Some(PathBuf::from(path));
        }
        dirs::home_dir().map(|d| d.join(".config").join("safe-rm").join("config.toml"))
    }

    /// デフォルトパスから設定を読み込み
    pub fn load() -> Self {
        Self::load_from_path(Self::config_path())
    }

    /// 指定パスから設定を読み込み
    ///
    /// `read_to_string()` を直接呼ぶことで、`Path::exists()` が権限エラーで
    /// `false` を返し permissive default に倒れてしまう経路を避ける。
    /// `ErrorKind::NotFound` の場合のみファイル不在として permissive default を返し、
    /// それ以外の I/O エラー（権限、I/O 失敗等）は fail-closed で strict モードへ倒す。
    pub fn load_from_path(path: Option<PathBuf>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };

        match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str::<Config>(&content) {
                Ok(mut config) => {
                    config.resolve_allowed_paths();
                    config
                }
                Err(e) => {
                    // fail-closed: 設定ファイルが壊れているときは strict モードへ倒す。
                    // permissive default に戻すと、利用者が意図した strict 設定が
                    // 一部の構文エラーで無効化されてしまうため。
                    eprintln!(
                        "safe-rm: warning: config parse error ({}): {}; falling back to strict mode (allow_project_deletion = false)",
                        path.display(),
                        e
                    );
                    Self::fail_closed_default()
                }
            },
            // ファイル不在のみ permissive default にフォールバック
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            // 権限エラー等のその他の I/O エラーは fail-closed
            Err(e) => {
                eprintln!(
                    "safe-rm: warning: cannot read config ({}): {}; falling back to strict mode (allow_project_deletion = false)",
                    path.display(),
                    e
                );
                Self::fail_closed_default()
            }
        }
    }

    /// allowed_paths をロード時に事前解決する（性能最適化）
    /// 手動で Config を組み立てるテストでも同じ解決処理に使う。
    pub fn resolve_allowed_paths(&mut self) {
        self.allowed_paths_resolved = self
            .allowed_paths
            .iter()
            .map(|entry| {
                let expanded = Self::expand_tilde(&entry.path);
                let canonical = Self::try_canonicalize(&expanded);
                AllowedPathResolved {
                    canonical_path: canonical,
                    recursive: entry.recursive,
                }
            })
            .collect();
    }

    /// チルダ（~）プレフィックスをユーザーのホームディレクトリに展開
    fn expand_tilde(path: &str) -> PathBuf {
        if path == "~" {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"))
        } else if let Some(rest) = path.strip_prefix("~/") {
            dirs::home_dir()
                .map(|home| home.join(rest))
                .unwrap_or_else(|| PathBuf::from(path))
        } else {
            PathBuf::from(path)
        }
    }

    /// 可能であれば canonicalize する。
    /// 末尾が未作成で失敗した場合は、既存の親ディレクトリまで canonicalize してから
    /// 未作成部分を再結合する。
    fn try_canonicalize(path: &Path) -> PathBuf {
        if let Ok(canonical) = std::fs::canonicalize(path) {
            return canonical;
        }

        let mut current = path;
        let mut missing_segments = Vec::new();

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
    /// 削除対象が symlink の場合、`remove_*` はリンクを辿らずリンクエントリ自体を
    /// 削除する。許可判定を「実際に削除されるエントリ」の位置で行うために、末尾
    /// コンポーネントは canonicalize しない。中間 symlink（エイリアス）は解決する。
    fn canonicalize_parent_keep_filename(path: &Path) -> PathBuf {
        let Some(file_name) = path.file_name() else {
            return Self::try_canonicalize(path);
        };
        let Some(parent) = path.parent() else {
            return path.to_path_buf();
        };
        Self::try_canonicalize(parent).join(file_name)
    }

    /// パスが許可ディレクトリ内にあるかチェック
    ///
    /// 指定パスが allowed_paths のいずれかのエントリに一致する場合 true を返す。
    /// 各エントリの `recursive` フラグを考慮。
    /// パフォーマンスのため事前解決済みパスを使用。
    pub fn is_path_allowed(&self, target: &Path) -> bool {
        if self.allowed_paths_resolved.is_empty() {
            return false;
        }

        // ターゲットパスを正規化（可能であれば絶対パスに解決）
        let target_normalized = if target.is_absolute() {
            target.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(target))
                .unwrap_or_else(|_| target.to_path_buf())
        };

        // 削除対象エントリ（末尾 symlink を辿らない位置）と、末尾まで解決した実体の
        // 両方を求める。削除は末尾エントリ自体に対して行われるため、エントリ位置が
        // 許可範囲内であることを要求しつつ、許可ディレクトリ外の symlink が許可内を
        // 指すバイパス（エントリは外なのに実体だけ内側）も塞ぐ。中間 symlink（別名）は
        // 両者とも解決され、別名 cwd や /var→/private/var 差異は吸収される。
        let entry_path = Self::canonicalize_parent_keep_filename(&target_normalized);
        let resolved_path = Self::try_canonicalize(&target_normalized);

        let entry_allowed = self
            .allowed_paths_resolved
            .iter()
            .any(|entry| Self::path_matches_allowed_entry(&entry_path, entry));
        let resolved_allowed = self
            .allowed_paths_resolved
            .iter()
            .any(|entry| Self::path_matches_allowed_entry(&resolved_path, entry));

        entry_allowed && resolved_allowed
    }

    /// ターゲットパスが許可エントリに一致するか判定する（recursive フラグを考慮）。
    fn path_matches_allowed_entry(path: &Path, entry: &AllowedPathResolved) -> bool {
        if entry.recursive {
            // 再帰: ターゲットは許可パス配下の任意の場所に存在可能
            path.starts_with(&entry.canonical_path)
        } else if let Some(parent) = path.parent() {
            // 非再帰: ターゲットは許可パスの直接の子でなければならない
            parent == entry.canonical_path
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;
    use std::sync::{LazyLock, Mutex};

    /// `SAFE_RM_CONFIG` を触るテスト同士が並列実行で干渉しないように直列化する。
    static SAFE_RM_CONFIG_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    /// テスト中だけ `SAFE_RM_CONFIG` を差し替え、終了時に元へ戻す。
    struct SafeRmConfigEnvGuard {
        original: Option<OsString>,
    }

    impl SafeRmConfigEnvGuard {
        fn set(value: impl AsRef<std::ffi::OsStr>) -> Self {
            let guard = Self {
                original: std::env::var_os("SAFE_RM_CONFIG"),
            };
            // SAFETY: テスト用ロックで `SAFE_RM_CONFIG` への同時アクセスを防いでいる。
            unsafe {
                std::env::set_var("SAFE_RM_CONFIG", value.as_ref());
            }
            guard
        }

        fn clear() -> Self {
            let guard = Self {
                original: std::env::var_os("SAFE_RM_CONFIG"),
            };
            // SAFETY: テスト用ロックで `SAFE_RM_CONFIG` への同時アクセスを防いでいる。
            unsafe {
                std::env::remove_var("SAFE_RM_CONFIG");
            }
            guard
        }
    }

    impl Drop for SafeRmConfigEnvGuard {
        fn drop(&mut self) {
            // SAFETY: テスト用ロックを保持した状態でのみ生成されるガードであり、
            // 復元時も他テストとの競合は起きない。
            unsafe {
                if let Some(value) = &self.original {
                    std::env::set_var("SAFE_RM_CONFIG", value);
                } else {
                    std::env::remove_var("SAFE_RM_CONFIG");
                }
            }
        }
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.allowed_paths.is_empty());
        assert!(!config.is_path_allowed(Path::new("/tmp/file.txt")));
    }

    #[test]
    fn test_default_config_has_allow_project_deletion_true() {
        let config = Config::default();
        assert!(
            config.allow_project_deletion,
            "Default allow_project_deletion should be true"
        );
    }

    #[test]
    fn test_parsed_config_defaults_allow_project_deletion_true() {
        // 空設定では allow_project_deletion = true が既定値になる
        let toml_content = "";
        let config: Config = toml::from_str(toml_content).unwrap();
        assert!(
            config.allow_project_deletion,
            "Parsed empty config should have allow_project_deletion = true"
        );
    }

    #[test]
    fn test_explicit_allow_project_deletion_false() {
        let toml_content = "allow_project_deletion = false\n";
        let config: Config = toml::from_str(toml_content).unwrap();
        assert!(
            !config.allow_project_deletion,
            "Explicit false should be respected"
        );
    }

    #[test]
    fn test_explicit_allow_project_deletion_true() {
        let toml_content = "allow_project_deletion = true\n";
        let config: Config = toml::from_str(toml_content).unwrap();
        assert!(
            config.allow_project_deletion,
            "Explicit true should be respected"
        );
    }

    #[test]
    fn test_load_missing_file() {
        // 設定ファイルが存在しない場合は permissive default (allow_project_deletion = true) を返す。
        // fail-closed (parse エラー時) と挙動を区別する必要があるため明示的に検証する。
        let config = Config::load_from_path(Some(PathBuf::from("/nonexistent/config.toml")));
        assert!(config.allowed_paths.is_empty());
        assert!(
            config.allow_project_deletion,
            "存在しない設定ファイルは permissive default にフォールバックすべき"
        );
    }

    #[test]
    fn test_load_none_path() {
        // 設定パスが None の場合も permissive default を返す。
        let config = Config::load_from_path(None);
        assert!(config.allowed_paths.is_empty());
        assert!(
            config.allow_project_deletion,
            "None パスは permissive default にフォールバックすべき"
        );
    }

    #[test]
    fn test_parse_valid_config() {
        let toml_content = r#"
[[allowed_paths]]
path = "/tmp/test-dir"
recursive = true

[[allowed_paths]]
path = "/home/user/.cache"
recursive = false
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        assert_eq!(config.allowed_paths.len(), 2);
        assert_eq!(config.allowed_paths[0].path, "/tmp/test-dir");
        assert!(config.allowed_paths[0].recursive);
        assert_eq!(config.allowed_paths[1].path, "/home/user/.cache");
        assert!(!config.allowed_paths[1].recursive);
    }

    #[test]
    fn test_parse_empty_config() {
        let toml_content = "";
        let config: Config = toml::from_str(toml_content).unwrap();
        assert!(config.allowed_paths.is_empty());
    }

    #[test]
    fn test_parse_invalid_config() {
        // 設定ファイルがパース失敗した場合、fail-closed で strict モード
        // (allow_project_deletion = false) にフォールバックする。
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::write(tmp.path(), "invalid[[[toml").unwrap();
        let config = Config::load_from_path(Some(tmp.path().to_path_buf()));
        assert!(config.allowed_paths.is_empty());
        assert!(
            !config.allow_project_deletion,
            "parse 失敗時は strict モード (allow_project_deletion = false) にフォールバックすべき"
        );
    }

    #[test]
    fn test_load_config_read_error_falls_back_to_strict_mode() {
        // 設定パスがファイルとして読めない場合、存在しない場合とは区別して
        // fail-closed で strict モードにフォールバックする。
        let tmp_dir = tempfile::tempdir().unwrap();

        let config = Config::load_from_path(Some(tmp_dir.path().to_path_buf()));

        assert!(config.allowed_paths.is_empty());
        assert!(
            !config.allow_project_deletion,
            "設定ファイルの読み取り失敗時は strict モード (allow_project_deletion = false) にフォールバックすべき"
        );
    }

    #[test]
    fn test_recursive_default_is_false() {
        let toml_content = r#"
[[allowed_paths]]
path = "/tmp/dir"
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        assert!(!config.allowed_paths[0].recursive);
    }

    // --- recursive = true のテスト ---

    #[test]
    fn test_recursive_allows_direct_child() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let allowed_dir = tmp_dir.path().join("allowed");
        fs::create_dir_all(&allowed_dir).unwrap();
        let child_file = allowed_dir.join("file.txt");
        fs::write(&child_file, "test").unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: true,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        assert!(config.is_path_allowed(&child_file));
    }

    #[test]
    fn test_recursive_allows_nested_child() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let allowed_dir = tmp_dir.path().join("allowed");
        let nested = allowed_dir.join("sub").join("deep");
        fs::create_dir_all(&nested).unwrap();
        let child_file = nested.join("file.txt");
        fs::write(&child_file, "test").unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: true,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        assert!(config.is_path_allowed(&child_file));
    }

    #[test]
    fn test_recursive_allows_subdirectory() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let allowed_dir = tmp_dir.path().join("allowed");
        let sub_dir = allowed_dir.join("subdir");
        fs::create_dir_all(&sub_dir).unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: true,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        assert!(config.is_path_allowed(&sub_dir));
    }

    // --- recursive = false のテスト ---

    #[test]
    fn test_non_recursive_allows_direct_child() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let allowed_dir = tmp_dir.path().join("allowed");
        fs::create_dir_all(&allowed_dir).unwrap();
        let child_file = allowed_dir.join("file.txt");
        fs::write(&child_file, "test").unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: false,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        assert!(config.is_path_allowed(&child_file));
    }

    #[test]
    fn test_non_recursive_blocks_nested_child() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let allowed_dir = tmp_dir.path().join("allowed");
        let nested = allowed_dir.join("sub");
        fs::create_dir_all(&nested).unwrap();
        let nested_file = nested.join("file.txt");
        fs::write(&nested_file, "test").unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: false,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        // recursive = false ではネストしたファイルは許可しない
        assert!(!config.is_path_allowed(&nested_file));
    }

    #[test]
    fn test_non_recursive_allows_direct_subdir() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let allowed_dir = tmp_dir.path().join("allowed");
        let sub_dir = allowed_dir.join("subdir");
        fs::create_dir_all(&sub_dir).unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: false,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        // 直下の子ディレクトリは許可される
        assert!(config.is_path_allowed(&sub_dir));
    }

    #[test]
    fn test_non_recursive_blocks_deep_subdir() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let allowed_dir = tmp_dir.path().join("allowed");
        let deep = allowed_dir.join("a").join("b");
        fs::create_dir_all(&deep).unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: false,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        // 深い階層のサブディレクトリは許可しない
        assert!(!config.is_path_allowed(&deep));
    }

    // --- その他のテスト ---

    #[test]
    fn test_path_not_allowed() {
        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: "/tmp/allowed-dir".to_string(),
                recursive: true,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();
        assert!(!config.is_path_allowed(Path::new("/tmp/other-dir/file.txt")));
    }

    #[test]
    fn test_multiple_entries() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let dir_a = tmp_dir.path().join("dir-a");
        let dir_b = tmp_dir.path().join("dir-b");
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();
        let file_a = dir_a.join("file.txt");
        let nested_b = dir_b.join("sub").join("file.txt");
        fs::write(&file_a, "a").unwrap();
        fs::create_dir_all(dir_b.join("sub")).unwrap();
        fs::write(&nested_b, "b").unwrap();

        let mut config = Config {
            allowed_paths: vec![
                AllowedPathEntry {
                    path: dir_a.to_string_lossy().to_string(),
                    recursive: false, // 直下の子のみ
                },
                AllowedPathEntry {
                    path: dir_b.to_string_lossy().to_string(),
                    recursive: true, // ネストした子も含める
                },
            ],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        assert!(config.is_path_allowed(&file_a)); // dir_a の直下の子
        assert!(config.is_path_allowed(&nested_b)); // dir_b 配下のネストした子（recursive）
        assert!(!config.is_path_allowed(&tmp_dir.path().join("dir-c").join("file.txt")));
    }

    #[test]
    fn test_config_path_location() {
        let _env_lock = SAFE_RM_CONFIG_ENV_LOCK.lock().unwrap();
        let _env_guard = SafeRmConfigEnvGuard::clear();
        let path = Config::config_path();
        if let Some(p) = path {
            assert!(p.to_string_lossy().contains("safe-rm"));
            assert!(p.to_string_lossy().contains("config.toml"));
        }
    }

    #[test]
    fn test_load_from_valid_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let content = r#"
[[allowed_paths]]
path = "/tmp/test"
recursive = true
"#;
        fs::write(tmp.path(), content).unwrap();
        let config = Config::load_from_path(Some(tmp.path().to_path_buf()));
        assert_eq!(config.allowed_paths.len(), 1);
        assert_eq!(config.allowed_paths[0].path, "/tmp/test");
        assert!(config.allowed_paths[0].recursive);
    }

    // --- チルダ展開のテスト ---

    #[test]
    fn test_expand_tilde_home() {
        let expanded = Config::expand_tilde("~");
        let home = dirs::home_dir().unwrap();
        assert_eq!(expanded, home);
    }

    #[test]
    fn test_expand_tilde_with_subpath() {
        let expanded = Config::expand_tilde("~/.claude/skills");
        let home = dirs::home_dir().unwrap();
        assert_eq!(expanded, home.join(".claude").join("skills"));
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        let expanded = Config::expand_tilde("/tmp/test");
        assert_eq!(expanded, PathBuf::from("/tmp/test"));
    }

    #[test]
    fn test_expand_tilde_not_prefix() {
        // 文字列の途中にある `~` は展開しない
        let expanded = Config::expand_tilde("/tmp/~user/dir");
        assert_eq!(expanded, PathBuf::from("/tmp/~user/dir"));
    }

    #[test]
    fn test_tilde_path_allowed_recursive() {
        // チルダ展開を検証するため、ホーム配下にディレクトリを作成
        let home = dirs::home_dir().unwrap();
        let tmp_dir = tempfile::tempdir_in(&home).unwrap();
        let dir_name = tmp_dir.path().file_name().unwrap().to_string_lossy();
        let child_file = tmp_dir.path().join("file.txt");
        fs::write(&child_file, "test").unwrap();

        let tilde_path = format!("~/{}", dir_name);
        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: tilde_path,
                recursive: true,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        assert!(config.is_path_allowed(&child_file));
    }

    #[test]
    fn test_tilde_path_allowed_non_recursive() {
        let home = dirs::home_dir().unwrap();
        let tmp_dir = tempfile::tempdir_in(&home).unwrap();
        let dir_name = tmp_dir.path().file_name().unwrap().to_string_lossy();
        let child_file = tmp_dir.path().join("file.txt");
        fs::write(&child_file, "test").unwrap();
        let nested = tmp_dir.path().join("sub");
        fs::create_dir_all(&nested).unwrap();
        let nested_file = nested.join("deep.txt");
        fs::write(&nested_file, "test").unwrap();

        let tilde_path = format!("~/{}", dir_name);
        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: tilde_path,
                recursive: false,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        assert!(config.is_path_allowed(&child_file)); // 直下の子は許可
        assert!(!config.is_path_allowed(&nested_file)); // ネスト先は拒否
    }

    // --- SAFE_RM_CONFIG 環境変数のテスト ---

    #[test]
    fn test_config_path_uses_env_var() {
        let _env_lock = SAFE_RM_CONFIG_ENV_LOCK.lock().unwrap();
        let _env_guard = SafeRmConfigEnvGuard::set("/custom/path/config.toml");

        let path = Config::config_path();
        assert_eq!(path, Some(PathBuf::from("/custom/path/config.toml")));
    }

    #[cfg(unix)]
    #[test]
    fn test_config_path_uses_non_utf8_env_var() {
        use std::os::unix::ffi::OsStringExt;

        let _env_lock = SAFE_RM_CONFIG_ENV_LOCK.lock().unwrap();
        let raw_path = OsString::from_vec(b"/tmp/safe-rm-\xFF-config.toml".to_vec());
        let expected = PathBuf::from(&raw_path);
        let _env_guard = SafeRmConfigEnvGuard::set(&raw_path);

        let path = Config::config_path();
        assert_eq!(path, Some(expected));
    }

    #[test]
    fn test_config_path_env_var_precedence() {
        // 環境変数はデフォルトパスより優先される
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let content = r#"
allow_project_deletion = false

[[allowed_paths]]
path = "/custom/via/env"
recursive = true
"#;
        fs::write(tmp.path(), content).unwrap();

        let _env_lock = SAFE_RM_CONFIG_ENV_LOCK.lock().unwrap();
        let _env_guard = SafeRmConfigEnvGuard::set(tmp.path());

        let config = Config::load();
        assert!(!config.allow_project_deletion);
        assert_eq!(config.allowed_paths.len(), 1);
        assert_eq!(config.allowed_paths[0].path, "/custom/via/env");
    }

    // --- 事前解決済みパスのテスト ---

    #[test]
    fn test_resolve_allowed_paths_canonicalizes() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let allowed_dir = tmp_dir.path().join("allowed");
        fs::create_dir_all(&allowed_dir).unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: true,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        // resolve_allowed_paths で allowed_paths_resolved が埋まる
        assert_eq!(config.allowed_paths_resolved.len(), 1);
        // canonicalize 後の絶対パスになっている
        assert!(
            config.allowed_paths_resolved[0]
                .canonical_path
                .is_absolute()
        );
    }

    #[test]
    fn test_resolve_allowed_paths_fallback_nonexistent() {
        // 存在しないパスは展開済みパスにフォールバックする
        let nonexistent = "/nonexistent/path/that/does/not/exist";
        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: nonexistent.to_string(),
                recursive: true,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        // 展開済みパスにフォールバックし、panic しない
        assert_eq!(config.allowed_paths_resolved.len(), 1);
        assert_eq!(
            config.allowed_paths_resolved[0].canonical_path,
            PathBuf::from(nonexistent)
        );
    }

    #[test]
    fn test_is_path_allowed_nonexistent_file_in_allowed_dir() {
        let tmp_dir = tempfile::tempdir().unwrap();
        // macOS の /var → /private/var 差異を避けるため canonical path を使う
        let canonical_tmp = tmp_dir.path().canonicalize().unwrap();
        let allowed_dir = canonical_tmp.join("allowed");
        fs::create_dir_all(&allowed_dir).unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: true,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        // 許可ディレクトリ配下の未作成ファイルでも一致する
        // canonicalize は未作成末尾でフォールバックするが、親は canonical なまま
        let nonexistent = allowed_dir.join("does_not_exist.txt");
        assert!(
            config.is_path_allowed(&nonexistent),
            "Non-existent file in allowed dir should be allowed"
        );
    }

    #[test]
    fn test_is_path_allowed_directory_itself() {
        // 許可ディレクトリ自体が対象の場合（recursive=true）
        let tmp_dir = tempfile::tempdir().unwrap();
        let canonical_tmp = tmp_dir.path().canonicalize().unwrap();
        let allowed_dir = canonical_tmp.join("allowed");
        fs::create_dir_all(&allowed_dir).unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: true,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        // 許可ディレクトリ自体は starts_with で一致するため true
        assert!(
            config.is_path_allowed(&allowed_dir),
            "許可ディレクトリ自体は recursive=true の場合に許可されるべき"
        );
    }

    #[test]
    fn test_is_path_allowed_directory_itself_non_recursive() {
        // 許可ディレクトリ自体が対象の場合（recursive=false）
        let tmp_dir = tempfile::tempdir().unwrap();
        let canonical_tmp = tmp_dir.path().canonicalize().unwrap();
        let allowed_dir = canonical_tmp.join("allowed");
        fs::create_dir_all(&allowed_dir).unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: false,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        // 非再帰ではディレクトリ自体は parent チェックで一致しない
        assert!(
            !config.is_path_allowed(&allowed_dir),
            "許可ディレクトリ自体は recursive=false の場合に許可されないべき"
        );
    }

    #[test]
    fn test_load_from_path_pre_resolves_allowed_paths() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let allowed_dir = tmp_dir.path().join("allowed_dir");
        fs::create_dir_all(&allowed_dir).unwrap();

        let config_file = tmp_dir.path().join("config.toml");
        let content = format!(
            r#"
[[allowed_paths]]
path = "{}"
recursive = true
"#,
            allowed_dir.to_string_lossy()
        );
        fs::write(&config_file, content).unwrap();

        let config = Config::load_from_path(Some(config_file));

        // allowed_path 内のファイルが許可されることを検証
        let test_file = allowed_dir.join("file.txt");
        fs::write(&test_file, b"content").unwrap();

        assert!(config.is_path_allowed(&test_file));
    }

    #[test]
    #[cfg(unix)]
    fn test_is_path_allowed_nonexistent_file_via_symlink_alias() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let allowed_dir = tmp_dir.path().join("allowed");
        fs::create_dir_all(&allowed_dir).unwrap();

        let alias_holder = tempfile::tempdir().unwrap();
        let allowed_alias = alias_holder.path().join("allowed-link");
        std::os::unix::fs::symlink(&allowed_dir, &allowed_alias).unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: true,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        let nonexistent = allowed_alias.join("missing.txt");
        assert!(
            config.is_path_allowed(&nonexistent),
            "未作成ファイルでも symlink 別名経由なら許可パスとして一致するべき"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_is_path_allowed_rejects_outside_symlink_into_allowed_dir() {
        // 許可ディレクトリ外にある symlink が、許可ディレクトリ内の実体を指すケース。
        // `remove_*` はリンクを辿らず symlink エントリ自体（許可ディレクトリ外）を
        // 削除するため、実体が許可内でも「削除されるエントリ」は許可範囲外として拒否する。
        let tmp_dir = tempfile::tempdir().unwrap();
        let canonical_tmp = tmp_dir.path().canonicalize().unwrap();
        let allowed_dir = canonical_tmp.join("allowed");
        fs::create_dir_all(&allowed_dir).unwrap();
        let inside = allowed_dir.join("inside.txt");
        fs::write(&inside, "x").unwrap();

        // 許可ディレクトリ外に、許可ディレクトリ内を指す symlink を作成
        let outside_link = canonical_tmp.join("outside-link.txt");
        std::os::unix::fs::symlink(&inside, &outside_link).unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: true,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        assert!(
            !config.is_path_allowed(&outside_link),
            "許可ディレクトリ外の symlink はリンク先が許可内でも許可されないべき"
        );
    }

    #[test]
    fn test_is_path_allowed_relative_path() {
        // 相対パス指定時に cwd と結合して許可判定される
        let tmp_dir = tempfile::tempdir().unwrap();
        let canonical_tmp = tmp_dir.path().canonicalize().unwrap();
        let allowed_dir = canonical_tmp.join("allowed");
        fs::create_dir_all(&allowed_dir).unwrap();

        // 許可ディレクトリ直下にファイルを作成
        let child_file = allowed_dir.join("file.txt");
        fs::write(&child_file, "test").unwrap();

        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: allowed_dir.to_string_lossy().to_string(),
                recursive: true,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        // 絶対パスで正しく判定されることを確認
        assert!(config.is_path_allowed(&child_file));
    }

    #[test]
    fn test_try_canonicalize_existing_path() {
        // 存在するパスは canonicalize 成功する
        let tmp_dir = tempfile::tempdir().unwrap();
        let dir_path = tmp_dir.path().canonicalize().unwrap();

        let result = Config::try_canonicalize(&dir_path);
        assert_eq!(result, dir_path);
    }

    #[test]
    fn test_try_canonicalize_partial_existing() {
        // 既存ディレクトリ + 未作成セグメント
        let tmp_dir = tempfile::tempdir().unwrap();
        let canonical_tmp = tmp_dir.path().canonicalize().unwrap();

        let missing_path = canonical_tmp.join("nonexistent").join("deep.txt");
        let result = Config::try_canonicalize(&missing_path);

        // canonical_tmp は解決済みなので、結果はそこから再結合される
        assert!(result.starts_with(&canonical_tmp));
        assert!(
            result.ends_with("nonexistent/deep.txt") || result.ends_with("nonexistent\\deep.txt")
        );
    }

    #[test]
    fn test_is_path_allowed_empty_resolved_paths() {
        // allowed_paths_resolved が空の場合、常に false を返す
        let config = Config {
            allowed_paths: Vec::new(),
            ..Default::default()
        };
        // resolve_allowed_paths を呼ばなくても allowed_paths_resolved は空のまま
        assert!(!config.is_path_allowed(Path::new("/tmp/any/file.txt")));
        assert!(!config.is_path_allowed(Path::new("/usr/local/bin/tool")));
        assert!(!config.is_path_allowed(Path::new("relative/path.rs")));
    }

    #[test]
    fn test_expand_tilde_only_home() {
        // "~" のみの展開がホームディレクトリになることを確認（正常系）
        let expanded = Config::expand_tilde("~");
        let home = dirs::home_dir().unwrap();
        assert_eq!(expanded, home);
        // ホームディレクトリは絶対パスである
        assert!(expanded.is_absolute());
        // "~/" 付きの展開結果と整合性がある
        let expanded_with_slash = Config::expand_tilde("~/");
        assert_eq!(expanded_with_slash, home.join(""));
    }

    #[test]
    fn test_try_canonicalize_all_missing_segments() {
        // 全セグメントが存在しない場合、元のパスがそのまま返される
        let path = Path::new("/nonexistent_root_xyz/a/b");
        let result = Config::try_canonicalize(path);
        assert_eq!(result, path.to_path_buf());
    }

    #[test]
    fn test_parse_unknown_fields_are_ignored() {
        // 将来の設定フィールド追加に対する前方互換性を検証
        let toml_content = r#"
allow_project_deletion = false
unknown_future_field = "should be ignored"
another_unknown = 42

[[allowed_paths]]
path = "/tmp/test"
recursive = true
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        assert!(!config.allow_project_deletion);
        assert_eq!(config.allowed_paths.len(), 1);
    }

    #[test]
    fn test_parse_strict_mode_only() {
        // allow_project_deletion = false のみで allowed_paths なしの設定
        let toml_content = "allow_project_deletion = false\n";
        let config: Config = toml::from_str(toml_content).unwrap();
        assert!(!config.allow_project_deletion);
        assert!(config.allowed_paths.is_empty());
    }

    #[test]
    fn test_is_path_allowed_no_match_among_entries() {
        // 複数エントリがあるが、いずれにもマッチしないパスは拒否される
        let tmp1 = tempfile::TempDir::new().unwrap();
        let tmp2 = tempfile::TempDir::new().unwrap();
        let canonical1 = tmp1.path().canonicalize().unwrap();
        let canonical2 = tmp2.path().canonicalize().unwrap();

        let mut config = Config {
            allowed_paths: vec![
                AllowedPathEntry {
                    path: canonical1.to_string_lossy().to_string(),
                    recursive: true,
                },
                AllowedPathEntry {
                    path: canonical2.to_string_lossy().to_string(),
                    recursive: false,
                },
            ],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        // どのエントリにもマッチしないパス
        assert!(!config.is_path_allowed(Path::new("/completely/unrelated/path")));
    }
}
