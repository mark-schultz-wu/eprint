//! `eprint fetch` subcommand handler.

use crate::cli::{Context, FetchArgs};
use anyhow::Result;

pub async fn run(_cx: &Context, _args: FetchArgs) -> Result<()> {
    anyhow::bail!("eprint fetch: not implemented yet")
}
