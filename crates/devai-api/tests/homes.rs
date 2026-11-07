//! The homes registry plane (REQ-HOME-01…09): the registry is stored data
//! of the control home, restart-safe; management writes land ONLY in
//! homes.json; merged views read homes live with honest attribution; an
//! absent home localizes its error and never collapses the view.
//!
//! The tests drive `Homes` directly (the API-plane object the UI commands
//! wrap) — the same code path, no process env mutation anywhere.

use std::path::{Path, PathBuf};

use chrono::Utc;
use devai_api::{HomeRecordsView, Homes};
use devai_core::types::*;
use devai_store::Store;

fn seed_home(root: &Path, project: &str, titles: &[&str]) {
    let store = Store::open(root).unwrap();
    let pid = new_id();
    store
        .create_project(&Project {
            id: pid,
            name: project.into(),
            description: String::new(),
            repos: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();
    for t in titles {
        let now = Utc::now();
        store
            .create_task(&Task {
                id: new_id(),
                project_id: Some(pid),
                title: (*t).into(),
                business_description: String::new(),
                human_name: None,
                state: TaskState::Draft,
                return_state: None,
                requirements_seq: 0,
                lock_version: 0,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
    }
}

/// A home as the PREVIOUS build wrote it (schema 7): projects and tasks
/// rows, and no `tasks.human_name` — the 009 rung has not run. Built with
/// plain SQL on purpose: `Store::open` would migrate it, which is exactly
/// what an observer must never do (REQ-HOME-09).
fn seed_lagging_home(root: &Path, project: &str, titles: &[&str]) {
    std::fs::create_dir_all(root).unwrap();
    let conn = rusqlite::Connection::open(root.join("devai.db")).unwrap();
    conn.execute_batch(
        "PRAGMA user_version=7;
         CREATE TABLE projects(id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE,
           description TEXT NOT NULL DEFAULT '', repos TEXT NOT NULL DEFAULT '[]',
           created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
         CREATE TABLE tasks(id TEXT PRIMARY KEY, project_id TEXT NULL REFERENCES projects(id),
           title TEXT NOT NULL, business_description TEXT NOT NULL DEFAULT '',
           state TEXT NOT NULL, return_state TEXT NULL,
           requirements_seq INTEGER NOT NULL DEFAULT 0, lock_version INTEGER NOT NULL DEFAULT 1,
           created_at TEXT NOT NULL, updated_at TEXT NOT NULL);",
    )
    .unwrap();
    let pid = new_id().to_string();
    conn.execute(
        "INSERT INTO projects(id,name,created_at,updated_at)
           VALUES(?1,?2,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
        rusqlite::params![pid, project],
    )
    .unwrap();
    for (i, t) in titles.iter().enumerate() {
        conn.execute(
            "INSERT INTO tasks(id,project_id,title,state,updated_at,created_at)
               VALUES(?1,?2,?3,'DRAFT',?4,'2026-01-01T00:00:00Z')",
            rusqlite::params![
                new_id().to_string(),
                pid,
                t,
                format!("2026-01-0{}T00:00:00Z", i + 2)
            ],
        )
        .unwrap();
    }
}

/// The stored rank and table count of a home, read straight from the file:
/// the observer's own claims are checked against the bytes on disk.
fn rank_and_tables(root: &Path) -> (i64, i64) {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("devai.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let rank: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let mut stmt = conn
        .prepare("SELECT count(*) FROM sqlite_master WHERE type='table'")
        .unwrap();
    let tables: i64 = stmt.query_row([], |r| r.get(0)).unwrap();
    (rank, tables)
}

/// A control home is a directory that may or may not carry a devai store;
/// the registry file lives inside it without one (it is a platform file,
/// not a domain table).
fn load(control: &Path) -> Homes {
    Homes::load(control).unwrap()
}

#[test]
fn empty_registry_launch_registers_and_activates_the_launch_home() {
    let dir = tempfile::TempDir::new().unwrap();
    let control = dir.path().join("control");
    let launch = control.clone();
    std::fs::create_dir_all(&launch).unwrap();
    seed_home(&launch, "devai", &["first"]);

    let mut homes = load(&control);
    assert!(
        homes.entries().is_empty(),
        "before the first record: empty, valid"
    );
    let active = homes.bootstrap_launch(&launch, true).unwrap();
    assert!(
        active.registered_here,
        "the first record of an empty registry is created by this launch"
    );
    let active = active.active;
    assert_eq!(active, Some(launch.canonicalize().unwrap()));
    assert_eq!(homes.entries().len(), 1);
    let e = &homes.entries()[0];
    assert!(e.enabled && e.active);
    assert!(!e.project.is_empty());
    assert!(e.added_at <= Utc::now());

    // Restart survival: a reload sees the record without re-adding.
    let again = load(&control);
    assert_eq!(again.entries().len(), 1);
    assert!(again.entries()[0].active);
}

#[test]
fn launch_from_a_new_home_registers_it_without_losing_records_or_env_priority() {
    let dir = tempfile::TempDir::new().unwrap();
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    seed_home(&a, "alpha", &[]);
    seed_home(&b, "beta", &[]);

    // A starts as the launch home (env set) → A active.
    let mut homes = load(&control);
    homes.bootstrap_launch(&a, true).unwrap();

    // B launches WITHOUT env: the record appears, the saved active stays
    // with A (ratified OQ-HOME-7: no env → the stored record wins).
    let active = homes.bootstrap_launch(&b, false).unwrap().active;
    assert_eq!(active, Some(a.canonicalize().unwrap()));
    assert_eq!(homes.entries().len(), 2);
    assert!(homes.entries().iter().all(|e| e.enabled));
    assert_eq!(
        homes.entries().iter().filter(|e| e.active).count(),
        1,
        "exactly one active is an invariant"
    );

    // B launches WITH env: env is authoritative → B becomes the active
    // record, A's record survives.
    let mut homes = load(&control);
    let active = homes.bootstrap_launch(&b, true).unwrap().active;
    assert_eq!(active, Some(b.canonicalize().unwrap()));
    assert_eq!(homes.entries().len(), 2);
    let reloaded = load(&control);
    assert_eq!(
        reloaded.active_entry().map(|e| e.path.clone()),
        Some(b.canonicalize().unwrap().display().to_string())
    );
}

#[test]
fn add_refuses_missing_paths_and_paths_without_a_store() {
    let dir = tempfile::TempDir::new().unwrap();
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).unwrap();
    let mut homes = load(&control);

    let ghost = dir.path().join("ghost");
    let e = homes.add(&ghost).unwrap_err();
    assert_eq!(e.code().0, "invalid_input");

    let naked = dir.path().join("naked");
    std::fs::create_dir_all(&naked).unwrap();
    let e = homes.add(&naked).unwrap_err();
    assert_eq!(e.code().0, "home_no_store");

    assert!(homes.entries().is_empty(), "refusals write nothing");
}

#[test]
fn add_refuses_an_uninitialised_or_foreign_store_file() {
    // R1 quality return: the existence of a file named devai.db is not a
    // home. The platform's own observer treats rank 0 as "not an
    // initialised home" (devai-store read_initialised_rank), so `add` must
    // refuse by the same probe instead of registering the path.
    let dir = tempfile::TempDir::new().unwrap();
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).unwrap();
    let mut homes = load(&control);

    // A zero-byte devai.db: opens, carries no schema (rank 0).
    let empty = dir.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::write(empty.join("devai.db"), b"").unwrap();
    let e = homes.add(&empty).unwrap_err();
    assert_eq!(e.code().0, devai_api::codes::HOME_NO_STORE);

    // A file of another format under the store name: unreadable as a
    // store, refused with a typed home_* code (never a panic, never a
    // registration).
    let foreign = dir.path().join("foreign");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::write(foreign.join("devai.db"), b"{\"not\": \"sqlite\"}").unwrap();
    let e = homes.add(&foreign).unwrap_err();
    assert_eq!(e.code().0, devai_api::codes::HOME_UNAVAILABLE);

    assert!(
        homes.entries().is_empty(),
        "refusals of uninitialised/foreign stores write nothing"
    );
    assert!(!control.join("homes.json").exists());
}

#[test]
fn add_registers_a_live_home_and_readd_reuses_the_recent_record() {
    let dir = tempfile::TempDir::new().unwrap();
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).unwrap();
    let tripsales = dir.path().join("tripsales");
    seed_home(&tripsales, "tripsales", &["gate-task"]);

    let mut homes = load(&control);
    let rec = homes.add(&tripsales).unwrap();
    assert_eq!(
        rec.project, "tripsales",
        "label reads the home's own project"
    );
    assert!(
        rec.enabled && rec.active,
        "the first registration has no competing active record, so the home \
         the operator named becomes the active one (ratified OQ-HOME-8: an \
         explicit act, never a silent default)"
    );
    // The registry file lives ONLY in the control home; the added home
    // gained no platform artifacts (REQ-HOME-06: one writable target).
    assert!(control.join("homes.json").exists());
    assert!(!tripsales.join("homes.json").exists());

    homes.set_enabled(&rec.path, false).unwrap();
    let first_added_at = homes.entries()[0].added_at;
    let again = homes.add(Path::new(&rec.path)).unwrap();
    assert_eq!(
        again.added_at, first_added_at,
        "recent-homes reuse keeps added_at"
    );
    assert!(again.enabled);
    assert_eq!(homes.entries().len(), 1);
}

/// Fixture of the disable tests: a control home and one seeded home already
/// registered and active. Returns the registry and that home's registry key.
fn single_home_registry(dir: &Path) -> (Homes, String) {
    let control = dir.join("control");
    std::fs::create_dir_all(&control).unwrap();
    let a = dir.join("a");
    seed_home(&a, "alpha", &["x"]);
    let mut homes = load(&control);
    homes.bootstrap_launch(&a, true).unwrap();
    let key = homes.entries()[0].path.clone();
    (homes, key)
}

#[test]
fn disabling_the_only_enabled_home_leaves_no_active_home() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut homes, key) = single_home_registry(dir.path());

    // Ratified OQ-HOME-8: switching the ACTIVE home off is a normal action.
    // With no other enabled record it leaves the honest "no active home"
    // state — no refusal, no silently chosen default.
    let recs = homes.set_enabled(&key, false).unwrap();
    assert_eq!(
        recs.len(),
        1,
        "the disabled record survives in the registry"
    );
    assert!(!recs[0].enabled);
    assert!(
        !recs[0].active,
        "the active marker leaves with the enabled flag"
    );
    assert!(
        homes.active_entry().is_none(),
        "no enabled home remains: no active home, not a default one"
    );
}

#[test]
fn disabling_keeps_each_home_on_disk_and_leaves_the_merged_view_empty() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut homes, key) = single_home_registry(dir.path());
    homes.set_enabled(&key, false).unwrap();

    // Disabling keeps the record and the home's data on disk (REQ-HOME-03).
    let b = dir.path().join("b");
    seed_home(&b, "beta", &["y"]);
    homes.add(&b).unwrap();
    let bp = homes
        .entries()
        .iter()
        .find(|e| e.project == "beta")
        .unwrap()
        .path
        .clone();
    let recs = homes.set_enabled(&bp, false).unwrap();
    assert_eq!(
        recs.len(),
        2,
        "the disabled record survives in the registry"
    );
    assert!(
        homes.tasks_all(None).unwrap().is_empty(),
        "both homes are switched off: the merged view is honestly empty"
    );
    assert!(
        PathBuf::from(&key).join("devai.db").exists()
            && PathBuf::from(&bp).join("devai.db").exists()
    );
}

