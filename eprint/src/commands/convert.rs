//! `eprint convert` subcommand handler.

use crate::cli::{Context, ConvertArgs};
use anyhow::Result;

pub async fn run(_cx: &Context, _args: ConvertArgs) -> Result<()> {
    anyhow::bail!("eprint convert: not implemented yet")
}
