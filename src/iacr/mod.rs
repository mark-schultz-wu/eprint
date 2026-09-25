//! Clients for eprint.iacr.org.
//!
//! - [`site`]: every eprint URL, under a configurable base.
//! - [`http`]: the HTTP client, rate limiter, and 429 back-off every request
//!   goes through.
//! - [`archive`]: the `/archive/versions/<id>` listing of a paper's revisions.
//! - [`landing`]: a paper's landing page (title, BibTeX, abstract).
//! - [`oai`]: the OAI-PMH endpoint (bulk sync, per-paper `GetRecord`).
//! - [`rss`]: the RSS feed of new and updated papers.

pub mod archive;
pub mod http;
pub mod landing;
pub mod oai;
pub mod rss;
pub mod site;