/// Fixture of the handover tests: three seeded homes (alpha active, beta,
/// gamma) in registry order. Returns the registry and the three keys.
fn three_home_registry(dir: &Path) -> (Homes, Vec<String>) {
    let control = dir.join("control");
    std::fs::create_dir_all(&control).unwrap();
    let mut homes = load(&control);
    for (dir_name, project) in [("a", "alpha"), ("b", "beta"), ("c", "gamma")] {
        seed_home(&dir.join(dir_name), project, &[]);
    }
    homes.bootstrap_launch(&dir.join("a"), true).unwrap();
    homes.add(&dir.join("b")).unwrap();
    homes.add(&dir.join("c")).unwrap();
    let keys = homes.entries().iter().map(|e| e.path.clone()).collect();
    (homes, keys)
}

/// Ratified OQ-HOME-8, successor rule: the next ENABLED record in registry
/// order becomes active after the active one is switched off, and the
/// registry never holds two actives.
#[test]
fn switching_the_active_home_off_hands_the_marker_to_the_next_enabled() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut homes, keys) = three_home_registry(dir.path());
    let (ap, bp) = (keys[0].clone(), keys[1].clone());

    let recs = homes.set_enabled(&ap, false).unwrap();

    let active: Vec<&str> = recs
        .iter()
        .filter(|r| r.active)
        .map(|r| r.path.as_str())
        .collect();
    assert_eq!(
        active,
        vec![bp.as_str()],
        "the next enabled record takes over"
    );
    assert_eq!(
        recs.iter().filter(|r| r.active).count(),
        1,
        "exactly one active record — never two"
    );
}

