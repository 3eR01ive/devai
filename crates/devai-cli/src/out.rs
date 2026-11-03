//! Central stdout channel for the CLI (REQ-PIPE-01).
//!
//! Every stdout write the CLI performs goes through this module. When the
//! reader is gone (`devai task status … | head` closes the pipe), the process
//! dies quietly with exit code 0 — SIGPIPE-ordered silence chosen for the
//! open question in the spec — and NEVER panics inside `println!`'s std
//! `io/stdio.rs` unwrap. Any other write error is a real failure: it stays
//! visible on stderr with a nonzero exit (REQ-PIPE-02), exactly like logical
//! errors (`task status <bad-id>`), which keep going through `eprintln!` +
//! `Err` as before.
//!
//! `main` installs a panic-hook backstop for stdout writes outside this
//! module's control (clap's own help/version printing): a stdout-EPIPE panic
//! there also dies quietly.

use std::io::Write;

/// Print one line to stdout (the `outln!` engine). Pipe death → quiet exit.
pub fn line(args: std::fmt::Arguments) {
    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    let r = h
        .write_fmt(args)
        .and_then(|_| h.write_all(b"\n"))
        .and_then(|_| h.flush());
    match r {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(0),
        Err(e) => {
            eprintln!("devai: stdout write failed: {e}");
            std::process::exit(1);
        }
    }
}

/// Write raw bytes to stdout (artifact content). BrokenPipe → quiet exit,
/// any other error propagates to the caller for normal reporting.
pub fn write_bytes_all(bytes: &[u8]) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    let r = h.write_all(bytes).and_then(|_| h.flush());
    if r.as_ref().err().map(|e| e.kind()) == Some(std::io::ErrorKind::BrokenPipe) {
        std::process::exit(0);
    }
    r
}

/// `println!` with BrokenPipe silence: same macro shape, routed through
/// [`line`]. This is the single stdout mechanism for the whole CLI — no
/// per-site try/print logic exists anywhere else (REQ-PIPE-01 AC4).
#[macro_export]
macro_rules! outln {
    ($($arg:tt)*) => { $crate::out::line(format_args!($($arg)*)) };
}
