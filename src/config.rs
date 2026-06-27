use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Main configuration structure for PSF Guard server
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Server configuration
    pub server: ServerConfig,
    /// Database configuration. Obsolete for server mode (databases come from
    /// the registry, see `db_registry.rs`); kept optional only for backward
    /// compatibility with old TOMLs that still carry a `[database]` section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<DatabaseConfig>,
    /// Image directories configuration. Obsolete for server mode (image dirs
    /// live in the registry); optional for backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<ImagesConfig>,
    /// Cache configuration
    pub cache: CacheConfig,
    /// Optional pregeneration configuration
    pub pregeneration: Option<PregenerationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Port to bind to (default: 3000)
    pub port: Option<u16>,
    /// Host to bind to (default: "0.0.0.0")
    pub host: Option<String>,
    /// Enable CORS (default: true)
    pub cors: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Path to the SQLite database file
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagesConfig {
    /// List of image directories to scan (in priority order)
    pub directories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Cache directory path (default: "./cache")
    pub directory: Option<String>,
    /// File cache TTL as human readable time (default: "5m")
    pub file_ttl: Option<String>,
    /// Directory tree cache TTL as human readable time (default: "5m")  
    pub directory_ttl: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PregenerationConfig {
    /// Enable pregeneration of images (default: false)
    pub enabled: Option<bool>,
    /// Screen resolution pregeneration (default: true if enabled)
    pub screen: Option<bool>,
    /// Large resolution pregeneration (default: false)
    pub large: Option<bool>,
    /// Number of worker threads (default: num_cpus)
    pub workers: Option<usize>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: Some(3000),
            host: Some("0.0.0.0".to_string()),
            cors: Some(true),
        }
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            directory: Some("./cache".to_string()),
            file_ttl: Some("5m".to_string()),
            directory_ttl: Some("5m".to_string()),
        }
    }
}

impl Config {
    /// Load configuration from TOML file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path.as_ref().display()))?;

