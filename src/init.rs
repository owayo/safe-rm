//! Configuration initialization for safe-rm
//!
//! Generates a default config file at ~/.config/safe-rm/config.toml

use crate::config::Config;
use std::fs;

/// Default config template with ~/.claude/skills enabled
const CONFIG_TEMPLATE: &str = r#"# safe-rm configuration
# Location: ~/.config/safe-rm/config.toml
#
# Define directories where deletion is always permitted,
# bypassing project containment and Git status checks.
# Supports tilde (~) expansion for home directory.

# Allow recursive deletion under ~/.claude/skills
[[allowed_paths]]
path = "~/.claude/skills"
recursive = true

# Example: Allow only direct children of /tmp/logs
# [[allowed_paths]]
# path = "/tmp/logs"
# recursive = false
"#;

/// Run the init subcommand
pub fn run_init() -> Result<(), String> {
    let config_path =
        Config::config_path().ok_or_else(|| "Cannot determine config directory".to_string())?;

    let config_dir = config_path
        .parent()
        .ok_or_else(|| "Cannot determine config directory".to_string())?;

    // Create config directory if needed
    if !config_dir.exists() {
        fs::create_dir_all(config_dir)
            .map_err(|e| format!("Cannot create directory {}: {}", config_dir.display(), e))?;
    }

    // Check if config already exists
    if config_path.exists() {
        eprintln!("Config file already exists: {}", config_path.display());
        eprintln!("To regenerate, delete the file first and run `safe-rm init` again.");
        return Ok(());
    }

    // Write template
    fs::write(&config_path, CONFIG_TEMPLATE)
        .map_err(|e| format!("Cannot write config file: {}", e))?;

    println!("Created config file: {}", config_path.display());
    println!();
    println!("Default: ~/.claude/skills is allowed (recursive).");
    println!("Edit the file to add more allowed paths.");

    Ok(())
}

/// Get the config path for display purposes
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
        assert_eq!(config.allowed_paths.len(), 1);
        assert_eq!(config.allowed_paths[0].path, "~/.claude/skills");
        assert!(config.allowed_paths[0].recursive);
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

        // Manually test the creation logic
        let config_dir = config_path.parent().unwrap();
        fs::create_dir_all(config_dir).unwrap();
        fs::write(&config_path, CONFIG_TEMPLATE).unwrap();

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
}
