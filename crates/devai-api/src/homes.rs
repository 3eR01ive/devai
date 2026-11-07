//! The platform homes registry (REQ-HOME-01…09): known project homes as
//! stored platform data, not strings in the memory of whoever launched the
//! process.
//!
//! Shape decisions (implementer zone per the spec's unknowns, ratified by
//! OQ-HOME-1): the registry lives in the DEFAULT home (~/.devai — the
//! control plane) as `homes.json`. It stores HOME DESCRIPTORS ONLY — path,
//! project label, added_at, enabled, stored active marker — never copies of
//! the homes' tasks or projects (REQ-HOME-08: the control plane does not
//! duplicate home contents; every merged-view row resolves to a live read
//! of its source home).
//!
//! The write discipline (REQ-HOME-06): this plane's only writable target is
//! the registry file in the control home. Domain data of an observed home
//! is read through the encapsulated observation open
//! (`Store::open_read_only`, the API-internal sibling of the ratified
//! `Api::open` bootstrap) and NEVER mutated: no migration, no creation, no
//! row writes — a home of any schema rank is readable with its version
//! stamped as data (REQ-HOME-09 with the operator-ratified decision of the
//! approval round: read any version with an explicit mark, write only the
//! compatible active home — silently mutating a foreign store is forbidden).
//!
//! Concurrency (rl/idempotency, closing the gap QA flagged at report v1):
//! every registry change is a read-modify-write TRANSACTION under an
//! exclusive lock on `homes.json.lock` — the disk document is re-read
//! inside the lock, one mutation is applied to the fresh state, and only a
//! changed document is written. Two UI processes therefore cannot lose a
//! record: a late writer applies its change on top of what the first one
//! committed, instead of overwriting it from a stale in-memory copy.
//!
//! Restart semantics (ratified OQ-HOME-7, documented explicitly as asked):
//! an explicitly set `$DEVAI_HOME` is authoritative — the launch home
//! becomes the stored active home; with no env override, the active home
//! is the saved `active` record of the registry. The launch home is
//! auto-registered either way (REQ-HOME-02), so a one-home run is the
//! degenerate case of the registry, not a different mode.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use devai_core::types::{Project, Task};
use devai_store::{DB_FILE_NAME, Store};
use serde::{Deserialize, Serialize};

use crate::codes;
use crate::error::{ApiError, ApiResult};

/// The registry file inside the control home. A physically separate file
/// from the domain store (`Store::DB_FILE_NAME` of devai-store — the one
/// name the observers check) keeps the platform plane out of the schema
/// ladder: observing/adding homes never grows a migration rung.
pub const HOMES_FILE: &str = "homes.json";
/// Format tag of the registry document (forward-compatible shape).
pub const HOMES_FORMAT_VERSION: u32 = 1;
/// Sidecar lock file serializing registry RMW transactions across
/// processes. Kept separate from `homes.json` so the atomic tmp+rename
/// write can never be confused with the guarded document itself.
pub const HOMES_LOCK_FILE: &str = "homes.json.lock";

/// How a home's project label was obtained (R3 quality return,
/// rl/error-handling). The distinction the operator needs: "the store was
/// read and it has no projects" (`Path` — a deterministic shape label) is
/// NOT "the store could not be read" (`Unreadable`, cause carried beside
/// it). The previous code flattened both into one silently-derived string,
/// so a transient read failure became a permanent label with no trace.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSource {
    /// A registry document written before this marker existed — the label
    /// is whatever the previous build stored, derivation unknown.
    #[default]
    Unknown,
    /// Read from the home's own projects (one or several names).
    Store,
    /// The store read fine but holds no projects: the label is the
    /// deterministic `<project>/.devai` path shape.
    Path,
    /// The store could not be opened/read; `project_error` names the cause
    /// and the label is the path fallback, marked as such.
    Unreadable,
}

/// One stored home descriptor — the persisting half of a registry record
/// (REQ-HOME-01: activity is a saved fact, not process memory). Naming
/// pair: `HomeEntry` is what homes.json holds; `HomeRecord` is what the
/// enumeration operation renders for a consumer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HomeEntry {
    pub path: String,
    pub project: String,
    /// How `project` was derived (see `ProjectSource`).
    #[serde(default)]
    pub project_source: ProjectSource,
    /// Cause text when `project_source` is `Unreadable`, None otherwise.
    #[serde(default)]
    pub project_error: Option<String>,
    pub added_at: DateTime<Utc>,
    pub enabled: bool,
    pub active: bool,
}

/// A registry record as the enumeration operation renders it: the stored
/// descriptor plus the attributes probed at read (availability, stored
/// schema rank, and the cause text when the home is absent or of a
/// different version — REQ-HOME-07 / REQ-HOME-09 honesty requirements).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct HomeRecord {
    pub path: String,
    pub project: String,
    /// Provenance of the stored project label (`store` / `path` /
    /// `unreadable` / `unknown`) — the UI states a path-derived label as
    /// such instead of presenting it as the home's own name.
    pub project_source: ProjectSource,
    /// Cause text of an unreadable store when the label fell back to the
    /// path; None when the label has a real source.
    pub project_error: Option<String>,
    pub added_at: DateTime<Utc>,
    pub enabled: bool,
    pub active: bool,
    /// Live probe: the directory and its `devai.db` are present, the
    /// observation open succeeded and its schema rank could be read.
    pub available: bool,
    /// Stored `user_version` of the home, None when unreadable.
    pub schema_version: Option<i64>,
    /// Cause text when the home's schema rank differs from this build
    /// (version + path as data, never a startup failure).
    pub version_note: Option<String>,
    /// Localized, TYPED read error for an unavailable/unopenable home;
    /// other homes keep working (REQ-HOME-07 AC3). R2 quality return: the
    /// cause is `HomeIssue` (code + path + hint + text), not a bare string —
    /// the UI branches on `code`, the text stays for the operator.
    pub read_error: Option<HomeIssue>,
}

/// A typed cause of one home's unavailability as it reaches the wire
/// (R2 quality return, rel/error-context): `code` is the machine token a
/// consumer branches on (`home_unavailable`, `home_no_store`,
/// `home_schema_version` — spelled once in `codes`), `path` names the home
/// the verdict is about, `hint` is the platform's own operator advice and
/// `message` is the human text. The previous shape was `Option<String>`, so
/// a UI could only see "there is text" and never branch on the kind.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct HomeIssue {
    pub code: String,
    pub path: String,
    pub message: String,
    pub hint: Option<String>,
}

impl HomeIssue {
    /// Build an issue from a homes machine code; the hint comes from the
    /// one dictionary in `codes`, never from a call site.
    pub fn new(code: &str, path: &Path, message: impl Into<String>) -> HomeIssue {
        HomeIssue {
            code: code.to_string(),
            path: path.display().to_string(),
            message: message.into(),
            hint: codes::home_hint(code).map(str::to_string),
        }
    }

