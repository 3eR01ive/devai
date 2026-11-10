//! THE ONE implementation of «хвост последних N строк» (QA R3, `mt/dry`).
//!
//! The rule is spelled once, here: a stream's tail is its last `lines` lines,
//! joined with `\n`, no trailing newline; a stream shorter than that is itself.
//! Every reader of a stored transcript goes through this module, so the bound
//! is edited in ONE place.
//!
//! The second job of this module is honesty about ABSENCE: an empty tail and an
//! unreadable file are DIFFERENT facts (previously four call sites collapsed
//! them, three of them silently). [`read_tail`] and [`trim_to_tail`] return a
//! [`TailError`] that names which one happened, so a caller can neither print
//! «nothing yet» for a permissions problem nor treat a missing evidence file as
//! an empty log.

use std::path::Path;

/// The ratified tail bound (OQ-1 answer: «4096 строк»).
pub const TAIL_LINES: usize = 4096;

/// Why a transcript could not be read or bounded — never conflated with an
/// empty tail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TailError {
    #[error("no such file: {0}")]
    NotFound(String),
    #[error("cannot read {0}: {1}")]
    Unreadable(String, String),
}

/// The tail of `text`: its last `lines` lines.
pub fn tail_of(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

/// The tail of raw bytes (lossy UTF-8, exactly like the settled writer).
pub fn tail_of_bytes(bytes: &[u8], lines: usize) -> String {
    tail_of(&String::from_utf8_lossy(bytes), lines)
}

/// The tail of a file on disk. A file that exists and reads back EMPTY yields
/// an empty string; a missing or unreadable one yields a typed error.
pub fn read_tail(path: &Path, lines: usize) -> Result<String, TailError> {
    let bytes = std::fs::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => TailError::NotFound(path.display().to_string()),
        _ => TailError::Unreadable(path.display().to_string(), e.to_string()),
    })?;
    Ok(tail_of_bytes(&bytes, lines))
}

/// Bound a stored stream file to the tail IN PLACE. `Ok(false)` means «already
/// short, nothing to do»; an unreadable/unwritable file is a typed error, not
/// a silent no-op.
pub fn trim_to_tail(path: &Path, lines: usize) -> Result<bool, TailError> {
    let bytes = std::fs::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => TailError::NotFound(path.display().to_string()),
        _ => TailError::Unreadable(path.display().to_string(), e.to_string()),
    })?;
    let text = String::from_utf8_lossy(&bytes);
    if text.lines().count() <= lines {
        return Ok(false);
    }
    std::fs::write(path, format!("{}\n", tail_of(&text, lines)))
        .map_err(|e| TailError::Unreadable(path.display().to_string(), e.to_string()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_keeps_the_last_lines_only() {
        assert_eq!(tail_of("a\nb\nc\nd\n", 2), "c\nd");
        assert_eq!(tail_of("a\nb", TAIL_LINES), "a\nb");
        assert_eq!(tail_of("", 5), "");
    }

    #[test]
    fn an_absent_file_is_a_typed_miss_not_an_empty_tail() {
        let d = tempfile::TempDir::new().unwrap();
        let absent = d.path().join("absent");
        assert_eq!(
            read_tail(&absent, 2),
            Err(TailError::NotFound(absent.display().to_string()))
        );
        let empty = d.path().join("empty");
        std::fs::write(&empty, "").unwrap();
        assert_eq!(read_tail(&empty, 2), Ok(String::new()));
    }

    #[test]
    fn trimming_bounds_a_long_file_and_reports_nothing_to_do() {
        let d = tempfile::TempDir::new().unwrap();
        let p = d.path().join("stream");
        let body: String = (1..=TAIL_LINES + 4)
            .map(|i| format!("line-{i}\n"))
            .collect();
        std::fs::write(&p, body).unwrap();
        assert_eq!(trim_to_tail(&p, TAIL_LINES), Ok(true));
        let kept = std::fs::read_to_string(&p).unwrap();
        assert_eq!(kept.lines().count(), TAIL_LINES);
        assert_eq!(trim_to_tail(&p, TAIL_LINES), Ok(false));
    }
}
