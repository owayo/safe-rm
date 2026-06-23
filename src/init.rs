//! safe-rm の設定初期化
//!
//! ~/.config/safe-rm/config.toml にデフォルト設定ファイルを生成する。

use crate::config::Config;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::PathBuf;

/// ~/.claude/skills と /tmp を有効にしたデフォルト設定テンプレート
const CONFIG_TEMPLATE: &str = r#"# safe-rm の設定
# 保存先: ~/.config/safe-rm/config.toml
#
# 常に削除を許可するディレクトリを定義する。
# これらのパスはプロジェクト包含チェックと Git ステータスチェックをバイパスする。
# ホームディレクトリ指定にはチルダ（~）展開を使える。

# ~/.claude/skills 配下を再帰的に許可
[[allowed_paths]]
path = "~/.claude/skills"
recursive = true

# /tmp 配下を再帰的に許可
[[allowed_paths]]
path = "/tmp"
recursive = true

# 例: /tmp/logs の直下の子だけを許可
# [[allowed_paths]]
# path = "/tmp/logs"
# recursive = false
"#;

/// init サブコマンドを実行
pub fn run_init() -> Result<(), String> {
    run_init_at(Config::config_path())
}

/// 指定された設定パスに初期設定を書き込む。
fn run_init_at(config_path: Option<PathBuf>) -> Result<(), String> {
    let config_path = config_path.ok_or_else(|| "Cannot determine config directory".to_string())?;

    let config_dir = config_path
        .parent()
        .ok_or_else(|| "Cannot determine config directory".to_string())?;

    // 必要に応じて設定ディレクトリを作成
    fs::create_dir_all(config_dir)
        .map_err(|e| format!("Cannot create directory {}: {}", config_dir.display(), e))?;

    // dangling symlink も既存エントリとして扱い、リンク先へ書き込まない。
    match fs::symlink_metadata(&config_path) {
        Ok(_) => {
            print_existing_config_message(&config_path);
            return Ok(());
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Cannot inspect config file {}: {}",
                config_path.display(),
                e
            ));
        }
    }

    // race で既存ファイルを上書きしないよう create_new で新規作成する。
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&config_path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            print_existing_config_message(&config_path);
            return Ok(());
        }
        Err(e) => return Err(format!("Cannot write config file: {}", e)),
    };

    if let Err(e) = file.write_all(CONFIG_TEMPLATE.as_bytes()) {
        return Err(format!("Cannot write config file: {}", e));
    }

    if let Err(e) = file.sync_all() {
        return Err(format!("Cannot write config file: {}", e));
    }

    println!("Created config file: {}", config_path.display());
    println!();
    println!("Defaults: ~/.claude/skills and /tmp are allowed (recursive).");
    println!("Edit the file to add more allowed paths.");

    Ok(())
}

/// 設定ファイルが既に存在する場合のメッセージを出力する。
fn print_existing_config_message(config_path: &std::path::Path) {
    eprintln!("Config file already exists: {}", config_path.display());
    eprintln!("To regenerate, delete the file first and run `safe-rm init` again.");
}

/// 表示用の設定パスを取得
pub fn config_path_display() -> String {
    Config::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.config/safe-rm/config.toml".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_template_is_valid_toml() {
        let config: Config = toml::from_str(CONFIG_TEMPLATE).unwrap();
        assert_eq!(config.allowed_paths.len(), 2);
        assert_eq!(config.allowed_paths[0].path, "~/.claude/skills");
        assert!(config.allowed_paths[0].recursive);
        assert_eq!(config.allowed_paths[1].path, "/tmp");
        assert!(config.allowed_paths[1].recursive);
    }

    #[test]
    fn test_config_template_uncommented_is_valid() {
        let uncommented = r#"
[[allowed_paths]]
path = "/Users/you/.claude/skills"
recursive = true

[[allowed_paths]]
path = "/tmp/logs"
recursive = false
"#;
        let config: Config = toml::from_str(uncommented).unwrap();
        assert_eq!(config.allowed_paths.len(), 2);
        assert!(config.allowed_paths[0].recursive);
        assert!(!config.allowed_paths[1].recursive);
    }

    #[test]
    fn test_run_init_creates_file() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let config_path = tmp_dir.path().join("safe-rm").join("config.toml");

        run_init_at(Some(config_path.clone())).unwrap();

        assert!(config_path.exists());
        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("allowed_paths"));
        assert!(content.contains("recursive"));
    }

    #[test]
    fn test_config_path_display_returns_string() {
        let display = config_path_display();
        assert!(display.contains("safe-rm"));
    }

    #[test]
    fn test_run_init_skips_existing_file() {
        // 設定ファイルが既に存在する場合、上書きせず正常終了する
        let tmp_dir = tempfile::tempdir().unwrap();
        let config_dir = tmp_dir.path().join("safe-rm");
        fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.toml");

        // 既存内容で作成
        let existing_content = "# 既存設定\n";
        fs::write(&config_path, existing_content).unwrap();

        run_init_at(Some(config_path.clone())).unwrap();

        let content = fs::read_to_string(&config_path).unwrap();
        assert_eq!(
            content, existing_content,
            "既存ファイルの内容が変更されてはならない"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_run_init_skips_dangling_symlink() {
        // dangling symlink は Path::exists() では false になるが、既存エントリとして
        // 扱わないとリンク先へ新規設定を書き込んでしまう。
        let tmp_dir = tempfile::tempdir().unwrap();
        let config_dir = tmp_dir.path().join("safe-rm");
        fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.toml");
        let dangling_target = tmp_dir.path().join("outside-target.toml");

        std::os::unix::fs::symlink(&dangling_target, &config_path).unwrap();

        run_init_at(Some(config_path.clone())).unwrap();

        assert!(
            fs::symlink_metadata(&config_path)
                .unwrap()
                .file_type()
                .is_symlink(),
            "既存の symlink エントリは残るべき"
        );
        assert!(
            !dangling_target.exists(),
            "dangling symlink のリンク先を作成してはならない"
        );
    }

    #[test]
    fn test_run_init_errors_without_config_path() {
        let error = run_init_at(None).unwrap_err();
        assert_eq!(error, "Cannot determine config directory");
    }

    #[test]
    fn test_config_template_contains_required_fields() {
        // テンプレートに必須フィールドが含まれていることを確認
        assert!(
            CONFIG_TEMPLATE.contains("allowed_paths"),
            "テンプレートに allowed_paths が必要"
        );
        assert!(
            CONFIG_TEMPLATE.contains("recursive"),
            "テンプレートに recursive が必要"
        );
        assert!(
            CONFIG_TEMPLATE.contains("~/.claude/skills"),
            "テンプレートに ~/.claude/skills が必要"
        );
        assert!(
            CONFIG_TEMPLATE.contains("\"/tmp\""),
            "テンプレートに /tmp が必要"
        );
    }
}
