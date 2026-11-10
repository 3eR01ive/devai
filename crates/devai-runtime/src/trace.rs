//! Best-effort side effects leave a TRACE, never silence (rework R5).
//!
//! Three writes on the launch/observation path are deliberately best effort:
//! a role's ANSWER must never be lost because its log tail could not be
//! written, and the caller may already be gone when the runner records what
//! happened. Best effort is not the same as silence, though: a swallowed
//! `io::Error` is an invisible failure — the operator sees «no tail», «no
//! activity», «no outcome» and cannot tell a missing file from a broken one.
//!
//! So every such write reports its failure on the platform's JOURNAL: the
//! process's stderr, which every platform-owned process redirects into the
//! run's own evidence log (`supervisor.log`). The write itself stays best
//! effort — losing the diagnostic must not turn into a new failure.

use std::io::Write;
use std::path::Path;

/// One best-effort failure, rendered as one line: WHAT was being written,
/// WHERE, and WHY it did not land. The caller supplies the sink, so the
/// contract (a single line naming all three facts) is testable without
/// capturing the process's stderr.
pub fn write_failure(
    out: &mut dyn Write,
    what: &str,
    target: &Path,
    err: &dyn std::fmt::Display,
) -> std::io::Result<()> {
    writeln!(
        out,
        "devai: best-effort {what} was not written to {}: {err}",
        target.display()
    )
}

/// The same line on the platform journal (stderr). A failure of THIS write is
/// the only thing that stays swallowed: stderr may be closed, and a
/// diagnostic must never become the failure it reports.
pub fn note_failure(what: &str, target: &Path, err: &dyn std::fmt::Display) {
    let _ = write_failure(&mut std::io::stderr(), what, target, err);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R5: a swallowed write is not silence — the line names the subject, the
    /// path and the reason, in ONE line (so it reads as one journal entry).
    #[test]
    fn a_best_effort_failure_names_subject_path_and_reason_in_one_line() {
        let mut out: Vec<u8> = Vec::new();
        let target = Path::new("/home/x/evidence/task/run/outcome.json");
        write_failure(
            &mut out,
            "run outcome",
            target,
            &"No such file or directory (os error 2)",
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 1, "{text:?}");
        assert!(text.contains("run outcome"), "{text:?}");
        assert!(text.contains(target.to_str().unwrap()), "{text:?}");
        assert!(text.contains("No such file or directory"), "{text:?}");
    }
}
