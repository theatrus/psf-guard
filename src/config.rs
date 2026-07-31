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
    /// Databases this server accepts remote scheduler sync for, one
    /// `[[remote_sync]]` block each. See [`RemoteSyncConfig`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_sync: Vec<RemoteSyncConfig>,
    /// Databases this server accepts remote image uploads for, one
    /// `[[remote_upload]]` block each. See [`RemoteUploadConfig`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_upload: Vec<RemoteUploadConfig>,
}

/// Turn on the remote sync protocol for one already-registered database.
///
/// The desktop app configures this in Settings, but a headless
/// `psf-guard server` has no Settings and should not have to open database
/// management — a far larger grant, since that route lets a network caller
/// name server filesystem paths — merely to accept a sync. An operator with
/// shell access on the box writes this instead.
///
/// It is applied to the in-memory database list at startup and never written
/// back to the registry, so the config file stays the whole truth for a
/// deployment and rotating a token is a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteSyncConfig {
    /// Registry slug of the database to open for sync.
    pub database: String,
    /// Bearer token, in the clear. Prefer `token_file`: this one is readable
    /// by anyone who can read the config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// File holding the bearer token, for systemd credentials, Docker
    /// secrets, and the like. Leading and trailing whitespace is trimmed, so
    /// the usual trailing newline is fine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_file: Option<String>,
}

/// Accept remote image uploads for one already-registered database.
///
/// The image counterpart of [`RemoteSyncConfig`], and separate for the same
/// reason the registry keeps the two grants apart: a telescope that ships
/// frames need not also be allowed to merge into the catalog, or the reverse.
/// A database named by both blocks must use the same key — one key per
/// database is what the token check assumes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteUploadConfig {
    /// Registry slug of the database to receive frames for.
    pub database: String,
    /// Directory the received frames are written to. Created if absent.
    pub image_dir: String,
    /// Bearer token, in the clear. Prefer `token_file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// File holding the bearer token. Whitespace-trimmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_file: Option<String>,
}

/// Read a token supplied either inline or by file, naming the database in any
/// complaint so an operator with several blocks knows which one is wrong.
fn read_token(
    database: &str,
    block: &str,
    inline: &Option<String>,
    file: &Option<String>,
) -> Result<String> {
    match (inline, file) {
        (Some(_), Some(_)) => anyhow::bail!(
            "{block} for database '{database}' sets both token and token_file; use one"
        ),
        (Some(token), None) => Ok(token.trim().to_string()),
        (None, Some(path)) => {
            let contents = std::fs::read_to_string(path).with_context(|| {
                format!("reading the {block} token for database '{database}' from {path}")
            })?;
            Ok(contents.trim().to_string())
        }
        (None, None) => {
            anyhow::bail!("{block} for database '{database}' needs a token or a token_file")
        }
    }
}

