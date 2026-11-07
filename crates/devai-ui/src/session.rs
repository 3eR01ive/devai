//! Session bootstrap for devai-ui (REQ-UI-01, extended by the homes
//! registry cycle REQ-HOME-02/06/07).
//!
//! Opening a home is NOT a registry operation — it is the bootstrap entry
//! `Api::open` (RATIFIED OQ-10 — cited by symbol in
//! `crates/devai-api/src/lib.rs`, by symbol and not by line number so the
//! citation cannot rot). The UI
//! never touches the store, never calls init/seed: it opens the chosen home
//! through the API and holds the handle inside itself.
//!
//! Resolution chains are the operator-ratified GUI answers:
//!   * home  (OQ-UI-1 → OQ-HOME-7): the launch home is the env chain —
//!           $DEVAI_HOME, else default ~/.devai. It is auto-registered in
//!           the homes registry; who ends up ACTIVE is the ratified rule:
//!           an explicitly set $DEVAI_HOME is authoritative, without env
//!           the stored active record of the registry wins.
//!   * control home (OQ-HOME-1): the registry of homes lives in the
//!           DEFAULT home (~/.devai) — independent of $DEVAI_HOME; this
//!           is the operator-ratified control plane. Project data stays
//!           in the project homes (the dogma is untouched).
//!   * actor (OQ-UI-8):  the CLI chain — $DEVAI_USER, else the user's env
//!                       variable ($USER), else the default "ui". This
//!                       iteration the UI writes no domain data, so the
//!                       actor marks the session and registry management
//!                       only.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use devai_api::{Api, ApiError, ApiResult, DB_FILE_NAME, HOME_DIR_NAME, Homes, LaunchResolution};
use serde::Serialize;

/// A resolved launch home for the GUI window. REQ-HOME-02 keeps the CHAIN
/// unchanged — `$DEVAI_HOME` (non-blank), else `~/.devai` — and the one-home
/// run stays the degenerate case of the registry; the default literal is now
/// the shared `HOME_DIR_NAME` instead of an inline `".devai"`, same path.
/// `lookup` is injectable so the chain is testable without mutating the
/// process env.
pub fn resolve_home(lookup: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(v) = lookup("DEVAI_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    // Default: ~/.devai (same fallback shape as the CLI's clap default).
    dirs::home_dir()
        .map(|h| h.join(HOME_DIR_NAME))
        .unwrap_or_else(|| PathBuf::from(HOME_DIR_NAME))
}

/// The control home of the homes registry (ratified OQ-HOME-1): the
/// DEFAULT home, NOT the env chain — `~/.devai` holds the platform's
/// registry data while every project's domain data stays in its own home.
/// The lookup IS used for the user's home directory (`$HOME`), so the
/// resolution is a real seam: tests inject a fake home, production reads
/// the process environment, and `$DEVAI_HOME` never moves this path.
pub fn resolve_control_home(lookup: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    lookup("HOME")
        .filter(|v| !v.trim().is_empty())
        .map(|h| PathBuf::from(h).join(HOME_DIR_NAME))
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(HOME_DIR_NAME))
                .unwrap_or_else(|| PathBuf::from(HOME_DIR_NAME))
        })
}

/// Actor identity for `Api::open` (OQ-UI-8 chain: DEVAI_USER > USER > "ui").
pub fn resolve_actor(lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let non_empty = |k: &str| {
        lookup(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    non_empty("DEVAI_USER")
        .or_else(|| non_empty("USER"))
        .unwrap_or_else(|| "ui".to_string())
}

/// The open session: the ACTIVE home's API handle, the loaded homes
/// registry, and the facts the status bar renders (REQ-UI-01 AC3: the home
/// path must be visible; REQ-HOME-01: which home is active is stored data).
pub struct Session {
    pub api: Api,
    pub homes: Homes,
    pub info: SessionInfo,
}

/// Why a home could not be opened at the bootstrap boundary — typed, in the
/// SAME machine vocabulary the domain uses (R5 quality return): `code` is a
/// stable token a consumer branches on (`home_unavailable` — path or store
/// gone; `home_no_store` — path present, no initialised store;
/// `home_schema_version` — the stored rank refused this build), never a
/// category re-parsed from prose. `message` keeps the operator-facing text.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct HomeFailure {
    pub code: &'static str,
    pub home: String,
    pub message: String,
}

impl HomeFailure {
    fn new(code: &'static str, home: &Path, message: impl Into<String>) -> HomeFailure {
        HomeFailure {
            code,
            home: home.display().to_string(),
            message: message.into(),
        }
    }

    /// The bootstrap boundary's translation of an API refusal: the API's
    /// own machine code decides the KIND, the text stays the human half.
    fn from_open(home: &Path, error: &ApiError) -> HomeFailure {
        HomeFailure::new(
            home_failure_code(error.code().0),
            home,
            format!("cannot open home {}: {error}", home.display()),
        )
    }
}

/// API error code → home failure token. The mapping itself lives in
/// `devai_api::codes::home_failure_code` — ONE classifier shared with the
/// registry probe (R2 quality return, rel/error-context: two producers of
/// the same vocabulary must not drift). The rationale is documented there:
/// `not_found` → `home_no_store`, `schema_version` → `home_schema_version`,
/// `io`/`corrupt` → `home_unavailable`, and an unknown future code fails
/// closed to the widest honest token. The API message still carries the
/// text and the home stays visible with its path (REQ-HOME-07).
fn home_failure_code(api_code: &str) -> &'static str {
    devai_api::codes::home_failure_code(api_code)
}

