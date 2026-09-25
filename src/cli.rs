//! Clap-derive CLI structures.
//!
//! Top-level shape: `eprint paper <id>` fetches/describes/converts a paper,
//! and a bare `eprint <id>` is shorthand for it. The other subcommands cover
//! discrete operations: `sync`, `feed`, `cache`.

use crate::config::Config;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use std::ffi::OsString;

/// Fetch, describe, and convert IACR ePrint papers.
#[derive(Debug, Parser)]
#[command(
    name = "eprint",
    version,
    about,
    arg_required_else_help = true,
    after_help = "Shorthand: `eprint <ID> [OPTIONS]` is `eprint paper <ID> [OPTIONS]`."
)]
pub struct Cli {
    /// Never make network requests; error if cache miss.
    #[arg(long, global = true)]
    pub offline: bool,
    /// Emit JSON output instead of human-readable text.
    #[arg(long, global = true)]
    pub json: bool,
    /// Increase verbosity: -v info, -vv debug, -vvv trace.
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
    /// Log output format.
    #[arg(long, value_enum, default_value_t = LogFormat::Pretty, global = true)]
    pub log_format: LogFormat,
    /// Run OAI-PMH sync if the cache is stale (true/false, yes/no, 1/0,
    /// on/off). Overrides EPRINT_AUTO_SYNC.
    #[arg(long, global = true, value_parser = clap::builder::BoolishValueParser::new())]
    pub auto_sync: Option<bool>,
    /// Cache staleness threshold in hours. Overrides EPRINT_SYNC_STALE_HOURS.
    #[arg(long, global = true)]
    pub sync_stale_hours: Option<u32>,

    #[command(subcommand)]
    pub command: Command,
}