    /// Classify a failed observation open through the shared mapping: the
    /// store error keeps its own machine code, which is then joined to the
    /// homes vocabulary (no re-parsing of prose).
    fn from_store_error(path: &Path, e: &devai_store::StoreError) -> HomeIssue {
        let api_code = crate::error::store_code(e).0;
        HomeIssue::new(codes::home_failure_code(api_code), path, e.to_string())
    }

    /// Restate the human text while keeping the typed cause (the probe
    /// names which read failed, the code still decides the kind).
    fn with_message(mut self, message: impl Into<String>) -> HomeIssue {
        self.message = message.into();
        self
    }
}

/// One home block of a merged view: the attribution stamps ride beside
/// the records, every row resolves to this live-read home (REQ-HOME-05,
/// REQ-HOME-08). `error` localizes an unreadable home without collapsing
/// the view (`HomeIssue`: machine code + path + hint + text); `records`
/// stays honestly empty then.
#[derive(Serialize, Clone, Debug)]
pub struct HomeRecordsView<T> {
    pub snapshot_at: DateTime<Utc>,
    pub home_path: String,
    pub home_project: String,
    pub is_active: bool,
    pub available: bool,
    pub version_note: Option<String>,
    /// The typed localized error of this block (same shape as
    /// `HomeRecord::read_error` — code + path + hint + text).
    pub error: Option<HomeIssue>,
    pub records: Vec<T>,
}

#[derive(Serialize, Deserialize)]
struct HomesFile {
    version: u32,
    homes: Vec<HomeEntry>,
}

// ---------------------------------------------------------------------
// The declarative record schema (R2 quality return, mt/change-isolation)
// ---------------------------------------------------------------------
//
// One description of the homes wire records: `(field name, TS type)` in the
// order the struct declares them. It is NOT documentation — the parity test
// in `devai-ui/src/commands.rs` compares this table against EVERY place
// that spells the same attributes: the persisted `HomeEntry` serde output,
// the `HomeRecord` serde output (and the `ipc_shapes.json` wire fixture),
// the published `home.list`/`home.projects_all` registry rows, the
// `types.ts` interfaces and the views' field references. An attribute can
// therefore not land in one of those sites alone: adding it to the struct
// without this table fails first, and adding it here without the TS type /
// view usage fails right after.
//
// The tuples of a Rust struct cannot be generated from a const without a
// macro; the ratified alternative was exactly this — one table validated by
// one parity test on all five sites.

/// Fields of one stored registry descriptor (`HomeEntry`, homes.json).
pub const HOME_ENTRY_SCHEMA: &[(&str, &str)] = &[
    ("path", "string"),
    ("project", "string"),
    (
        "project_source",
        "\"store\" | \"path\" | \"unreadable\" | \"unknown\"",
    ),
    ("project_error", "string | null"),
    ("added_at", "string"),
    ("enabled", "boolean"),
    ("active", "boolean"),
];

/// Fields of one enumeration record (`HomeRecord`, `home.list` rows): the
/// persisted descriptor plus the attributes probed at read.
pub const HOME_RECORD_SCHEMA: &[(&str, &str)] = &[
    ("path", "string"),
    ("project", "string"),
    (
        "project_source",
        "\"store\" | \"path\" | \"unreadable\" | \"unknown\"",
    ),
    ("project_error", "string | null"),
    ("added_at", "string"),
    ("enabled", "boolean"),
    ("active", "boolean"),
    ("available", "boolean"),
    ("schema_version", "number | null"),
    ("version_note", "string | null"),
    ("read_error", "HomeIssue | null"),
];

/// Fields of one merged-view block (`HomeRecordsView<T>`).
pub const HOME_VIEW_SCHEMA: &[(&str, &str)] = &[
    ("snapshot_at", "string"),
    ("home_path", "string"),
    ("home_project", "string"),
    ("is_active", "boolean"),
    ("available", "boolean"),
    ("version_note", "string | null"),
    ("error", "HomeIssue | null"),
    ("records", "T[]"),
];

/// Fields of the typed unavailability cause (`HomeIssue`).
pub const HOME_ISSUE_SCHEMA: &[(&str, &str)] = &[
    ("code", "string"),
    ("path", "string"),
    ("message", "string"),
    ("hint", "string | null"),
];

/// Field names of a schema table (the wire contract's key set).
pub fn schema_names<'a>(schema: &'a [(&'a str, &'a str)]) -> Vec<&'a str> {
    schema.iter().map(|(name, _)| *name).collect()
}

/// The result of one observation open of a home. Named fields, because the
/// verdicts are consumer-visible honesty data (REQ-HOME-07/09): a failed
/// PRAGMA is a stated read error, never a flattened "available, version
/// unknown" wash (rl/error-handling, rel/error-context at report v1).
struct HomeProbe {
    /// The store opened and its stored schema rank was readable.
    available: bool,
    schema_version: Option<i64>,
    version_note: Option<String>,
    read_error: Option<HomeIssue>,
}

impl HomeProbe {
    /// A home that could not be observed: typed cause stated, nothing
    /// invented.
    fn failed(issue: HomeIssue) -> HomeProbe {
        HomeProbe {
            available: false,
            schema_version: None,
            version_note: None,
            read_error: Some(issue),
        }
    }

    fn observed(version: i64) -> HomeProbe {
        HomeProbe {
            available: true,
            schema_version: Some(version),
            version_note: version_note(version),
            read_error: None,
        }
    }

    /// The open worked, the row read did not: availability is honestly
    /// revoked (this enumeration could not be produced), cause typed and
    /// localized.
    fn read_failed(mut self, issue: HomeIssue) -> HomeProbe {
        self.available = false;
        self.read_error = Some(issue);
        self
    }
}

/// One observation: the probe verdict plus the rows a read closure
/// produced (or an empty vec when the read failed). Returned as a named
/// struct, not a positional tuple — consumers reach fields by name
/// (cl/meaningful-names at report v1).
struct HomeObservation<T> {
    probe: HomeProbe,
    records: Vec<T>,
}

/// The loaded registry of the control home. Management transactions are
/// the ONLY writes of this plane and land in `homes.json` of the control
/// home; nothing here ever opens a writable store of another home.
/// `entries` is a cache of the last committed document: every mutator
/// re-reads the disk truth inside the lock and syncs this field after the
/// transaction, so the in-memory view can never write a registry change
/// over a document it never saw.
pub struct Homes {
    control_home: PathBuf,
    entries: Vec<HomeEntry>,
}