/// Typed refusal at the SESSION BOOTSTRAP boundary (R3 quality return): the
/// four boundaries (`load_registry`, `register_launch`,
/// `open_active_or_control`, `open_resolved`) used to flatten every failure
/// into prose, so a consumer could not branch on the kind — the path and the
/// cause were only readable by parsing the sentence. This is the bootstrap
/// sibling of `HomeFailure`: a stable machine `code` (spelled in
/// `devai_api::codes`), the home the refusal is about, and the operator
/// text. `main` prints by code; a future consumer branches on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapError {
    pub code: &'static str,
    pub home: String,
    pub message: String,
}

impl BootstrapError {
    fn new(code: &'static str, home: &Path, message: impl Into<String>) -> BootstrapError {
        BootstrapError {
            code,
            home: home.display().to_string(),
            message: message.into(),
        }
    }

    /// A home that refused observation, carried over from the domain's own
    /// typed refusal (the control-home case has nowhere to degrade to).
    fn from_home_failure(failure: HomeFailure) -> BootstrapError {
        BootstrapError {
            code: failure.code,
            home: failure.home,
            message: failure.message,
        }
    }
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}: {}", self.code, self.home, self.message)
    }
}

impl std::error::Error for BootstrapError {}

/// Registry-plane translation of an API refusal (R3): a parse/format
/// refusal keeps its own token, every other failure of the registry
/// document (permissions, a vanished control directory, a failed rename) is
/// `homes_registry_unavailable` — the document was never trusted because the
/// act of reading/writing it failed. Both are registry tokens; a caller
/// tells "repair the file" from "the control home is not usable".
fn registry_failure(control: &Path, error: &ApiError) -> BootstrapError {
    let code = match error.code().0 {
        devai_api::codes::HOMES_REGISTRY_CORRUPT => devai_api::codes::HOMES_REGISTRY_CORRUPT,
        _ => devai_api::codes::HOMES_REGISTRY_UNAVAILABLE,
    };
    BootstrapError::new(
        code,
        control,
        format!(
            "cannot use the homes registry at {}: {error}",
            control.display()
        ),
    )
}

#[derive(Serialize, Clone, Debug)]
pub struct SessionInfo {
    pub home: String,
    pub actor: String,
    pub opened_at: DateTime<Utc>,
    /// Where the homes registry lives (OQ-HOME-1: the control home).
    pub control_home: String,
    /// Honest degraded-open fact (REQ-HOME-07 AC1): when the stored
    /// active home could not be reopened at startup, the session observes
    /// through the control home handle and names the failed home + typed
    /// cause here. The stored active record is NOT reassigned by the
    /// fallback — the home returns with one click when its path returns.
    pub degraded_from: Option<HomeFailure>,
}

/// Fields of the `session_info` wire shape (R5 quality return): the title
/// bar facts are a contract like every domain record — described once,
/// pinned in `tests/fixtures/ipc_shapes.json` and mirrored by
/// `frontend/src/types.ts`. `session_info` and `homes_activate` both return
/// this object, so the shape cannot drift between the two commands.
#[allow(dead_code)] // consumed by the IPC parity test (commands.rs)
pub const SESSION_INFO_SCHEMA: &[(&str, &str)] = &[
    ("home", "string"),
    ("actor", "string"),
    ("opened_at", "string"),
    ("control_home", "string"),
    ("degraded_from", "HomeFailure | null"),
];

/// Symlink-free equality key for a path the process was handed. A launch
/// path may not exist yet, so the raw path is the honest fallback.
fn canonical_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The startup registry read (REQ-HOME-01 / OQ-HOME-1): the control home's
/// document, its absence being the valid pre-registration state. A failure
/// is typed (R3): `homes_registry_corrupt` / `homes_registry_unavailable`
/// plus the control-home path, never a sentence alone.
fn load_registry(control: &Path) -> Result<Homes, BootstrapError> {
    Homes::load(control).map_err(|e| registry_failure(control, &e))
}

