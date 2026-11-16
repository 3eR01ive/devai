//! The MODEL-BUNDLE PROBE (task 01a1b71f, REQ-MS-03).
//!
//! One real, non-interactive call of the SAME driver a run would use, with
//! the SAME launch mechanism — `claude -p` / `hermes -z` — carrying one
//! phrase from the bundle's declared policy. No role, no tools, no contract:
//! the probe answers exactly one question («does this bundle answer at
//! all?») and it answers it BEFORE a switch, so a dead provider cannot show
//! up half an hour later as silently stalled tasks.
//!
//! Facts, not judgments: the verdict is a closed vocabulary entry
//! (`devai_core::types::ProbeVerdict`), the evidence (exit code, bounded
//! output fragment, latency, time) is kept beside it. A sample table below
//! maps the OBSERVED provider refusals (402/404/401 are observations of
//! 2026-11-07, not a contract) onto the vocabulary; anything unrecognised is
//! `unknown` — «не доказано», never a fabricated «no credits».
//!
//! The probe changes NOTHING: no row, no marker, no config. It returns its
//! outcome; the caller decides what to write.

use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use chrono::Utc;
use devai_core::types::{ProbeVerdict, Timestamp};
use serde::Deserialize;

/// The bounded evidence fragment kept with every probe (REQ-MS-05: no
/// unlimited output in history).
pub const OUTPUT_EXCERPT_LIMIT: usize = 600;

/// The factory probe phrase — one source with the loader's default (R6).
pub use devai_core::types::DEFAULT_PROBE_PHRASE;

/// One probe request: the identity to call plus its declared policy.
#[derive(Clone, Debug)]
pub struct ProbeRequest {
    pub driver: String,
    pub llm_provider: String,
    pub model: String,
    /// the ratified OQ-6 answer: the reasoning level of the selection — passed to the driver
    /// exactly as a run would pass it (part of the bundle, not a default).
    pub reasoning: String,
    pub phrase: String,
    pub timeout_ms: u64,
}

impl ProbeRequest {
    pub fn new(driver: &str, llm_provider: &str, model: &str, reasoning: &str) -> Self {
        Self {
            driver: driver.to_string(),
            llm_provider: llm_provider.to_string(),
            model: model.to_string(),
            reasoning: reasoning.to_string(),
            phrase: DEFAULT_PROBE_PHRASE.to_string(),
            timeout_ms: devai_core::types::DEFAULT_PROBE_TIMEOUT_MS,
        }
    }

    pub fn with_policy(mut self, phrase: &str, timeout_ms: u64) -> Self {
        if !phrase.trim().is_empty() {
            self.phrase = phrase.to_string();
        }
        if timeout_ms > 0 {
            self.timeout_ms = timeout_ms;
        }
        self
    }
}

/// The stored evidence of one probe.
#[derive(Clone, Debug)]
pub struct ProbeOutcome {
    pub verdict: ProbeVerdict,
    pub latency_ms: u64,
    pub exit_code: Option<i32>,
    pub output_excerpt: String,
    pub at: Timestamp,
}

impl ProbeOutcome {
    fn unprobeable() -> Self {
        Self {
            verdict: ProbeVerdict::NotProbeable,
            latency_ms: 0,
            exit_code: None,
            output_excerpt: String::new(),
            at: Utc::now(),
        }
    }
}

/// The measured result of one child process, shared by `probe` and by the
/// deadline test that must exercise THIS code rather than a copy of it.
struct RunOutcome {
    exit_code: Option<i32>,
    timed_out: bool,
    combined: String,
    latency_ms: u64,
    /// Set when the binary could not be launched at all (a LOCAL fact, not a
    /// provider refusal).
    spawn_error: Option<String>,
}

/// Run `<argv[0]> <argv[1..]>` under a hard wall-clock deadline. The child
/// is killed and reaped at the deadline, so a probe never outlives its
/// timeout. Output is drained on separate threads: a child that fills a pipe
/// buffer would otherwise block forever and defeat the deadline.
fn run_argv(argv: &[String], timeout_ms: u64) -> RunOutcome {
    let started = Instant::now();
    let mut child = match spawn_argv(argv) {
        Ok(child) => child,
        Err(outcome) => return outcome,
    };
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout = std::thread::spawn(move || drain(stdout_pipe));
    let stderr = std::thread::spawn(move || drain(stderr_pipe));
    let (status, timed_out) = wait_child(&mut child, timeout_ms);
    RunOutcome {
        exit_code: status.and_then(|s| s.code()),
        timed_out,
        combined: join_output(
            stdout.join().unwrap_or_default(),
            stderr.join().unwrap_or_default(),
        ),
        latency_ms: started.elapsed().as_millis() as u64,
        spawn_error: None,
    }
}