/// Deterministic path key: canonical when the path exists (symlink-free
/// equality), the raw string otherwise (a launch home may not exist yet).
fn norm(p: &Path) -> String {
    fs::canonicalize(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .display()
        .to_string()
}

fn same_path(stored: &str, given: &Path) -> bool {
    stored == norm(given)
}

/// The registrability verdict of one path (REQ-HOME-03 AC3): an existing
/// directory holding an INITIALISED store — a plain file is not yet a home,
/// and a zero-byte/foreign-format `devai.db` must be refused typed instead
/// of registered. Returns the canonical registry key; nothing is written
/// here, so a refusal leaves disk and registry untouched.
fn resolve_registrable_home(path: &Path) -> ApiResult<String> {
    if !path.is_dir() {
        return Err(ApiError::InvalidInput {
            field: "path",
            message: format!(
                "no directory at {} — a home is an existing folder with a devai store",
                path.display()
            ),
        });
    }
    if !path.join(DB_FILE_NAME).exists() {
        return Err(ApiError::refused(
            codes::HOME_NO_STORE,
            format!(
                "{} has no {DB_FILE_NAME} — initialise it first (`devai init`) or pick a project home",
                path.display()
            ),
        ));
    }
    // The platform's own observer refuses a rank-0 store
    // (`devai-store` read_initialised_rank), so the registration probes the
    // path with the same read-only open.
    if let Err(e) = Store::open_read_only(path) {
        let code = codes::home_failure_code(crate::error::store_code(&e).0);
        return Err(ApiError::refused(
            code,
            format!("{} is not an initialised devai home: {e}", path.display()),
        ));
    }
    Ok(norm(path))
}

/// Version verdict as DATA (REQ-HOME-09): a home behind or ahead of this
/// build is observable; the note names both ranks and the guarantee.
fn version_note(found: i64) -> Option<String> {
    if found == devai_store::SCHEMA_VERSION {
        return None;
    }
    let rel = if found > devai_store::SCHEMA_VERSION {
        "ahead of"
    } else {
        "behind"
    };
    Some(format!(
        "stored schema v{found} is {rel} this build (v{}); rows are shown as stored — the observer never migrates a home it watches",
        devai_store::SCHEMA_VERSION
    ))
}

/// The verdict of reading a home's project label: the label itself, how it
/// was obtained, and — when the store refused — the cause. R3 quality
/// return: the old `label_for` swallowed both a failed open and an empty
/// project list into one string, so a transient failure was pinned as the
/// home's name with no trace.
#[derive(Debug)]
struct LabelProbe {
    label: String,
    source: ProjectSource,
    error: Option<String>,
}

/// Read the project label of a home: the home's own projects when they
/// resolve (one → its name, several → joined names), otherwise the path
/// shape — but the verdict says WHICH of the two happened and why
/// (`ProjectSource`). The label is stored at registration (store-is-cache),
/// never re-derived per read.
fn label_probe(root: &Path) -> LabelProbe {
    let store = match Store::open_read_only(root) {
        Ok(store) => store,
        Err(e) => {
            return path_label(
                root,
                ProjectSource::Unreadable,
                Some(format!(
                    "the store of {} could not be opened to read the project label: {e}",
                    root.display()
                )),
            );
        }
    };
    match store.list_projects() {
        Ok(projects) if !projects.is_empty() => LabelProbe {
            label: projects
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            source: ProjectSource::Store,
            error: None,
        },
        // The store answered with no projects: not an error, the path shape
        // names the home deterministically.
        Ok(_) => path_label(root, ProjectSource::Path, None),
        Err(e) => path_label(
            root,
            ProjectSource::Unreadable,
            Some(format!(
                "reading the project list of {} failed: {e}",
                root.display()
            )),
        ),
    }
}

/// The deterministic path-shape label, carrying the verdict that produced
/// it (the `<project>/.devai` dogma lives with the layout owner — the
/// mechanical half is `devai_store::project_label_from_path`).
fn path_label(root: &Path, source: ProjectSource, error: Option<String>) -> LabelProbe {
    let label = devai_store::project_label_from_path(root)
        .or_else(|| root.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| root.display().to_string());
    LabelProbe {
        label,
        source,
        error,
    }
}

/// One observation open of a home: the probe verdict, rows optional.
/// Encapsulated — the store handle never leaves this module (the
/// `Api::open` bootstrap exception keeps its exact shape: consumers reach
/// homes through these operations, not through stores).
fn observe<T>(
    root: &Path,
    read: impl FnOnce(&Store) -> Result<Vec<T>, devai_store::StoreError>,
) -> HomeObservation<T> {
    let store = match open_observed_store(root) {
        Ok(store) => store,
        Err(probe) => return unobservable(probe),
    };
    let probe = match probe_schema_rank(root, &store) {
        Ok(probe) => probe,
        Err(probe) => return unobservable(probe),
    };
    match read(&store) {
        Ok(rows) => HomeObservation {
            probe,
            records: rows,
        },
        Err(e) => HomeObservation {
            probe: probe.read_failed(HomeIssue::from_store_error(root, &e)),
            records: Vec::new(),
        },
    }
}

/// The observation open of one home: the READ-ONLY store handle, or the
/// typed probe of exactly why this home cannot be read. A vanished
/// directory is `home_unavailable`, not the store's generic `not_found`:
/// the path is the cause, and the UI tells "bring the path back" from
/// "initialise the store" by the code alone (REQ-HOME-07).
fn open_observed_store(root: &Path) -> Result<Store, HomeProbe> {
    if !root.is_dir() {
        return Err(HomeProbe::failed(HomeIssue::new(
            codes::HOME_UNAVAILABLE,
            root,
            format!("home directory {} is not on disk", root.display()),
        )));
    }
    Store::open_read_only(root)
        .map_err(|e| HomeProbe::failed(HomeIssue::from_store_error(root, &e)))
}

/// The stored schema rank of an open observation handle. A PRAGMA failure
/// on an open handle is a localized read error, stated as one
/// (rel/error-context): the home is reported unavailable with this cause,
/// not "available, version unknown".
fn probe_schema_rank(root: &Path, store: &Store) -> Result<HomeProbe, HomeProbe> {
    store
        .schema_version()
        .map(HomeProbe::observed)
        .map_err(|e| {
            HomeProbe::failed(HomeIssue::from_store_error(root, &e).with_message(format!(
                "cannot read the schema rank of {}: {e}",
                root.display()
            )))
        })
}

/// The "nothing can be read here" shape: a typed probe and honestly empty
/// records (never invented rows).
fn unobservable<T>(probe: HomeProbe) -> HomeObservation<T> {
    HomeObservation {
        probe,
        records: Vec::new(),
    }
}

/// Probe without rows — the `home.list` half of the same open discipline
/// (record() and observe() share one probe, so the shape of an
/// unavailable home cannot drift between the two planes).
fn probe(root: &Path) -> HomeProbe {
    observe(root, |_| Ok::<Vec<()>, devai_store::StoreError>(Vec::new())).probe
}

/// One entry rendered with live probe attributes.
fn record_of(e: &HomeEntry) -> HomeRecord {
    let p = probe(Path::new(&e.path));
    HomeRecord {
        path: e.path.clone(),
        project: e.project.clone(),
        project_source: e.project_source,
        project_error: e.project_error.clone(),
        added_at: e.added_at,
        enabled: e.enabled,
        active: e.active,
        available: p.available,
        schema_version: p.schema_version,
        version_note: p.version_note,
        read_error: p.read_error,
    }
}

/// The record of a home whose observation thread panicked (rl/crash-free):
/// typed unavailability with the path kept, never a fabricated row. Named
/// so the fallback itself is exercised by a test instead of living only
/// inside a closure (R4 quality return).
fn panic_record(e: &HomeEntry) -> HomeRecord {
    HomeRecord {
        path: e.path.clone(),
        project: e.project.clone(),
        project_source: e.project_source,
        project_error: e.project_error.clone(),
        added_at: e.added_at,
        enabled: e.enabled,
        active: e.active,
        available: false,
        schema_version: None,
        version_note: None,
        read_error: Some(HomeIssue::new(
            codes::HOME_UNAVAILABLE,
            Path::new(&e.path),
            PANIC_MESSAGE,
        )),
    }
}

/// The merged-view block of a panicked observation thread: the same typed
/// unavailability, localized to that block.
fn panic_block<T>(e: &HomeEntry, now: DateTime<Utc>) -> HomeRecordsView<T> {
    HomeRecordsView {
        snapshot_at: now,
        home_path: e.path.clone(),
        home_project: e.project.clone(),
        is_active: e.active,
        available: false,
        version_note: None,
        error: Some(HomeIssue::new(
            codes::HOME_UNAVAILABLE,
            Path::new(&e.path),
            PANIC_MESSAGE,
        )),
        records: Vec::new(),
    }
}

/// The one text of a panicked observation, so the record and the view block
/// cannot drift apart.
const PANIC_MESSAGE: &str = "the observation thread panicked — state unknown";

/// The observation fan-out (pf/parallelism at report v1): homes are
/// independent stores, so probes run in scoped threads instead of one
/// sequential chain. Order is preserved — the result vec follows the
/// entry vec — and a hypothetical panic in one observation is localized
/// by the caller's fallback instead of propagating through `join`.
fn par_map<A, F, G>(entries: &[&HomeEntry], work: F, on_panic: G) -> Vec<A>
where
    A: Send,
    F: Fn(&HomeEntry) -> A + Sync,
    G: Fn(&HomeEntry) -> A + Sync,
{
    std::thread::scope(|scope| {
        let handles: Vec<_> = entries.iter().map(|e| scope.spawn(|| work(e))).collect();
        handles
            .into_iter()
            .zip(entries.iter())
            .map(|(h, e)| h.join().unwrap_or_else(|_| on_panic(e)))
            .collect()
    })
}

/// Exclusive advisory lock over the registry RMW window (flock on unix).
/// Dropping the guard releases it; the file itself stays as the lock
/// anchor (an inode to lock is cheaper than deleting it between writers).
struct RegistryWriteLock {
    file: fs::File,
}

impl RegistryWriteLock {
    fn acquire(control_home: &Path) -> ApiResult<RegistryWriteLock> {
        // R1 quality return (first-run regression): the lock anchor lives IN
        // the control home, which may not exist yet on a machine that never
        // ran the platform. Creating the directory here — before the lock is
        // opened — is what makes the very first registration possible; the
        // document write itself already ensures the directory, but it lands
        // after the lock, so relying on it alone turned the valid empty
        // state into an ENOENT startup death.
        fs::create_dir_all(control_home)?;
        let path = control_home.join(HOMES_LOCK_FILE);
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            // Blocking exclusive lock: single-operator N is tiny, waiting
            // IS the transaction semantics (the loser re-reads fresh state).
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if rc != 0 {
                return Err(ApiError::Io(std::io::Error::last_os_error()));
            }
        }
        #[cfg(not(unix))]
        {
            // No portable advisory primitive here; the reload-inside-lock
            // below still narrows the race window. ASM-04 keeps this
            // best-effort honest for the single-operator model.
            let _ = &file;
        }
        Ok(RegistryWriteLock { file })
    }
}

