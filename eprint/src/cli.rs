//! Clap-derive CLI structures.
//!
//! Top-level shape: a bareword `eprint <id>` form (no subcommand) is the
//! default, used for describe/fetch/convert. Explicit subcommands cover
//! discrete operations: `sync`, `feed`, `cache`.

use crate::config::Config;
use clap::{Args, Parser, Subcommand, ValueEnum};
use papermd::Quality;

/// Fetch, describe, and convert IACR ePrint papers.
#[derive(Debug, Parser)]
#[command(name = "eprint", version, about, arg_required_else_help = true)]
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
    /// Run OAI-PMH sync if cache is stale.
    #[arg(long, global = true, env = "EPRINT_AUTO_SYNC", value_parser = clap::value_parser!(bool))]
    pub auto_sync: Option<bool>,
    /// Cache staleness threshold in hours.
    #[arg(long, global = true, env = "EPRINT_SYNC_STALE_HOURS")]
    pub sync_stale_hours: Option<u32>,

    #[command(subcommand)]
    pub command: Command,
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
}

#[derive(Debug, Args)]
pub struct PaperArgs {
    /// Paper id (e.g. "2024/463", "2024-463", or full eprint URL).
    pub id: String,
    /// Operate on a specific historical version (canonical timestamp,
    /// e.g. `20240319T143540Z`). Defaults to current.
    #[arg(long)]
    pub version: Option<String>,
    /// Open an interactive picker over known versions.
    #[arg(long)]
    pub select_version: bool,
    /// Also produce Markdown. With no value, defaults to text-quality.
    #[arg(long, value_enum, num_args = 0..=1, default_missing_value = "text")]
    pub md: Option<MdQuality>,
    /// Skip the staleness check; always hit the network.
    #[arg(long)]
    pub force: bool,
    /// Skip printing the abstract at the bottom of the human-readable output.
    #[arg(long)]
    pub no_abstract: bool,
}

impl PaperArgs {
    pub fn md_quality(&self) -> Option<Quality> {
        self.md.map(|q| match q {
            MdQuality::Text => Quality::Text,
            MdQuality::Ml => Quality::Ml,
        })
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum MdQuality {
    Text,
    Ml,
}

#[derive(Debug, Args)]
pub struct SyncArgs {
    #[arg(long)]
    pub since: Option<String>,
    #[arg(long, default_value_t = 30)]
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
    Clear {
        #[arg(long)]
        dry_run: bool,
    },
}