/// Ratified OQ-HOME-8, tail wrap: disabling the last enabled records leaves
/// "no active home"; enabling one afterwards adopts exactly the home the
/// operator named (the FIRST record here, reached by wrapping).
#[test]
fn disabling_every_home_then_enabling_one_adopts_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut homes, keys) = three_home_registry(dir.path());
    let (ap, bp, cp) = (keys[0].clone(), keys[1].clone(), keys[2].clone());
    homes.set_enabled(&ap, false).unwrap();
    homes.set_enabled(&bp, false).unwrap();
    assert!(
        homes
            .entries()
            .iter()
            .find(|e| e.path == cp)
            .unwrap()
            .active
    );

    let _ = homes.set_enabled(&cp, false).unwrap();
    assert!(homes.active_entry().is_none());
    let recs = homes.set_enabled(&ap, true).unwrap();
    assert!(
        recs.iter().find(|r| r.path == ap).unwrap().active,
        "enabling a home while no active remains adopts it"
    );
}

/// R1 quality return (ratified OQ-HOME-8): starting the process from inside
/// a switched-off home does NOT switch anything on. With every record
/// switched off the registry keeps the honest "no active home" state —
/// `enabled` and `active` stay false and `bootstrap_launch` reports no
/// active path instead of silently picking one.
#[test]
fn launch_with_every_home_switched_off_changes_nothing() {
    let dir = tempfile::TempDir::new().unwrap();
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    seed_home(&a, "alpha", &[]);
    seed_home(&b, "beta", &[]);
    let mut homes = load(&control);
    homes.bootstrap_launch(&a, true).unwrap();
    homes.add(&b).unwrap();
    let paths: Vec<String> = homes.entries().iter().map(|e| e.path.clone()).collect();
    for path in &paths {
        homes.set_enabled(path, false).unwrap();
    }
    assert!(
        homes.active_entry().is_none(),
        "precondition: every home is switched off"
    );

    let resolution = homes.bootstrap_launch(&a, false).unwrap();

    assert_eq!(
        resolution.active, None,
        "a launch must not pick an active home when none is enabled"
    );
    let reloaded = load(&control);
    assert!(
        reloaded.entries().iter().all(|e| !e.enabled && !e.active),
        "the launch re-enabled or reactivated a switched-off record: {:?}",
        reloaded.entries()
    );
}