impl Drop for RegistryWriteLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
        #[cfg(not(unix))]
        let _ = &self.file;
    }
}

/// Read and parse the registry document of a control home. Shared by
/// `Homes::load` and the transaction reload so both enforce the same
/// strict format gate (corrupt bytes and unknown versions fail loud).
fn read_entries(control_home: &Path) -> ApiResult<Vec<HomeEntry>> {
    let file = control_home.join(HOMES_FILE);
    match fs::read_to_string(&file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(ApiError::Io(e)),
        Ok(text) => {
            let doc: HomesFile = serde_json::from_str(&text).map_err(|e| {
                ApiError::refused(
                    codes::HOMES_REGISTRY_CORRUPT,
                    format!("{} is not a readable homes registry: {e}", file.display()),
                )
            })?;
            if doc.version != HOMES_FORMAT_VERSION {
                return Err(ApiError::refused(
                    codes::HOMES_REGISTRY_CORRUPT,
                    format!(
                        "{} carries homes format v{}, this build reads v{HOMES_FORMAT_VERSION}",
                        file.display(),
                        doc.version
                    ),
                ));
            }
            Ok(doc.homes)
        }
    }
}

/// Persist atomically: a crash between write and read never leaves a
/// half-written registry (tmp file + rename in the control home). Called
/// only while the write lock is held.
fn write_entries(control_home: &Path, entries: &[HomeEntry]) -> ApiResult<()> {
    fs::create_dir_all(control_home)?;
    let doc = HomesFile {
        version: HOMES_FORMAT_VERSION,
        homes: entries.to_vec(),
    };
    let text =
        serde_json::to_string_pretty(&doc).map_err(|e| ApiError::Io(std::io::Error::other(e)))?;
    let file = control_home.join(HOMES_FILE);
    let tmp = control_home.join(format!("{HOMES_FILE}.tmp"));
    fs::write(&tmp, text)?;
    fs::rename(&tmp, &file)?;
    Ok(())
}