/// Start the child with piped output and no stdin, leading its OWN process
/// group. A binary that cannot be launched at all is a LOCAL fact: the outcome
/// carries it as `spawn_error`.
///
/// The group matters for the deadline: a driver that forks a helper (a shell
/// running one command, a wrapper around a model client) leaves a descendant
/// holding the inherited pipes. Killing only the direct child would let that
/// descendant keep the pipes open, and the readers would wait for it instead
/// of honouring the deadline.
fn spawn_argv(argv: &[String]) -> Result<Child, RunOutcome> {
    let started = Instant::now();
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            // setpgid(0, 0): the child becomes the leader of a fresh group, so
            // `kill(-pid, …)` reaches everything it starts.
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn().map_err(|e| RunOutcome {
        exit_code: None,
        timed_out: false,
        combined: bounded(&format!("cannot launch `{}`: {e}", argv[0])),
        latency_ms: started.elapsed().as_millis() as u64,
        spawn_error: Some(e.to_string()),
    })
}

/// Take down the child AND everything it started, then reap it. The spawn put
/// the child at the head of its own process group, so a negative pid signals
/// the whole group: descendants die with it and the pipes close at once.
fn kill_and_reap(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // SAFETY: plain syscall with a pid this process owns; a group that is
        // already gone just fails, which is why the result is ignored.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Poll the child until it exits or the deadline passes. At the deadline the
/// child is killed AND reaped before this returns (REQ-MS-03 AC1), together
/// with any descendant that would otherwise hold its pipes open.
fn wait_child(child: &mut Child, timeout_ms: u64) -> (Option<ExitStatus>, bool) {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1));
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return (Some(status), false),
            Ok(None) if Instant::now() >= deadline => {
                kill_and_reap(child);
                return (None, true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                kill_and_reap(child);
                return (None, true);
            }
        }
    }
}

/// The evidence text of one run: stderr joins stdout when both spoke, and a
/// silent side never leaves a stray newline in the fragment.
fn join_output(out_text: String, err_text: String) -> String {
    if err_text.trim().is_empty() {
        out_text
    } else if out_text.trim().is_empty() {
        err_text
    } else {
        format!("{out_text}\n{err_text}")
    }
}

/// Run one probe. Never hangs past `timeout_ms` + a fixed reap allowance: the
/// child is killed at the deadline and reaped before this returns.
pub fn probe(req: &ProbeRequest) -> ProbeOutcome {
    let argv = match argv_of(req) {
        Ok(argv) => argv,
        // `mock` (and any driver without a network leg) is not probed at
        // all: no process is started (REQ-MS-03 AC6).
        Err(verdict) => {
            let mut out = ProbeOutcome::unprobeable();
            out.verdict = verdict;
            return out;
        }
    };
    let run = run_argv(&argv, req.timeout_ms);
    let mut outcome = ProbeOutcome {
        verdict: ProbeVerdict::Unknown,
        latency_ms: run.latency_ms,
        exit_code: run.exit_code,
        output_excerpt: bounded(&run.combined),
        at: Utc::now(),
    };
    outcome.verdict = if run.spawn_error.is_some() {
        // A missing binary is a LOCAL fact, not a provider refusal: there is
        // no evidence to classify, so the verdict is unknown.
        ProbeVerdict::Unknown
    } else if run.timed_out {
        ProbeVerdict::Timeout
    } else if run.exit_code == Some(0) {
        ProbeVerdict::Ok
    } else {
        classify(&run.combined)
    };
    outcome
}

/// Build `<binary> <args…>`. `Err(verdict)` means "do not run a process".
fn argv_of(req: &ProbeRequest) -> Result<Vec<String>, ProbeVerdict> {
    match req.driver.as_str() {
        "claude" => Ok(claude_argv(req)),
        "hermes" => Ok(hermes_argv(req)),
        // `mock` is a diagnostic driver with no network leg: not probeable.
        "mock" => Err(ProbeVerdict::NotProbeable),
        _ => Err(ProbeVerdict::Unknown),
    }
}

/// `claude -p <phrase> --output-format json`, no session, no tools, then the
/// declared model and the declared effort level.
fn claude_argv(req: &ProbeRequest) -> Vec<String> {
    let mut argv = vec![
        "claude".to_string(),
        "-p".to_string(),
        req.phrase.clone(),
        "--output-format".to_string(),
        "json".to_string(),
        "--no-session-persistence".to_string(),
        // No tools: the probe is a phrase, not a role (REQ-MS-03).
        "--allowedTools".to_string(),
        String::new(),
    ];
    push_flag(&mut argv, "--model", &req.model);
    // The level rides the driver's own flag; a bundle without it never
    // loads, so an empty level here is a caller bug, not a reason to invent
    // a default.
    push_flag(&mut argv, "--effort", &req.reasoning);
    argv
}

