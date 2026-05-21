//! `eprint` CLI entry point.
//!
//! See `cli` for the command structure. Each subcommand has its own module
//! under `commands`. Top-level: install tracing, parse args, dispatch.

mod cache;
mod cli;
mod commands;
mod config;
mod id;
mod net;
mod scrape;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::Cli::parse();
    init_tracing(args.verbose, args.log_format);
    let cfg = config::load(args.config.as_deref())?;
    let cx = cli::Context { cfg, offline: args.offline, json: args.json };
    match args.command {
        cli::Command::Fetch(c) => commands::fetch::run(&cx, c).await,
        cli::Command::Show(c) => commands::show::run(&cx, c).await,
        cli::Command::Convert(c) => commands::convert::run(&cx, c).await,
        cli::Command::Refresh(c) => commands::refresh::run(&cx, c).await,
        cli::Command::Check(c) => commands::check::run(&cx, c).await,
        cli::Command::Cache(c) => commands::cache_cmd::run(&cx, c).await,
    }
}

fn init_tracing(verbose: u8, format: cli::LogFormat) {
    // -v / -vv / -vvv override the env filter; RUST_LOG still wins if set.
    let default_level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("eprint={default_level},papermd={default_level}")));
    let registry = tracing_subscriber::registry().with(filter);
    match format {
        cli::LogFormat::Pretty => registry.with(tracing_subscriber::fmt::layer()).init(),
        cli::LogFormat::Json => registry.with(tracing_subscriber::fmt::layer().json()).init(),
    }
}