/// Open the configured databases for remote access, in memory only.
///
/// Returns a line per database describing what was opened, for the startup
/// log. An unknown slug is fatal: a typo would otherwise leave the operator
/// with a server that answers every remote request with 403 and nothing to
/// say why.
pub fn apply_remote_access(
    entries: &mut [crate::db_registry::DbEntry],
    sync: &[RemoteSyncConfig],
    upload: &[RemoteUploadConfig],
) -> Result<Vec<String>> {
    let mut opened: Vec<(String, Vec<&'static str>)> = Vec::new();
    // Databases this config run has already keyed. The conflict check is
    // between blocks the operator wrote together; replacing a key the desktop
    // Settings panel left in the registry is the config file doing its job.
    let mut keyed: Vec<(String, String)> = Vec::new();
    for config in sync {
        let token = read_token(
            &config.database,
            "remote_sync",
            &config.token,
            &config.token_file,
        )?;
        let access = open(entries, &config.database, &token, &mut keyed)?;
        access.sync_enabled = true;
        note(&mut opened, &config.database, "sync");
    }
    for config in upload {
        let token = read_token(
            &config.database,
            "remote_upload",
            &config.token,
            &config.token_file,
        )?;
        std::fs::create_dir_all(&config.image_dir).with_context(|| {
            format!(
                "creating the remote upload directory {} for database '{}'",
                config.image_dir, config.database
            )
        })?;
        let access = open(entries, &config.database, &token, &mut keyed)?;
        access.enabled = true;
        access.image_dir = config.image_dir.clone();
        note(&mut opened, &config.database, "image upload");
    }
    Ok(opened
        .into_iter()
        .map(|(database, grants)| format!("{database} ({})", grants.join(" + ")))
        .collect())
}

/// Find the named database and set its key, leaving every other setting be.
fn open<'a>(
    entries: &'a mut [crate::db_registry::DbEntry],
    database: &str,
    token: &str,
    keyed: &mut Vec<(String, String)>,
) -> Result<&'a mut crate::db_registry::RemoteImageUploadConfig> {
    // A database has one key. Two blocks naming it with different tokens
    // would leave whichever ran first silently unusable.
    if let Some((_, first)) = keyed.iter().find(|(name, _)| name == database)
        && first != token
    {
        anyhow::bail!(
            "database '{database}' is configured with two different remote keys; \
             a database has one key, used by whichever grants it holds"
        );
    }
    keyed.push((database.to_string(), token.to_string()));
    let known = entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let entry = entries
        .iter_mut()
        .find(|entry| entry.id == database)
        .with_context(|| {
            format!(
                "remote access names database '{database}', which is not registered. \
                 Configured: {known}"
            )
        })?;
    let access = entry
        .remote_image_upload
        .get_or_insert_with(Default::default);
    access
        .set_token(token)
        .with_context(|| format!("remote key for database '{database}'"))?;
    Ok(access)
}

fn note(opened: &mut Vec<(String, Vec<&'static str>)>, database: &str, grant: &'static str) {
    match opened.iter_mut().find(|(name, _)| name == database) {
        Some((_, grants)) => grants.push(grant),
        None => opened.push((database.to_string(), vec![grant])),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Port to bind to (default: 3000)
    pub port: Option<u16>,
    /// Host to bind to (default: "0.0.0.0")
    pub host: Option<String>,
    /// Enable CORS (default: true)
    pub cors: Option<bool>,
    /// Optional browser session policy and bootstrap accounts for server
    /// mode. Managed accounts live in auth.json. Tauri loads neither source
    /// and keeps its localhost server unauthenticated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<ServerAuthConfig>,
    /// Fraction of logical CPU cores interactive, user-triggered work (the
    /// occlusion / spatial scan) may use (0.0–1.0, default 0.5). It runs on
    /// the blocking pool while the server keeps serving the UI, so this leaves
    /// headroom; it is further bounded by available memory and a hard maximum.
    /// `1.0` uses all cores. See `concurrency::WorkerPolicy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_worker_ratio: Option<f64>,
    /// Fraction of logical CPU cores background work (image-preview
    /// pre-generation) may use (0.0–1.0, default 0.25). Kept below
    /// `scan_worker_ratio`; background work additionally pauses entirely while
    /// an interactive scan is running. See `concurrency::WorkerPolicy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_worker_ratio: Option<f64>,
    /// Optional notice shown below the application header on every page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner: Option<SiteBannerConfig>,
    /// File format for generated preview and annotated images: `png`
    /// (default, exact) or `jpeg` (smaller cache, lossy). See
    /// [`crate::preview_format`] for what the trade costs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_format: Option<String>,
    /// JPEG quality, 50–100, when `preview_format = "jpeg"`. Ignored for PNG.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_jpeg_quality: Option<u8>,
    /// Whether one-shot-color frames are shown in colour unless a viewer says
    /// otherwise. Default true.
    ///
    /// This also decides which rendition background pre-generation warms, so
    /// on a site whose observers prefer luminance, setting it false keeps the
    /// warmed cache and the viewer asking for the same thing. Mono rigs are
    /// unaffected either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_color: Option<bool>,
}

/// Two simple browser roles. This is deliberately not an ACL system: one
/// optional viewer credential and one optional editor credential cover the
/// small-server deployment case.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerAuthConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<ServerAuthCredentialConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_write: Option<ServerAuthCredentialConfig>,
    /// Browser session lifetime. Defaults to seven days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_hours: Option<u64>,
    /// Mark the session cookie Secure. Defaults to true; leave it false only
    /// for direct HTTP development servers.
    #[serde(default = "default_secure_cookie")]
    pub secure_cookie: bool,
    /// Allow read-only accounts to start costly derived-data jobs such as
    /// stacks, plate solves, and satellite predictions. Defaults to false.
    #[serde(default)]
    pub allow_read_only_compute: bool,
}

fn default_secure_cookie() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerAuthCredentialConfig {
    pub username: String,
    /// Inline password. Prefer `password_file` for deployed servers.
    #[serde(default, skip_serializing)]
    pub password: Option<String>,
    /// File holding the password. Leading and trailing whitespace is trimmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_file: Option<String>,
}

impl std::fmt::Debug for ServerAuthCredentialConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerAuthCredentialConfig")
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("password_file", &self.password_file)
            .finish()
    }
}

