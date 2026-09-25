//! Print a [`PaperReport`] in either human-readable or JSON form.

use crate::cache;
use crate::cli::{Context, PaperArgs};
use crate::commands::format::fmt_bytes;
use crate::commands::paper::PaperReport;
use crate::ids::PaperId;
use anyhow::Result;

pub async fn print(cx: &Context, args: &PaperArgs, report: &PaperReport) -> Result<()> {
    if cx.json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!("{}", report.id);
    if let Some(t) = &report.title {
        field("title", t);
    }
    if let Some(v) = &report.current_version {
        field("current version", v);
    }
    if Some(&report.resolved_version) != report.current_version.as_ref() {
        field("resolved to", report.resolved_version);
    }
    if !report.known_versions.is_empty() {
        let total = report.known_versions.len();
        let cached = report.cached_versions.len();
        field("versions", format!("{total} known, {cached} cached"));
        for v in report.known_versions.iter().rev() {
            let mut tags = Vec::new();
            if Some(v) == report.current_version.as_ref() {
                tags.push("current");
            }
            if report.cached_versions.contains(v) {
                tags.push("cached");
            }
            if tags.is_empty() {
                println!("    {v}");
            } else {
                println!("    {v}  ({})", tags.join(", "));
            }
        }
    }
    if let Some(c) = &report.md_converter {
        field("markdown", format!("{}/paper.md ({c})", report.directory));
    }
    if !report.actions.is_empty() {
        field("did", report.actions.join(", "));
    }
    if report.bytes_downloaded > 0 {
        field("downloaded", fmt_bytes(report.bytes_downloaded));
    }
    if !args.no_abstract {
        let id: PaperId = args.id.parse()?;
        let abstract_path =
            cache::version_paths(&cx.cfg.cache_root, id, &report.resolved_version).abstract_;
        if let Ok(abs) = tokio::fs::read_to_string(&abstract_path).await {
            println!();
            println!("Abstract:");
            for line in abs.lines() {
                println!("  {line}");
            }
        }
    }
    Ok(())
}

/// One `  label:  value` line, values aligned in a column.
fn field(label: &str, value: impl std::fmt::Display) {
    println!("  {:<18}{value}", format!("{label}:"));
}