/// `hermes -z <phrase>`, then the declared model, LLM-provider and level —
/// each flag only when the bundle declares the value.
fn hermes_argv(req: &ProbeRequest) -> Vec<String> {
    let mut argv = vec!["hermes".to_string(), "-z".to_string(), req.phrase.clone()];
    push_flag(&mut argv, "-m", &req.model);
    if crate::adapters::has_provider_override(&req.llm_provider) {
        argv.push("--provider".into());
        argv.push(req.llm_provider.clone());
    }
    push_flag(&mut argv, "--reasoning", &req.reasoning);
    argv
}

/// Append `flag value` only when the value is declared (empty = not passed).
fn push_flag(argv: &mut Vec<String>, flag: &str, value: &str) {
    if !value.is_empty() {
        argv.push(flag.to_string());
        argv.push(value.to_string());
    }
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> String {
    let Some(mut p) = pipe else {
        return String::new();
    };
    let mut buf = Vec::new();
    let _ = p.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

fn bounded(text: &str) -> String {
    let cut: String = text.chars().take(OUTPUT_EXCERPT_LIMIT).collect();
    if text.chars().count() > OUTPUT_EXCERPT_LIMIT {
        format!("{cut}…")
    } else {
        cut
    }
}

/// One row of the observed-refusal DATA (`data/probe_samples.yaml`): a marker
/// found in the provider's output and the verdict token it stands for.
#[derive(Deserialize)]
struct SampleRow {
    marker: String,
    verdict: String,
}

#[derive(Deserialize)]
struct SampleFile {
    #[serde(default)]
    samples: Vec<SampleRow>,
}

/// The observed refusal samples, loaded from the DECLARATIVE data file baked
/// into the binary (R7 quality return: an observation catalogue is data, not
/// a typed const table; adding a sample is a YAML row, not a code edit). A
/// malformed/absent list degrades to «nothing recognised» — `unknown`, which
/// is the honest fail-closed answer, never a panic and never a dead verdict.
fn samples() -> Vec<(String, ProbeVerdict)> {
    let doc: SampleFile = serde_yaml::from_str(include_str!("../data/probe_samples.yaml"))
        .unwrap_or(SampleFile {
            samples: Vec::new(),
        });
    doc.samples
        .into_iter()
        .filter_map(|r| ProbeVerdict::parse(&r.verdict).map(|v| (r.marker, v)))
        .collect()
}

/// Classify a non-zero, non-timed-out exit by the stored evidence. Unknown
/// when the observation data does not speak — «не доказано», never «мёртвая».
/// Order is the data file's order: the first marker wins.
pub fn classify(output: &str) -> ProbeVerdict {
    let haystack = output.to_ascii_lowercase();
    for (marker, verdict) in samples() {
        if haystack.contains(&marker) {
            return verdict;
        }
    }
    ProbeVerdict::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixed_samples_map_onto_the_closed_vocabulary() {
        // The 2026-11-07 observations, verbatim shapes.
        assert_eq!(
            classify("Error code: 402 - {\"error\":\"Insufficient Balance\"}"),
            ProbeVerdict::NoCredits
        );
        assert_eq!(
            classify("404 Model Not Exist: deepseek-v4.1-flash-1017"),
            ProbeVerdict::ModelNotFound
        );
        assert_eq!(
            classify("HTTP 401 Unauthorized: Incorrect API key provided"),
            ProbeVerdict::NoKey
        );
        assert_eq!(
            classify("Connection refused (os error 111)"),
            ProbeVerdict::Network
        );
        // Anything else is not proven — never a dead verdict.
        assert_eq!(classify("weird provider noise"), ProbeVerdict::Unknown);
    }

    #[test]
    fn mock_is_not_probeable_without_a_process() {
        let out = probe(&ProbeRequest::new("mock", "", "", ""));
        assert_eq!(out.verdict, ProbeVerdict::NotProbeable);
        assert!(out.exit_code.is_none());
        assert!(out.output_excerpt.is_empty());
    }

    #[test]
    fn a_clean_exit_is_ok_and_a_missing_binary_is_unknown() {
        // `true` is not a driver name, so the vocabulary refuses it without a
        // process; the runnable path is covered by the timeout test below.
        let out = probe(&ProbeRequest::new("nope", "", "", ""));
        assert_eq!(out.verdict, ProbeVerdict::Unknown);
        assert!(out.exit_code.is_none());
    }

    #[test]
    fn the_deadline_kills_and_reaps_the_process() {
        // `sh` stands in for a provider that never answers. This exercises
        // THE deadline mechanics `probe` runs (`run_argv`), not a copy of
        // them (REQ-MS-03 AC1).
        let started = Instant::now();
        let argv = vec!["sh".to_string(), "-c".to_string(), "sleep 30".to_string()];
        let run = run_argv(&argv, 200);
        assert!(run.timed_out, "the child must be killed at the deadline");
        assert_eq!(run.exit_code, None, "a killed child has no exit code");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline must return promptly"
        );
    }

    #[test]
    fn the_deadline_takes_down_the_children_of_a_driver() {
        // A driver that forks a helper: the helper inherits the pipes, so
        // killing only the direct child leaves the readers waiting for it and
        // the deadline silently becomes "whenever the helper ends" (CI caught
        // exactly that: 30 s instead of 200 ms).
        let started = Instant::now();
        let argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            "sleep 30 & sleep 30".to_string(),
        ];
        let run = run_argv(&argv, 200);
        assert!(run.timed_out, "the child must be killed at the deadline");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "descendants must die with the child, not outlive the deadline"
        );
    }

    #[test]
    fn a_clean_exit_is_ok_and_provider_evidence_is_classified() {
        // Exit 0 → ok, whatever the output says.
        let ok = run_argv(&["sh".into(), "-c".into(), "echo ping".into()], 5_000);
        assert_eq!(ok.exit_code, Some(0));
        assert!(!ok.timed_out);
        // A non-zero exit is classified ONLY from the stored evidence: the
        // same strings `probe` reads back later.
        let dead = run_argv(
            &[
                "sh".into(),
                "-c".into(),
                "echo 'Error code: 402 - {\"error\":\"Insufficient Balance\"}' >&2; exit 1".into(),
            ],
            5_000,
        );
        assert_eq!(dead.exit_code, Some(1));
        assert_eq!(classify(&dead.combined), ProbeVerdict::NoCredits);
    }

    #[test]
    fn the_evidence_fragment_is_bounded() {
        let long = "x".repeat(OUTPUT_EXCERPT_LIMIT * 3);
        assert!(bounded(&long).chars().count() <= OUTPUT_EXCERPT_LIMIT + 1);
    }

    #[test]
    fn argv_carries_the_phrase_and_only_declared_fields() {
        let claude = argv_of(&ProbeRequest::new("claude", "openrouter", "m", "high")).unwrap();
        assert!(claude.contains(&"claude".to_string()));
        assert!(claude.contains(&"m".to_string()));
        // claude chooses its own LLM-provider: no provider flag is invented.
        assert!(!claude.iter().any(|a| a == "--provider"));
        // The ratified OQ-6 answer: the level rides claude's own `--effort`.
        let pos = claude.iter().position(|a| a == "--effort").unwrap();
        assert_eq!(claude[pos + 1], "high");

        let hermes = argv_of(&ProbeRequest::new("hermes", "deepseek", "m", "medium")).unwrap();
        let pos = hermes.iter().position(|a| a == "--provider").unwrap();
        assert_eq!(hermes[pos + 1], "deepseek");
        // And hermes' own `--reasoning`.
        let pos = hermes.iter().position(|a| a == "--reasoning").unwrap();
        assert_eq!(hermes[pos + 1], "medium");
    }

    /// R8 quality return: an EMPTY identity field emits no flag at all — the
    /// driver must not receive a blank model/provider/level argument.
    #[test]
    fn empty_identity_fields_emit_no_flags() {
        let claude = argv_of(&ProbeRequest::new("claude", "", "", "")).unwrap();
        assert!(!claude.iter().any(|a| a == "--model" || a == "--effort"));
        let hermes = argv_of(&ProbeRequest::new("hermes", "", "", "")).unwrap();
        assert!(
            !hermes
                .iter()
                .any(|a| a == "-m" || a == "--provider" || a == "--reasoning")
        );
    }

    /// A binary that cannot be launched is a LOCAL fact, never a provider
    /// refusal: the outcome says `spawn_error` and carries no exit code.
    #[test]
    fn a_missing_binary_is_a_spawn_failure_not_a_provider_verdict() {
        let run = run_argv(&["/nonexistent/devai-probe-binary".to_string()], 1_000);
        assert!(run.spawn_error.is_some(), "the failure must stay local");
        assert_eq!(run.exit_code, None);
        assert!(!run.timed_out);
        assert!(run.combined.contains("cannot launch"), "{}", run.combined);
    }
}