        let config: Config = toml_edit::de::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.as_ref().display()))?;

        Ok(config)
    }

    /// Save configuration to TOML file
    pub fn to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let toml_string = toml_edit::ser::to_string_pretty(self)
            .context("Failed to serialize configuration to TOML")?;

        std::fs::write(&path, toml_string)
            .with_context(|| format!("Failed to write config file: {}", path.as_ref().display()))?;

        Ok(())
    }

    /// Merge configuration with command line arguments, prioritizing CLI values
    pub fn merge_with_cli(
        &mut self,
        database_path: Option<String>,
        image_dirs: Option<Vec<String>>,
        port: Option<u16>,
        cache_dir: Option<String>,
    ) {
        // CLI database path overrides config (legacy; server mode ignores this)
        if let Some(db_path) = database_path {
            self.database = Some(DatabaseConfig { path: db_path });
        }

        // CLI image directories override config (legacy; server mode ignores this)
        if let Some(dirs) = image_dirs {
            if !dirs.is_empty() {
                self.images = Some(ImagesConfig { directories: dirs });
            }
        }

        // CLI port overrides config
        if let Some(cli_port) = port {
            self.server.port = Some(cli_port);
        }

        // CLI cache directory overrides config
        if let Some(cache) = cache_dir {
            self.cache.directory = Some(cache);
        }
    }

    /// Get the effective values with defaults applied
    pub fn get_port(&self) -> u16 {
        self.server.port.unwrap_or(3000)
    }

    pub fn get_host(&self) -> String {
        self.server
            .host
            .clone()
            .unwrap_or_else(|| "0.0.0.0".to_string())
    }

    pub fn get_cors_enabled(&self) -> bool {
        self.server.cors.unwrap_or(true)
    }

    pub fn get_cache_directory(&self) -> String {
        self.cache
            .directory
            .clone()
            .unwrap_or_else(|| "./cache".to_string())
    }

    pub fn get_file_ttl(&self) -> Duration {
        let ttl_str = self.cache.file_ttl.as_deref().unwrap_or("5m");
        humantime::parse_duration(ttl_str).unwrap_or(Duration::from_secs(300))
    }

    pub fn get_directory_ttl(&self) -> Duration {
        let ttl_str = self.cache.directory_ttl.as_deref().unwrap_or("5m");
        humantime::parse_duration(ttl_str).unwrap_or(Duration::from_secs(300))
    }

    /// Get pregeneration configuration for use with CLI converter
    pub fn get_pregeneration(&self) -> Option<&PregenerationConfig> {
        self.pregeneration.as_ref()
    }

    /// Validate configuration values
    pub fn validate(&self) -> Result<()> {
        // The `[database]` / `[images]` sections are obsolete for server mode
        // (databases come from the registry). Only validate them when present,
        // for the benefit of any legacy caller that still sets them.
        if let Some(database) = &self.database {
            let db_path = Path::new(&database.path);
            if !db_path.exists() {
                return Err(anyhow::anyhow!(
                    "Database file does not exist: {}",
                    database.path
                ));
            }
        }

        if let Some(images) = &self.images {
            if images.directories.is_empty() {
                return Err(anyhow::anyhow!(
                    "At least one image directory must be specified"
                ));
            }

            for dir in &images.directories {
                let path = Path::new(dir);
                if !path.exists() {
                    return Err(anyhow::anyhow!("Image directory does not exist: {}", dir));
                }
                if !path.is_dir() {
                    return Err(anyhow::anyhow!("Image path is not a directory: {}", dir));
                }
            }
        }

        // Validate port range (u16 max is 65535, so only need to check lower bound)
        let port = self.get_port();
        if port < 1024 {
            return Err(anyhow::anyhow!(
                "Port must be 1024 or higher, got: {}",
                port
            ));
        }

        // Validate TTL values by parsing them
        let file_ttl = self.get_file_ttl();
        let dir_ttl = self.get_directory_ttl();
        if file_ttl.is_zero() || dir_ttl.is_zero() {
            return Err(anyhow::anyhow!("Cache TTL values must be greater than 0"));
        }

        // Also validate that the TTL strings are parseable
        if let Some(ref file_ttl_str) = self.cache.file_ttl {
            humantime::parse_duration(file_ttl_str)
                .with_context(|| format!("Invalid file_ttl format: {}", file_ttl_str))?;
        }
        if let Some(ref dir_ttl_str) = self.cache.directory_ttl {
            humantime::parse_duration(dir_ttl_str)
                .with_context(|| format!("Invalid directory_ttl format: {}", dir_ttl_str))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.get_port(), 3000);
        assert_eq!(config.get_host(), "0.0.0.0");
        assert!(config.get_cors_enabled());
        // Database/images are obsolete and default to absent.
        assert!(config.database.is_none());
        assert!(config.images.is_none());
        assert_eq!(config.get_cache_directory(), "./cache");
        assert_eq!(config.get_file_ttl(), Duration::from_secs(300));
        assert_eq!(config.get_directory_ttl(), Duration::from_secs(300));
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let toml_string = toml_edit::ser::to_string_pretty(&config).unwrap();

        // Should contain the live sections; obsolete database/images are
        // skipped when absent (the default).
        assert!(toml_string.contains("[server]"));
        assert!(!toml_string.contains("[database]"));
        assert!(!toml_string.contains("[images]"));
        assert!(toml_string.contains("[cache]"));

        // Parse back
        let parsed: Config = toml_edit::de::from_str(&toml_string).unwrap();
        assert_eq!(parsed.get_port(), config.get_port());
        assert_eq!(parsed.database.is_none(), config.database.is_none());
    }

    #[test]
    fn test_config_parses_without_database_or_images_sections() {
        // A server-mode TOML carries only server/cache/pregeneration knobs;
        // databases come from the registry. This must parse cleanly.
        let toml = r#"
[server]
port = 3002

[cache]
directory = "./cache"
"#;
        let config: Config = toml_edit::de::from_str(toml).unwrap();
        assert_eq!(config.get_port(), 3002);
        assert!(config.database.is_none());
        assert!(config.images.is_none());
    }

    #[test]
    fn test_config_still_parses_legacy_database_section() {
        // Old TOMLs that still carry [database]/[images] must keep loading.
        let toml = r#"
[server]
port = 3000

[database]
path = "/tmp/legacy.sqlite"

[images]
directories = ["/tmp/imgs"]

[cache]
directory = "./cache"
"#;
        let config: Config = toml_edit::de::from_str(toml).unwrap();
        assert_eq!(config.database.unwrap().path, "/tmp/legacy.sqlite");
        assert_eq!(config.images.unwrap().directories, vec!["/tmp/imgs"]);
    }

    #[test]
    fn test_config_merge_with_cli() {
        let mut config = Config::default();

        config.merge_with_cli(
            Some("/new/database.sqlite".to_string()),
            Some(vec!["/new/images1".to_string(), "/new/images2".to_string()]),
            Some(8080),
            Some("/new/cache".to_string()),
        );

        assert_eq!(config.get_port(), 8080);
        assert_eq!(config.get_cache_directory(), "/new/cache");
        assert_eq!(config.database.unwrap().path, "/new/database.sqlite");
        assert_eq!(
            config.images.unwrap().directories,
            vec!["/new/images1", "/new/images2"]
        );
    }

    #[test]
    fn test_config_file_operations() {
        let config = Config::default();
        let temp_file = NamedTempFile::new().unwrap();

        // Save to file
        config.to_file(temp_file.path()).unwrap();

        // Load from file
        let loaded_config = Config::from_file(temp_file.path()).unwrap();
        assert_eq!(loaded_config.get_port(), config.get_port());
        assert_eq!(loaded_config.database.is_none(), config.database.is_none());
        assert_eq!(loaded_config.images.is_none(), config.images.is_none());
    }

    #[test]
    fn test_pregeneration_config_access() {
        let config = Config {
            pregeneration: Some(PregenerationConfig {
                enabled: Some(true),
                screen: Some(false),
                large: Some(true),
                workers: Some(4),
            }),
            ..Default::default()
        };

        let pregen_config = config.get_pregeneration().unwrap();
        assert_eq!(pregen_config.enabled, Some(true));
        assert_eq!(pregen_config.screen, Some(false));
        assert_eq!(pregen_config.large, Some(true));
        assert_eq!(pregen_config.workers, Some(4));
    }

    #[test]
    fn test_humantime_ttl_parsing() {
        let mut config = Config::default();
        config.cache.file_ttl = Some("2h30m".to_string());
        config.cache.directory_ttl = Some("10s".to_string());

        assert_eq!(
            config.get_file_ttl(),
            Duration::from_secs(2 * 3600 + 30 * 60)
        ); // 2h30m
        assert_eq!(config.get_directory_ttl(), Duration::from_secs(10)); // 10s

        // Test invalid format falls back to default
        config.cache.file_ttl = Some("invalid".to_string());
        assert_eq!(config.get_file_ttl(), Duration::from_secs(300)); // Falls back to 5m default
    }

    #[test]
    fn test_config_validation_invalid_ttl() {
        // Need to set valid directories and database for validation to get to TTL check
        let mut config = Config {
            images: Some(ImagesConfig {
                directories: vec!["src".to_string()], // Use src dir which exists
            }),
            database: Some(DatabaseConfig {
                path: "Cargo.toml".to_string(), // Use Cargo.toml which exists
            }),
            ..Default::default()
        };
        config.cache.file_ttl = Some("invalid_format".to_string());

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid file_ttl format"));
    }
}
