//! Human-readable formatting shared by the commands.

/// `1 paper`, `2 papers`, `3 numbered directories`.
pub fn count(n: usize, noun: &str) -> String {
    match (n, noun.strip_suffix('y')) {
        (1, _) => format!("1 {noun}"),
        (_, Some(stem)) => format!("{n} {stem}ies"),
        (_, None) => format!("{n} {noun}s"),
    }
}

pub fn fmt_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut f = n as f64;
    let mut u = 0;
    while f >= 1024.0 && u < UNITS.len() - 1 {
        f /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{f:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_pluralizes() {
        assert_eq!(count(1, "paper"), "1 paper");
        assert_eq!(count(0, "paper"), "0 papers");
        assert_eq!(count(1, "numbered directory"), "1 numbered directory");
        assert_eq!(count(3, "numbered directory"), "3 numbered directories");
    }

    #[test]
    fn fmt_bytes_uses_binary_units() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1536), "1.5 KB");
        assert_eq!(fmt_bytes(2_326_000_000), "2.2 GB");
        assert_eq!(fmt_bytes(1 << 50), "1048576.0 GB", "GB is the largest unit");
    }
}