/// RA1 quality return: becoming the stored active home is a REGISTRY act,
/// not a migration. A home of an older schema rank (here: v7, the shape the
/// tripsales home carries) is opened through the observation door — its
/// `user_version`, tables and rows stay exactly as they were, while the
/// registry flip happens and `home.list` still marks the version.
#[test]
fn activation_never_migrates_the_activated_home() {
    let dir = tempfile::TempDir::new().unwrap();
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).unwrap();
    let launch = dir.path().join("launch");
    let lagging = dir.path().join("lagging");
    seed_home(&launch, "launcher", &[]);
    seed_lagging_home(&lagging, "lagging", &[]);
    let before = rank_and_tables(&lagging);

    let mut homes = load(&control);
    homes.bootstrap_launch(&launch, true).unwrap();
    let lp = homes.add(&lagging).unwrap().path;

    let api = devai_api::Api::activate_home(&mut homes, &lp, "operator").unwrap();
    assert_eq!(
        api.home(),
        lagging.canonicalize().unwrap(),
        "the session holds the chosen home"
    );
    assert_eq!(homes.active_entry().unwrap().path, lp, "the flip landed");
    assert_eq!(
        rank_and_tables(&lagging),
        before,
        "activation never runs the migration ladder on the activated home"
    );
    assert_lagging_home_readable(&mut homes, &lagging, &lp);
}

/// The activated lagging home reads its rows and is marked by version.
fn assert_lagging_home_readable(homes: &mut Homes, lagging: &Path, lp: &str) {
    let store = Store::open_read_only(lagging).unwrap();
    assert_eq!(
        store.list_projects().unwrap().len(),
        1,
        "the lagging rows stay readable, nothing was seeded"
    );
    let rec = homes
        .records()
        .unwrap()
        .into_iter()
        .find(|r| r.path == lp)
        .unwrap();
    assert!(rec.active && rec.available);
    assert_eq!(rec.schema_version, Some(7));
    assert!(
        rec.version_note.as_deref().unwrap_or("").contains("behind"),
        "the UI still marks the home as a behind version"
    );
}

/// R1/R2 quality return: the live tripsales shape — a home of schema 7,
/// whose `tasks` table has no `human_name` column. `home.tasks_all` must
/// READ those rows (REQ-HOME-08 AC3), mark the home "behind", and leave the
/// file exactly as it was (REQ-HOME-09: the observer never migrates).
#[test]
fn tasks_all_reads_a_lagging_home_without_a_human_name_column() {
    let dir = tempfile::TempDir::new().unwrap();
    let control = dir.path().join("control");
    let lagging = dir.path().join("tripsales");
    seed_home(&control, "devai", &[]);
    seed_lagging_home(&lagging, "tripsales", &["gate task", "second"]);
    let before = rank_and_tables(&lagging);

    let mut homes = load(&control);
    homes.bootstrap_launch(&control, true).unwrap();
    let lp = homes.add(&lagging).unwrap().path;
    let view = homes.tasks_all(None).unwrap();

    let block = view
        .iter()
        .find(|b| b.home_path == lp)
        .expect("the lagging home has its own block");
    assert_lagging_tasks_are_read(block);
    assert_eq!(
        rank_and_tables(&lagging),
        before,
        "observation migrated or reshaped the lagging home"
    );
}

/// The rows are read (not a missing-column failure), the absent v9 name is
/// NULL and the home is marked by its rank.
fn assert_lagging_tasks_are_read(block: &HomeRecordsView<Task>) {
    assert!(
        block.available,
        "the lagging home is readable, not failed: {:?}",
        block.error
    );
    let mut titles: Vec<&str> = block.records.iter().map(|t| t.title.as_str()).collect();
    titles.sort_unstable();
    assert_eq!(
        titles,
        vec!["gate task", "second"],
        "rows of a home behind this build are read, never dropped"
    );
    assert!(
        block.records.iter().all(|t| t.human_name.is_none()),
        "the absent v9 column projects as NULL — no name is invented"
    );
    assert!(
        block
            .version_note
            .as_deref()
            .unwrap_or("")
            .contains("behind"),
        "the home is marked by its version: {:?}",
        block.version_note
    );
}