/// REQ-HOME-02: the launch home is auto-registered; the ratified active
/// resolution (OQ-HOME-7) lives in the registry itself. A refusal to write
/// the registry is a typed bootstrap error about the registry path, not
/// about the launch home (which the operator did not fail to provide).
/// The resolution also says whether THIS launch created the record — the
/// only case where a missing store may be created (R4 quality return).
fn register_launch(
    homes: &mut Homes,
    launch: &Path,
    env_set: bool,
) -> Result<LaunchResolution, BootstrapError> {
    homes.bootstrap_launch(launch, env_set).map_err(|e| {
        let mut failure = registry_failure(homes.control_home(), &e);
        failure.message = format!(
            "cannot register the launch home {}: {}",
            launch.display(),
            failure.message
        );
        failure
    })
}

/// The observation open of ONE home (REQ-HOME-09): a store already on disk
/// is opened read-only — no migration ladder, no `user_version` bump, no
/// directory seeding. The migrating bootstrap `Api::open` is kept for the
/// single case of a home that carries no store yet and must be CREATED once
/// (a first launch, REQ-HOME-02). The typed refusal is returned as-is, so
/// the caller keeps its machine code (R5).
fn open_home(home: &Path, actor: &str) -> ApiResult<Api> {
    if home.join(DB_FILE_NAME).exists() {
        Api::open_observed(home, actor)
    } else {
        Api::open(home, actor)
    }
}

/// The startup verdict on the stored ACTIVE home's disk state (R3: the
/// state decision is its own unit, apart from registry resolution and from
/// opening).
enum ActiveDiskState {
    /// The home may be opened: its store is on disk, OR this launch CREATED
    /// its registry record and named it through `$DEVAI_HOME` (REQ-HOME-02
    /// first launch), OR it IS the control home (created on first use).
    Openable,
    /// A registered home whose store is gone. NOT a first launch: opening
    /// it would resurrect an empty home and hide the real one behind a lie
    /// (REQ-HOME-07) — the session degrades and the record stays.
    Gone(HomeFailure),
}

/// `registered_here` is the R4 distinction: only a record THIS launch
/// created may be materialised as a fresh home — a home the registry already
/// knew and that vanished from disk is `not on disk`, never a new empty home
/// (REQ-HOME-02 AC1 / REQ-HOME-07).
fn active_disk_state(
    launch: &Path,
    active: &Path,
    control: &Path,
    env_set: bool,
    registered_here: bool,
) -> ActiveDiskState {
    let is = |a: &Path, b: &Path| canonical_path(a) == canonical_path(b);
    if active.join(DB_FILE_NAME).exists() || is(active, control) {
        return ActiveDiskState::Openable;
    }
    if registered_here && env_set && is(launch, active) && !active.exists() {
        return ActiveDiskState::Openable;
    }
    ActiveDiskState::Gone(gone_failure(active))
}

/// A registered home whose store is gone (REQ-HOME-07): the typed reason the
/// session degrades with. A directory that exists but carries no store is a
/// different case from a path that vanished — creating a fresh empty store
/// over either would hide the real data behind a clean session, so nothing is
/// written and the registry record stays.
///
/// ONE source for this verdict (R3 quality return): the session bootstrap and
/// the UI's post-registry rebind both call it, so the tokens cannot drift.
pub(crate) fn gone_failure(active: &Path) -> HomeFailure {
    let (code, state) = if active.is_dir() {
        (
            devai_api::codes::HOME_NO_STORE,
            format!("exists but has no {DB_FILE_NAME}"),
        )
    } else {
        (
            devai_api::codes::HOME_UNAVAILABLE,
            "is not on disk".to_string(),
        )
    };
    HomeFailure::new(
        code,
        active,
        format!(
            "home {} {state} — no empty store is created over it; the registry record stays, initialise the store (`devai init`) or activate another home",
            active.display()
        ),
    )
}

/// The degradation fact of a registry with NO active record (every home is
/// switched off, ratified OQ-HOME-8): a defined state, not an error, and it
/// names the control home the session falls back to. ONE source for the
/// startup bootstrap and the post-registry rebind (`commands.rs`).
pub(crate) fn no_active_failure(control: &Path) -> HomeFailure {
    HomeFailure::new(
        devai_api::codes::HOME_NONE_ACTIVE,
        control,
        "no active home — enable or add a home; the registry keeps every record",
    )
}

