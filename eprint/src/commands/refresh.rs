//! `eprint refresh` subcommand handler.

use crate::cli::{Context, RefreshArgs};
use anyhow::Result;

pub async fn run(_cx: &Context, _args: RefreshArgs) -> Result<()> {
    anyhow::bail!("eprint refresh: not implemented yet")
}