/// Fixture of the activation tests: control home plus seeded alpha (active)
/// and beta homes. Returns the registry, both home dirs and the keys.
fn two_home_registry(dir: &Path) -> (Homes, PathBuf, PathBuf, String, String) {
    let control = dir.join("control");
    std::fs::create_dir_all(&control).unwrap();
    let a = dir.join("a");
    let b = dir.join("b");
    seed_home(&a, "alpha", &[]);
    seed_home(&b, "beta", &[]);
    let mut homes = load(&control);
    homes.bootstrap_launch(&a, true).unwrap();
    homes.add(&b).unwrap();
    let keys: Vec<String> = homes.entries().iter().map(|e| e.path.clone()).collect();
    (homes, a, b, keys[0].clone(), keys[1].clone())
}

#[test]
fn activation_flips_the_stored_active_exactly_once() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut homes, _a, b, _ap, bp) = two_home_registry(dir.path());
    let control = dir.path().join("control");

    let target = homes.activation_target(&bp).unwrap();
    assert_eq!(target, b.canonicalize().unwrap());
    homes.set_active(&target).unwrap();

    assert_eq!(homes.active_entry().unwrap().path, bp);
    let reloaded = load(&control);
    assert_eq!(reloaded.active_entry().unwrap().path, bp);
    assert_eq!(
        reloaded.entries().iter().filter(|e| e.active).count(),
        1,
        "the flip keeps exactly one active"
    );
}

#[test]
fn activation_target_refuses_unknown_and_vanished_homes() {
    let dir = tempfile::TempDir::new().unwrap();
    let (homes, a, _b, ap, _bp) = two_home_registry(dir.path());

    // Unknown path: typed not_found. Missing dir: typed home_unavailable.
    assert_eq!(
        homes
            .activation_target("/nope/not-registered")
            .unwrap_err()
            .code()
            .0,
        "not_found"
    );
    std::fs::remove_dir_all(a.canonicalize().unwrap()).unwrap();
    assert_eq!(
        homes.activation_target(&ap).unwrap_err().code().0,
        "home_unavailable"
    );
}

/// The shared fixture of the merged-view tests: alpha (active), beta and a
/// ghost home whose folder is deleted after registration. Returns the
/// registry and the two non-active home paths.
fn mixed_registry(dir: &Path) -> (Homes, String, String) {
    let control = dir.join("control");
    std::fs::create_dir_all(&control).unwrap();
    let a = dir.join("a");
    let b = dir.join("b");
    // Same-named tasks in two homes — the view must never collapse them.
    seed_home(&a, "alpha", &["same title", "alpha only"]);
    seed_home(&b, "beta", &["same title"]);
    let ghost = dir.join("ghost");
    seed_home(&ghost, "ghosty", &["was here"]);

    let mut homes = load(&control);
    homes.bootstrap_launch(&a, true).unwrap();
    let beta_path = homes.add(&b).unwrap().path;
    let ghost_path = homes.add(&ghost).unwrap().path;
    std::fs::remove_dir_all(ghost.canonicalize().unwrap()).unwrap();
    (homes, beta_path, ghost_path)
}

#[test]
fn merged_views_attribute_live_rows_and_localize_dead_homes() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut homes, beta_path, ghost_path) = mixed_registry(dir.path());

    let view = homes.projects_all(None).unwrap();
    assert_eq!(
        view.len(),
        3,
        "the absent home keeps its block (path + cause)"
    );
    let active_block = view.iter().find(|v| v.is_active).unwrap();
    assert_eq!(active_block.home_project, "alpha");
    assert_eq!(active_block.records.len(), 1);
    let ghost_block = view.iter().find(|v| v.home_path == ghost_path).unwrap();
    assert!(!ghost_block.available);
    // R2 quality return (rel/error-context): the cause is TYPED on the
    // wire — code + path + hint ride beside the human text, so a consumer
    // branches on the kind without parsing prose.
    let ghost_issue = ghost_block
        .error
        .as_ref()
        .expect("the dead home states its cause");
    assert_eq!(ghost_issue.code, devai_api::codes::HOME_UNAVAILABLE);
    assert_eq!(ghost_issue.path, ghost_path, "the issue names its home");
    assert!(
        ghost_issue.hint.is_some(),
        "the platform hint reaches the wire: {ghost_issue:?}"
    );
    assert!(ghost_issue.message.contains("not on disk"));
    assert!(ghost_block.records.is_empty(), "its rows are not invented");
    let beta = view.iter().find(|v| v.home_project == "beta").unwrap();
    assert_eq!(beta.records.len(), 1, "the other homes still read normally");
    assert_eq!(beta.home_path, beta_path);
    // The observation NEVER touched a home's store data (REQ-HOME-06 AC2).
    let sa = Store::open_read_only(Path::new(&beta.home_path)).unwrap();
    assert_eq!(sa.list_tasks().unwrap().len(), 1);
}