/// Exactly-one-active flip over a document; true when the flags changed.
/// Activation IMPLIES inclusion: an `active && !enabled` record is a
/// contradiction (the registry would hold an active home the view refuses
/// to observe), so the entry being marked active is included in the same
/// flip. (The reverse direction — an operator switching the active home
/// off — is ratified OQ-HOME-8 and handled by `set_enabled`, which clears
/// the marker and hands it to `next_enabled_after`.)
fn mark_active(entries: &mut [HomeEntry], key: &str) -> bool {
    let mut changed = false;
    for e in entries.iter_mut() {
        let want = e.path == key;
        if want && !e.enabled {
            e.enabled = true;
            changed = true;
        }
        if e.active != want {
            e.active = want;
            changed = true;
        }
    }
    changed
}

/// The successor of a switched-off active home (ratified OQ-HOME-8): the
/// next ENABLED record in registry order after `from`, wrapping to the
/// first enabled record when the tail holds none. `None` when no enabled
/// record remains at all — the registry then holds the honest "no active
/// home" state instead of a silently chosen default.
fn next_enabled_after(entries: &[HomeEntry], from: usize) -> Option<usize> {
    (from + 1..entries.len())
        .chain(0..from)
        .find(|&i| entries[i].enabled)
}

/// A fresh registry record from a label verdict (registration paths).
fn new_entry(key: &str, probe: &LabelProbe, added_at: DateTime<Utc>) -> HomeEntry {
    HomeEntry {
        path: key.to_string(),
        project: probe.label.clone(),
        project_source: probe.source,
        project_error: probe.error.clone(),
        added_at,
        enabled: true,
        active: false,
    }
}

/// Overwrite a record's label with a freshly probed verdict; true when the
/// stored label/verdict actually changed (registration is the one moment a
/// label is derived — store-is-cache).
fn apply_label(e: &mut HomeEntry, probe: &LabelProbe) -> bool {
    if e.project == probe.label
        && e.project_source == probe.source
        && e.project_error == probe.error
    {
        return false;
    }
    e.project = probe.label.clone();
    e.project_source = probe.source;
    e.project_error = probe.error.clone();
    true
}

impl Homes {
    /// Load the registry of a control home. A missing file is the valid
    /// empty state (REQ-HOME-02 AC: before the first registration the
    /// registry is empty — behaviour as today, one-home mode); a present
    /// but unparsable file fails loud (this file is written only by this
    /// plane — corrupt bytes are never quietly discarded).
    pub fn load(control_home: &Path) -> ApiResult<Homes> {
        Ok(Homes {
            control_home: control_home.to_path_buf(),
            entries: read_entries(control_home)?,
        })
    }

    pub fn control_home(&self) -> &Path {
        &self.control_home
    }

    pub fn entries(&self) -> &[HomeEntry] {
        &self.entries
    }

