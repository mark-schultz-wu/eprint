//! `eprint` CLI entry point.

mod cache;
mod cli;
mod commands;
mod config;
mod exit;
mod iacr;
mod ids;
mod markdown;
mod sources;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::Cli::parse_from(cli::expand_shorthand(std::env::args_os().collect()));
    init_tracing(args.verbose, args.log_format);
    let mut cfg = config::Config::from_env();
    if let Some(v) = args.auto_sync {
        cfg.sync.auto = v;
    }
    if let Some(h) = args.sync_stale_hours {
        cfg.sync.stale_after = time::Duration::hours(h.into());
    }
    let rate_limiter = iacr::http::rate_limiter(cfg.network.min_interval, 3);
    let cx = cli::Context {
        site: iacr::site::Site::new(&cfg.network.base_url),
        http: iacr::http::client(cfg.network.contact.as_deref())?,
        cfg,
        offline: args.offline,
        json: args.json,
        rate_limiter,
    };
    let result = match args.command {
        cli::Command::Paper(c) => commands::paper::run(&cx, c).await,
        cli::Command::Sync(c) => commands::sync::run(&cx, c).await,
        cli::Command::Feed(c) => commands::feed::run(&cx, c).await,
        cli::Command::Cache(c) => commands::cache::run(&cx, c).await,
    };
    // Map typed failures to distinct, scriptable exit codes. Printing/exiting
    // here (rather than returning the Result for anyhow's Termination) is what
    // lets a caller branch on the *reason* — independent of the trace level,
    // which may have suppressed the corresponding warning.
    if let Err(e) = result {
        eprintln!("Error: {e:#}");
        std::process::exit(exit::CommandFailure::code_of(&e));
    }
    Ok(())
}

fn init_tracing(verbose: u8, format: cli::LogFormat) {
    let env_value = std::env::var(EnvFilter::DEFAULT_ENV).ok();
    let filter = build_log_filter(verbose, env_value.as_deref());
    let registry = tracing_subscriber::registry().with(filter);
    // Logs go to stderr: stdout carries the command's output (e.g. --json),
    // which a warning must never corrupt.
    let layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    match format {
        cli::LogFormat::Pretty => registry.with(layer).init(),
        cli::LogFormat::Json => registry.with(layer.json()).init(),
    }
}

fn build_log_filter(verbose: u8, env_value: Option<&str>) -> EnvFilter {
    let default_level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let mut filter = EnvFilter::new(format!("eprint={default_level}"));
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
        for (v, level) in [(1, "info"), (2, "debug"), (3, "trace"), (9, "trace")] {
            let s = format!("{}", build_log_filter(v, None));
            assert!(s.contains(&format!("eprint={level}")), "-v x{v}: {s}");
        }
    }

    #[test]
    fn env_directive_overrides_default_for_same_target() {
        let s = format!("{}", build_log_filter(0, Some("eprint=trace")));
        assert!(s.contains("eprint=trace"));
        assert!(!s.contains("eprint=warn"));
    }
}