/// REQ-HOME-04 AC2/AC3: live task reads, attribution per block, one honest
/// snapshot timestamp per observation, single-home filter, and a typed
/// refusal when the filter names a switched-off home.
#[test]
fn merged_task_view_keeps_same_titles_distinct_and_filters_one_home() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut homes, beta_path, _) = mixed_registry(dir.path());

    let tv = homes.tasks_all(None).unwrap();
    let same_titles: Vec<_> = tv
        .iter()
        .flat_map(|b| {
            b.records
                .iter()
                .filter(|t| t.title == "same title")
                .map(move |t| (b.home_path.clone(), t.id))
        })
        .collect();
    assert_eq!(same_titles.len(), 2, "two homes carry one such task each");
    assert_ne!(
        same_titles[0].0, same_titles[1].0,
        "distinct homes, distinct rows"
    );
    assert_eq!(
        tv[0].snapshot_at, tv[1].snapshot_at,
        "one honest snapshot timestamp per observation"
    );

    // Single-home filter (REQ-HOME-04 AC2: default all, on request one).
    let one = homes.tasks_all(Some(&beta_path)).unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].home_project, "beta");

    // A disabled home cannot answer the filter — typed refusal, not silence.
    homes.set_enabled(&beta_path, false).unwrap();
    let e = homes.tasks_all(Some(&beta_path)).unwrap_err();
    assert_eq!(e.code().0, "not_found");
}

/// REQ-HOME-07 AC4: the path returns — the SAME registry record reads the
/// home again, with no re-add action by the operator.
#[test]
fn a_returning_home_reads_again_without_a_re_add() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut homes, _, ghost_path) = mixed_registry(dir.path());
    assert!(
        !homes
            .records()
            .unwrap()
            .iter()
            .find(|r| r.path == ghost_path)
            .unwrap()
            .available,
        "the ghost home starts unavailable"
    );

    seed_home(
        &dir.path().join("ghost"),
        "ghosty",
        &["was here", "and here"],
    );
    let back = homes.records().unwrap();
    let g = back.iter().find(|r| r.path == ghost_path).unwrap();
    assert!(
        g.available && g.read_error.is_none(),
        "return restores access"
    );
    let tv2 = homes.tasks_all(None).unwrap();
    let g2 = tv2.iter().find(|b| b.home_path == ghost_path).unwrap();
    assert_eq!(g2.records.len(), 2, "returned home reads live again");
}

/// rl/idempotency, process-level (R4 quality return): the RMW window is
/// serialized across PROCESSES, not merely across calls in one process.
/// Two falsifiable facts, both driven through the real `Homes` code:
///   1. mutual exclusion is DETERMINISTIC, not statistical: this test
///      process takes the registry lock like any other writer, then lets
///      the two child processes into their write loops. While the lock is
///      held nothing may land in the document — a process-local mutex
///      would fail exactly here, because the holder is another process;
///   2. no record is lost and exactly one home ends active once both
///      children race for the whole run (CHILD_WRITES add+activate cycles
///      each, after the release).
/// The child side is this same function re-executed with a sentinel env
/// var, so the spawned processes are ordinary registry writers.
#[cfg(unix)]
#[test]
fn registry_writes_from_two_processes_never_lose_records() {
    if let Some(role) = std::env::var_os(CHILD_ROLE_ENV) {
        let control = PathBuf::from(std::env::var_os(CHILD_CONTROL_ENV).expect("child control"));
        let root = PathBuf::from(std::env::var_os(CHILD_ROOT_ENV).expect("child root"));
        child_registry_writer(&role.to_string_lossy(), &control, &root);
        return;
    }
    let dir = tempfile::TempDir::new().unwrap();
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).unwrap();
    let root = dir.path().join("homes");
    std::fs::create_dir_all(&root).unwrap();

    let mut first = spawn_writer("a", &control, &root);
    let mut second = spawn_writer("b", &control, &root);
    let parked = release_under_the_lock(&control, &root);
    let (ok1, log1) = collect(&mut first);
    let (ok2, log2) = collect(&mut second);
    let logs = format!("--- writer a ---\n{log1}\n--- writer b ---\n{log2}");
    assert!(ok1 && ok2, "a registry writer process died:\n{logs}");
    assert!(
        parked == 0,
        "{parked} record(s) landed while another process held {} — the write \
         window is not exclusive across processes",
        devai_api::HOMES_LOCK_FILE
    );
    assert_nothing_lost_and_one_active(&control, &logs);
}

/// Fact (1): hold the registry lock like a competing writer, release the
/// children into their loops, and report how many records landed during the
/// hold. Only a cross-process lock makes that number zero.
#[cfg(unix)]
fn release_under_the_lock(control: &Path, root: &Path) -> usize {
    let gate = hold_registry_lock(control);
    std::fs::write(root.join(GO_FILE), b"go").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(HOLD_MS));
    let parked = load(control).entries().len();
    drop(gate);
    parked
}

