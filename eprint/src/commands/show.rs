//! `eprint show` subcommand handler.

use crate::cli::{Context, ShowArgs};
use anyhow::Result;

pub async fn run(_cx: &Context, _args: ShowArgs) -> Result<()> {
    anyhow::bail!("eprint show: not implemented yet")
}
