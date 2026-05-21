//! `eprint refresh <ref>` — force a re-check of one paper. If a newer
//! version is detected via OAI-PMH, a new `v{N+1}/` is fetched. If not,
//! reports the current version is fresh.

use crate::cache;
use crate::cli::{Context, RefreshArgs};
use crate::commands::{fetch, sync};
use crate::id::PaperRef;
use crate::net::{self, RateLimiter};
use crate::oai;
use anyhow::{Context as _, Result};
use tracing::info;

pub async fn run(cx: &Context, args: RefreshArgs) -> Result<()> {
    if cx.offline {
        anyhow::bail!("--offline set; refresh requires network");
    }
    let r: PaperRef = args.id.parse().context("parsing paper reference")?;
    anyhow::ensure!(
        r.version.is_none(),
        "`refresh` does not accept a version pin ({})",
        r
    );
    let root = &cx.cfg.cache_root;

    // 1. Hit OAI-PMH with from=<now - some_window> to pick up this paper
    //    if it was recently modified. Cheaper: GetRecord for just this id.
    let client = net::client(cx.cfg.network.contact.as_deref())?;
    let rl = RateLimiter::new(net::rate_limit_path(root), cx.cfg.network.min_interval_s);
    let url = format!(
        "{}?verb=GetRecord&identifier=oai:eprint.iacr.org:{}/{}&metadataPrefix=oai_dc",
        oai::BASE_URL,
        r.id.year,
        r.id.num
    );
    let body = net::get_text(&client, &rl, &url).await?;
    let page = oai::parse_page(&body).context("parsing OAI-PMH GetRecord response")?;

    if let Some(rec) = page.records.first() {
        let mut pm = cache::read_paper_meta(root, r.id).await;
        let need_write = pm
            .latest_known_oai_datestamp
            .as_deref()
            .map(|existing| existing.as_bytes() < rec.datestamp.as_bytes())
            .unwrap_or(true);
        if need_write {
            pm.latest_known_oai_datestamp = Some(rec.datestamp.clone());
            cache::write_paper_meta(root, r.id, &pm).await?;
            info!(id = %r.id, datestamp = %rec.datestamp, "bumped latest_known_oai_datestamp");
        }
    } else {
        anyhow::bail!("OAI-PMH returned no record for {}", r.id);
    }

    if args.dry_run {
        println!("{}: would call fetch (dry-run)", r.id);
        return Ok(());
    }
    if cx.offline {
        return Ok(());
    }
    // Now the regular fetch path: it will see the bumped datestamp and
    // either noop (if v{current}.oai_datestamp is up to date) or pull v{N+1}.
    let _ = sync::maybe_auto_sync; // sync_hook noop; refresh already did its own
    let report = fetch::fetch_ref(cx, PaperRef::current(r.id)).await?;
    if cx.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} → v{} ({})",
            report.id, report.version, report.action
        );
    }
    Ok(())
}
