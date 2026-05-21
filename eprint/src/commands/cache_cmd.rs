//! `eprint cache` subcommand handler.

use crate::cli::{CacheArgs, CacheCommand, Context};
use anyhow::Result;

pub async fn run(cx: &Context, args: CacheArgs) -> Result<()> {
    match args.command {
        CacheCommand::Path => {
            println!("{}", cx.cfg.cache_root().display());
            Ok(())
        }
        CacheCommand::List => anyhow::bail!("eprint cache list: not implemented yet"),
        CacheCommand::Clear { dry_run: _ } => {
            anyhow::bail!("eprint cache clear: not implemented yet")
        }
    }
}
