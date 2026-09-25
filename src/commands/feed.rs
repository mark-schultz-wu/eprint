//! `eprint feed [new|updates]` — read the eprint RSS feed and print recent items.
//! Read-only browse; doesn't touch the cache.

use crate::cli::{Context, FeedArgs, FeedView};
use crate::feed::{self, Item};
use crate::net;
use anyhow::Result;

pub async fn run(cx: &Context, args: FeedArgs) -> Result<()> {
    if cx.offline {
        anyhow::bail!("--offline set; feed requires network");
    }
    let url = build_url(&args);
    let client = net::client(cx.cfg.network.contact.as_deref())?;
    let body = net::get_text(&client, &cx.rate_limiter, &url).await?;
    let items = feed::parse_rss(&body).map_err(|e| anyhow::anyhow!("{e}"))?;
    let shown: Vec<&Item> = items.iter().take(args.limit).collect();
    if cx.json {
        let payload: Vec<_> = shown
            .iter()
            .map(|it| {
                serde_json::json!({
                    "title": it.title,
                    "link": it.link,
                    "pub_date": it.pub_date,
                    "category": it.category,
                    "authors": it.authors,
                    "description": it.description,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        for (i, it) in shown.iter().enumerate() {
            println!("{}. {}", i + 1, it.title);
            if !it.authors.is_empty() {
                println!("   {}", it.authors.join(", "));
            }
            if let Some(c) = &it.category {
                println!("   [{}]", c);
            }
            println!("   {}", it.link);
            if let Some(d) = &it.pub_date {
                println!("   {d}");
            }
            println!();
        }
    }
    Ok(())
}

fn build_url(args: &FeedArgs) -> String {
    let mut params: Vec<String> = Vec::new();
    if matches!(args.view, FeedView::New) {
        params.push("order=recent".into());
    }
    if let Some(c) = args.category {
        params.push(format!("category={}", c.as_query()));
    }
    if params.is_empty() {
        feed::RSS_URL.into()
    } else {
        format!("{}?{}", feed::RSS_URL, params.join("&"))
    }
}
