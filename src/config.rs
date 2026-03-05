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
/// # Allow deletion of any file within the current project (Git repository)
/// # without requiring the file to be committed or ignored.
/// # Containment check is still enforced (cannot delete outside project).
/// allow_project_deletion = true
///
/// [[allowed_paths]]
/// path = "/Users/owa/.claude/skills"
/// recursive = true
///
/// [[allowed_paths]]
/// path = "/tmp/logs"
/// recursive = false  # only direct children
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
    pub fn config_path() -> Option<PathBuf> {
        if let Ok(path) = std::env::var("SAFE_RM_CONFIG") {
            return Some(PathBuf::from(path));
        }
        dirs::home_dir().map(|d| d.join(".config").join("safe-rm").join("config.toml"))
    }

    /// デフォルトパスから設定を読み込み
    pub fn load() -> Self {
        Self::load_from_path(Self::config_path())
    }

    /// 指定パスから設定を読み込み
    pub fn load_from_path(path: Option<PathBuf>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };

        if !path.exists() {
            return Self::default();
        }

        match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str::<Config>(&content) {
                Ok(mut config) => {
                    config.resolve_allowed_paths();
                    config
                }
                Err(e) => {
                    eprintln!(
                        "safe-rm: warning: config parse error ({}): {}",
                        path.display(),
                        e
                    );
                    Self::default()
                }
            },
            Err(e) => {
                eprintln!(
                    "safe-rm: warning: cannot read config ({}): {}",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    /// Pre-resolve allowed paths at load time (performance optimization)
    /// Also used in tests to resolve paths after manual Config construction.
    pub fn resolve_allowed_paths(&mut self) {
        self.allowed_paths_resolved = self
            .allowed_paths
            .iter()
            .map(|entry| {
                let expanded = Self::expand_tilde(&entry.path);
                let canonical = std::fs::canonicalize(&expanded).unwrap_or(expanded);
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

        // シンボリックリンク解決のため canonicalize を試行
        let target_resolved =
            std::fs::canonicalize(&target_normalized).unwrap_or(target_normalized);

        // 事前解決済みパスを使用（ここでは canonicalize を呼ばない — ロード時に完了済み）
        for entry in &self.allowed_paths_resolved {
            if entry.recursive {
                // 再帰: ターゲットは許可パス配下の任意の場所に存在可能
                if target_resolved.starts_with(&entry.canonical_path) {
                    return true;
                }
            } else {
                // 非再帰: ターゲットは許可パスの直接の子でなければならない
                if let Some(parent) = target_resolved.parent() {
                    if parent == entry.canonical_path {
                        return true;
                    }
                }
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
        // Empty config should default to allow_project_deletion = true
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
        let config = Config::load_from_path(Some(PathBuf::from("/nonexistent/config.toml")));
        assert!(config.allowed_paths.is_empty());
    }

    #[test]
    fn test_load_none_path() {
        let config = Config::load_from_path(None);
        assert!(config.allowed_paths.is_empty());
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
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::write(tmp.path(), "invalid[[[toml").unwrap();
        let config = Config::load_from_path(Some(tmp.path().to_path_buf()));
        assert!(config.allowed_paths.is_empty());
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

    // --- recursive = true tests ---

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

    // --- recursive = false tests ---

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

        // Nested file should NOT be allowed with recursive = false
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

        // Direct child directory is allowed
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

        // Deep subdirectory should NOT be allowed
        assert!(!config.is_path_allowed(&deep));
    }

    // --- Other tests ---

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
                    recursive: false, // only direct children
                },
                AllowedPathEntry {
                    path: dir_b.to_string_lossy().to_string(),
                    recursive: true, // all nested
                },
            ],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        assert!(config.is_path_allowed(&file_a)); // direct child of dir_a
        assert!(config.is_path_allowed(&nested_b)); // nested in dir_b (recursive)
        assert!(!config.is_path_allowed(&tmp_dir.path().join("dir-c").join("file.txt")));
    }

    #[test]
    fn test_config_path_location() {
        // SAFE_RM_CONFIG が設定されている場合はその値が返るためスキップ
        if std::env::var("SAFE_RM_CONFIG").is_ok() {
            return;
        }
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

    // --- Tilde expansion tests ---

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
        // ~ in the middle should not be expanded
        let expanded = Config::expand_tilde("/tmp/~user/dir");
        assert_eq!(expanded, PathBuf::from("/tmp/~user/dir"));
    }

    #[test]
    fn test_tilde_path_allowed_recursive() {
        // Create a directory under home to test tilde expansion
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

        assert!(config.is_path_allowed(&child_file)); // direct child OK
        assert!(!config.is_path_allowed(&nested_file)); // nested blocked
    }

    // --- SAFE_RM_CONFIG environment variable tests ---

    #[test]
    fn test_config_path_uses_env_var() {
        // Save original value and set test value
        let original = std::env::var("SAFE_RM_CONFIG").ok();
        // SAFETY: Tests run single-threaded with --test-threads=1 or serially
        unsafe {
            std::env::set_var("SAFE_RM_CONFIG", "/custom/path/config.toml");
        }

        let path = Config::config_path();
        assert_eq!(path, Some(PathBuf::from("/custom/path/config.toml")));

        // Restore original value
        // SAFETY: Tests run single-threaded
        unsafe {
            if let Some(val) = original {
                std::env::set_var("SAFE_RM_CONFIG", val);
            } else {
                std::env::remove_var("SAFE_RM_CONFIG");
            }
        }
    }

    #[test]
    fn test_config_path_env_var_precedence() {
        // Env var should take precedence over default path
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let content = r#"
allow_project_deletion = false

[[allowed_paths]]
path = "/custom/via/env"
recursive = true
"#;
        fs::write(tmp.path(), content).unwrap();

        let original = std::env::var("SAFE_RM_CONFIG").ok();
        // SAFETY: Tests run single-threaded
        unsafe {
            std::env::set_var("SAFE_RM_CONFIG", tmp.path());
        }

        let config = Config::load();
        assert!(!config.allow_project_deletion);
        assert_eq!(config.allowed_paths.len(), 1);
        assert_eq!(config.allowed_paths[0].path, "/custom/via/env");

        // Restore
        // SAFETY: Tests run single-threaded
        unsafe {
            if let Some(val) = original {
                std::env::set_var("SAFE_RM_CONFIG", val);
            } else {
                std::env::remove_var("SAFE_RM_CONFIG");
            }
        }
    }

    // --- Pre-resolved paths tests ---

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

        // Verify that resolve populates allowed_paths_resolved
        assert_eq!(config.allowed_paths_resolved.len(), 1);
        // Canonical path should be resolvable
        assert!(
            config.allowed_paths_resolved[0]
                .canonical_path
                .is_absolute()
        );
    }

    #[test]
    fn test_resolve_allowed_paths_fallback_nonexistent() {
        // Non-existent paths should use expanded path as fallback
        let nonexistent = "/nonexistent/path/that/does/not/exist";
        let mut config = Config {
            allowed_paths: vec![AllowedPathEntry {
                path: nonexistent.to_string(),
                recursive: true,
            }],
            ..Default::default()
        };
        config.resolve_allowed_paths();

        // Should fallback to expanded path (no panic)
        assert_eq!(config.allowed_paths_resolved.len(), 1);
        assert_eq!(
            config.allowed_paths_resolved[0].canonical_path,
            PathBuf::from(nonexistent)
        );
    }

    #[test]
    fn test_is_path_allowed_nonexistent_file_in_allowed_dir() {
        let tmp_dir = tempfile::tempdir().unwrap();
        // Use canonical path to avoid macOS /var → /private/var mismatch
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

        // Non-existent file in allowed dir should still match
        // (canonicalize falls back to un-canonicalized path, but parent is canonical)
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

        // non-recursive ではディレクトリ自体は parent チェックで一致しない
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

        // allowed_path内のファイルが許可されることを検証
        let test_file = allowed_dir.join("file.txt");
        fs::write(&test_file, b"content").unwrap();

        assert!(config.is_path_allowed(&test_file));
    }
}
