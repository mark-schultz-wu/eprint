//! Print a [`PaperReport`] in either human-readable or JSON form.

use crate::cache;
use crate::cli::{Context, PaperArgs};
use crate::commands::paper::PaperReport;
use crate::id::PaperId;
use anyhow::Result;

pub async fn print(cx: &Context, args: &PaperArgs, report: &PaperReport) -> Result<()> {
    if cx.json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!("{}", report.id);
    if let Some(t) = &report.title {
        println!("  title: {t}");
    }
    if let Some(v) = &report.current_version {
        println!("  current version: {v}");
    }
    if let Some(v) = &report.resolved_version {
        if Some(v) != report.current_version.as_ref() {
            println!("  resolved to:     {v}");
        }
    }
    if !report.known_versions.is_empty() {
        let total = report.known_versions.len();
        let cached = report.cached_versions.len();
        println!("  versions: {total} known, {cached} cached");
        for v in report.known_versions.iter().rev() {
            let mut tags = Vec::new();
            if Some(v) == report.current_version.as_ref() { tags.push("current"); }
            if report.cached_versions.contains(v) { tags.push("cached"); }
            let tag_str = if tags.is_empty() {
                String::new()
            } else {
                format!("   ({})", tags.join(", "))
            };
            println!("{v}{tag_str}");
        }
    }
    if let Some(q) = &report.md_quality {
        println!("  markdown:        {q}");
    }
    if !report.actions.is_empty() {
        println!("  did:             {}", report.actions.join(", "));
    }
    if report.bytes_downloaded > 0 {
        println!("  bytes:           {}", report.bytes_downloaded);
    }
    if !args.no_abstract {
        if let Some(v) = &report.resolved_version {
            let id: PaperId = args.id.parse()?;
            let abstract_path = cache::version_paths(&cx.cfg.cache_root, id, v).abstract_;
            if let Ok(abs) = tokio::fs::read_to_string(&abstract_path).await {
                println!();
                println!("Abstract:");
                for line in abs.lines() {
                    println!("  {line}");
                }
            }
        }
    }
    Ok(())
}
