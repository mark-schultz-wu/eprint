//! `eprint` CLI entry point.
//!
//! See `cli` for the command structure. Each subcommand has its own module
//! under `commands`. Top-level: install tracing, parse args, dispatch.

mod cache;
mod cli;
mod commands;
mod config;
mod feed;
mod id;
mod net;
mod oai;
mod scrape;

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
    let cx = cli::Context { cfg, offline: args.offline, json: args.json };
    match args.command {
        cli::Command::Fetch(c) => commands::fetch::run(&cx, c).await,
        cli::Command::Show(c) => commands::show::run(&cx, c).await,
        cli::Command::Convert(c) => commands::convert::run(&cx, c).await,
        cli::Command::Refresh(c) => commands::refresh::run(&cx, c).await,
        cli::Command::Check(c) => commands::check::run(&cx, c).await,
        cli::Command::Cache(c) => commands::cache_cmd::run(&cx, c).await,
        cli::Command::Sync(c) => commands::sync::run(&cx, c).await,
        cli::Command::Feed(c) => commands::feed::run(&cx, c).await,
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

/// Build the `EnvFilter` used by the CLI.
///
/// Precedence: `-v / -vv / -vvv` sets a default filter; the `RUST_LOG`-style
/// `env_value` (if provided) *augments* rather than replaces. Lets users say
/// `-v RUST_LOG=papermd::local=trace` to opt in to module-level detail.
///
/// EnvFilter deduplicates directives sharing the same target — the
/// most-recently-added one replaces earlier ones. Since `env_value`
/// entries are pushed after the default, an exact-target directive in
/// `env_value` (e.g. `eprint=trace`) overrides the default `eprint=warn`.
/// More-specific paths (e.g. `papermd::local=trace`) coexist with the
/// `papermd=warn` baseline and are picked when their target matches.
/// See the tests below for the pinned behavior.
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

    /// Sanity: the default filter has `eprint` and `papermd` at the level
    /// matching the verbosity setting.
    #[test]
    fn default_filter_carries_verbosity() {
        let s = format!("{}", build_log_filter(0, None));
        assert!(s.contains("eprint=warn"));
        assert!(s.contains("papermd=warn"));
        let s = format!("{}", build_log_filter(1, None));
        assert!(s.contains("eprint=info"));
    }

    /// Critical: a `RUST_LOG`-supplied directive for the same target must
    /// win over the default. EnvFilter deduplicates same-target directives,
    /// keeping the most recently added one (since we add the env override
    /// *after* the default, the override wins).
    #[test]
    fn env_directive_overrides_default_for_same_target() {
        let s = format!("{}", build_log_filter(0, Some("eprint=trace")));
        assert!(
            s.contains("eprint=trace"),
            "env override must be present in the filter; got: {s}"
        );
        assert!(
            !s.contains("eprint=warn"),
            "default `eprint=warn` must have been replaced by the env override; got: {s}"
        );
    }

    /// Module-specific env directives don't tie with broader defaults — the
    /// more-specific target wins regardless of position. Just verify they
    /// land in the filter at all.
    #[test]
    fn module_specific_env_directive_lands() {
        let s = format!("{}", build_log_filter(0, Some("papermd::local=trace")));
        assert!(s.contains("papermd::local=trace"));
        assert!(s.contains("eprint=warn"));
    }

    /// Malformed env directives are skipped silently rather than failing
    /// the whole filter build.
    #[test]
    fn malformed_env_directive_is_ignored() {
        let s = format!(
            "{}",
            build_log_filter(0, Some("eprint=trace,!@#bogus,papermd=debug"))
        );
        assert!(s.contains("eprint=trace"));
        assert!(s.contains("papermd=debug"));
    }
}