    fn find(&self, path: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| same_path(&e.path, Path::new(path)))
    }

    /// Re-read the committed document into the in-memory cache. Reads
    /// need no lock: `write_entries` lands via tmp+rename, so a reader
    /// always sees a whole, previously-valid document (or the older one),
    /// never torn bytes.
    fn sync(&mut self) -> ApiResult<()> {
        self.entries = read_entries(&self.control_home)?;
        Ok(())
    }

    /// The one shape every registry write takes: lock, re-read the disk
    /// document fresh, apply the mutation to THAT state, write only when
    /// it changed, sync the in-memory cache with the committed truth.
    /// Returning Err leaves disk and cache untouched.
    fn transact<T>(
        &mut self,
        apply: impl FnOnce(&mut Vec<HomeEntry>) -> ApiResult<(bool, T)>,
    ) -> ApiResult<T> {
        let _lock = RegistryWriteLock::acquire(&self.control_home)?;
        let mut entries = read_entries(&self.control_home)?;
        let (changed, out) = apply(&mut entries)?;
        if changed {
            write_entries(&self.control_home, &entries)?;
        }
        self.entries = entries;
        Ok(out)
    }

    /// Startup auto-registration (REQ-HOME-02) plus the ratified active
    /// resolution (OQ-HOME-7). Returns the path the process must treat as
    /// active:
    ///   * the launch home always ends up registered (a missing path is
    ///     still registered — `Api::open` bootstraps a fresh home, and the
    ///     launch record must survive from the very first start);
    ///   * `$DEVAI_HOME` explicitly set → the launch home becomes active
    ///     (env is authoritative over a saved record, OQ-HOME-7). An
    ///     explicit env choice is also INCLUSION: a record the operator had
    ///     switched off is re-enabled by the env act instead of turning
    ///     into the contradiction `active && !enabled` (R4 quality return;
    ///     the flag is flipped inside `mark_active`, so the invariant holds
    ///     on every activation path, not just this one);
    ///   * no env override → the saved `active` record stays active; a
    ///     launch home that this launch did NOT activate keeps its stored
    ///     `enabled` flag exactly as the operator left it — starting a
    ///     process from inside a switched-off home does not silently
    ///     re-include it, the registry simply reports it switched off
    ///     (R4 second case);
    ///   * an empty registry (or a registry without any active mark) adopts
    ///     the launch home as active — activation implies inclusion, so the
    ///     adopted record is enabled in the same flip.
    ///
    /// Existing records are never lost by a launch (REQ-HOME-02 AC2).
    pub fn bootstrap_launch(&mut self, launch: &Path, env_set: bool) -> ApiResult<PathBuf> {
        let key = norm(launch);
        // The label may read the launch store; probe it OUTSIDE the lock
        // (read-only opens never serialize against the registry file).
        let probe = label_probe(launch);
        let active = self.transact(|entries| {
            let registry_had_entries = !entries.is_empty();
            let mut changed = false;
            match entries.iter_mut().find(|e| e.path == key) {
                None => {
                    entries.push(new_entry(&key, &probe, Utc::now()));
                    changed = true;
                }
                // R3 quality return: a label that was persisted while the
                // store could not be read is re-derived on re-registration
                // instead of pinning the transient failure forever.
                Some(entry) if entry.project_source == ProjectSource::Unreadable => {
                    changed = apply_label(entry, &probe);
                }
                Some(_) => {}
            }
            // Who is active (the stored fact — never read-time env
            // re-derivation beyond the ratified OQ-HOME-7 rule itself).
            // The adoption rule below is documented verbatim in
            // docs/homes-registry.md, section «Реестр без активной метки»:
            // the launch home is activated when it came from an explicitly
            // set DEVAI_HOME, when the registry had no records at all, or
            // when no OTHER record carries the active mark; otherwise the
            // stored activity is left untouched.
            let saved_active = entries.iter().any(|e| e.active && e.path != key);
            if env_set || !registry_had_entries || !saved_active {
                changed = mark_active(entries, &key) || changed;
            }
            let active = entries
                .iter()
                .find(|e| e.active)
                .map(|e| e.path.clone())
                .unwrap_or_else(|| key.clone());
            Ok((changed, PathBuf::from(active)))
        })?;
        Ok(active)
    }

    /// `home.add`: register an existing devai home. Validation is the
    /// ratified REQ-HOME-03 AC3 pair — a missing directory and a directory
    /// without a store refuse with distinct typed errors, and the UI
    /// survives either (both live in `resolve_registrable_home`). Re-adding
    /// a known path re-enables its record and keeps the original
    /// `added_at` (recent-homes reuse), refreshing the label verdict.
    /// Adding never copies or migrates the managed home's content
    /// (REQ-HOME-08).
    ///
    /// Three units, ≤40 lines each (R1 quality return, cl/short-units):
    /// resolve the path → probe the label → upsert the entry.
    pub fn add(&mut self, path: &Path) -> ApiResult<HomeRecord> {
        let key = resolve_registrable_home(path)?;
        let probe = label_probe(path);
        let idx = self.upsert(&key, &probe)?;
        Ok(record_of(&self.entries[idx]))
    }

    /// Register the entry or refresh a known one: re-enable it and store
    /// the freshly probed label (a store that answers now replaces a label
    /// derived while it was unreadable — R3). Returns the record index.
    fn upsert(&mut self, key: &str, probe: &LabelProbe) -> ApiResult<usize> {
        let now = Utc::now();
        self.transact(|entries| match entries.iter().position(|e| e.path == key) {
            Some(i) => {
                let mut changed = !entries[i].enabled;
                entries[i].enabled = true;
                changed = apply_label(&mut entries[i], probe) || changed;
                if !entries.iter().any(|e| e.active) {
                    // No active home (all switched off, OQ-HOME-8): adding
                    // or re-adding a home is the operator's explicit way
                    // back — the record they named becomes active.
                    entries[i].active = true;
                    changed = true;
                }
                Ok((changed, i))
            }
            None => {
                entries.push(new_entry(key, probe, now));
                let i = entries.len() - 1;
                if !entries.iter().any(|e| e.active) {
                    entries[i].active = true;
                }
                Ok((true, i))
            }
        })
    }

    /// `home.set_enabled`: include/exclude a home from observation.
    /// Ratified OQ-HOME-3: removal from the registry IS disabling — the
    /// record survives with its attributes, physical deletion is out of
    /// this iteration.
    ///
    /// Ratified OQ-HOME-8: switching the ACTIVE home off is a normal
    /// operator action. Its outcome is defined, not refused: the next
    /// ENABLED record in registry order becomes active (wrapping to the
    /// first enabled one when the tail holds none); when no enabled record
    /// remains the registry keeps working with NO active home — no home is
    /// chosen silently for the operator. The records come back exactly as
    /// committed, so the UI renders the outcome it just caused.
    pub fn set_enabled(&mut self, path: &str, enabled: bool) -> ApiResult<Vec<HomeRecord>> {
        let target = Path::new(path).to_path_buf();
        self.transact(|entries| {
            let idx = entries
                .iter()
                .position(|e| same_path(&e.path, &target))
                .ok_or_else(|| store_not_found("home", path))?;
            let mut changed = entries[idx].enabled != enabled;
            entries[idx].enabled = enabled;
            if !enabled && entries[idx].active {
                // The stored active marker leaves with the enabled flag;
                // the ratified successor rule takes over (OQ-HOME-8).
                entries[idx].active = false;
                changed = true;
                if let Some(next) = next_enabled_after(entries, idx) {
                    entries[next].active = true;
                }
            } else if enabled && !entries.iter().any(|e| e.active) {
                // The operator's explicit enable is the way out of the
                // "no active home" state: the enabled home they named
                // becomes active. Still no silent pick — this record is
                // the one they acted on.
                entries[idx].active = true;
                changed = true;
            }
            Ok((changed, ()))
        })?;
        Ok(self.records()?)
    }

    /// Validate an activation target without writing anything: the record
    /// must exist, be enabled and be present on disk. The API transaction
    /// (`Api::activate_home`) reopens the target as an OBSERVATION
    /// (`Api::open_observed` — no migration, REQ-HOME-09) and only then
    /// commits with `set_active` — so a failed reopen never leaves the
    /// registry pointing at a home the session does not hold
    /// (REQ-HOME-06 AC1).
    pub fn activation_target(&self, path: &str) -> ApiResult<PathBuf> {
        let idx = self
            .find(path)
            .ok_or_else(|| store_not_found("home", path))?;
        let entry = &self.entries[idx];
        if !entry.enabled {
            return Err(ApiError::refused(
                codes::HOME_DISABLED,
                format!(
                    "{} is registered but switched off — enable it first",
                    entry.path
                ),
            ));
        }
        let root = PathBuf::from(&entry.path);
        if !root.is_dir() {
            return Err(ApiError::refused(
                codes::HOME_UNAVAILABLE,
                format!("home directory {} is not on disk", entry.path),
            ));
        }
        if !root.join(DB_FILE_NAME).exists() {
            return Err(ApiError::refused(
                codes::HOME_UNAVAILABLE,
                format!("home {} lost its {DB_FILE_NAME}", entry.path),
            ));
        }
        Ok(root)
    }

    /// Commit the stored active marker (exactly one active, the registry
    /// file of the control home is the only write).
    pub fn set_active(&mut self, path: &Path) -> ApiResult<()> {
        let key = norm(path);
        self.transact(|entries| {
            let idx = entries
                .iter()
                .position(|e| e.path == key)
                .ok_or_else(|| store_not_found("home", &key))?;
            if !entries[idx].enabled {
                return Err(ApiError::refused(
                    codes::HOME_DISABLED,
                    format!("{key} is switched off — enable it before activating"),
                ));
            }
            Ok((mark_active(entries, &key), ()))
        })
    }

    /// The path stored for an entry after a (re)open — the canonical key.
    pub fn active_entry(&self) -> Option<&HomeEntry> {
        self.entries.iter().find(|e| e.active)
    }

    /// `home.list`: every stored record with the live attributes. An
    /// absent home renders with path + cause and survives for the return
    /// (REQ-HOME-07: no re-add action needed; the probe sees it again).
    /// The stored document is re-read first: enumeration always reports
    /// the committed truth, including registrations a second session
    /// made in the meantime (live-read doctrine, ratified OQ-HOME-4).
    pub fn records(&mut self) -> ApiResult<Vec<HomeRecord>> {
        self.sync()?;
        let entries: Vec<&HomeEntry> = self.entries.iter().collect();
        Ok(par_map(
            &entries,
            record_of,
            // A panic in one observation is localized as a read error for
            // that one home, never a crash of the list (rl/crash-free).
            panic_record,
        ))
    }

    /// `home.projects_all`: the merged projects plane (REQ-HOME-04/05).
    /// Enabled homes only; each block carries its attribution and its own
    /// localized error; `only` narrows the view to one home on request.
    /// The registry document is re-read first, so the enabled set is the
    /// committed truth, not this session's startup snapshot.
    pub fn projects_all(&mut self, only: Option<&str>) -> ApiResult<Vec<HomeRecordsView<Project>>> {
        self.view_all(only, |s| s.list_projects())
    }

    /// `home.tasks_all`: the merged tasks plane, same shape and discipline
    /// — same builder, only the read closure differs.
    pub fn tasks_all(&mut self, only: Option<&str>) -> ApiResult<Vec<HomeRecordsView<Task>>> {
        self.view_all(only, |s| s.list_tasks())
    }

    /// The merged plane over the enabled homes: per-home attribution,
    /// live resolution, localized errors and panic isolation are identical
    /// for every record kind, so both `*_all` operations share one builder.
    fn view_all<T: Send>(
        &mut self,
        only: Option<&str>,
        read: impl Fn(&Store) -> Result<Vec<T>, devai_store::StoreError> + Sync,
    ) -> ApiResult<Vec<HomeRecordsView<T>>> {
        self.sync()?;
        let now = Utc::now();
        let entries = self.scoped(only)?;
        Ok(par_map(
            &entries,
            |e| {
                let HomeObservation { probe, records } = observe(Path::new(&e.path), &read);
                HomeRecordsView {
                    snapshot_at: now,
                    home_path: e.path.clone(),
                    home_project: e.project.clone(),
                    is_active: e.active,
                    available: probe.available,
                    version_note: probe.version_note,
                    error: probe.read_error,
                    records,
                }
            },
            |e| panic_block(e, now),
        ))
    }

    /// Enabled entries (in registry order), optionally narrowed to one
    /// home by path. The filter is an AND with inclusion: naming a
    /// registered-but-disabled home yields nothing, and since that is a
    /// refusal (never an empty "as if observed" view) it surfaces typed as
    /// `not_found` for an enabled home. An active home is always enabled
    /// (`mark_active` implies inclusion), so the writable home is always
    /// inside the merged view.
    fn scoped(&self, only: Option<&str>) -> ApiResult<Vec<&HomeEntry>> {
        match only {
            None => Ok(self.entries.iter().filter(|e| e.enabled).collect()),
            Some(path) => {
                let key = norm(Path::new(path));
                let hits: Vec<&HomeEntry> = self
                    .entries
                    .iter()
                    .filter(|e| e.path == key && e.enabled)
                    .collect();
                if hits.is_empty() {
                    return Err(store_not_found("enabled home", &key));
                }
                Ok(hits)
            }
        }
    }
}

