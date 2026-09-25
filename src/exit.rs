//! Typed command failures with stable process exit codes.
//!
//! Some failure modes are *silent* today — they log a `warn!` and then either
//! return `Ok(())` or write empty output, so a caller scripting `eprint` (the
//! batch shell loop over paper ids) sees exit 0
//! and assumes success. Tracing alone can't fix that: the configured log level
//! may suppress the warning entirely.
//!
//! [`CommandFailure`] makes each of these a hard error carrying a *distinct*
//! exit code, so callers can branch on **why** a run failed without parsing log
//! text. [`main`](crate) downcasts to this type and exits with [`CommandFailure::code`];
//! any other error keeps anyhow's default exit code 1.

/// A failure with a stable, scriptable exit code.
#[derive(thiserror::Error, Debug)]
pub enum CommandFailure {
    /// No version could be resolved: the archive listing failed and the
    /// OAI fallback was empty, so there is nothing to fetch or file the PDF
    /// under.
    #[error("{0}")]
    NoVersionResolved(String),

    /// No source produced the PDF bytes (not cached under --offline,
    /// HTTP fetch failed or rate-limited past the retry budget, etc.).
    #[error("{0}")]
    PdfUnavailable(String),

    /// Conversion ran but yielded no usable Markdown (the model found no
    /// content on any page, e.g. a blank or unrenderable PDF).
    #[error("{0}")]
    EmptyConversion(String),

    /// Conversion finished, but some pages failed: the Markdown has
    /// placeholders for them, and re-running retries just those pages.
    #[error("{0}")]
    PartialConversion(String),
}

impl CommandFailure {
    /// Process exit code for this failure. Kept stable for scripting; values
    /// above 1 distinguish the reason (1 stays anyhow's catch-all).
    pub fn code(&self) -> i32 {
        match self {
            CommandFailure::NoVersionResolved(_) => 2,
            CommandFailure::PdfUnavailable(_) => 3,
            CommandFailure::EmptyConversion(_) => 4,
            CommandFailure::PartialConversion(_) => 5,
        }
    }

    /// Pull the exit code out of an arbitrary error chain, defaulting to 1 for
    /// anything that isn't a typed [`CommandFailure`].
    pub fn code_of(err: &anyhow::Error) -> i32 {
        err.downcast_ref::<CommandFailure>()
            .map_or(1, CommandFailure::code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_and_distinct() {
        assert_eq!(CommandFailure::NoVersionResolved(String::new()).code(), 2);
        assert_eq!(CommandFailure::PdfUnavailable(String::new()).code(), 3);
        assert_eq!(CommandFailure::EmptyConversion(String::new()).code(), 4);
        assert_eq!(CommandFailure::PartialConversion(String::new()).code(), 5);
    }

    #[test]
    fn code_survives_anyhow_round_trip() {
        let err: anyhow::Error = CommandFailure::PdfUnavailable("nope".into()).into();
        assert_eq!(CommandFailure::code_of(&err), 3);
    }

    #[test]
    fn untyped_errors_default_to_one() {
        let err = anyhow::anyhow!("some other failure");
        assert_eq!(CommandFailure::code_of(&err), 1);
    }
}