/// Open the active home for observation, or degrade to the control home and
/// say honestly why (REQ-HOME-07). Only a refusal of the CONTROL home itself
/// has nowhere left to degrade to, so that one is the startup error.
///
/// `resolution.active == None` is the ratified "no active home" state
/// (OQ-HOME-8): every record is switched off, so the session observes
/// through the control home and reports `home_none_active` — no home is
/// invented and no record is re-enabled by starting the process (R1).
fn open_active_or_control(
    homes: &Homes,
    launch: &Path,
    resolution: &LaunchResolution,
    actor: &str,
    env_set: bool,
) -> Result<(Api, Option<HomeFailure>), BootstrapError> {
    let Some(active) = resolution.active.as_deref() else {
        let failure = no_active_failure(homes.control_home());
        let fallback = degrade_to_control(homes.control_home(), &failure, actor)?;
        return Ok((fallback, Some(failure)));
    };
    let state = active_disk_state(
        launch,
        active,
        homes.control_home(),
        env_set,
        resolution.registered_here,
    );
    let failure = match state {
        ActiveDiskState::Openable => match open_home(active, actor) {
            Ok(api) => return Ok((api, None)),
            Err(error) => {
                let failure = HomeFailure::from_open(active, &error);
                if canonical_path(active) == canonical_path(homes.control_home()) {
                    // The one refusal with nowhere to degrade to: typed at
                    // the bootstrap boundary by its domain code (R3).
                    return Err(BootstrapError::from_home_failure(failure));
                }
                // The active home refused the bootstrap (e.g. its schema
                // rank is ahead of this binary, REQ-HOME-09): degrade to
                // the control plane, keep the stored record.
                failure
            }
        },
        ActiveDiskState::Gone(failure) => failure,
    };
    let fallback = degrade_to_control(homes.control_home(), &failure, actor)?;
    Ok((fallback, Some(failure)))
}

/// The last resort of a session whose active home cannot be observed: the
/// control home's handle. Only a refusal of the CONTROL home itself is a
/// startup error — it has nowhere left to degrade to — and it names both
/// causes.
fn degrade_to_control(
    control: &Path,
    active_failure: &HomeFailure,
    actor: &str,
) -> Result<Api, BootstrapError> {
    open_home(control, actor).map_err(|e| {
        let control_failure = HomeFailure::from_open(control, &e);
        BootstrapError::new(
            control_failure.code,
            control,
            format!(
                "{}; the active home could not be observed either: {}",
                control_failure.message, active_failure.message
            ),
        )
    })
}

impl Session {
    /// Opens the session with the process environment (the chains above):
    /// load the registry of the control home, auto-register the launch
    /// home (REQ-HOME-02), resolve the stored active home, and open it for
    /// OBSERVATION (`Api::open_observed`; the migrating bootstrap is kept
    /// for a home that does not exist yet). A broken ACTIVE home is no
    /// longer a startup death (REQ-HOME-07 changes exactly the old exit
    /// 2 path): the session falls back to the control home's handle for
    /// observation and reports the failed home + typed cause.
    pub fn open_env() -> Result<Session, BootstrapError> {
        let lookup = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let env_set = lookup("DEVAI_HOME").is_some();
        let launch = resolve_home(&lookup);
        let actor = resolve_actor(&lookup);
        let control = resolve_control_home(&lookup);
        Self::open_resolved(launch, actor, control, env_set)
    }

    /// The session bootstrap over already-resolved facts (the env chains of
    /// `open_env` live above; this is the seam the tests drive — no process
    /// env mutation anywhere). Every home this function opens is an
    /// OBSERVED home: an existing store is opened through
    /// `Api::open_observed` (read-only, no migration ladder, no
    /// `user_version` bump — REQ-HOME-09, R1 quality return), and the
    /// migrating bootstrap `Api::open` is used for exactly one case: a home
    /// that carries no store on disk yet and must be created once (the
    /// first launch of a home, REQ-HOME-02).
    pub(crate) fn open_resolved(
        launch: PathBuf,
        actor: String,
        control: PathBuf,
        env_set: bool,
    ) -> Result<Session, BootstrapError> {
        let mut homes = load_registry(&control)?;
        let resolution = register_launch(&mut homes, &launch, env_set)?;
        let (api, degraded_from) =
            open_active_or_control(&homes, &launch, &resolution, &actor, env_set)?;
        let info = SessionInfo {
            home: api.home().display().to_string(),
            actor: api.actor().to_string(),
            opened_at: Utc::now(),
            control_home: homes.control_home().display().to_string(),
            degraded_from,
        };
        Ok(Session { api, homes, info })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| m.get(k).cloned()
    }

    #[test]
    fn home_prefers_devai_home_env() {
        // OQ-UI-1: $DEVAI_HOME wins when set.
        let h = resolve_home(&env(&[("DEVAI_HOME", "/srv/other.devai")]));
        assert_eq!(h, PathBuf::from("/srv/other.devai"));
    }