/// Fact (2): after the race every home is recorded once and exactly one
/// record carries the active mark.
#[cfg(unix)]
fn assert_nothing_lost_and_one_active(control: &Path, logs: &str) {
    let mut on_disk = load(control);
    let recs = on_disk.records().unwrap();
    assert_eq!(
        recs.len(),
        2 * CHILD_WRITES,
        "records lost to a concurrent writer ({} on disk, {} expected)\n{logs}",
        recs.len(),
        2 * CHILD_WRITES
    );
    let mut paths: Vec<&str> = recs.iter().map(|r| r.path.as_str()).collect();
    paths.sort_unstable();
    paths.dedup();
    assert_eq!(
        paths.len(),
        2 * CHILD_WRITES,
        "the same home was recorded twice"
    );
    let active: Vec<&str> = recs
        .iter()
        .filter(|r| r.active)
        .map(|r| r.path.as_str())
        .collect();
    assert_eq!(
        active.len(),
        1,
        "exactly one active home after concurrent activation: {active:?}"
    );
}

const CHILD_ROLE_ENV: &str = "DEVAI_HOMES_TEST_CHILD_ROLE";
const CHILD_CONTROL_ENV: &str = "DEVAI_HOMES_TEST_CHILD_CONTROL";
const CHILD_ROOT_ENV: &str = "DEVAI_HOMES_TEST_CHILD_ROOT";
/// Homes added per process: enough interleaving that an unlocked RMW
/// window loses a record with near-certainty.
const CHILD_WRITES: usize = 25;
/// The parent's lock hold: far longer than one registry transaction, so a
/// write that is not serialized lands inside it.
const HOLD_MS: u64 = 400;
/// The parent's release marker: children start writing only after it.
const GO_FILE: &str = "go";

/// Spawn one registry writer process: the same test binary, re-executed
/// with the sentinel env vars so it takes the child branch.
#[cfg(unix)]
fn spawn_writer(role: &str, control: &Path, root: &Path) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().unwrap())
        .arg("registry_writes_from_two_processes_never_lose_records")
        .env(CHILD_ROLE_ENV, role)
        .env(CHILD_CONTROL_ENV, control)
        .env(CHILD_ROOT_ENV, root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn registry writer process")
}

/// Take the registry lock exactly the way the platform does (flock on the
/// lock anchor). Dropping the handle releases it.
#[cfg(unix)]
fn hold_registry_lock(control: &Path) -> std::fs::File {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(control.join(devai_api::HOMES_LOCK_FILE))
        .unwrap();
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    assert_eq!(rc, 0, "flock the registry lock anchor");
    file
}

/// Drain a child's pipes to EOF, then reap it: reading before waiting means
/// a chatty child can never deadlock on a full pipe buffer.
#[cfg(unix)]
fn collect(c: &mut std::process::Child) -> (bool, String) {
    use std::io::Read;
    let mut buf = String::new();
    if let Some(mut o) = c.stdout.take() {
        let _ = o.read_to_string(&mut buf);
    }
    if let Some(mut e) = c.stderr.take() {
        let _ = e.read_to_string(&mut buf);
    }
    let ok = c.wait().map(|s| s.success()).unwrap_or(false);
    (ok, buf)
}

