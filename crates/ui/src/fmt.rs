//! Human-friendly formatting for durations, byte sizes, and counts.

use std::time::Duration;

/// `840ms`, `1.24s`, `1m05s`.
pub fn duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        return format!("{secs:.2}s");
    }
    let m = (secs / 60.0).floor() as u64;
    let s = (secs - (m * 60) as f64).round() as u64;
    format!("{m}m{s:02}s")
}

/// `512 B`, `1.4 MB`, `2.0 GB` (binary units).
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut size = n as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

/// `1,024` with comma thousands separators.
pub fn count(n: u64) -> String {
    let s = n.to_string();
    let digits = s.as_bytes();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, b) in digits.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_scale() {
        assert_eq!(duration(Duration::from_millis(840)), "840ms");
        assert_eq!(duration(Duration::from_millis(1240)), "1.24s");
        assert_eq!(duration(Duration::from_secs(65)), "1m05s");
    }

    #[test]
    fn bytes_scale() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KB");
    }

    #[test]
    fn counts_get_separators() {
        assert_eq!(count(1024), "1,024");
        assert_eq!(count(7), "7");
    }
}