    #[test]
    fn home_blank_env_falls_back_to_default() {
        // A blank $DEVAI_HOME is not a chosen home — fall through.
        let h = resolve_home(&env(&[("DEVAI_HOME", "   ")]));
        let default = dirs::home_dir().unwrap().join(".devai");
        assert_eq!(h, default);
    }

    #[test]
    fn control_home_ignores_the_env_chain() {
        // OQ-HOME-1: the registry plane is the DEFAULT home regardless of
        // where the process was launched — even a set $DEVAI_HOME does not
        // move the control home. R5 quality return: the injected lookup is
        // the real input ($HOME), so this test proves the resolution, not
        // merely re-asserts `dirs::home_dir()`.
        let c = resolve_control_home(&env(&[
            ("HOME", "/srv/operator"),
            ("DEVAI_HOME", "/srv/other.devai"),
        ]));
        assert_eq!(c, PathBuf::from("/srv/operator/.devai"));
    }

    #[test]
    fn control_home_falls_back_to_dirs_when_home_is_not_injected() {
        let c = resolve_control_home(&env(&[]));
        assert_eq!(c, dirs::home_dir().unwrap().join(".devai"));
    }

    #[test]
    fn actor_chain_is_devai_user_then_user_then_ui() {
        // OQ-UI-8, ratified as the CLI chain + "ui" default.
        assert_eq!(
            resolve_actor(&env(&[("DEVAI_USER", "guerrero"), ("USER", "someone")])),
            "guerrero"
        );
        assert_eq!(resolve_actor(&env(&[("USER", "someone")])), "someone");
        assert_eq!(resolve_actor(&env(&[("DEVAI_USER", "")])), "ui");
        assert_eq!(resolve_actor(&env(&[])), "ui");
    }