impl ServerConfig {
    /// Whether previews default to colour.
    pub fn preview_color(&self) -> bool {
        self.preview_color.unwrap_or(true)
    }

    /// How generated previews are encoded. An unreadable format name is an
    /// error rather than a silent fall back to PNG: an operator who set it
    /// meant something by it.
    pub fn preview_encoding(&self) -> Result<crate::preview_format::PreviewEncoding> {
        use crate::preview_format::{PreviewEncoding, PreviewFormat, DEFAULT_JPEG_QUALITY};

        let format = match &self.preview_format {
            Some(name) => PreviewFormat::parse(name).context("server.preview_format")?,
            None => return Ok(PreviewEncoding::png()),
        };
        Ok(match format {
            PreviewFormat::Png => PreviewEncoding::png(),
            PreviewFormat::Jpeg => {
                PreviewEncoding::jpeg(self.preview_jpeg_quality.unwrap_or(DEFAULT_JPEG_QUALITY))
            }
        })
    }
}

/// Plain-text site notice configured by the server administrator.
///
/// The frontend never renders these values as HTML. An optional link must use
/// HTTP(S), which prevents a config typo from exposing a script URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteBannerConfig {
    pub title: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_url: Option<String>,
}

impl SiteBannerConfig {
    fn normalized(&self) -> Result<Self> {
        let title = self.title.trim();
        let message = self.message.trim();
        if title.is_empty() {
            return Err(anyhow::anyhow!("server.banner.title must not be empty"));
        }
        if title.chars().count() > 80 {
            return Err(anyhow::anyhow!(
                "server.banner.title must be 80 characters or fewer"
            ));
        }
        if message.is_empty() {
            return Err(anyhow::anyhow!("server.banner.message must not be empty"));
        }
        if message.chars().count() > 500 {
            return Err(anyhow::anyhow!(
                "server.banner.message must be 500 characters or fewer"
            ));
        }

        let link_text = self
            .link_text
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let link_url = self
            .link_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if link_text.is_some() != link_url.is_some() {
            return Err(anyhow::anyhow!(
                "server.banner.link_text and link_url must be set together"
            ));
        }
        if link_text.is_some_and(|value| value.chars().count() > 80) {
            return Err(anyhow::anyhow!(
                "server.banner.link_text must be 80 characters or fewer"
            ));
        }
        if let Some(url) = link_url {
            if url.chars().count() > 2048 {
                return Err(anyhow::anyhow!(
                    "server.banner.link_url must be 2048 characters or fewer"
                ));
            }
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err(anyhow::anyhow!(
                    "server.banner.link_url must start with http:// or https://"
                ));
            }
        }