/// One concurrent writer: wait for the parent's release, then seed a fresh
/// home, register it, make it active — repeatedly, under contention with
/// the sibling process.
#[cfg(unix)]
fn child_registry_writer(role: &str, control: &Path, root: &Path) {
    let go = root.join(GO_FILE);
    let mut released = false;
    for _ in 0..5_000 {
        if go.exists() {
            released = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(released, "the parent never released the writer ({role})");
    let mut homes = load(control);
    for i in 0..CHILD_WRITES {
        let home = root.join(format!("{role}{i}"));
        seed_home(&home, &format!("{role}-project-{i}"), &[]);
        homes.add(&home).unwrap();
        homes.set_active(&home).unwrap();
    }
}

/// Same-process RMW rebase: two loaded handles on one control home must
/// not lose records to last-writer-wins — every registry write re-reads
/// the disk document inside the lock, so the second writer commits on top
/// of the first one's truth. The cross-PROCESS proof is the test above;
/// this one pins the transaction semantics for a single process.
#[test]
fn concurrent_managers_never_lose_registry_records() {
    let dir = tempfile::TempDir::new().unwrap();
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    seed_home(&a, "alpha", &[]);
    seed_home(&b, "beta", &[]);

    // Both "processes" load the same empty truth…
    let mut h1 = load(&control);
    let mut h2 = load(&control);
    // …then each registers its own home. A naive in-memory writer would
    // now overwrite the other's document (h2's file: only `b`).
    h1.add(&a).unwrap();
    h2.add(&b).unwrap();

    let on_disk = load(&control);
    let projects: Vec<&str> = on_disk
        .entries()
        .iter()
        .map(|e| e.project.as_str())
        .collect();
    assert!(
        projects.contains(&"alpha") && projects.contains(&"beta"),
        "both registrations survived: {projects:?}"
    );
    // Enumeration re-reads the committed document, so the first process
    // sees the second one's registration without any restart.
    let recs = h1.records().unwrap();
    assert_eq!(
        recs.len(),
        2,
        "a fresh enumeration sees the other session's add"
    );
}

/// pf/io-churn (report v1: «метрик не снято») — the measurement probe.
/// Not an assertion: the numbers go to the evidence log. Observing a home
/// costs one read-only sqlite open per home per call; the merged views
/// fan the opens out across scoped threads (pf/parallelism). Operator
/// scale is a handful of homes, so this records the constant rather than
/// pretending a cache (live-snapshot semantics, ratified OQ-HOME-4).
#[test]
#[ignore = "measurement probe: cargo test -p devai-api -- --ignored --nocapture observation_cost"]
fn observation_cost_is_measured_at_operator_scale() {
    use std::time::Instant;
    let dir = tempfile::TempDir::new().unwrap();
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).unwrap();
    let mut homes = load(&control);
    let n = 4;
    for i in 0..n {
        let h = dir.path().join(format!("home{i}"));
        seed_home(&h, &format!("project{i}"), &["a", "b", "c"]);
        if i == 0 {
            homes.bootstrap_launch(&h, true).unwrap();
        } else {
            homes.add(&h).unwrap();
        }
    }
    let t0 = Instant::now();
    let recs = homes.records().unwrap();
    let t1 = Instant::now();
    let tv = homes.tasks_all(None).unwrap();
    let t2 = Instant::now();
    let rows: usize = tv.iter().map(|b| b.records.len()).sum();
    println!(
        "homes={n} records()={:?} tasks_all()={:?} rows={rows} (one RO open/home/call, scoped-thread fan-out)",
        t1 - t0,
        t2 - t1
    );
    assert_eq!(recs.len(), n);
}

#[test]
fn records_carry_attributes_for_the_view_layer() {
    let dir = tempfile::TempDir::new().unwrap();
    let control = dir.path().join("control");
    std::fs::create_dir_all(&control).unwrap();
    let mut homes = load(&control);
    homes.bootstrap_launch(&control, true).unwrap();
    // control home was registered before its store existed; probe shape:
    let recs = homes.records().unwrap();
    assert_eq!(recs.len(), 1);
    let r = &recs[0];
    assert_eq!(
        r.path,
        control.canonicalize().unwrap().display().to_string()
    );
    assert!(r.enabled && r.active);
    assert!(r.added_at <= Utc::now());
    // The JSON document is the stored-data contract: field names pinned.
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(control.join("homes.json")).unwrap())
            .unwrap();
    assert_eq!(doc["version"].as_u64().unwrap(), 1);
    let keys: Vec<&String> = doc["homes"][0].as_object().unwrap().keys().collect();
    for want in ["path", "project", "added_at", "enabled", "active"] {
        assert!(keys.iter().any(|k| *k == want), "homes.json field {want}");
    }
}

/// The domain-plane snapshot the AC names: schema rank plus the ids of every
/// table the registry plane must not touch. Read straight from the file.
type DomainSnapshot = (
    i64,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
);

fn domain_snapshot(control: &Path) -> DomainSnapshot {
    use rusqlite::OpenFlags;
    let conn = rusqlite::Connection::open_with_flags(
        control.join("devai.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let rank: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let ids = |sql: &str| -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        rows
    };
    (
        rank,
        ids("SELECT id FROM projects ORDER BY id"),
        ids("SELECT id FROM tasks ORDER BY id"),
        ids("SELECT id FROM artifacts ORDER BY id"),
        ids("SELECT id FROM workflow_events ORDER BY id"),
        ids("SELECT id FROM decisions ORDER BY id"),
    )
}

/// REQ-HOME-06 AC3: the control home may itself be a WORKING home — the
/// registry file lives inside it. Adding, disabling, re-enabling and
/// activating homes must leave its domain plane exactly as it was: same
/// schema rank, same rows in projects/tasks/artifacts/workflow_events/
/// decisions (counters and the controlling ids), and the registry file as
/// the only write target. Read straight from the file, because the AC names
/// tables and rows, not API shapes.
#[test]
fn registry_management_changes_only_the_registry_of_the_control_home() {
    let dir = tempfile::TempDir::new().unwrap();
    let control = dir.path().join("control");
    seed_home(&control, "devai", &["existing task"]);
    let other = dir.path().join("tripsales");
    seed_home(&other, "tripsales", &["gate task"]);
    let before = domain_snapshot(&control);

    let mut homes = load(&control);
    homes.bootstrap_launch(&other, true).unwrap();
    let rec = homes.add(&other).unwrap();
    homes.set_enabled(&rec.path, false).unwrap();
    homes.set_enabled(&rec.path, true).unwrap();
    devai_api::Api::activate_home(&mut homes, &rec.path, "tester").unwrap();

    assert_eq!(
        domain_snapshot(&control),
        before,
        "registry management gained or lost domain rows (or moved the schema rank) \
         in the home where the registry lives"
    );
    assert!(
        control.join("homes.json").exists(),
        "the one write target of the registry plane is the registry file"
    );
}
