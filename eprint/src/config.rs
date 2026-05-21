//! Config file loading + env var overrides.
//!
//! Default config path: `$XDG_CONFIG_HOME/eprint/config.toml`. Env vars
//! prefixed with `EPRINT_` override fields.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Top-level config. All fields have defaults so a missing config file
/// works fine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub fetch: FetchConfig,
    #[serde(default)]
    pub convert: ConvertConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub network: NetworkConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FetchConfig {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConvertConfig {
    /// Quality-tier-specific backend selection.
    #[serde(default)]
    pub ml: BackendConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    /// "local" or "remote".
    #[serde(default = "default_backend")]
    pub backend: String,
    /// For backend = "remote": base URL.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// For backend = "remote": env var holding the bearer token.
    #[serde(default)]
    pub token_env: Option<String>,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            endpoint: None,
            token_env: None,
        }
    }
}

fn default_backend() -> String {
    "local".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Cache root. Defaults to `$XDG_CACHE_HOME/eprint/`.
    #[serde(default)]
    pub root: Option<PathBuf>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self { root: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Contact address for the User-Agent string. Polite for archives.
    #[serde(default)]
    pub contact: Option<String>,
    /// Minimum interval between outbound requests, in seconds.
    #[serde(default = "default_min_interval")]
    pub min_interval_s: f64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self { contact: None, min_interval_s: default_min_interval() }
    }
}

fn default_min_interval() -> f64 {
    2.0
}

/// Load config from disk, applying env-var overrides. A missing config
/// file is not an error.
pub fn load(explicit: Option<&Path>) -> Result<Config> {
    let path = explicit
        .map(Path::to_path_buf)
        .or_else(default_config_path);
    let mut cfg: Config = match &path {
        Some(p) if p.exists() => {
            let s = std::fs::read_to_string(p)?;
            toml::from_str(&s)?
        }
        _ => Config::default(),
    };
    apply_env_overrides(&mut cfg);
    Ok(cfg)
}

fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("eprint").join("config.toml"))
}

fn apply_env_overrides(cfg: &mut Config) {
    if let Ok(v) = std::env::var("EPRINT_CACHE_DIR") {
        cfg.cache.root = Some(PathBuf::from(v));
    }
    if let Ok(v) = std::env::var("EPRINT_CONTACT") {
        cfg.network.contact = Some(v);
    }
    if let Ok(v) = std::env::var("EPRINT_MIN_INTERVAL_S") {
        if let Ok(parsed) = v.parse::<f64>() {
            cfg.network.min_interval_s = parsed;
        }
    }
}

impl Config {
    /// Resolve the cache root, applying the XDG default if unset.
    pub fn cache_root(&self) -> PathBuf {
        self.cache.root.clone().unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("eprint")
        })
    }
}
