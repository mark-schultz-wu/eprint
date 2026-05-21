//! Clap-derive CLI structures.

use crate::config::Config;
use clap::{Args, Parser, Subcommand, ValueEnum};
use papermd::Quality;
use std::path::PathBuf;

/// Fetch and convert IACR ePrint papers.
#[derive(Debug, Parser)]
#[command(name = "eprint", version, about)]
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
    /// Run OAI-PMH sync if cache is stale (overrides `EPRINT_AUTO_SYNC`).
    #[arg(long, global = true, env = "EPRINT_AUTO_SYNC", value_parser = clap::value_parser!(bool))]
    pub auto_sync: Option<bool>,
    /// Cache staleness threshold in hours (overrides `EPRINT_SYNC_STALE_HOURS`).
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
    /// Fetch a paper's artifacts (PDF, BibTeX, abstract) into the cache.
    Fetch(FetchArgs),
    /// Print cached metadata for a paper.
    Show(ShowArgs),
    /// Convert a paper's PDF to Markdown.
    Convert(ConvertArgs),
    /// Re-fetch all artifacts for a paper, replacing the cached copies.
    Refresh(RefreshArgs),
    /// Report cache staleness for a paper.
    Check(CheckArgs),
    /// Cache management.
    Cache(CacheArgs),
    /// Run an OAI-PMH sync: annotate cached papers with newer
    /// modification dates. Does NOT download PDFs; next `fetch` does.
    Sync(SyncArgs),
    /// Browse the eprint RSS feed.
    Feed(FeedArgs),
}

/// Shared bag of CLI-wide context passed to each subcommand handler.
pub struct Context {
    pub cfg: Config,
    pub offline: bool,
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct FetchArgs {
    /// Paper id, e.g. "2024/463".
    pub id: String,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    pub id: String,
}

#[derive(Debug, Args)]
pub struct ConvertArgs {
    pub id: String,
    /// Output quality tier.
    #[arg(long, value_enum, default_value_t = QualityArg::Text)]
    pub quality: QualityArg,
    /// Write markdown to this path instead of stdout.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum QualityArg {
    Text,
    Ml,
}

impl From<QualityArg> for Quality {
    fn from(q: QualityArg) -> Self {
        match q {
            QualityArg::Text => Quality::Text,
            QualityArg::Ml => Quality::Ml,
        }
    }
}

#[derive(Debug, Args)]
pub struct RefreshArgs {
    pub id: String,
    /// Print the actions that would be taken without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    pub id: String,
}

#[derive(Debug, Args)]
pub struct FeedArgs {
    /// Which feed view to use.
    #[arg(value_enum, default_value_t = FeedView::Updates)]
    pub view: FeedView,
    /// Filter by eprint category.
    #[arg(long, value_enum)]
    pub category: Option<FeedCategory>,
    /// Maximum number of items to display.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
}

/// `new` = items ordered by publication date (`?order=recent`). `updates`
/// = items ordered by last-modified (default eprint ordering — revisions
/// bump back to the top).
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
pub struct SyncArgs {
    /// ISO date (or datetime) to start from. Overrides the cached
    /// `.last_sync_unix_s` timestamp.
    #[arg(long)]
    pub since: Option<String>,
    /// If no last-sync timestamp and `--since` not given, default to N days ago.
    #[arg(long, default_value_t = 30)]
    pub default_window_days: u32,
}

#[derive(Debug, Args)]
pub struct CacheArgs {
    #[command(subcommand)]
    pub command: CacheCommand,
}

#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// Print the cache root path.
    Path,
    /// List cached papers (with sizes).
    List,
    /// Delete all cached papers.
    Clear {
        /// Print what would be deleted without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
}
