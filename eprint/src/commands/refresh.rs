//! `eprint refresh <ref>` — force a re-check of one paper. If a newer
//! version is detected via OAI-PMH, a new `v{N+1}/` is fetched. If not,
//! reports the current version is fresh.

use crate::cache;
use crate::cli::{Context, RefreshArgs};
use crate::commands::fetch;
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
        "refresh always targets the latest version; remove the @vN suffix \
         from {} (to read a specific cached version, use `fetch <id>@vN`)",
        r
    );
    let root = &cx.cfg.cache_root;

    // GetRecord for just this paper. The datestamp it returns is what we'd
    // write into latest_known_oai_datestamp.
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
    let rec = page
        .records
        .first()
        .ok_or_else(|| anyhow::anyhow!("OAI-PMH returned no record for {}", r.id))?;

    let mut pm = cache::read_paper_meta(root, r.id).await;
    let need_write = match pm.latest_known_oai_datestamp.as_deref() {
        Some(existing) => oai::datestamp_cmp(existing, &rec.datestamp)?.is_lt(),
        None => true,
    };

    if args.dry_run {
        if need_write {
            println!(
                "{}: would bump latest_known_oai_datestamp -> {} and fetch new version",
                r.id, rec.datestamp
            );
        } else {
            println!("{}: already at {}, no fetch needed", r.id, rec.datestamp);
        }
        return Ok(());
    }

    if need_write {
        pm.latest_known_oai_datestamp = Some(rec.datestamp.clone());
        cache::write_paper_meta(root, r.id, &pm).await?;
        info!(id = %r.id, datestamp = %rec.datestamp, "bumped latest_known_oai_datestamp");
    }

    let report = fetch::fetch_ref(cx, PaperRef::current(r.id)).await?;
    if cx.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{} → v{} ({})", report.id, report.version, report.action);
    }
    Ok(())
}