    /// R1 quality return (same class as RA1, on the STARTUP path): restoring
    /// the stored active home is a read act — the session must open it as an
    /// OBSERVATION, never through the migrating bootstrap. A home of an
    /// older schema rank (v7; the fixture is the v8 shape minus the 008 rung,
    /// generated once with sqlite3 and inert since) must come out of startup
    /// with its rank untouched, marked "behind", and readable.
    /// Fixture of the startup-observation test: a control home, the v7
    /// lagging home with a stored active marker (a registry fact, not a
    /// domain write), and the session opened without `$DEVAI_HOME`.
    fn open_lagging_active_session(dir: &Path) -> (PathBuf, Session) {
        const FIXTURE: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/lagging_home_v7.db"
        );
        let control = dir.join("control");
        let lagging = dir.join("lagging");
        std::fs::create_dir_all(&control).unwrap();
        std::fs::create_dir_all(&lagging).unwrap();
        std::fs::copy(FIXTURE, lagging.join(DB_FILE_NAME)).unwrap();
        let mut homes = Homes::load(&control).unwrap();
        assert_eq!(
            homes.add(&lagging).unwrap().schema_version,
            Some(7),
            "fixture carries the older rank this build (v8) would migrate"
        );
        homes.set_active(&lagging).unwrap();
        // Launch home == stored active home; no $DEVAI_HOME, so the saved
        // record wins (OQ-HOME-7).
        let session = Session::open_resolved(
            lagging.clone(),
            "tester".to_string(),
            control.clone(),
            false,
        )
        .expect("an old-schema active home is observable, not a startup death");
        (lagging, session)
    }

    /// The rank is still 7: no rung ran, no `user_version` bump. The probe
    /// is the API's own live attribute, not a test-side sqlite read.
    fn assert_lagging_rank_untouched(session: &mut Session) {
        let rec = session.homes.records().unwrap().remove(0);
        assert_eq!(rec.schema_version, Some(7), "startup migrated the home");
        assert!(rec.active, "the stored active marker is unchanged");
        assert!(
            rec.version_note
                .as_deref()
                .is_some_and(|n| n.contains("behind")),
            "the home is marked behind by its version, not hidden: {:?}",
            rec.version_note
        );
    }

    /// R1/R2 quality return: the v7 tasks table has no `human_name` and the
    /// fixture carries real rows — the task plane reads them (no
    /// missing-column failure), both through the session and through the
    /// merged home view.
    fn assert_lagging_rows_are_read(session: &mut Session) {
        let projects = session.api.project_list().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "lagging");
        let tasks = session.api.task_list().unwrap();
        assert_eq!(tasks.len(), 2, "the lagging home's tasks are readable");
        assert!(tasks.iter().all(|t| t.human_name.is_none()));
        let mut view = session.homes.tasks_all(None).unwrap();
        let block = view
            .iter_mut()
            .find(|b| b.available)
            .expect("the enabled home answers");
        assert_eq!(block.records.len(), 2);
        assert!(block.error.is_none());
    }

    #[test]
    fn startup_with_a_stored_active_home_of_an_older_schema_never_migrates_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let (lagging, mut session) = open_lagging_active_session(dir.path());

        assert_eq!(
            std::fs::canonicalize(session.api.home()).unwrap(),
            std::fs::canonicalize(&lagging).unwrap(),
            "the session holds the stored active home itself"
        );
        assert!(
            session.info.degraded_from.is_none(),
            "observation is not a degradation"
        );
        assert_lagging_rank_untouched(&mut session);
        assert_lagging_rows_are_read(&mut session);
    }

    /// R4 quality return: creating a store is the FIRST registration's act.
    /// A home the registry already recorded, whose folder vanished, is
    /// `not on disk` even when `$DEVAI_HOME` names it — the bootstrap must
    /// not materialise a fresh empty home over the record (REQ-HOME-02 AC1
    /// vs REQ-HOME-07).
    #[test]
    fn a_recorded_home_that_vanished_is_not_recreated_by_an_env_launch() {
        let dir = tempfile::TempDir::new().unwrap();
        let control = dir.path().join("control");
        let other = dir.path().join("other/.devai");
        let vanished = dir.path().join("gone/.devai");
        std::fs::create_dir_all(&control).unwrap();

        let mut homes = Homes::load(&control).unwrap();
        homes.bootstrap_launch(&other, true).unwrap();
        std::fs::create_dir_all(&vanished).unwrap();
        homes.bootstrap_launch(&vanished, false).unwrap();
        std::fs::remove_dir_all(&vanished).unwrap();

        // $DEVAI_HOME names the vanished home: env wins the ACTIVE marker,
        // but the record predates this launch, so nothing may be created.
        let session = Session::open_resolved(
            vanished.clone(),
            "tester".to_string(),
            control.clone(),
            true,
        )
        .expect("a vanished recorded home degrades the session, it is no startup death");

        let failure = session
            .info
            .degraded_from
            .as_ref()
            .expect("the degrade is reported, not hidden");
        assert_eq!(failure.code, devai_api::codes::HOME_UNAVAILABLE);
        assert!(failure.home.ends_with("gone/.devai"));
        assert!(
            !vanished.exists(),
            "the bootstrap created a home over a vanished registry record"
        );
        assert_eq!(
            std::fs::canonicalize(session.api.home()).unwrap(),
            std::fs::canonicalize(&control).unwrap(),
            "observation falls back to the control home"
        );
    }

    /// R1 quality return (ratified OQ-HOME-8): starting the process while
    /// every home is switched off is NOT a switch-on. The launch home keeps
    /// `enabled:false`/`active:false`, the registry stays without an active
    /// record, and the session observes the control home under the honest
    /// `home_none_active` fact.
    #[test]
    fn startup_with_every_home_switched_off_does_not_switch_anything_on() {
        let dir = tempfile::TempDir::new().unwrap();
        let control = dir.path().join("control");
        let a = dir.path().join("a/.devai");
        let b = dir.path().join("b/.devai");
        std::fs::create_dir_all(&control).unwrap();
        let mut homes = Homes::load(&control).unwrap();
        homes.bootstrap_launch(&a, true).unwrap();
        homes.bootstrap_launch(&b, false).unwrap();
        let paths: Vec<String> = homes.entries().iter().map(|e| e.path.clone()).collect();
        for path in &paths {
            homes.set_enabled(path, false).unwrap();
        }
        drop(homes);

        // Launch from inside the switched-off home, WITHOUT $DEVAI_HOME.
        let session =
            Session::open_resolved(a.clone(), "tester".to_string(), control.clone(), false)
                .expect("the no-active state is not a startup death");

        let failure = session.info.degraded_from.as_ref().expect("reported");
        assert_eq!(failure.code, devai_api::codes::HOME_NONE_ACTIVE);
        assert_eq!(
            std::fs::canonicalize(session.api.home()).unwrap(),
            std::fs::canonicalize(&control).unwrap(),
            "the session observes the control home, no domain home is picked"
        );
        let reloaded = Homes::load(&control).unwrap();
        assert!(
            reloaded.entries().iter().all(|e| !e.enabled && !e.active),
            "the startup re-enabled or reactivated a record: {:?}",
            reloaded.entries()
        );
    }

    /// Fixture: a control home and a stored active home whose folder exists
    /// without a store, opened without `$DEVAI_HOME` (the saved record wins).
    fn open_with_storeless_active(dir: &Path) -> (PathBuf, PathBuf, Session) {
        let control = dir.join("control");
        let broken = dir.join("project/.devai");
        std::fs::create_dir_all(&control).unwrap();
        std::fs::create_dir_all(&broken).unwrap();
        // Registry fact only: the record of a home whose store never
        // existed (bootstrap_launch registers without needing one).
        let mut homes = Homes::load(&control).unwrap();
        homes.bootstrap_launch(&broken, true).unwrap();
        drop(homes);
        let session = Session::open_resolved(
            control.clone(),
            "tester".to_string(),
            control.clone(),
            false,
        )
        .expect("a broken active home degrades the session, it is no startup death");
        (control, broken, session)
    }

    /// R5 (rel/error-context): the bootstrap boundary must not flatten the
    /// domain's typed refusal into prose. A stored active home whose folder
    /// is still there but whose store is gone degrades with the
    /// `home_no_store` token, the home path and the operator text — the
    /// window renders the code, so the KIND has to survive the boundary.
    #[test]
    fn degraded_startup_keeps_the_typed_home_failure() {
        let dir = tempfile::TempDir::new().unwrap();
        let (control, broken, session) = open_with_storeless_active(dir.path());

        let failure = session
            .info
            .degraded_from
            .as_ref()
            .expect("the degraded open must be reported, not hidden");
        assert_eq!(failure.code, "home_no_store");
        assert_eq!(
            std::fs::canonicalize(&failure.home).unwrap(),
            std::fs::canonicalize(&broken).unwrap(),
            "the failure names the home it is about"
        );
        assert!(
            failure.message.contains("project/.devai"),
            "the operator text still points at the path: {}",
            failure.message
        );
        // The wire shape types.ts mirrors: code + home + message.
        let json = serde_json::to_value(failure).unwrap();
        assert_eq!(json["code"], "home_no_store");
        assert_eq!(json["home"], failure.home);
        assert_eq!(json["message"], failure.message);
        // The session observes through the control home; the record stays.
        assert_eq!(
            std::fs::canonicalize(session.api.home()).unwrap(),
            std::fs::canonicalize(&control).unwrap()
        );
    }

    /// R5, second kind: a stored active home whose folder vanished is
    /// `home_unavailable` — a different token from `home_no_store`, so the
    /// consumer can tell "bring the store back" from "bring the path back".
    #[test]
    fn missing_active_home_degrades_as_unavailable() {
        let dir = tempfile::TempDir::new().unwrap();
        let control = dir.path().join("control");
        let gone = dir.path().join("gone/.devai");
        std::fs::create_dir_all(&control).unwrap();

        let mut homes = Homes::load(&control).unwrap();
        // Registered while it still existed, then the folder disappeared.
        std::fs::create_dir_all(&gone).unwrap();
        homes.bootstrap_launch(&gone, true).unwrap();
        drop(homes);
        std::fs::remove_dir_all(&gone).unwrap();

        let session = Session::open_resolved(
            control.clone(),
            "tester".to_string(),
            control.clone(),
            false,
        )
        .expect("an absent active home degrades the session");

        let failure = session.info.degraded_from.as_ref().unwrap();
        assert_eq!(failure.code, "home_unavailable");
        assert!(failure.home.ends_with("gone/.devai"));
        assert!(
            failure.message.contains("not on disk"),
            "the cause is stated: {}",
            failure.message
        );
    }

    /// R2 quality return: an existing `$DEVAI_HOME` directory whose
    /// `devai.db` was deleted must not be silently re-created as an empty
    /// store and reported as a clean session. A missing directory is a
    /// fresh launch (REQ-HOME-02); an existing directory without a store
    /// is `home_no_store` — and the observation open writes nothing.
    #[test]
    fn existing_launch_dir_without_a_store_degrades_instead_of_creating_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let control = dir.path().join("control");
        let launch = dir.path().join("project/.devai");
        std::fs::create_dir_all(&control).unwrap();
        std::fs::create_dir_all(&launch).unwrap();
        assert!(!launch.join(DB_FILE_NAME).exists(), "store is gone");

        // $DEVAI_HOME names this home explicitly — the old code classified
        // it as a fresh launch and `Api::open` created an empty store.
        let session =
            Session::open_resolved(launch.clone(), "tester".to_string(), control.clone(), true)
                .expect("a deleted store degrades the session, it is no startup death");

        let failure = session
            .info
            .degraded_from
            .as_ref()
            .expect("the deleted store must be reported, not hidden");
        assert_eq!(failure.code, devai_api::codes::HOME_NO_STORE);
        assert!(
            !launch.join(DB_FILE_NAME).exists(),
            "the observer created an empty store over the deleted one"
        );
        assert_eq!(
            std::fs::canonicalize(session.api.home()).unwrap(),
            std::fs::canonicalize(&control).unwrap(),
            "observation falls back to the control home"
        );
        let entries = session.homes.entries();
        assert_eq!(entries.len(), 1, "the launch record still exists");
        assert!(entries[0].active, "the stored active mark is unchanged");
    }

    /// R1/R4 quality return, in acceptance terms: the FIRST RUN of a new
    /// operator — no control home, no registry, no store in the launch home.
    /// Startup must create everything it needs (control home + lock anchor +
    /// registry record + launch store) and return a session instead of the
    /// old exit-2 path. `open_resolved` is exactly what `main` calls, so a
    /// session returned here is the "UI opens" verdict of that run.
    #[test]
    fn first_ever_launch_registers_a_home_with_no_control_home_on_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let control = dir.path().join("home/.devai"); // does not exist
        let launch = dir.path().join("project/.devai"); // does not exist
        assert!(!control.exists() && !launch.exists(), "a clean machine");

        // $DEVAI_HOME set → the launch home is authoritative (OQ-HOME-7)
        // and may be created by the bootstrap (REQ-HOME-02 first launch).
        let session =
            Session::open_resolved(launch.clone(), "tester".to_string(), control.clone(), true)
                .expect("first run must open, not exit 2");

        assert_eq!(
            std::fs::canonicalize(session.api.home()).unwrap(),
            std::fs::canonicalize(&launch).unwrap(),
            "the session observes the launch home it just created"
        );
        assert!(session.info.degraded_from.is_none());
        let registry = control.join(devai_api::HOMES_FILE);
        assert!(registry.is_file(), "the registry was created on first run");
        let entries = devai_api::Homes::load(&control).unwrap().entries().to_vec();
        assert_eq!(entries.len(), 1, "exactly the launch home is registered");
        assert!(entries[0].active && entries[0].enabled);
        assert_eq!(
            std::fs::canonicalize(&entries[0].path).unwrap(),
            std::fs::canonicalize(&launch).unwrap()
        );
        assert!(
            launch.join(DB_FILE_NAME).is_file(),
            "the launch store was created once — Bootstrap created the home"
        );
    }

    /// R5: the API-code → home-token mapping is the whole classification,
    /// so it is pinned directly (the rank case is the one the observation
    /// open itself tolerates, hence not reachable through a real open here).
    #[test]
    fn api_codes_map_to_home_failure_tokens() {
        assert_eq!(home_failure_code("not_found"), "home_no_store");
        assert_eq!(home_failure_code("schema_version"), "home_schema_version");
        assert_eq!(home_failure_code("io"), "home_unavailable");
        assert_eq!(home_failure_code("corrupt"), "home_unavailable");
    }

    /// R3 quality return: the bootstrap boundary is TYPED. A corrupt
    /// registry is a refusal of the control home with the
    /// `homes_registry_corrupt` token and the registry path — not a
    /// sentence the caller would have to parse.
    #[test]
    fn corrupt_registry_fails_the_bootstrap_with_a_typed_code() {
        let dir = tempfile::TempDir::new().unwrap();
        let control = dir.path().join("control");
        let launch = dir.path().join("project/.devai");
        std::fs::create_dir_all(&control).unwrap();
        // A document this plane can never have written: fails loud.
        std::fs::write(control.join(devai_api::HOMES_FILE), "{not json").unwrap();

        let failure =
            match Session::open_resolved(launch, "tester".to_string(), control.clone(), true) {
                Ok(_) => panic!("a corrupt registry is a startup refusal"),
                Err(failure) => failure,
            };
        assert_eq!(failure.code, devai_api::codes::HOMES_REGISTRY_CORRUPT);
        assert_eq!(
            std::fs::canonicalize(&failure.home).unwrap(),
            std::fs::canonicalize(&control).unwrap(),
            "the failure names the control home"
        );
        assert!(
            failure.message.contains("homes registry"),
            "the operator text still says what failed: {}",
            failure.message
        );
        // The rendered line starts with the machine code (main.rs prints it).
        let rendered = failure.to_string();
        assert!(
            rendered.starts_with("[homes_registry_corrupt]"),
            "Display leads with the code: {rendered}"
        );
    }

    /// R3: a registry I/O failure (not a parse refusal) carries the
    /// registry-plane token `homes_registry_unavailable`, so a consumer can
    /// tell "repair the document" from "the control home is not usable".
    #[test]
    fn registry_io_failures_map_to_the_unavailable_token() {
        use devai_api::codes;
        let control = Path::new("/control");
        let io = ApiError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ));
        let mapped = registry_failure(control, &io);
        assert_eq!(mapped.code, codes::HOMES_REGISTRY_UNAVAILABLE);
        assert_eq!(mapped.home, "/control");

        let corrupt = ApiError::Refused {
            code: devai_api::ErrorCode(codes::HOMES_REGISTRY_CORRUPT),
            message: "bad json".to_string(),
        };
        assert_eq!(
            registry_failure(control, &corrupt).code,
            codes::HOMES_REGISTRY_CORRUPT
        );
    }
}