        Ok(Self {
            title: title.to_string(),
            message: message.to_string(),
            link_text: link_text.map(ToOwned::to_owned),
            link_url: link_url.map(ToOwned::to_owned),
        })
    }
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
            auth: None,
            scan_worker_ratio: None,
            background_worker_ratio: None,
            banner: None,
            preview_format: None,
            preview_jpeg_quality: None,
            preview_color: None,
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
        host: Option<String>,
        cache_dir: Option<String>,
    ) {
        // CLI database path overrides config (legacy; server mode ignores this)
        if let Some(db_path) = database_path {
            self.database = Some(DatabaseConfig { path: db_path });
        }

        // CLI image directories override config (legacy; server mode ignores this)
        if let Some(dirs) = image_dirs
            && !dirs.is_empty()
        {
            self.images = Some(ImagesConfig { directories: dirs });
        }

        // CLI port overrides config
        if let Some(cli_port) = port {
            self.server.port = Some(cli_port);
        }

        // CLI host overrides config
        if let Some(cli_host) = host {
            self.server.host = Some(cli_host);
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

    pub fn get_server_auth(&self) -> Result<Option<crate::server::auth::ServerAuth>> {
        self.server
            .auth
            .as_ref()
            .map(crate::server::auth::ServerAuth::from_config)
            .transpose()
    }

    /// Validated, whitespace-normalized site banner for the server API.
    pub fn get_site_banner(&self) -> Result<Option<SiteBannerConfig>> {
        self.server
            .banner
            .as_ref()
            .map(SiteBannerConfig::normalized)
            .transpose()
    }

    /// Effective worker tuning policy for the parallel scans and background
    /// pre-generation. The on-disk TOML surfaces the two core ratios; the other
    /// knobs keep their compiled-in defaults. Ratios are clamped to
    /// `[0.05, 1.0]` so a typo can't disable the work or over-subscribe.
    pub fn get_worker_policy(&self) -> crate::concurrency::WorkerPolicy {
        let interactive = self
            .server
            .scan_worker_ratio
            .unwrap_or(crate::concurrency::DEFAULT_INTERACTIVE_RATIO)
            .clamp(0.05, 1.0);
        let background = self
            .server
            .background_worker_ratio
            .unwrap_or(crate::concurrency::DEFAULT_BACKGROUND_RATIO)
            .clamp(0.05, 1.0);
        crate::concurrency::WorkerPolicy::default()
            .with_interactive_ratio(interactive)
            .with_background_ratio(background)
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

        self.get_site_banner()?;
        self.get_server_auth()?;

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
        assert!(config.get_server_auth().unwrap().is_none());
        // Database/images are obsolete and default to absent.
        assert!(config.database.is_none());
        assert!(config.images.is_none());
        assert_eq!(config.get_cache_directory(), "./cache");
        assert_eq!(config.get_file_ttl(), Duration::from_secs(300));
        assert_eq!(config.get_directory_ttl(), Duration::from_secs(300));
        assert!(config.get_site_banner().unwrap().is_none());
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
        assert!(!toml_string.contains("[server.banner]"));

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
    fn server_auth_parses_role_credentials_and_secret_files() {
        let secret = NamedTempFile::new().unwrap();
        std::fs::write(secret.path(), "editor-secret\n").unwrap();
        let secret_path =
            toml_edit::Value::from(secret.path().to_string_lossy().as_ref()).to_string();
        let toml = format!(
            r#"
[server]
port = 3000

[server.auth]
session_hours = 24
secure_cookie = true

[server.auth.read_only]
username = "viewer"
password = "viewer-secret"

[server.auth.read_write]
username = "editor"
password_file = {secret_path}

[cache]
directory = "./cache"
"#
        );
        let config: Config = toml_edit::de::from_str(&toml).unwrap();
        let config_debug = format!("{config:?}");
        assert!(!config_debug.contains("viewer-secret"));
        assert!(!config_debug.contains("editor-secret"));
        let auth = config.get_server_auth().unwrap().unwrap();
        let debug = format!("{auth:?}");
        assert!(debug.contains("viewer"));
        assert!(debug.contains("editor"));
        assert!(!debug.contains("viewer-secret"));
        assert!(!debug.contains("editor-secret"));
    }

    #[test]
    fn server_auth_defaults_to_secure_cookies() {
        let config: Config = toml_edit::de::from_str(
            r#"
[server.auth.read_write]
username = "editor"
password = "development-only"

[cache]
directory = "./cache"
"#,
        )
        .unwrap();

        let auth = config.server.auth.unwrap();
        assert!(auth.secure_cookie);
        assert!(!auth.allow_read_only_compute);
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
            Some("127.0.0.1".to_string()),
            Some("/new/cache".to_string()),
        );

        assert_eq!(config.get_port(), 8080);
        assert_eq!(config.get_host(), "127.0.0.1");
        assert_eq!(config.get_cache_directory(), "/new/cache");
        assert_eq!(config.database.unwrap().path, "/new/database.sqlite");
        assert_eq!(
            config.images.unwrap().directories,
            vec!["/new/images1", "/new/images2"]
        );
    }

    #[test]
    fn merge_with_cli_without_host_keeps_config_default() {
        // Regression: --host used to carry a clap default of 127.0.0.1 that was
        // then silently ignored — the server always bound the config default.
        // Now the flag is optional: absent → config default (0.0.0.0) stands.
        let mut config = Config::default();
        config.merge_with_cli(None, None, None, None, None);
        assert_eq!(config.get_host(), "0.0.0.0");
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
    fn test_worker_ratios_default_and_clamp() {
        // Absent -> compiled-in defaults.
        let config = Config::default();
        let policy = config.get_worker_policy();
        assert_eq!(
            policy.interactive_ratio,
            crate::concurrency::DEFAULT_INTERACTIVE_RATIO
        );
        assert_eq!(
            policy.background_ratio,
            crate::concurrency::DEFAULT_BACKGROUND_RATIO
        );

        // Configured values are honored.
        let mut config = Config::default();
        config.server.scan_worker_ratio = Some(0.75);
        config.server.background_worker_ratio = Some(0.1);
        let policy = config.get_worker_policy();
        assert_eq!(policy.interactive_ratio, 0.75);
        assert_eq!(policy.background_ratio, 0.1);

        // Out-of-range values are clamped so a typo can't disable the work or
        // over-subscribe.
        config.server.scan_worker_ratio = Some(0.0);
        config.server.background_worker_ratio = Some(5.0);
        let policy = config.get_worker_policy();
        assert_eq!(policy.interactive_ratio, 0.05);
        assert_eq!(policy.background_ratio, 1.0);
    }

    #[test]
    fn test_worker_ratios_toml_roundtrip() {
        // The knobs live in [server] alongside port/host and round-trip.
        let toml = r#"
[server]
port = 3000
scan_worker_ratio = 0.25
background_worker_ratio = 0.1

[cache]
directory = "./cache"
"#;
        let config: Config = toml_edit::de::from_str(toml).unwrap();
        let policy = config.get_worker_policy();
        assert_eq!(policy.interactive_ratio, 0.25);
        assert_eq!(policy.background_ratio, 0.1);

        // Absent keys must keep parsing (backward compatibility) and default.
        let toml_no_key = r#"
[server]
port = 3000

[cache]
directory = "./cache"
"#;
        let config: Config = toml_edit::de::from_str(toml_no_key).unwrap();
        let policy = config.get_worker_policy();
        assert_eq!(
            policy.interactive_ratio,
            crate::concurrency::DEFAULT_INTERACTIVE_RATIO
        );
        assert_eq!(
            policy.background_ratio,
            crate::concurrency::DEFAULT_BACKGROUND_RATIO
        );

        // Default serialization omits the keys (kept clean like the other
        // optional knobs) so older binaries ignore them cleanly.
        let json = toml_edit::ser::to_string_pretty(&Config::default()).unwrap();
        assert!(
            !json.contains("scan_worker_ratio") && !json.contains("background_worker_ratio"),
            "default config should not write the keys: {json}"
        );
    }

    #[test]
    fn site_banner_parses_and_normalizes() {
        let toml = r#"
[server]
port = 3000

[server.banner]
title = "  Demo site  "
message = "  Sample data; changes may be reset.  "
link_text = "  Learn more  "
link_url = "  https://psf-guard.com/  "

[cache]
directory = "./cache"
"#;
        let config: Config = toml_edit::de::from_str(toml).unwrap();
        let banner = config.get_site_banner().unwrap().unwrap();
        assert_eq!(banner.title, "Demo site");
        assert_eq!(banner.message, "Sample data; changes may be reset.");
        assert_eq!(banner.link_text.as_deref(), Some("Learn more"));
        assert_eq!(banner.link_url.as_deref(), Some("https://psf-guard.com/"));
    }

    #[test]
    fn site_banner_rejects_incomplete_or_unsafe_links() {
        let mut config = Config::default();
        config.server.banner = Some(SiteBannerConfig {
            title: "Demo".into(),
            message: "Sample data".into(),
            link_text: Some("Learn more".into()),
            link_url: None,
        });
        assert!(config
            .get_site_banner()
            .unwrap_err()
            .to_string()
            .contains("must be set together"));

        config.server.banner.as_mut().unwrap().link_url = Some("javascript:alert(1)".into());
        assert!(config
            .get_site_banner()
            .unwrap_err()
            .to_string()
            .contains("must start with http:// or https://"));
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

    fn entry(id: &str) -> crate::db_registry::DbEntry {
        crate::db_registry::DbEntry {
            id: id.into(),
            name: id.into(),
            db_path: format!("/tmp/{id}.sqlite"),
            image_dirs: vec![],
            reject_archive: None,
            remote_image_upload: None,
        }
    }

    const TOKEN: &str = "a-remote-sync-token-long-enough";

    #[test]
    fn the_preview_format_is_png_unless_asked_otherwise() {
        use crate::preview_format::{PreviewFormat, DEFAULT_JPEG_QUALITY};

        let config: Config = toml_edit::de::from_str("[server]\n\n[cache]\n").unwrap();
        assert_eq!(
            config.server.preview_encoding().unwrap().format,
            PreviewFormat::Png
        );

        let config: Config = toml_edit::de::from_str(
            "[server]\npreview_format = \"jpeg\"\npreview_jpeg_quality = 70\n\n[cache]\n",
        )
        .unwrap();
        let encoding = config.server.preview_encoding().unwrap();
        assert_eq!(encoding.format, PreviewFormat::Jpeg);
        assert_eq!(encoding.jpeg_quality, 70);

        // A quality with no format named is not a request for JPEG.
        let config: Config =
            toml_edit::de::from_str("[server]\npreview_jpeg_quality = 70\n\n[cache]\n").unwrap();
        assert_eq!(
            config.server.preview_encoding().unwrap().format,
            PreviewFormat::Png
        );

        // And JPEG with no quality takes the default rather than zero.
        let config: Config =
            toml_edit::de::from_str("[server]\npreview_format = \"jpg\"\n\n[cache]\n").unwrap();
        assert_eq!(
            config.server.preview_encoding().unwrap().jpeg_quality,
            DEFAULT_JPEG_QUALITY
        );
    }

    #[test]
    fn an_unknown_preview_format_stops_the_server() {
        // Falling back to PNG would leave an operator who asked for a smaller
        // cache with a full-size one and no indication why.
        let config: Config =
            toml_edit::de::from_str("[server]\npreview_format = \"webp\"\n\n[cache]\n").unwrap();
        let error = config.server.preview_encoding().unwrap_err().to_string();
        assert!(error.contains("preview_format"), "{error}");
    }

    #[test]
    fn remote_sync_opens_only_the_named_database_and_only_for_sync() {
        let config: Config = toml_edit::de::from_str(&format!(
            r#"
            [server]
            [cache]
            [[remote_sync]]
            database = "telescope"
            token = "{TOKEN}"
            "#
        ))
        .unwrap();
        let mut entries = vec![entry("telescope"), entry("review")];

        let opened =
            apply_remote_access(&mut entries, &config.remote_sync, &config.remote_upload).unwrap();

        assert_eq!(opened, vec!["telescope (sync)".to_string()]);
        let upload = entries[0].remote_image_upload.as_ref().unwrap();
        assert!(upload.sync_enabled);
        assert!(upload.token_matches(TOKEN));
        assert!(
            !upload.enabled,
            "opening sync must not also accept image uploads"
        );
        assert!(
            entries[1].remote_image_upload.is_none(),
            "an unnamed database stays closed"
        );
    }

    #[test]
    fn remote_sync_keeps_an_existing_image_upload_grant() {
        let mut existing = crate::db_registry::RemoteImageUploadConfig {
            enabled: true,
            image_dir: "/data/incoming".into(),
            ..Default::default()
        };
        existing
            .set_token("an-image-upload-token-long-enough")
            .unwrap();
        let mut entries = vec![entry("telescope")];
        entries[0].remote_image_upload = Some(existing);

        apply_remote_access(
            &mut entries,
            &[RemoteSyncConfig {
                database: "telescope".into(),
                token: Some(TOKEN.into()),
                token_file: None,
            }],
            &[],
        )
        .unwrap();

        let upload = entries[0].remote_image_upload.as_ref().unwrap();
        assert!(upload.enabled, "the upload grant survives");
        assert_eq!(upload.image_dir, "/data/incoming");
        assert!(upload.sync_enabled);
        // One key per database: the configured token replaces the old one.
        assert!(upload.token_matches(TOKEN));
    }

    #[test]
    fn a_token_file_is_read_and_trimmed() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), format!("{TOKEN}\n")).unwrap();
        let mut entries = vec![entry("telescope")];

        apply_remote_access(
            &mut entries,
            &[RemoteSyncConfig {
                database: "telescope".into(),
                token: None,
                token_file: Some(file.path().to_string_lossy().into_owned()),
            }],
            &[],
        )
        .unwrap();

        // The trailing newline every editor adds must not become part of the
        // token, or the operator's key silently never matches.
        assert!(entries[0]
            .remote_image_upload
            .as_ref()
            .unwrap()
            .token_matches(TOKEN));
    }

    #[test]
    fn an_unregistered_database_is_fatal_and_says_what_is_configured() {
        let mut entries = vec![entry("review")];
        let error = apply_remote_access(
            &mut entries,
            &[RemoteSyncConfig {
                database: "telescop".into(),
                token: Some(TOKEN.into()),
                token_file: None,
            }],
            &[],
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("telescop"), "{error}");
        assert!(error.contains("review"), "{error}");
    }

    #[test]
    fn a_remote_sync_block_without_a_token_is_refused() {
        let error = apply_remote_access(
            &mut [entry("telescope")],
            &[RemoteSyncConfig {
                database: "telescope".into(),
                token: None,
                token_file: None,
            }],
            &[],
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("token"), "{error}");
    }

    #[test]
    fn a_short_token_is_refused_before_the_server_starts() {
        let error = apply_remote_access(
            &mut [entry("telescope")],
            &[RemoteSyncConfig {
                database: "telescope".into(),
                token: Some("too-short".into()),
                token_file: None,
            }],
            &[],
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("telescope"), "{error}");
    }

    #[test]
    fn both_grants_can_be_configured_for_one_database() {
        let directory = tempfile::tempdir().unwrap();
        let image_dir = directory.path().join("incoming");
        let mut entries = vec![entry("telescope")];

        let opened = apply_remote_access(
            &mut entries,
            &[RemoteSyncConfig {
                database: "telescope".into(),
                token: Some(TOKEN.into()),
                token_file: None,
            }],
            &[RemoteUploadConfig {
                database: "telescope".into(),
                image_dir: image_dir.to_string_lossy().into_owned(),
                token: Some(TOKEN.into()),
                token_file: None,
            }],
        )
        .unwrap();

        assert_eq!(opened, vec!["telescope (sync + image upload)".to_string()]);
        let access = entries[0].remote_image_upload.as_ref().unwrap();
        assert!(access.sync_enabled);
        assert!(access.enabled);
        assert_eq!(access.image_dir, image_dir.to_string_lossy());
        assert!(access.token_matches(TOKEN));
        // The receive directory is made ready at startup, not on the first
        // upload, so a bad path fails while the operator is still watching.
        assert!(image_dir.is_dir());
    }

    #[test]
    fn two_grants_with_different_keys_are_refused() {
        // Silently letting the second win would leave the first grant's key
        // rejected with no hint as to why.
        let directory = tempfile::tempdir().unwrap();
        let error = apply_remote_access(
            &mut [entry("telescope")],
            &[RemoteSyncConfig {
                database: "telescope".into(),
                token: Some(TOKEN.into()),
                token_file: None,
            }],
            &[RemoteUploadConfig {
                database: "telescope".into(),
                image_dir: directory.path().to_string_lossy().into_owned(),
                token: Some("a-different-token-long-enough".into()),
                token_file: None,
            }],
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("one key"), "{error}");
    }
}