/// Rewrite the shorthand `eprint [globals] <id> ...` to
/// `eprint [globals] paper <id> ...`: if the first positional argument isn't
/// a subcommand (or `help`), it's a paper id. Top-level options and their
/// values are skipped when looking for it, using clap's own definition of
/// the command, so this can't drift from the real CLI.
pub fn expand_shorthand(argv: Vec<OsString>) -> Vec<OsString> {
    let cmd = Cli::command();
    let is_subcommand = |s: &str| {
        s == "help"
            || cmd
                .get_subcommands()
                .any(|c| c.get_name() == s || c.get_all_aliases().any(|a| a == s))
    };
    let takes_value = |flag: &str| {
        cmd.get_arguments().any(|a| {
            let named = a.get_long().is_some_and(|l| flag == format!("--{l}"))
                || a.get_short().is_some_and(|c| flag == format!("-{c}"));
            named && a.get_action().takes_values()
        })
    };

    let mut i = 1; // argv[0] is the program
    while let Some(arg) = argv.get(i).and_then(|a| a.to_str()) {
        if arg == "--" || !arg.starts_with('-') {
            if !arg.starts_with('-') && !is_subcommand(arg) {
                let mut out = argv;
                out.insert(i, "paper".into());
                return out;
            }
            break;
        }
        // Skip an option, and its value if given separately (`--opt value`).
        i += if !arg.contains('=') && takes_value(arg) {
            2
        } else {
            1
        };
    }
    argv
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LogFormat {
    Pretty,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Describe + acquire a paper.
    #[command(alias = "p")]
    Paper(PaperArgs),
    /// Bulk OAI-PMH annotation across the cache.
    Sync(SyncArgs),
    /// Browse the eprint RSS feed.
    Feed(FeedArgs),
    /// Cache management.
    Cache(CacheArgs),
}

pub struct Context {
    pub cfg: Config,
    pub offline: bool,
    pub json: bool,
    /// Where eprint lives; builds every request URL.
    pub site: crate::iacr::site::Site,
    /// One HTTP client for the whole run, so connections are reused.
    pub http: reqwest::Client,
    /// Shared token-bucket rate limiter for all outbound HTTP. Built
    /// once in `main` from `cfg.network` (interval) and a small burst
    /// budget, then handed off so all callers serialize through it.
    pub rate_limiter: std::sync::Arc<crate::iacr::http::RateLimiter>,
}

#[derive(Debug, Args)]
pub struct PaperArgs {
    /// Paper id (e.g. "2024/463", "2024-463", or full eprint URL).
    pub id: String,
    /// Operate on a specific version of the paper, by its timestamp (e.g.
    /// `20240319T143540Z`; see the version list). Defaults to the current one.
    #[arg(long, value_name = "VERSION")]
    pub at: Option<String>,
    /// Open an interactive picker over known versions.
    #[arg(long)]
    pub select_version: bool,
    /// Also produce Markdown (math as LaTeX) with MinerU2.5-Pro. Slow: about
    /// half a minute per page on a GPU. Downloads a 2.2 GB model on first use.
    #[arg(long)]
    pub md: bool,
    /// Re-fetch the paper's version list from eprint even if the cached one
    /// looks current.
    #[arg(long)]
    pub force: bool,
    /// Skip printing the abstract at the bottom of the human-readable output.
    #[arg(long)]
    pub no_abstract: bool,
}

#[derive(Debug, Args)]
pub struct SyncArgs {
    #[arg(long)]
    pub since: Option<String>,
    #[arg(long, default_value_t = crate::commands::sync::DEFAULT_WINDOW_DAYS)]
    pub default_window_days: u32,
}

#[derive(Debug, Args)]
pub struct FeedArgs {
    #[arg(value_enum, default_value_t = FeedView::Updates)]
    pub view: FeedView,
    #[arg(long, value_enum)]
    pub category: Option<FeedCategory>,
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum FeedView {
    New,
    Updates,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum FeedCategory {
    Applications,
    Protocols,
    Foundations,
    Implementation,
    Secretkey,
    Publickey,
    Attacks,
}

impl FeedCategory {
    pub fn as_query(&self) -> &'static str {
        match self {
            FeedCategory::Applications => "APPLICATIONS",
            FeedCategory::Protocols => "PROTOCOLS",
            FeedCategory::Foundations => "FOUNDATIONS",
            FeedCategory::Implementation => "IMPLEMENTATION",
            FeedCategory::Secretkey => "SECRETKEY",
            FeedCategory::Publickey => "PUBLICKEY",
            FeedCategory::Attacks => "ATTACKS",
        }
    }
}

#[derive(Debug, Args)]
pub struct CacheArgs {
    #[command(subcommand)]
    pub command: CacheCommand,
}

#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    Path,
    List,
    /// Delete cached papers. The Markdown model is kept unless `--models`.
    Clear {
        #[arg(long)]
        dry_run: bool,
        /// Also delete the downloaded Markdown model weights (2.2 GB; they
        /// re-download on the next `--md`).
        #[arg(long)]
        models: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: clap also read EPRINT_AUTO_SYNC with a strict true/false
    /// parser, so `EPRINT_AUTO_SYNC=1` (documented as valid) made every
    /// command fail to parse. Env vars now belong to `config` alone.
    #[test]
    fn env_vars_do_not_reach_argument_parsing() {
        std::env::set_var("EPRINT_AUTO_SYNC", "1");
        std::env::set_var("EPRINT_SYNC_STALE_HOURS", "not a number");
        let cli = Cli::try_parse_from(["eprint", "cache", "path"]).unwrap();
        assert_eq!(cli.auto_sync, None);
        assert_eq!(cli.sync_stale_hours, None);
    }

    fn parse(argv: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(expand_shorthand(argv.iter().map(OsString::from).collect()))
    }

    fn expanded(argv: &[&str]) -> Vec<String> {
        expand_shorthand(argv.iter().map(OsString::from).collect())
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect()
    }

    #[test]
    fn bare_id_is_shorthand_for_paper() {
        for argv in [
            &[
                "eprint",
                "paper",
                "2024/463",
                "--at",
                "20240319T143540Z",
                "--md",
            ][..],
            &["eprint", "2024/463", "--at", "20240319T143540Z", "--md"],
        ] {
            let Command::Paper(p) = parse(argv).unwrap().command else {
                panic!("{argv:?} should be a paper command");
            };
            assert_eq!(p.id, "2024/463");
            assert_eq!(p.at.as_deref(), Some("20240319T143540Z"));
            assert!(p.md);
        }
    }

    #[test]
    fn shorthand_skips_global_options_and_their_values() {
        assert_eq!(
            expanded(&[
                "eprint",
                "--json",
                "--log-format",
                "json",
                "-vv",
                "2024/463"
            ]),
            [
                "eprint",
                "--json",
                "--log-format",
                "json",
                "-vv",
                "paper",
                "2024/463"
            ]
        );
        assert_eq!(
            expanded(&["eprint", "--auto-sync=no", "2024/463", "--json"]),
            ["eprint", "--auto-sync=no", "paper", "2024/463", "--json"]
        );
        let cli = parse(&["eprint", "--offline", "2024/463", "--json"]).unwrap();
        assert!(cli.offline && cli.json);
    }

    /// Regression guard: global flags before a real subcommand must leave it
    /// a subcommand (clap's args-conflict-with-subcommands mode broke this).
    #[test]
    fn subcommands_and_help_are_left_alone() {
        for argv in [
            &["eprint", "--json", "cache", "list"][..],
            &["eprint", "--auto-sync", "yes", "sync"],
            &["eprint", "p", "2024/463"],
            &["eprint", "help"],
            &["eprint", "--version"],
            &["eprint"],
        ] {
            assert_eq!(expanded(argv), argv, "{argv:?}");
        }
        assert!(matches!(
            parse(&["eprint", "--json", "cache", "list"])
                .unwrap()
                .command,
            Command::Cache(_)
        ));
    }

    #[test]
    fn version_flag_still_means_the_tool_version() {
        let err = parse(&["eprint", "--version"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    #[test]
    fn auto_sync_flag_accepts_boolish_values() {
        for (arg, want) in [("yes", true), ("1", true), ("off", false), ("false", false)] {
            let cli = Cli::try_parse_from(["eprint", "--auto-sync", arg, "cache", "path"]).unwrap();
            assert_eq!(cli.auto_sync, Some(want), "--auto-sync {arg}");
        }
    }
}
