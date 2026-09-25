//! Effective settings, computed from environment variables.
//!
//! There is no config file. Persistent preferences live in env vars
//! exported from your shell rc (or a `direnv`-style file you source).
//!
//! Recognised env vars:
//!
//! | Var                          | Meaning                                                           |
//! |------------------------------|-------------------------------------------------------------------|
//! | `EPRINT_CACHE_DIR`           | cache root (default: OS cache dir + `/eprint`)                    |
//! | `EPRINT_BASE_URL`            | eprint server (default `https://eprint.iacr.org`; for mirrors/tests) |
//! | `EPRINT_CONTACT`             | contact appended to outbound `User-Agent`                         |
//! | `EPRINT_MIN_INTERVAL_S`      | minimum seconds between outbound HTTP requests (default `2.0`)    |
//! | `EPRINT_MD_DEVICE`           | Markdown converter device: `metal`, `cuda`, `cpu` (default: best built-in) |
//! | `EPRINT_AUTO_SYNC`           | `true` (default) / `false` — auto-run OAI-PMH sync on staleness   |
//! | `EPRINT_SYNC_STALE_HOURS`    | hours after which the cache is considered stale (default `24`)    |
//!
//! This module is the only reader of these variables; the `--auto-sync` and
//! `--sync-stale-hours` flags override them (see `main`). A set but
//! unparseable value is warned about and ignored.

use std::path::PathBuf;

/// Effective settings used by the running command.
#[derive(Debug, Clone)]
pub struct Config {
    pub cache_root: PathBuf,
    pub network: Network,
    /// `EPRINT_MD_DEVICE`; `None` means [`crate::markdown::default_device`].
    pub md_device: Option<String>,
    pub sync: Sync,
}

#[derive(Debug, Clone)]
pub struct Network {
    /// eprint server base URL; see [`crate::iacr::site`].
    pub base_url: String,
    pub contact: Option<String>,
    pub min_interval_s: f64,
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
                base_url: env_string("EPRINT_BASE_URL")
                    .unwrap_or_else(|| crate::iacr::site::DEFAULT_BASE_URL.to_owned()),
                contact: env_string("EPRINT_CONTACT"),
                min_interval_s: env_f64("EPRINT_MIN_INTERVAL_S").unwrap_or(2.0),
            },
            md_device: env_string("EPRINT_MD_DEVICE"),
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
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("eprint")
}

fn env_string(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

/// Parse an env var, warning (rather than silently falling back to the
/// default) when it's set but unparseable.
fn env_parsed<T>(key: &str, parse: impl FnOnce(&str) -> Option<T>) -> Option<T> {
    let raw = env_string(key)?;
    let parsed = parse(&raw);
    if parsed.is_none() {
        tracing::warn!("ignoring {key}={raw:?}: not a valid value; using the default");
    }
    parsed
}

fn env_f64(key: &str) -> Option<f64> {
    env_parsed(key, |s| s.parse().ok())
}

fn env_u32(key: &str) -> Option<u32> {
    env_parsed(key, |s| s.parse().ok())
}

fn env_bool(key: &str) -> Option<bool> {
    env_parsed(key, parse_bool)
}

/// The same spellings clap's `BoolishValueParser` accepts for `--auto-sync`.
fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bool_accepts_common_spellings() {
        for s in ["1", "true", "YES", "on", "y"] {
            assert_eq!(parse_bool(s), Some(true), "{s}");
        }
        for s in ["0", "false", "No", "off", "n"] {
            assert_eq!(parse_bool(s), Some(false), "{s}");
        }
        assert_eq!(parse_bool("maybe"), None);
    }
}