/// Typed not-found for registry-plane lookups (the store error keeps its
/// `not_found` machine code through `ApiError::code`).
fn store_not_found(what: &str, given: &str) -> ApiError {
    ApiError::Store(devai_store::StoreError::NotFound(what.into(), given.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_note_names_both_ranks_and_the_guarantee() {
        assert_eq!(version_note(devai_store::SCHEMA_VERSION), None);
        let behind = version_note(devai_store::SCHEMA_VERSION - 1).unwrap();
        assert!(behind.contains("behind") && behind.contains("never migrates"));
        let ahead = version_note(devai_store::SCHEMA_VERSION + 1).unwrap();
        assert!(ahead.contains("ahead of") && ahead.contains("never migrates"));
    }

    #[test]
    fn registry_document_roundtrips_the_pinned_shape() {
        // homes.json is platform data: the field names are its contract.
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join(HOMES_FILE);
        let doc = HomesFile {
            version: HOMES_FORMAT_VERSION,
            homes: vec![HomeEntry {
                path: "/x/.devai".into(),
                project: "x".into(),
                project_source: ProjectSource::Store,
                project_error: None,
                added_at: Utc::now(),
                enabled: true,
                active: true,
            }],
        };
        fs::write(&file, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
        let homes = Homes::load(dir.path()).unwrap();
        assert_eq!(homes.entries().len(), 1);
        assert!(homes.entries()[0].active);
    }

    /// R1 quality return (regression of the first run): on a machine that
    /// never ran the platform the control home does not exist yet. That is
    /// the valid empty state, so the first registration must CREATE the
    /// control home (and its lock anchor) instead of dying on ENOENT — this
    /// is the exact hole the previous version had: the lock was opened
    /// before any `create_dir_all`.
    #[test]
    fn first_registration_creates_a_missing_control_home() {
        let dir = tempfile::TempDir::new().unwrap();
        // Neither the control home nor the launch home exists on disk.
        let control = dir.path().join("fresh/.devai");
        let launch = dir.path().join("project/.devai");
        assert!(!control.exists(), "precondition: no control home");

        let mut homes = Homes::load(&control).unwrap();
        assert!(homes.entries().is_empty(), "empty registry is valid");

        let active = homes
            .bootstrap_launch(&launch, true)
            .expect("the first launch registers the launch home, it is no startup death");

        assert_eq!(
            active, launch,
            "the registry names the launch home (no store is created by registration alone)"
        );
        let file = control.join(HOMES_FILE);
        assert!(file.is_file(), "the registry document was created");
        assert!(
            control.join(HOMES_LOCK_FILE).is_file(),
            "lock anchor exists"
        );
        let reread = Homes::load(&control).unwrap();
        assert_eq!(reread.entries().len(), 1);
        assert!(
            reread.entries()[0].active && reread.entries()[0].enabled,
            "the launch record is the active and included one"
        );
    }

    #[test]
    fn corrupt_registry_fails_loud_not_quiet() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join(HOMES_FILE), "{ nope").unwrap();
        match Homes::load(dir.path()) {
            Err(e) => assert_eq!(e.code().0, codes::HOMES_REGISTRY_CORRUPT),
            Ok(_) => panic!("a corrupt registry must fail loud, never be quietly discarded"),
        }
    }

    /// R4 (rl/edge-cases), first case: an explicit `$DEVAI_HOME` naming a
    /// home the operator had switched off is the ratified env priority
    /// (OQ-HOME-7) — the record becomes active AND enabled. The registry
    /// must never hold `active && !enabled`: that row would be active in
    /// fact but absent from every merged view.
    #[test]
    fn explicit_env_reactivates_and_includes_a_disabled_home() {
        let dir = tempfile::TempDir::new().unwrap();
        let control = dir.path().join("control");
        let home = dir.path().join("project/.devai");
        let other = dir.path().join("other/.devai");
        for p in [&control, &home, &other] {
            fs::create_dir_all(p).unwrap();
        }
        let mut homes = Homes::load(&control).unwrap();
        homes.bootstrap_launch(&other, true).unwrap();
        homes.bootstrap_launch(&home, false).unwrap();
        let key = norm(&home);
        homes.set_enabled(&key, false).unwrap();
        assert!(
            !homes
                .entries()
                .iter()
                .find(|e| e.path == key)
                .unwrap()
                .enabled,
            "precondition: the operator switched the home off"
        );

        let active = homes.bootstrap_launch(&home, true).unwrap();

        assert_eq!(
            fs::canonicalize(&active).unwrap(),
            fs::canonicalize(&home).unwrap(),
            "explicit $DEVAI_HOME wins the active mark (OQ-HOME-7)"
        );
        let rec = homes.entries().iter().find(|e| e.path == key).unwrap();
        assert!(rec.active, "the env-named home became the active record");
        assert!(
            rec.enabled,
            "an explicit env choice also includes the home — no active && !enabled row"
        );
        assert!(
            !homes
                .entries()
                .iter()
                .find(|e| e.path == norm(&other))
                .unwrap()
                .active,
            "exactly one active"
        );
    }

    /// R4 (rl/edge-cases), second case: WITHOUT the env override, launching
    /// a process from inside a switched-off home does not silently
    /// re-include or activate it — the stored flags win and the registry
    /// reports the home as switched off.
    #[test]
    fn launch_without_env_leaves_a_disabled_home_switched_off() {
        let dir = tempfile::TempDir::new().unwrap();
        let control = dir.path().join("control");
        let home = dir.path().join("project/.devai");
        let other = dir.path().join("other/.devai");
        for p in [&control, &home, &other] {
            fs::create_dir_all(p).unwrap();
        }
        let mut homes = Homes::load(&control).unwrap();
        homes.bootstrap_launch(&other, true).unwrap();
        homes.bootstrap_launch(&home, false).unwrap();
        let key = norm(&home);
        homes.set_enabled(&key, false).unwrap();

        let active = homes.bootstrap_launch(&home, false).unwrap();

        assert_eq!(
            fs::canonicalize(&active).unwrap(),
            fs::canonicalize(&other).unwrap(),
            "the saved active record stays active without an env override"
        );
        let rec = homes.entries().iter().find(|e| e.path == key).unwrap();
        assert!(
            !rec.enabled && !rec.active,
            "a switched-off launch home keeps its stored flags: {rec:?}"
        );
    }

    /// The registry invariant behind R4: every activation path implies
    /// inclusion, so no consumer can ever read an active-but-switched-off
    /// record.
    #[test]
    fn activation_always_includes_the_home() {
        let mut entries = vec![HomeEntry {
            path: "/x/.devai".into(),
            project: "x".into(),
            project_source: ProjectSource::Store,
            project_error: None,
            added_at: Utc::now(),
            enabled: false,
            active: false,
        }];
        assert!(mark_active(&mut entries, "/x/.devai"));
        assert!(entries[0].active && entries[0].enabled);
    }

    /// R3 quality return (rl/error-handling): "the store answered with no
    /// projects" is a path-shape label, NOT a failure; "the store could not
    /// be read" is `Unreadable` with the cause carried beside the label.
    /// The old `label_for` collapsed both into one silent string.
    #[test]
    fn label_verdict_separates_an_empty_store_from_an_unreadable_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let empty = dir.path().join("empty-project/.devai");
        Store::open(&empty).unwrap();
        let probe = label_probe(&empty);
        assert_eq!(
            probe.source,
            ProjectSource::Path,
            "a readable store without projects yields the deterministic path label"
        );
        assert!(probe.error.is_none(), "no failure to report: {probe:?}");
        assert_eq!(probe.label, "empty-project");

        let broken = dir.path().join("broken-project/.devai");
        fs::create_dir_all(&broken).unwrap();
        let probe = label_probe(&broken);
        assert_eq!(
            probe.source,
            ProjectSource::Unreadable,
            "a directory without a store is a read failure, not an empty home"
        );
        assert!(
            probe.error.as_deref().unwrap_or("").contains("devai.db"),
            "the cause names the failing read: {probe:?}"
        );
        assert_eq!(probe.label, "broken-project", "the fallback path label");
    }

    /// R3 quality return: a label persisted while the store was unreadable
    /// is re-derived on re-registration instead of pinning the transient
    /// failure forever (bootstrap_launch used to skip the derivation for a
    /// known path).
    #[test]
    fn an_unreadable_label_is_marked_and_re_derived_on_relaunch() {
        let dir = tempfile::TempDir::new().unwrap();
        let control = dir.path().join("control");
        let home = dir.path().join("project/.devai");
        fs::create_dir_all(&home).unwrap();

        let mut homes = Homes::load(&control).unwrap();
        homes.bootstrap_launch(&home, true).unwrap();
        let first = homes.entries()[0].clone();
        assert_eq!(
            first.project_source,
            ProjectSource::Unreadable,
            "the label is marked as derived-with-error, not silently \"project\""
        );
        assert!(first.project_error.is_some(), "the cause survives in data");

        // The disk comes back (the operator ran `devai init`): the same
        // launch re-derives the stored verdict.
        Store::open(&home).unwrap();
        homes.bootstrap_launch(&home, true).unwrap();
        let second = &homes.entries()[0];
        assert_ne!(
            second.project_source,
            ProjectSource::Unreadable,
            "a transient failure is not pinned: {second:?}"
        );
        assert!(second.project_error.is_none());
        assert_eq!(
            second.added_at, first.added_at,
            "re-registration keeps the original added_at"
        );
    }

    /// R4 quality return (ts/coverage-static): the panic fallback of both
    /// observation planes is a named constructor, so the localized branch is
    /// executed by a test instead of living untested inside a closure.
    #[test]
    fn panic_fallback_states_unavailability_without_inventing_rows() {
        let entry = HomeEntry {
            path: "/gone/.devai".into(),
            project: "gone".into(),
            project_source: ProjectSource::Path,
            project_error: None,
            added_at: Utc::now(),
            enabled: true,
            active: false,
        };

        let rec = panic_record(&entry);
        assert!(!rec.available && rec.schema_version.is_none());
        assert_eq!(rec.path, entry.path, "the path is kept");
        assert_eq!(
            rec.read_error.as_ref().map(|i| i.code.as_str()),
            Some(codes::HOME_UNAVAILABLE)
        );
        assert_eq!(rec.project, "gone", "the stored label is passed through");

        let block: HomeRecordsView<Project> = panic_block(&entry, Utc::now());
        assert!(!block.available && block.records.is_empty());
        assert_eq!(
            block.error.as_ref().map(|i| i.code.as_str()),
            Some(codes::HOME_UNAVAILABLE),
            "the block carries the same typed cause"
        );
    }
}
