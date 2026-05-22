//! `eprint` CLI entry point.

mod archive;
mod cache;
mod cli;
mod commands;
mod config;
mod feed;
mod id;
mod net;
mod oai;
mod scrape;
mod version;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::Cli::parse();
    init_tracing(args.verbose, args.log_format);
    let mut cfg = config::Config::from_env();
    if let Some(v) = args.auto_sync {
        cfg.sync.auto = v;
    }
    if let Some(h) = args.sync_stale_hours {
        cfg.sync.stale_after_hours = h;
    }
    let rate_limiter = net::rate_limiter(cfg.network.min_interval_s, 3);
    let cx = cli::Context { cfg, offline: args.offline, json: args.json, rate_limiter };
    match args.command {
        cli::Command::Paper(c) => commands::paper::run(&cx, c).await,
        cli::Command::Sync(c) => commands::sync::run(&cx, c).await,
        cli::Command::Feed(c) => commands::feed::run(&cx, c).await,
        cli::Command::Cache(c) => commands::cache_cmd::run(&cx, c).await,
    }
}

fn init_tracing(verbose: u8, format: cli::LogFormat) {
    let env_value = std::env::var(EnvFilter::DEFAULT_ENV).ok();
    let filter = build_log_filter(verbose, env_value.as_deref());
    let registry = tracing_subscriber::registry().with(filter);
    match format {
        cli::LogFormat::Pretty => registry.with(tracing_subscriber::fmt::layer()).init(),
        cli::LogFormat::Json => registry.with(tracing_subscriber::fmt::layer().json()).init(),
    }
}

fn build_log_filter(verbose: u8, env_value: Option<&str>) -> EnvFilter {
    let default_level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let mut filter = EnvFilter::new(format!("eprint={default_level},papermd={default_level}"));
    if let Some(env_filter) = env_value {
        for directive in env_filter.split(',') {
            if let Ok(parsed) = directive.parse() {
                filter = filter.add_directive(parsed);
            }
        }
    }
    filter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter_carries_verbosity() {
        let s = format!("{}", build_log_filter(0, None));
        assert!(s.contains("eprint=warn"));
        assert!(s.contains("papermd=warn"));
    }

    #[test]
    fn env_directive_overrides_default_for_same_target() {
        let s = format!("{}", build_log_filter(0, Some("eprint=trace")));
        assert!(s.contains("eprint=trace"));
        assert!(!s.contains("eprint=warn"));
    }
}
