//! Effective settings, computed from environment variables.
//!
//! There is no config file. Persistent preferences live in env vars
//! exported from your shell rc (or a `direnv`-style file you source).
//!
//! Recognised env vars:
//!
//! | Var                          | Meaning                                                           |
//! |------------------------------|-------------------------------------------------------------------|
//! | `EPRINT_CACHE_DIR`           | cache root (default: `$XDG_CACHE_HOME/eprint`)                    |
//! | `EPRINT_CONTACT`             | contact appended to outbound `User-Agent`                         |
//! | `EPRINT_MIN_INTERVAL_S`      | minimum seconds between outbound HTTP requests (default `2.0`)    |
//! | `EPRINT_ML_BACKEND`          | `local` (default) or `remote`                                     |
//! | `EPRINT_ML_ENDPOINT`         | base URL for `remote` backend                                     |
//! | `EPRINT_ML_TOKEN_ENV`        | name of env var holding bearer token for `remote` backend         |
//! | `EPRINT_AUTO_SYNC`           | `true` (default) / `false` — auto-run OAI-PMH sync on staleness   |
//! | `EPRINT_SYNC_STALE_HOURS`    | hours after which the cache is considered stale (default `24`)    |
//!
//! CLI flags override env vars for the per-invocation settings; see `cli`.

use std::path::PathBuf;

/// Effective settings used by the running command.
#[derive(Debug, Clone)]
pub struct Config {
    pub cache_root: PathBuf,
    pub network: Network,
    pub ml: Backend,
    pub sync: Sync,
}

#[derive(Debug, Clone)]
pub struct Network {
    pub contact: Option<String>,
    pub min_interval_s: f64,
}

#[derive(Debug, Clone)]
pub struct Backend {
    pub kind: BackendKind,
    pub endpoint: Option<String>,
    pub token_env: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Local,
    Remote,
}

#[derive(Debug, Clone)]
pub struct Sync {
    pub auto: bool,
    pub stale_after_hours: u32,
}

impl Config {
    /// Compute the effective config from env vars + built-in defaults.
    pub fn from_env() -> Self {
        Self {
            cache_root: cache_root_from_env(),
            network: Network {
                contact: env_string("EPRINT_CONTACT"),
                min_interval_s: env_f64("EPRINT_MIN_INTERVAL_S").unwrap_or(2.0),
            },
            ml: Backend {
                kind: env_string("EPRINT_ML_BACKEND")
                    .and_then(|s| match s.as_str() {
                        "local" => Some(BackendKind::Local),
                        "remote" => Some(BackendKind::Remote),
                        _ => None,
                    })
                    .unwrap_or(BackendKind::Local),
                endpoint: env_string("EPRINT_ML_ENDPOINT"),
                token_env: env_string("EPRINT_ML_TOKEN_ENV"),
            },
            sync: Sync {
                auto: env_bool("EPRINT_AUTO_SYNC").unwrap_or(true),
                stale_after_hours: env_u32("EPRINT_SYNC_STALE_HOURS").unwrap_or(24),
            },
        }
    }
}

fn cache_root_from_env() -> PathBuf {
    if let Some(v) = env_string("EPRINT_CACHE_DIR") {
        return PathBuf::from(v);
    }
    dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".")).join("eprint")
}

fn env_string(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

fn env_f64(key: &str) -> Option<f64> {
    env_string(key).and_then(|s| s.parse().ok())
}

fn env_u32(key: &str) -> Option<u32> {
    env_string(key).and_then(|s| s.parse().ok())
}

fn env_bool(key: &str) -> Option<bool> {
    let s = env_string(key)?.to_ascii_lowercase();
    match s.as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}
