//! `eprint check` subcommand handler.

use crate::cli::{Context, CheckArgs};
use anyhow::Result;

pub async fn run(_cx: &Context, _args: CheckArgs) -> Result<()> {
    anyhow::bail!("eprint check: not implemented yet")
}
