//! Local persistence for Devai: SQLite database plus a content-addressed
//! artifact filestore, and portable task bundles (export/import).
//!
//! Business rules are NOT implemented here: this crate stores the domain
//! data produced by `devai-core` and enforces a few storage-level invariants
//! (append-only event log, optimistic task locking, content hashes).

pub mod bundle;
pub mod config;
mod error;
pub mod models;
mod rows;
pub mod seed;

pub use bundle::{BundleManifest, ImportOutcome};
pub use error::{StoreError, StoreResult};
pub use models::{
    CatalogEdit, MODEL_CONFIG_FILE, ModelBundle, ModelCatalog, PROBEABLE_DRIVERS, ProbePolicy,
    SELF_PROVIDER_DRIVER, bundle_id_from_label, bundle_keys, catalog_edition, catalog_revision,
    load_catalog, probe_keys, restore_catalog_text, write_catalog, write_catalog_with_previous,
};

/// The ONE name of the home's store file (arch/no-magic: the homes cycle
/// spread this literal across api/ui consumers; every join site now reads
/// this constant, so renaming the file is a one-line act with the grep
/// proof in the store itself).
pub const DB_FILE_NAME: &str = "devai.db";

/// The project-home dogma directory name (`<project>/.devai`, ratified
/// OQ-HOME-1 wording): the layout fact lives with the layout owner — the
/// store — and the API/UI reach it through re-exports instead of
/// re-spelling the shape rule.
pub const HOME_DIR_NAME: &str = ".devai";

/// The home's project label derived from the DIRECTORY SHAPE only: a path
/// ending in `/.devai` names its parent directory (`<project>/.devai`).
/// None for any other shape — callers keep their own fallback. This is the
/// mechanical half of the label rule; reading the home's own project rows
/// stays an API concern (`label_for` in devai-api).
pub fn project_label_from_path(root: &Path) -> Option<String> {
    let raw = root.display().to_string();
    let suffix = ["/", HOME_DIR_NAME].concat();
    if raw.ends_with(&suffix) {
        let parent = raw.trim_end_matches(&suffix[..]);
        if let Some(name) = Path::new(parent).file_name() {
            return Some(name.to_string_lossy().to_string());
        }
    }
    None
}

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use devai_core::types::*;
use devai_core::workflow::Mutation;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

// The schema ladder is DATA (REQ-004 / arch/registry-in-data v5): the
// catalog — order, `from`/`to`, stable id, SQL script path and apply-handler
// tag per rung — lives in `migrations/manifest.json`; `build.rs` mirrors it
// (plus the script contents, single-binary discipline) into the generated
// file included here, failing the BUILD when the ladder is not continuous
// 1→N or a cited script is missing. `SCHEMA_VERSION` and `SCHEMA_RUNGS_RAW`
// below are that generated data, not hand-written code: adding a schema
// version appends a manifest record + a .sql file and never edits `open()`.
include!(concat!(env!("OUT_DIR"), "/schema_manifest.rs"));

/// Actor prefix on migration events: `migration:<rung id>` — derived from
/// the registry record, so renaming a rung relabels its events consistently.
const MIGRATION_ACTOR_PREFIX: &str = "migration:";

/// Why the supersession backfill flipped rows — cited verbatim in its
/// events so the one-time repair is auditable, not silent (REQ-003).
const SUPERSESSION_BACKFILL_REASON: &str = "publication did not honor supersession before v3: within each (task_id, kind) the max-seq CURRENT row is kept CURRENT, the rest marked SUPERSEDED (status transition only — nothing deleted)";

/// Why the decisions backfill copied rationale out of event payloads —
/// cited verbatim in its fact events (REQ-BF-004): the one-time extraction
/// is auditable, not silent.
const DECISIONS_BACKFILL_REASON: &str = "approve/reject/return rationales lived only in event payloads before the decisions write-path: each undisclosed rationale is copied VERBATIM into a NEW decision row (actor/created_at from the event columns, refs cite the source event id) — existing decisions and events are never touched";

/// Why the v7 single-writer migration closed RUNNING rows — the ratified
/// marker string (OQ-1: «остальные закрываются FAILED с явной пометкой
/// 'single-writer migration'») written into each closed row's `error`
/// column AND cited verbatim in its repair event, so the one-time closure
/// is auditable, never silent.
const SINGLE_WRITER_REPAIR_REASON: &str = "single-writer migration: the v7 index allows at most one RUNNING execution per task_id; within each duplicated group the newest started_at stays RUNNING (equal started_at resolves on the UUIDv7 id order, same tie discipline as rung 003), the others are closed FAILED — a status/finished_at/error write only, no row or event is deleted or rewritten";

/// The shared 21-column projection of `agent_failures` (task 01a1b60c):
/// producer (`record_agent_failure`) and every read path use ONE column
/// list, so a column added to the table cannot be read in a different order
/// by two surfaces (the `rows::agent_failure_from_row` index order).
const AGENT_FAILURE_COLUMNS: &str = "id,task_id,execution_id,agent_name,driver,llm_provider,model,class,message,exit_code,duration_ms,tail,output_empty,driver_log_tail,driver_log_note,basis,window_count,window_started_at,first_at,last_at,threshold,created_at";

/// The loser predicate of the v7 repair: a RUNNING row that has a strictly
/// better RUNNING sibling in its task under the total order (started_at,
/// id) — newest started_at wins, ties break on the UUIDv7 id (lexicographic
/// order IS time order). Detection runs FIRST inside the rung transaction,
/// exactly like `MULTI_CURRENT_LOSER_PREDICATE` in rung 003, so each closed
/// row is named by its repair event before the UPDATE flips it.
const RUNNING_DUPLICATE_LOSER_PREDICATE: &str = "status = 'RUNNING' AND EXISTS (\
    SELECT 1 FROM role_executions AS b \
    WHERE b.task_id = role_executions.task_id AND b.status = 'RUNNING' \
      AND (b.started_at > role_executions.started_at \
        OR (b.started_at = role_executions.started_at AND b.id > role_executions.id)))";

/// The loser predicate of the v3 repair: a CURRENT row that has a strictly
/// better CURRENT sibling in its (task_id, kind) group under the total
/// order (seq, created_at, id). Kept byte-identical to the UPDATE in
/// migrations/003_artifact_supersession.sql: this detection SELECT runs
/// FIRST inside the rung transaction so the repair events can cite exactly
/// the rows the script flips. The tie-break is deterministic (equal seq is
/// resolved by created_at, then by the unique id — a UUIDv7, so text order
/// is time order), which makes the backfill unconditional: every seeded
/// multi-CURRENT group converges to exactly one CURRENT instead of
/// aborting the open (divergence from the WIP draft 5db7fb5, open question
/// "equal-seq rungs" resolved toward repair, not operator intervention).
const MULTI_CURRENT_LOSER_PREDICATE: &str = "status = 'CURRENT' AND EXISTS (\
    SELECT 1 FROM artifacts AS b \
    WHERE b.task_id = artifacts.task_id AND b.kind = artifacts.kind AND b.status = 'CURRENT' \
      AND (b.seq > artifacts.seq \
        OR (b.seq = artifacts.seq AND b.created_at > artifacts.created_at) \
        OR (b.seq = artifacts.seq AND b.created_at = artifacts.created_at AND b.id > artifacts.id)))";

/// One rung of the ladder, materialised from the generated manifest data.
pub struct SchemaRung {
    /// The version this rung upgrades FROM; execution order is manifest order.
    pub from: i64,
    pub to: i64,
    /// Stable id: cited in migration events (`migration:<id>`) and errors.
    pub id: &'static str,
    /// Repository-relative path of the shipped SQL, manifest data (cited
    /// in repair-event payloads for auditability).
    pub script_path: &'static str,
    /// Apply-handler tag from the manifest, resolved through `RUNG_APPLIERS`.
    pub apply: &'static str,
    /// The script content itself, baked from `script_path` at build time
    /// (the single-binary discipline of `include_str!`, via `build.rs`).
    pub script: &'static str,
}

/// Load the ladder as data: one `SchemaRung` per manifest record, in the
/// manifest's order. The generic executor walks this; it knows no rung name.
pub(crate) fn schema_rungs() -> Vec<SchemaRung> {
    SCHEMA_RUNGS_RAW
        .iter()
        .map(|(id, from, to, script_path, apply, script)| SchemaRung {
            id,
            from: *from,
            to: *to,
            script_path,
            apply,
            script,
        })
        .collect()
}

/// The apply-handler table — the single place where the ladder stays CODE,
/// and why that is the right boundary (REQ-004 allows `apply` to remain a
/// function pointer only with this justification): pure SQL expresses every
/// rung's DDL/DML verbatim (the `"script"` tag), but SQL CANNOT observe the
/// pre-mutation state BEFORE the UPDATE so the backfill events can cite
/// exactly the rows the script flips (`supersession_backfill`), nor parse a
/// JSON payload / mint UUID ids while appending repair rows
/// (`decisions_backfill`), nor — for future rungs — detect table shape
/// before a rebuild. A new pure-SQL rung
/// appends a manifest record and NO code; only genuinely un-expressible
/// behavior appends a handler here. The tag NAME is manifest data; this
/// table maps name → behavior, failing loud on a tag it cannot run.
type RungApply = fn(&Transaction<'_>, &SchemaRung) -> StoreResult<()>;
const RUNG_APPLIERS: &[(&str, RungApply)] = &[
    ("script", apply_script),
    ("supersession_backfill", supersession_backfill),
    ("decisions_backfill", decisions_backfill),
    ("execution_form_columns", execution_form_columns),
    ("single_writer_repair", single_writer_repair),
    ("additive_human_name_column", additive_human_name_column),
    (
        "additive_decision_payload_column",
        additive_decision_payload_column,
    ),
    (
        "additive_model_bundle_columns",
        additive_model_bundle_columns,
    ),
    (
        "additive_model_reasoning_column",
        additive_model_reasoning_column,
    ),
];

/// The switch request of [`Store::apply_bundle_switch`] — the exact content
/// the agent record and the `model.switched` event must carry (REQ-MS-04).
pub struct BundleSwitch {
    pub agent: String,
    pub driver: String,
    pub llm_provider: String,
    pub model: String,
    /// the ratified OQ-6 answer: the reasoning level of the selection — part of the bundle,
    /// carried into the agent record by the same transaction.
    pub reasoning: String,
    pub actor: String,
    /// Event payload WITHOUT `from`/`agent_changed` — the store fills those
    /// in from the row it actually moved.
    pub payload: serde_json::Value,
    pub now: Timestamp,
}

/// Handle to the local store. `root` contains `devai.db` and `blobs/`.
pub struct Store {
    pub(crate) conn: Connection,
    root: PathBuf,
    pub(crate) blobs: PathBuf,
    /// Cached column verdict of `tasks.human_name` for THIS handle (a
    /// property of the stored schema rank, asked once — R1 quality return:
    /// a home behind this build is READ, not failed on a missing column).
    task_human_name: std::cell::OnceCell<bool>,
}

/// Reader handle of an EXISTING home for observation: strictly read-only
/// first (no file handle can write); a WAL home whose wal-index cannot be
/// built read-only (shm missing) falls back to a read-write handle pinned
/// with `query_only=ON` — statements still cannot mutate rows, tables or
/// the schema version. A missing `devai.db` refuses, never creates.
fn reader_conn(db: &Path) -> StoreResult<Connection> {
    if !db.exists() {
        return Err(StoreError::NotFound(
            "home store".into(),
            format!("{} — open_read_only never creates a home", db.display()),
        ));
    }
    match Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => Ok(c),
        Err(_) => {
            let c = Connection::open(db)?;
            c.pragma_update(None, "query_only", "ON")?;
            Ok(c)
        }
    }
}

/// The stored rank of a reader handle, refusing at 0: a file without the
/// devai schema is not an initialised home, and the observer never seeds
/// one. Any non-zero rank — behind or ahead of this build — is readable.
fn read_initialised_rank(conn: &Connection, db: &Path) -> StoreResult<i64> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == 0 {
        return Err(StoreError::NotFound(
            "home store schema".into(),
            format!(
                "{} carries no devai schema — it is not an initialised home",
                db.display()
            ),
        ));
    }
    Ok(version)
}

/// Generic ladder executor (REQ-004, registry-in-data): walk the manifest
/// rungs from the stored version up to SCHEMA_VERSION. Which rung runs
/// next, what SQL it ships and which handler applies it are READ FROM THE
/// MANIFEST DATA — adding a schema version appends a
/// `migrations/manifest.json` record and never grows a branch here.
fn run_ladder(conn: &mut Connection, version: i64) -> StoreResult<()> {
    // ONE `BEGIN IMMEDIATE … COMMIT` per rung: the DDL, the backfill
    // UPDATEs, the repair events and the `user_version` bump land atomically
    // together — a crash can never leave a half-repaired group (REQ-003
    // acceptance "сбой посреди ранга"). Rungs are additionally idempotent,
    // so a database found in a hypothetical partial form (index present,
    // version behind) is carried to completion instead of aborted.
    let rungs = schema_rungs();
    let mut from = version;
    while from < SCHEMA_VERSION {
        let step = rungs
            .iter()
            .find(|s| s.from == from)
            .ok_or(StoreError::SchemaVersion {
                found: from,
                expected: SCHEMA_VERSION,
            })?;
        let apply = applier_for(step)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        apply(&tx, step)?;
        tx.pragma_update(None, "user_version", step.to)?;
        tx.commit()?;
        from = step.to;
    }
    Ok(())
}

/// The behavior behind a manifest tag. Fail loud when the catalog names a
/// handler this build does not know — never silently skip a promised rung.
fn applier_for(step: &SchemaRung) -> StoreResult<RungApply> {
    RUNG_APPLIERS
        .iter()
        .find(|(name, _)| *name == step.apply)
        .map(|(_, f)| *f)
        .ok_or_else(|| {
            StoreError::Corrupt(format!(
                "manifest rung `{}` names unknown apply handler `{}` — refusing to run a migration it cannot describe",
                step.id, step.apply
            ))
        })
}

impl Store {
    pub fn open(root: &Path) -> StoreResult<Store> {
        std::fs::create_dir_all(root)?;
        let blobs = root.join("blobs");
        std::fs::create_dir_all(&blobs)?;
        let mut conn = Connection::open(root.join(DB_FILE_NAME))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            // Fresh home: the current schema is created whole; no rungs run.
            conn.execute_batch(include_str!("../schema.sql"))?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if version > SCHEMA_VERSION {
            return Err(StoreError::SchemaVersion {
                found: version,
                expected: SCHEMA_VERSION,
            });
        } else {
            run_ladder(&mut conn, version)?;
        }
        Ok(Store {
            conn,
            root: root.to_path_buf(),
            blobs,
            task_human_name: std::cell::OnceCell::new(),
        })
    }

    /// Observation open (REQ-HOME-09): read an EXISTING home without ever
    /// mutating it. This is the one entry that never creates directories,
    /// never seeds, never runs the migration ladder and never bumps
    /// `user_version`: a store of any schema rank — behind or ahead of this
    /// build — is opened as it is, so an observer cannot migrate a foreign
    /// home by the act of opening it. A home newer than the binary is
    /// readable here (its rows still load; the version is reported through
    /// [`Store::schema_version`] and stamped by the caller), a home with no
    /// `devai.db` refuses with `NotFound` — never a silent creation.
    ///
    /// The open is attempted strictly read-only first (no file handle can
    /// write); WAL homes whose wal-index cannot be initialised from a
    /// read-only connection fall back to a read-write handle pinned with
    /// `PRAGMA query_only=ON` — statements still cannot mutate rows,
    /// tables or the schema version. The single-writer guard of a live
    /// engine inside the home is untouched (REQ-HOME-06): this handle is a
    /// WAL reader next to it, best-effort snapshot semantics.
    pub fn open_read_only(root: &Path) -> StoreResult<Store> {
        let db = root.join(DB_FILE_NAME);
        let conn = reader_conn(&db)?;
        read_initialised_rank(&conn, &db)?;
        Ok(Store {
            conn,
            root: root.to_path_buf(),
            blobs: root.join("blobs"),
            task_human_name: std::cell::OnceCell::new(),
        })
    }

    /// The stored schema rank of this handle (`PRAGMA user_version`). Read
    /// as data, never compared-and-repaired here: the OBSERVER stamps the
    /// verdict into its own record (REQ-HOME-09: version + path as cause).
    pub fn schema_version(&self) -> StoreResult<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    // ------------------------------------------------------------------
    // projects
    // ------------------------------------------------------------------

    pub fn create_project(&self, p: &Project) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO projects(id,name,description,repos,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![p.id.to_string(), p.name, p.description, json_string(&p.repos), rows::ts(&p.created_at), rows::ts(&p.updated_at)],
        )?;
        Ok(())
    }

    pub fn list_projects(&self) -> StoreResult<Vec<Project>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,name,description,repos,created_at,updated_at FROM projects ORDER BY name",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::project_from_row(r)?);
        }
        Ok(out)
    }

    pub fn get_project(&self, id_or_name: &str) -> StoreResult<Option<Project>> {
        rows::unwrap_row(
            self.conn
                .query_row(
                    "SELECT id,name,description,repos,created_at,updated_at FROM projects WHERE id = ?1 OR name = ?1",
                    params![id_or_name],
                    |r| rows::r2sql(rows::project_from_row(r)),
                )
                .optional(),
        )
    }

    // ------------------------------------------------------------------
    // tasks
    // ------------------------------------------------------------------

    pub fn create_task(&self, t: &Task) -> StoreResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        Self::insert_task_row(&tx, t)?;
        tx.commit()?;
        Ok(())
    }

    /// The task row + its `task.created` event, as ONE statement pair a
    /// caller can fold into its own transaction (shared by `create_task`
    /// and the atomic compound below — the insert SQL stays in one place).
    fn insert_task_row(tx: &Transaction<'_>, t: &Task) -> StoreResult<()> {
        tx.execute(
            "INSERT INTO tasks(id,project_id,title,business_description,human_name,state,return_state,requirements_seq,lock_version,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                t.id.to_string(),
                t.project_id.map(|i| i.to_string()),
                t.title,
                t.business_description,
                t.human_name,
                t.state.as_str(),
                t.return_state.map(|s| s.as_str()),
                t.requirements_seq,
                t.lock_version as i64,
                rows::ts(&t.created_at),
                rows::ts(&t.updated_at),
            ],
        )?;
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                t.id.to_string(),
                event_kind::TASK_CREATED,
                "user",
                json_string(&serde_json::json!({"title": t.title, "state": t.state.as_str()})),
                rows::ts(&t.created_at),
            ],
        )?;
        Ok(())
    }

    /// One ACTIVE dependency edge row and its `task.dependency.linked`
    /// event, inserted into the CALLER's transaction — the single INSERT
    /// path shared by the atomic compound (`create_task_with_dependencies`)
    /// and the retroactive `task depend` (`link_task_dependency`). R1 QA
    /// (mt/dry): the two paths duplicated these statements byte-for-byte;
    /// drift here would let them store different facts while both claiming
    /// "an edge was linked", so the insertion lives on exactly one line
    /// per statement for the whole crate.
    fn insert_dependency_edge(tx: &Transaction<'_>, e: &TaskDependency) -> StoreResult<()> {
        tx.execute(
            "INSERT INTO task_dependencies(id,task_id,depends_on_id,kind,status,actor,created_at) VALUES(?1,?2,?3,?4,'ACTIVE',?5,?6)",
            params![e.id.to_string(), e.task_id.to_string(), e.depends_on_id.to_string(), e.kind.as_str(), e.actor, rows::ts(&e.created_at)],
        )?;
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                e.task_id.to_string(),
                event_kind::TASK_DEPENDENCY_LINKED,
                e.actor,
                json_string(&serde_json::json!({
                    (json_field::DEPENDENCY_ID): e.id.to_string(),
                    (json_field::DEPENDS_ON_ID): e.depends_on_id.to_string(),
                    (json_field::DEPENDENCY_KIND): e.kind.as_str(),
                })),
                rows::ts(&e.created_at),
            ],
        )?;
        Ok(())
    }

    /// REQ-API-01: the ATOMIC compound creation — the task row and event,
    /// every dependency edge and its `task.dependency.linked` event, and
    /// the creation-key registration (REQ-API-02) land in ONE transaction.
    /// Any failure (constraint violation, IO error, a second registration
    /// of the same key) rolls the ENTIRE effect back: a caller can never
    /// observe an orphan task or a partially linked one. Endpoints arrive
    /// ALREADY RESOLVED — ref resolution is `resolve_task`'s job and the
    /// API does it before calling here, so a refused Ambiguous/NotFound
    /// leaves this method (and the store) untouched.
    ///
    /// The task is brand-new, so the shape checks of `link_task_dependency`
    /// reduce: it has no edge rows (stored duplicates impossible) and no
    /// incoming edges (a cycle through it is un-closeable) — only WITHIN
    /// this request can two identical refs collide, rejected with the exact
    /// error family the retroactive `task depend` path produces.
    pub fn create_task_with_dependencies(
        &self,
        t: &Task,
        deps: &[(&Task, DependencyKind)],
        actor: &str,
        now: Timestamp,
        idempotency_key: Option<&str>,
    ) -> StoreResult<Vec<TaskDependency>> {
        let mut edges: Vec<TaskDependency> = Vec::new();
        let mut seen: Vec<(Id, DependencyKind)> = Vec::new();
        for (dep, kind) in deps {
            if t.id == dep.id {
                return Err(StoreError::InvalidDependency(format!(
                    "task `{}` cannot depend on itself",
                    t.id
                )));
            }
            if seen.contains(&(dep.id, *kind)) {
                return Err(StoreError::InvalidDependency(format!(
                    "dependency `{}` —{}→ `{}` already exists",
                    t.id,
                    kind.as_str(),
                    dep.id
                )));
            }
            seen.push((dep.id, *kind));
            edges.push(TaskDependency {
                id: new_id(),
                task_id: t.id,
                task_state: t.state,
                task_display: t.display_sign(),
                task_title: t.title.clone(),
                depends_on_id: dep.id,
                depends_on_state: dep.state,
                depends_on_display: dep.display_sign(),
                depends_on_title: dep.title.clone(),
                kind: *kind,
                actor: actor.to_string(),
                created_at: now,
            });
        }
        let tx = self.conn.unchecked_transaction()?;
        Self::insert_task_row(&tx, t)?;
        for e in &edges {
            Self::insert_dependency_edge(&tx, e)?;
        }
        if let Some(key) = idempotency_key {
            tx.execute(
                "INSERT INTO task_creation_keys(idempotency_key,task_id,created_at) VALUES(?1,?2,?3)",
                params![key, t.id.to_string(), rows::ts(&now)],
            )?;
        }
        tx.commit()?;
        Ok(edges)
    }

    /// REQ-API-02 (ratified OQ-1): the task a committed idempotency key
    /// already registered, if any. The registry row is written inside the
    /// creation transaction, so it is the PROOF of a successful create:
    /// a lost-response retry asks here instead of minting a duplicate.
    pub fn task_by_creation_key(&self, key: &str) -> StoreResult<Option<Task>> {
        Ok(rows::unwrap_row(
            self.conn
                .query_row(
                    "SELECT t.id,t.project_id,t.title,t.business_description,t.state,t.return_state,t.requirements_seq,t.lock_version,t.created_at,t.updated_at,t.human_name
                     FROM task_creation_keys k JOIN tasks t ON t.id = k.task_id
                     WHERE k.idempotency_key = ?1",
                    params![key],
                    |r| rows::r2sql(rows::task_from_row(r)),
                )
                .optional(),
        )?)
    }

    /// Does this store carry `column` on `table`? Asked per HANDLE: the
    /// answer is a property of the stored schema rank, not of the query, and
    /// a read must not re-derive it every time (REQ-HOME-09).
    fn has_column(&self, table: &str, column: &str) -> StoreResult<bool> {
        let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let name: String = r.get(1)?;
            if name == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The task projection this store can answer: a home OBSERVED by this
    /// build may be behind it (v8 has no `tasks.human_name`), and a merged
    /// view of such a home must READ its rows, not fail on the missing
    /// column (REQ-HOME-08 AC3 / REQ-HOME-09, R1 quality return). The
    /// absent column is projected as a typed NULL in the same position, so
    /// every task row mapper stays one shape. Probe cached per handle.
    fn task_projection(&self) -> StoreResult<&'static str> {
        let present = match self.task_human_name.get() {
            Some(v) => *v,
            None => {
                let v = self.has_column("tasks", "human_name")?;
                let _ = self.task_human_name.set(v);
                v
            }
        };
        Ok(if present {
            "id,project_id,title,business_description,state,return_state,requirements_seq,lock_version,created_at,updated_at,human_name"
        } else {
            "id,project_id,title,business_description,state,return_state,requirements_seq,lock_version,created_at,updated_at,NULL AS human_name"
        })
    }

    pub fn get_task(&self, id: &str) -> StoreResult<Task> {
        rows::unwrap_row(
            self.conn
                .query_row(
                    &format!(
                        "SELECT {} FROM tasks WHERE id = ?1",
                        self.task_projection()?
                    ),
                    params![id],
                    |r| rows::r2sql(rows::task_from_row(r)),
                )
                .optional(),
        )?
        .ok_or_else(|| StoreError::NotFound("task".into(), id.to_string()))
    }

    /// Resolves a task by a unique id prefix (human-friendly CLI usage).
    pub fn resolve_task(&self, prefix: &str) -> StoreResult<Task> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM tasks WHERE id = ?1 OR id LIKE ?1 || '%' LIMIT 2",
            self.task_projection()?
        ))?;
        let mut rows = stmt.query(params![prefix.to_lowercase()])?;
        let mut found: Vec<Task> = Vec::new();
        while let Some(r) = rows.next()? {
            found.push(rows::task_from_row(r)?);
        }
        match found.len() {
            0 => Err(StoreError::NotFound("task".into(), prefix.to_string())),
            // invariant: found.len()==1 here — the match arm above guarantees
            // exactly one element, pop() cannot return None.
            1 => Ok(found.pop().unwrap()),
            _ => Err(StoreError::Ambiguous(format!(
                "task id prefix `{prefix}` matches multiple tasks"
            ))),
        }
    }

    pub fn list_tasks(&self) -> StoreResult<Vec<Task>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM tasks ORDER BY updated_at DESC",
            self.task_projection()?
        ))?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::task_from_row(r)?);
        }
        Ok(out)
    }

    /// Applies a validated workflow mutation transactionally with an
    /// optimistic concurrency check against `base_lock`.
    pub fn commit(&self, base_lock: u64, m: &Mutation) -> StoreResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        let affected = tx.execute(
            "UPDATE tasks SET state=?1, return_state=?2, requirements_seq=?3, lock_version=?4, updated_at=?5, human_name=?6
             WHERE id=?7 AND lock_version=?8",
            params![
                m.task.state.as_str(),
                m.task.return_state.map(|s| s.as_str()),
                m.task.requirements_seq,
                m.task.lock_version as i64,
                rows::ts(&m.task.updated_at),
                // REQ-TN-01: the human signboard travels WITH the mutation it
                // belongs to — a requirements publication stamps the name and
                // advances the state in ONE transaction; every other
                // transition carries the stored value back unchanged (the
                // task it clones came out of this same read plane).
                m.task.human_name,
                m.task.id.to_string(),
                base_lock as i64,
            ],
        )?;
        if affected == 0 {
            drop(tx);
            return Err(StoreError::Conflict {
                what: format!("task {}", m.task.id),
                expected: base_lock,
                found: self.get_task(&m.task.id.to_string())?.lock_version,
            });
        }
        for (id, status) in &m.artifact_status {
            tx.execute(
                "UPDATE artifacts SET status=?1 WHERE id=?2",
                params![status.as_str(), id.to_string()],
            )?;
        }
        for ev in &m.events {
            tx.execute(
                "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    new_id().to_string(),
                    m.task.id.to_string(),
                    ev.kind,
                    ev.actor,
                    ev.payload.to_string(),
                    rows::ts(&Utc::now()),
                ],
            )?;
        }
        // A mutation that PARKS the task on a blocking question carries the
        // question row itself: it lands in this SAME transaction, so a held
        // task is never left without an answerable question (REQ-RG-02: the
        // empty-rework gate is loud only if the operator can act on it).
        for q in &m.questions {
            tx.execute(
                "INSERT INTO questions(id,task_id,execution_id,text,context,options,blocking,status,answer,answered_by,answered_at,created_at)
                 VALUES(?1,?2,NULL,?3,?4,?5,?6,'OPEN',NULL,NULL,NULL,?7)",
                params![
                    q.id.to_string(),
                    m.task.id.to_string(),
                    q.text,
                    q.context,
                    json_string(&q.options),
                    q.blocking as i64,
                    rows::ts(&Utc::now()),
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// REQ-BIND-02 (C5 D1): assign/replace the OWNING project of an
    /// existing task — the write half of the `task.project.set` operation.
    /// Optimistic concurrency exactly like `commit`: the UPDATE is keyed on
    /// the caller's loaded `lock_version` and bumps it; a stale base yields
    /// `Conflict` with the version actually found. The row change and its
    /// append-only `task.project.set` event land in ONE transaction — the
    /// binding and its provenance are never half-written (the
    /// `wf_events_no_delete`/`wf_events_no_update` triggers stay untouched:
    /// this INSERTS, never rewrites). Workflow state is NOT involved: the
    /// task keeps its state, seq and edges.
    pub fn set_task_project(
        &self,
        t: &Task,
        project_id: Id,
        actor: &str,
        now: Timestamp,
    ) -> StoreResult<Task> {
        let next_lock = t.lock_version + 1;
        let tx = self.conn.unchecked_transaction()?;
        let affected = tx.execute(
            "UPDATE tasks SET project_id=?1, lock_version=?2, updated_at=?3
             WHERE id=?4 AND lock_version=?5",
            params![
                project_id.to_string(),
                next_lock as i64,
                rows::ts(&now),
                t.id.to_string(),
                t.lock_version as i64,
            ],
        )?;
        if affected == 0 {
            drop(tx);
            return Err(StoreError::Conflict {
                what: format!("task {}", t.id),
                expected: t.lock_version,
                found: self.get_task(&t.id.to_string())?.lock_version,
            });
        }
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                t.id.to_string(),
                event_kind::TASK_PROJECT_SET,
                actor,
                json_string(&serde_json::json!({
                    (json_field::PROJECT_ID): project_id.to_string(),
                    (json_field::PREVIOUS_PROJECT_ID): t.project_id.map(|p| p.to_string()),
                })),
                rows::ts(&now),
            ],
        )?;
        tx.commit()?;
        Ok(Task {
            project_id: Some(project_id),
            lock_version: next_lock,
            updated_at: now,
            ..t.clone()
        })
    }

    /// REQ-TN (ratified OQ-TN-1): the HUMAN write path of the signboard —
    /// `task.rename` sets or replaces the human name directly, no rework
    /// cycle, no third role. Mirrors `set_task_project` exactly: optimistic
    /// lock, one `task.human_name.set` event in the SAME transaction as the
    /// UPDATE, one effective change per call (a NO-OP re-apply is the API's
    /// concern and writes nothing). Window validation belongs to the API
    /// gate (the config-driven policy), not to this half; the store keeps
    /// the string it was given verbatim. A publication-side stamp never
    /// travels here: it rides the workflow mutation through `commit`
    /// (REQ-TN-02), so the two writers share no code path by design.
    pub fn set_task_human_name(
        &self,
        t: &Task,
        human_name: Option<&str>,
        actor: &str,
        now: Timestamp,
    ) -> StoreResult<Task> {
        let next_lock = t.lock_version + 1;
        let tx = self.conn.unchecked_transaction()?;
        let affected = tx.execute(
            "UPDATE tasks SET human_name=?1, lock_version=?2, updated_at=?3
             WHERE id=?4 AND lock_version=?5",
            params![
                human_name,
                next_lock as i64,
                rows::ts(&now),
                t.id.to_string(),
                t.lock_version as i64,
            ],
        )?;
        if affected == 0 {
            drop(tx);
            return Err(StoreError::Conflict {
                what: format!("task {}", t.id),
                expected: t.lock_version,
                found: self.get_task(&t.id.to_string())?.lock_version,
            });
        }
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                t.id.to_string(),
                event_kind::TASK_HUMAN_NAME_SET,
                actor,
                json_string(&serde_json::json!({
                    (json_field::HUMAN_NAME): human_name,
                })),
                rows::ts(&now),
            ],
        )?;
        tx.commit()?;
        Ok(Task {
            human_name: human_name.map(|s| s.to_string()),
            lock_version: next_lock,
            updated_at: now,
            ..t.clone()
        })
    }

    // ------------------------------------------------------------------
    // task dependency graph (REQ-01/02/07)
    // ------------------------------------------------------------------

    /// Shared projection of one ACTIVE stored edge with the JOINed
    /// states/titles of both endpoint tasks. The states are a read-time
    /// projection, never denormalised onto the edge — one query yields
    /// dependencies AND task states (REQ-04 AC1). Callers add their own
    /// `WHERE d.status='ACTIVE' AND ...`: SUPERSEDED rows are history, they
    /// leave every live read (ratified OQ7) but stay in the table forever.
    const DEPENDENCY_EDGE_SELECT: &'static str = "SELECT d.id, d.task_id, t.state, t.title, d.depends_on_id, p.state, p.title, d.kind, d.actor, d.created_at, t.human_name, t.business_description, p.human_name, p.business_description \
        FROM task_dependencies d JOIN tasks t ON t.id = d.task_id JOIN tasks p ON p.id = d.depends_on_id";

    /// Is there already an ACTIVE path `from →*→ to` over edges of `kind`?
    /// Used by `link_task_dependency` to reject cycles WITHIN one kind
    /// (ratified OQ4: the parent graph and the blocks graph are each a DAG)
    /// while a MIXED cycle — the standard backfill/gate case, child `blocks`
    /// its parent while the parent `parent`-links the child — stays legal.
    /// A one-row SQL constraint cannot express transitive reachability, so
    /// the walk is code (the documented data/code boundary of the ladder).
    fn kind_path_exists(&self, from: Id, to: Id, kind: DependencyKind) -> StoreResult<bool> {
        let mut frontier = vec![from.to_string()];
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        seen.insert(from.to_string());
        let target = to.to_string();
        while let Some(node) = frontier.pop() {
            let mut stmt = self
                .conn
                .prepare("SELECT depends_on_id FROM task_dependencies WHERE task_id=?1 AND kind=?2 AND status='ACTIVE'")?;
            let mut rows = stmt.query(params![node, kind.as_str()])?;
            while let Some(r) = rows.next()? {
                let next: String = r.get(0)?;
                if next == target {
                    return Ok(true);
                }
                if seen.insert(next.clone()) {
                    frontier.push(next);
                }
            }
        }
        Ok(false)
    }

    /// Registers a typed dependency edge `task_id → depends_on_id` (REQ-07).
    /// Integrity is LOUD AT WRITE, never silently ignored on read:
    /// self-dependency, a duplicate ACTIVE edge and — ratified OQ4 — an edge
    /// closing a cycle inside one kind are rejected here (the self-check and
    /// the single-ACTIVE partial index pin the shapes again in v4 DDL); a
    /// dangling endpoint surfaces as `NotFound`. The edge stores its author
    /// and creation time (the `decisions.actor` precedent) and lands in
    /// `workflow_events` as `task.dependency.linked` inside ONE transaction
    /// — the relation and its provenance are never half-written.
    pub fn link_task_dependency(
        &self,
        task_id: Id,
        depends_on_id: Id,
        kind: DependencyKind,
        actor: &str,
        now: Timestamp,
    ) -> StoreResult<TaskDependency> {
        if task_id == depends_on_id {
            return Err(StoreError::InvalidDependency(format!(
                "task `{task_id}` cannot depend on itself"
            )));
        }
        // Loud reference check before the INSERT (REQ-07 AC2): the dependent
        // and the depended-on task must both exist as platform data.
        let dependent = self.get_task(&task_id.to_string())?;
        let depends_on = self.get_task(&depends_on_id.to_string())?;
        let duplicate: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM task_dependencies WHERE task_id=?1 AND depends_on_id=?2 AND kind=?3 AND status='ACTIVE'",
                params![task_id.to_string(), depends_on_id.to_string(), kind.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if duplicate.is_some() {
            return Err(StoreError::InvalidDependency(format!(
                "dependency `{task_id}` —{}→ `{depends_on_id}` already exists",
                kind.as_str()
            )));
        }
        // Ratified OQ4: adding `task_id → depends_on_id` would close a cycle
        // in this kind iff an ACTIVE same-kind path already runs the OTHER
        // way, `depends_on_id →*→ task_id`. Refused loudly, never stored.
        if self.kind_path_exists(depends_on_id, task_id, kind)? {
            return Err(StoreError::InvalidDependency(format!(
                "dependency `{task_id}` —{}→ `{depends_on_id}` would close a {}-cycle; each kind must stay a DAG (mixed parent/blocks cycles are legal)",
                kind.as_str(),
                kind.as_str()
            )));
        }
        let edge = TaskDependency {
            id: new_id(),
            task_id,
            task_state: dependent.state,
            task_display: dependent.display_sign(),
            task_title: dependent.title,
            depends_on_id,
            depends_on_state: depends_on.state,
            depends_on_display: depends_on.display_sign(),
            depends_on_title: depends_on.title,
            kind,
            actor: actor.to_string(),
            created_at: now,
        };
        let tx = self.conn.unchecked_transaction()?;
        Self::insert_dependency_edge(&tx, &edge)?;
        tx.commit()?;
        Ok(edge)
    }

    /// Outgoing ACTIVE edges of one task — what it depends on (the
    /// resume/accept gate input; REQ-03). A task with no edge rows (every
    /// task created before v4, or one with nothing registered) returns an
    /// EMPTY list, never an error (REQ-08 AC2). SUPERSEDED edges are
    /// history and never appear here (ratified OQ7).
    pub fn task_dependencies(&self, task_id: Id) -> StoreResult<Vec<TaskDependency>> {
        let mut stmt = self.conn.prepare(&format!(
            "{} WHERE d.status='ACTIVE' AND d.task_id=?1 ORDER BY d.created_at, d.id",
            Self::DEPENDENCY_EDGE_SELECT
        ))?;
        let mut rows = stmt.query(params![task_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::dependency_from_row(r)?);
        }
        Ok(out)
    }

    /// Incoming ACTIVE edges of one task — which tasks depend on it. This
    /// is the SAME query shape as the outgoing view: an "epic" needs no
    /// separate entity at all — ratified OQ8 names it by shape over this
    /// ordinary dependency relation (`epic_heads` below, REQ-06 AC2).
    pub fn incoming_dependencies(&self, task_id: Id) -> StoreResult<Vec<TaskDependency>> {
        let mut stmt = self.conn.prepare(&format!(
            "{} WHERE d.status='ACTIVE' AND d.depends_on_id=?1 ORDER BY d.created_at, d.id",
            Self::DEPENDENCY_EDGE_SELECT
        ))?;
        let mut rows = stmt.query(params![task_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::dependency_from_row(r)?);
        }
        Ok(out)
    }

    /// The approved removal semantics of an edge (RATIFIED DECISION OQ7):
    /// "delete" = mark the row SUPERSEDED, never erase. The same-status
    /// pattern the artifacts table set in v3 — history is COMPLETE: the row
    /// keeps its author and created_at and gains who and when superseded
    /// it. Afterwards the live graph no longer sees the edge (a gate it
    /// held opens), a duplicate re-registration may create a fresh ACTIVE
    /// row (the partial unique index covers ACTIVE only), and both rows
    /// stay readable in the table forever. One transaction writes the flip
    /// and the `task.dependency.unlinked` event.
    pub fn supersede_task_dependency(
        &self,
        task_id: Id,
        depends_on_id: Id,
        kind: DependencyKind,
        actor: &str,
        now: Timestamp,
    ) -> StoreResult<TaskDependency> {
        let found: Option<TaskDependency> = self
            .conn
            .query_row(
                &format!(
                    "{} WHERE d.status='ACTIVE' AND d.task_id=?1 AND d.depends_on_id=?2 AND d.kind=?3",
                    Self::DEPENDENCY_EDGE_SELECT
                ),
                params![task_id.to_string(), depends_on_id.to_string(), kind.as_str()],
                |r| rows::r2sql(rows::dependency_from_row(r)),
            )
            .optional()?;
        let Some(edge) = found else {
            return Err(StoreError::InvalidDependency(format!(
                "no active `{task_id}` —{}→ `{depends_on_id}` dependency to supersede",
                kind.as_str()
            )));
        };
        let edge_id = edge.id.to_string();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE task_dependencies SET status='SUPERSEDED', superseded_actor=?2, superseded_at=?3 WHERE id=?1 AND status='ACTIVE'",
            params![edge_id, actor, rows::ts(&now)],
        )?;
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                task_id.to_string(),
                event_kind::TASK_DEPENDENCY_UNLINKED,
                actor,
                json_string(&serde_json::json!({
                    (json_field::DEPENDENCY_ID): edge_id,
                    (json_field::DEPENDS_ON_ID): depends_on_id.to_string(),
                    (json_field::DEPENDENCY_KIND): kind.as_str(),
                })),
                rows::ts(&now),
            ],
        )?;
        tx.commit()?;
        // The caller gets back the edge exactly as it was while ACTIVE.
        Ok(edge)
    }

    /// The epic registry as a VIEW over the stored graph (REQ-06, RATIFIED
    /// DECISION OQ8): epic = vertex with NO outgoing ACTIVE `parent` edge
    /// and AT LEAST ONE incoming one. No epic table, no epic column — the
    /// SAME `task_dependencies` relation and its ACTIVE rows answer, in one
    /// query (REQ-06 AC1/AC2). Blocking edges do not make a vertex an epic:
    /// only the genealogy kind defines the derivation tree.
    pub fn epic_heads(&self) -> StoreResult<Vec<EpicVertex>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.project_id, t.title, t.business_description, t.state, t.return_state,
                    t.requirements_seq, t.lock_version, t.created_at, t.updated_at, t.human_name,
                    (SELECT count(*) FROM task_dependencies c
                      WHERE c.depends_on_id = t.id AND c.kind = 'parent' AND c.status = 'ACTIVE')
             FROM tasks t
             WHERE NOT EXISTS (SELECT 1 FROM task_dependencies p
                                 WHERE p.task_id = t.id AND p.kind = 'parent' AND p.status = 'ACTIVE')
               AND EXISTS (SELECT 1 FROM task_dependencies c
                            WHERE c.depends_on_id = t.id AND c.kind = 'parent' AND c.status = 'ACTIVE')
             ORDER BY t.updated_at DESC, t.id",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(EpicVertex {
                task: rows::task_from_row(r)?,
                // column 10 is the task's own `human_name` (v9), 11 the
                // derived-children count (the task projection moved to 11 cols).
                derived_children: r.get::<_, i64>(11)? as usize,
            });
        }
        Ok(out)
    }

    /// The WHOLE graph in ONE query: every task node with its state and its
    /// outgoing ACTIVE typed edges (both endpoint states included). This is
    /// what makes the plan computable from platform data alone — a roadmap
    /// consumer takes this set, the ready set is `open_blockers` per node
    /// (REQ-04 AC1/AC2) — and the planning-level "what is happening"
    /// view (REQ-05 AC1). Tasks without edges appear with an empty set;
    /// SUPERSEDED edges join in only as nothing (live graph = ACTIVE).
    pub fn dependency_graph(&self) -> StoreResult<Vec<DependencyNode>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.project_id, t.title, t.business_description, t.state, t.return_state,
                    t.requirements_seq, t.lock_version, t.created_at, t.updated_at, t.human_name,
                    d.id, d.kind, d.depends_on_id, d.actor, d.created_at, p.state, p.title,
                    p.human_name, p.business_description
             FROM tasks t
             LEFT JOIN task_dependencies d ON d.task_id = t.id AND d.status = 'ACTIVE'
             LEFT JOIN tasks p ON p.id = d.depends_on_id
             ORDER BY t.updated_at DESC, t.id, d.created_at, d.id",
        )?;
        let mut rows = stmt.query([])?;
        let mut out: Vec<DependencyNode> = Vec::new();
        while let Some(r) = rows.next()? {
            let task = rows::task_from_row(r)?;
            let edge = rows::dependency_tail_from_row(r, &task)?;
            match out.last_mut() {
                // Rows of one task are consecutive (the ORDER BY): append
                // the edge to the node instead of emitting the task twice.
                Some(node) if node.task.id == task.id => {
                    if let Some(e) = edge {
                        node.dependencies.push(e);
                    }
                }
                _ => out.push(DependencyNode {
                    task,
                    dependencies: edge.into_iter().collect(),
                }),
            }
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // artifacts & content-addressed store
    // ------------------------------------------------------------------

    /// Publishes a new artifact version. The supersession invariant
    /// (REQ-001): every previous CURRENT row of the same (task_id, kind)
    /// group flips to SUPERSEDED inside the SAME transaction as the INSERT
    /// and the events — a crash can never leave zero or two CURRENT rows.
    /// Append-only discipline: only the status column is UPDATEd and event
    /// rows are INSERTed; nothing is deleted, no blob is rewritten.
    pub fn publish_artifact(&self, draft: ArtifactDraft) -> StoreResult<Artifact> {
        let sha = hex_encode(&Sha256::digest(&draft.content));
        let path = self.blob_path(&sha);
        // invariant: why unreachable — `path` is `blob_path(sha)` =
        // `<root>/blobs/<xx>/<yy>/<sha>`: at least three components are
        // appended to a rooted dir, so `parent()` is always `Some` by the
        // shape of the blob layout itself.
        std::fs::create_dir_all(path.parent().unwrap())?;
        if !path.exists() {
            // write-then-rename keeps the store crash-safe
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, &draft.content)?;
            std::fs::rename(&tmp, &path)?;
        }
        let artifact = Artifact {
            id: new_id(),
            task_id: draft.task_id,
            kind: draft.kind,
            name: draft.name,
            // seq: MAX(seq)+1 inside the group, computed inside the publish
            // transaction (was: outside it — the TOCTOU that made equal-seq
            // CURRENT siblings reachable on v2 databases).
            seq: 0,
            category: draft.category,
            status: ArtifactStatus::Current,
            sha256: sha.clone(),
            size: draft.content.len() as u64,
            media_type: draft.media_type,
            contract: draft.contract,
            created_by: draft.created_by,
            execution_id: draft.execution_id,
            depends_on: draft.depends_on,
            created_at: Utc::now(),
        };
        let tx = self.conn.unchecked_transaction()?;
        let seq: u32 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq),0)+1 FROM artifacts WHERE task_id=?1 AND kind=?2",
                params![artifact.task_id.to_string(), artifact.kind.as_str()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(1);
        let artifact = Artifact { seq, ..artifact };
        // REQ-001: supersede first, then INSERT — the partial UNIQUE index
        // (one CURRENT per (task_id, kind), schema v3) is never transiently
        // violated, and a failed INSERT rolls the flip back with it.
        let superseded: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM artifacts WHERE task_id=?1 AND kind=?2 AND status='CURRENT' ORDER BY seq",
            )?;
            let mut rows = stmt.query(params![
                artifact.task_id.to_string(),
                artifact.kind.as_str()
            ])?;
            let mut ids = Vec::new();
            while let Some(r) = rows.next()? {
                ids.push(r.get(0)?);
            }
            ids
        };
        if !superseded.is_empty() {
            tx.execute(
                "UPDATE artifacts SET status='SUPERSEDED' WHERE task_id=?1 AND kind=?2 AND status='CURRENT'",
                params![artifact.task_id.to_string(), artifact.kind.as_str()],
            )?;
        }
        tx.execute(
            "INSERT INTO artifacts(id,task_id,kind,name,seq,category,status,sha256,size,media_type,contract_name,contract_version,created_by,execution_id,depends_on,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![
                artifact.id.to_string(),
                artifact.task_id.to_string(),
                artifact.kind.as_str(),
                artifact.name,
                artifact.seq,
                artifact.category.as_str(),
                artifact.status.as_str(),
                artifact.sha256,
                artifact.size as i64,
                artifact.media_type,
                artifact.contract.as_ref().map(|c| c.name.clone()),
                artifact.contract.as_ref().map(|c| c.version),
                artifact.created_by,
                artifact.execution_id.map(|i| i.to_string()),
                json_string(&artifact.depends_on),
                rows::ts(&artifact.created_at),
            ],
        )?;
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                artifact.task_id.to_string(),
                event_kind::ARTIFACT_PUBLISHED,
                artifact.created_by,
                json_string(&serde_json::json!({
                    (json_field::ARTIFACT_ID): artifact.id.to_string(),
                    (json_field::KIND): artifact.kind.as_str(),
                    (json_field::NAME): artifact.name,
                    (json_field::SEQ): artifact.seq,
                    (json_field::SHA256): artifact.sha256,
                    (json_field::SIZE): artifact.size,
                    (json_field::CONTRACT): artifact.contract.as_ref().map(|c| c.to_string()),
                })),
                rows::ts(&artifact.created_at),
            ],
        )?;
        // REQ-002: exactly one batched supersession event per publication
        // that superseded something — and nothing at all for the first
        // version of a kind (zero SUPERSEDED rows, zero events).
        if !superseded.is_empty() {
            tx.execute(
                "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    new_id().to_string(),
                    artifact.task_id.to_string(),
                    event_kind::ARTIFACT_SUPERSEDED,
                    artifact.created_by,
                    json_string(&serde_json::json!({
                        (json_field::ARTIFACT_ID): artifact.id.to_string(),
                        (json_field::SUPERSEDED): superseded,
                        (json_field::KIND): artifact.kind.as_str(),
                        (json_field::TASK_ID): artifact.task_id.to_string(),
                    })),
                    rows::ts(&artifact.created_at),
                ],
            )?;
        }
        tx.commit()?;
        Ok(artifact)
    }

    pub fn read_blob(&self, sha: &str) -> StoreResult<Vec<u8>> {
        Ok(std::fs::read(self.blob_path(sha))?)
    }

    pub fn blob_path(&self, sha: &str) -> PathBuf {
        self.blobs.join(&sha[0..2]).join(&sha[2..4]).join(sha)
    }

    /// The ONE read of an artifact's stored body: every consumer that needs
    /// the bytes a stored artifact row names goes through here, so the error
    /// policy is single and cannot drift site by site. The policy:
    ///
    /// * the blob file is ABSENT → `NotFound("artifact blob", <sha>)` — the
    ///   row outlived its content; whether that reads as a missing body (the
    ///   gate's typed report-body outcome) or as an export failure (the
    ///   bundle writer) is the caller's presentation, not the store's;
    /// * the file EXISTS but cannot be read (permissions, a directory in its
    ///   place, a broken filesystem) → `Corrupt` naming the artifact — that
    ///   is a broken store, and folding it into «missing» would call it empty.
    ///
    /// Content-addressed, so nothing here re-writes or re-derives the bytes.
    pub fn read_artifact_body(&self, a: &Artifact) -> StoreResult<Vec<u8>> {
        match std::fs::read(self.blob_path(&a.sha256)) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(StoreError::NotFound(
                "artifact blob".into(),
                a.sha256.clone(),
            )),
            Err(e) => Err(StoreError::Corrupt(format!(
                "artifact {} body {} is stored but unreadable: {e}",
                a.id, a.sha256
            ))),
        }
    }

    pub fn artifacts(&self, task_id: Id) -> StoreResult<Vec<Artifact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,kind,name,seq,category,status,sha256,size,media_type,contract_name,contract_version,created_by,execution_id,depends_on,created_at
             FROM artifacts WHERE task_id=?1 ORDER BY created_at",
        )?;
        let mut rows = stmt.query(params![task_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::artifact_from_row(r)?);
        }
        Ok(out)
    }

    /// Every artifact row of the home in ONE snapshot (same ordering as
    /// `artifacts`). Companion of `questions_all` for the human-wait batch
    /// read: one gate presentation per APPROVAL/QA row must not cost an
    /// `artifact.list` call each.
    pub fn artifacts_all(&self) -> StoreResult<Vec<Artifact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,kind,name,seq,category,status,sha256,size,media_type,contract_name,contract_version,created_by,execution_id,depends_on,created_at
             FROM artifacts ORDER BY created_at",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::artifact_from_row(r)?);
        }
        Ok(out)
    }

    /// Resolve an artifact by full id or unique prefix (OQ-3, ratified:
    /// EVERY entity resolver keeps ONE rule — a unique prefix resolves, a
    /// prefix matching several rows fails LOUDLY, the `resolve_task` /
    /// `get_execution` discipline). The old `LIMIT 1` silently picked
    /// whichever colliding row SQLite met first; UUIDv7 display-prefixes
    /// collide for same-instant rows, and guessing between two artifacts
    /// reads the WRONG one.
    pub fn get_artifact(&self, id: &str) -> StoreResult<Artifact> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,kind,name,seq,category,status,sha256,size,media_type,contract_name,contract_version,created_by,execution_id,depends_on,created_at
             FROM artifacts WHERE id = ?1 OR id LIKE ?1 || '%' LIMIT 2",
        )?;
        let mut rows = stmt.query(params![id.to_lowercase()])?;
        let mut found: Vec<Artifact> = Vec::new();
        while let Some(r) = rows.next()? {
            found.push(rows::artifact_from_row(r)?);
        }
        match found.len() {
            0 => Err(StoreError::NotFound("artifact".into(), id.to_string())),
            // invariant: found.len()==1 here — the match arm above guarantees
            // exactly one element, pop() cannot return None.
            1 => Ok(found.pop().unwrap()),
            _ => Err(StoreError::Ambiguous(format!(
                "artifact id prefix `{id}` matches multiple artifacts"
            ))),
        }
    }

    // ------------------------------------------------------------------
    // events
    // ------------------------------------------------------------------

    pub fn events(&self, task_id: Id, limit: Option<usize>) -> StoreResult<Vec<WorkflowEvent>> {
        let sql = "SELECT id,task_id,kind,actor,payload,created_at FROM workflow_events WHERE task_id=?1 ORDER BY seq";
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(params![task_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            if let Some(l) = limit {
                if out.len() >= l {
                    break;
                }
            }
            out.push(rows::event_from_row(r)?);
        }
        Ok(out)
    }

    pub fn all_events_for_task(&self, task_id: Id) -> StoreResult<Vec<WorkflowEvent>> {
        self.events(task_id, None)
    }

    // ------------------------------------------------------------------
    // roles / agents / contracts
    // ------------------------------------------------------------------

    pub fn put_role(&self, r: &RoleDefinition) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO role_definitions(id,slug,version,name,purpose,instructions,tools,input_contract,output_contract,done_criteria,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                r.id.to_string(),
                r.slug,
                r.version,
                r.name,
                r.purpose,
                r.instructions,
                json_string(&r.tools),
                r.input_contract.as_ref().map(|c| c.to_string()),
                r.output_contract.as_ref().map(|c| c.to_string()),
                r.done_criteria,
                rows::ts(&r.created_at),
            ],
        )?;
        Ok(())
    }

    pub fn list_roles(&self) -> StoreResult<Vec<RoleDefinition>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,slug,version,name,purpose,instructions,tools,input_contract,output_contract,done_criteria,created_at
             FROM role_definitions ORDER BY slug, version",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::role_from_row(r)?);
        }
        Ok(out)
    }

    pub fn get_role(&self, slug: &str) -> StoreResult<Option<RoleDefinition>> {
        // `slug` may be `name@version` or plain name (latest wins)
        let (name, version) = match slug.split_once('@') {
            Some((n, v)) => (
                n.to_string(),
                Some(
                    v.parse::<u32>()
                        .map_err(|e| StoreError::Corrupt(format!("bad role version: {e}")))?,
                ),
            ),
            None => (slug.to_string(), None),
        };
        // Both branches bind two params; the plain-name case passes NULL so the
        // `?2 IS NULL` clause matches every version and ORDER BY picks the latest.
        let sql = match version {
            Some(_) => {
                "SELECT id,slug,version,name,purpose,instructions,tools,input_contract,output_contract,done_criteria,created_at FROM role_definitions WHERE slug=?1 AND version=?2"
            }
            None => {
                "SELECT id,slug,version,name,purpose,instructions,tools,input_contract,output_contract,done_criteria,created_at FROM role_definitions WHERE slug=?1 AND (?2 IS NULL OR version=?2) ORDER BY version DESC LIMIT 1"
            }
        };
        let v = version.map(|x| x.to_string());
        rows::unwrap_row(
            self.conn
                .query_row(
                    sql,
                    rusqlite::params_from_iter([Some(name), v].iter()),
                    |r| rows::r2sql(rows::role_from_row(r)),
                )
                .optional(),
        )
    }

    pub fn put_agent(&self, a: &AgentInstance) -> StoreResult<()> {
        // The `active` marker is NOT writable here (REQ-MS-02/04): a plain
        // `agent add`/upsert never marks or unmarks the bundle in force —
        // the marker moves only through the switch transaction. A new record
        // is therefore born unmarked, and an update leaves the marker as is.
        self.conn.execute(
            "INSERT INTO agent_instances(id,name,provider,model,runtime,reasoning,created_at,active) VALUES(?1,?2,?3,?4,?5,?6,?7,0)
             ON CONFLICT(name) DO UPDATE SET provider=?3, model=?4, runtime=?5, reasoning=?6",
            params![a.id.to_string(), a.name, a.provider, a.model, a.runtime, a.reasoning, rows::ts(&a.created_at)],
        )?;
        Ok(())
    }

    pub fn list_agents(&self) -> StoreResult<Vec<AgentInstance>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,name,provider,model,runtime,reasoning,created_at,active FROM agent_instances ORDER BY name",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::agent_from_row(r)?);
        }
        Ok(out)
    }

    /// The ONE marked agent record — «the bundle in force» (REQ-MS-02/04).
    /// `None` means «связка не выбрана»; the caller never substitutes a
    /// built-in fallback for it. More than one marked row is impossible by
    /// the partial unique index (`agent_instances_single_active`), so this is
    /// a plain lookup, not a conflict resolution.
    pub fn active_agent(&self) -> StoreResult<Option<AgentInstance>> {
        Self::marked_agent(&self.conn)
    }

    /// THE switch (REQ-MS-04): upsert the agent record by NAME (its id and
    /// `created_at` survive — ASM-8), move the marker onto it, append the
    /// `model.switched` event and VERIFY before commit. One transaction: a
    /// failure at any step rolls the whole thing back, so a half-switched
    /// platform is unrepresentable. Returns the record that was in force
    /// BEFORE (the event's «from»).
    ///
    /// Probe/verdict policy is NOT here: the caller refuses before calling.
    /// This method only ever performs an approved move.
    pub fn apply_bundle_switch(&self, s: &BundleSwitch) -> StoreResult<Option<AgentInstance>> {
        let tx = self.conn.unchecked_transaction()?;
        let previous = Self::marked_agent(&tx)?;
        Self::upsert_switch_record(&tx, s)?;
        Self::move_marker(&tx, &s.agent)?;
        Self::verify_switch_record(&tx, s)?;
        Self::append_switch_event(&tx, s, &Self::switch_event_payload(s, previous.as_ref()))?;
        tx.commit()?;
        Ok(previous)
    }

    /// Apply a new IDENTITY to the record of an edited bundle that is in
    /// force (task 01a1be5c, REQ-MC-03 AC3/AC4) AND append the edit's own
    /// fact in ONE transaction (REQ-MC-06 AC5): the record, the marker and
    /// the `kind` history row land together or not at all — there is no
    /// window in which the record moved without its history row. The marker
    /// stays on the record; a changed agent NAME creates/updates the new
    /// record and marks it, leaving the old one intact and unmarked (runs in
    /// flight keep their own stamped identity).
    pub fn apply_bundle_identity_with_fact(
        &self,
        s: &BundleSwitch,
        kind: &str,
    ) -> StoreResult<Option<AgentInstance>> {
        let tx = self.conn.unchecked_transaction()?;
        let previous = Self::marked_agent(&tx)?;
        Self::upsert_switch_record(&tx, s)?;
        Self::move_marker(&tx, &s.agent)?;
        Self::verify_switch_record(&tx, s)?;
        Self::append_event(&tx, kind, &s.actor, &s.payload, s.now)?;
        tx.commit()?;
        Ok(previous)
    }

    /// The ONE marked agent record, read on an open connection/transaction
    /// (`active_agent` serves the same predicate without one).
    fn marked_agent(conn: &Connection) -> StoreResult<Option<AgentInstance>> {
        rows::unwrap_row(
            conn.query_row(
                "SELECT id,name,provider,model,runtime,reasoning,created_at,active FROM agent_instances WHERE active=1",
                [],
                |r| rows::r2sql(rows::agent_from_row(r)),
            )
            .optional(),
        )
    }

    /// Upsert the record by NAME: id/`created_at` are preserved (the ON
    /// CONFLICT set deliberately omits both), so the record keeps its
    /// identity. Column order is the STORE's: `provider` holds the DRIVER,
    /// `model` the model, `runtime` the LLM-provider (REQ-MS-01).
    fn upsert_switch_record(tx: &Transaction<'_>, s: &BundleSwitch) -> StoreResult<()> {
        tx.execute(
            "INSERT INTO agent_instances(id,name,provider,model,runtime,reasoning,created_at,active) VALUES(?1,?2,?3,?4,?5,?6,?7,0)
             ON CONFLICT(name) DO UPDATE SET provider=?3, model=?4, runtime=?5, reasoning=?6",
            params![
                new_id().to_string(),
                s.agent,
                s.driver,
                s.model,
                s.llm_provider,
                s.reasoning,
                rows::ts(&s.now)
            ],
        )?;
        Ok(())
    }

    /// Move the marker: clear every other row first (the partial unique index
    /// would reject two marked rows), then mark the target.
    fn move_marker(tx: &Transaction<'_>, agent: &str) -> StoreResult<()> {
        tx.execute(
            "UPDATE agent_instances SET active=0 WHERE active=1 AND name<>?1",
            params![agent],
        )?;
        tx.execute(
            "UPDATE agent_instances SET active=1 WHERE name=?1",
            params![agent],
        )?;
        Ok(())
    }

    /// VERIFY inside the transaction: the record now carries the bundle
    /// identity and it is the marked one. A divergence is a corrupt state,
    /// never a silent half-switch.
    fn verify_switch_record(tx: &Transaction<'_>, s: &BundleSwitch) -> StoreResult<()> {
        let (name, driver, llm_provider, model, reasoning, active): (
            String,
            String,
            String,
            String,
            String,
            i64,
        ) = tx.query_row(
            "SELECT name,provider,runtime,model,reasoning,active FROM agent_instances WHERE name=?1",
            params![s.agent],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )?;
        if name != s.agent
            || driver != s.driver
            || llm_provider != s.llm_provider
            || model != s.model
            || reasoning != s.reasoning
            || active != 1
        {
            return Err(StoreError::Corrupt(format!(
                "model switch verification failed for agent `{}`: record reads (driver={driver}, llm_provider={llm_provider}, model={model}, reasoning={reasoning}, active={active}), expected ({}, {}, {}, {}, active=1) — rolled back",
                s.agent, s.driver, s.llm_provider, s.model, s.reasoning
            )));
        }
        Ok(())
    }

    /// The event payload: the caller's keys plus the two facts only the store
    /// knows — the «from» snapshot and whether the agent NAME changed (a
    /// first selection has no name to have changed, REQ-MS-04 AC4).
    fn switch_event_payload(
        s: &BundleSwitch,
        previous: Option<&AgentInstance>,
    ) -> serde_json::Value {
        let mut payload = s.payload.clone();
        if let serde_json::Value::Object(map) = &mut payload {
            map.insert(
                json_field::FROM.into(),
                match previous {
                    Some(p) => serde_json::json!({
                        (json_field::DRIVER): p.provider,
                        (json_field::LLM_PROVIDER): p.runtime,
                        (json_field::MODEL): p.model,
                        (json_field::REASONING): p.reasoning,
                    }),
                    None => serde_json::Value::Null,
                },
            );
            map.insert(
                json_field::AGENT_CHANGED.into(),
                serde_json::Value::Bool(
                    previous
                        .as_ref()
                        .map(|p| p.name != s.agent)
                        .unwrap_or(false),
                ),
            );
        }
        payload
    }

    /// Append the immutable `model.switched` fact — no task, home level.
    fn append_switch_event(
        tx: &Transaction<'_>,
        s: &BundleSwitch,
        payload: &serde_json::Value,
    ) -> StoreResult<()> {
        Self::append_event(tx, event_kind::MODEL_SWITCHED, &s.actor, payload, s.now)
    }

    /// Append ONE home-level fact inside the caller's transaction: the kind
    /// is the caller's (the switch and the bundle edit write different
    /// facts, both in the same transaction as the move they describe).
    fn append_event(
        tx: &Transaction<'_>,
        kind: &str,
        actor: &str,
        payload: &serde_json::Value,
        at: Timestamp,
    ) -> StoreResult<()> {
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,NULL,?2,?3,?4,?5)",
            params![new_id().to_string(), kind, actor, payload.to_string(), rows::ts(&at)],
        )?;
        Ok(())
    }

    /// Home-level history of the bundle surface (REQ-MS-05, REQ-MC-07):
    /// every probe, every switch attempt and every bundle create/edit/delete
    /// attempt, oldest first. These events carry NO task — the read is not a
    /// task read and is served by one operation for both the console and the
    /// window.
    pub fn model_events(&self) -> StoreResult<Vec<WorkflowEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,kind,actor,payload,created_at FROM workflow_events WHERE kind IN (?1,?2,?3,?4,?5) ORDER BY seq",
        )?;
        let mut rows = stmt.query(params![
            event_kind::MODEL_PROBE,
            event_kind::MODEL_SWITCHED,
            event_kind::MODEL_BUNDLE_CREATED,
            event_kind::MODEL_BUNDLE_UPDATED,
            event_kind::MODEL_BUNDLE_DELETED
        ])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::event_from_row(r)?);
        }
        Ok(out)
    }

    /// The newest FINISHED execution row of one identity stamp — the second
    /// evidence input of the status field (REQ-MS-06). RUNNING rows never
    /// determine status. Ties broken by the larger row id, exactly as the
    /// snapshot documents.
    pub fn latest_finished_execution_for_stamp(
        &self,
        driver: &str,
        llm_provider: &str,
        model: &str,
        reasoning: &str,
    ) -> StoreResult<Option<RoleExecution>> {
        rows::unwrap_row(
            self.conn
                .query_row(
                    "SELECT id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision,attempt,parent_execution,failure_class,owner_pid,owner_host,owner_process_start,driver,llm_provider,model,reasoning
                     FROM role_executions
                     WHERE status <> 'RUNNING' AND driver=?1 AND llm_provider=?2 AND model=?3 AND reasoning=?4
                     ORDER BY finished_at DESC, id DESC LIMIT 1",
                    params![driver, llm_provider, model, reasoning],
                    |r| rows::r2sql(rows::execution_from_row(r)),
                )
                .optional(),
        )
    }

    pub fn put_contract(&self, name: &str, version: u32, schema: &str) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO contracts(name,version,schema,created_at) VALUES(?1,?2,?3,?4)
             ON CONFLICT(name,version) DO UPDATE SET schema=?3",
            params![name, version, schema, rows::ts(&Utc::now())],
        )?;
        Ok(())
    }

    pub fn get_contract(&self, name: &str, version: u32) -> StoreResult<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT schema FROM contracts WHERE name=?1 AND version=?2",
                params![name, version],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn all_contracts(&self) -> StoreResult<Vec<(String, u32, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name,version,schema FROM contracts ORDER BY name, version")?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push((r.get(0)?, r.get(1)?, r.get(2)?));
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // evaluation profile history (insert-only)
    // ------------------------------------------------------------------

    /// Snapshot one evaluation profile definition. INSERT-only: the first
    /// stored text for `(name, version)` IS the definition of that version —
    /// later in-place edits bump the version and land as a new row, so every
    /// profile identity recorded in an old execution stays resolvable.
    pub fn put_evaluation_profile(
        &self,
        name: &str,
        version: u32,
        project: &str,
        body: &str,
        created_at: &Timestamp,
    ) -> StoreResult<bool> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO evaluation_profiles(name,version,project,body,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![name, version, project, body, rows::ts(created_at)],
        )?;
        Ok(inserted > 0)
    }

    /// Resolve a recorded `name@version` profile identity to its stored body.
    pub fn get_evaluation_profile(
        &self,
        name: &str,
        version: u32,
    ) -> StoreResult<Option<(String, String)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT project, body FROM evaluation_profiles WHERE name=?1 AND version=?2",
                params![name, version],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn list_evaluation_profiles(&self) -> StoreResult<Vec<(String, u32, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, version, project FROM evaluation_profiles ORDER BY name, version",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push((r.get(0)?, r.get(1)?, r.get(2)?));
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // executions
    // ------------------------------------------------------------------

    pub fn start_execution(&self, e: &RoleExecution) -> StoreResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        // The workspace revision is stored as the JSON object the engine
        // computed at execution start. NULL is reserved for rows written
        // before provenance existed and for manual rows without one.
        let revision = e.workspace_revision.as_ref().map(|r| json_string(r));
        let insert = tx.execute(
            "INSERT INTO role_executions(id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision,attempt,parent_execution,failure_class,owner_pid,owner_host,owner_process_start,driver,llm_provider,model,reasoning)
             VALUES(?1,?2,?3,?4,?5,?6,?7,NULL,NULL,?8,NULL,?9,?10,?11,NULL,?12,?13,?14,?15,?16,?17,?18)",
            params![
                e.id.to_string(),
                e.task_id.to_string(),
                e.role_slug,
                e.role_version,
                e.agent_name,
                e.status.as_str(),
                json_string(&e.inputs),
                rows::ts(&e.started_at),
                revision,
                e.attempt as i64,
                e.parent_execution.map(|p| p.to_string()),
                // Owner identity (REQ-SW-03): what the platform-side caller
                // recorded at construction. Direct store users may leave
                // NULL — such rows read as «owner unknown», never «dead».
                e.owner_pid,
                e.owner_host,
                e.owner_process_start,
                // The run STAMP (REQ-MS-05): the identity the platform copied
                // off the agent record at start. NULL stays NULL — a row
                // without a stamp reads «связка не записана».
                e.bundle.as_ref().map(|b| b.driver.clone()),
                e.bundle.as_ref().map(|b| b.llm_provider.clone()),
                e.bundle.as_ref().map(|b| b.model.clone()),
                e.bundle.as_ref().map(|b| b.reasoning.clone()),
            ],
        );
        if let Err(err) = insert {
            // REQ-SW-01/02: the partial unique index rejected a second
            // RUNNING row for this task. Map the opaque violation to the
            // named variant citing the blocking row (the raw SQLite message
            // carries no execution id — the acceptance demands one). A
            // unique violation NOT naming task_id (the row id) or a missing
            // RUNNING row means some other constraint fired: propagate.
            if is_single_writer_violation(&err) {
                let blocker: Option<(String, String, Option<i64>, Option<String>)> = tx
                    .query_row(
                        "SELECT id, started_at, owner_pid, owner_host FROM role_executions WHERE task_id=?1 AND status='RUNNING' ORDER BY started_at, id LIMIT 1",
                        params![e.task_id.to_string()],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .optional()?;
                if let Some((id, started_at, owner_pid, owner_host)) = blocker {
                    return Err(StoreError::SingleWriterBusy {
                        task_id: e.task_id.to_string(),
                        existing_id: id,
                        started_at,
                        owner_pid,
                        owner_host,
                    });
                }
            }
            return Err(err.into());
        }
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                e.task_id.to_string(),
                event_kind::EXECUTION_STARTED,
                format!("role:{}", e.role_slug),
                json_string(&serde_json::json!({
                    "execution_id": e.id.to_string(),
                    (json_field::ROLE): e.role_slug,
                    "role_version": e.role_version,
                    "agent": e.agent_name,
                    "workspace_revision": e.workspace_revision,
                })),
                rows::ts(&e.started_at),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn finish_execution(
        &self,
        e: &RoleExecution,
        status: ExecutionStatus,
        summary: Option<&str>,
        error: Option<&str>,
        now: Timestamp,
    ) -> StoreResult<()> {
        self.finish_execution_classified(e, status, summary, error, None, now)
    }

    /// `finish_execution` plus the `failure_class` write (REQ-FG-01): WHY a
    /// Failed row failed — `response_form` / `provider` / `platform` (the
    /// `failure_class` vocabulary in core) — so form-vs-content stays
    /// answerable as data, never by parsing the error text.
    pub fn finish_execution_classified(
        &self,
        e: &RoleExecution,
        status: ExecutionStatus,
        summary: Option<&str>,
        error: Option<&str>,
        failure_class: Option<&str>,
        now: Timestamp,
    ) -> StoreResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE role_executions SET status=?1, summary=?2, error=?3, finished_at=?4, failure_class=COALESCE(?6, failure_class) WHERE id=?5",
            params![status.as_str(), summary, error, rows::ts(&now), e.id.to_string(), failure_class],
        )?;
        let kind = match status {
            ExecutionStatus::Completed => event_kind::EXECUTION_FINISHED,
            ExecutionStatus::Failed => event_kind::EXECUTION_FAILED,
            _ => event_kind::EXECUTION_CANCELLED,
        };
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                e.task_id.to_string(),
                kind,
                format!("role:{}", e.role_slug),
                json_string(&serde_json::json!({
                    "execution_id": e.id.to_string(),
                    (json_field::ROLE): e.role_slug,
                    (json_field::STATUS): status.as_str(),
                    (json_field::SUMMARY): summary,
                    "error": error,
                })),
                rows::ts(&now),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Agent failures (task 01a1b60c, REQ-AF-01/05/06)
    // ------------------------------------------------------------------

    /// One actual agent failure. The platform calls this at execution
    /// completion (never the launching loop, REQ-AF-01 AC6). Inside ONE
    /// transaction:
    ///   * the (agent, class) anti-spam WINDOW decides the event: its first
    ///     failure appends exactly one `agent.failed` event, a repeat inside
    ///     the window appends nothing (REQ-AF-06 AC1/AC2);
    ///   * the (task, agent, class) evidence row grows (`window_count += 1`)
    ///     or is created, so EVERY task keeps its own class/message/tail for
    ///     its progress line even when the window suppressed its event
    ///     (REQ-AF-05 AC3);
    ///   * the (task, agent) SERIES counter grows by one (REQ-AF-05 AC2);
    ///   * the accumulated total is recomputed from window counters, so a
    ///     suppressed repeat still counts toward the parking threshold
    ///     (REQ-AF-06 AC6).
    ///     A restart cannot lose the counters or the window boundary: they
    ///     are rows, not process state.
    pub fn record_agent_failure(
        &self,
        rec: &AgentFailureRecord,
    ) -> StoreResult<AgentFailureRecorded> {
        let tx = self.conn.unchecked_transaction()?;
        let floor = rec.at - chrono::Duration::minutes(rec.window_minutes.max(1));
        // The anti-spam WINDOW is the (agent, class) pair, across tasks
        // (REQ-AF-06 AC1/AC5): a row of ANY task of that pair inside the
        // window means the event is already written and must not repeat.
        let window: Option<(String, String)> = rows::unwrap_row(
            tx.query_row(
                "SELECT id, window_started_at FROM agent_failures \
                 WHERE agent_name=?1 AND class=?2 AND last_at>=?3 \
                 ORDER BY last_at DESC LIMIT 1",
                params![rec.agent_name, rec.class, rows::ts(&floor)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional(),
        )?;
        // THIS task's own evidence row of the pair inside the window — it
        // grows; the row of another task is never overwritten, so every
        // stuck task keeps its own class, message and tail (REQ-AF-05 AC3).
        let own: Option<(String, i64)> = rows::unwrap_row(
            tx.query_row(
                "SELECT id, window_count FROM agent_failures \
                 WHERE agent_name=?1 AND class=?2 AND task_id=?3 AND last_at>=?4 \
                 ORDER BY last_at DESC LIMIT 1",
                params![
                    rec.agent_name,
                    rec.class,
                    rec.task_id.to_string(),
                    rows::ts(&floor)
                ],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional(),
        )?;
        let event_written = window.is_none();
        let (window_count, window_started_at) = match own {
            Some((id, count)) => {
                // A suppressed repeat: the counter grows, the evidence is
                // refreshed to the LATEST attempt (the event keeps the first).
                tx.execute(
                    "UPDATE agent_failures SET window_count=?1, last_at=?2, exit_code=?3, \
                     duration_ms=?4, tail=?5, output_empty=?6, driver_log_tail=?7, \
                     driver_log_note=?8, basis=?9, threshold=?10, message=?11, execution_id=?12 \
                     WHERE id=?13",
                    params![
                        count + 1,
                        rows::ts(&rec.at),
                        rec.exit_code,
                        rec.duration_ms,
                        rec.tail,
                        rec.output_empty as i64,
                        rec.driver_log_tail,
                        rec.driver_log_note,
                        rec.basis,
                        rec.threshold,
                        rec.message,
                        rec.execution_id.map(|e| e.to_string()),
                        id,
                    ],
                )?;
                let started = if event_written {
                    rec.at
                } else {
                    rows::parse_ts(
                        window
                            .as_ref()
                            .map(|w| w.1.clone())
                            .unwrap_or_else(|| rows::ts(&rec.at)),
                    )?
                };
                (count + 1, started)
            }
            None => {
                tx.execute(
                    "INSERT INTO agent_failures(id,task_id,execution_id,agent_name,driver,llm_provider,model,class,message,exit_code,duration_ms,tail,output_empty,driver_log_tail,driver_log_note,basis,window_count,window_started_at,first_at,last_at,threshold,created_at) \
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,1,?17,?17,?17,?18,?17)",
                    params![
                        rec.id.to_string(),
                        rec.task_id.to_string(),
                        rec.execution_id.map(|e| e.to_string()),
                        rec.agent_name,
                        rec.driver,
                        rec.llm_provider,
                        rec.model,
                        rec.class,
                        rec.message,
                        rec.exit_code,
                        rec.duration_ms,
                        rec.tail,
                        rec.output_empty as i64,
                        rec.driver_log_tail,
                        rec.driver_log_note,
                        rec.basis,
                        rows::ts(&rec.at),
                        rec.threshold,
                    ],
                )?;
                let started = if event_written {
                    rec.at
                } else {
                    rows::parse_ts(
                        window
                            .as_ref()
                            .map(|w| w.1.clone())
                            .unwrap_or_else(|| rows::ts(&rec.at)),
                    )?
                };
                (1, started)
            }
        };

        // The SERIES counter: only a success resets it (REQ-AF-05 AC2/AC6).
        let series: Option<(i64, String)> = rows::unwrap_row(
            tx.query_row(
                "SELECT consecutive_count, first_at FROM agent_failure_series \
                 WHERE task_id=?1 AND agent_name=?2",
                params![rec.task_id.to_string(), rec.agent_name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional(),
        )?;
        let (series_count, first_at) = match series {
            Some((count, first)) => {
                tx.execute(
                    "UPDATE agent_failure_series SET consecutive_count=?1, last_at=?2, last_class=?3 \
                     WHERE task_id=?4 AND agent_name=?5",
                    params![
                        count + 1,
                        rows::ts(&rec.at),
                        rec.class,
                        rec.task_id.to_string(),
                        rec.agent_name,
                    ],
                )?;
                (count + 1, rows::parse_ts(first)?)
            }
            None => {
                tx.execute(
                    "INSERT INTO agent_failure_series(task_id,agent_name,consecutive_count,first_at,last_at,last_class,parked_at) \
                     VALUES(?1,?2,1,?3,?3,?4,NULL)",
                    params![
                        rec.task_id.to_string(),
                        rec.agent_name,
                        rows::ts(&rec.at),
                        rec.class,
                    ],
                )?;
                (1, rec.at)
            }
        };
        let total_failures: i64 = tx.query_row(
            "SELECT COALESCE(SUM(window_count),0) FROM agent_failures WHERE agent_name=?1",
            params![rec.agent_name],
            |r| r.get(0),
        )?;

        if event_written {
            let payload = serde_json::json!({
                (json_field::TASK_ID): rec.task_id.to_string(),
                "execution_id": rec.execution_id.map(|e| e.to_string()),
                (json_field::AGENT): rec.agent_name,
                (json_field::DRIVER): rec.driver,
                (json_field::LLM_PROVIDER): rec.llm_provider,
                (json_field::MODEL): rec.model,
                (json_field::CLASS): rec.class,
                (json_field::MESSAGE): rec.message,
                // The three-class split stays intact: the detailed class is a
                // SUB-class of provider, never a fourth stored value on the
                // execution row (REQ-AF-03 AC1/AC2).
                (json_field::FAILURE_CLASS): failure_class::PROVIDER,
                (json_field::EXIT_CODE): rec.exit_code,
                (json_field::DURATION_MS): rec.duration_ms,
                (json_field::TAIL): rec.tail,
                (json_field::OUTPUT_EMPTY): rec.output_empty,
                (json_field::DRIVER_LOG_TAIL): rec.driver_log_tail,
                (json_field::DRIVER_LOG_NOTE): rec.driver_log_note,
                (json_field::BASIS): rec.basis,
                (json_field::WINDOW_COUNT): window_count,
                (json_field::WINDOW_STARTED_AT): rows::ts(&window_started_at),
                (json_field::SERIES_COUNT): series_count,
                (json_field::FAILURES_TOTAL): total_failures,
                (json_field::FIRST_AT): rows::ts(&first_at),
                (json_field::LAST_AT): rows::ts(&rec.at),
                (json_field::THRESHOLD): rec.threshold,
                (json_field::AT): rows::ts(&rec.at),
            });
            tx.execute(
                "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) \
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    new_id().to_string(),
                    rec.task_id.to_string(),
                    event_kind::AGENT_FAILED,
                    format!("agent:{}", rec.agent_name),
                    payload.to_string(),
                    rows::ts(&rec.at),
                ],
            )?;
        }
        tx.commit()?;
        Ok(AgentFailureRecorded {
            event_written,
            window_count,
            window_started_at,
            series_count,
            first_at,
            last_at: rec.at,
            total_failures,
        })
    }

    /// The newest stored failure of a task (any class) — the evidence a
    /// progress row / wait row presents.
    pub fn latest_agent_failure(&self, task_id: Id) -> StoreResult<Option<AgentFailureRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {AGENT_FAILURE_COLUMNS} FROM agent_failures WHERE task_id=?1 \
             ORDER BY last_at DESC LIMIT 1"
        ))?;
        let mut rows = stmt.query(params![task_id.to_string()])?;
        match rows.next()? {
            Some(r) => Ok(Some(rows::agent_failure_from_row(r)?)),
            None => Ok(None),
        }
    }

    /// The newest stored failure of an agent (any task) — the status line's
    /// class evidence (REQ-AF-07).
    pub fn latest_agent_failure_for_agent(
        &self,
        agent: &str,
    ) -> StoreResult<Option<AgentFailureRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {AGENT_FAILURE_COLUMNS} FROM agent_failures WHERE agent_name=?1 \
             ORDER BY last_at DESC LIMIT 1"
        ))?;
        let mut rows = stmt.query(params![agent])?;
        match rows.next()? {
            Some(r) => Ok(Some(rows::agent_failure_from_row(r)?)),
            None => Ok(None),
        }
    }

    pub fn agent_failure_series(
        &self,
        task_id: Id,
        agent: &str,
    ) -> StoreResult<Option<AgentFailureSeries>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id,agent_name,consecutive_count,first_at,last_at,last_class,parked_at \
             FROM agent_failure_series WHERE task_id=?1 AND agent_name=?2",
        )?;
        let mut rows = stmt.query(params![task_id.to_string(), agent])?;
        match rows.next()? {
            Some(r) => Ok(Some(rows::agent_failure_series_from_row(r)?)),
            None => Ok(None),
        }
    }

    /// The longest CURRENT series of an agent across tasks — what the status
    /// line shows as «N отказов подряд» (REQ-AF-07).
    pub fn agent_failure_series_for_agent(
        &self,
        agent: &str,
    ) -> StoreResult<Option<AgentFailureSeries>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id,agent_name,consecutive_count,first_at,last_at,last_class,parked_at \
             FROM agent_failure_series WHERE agent_name=?1 \
             ORDER BY consecutive_count DESC, last_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![agent])?;
        match rows.next()? {
            Some(r) => Ok(Some(rows::agent_failure_series_from_row(r)?)),
            None => Ok(None),
        }
    }

    /// All actual failures of an agent (window counters summed — suppressed
    /// repeats included, REQ-AF-06 AC6).
    pub fn agent_failure_total(&self, agent: &str) -> StoreResult<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(window_count),0) FROM agent_failures WHERE agent_name=?1",
            params![agent],
            |r| r.get(0),
        )?)
    }

    /// The latest anti-spam window row of one (agent, class) — the status
    /// surface shows its count/time.
    pub fn agent_failure_window(
        &self,
        agent: &str,
        class: &str,
    ) -> StoreResult<Option<AgentFailureRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {AGENT_FAILURE_COLUMNS} FROM agent_failures \
             WHERE agent_name=?1 AND class=?2 ORDER BY last_at DESC LIMIT 1"
        ))?;
        let mut rows = stmt.query(params![agent, class])?;
        match rows.next()? {
            Some(r) => Ok(Some(rows::agent_failure_from_row(r)?)),
            None => Ok(None),
        }
    }

    /// A success of the agent ANYWHERE resets its series (REQ-AF-05 AC2/AC6:
    /// only success resets — answering the parked question does not).
    pub fn reset_agent_failure_series(&self, agent: &str) -> StoreResult<()> {
        self.conn.execute(
            "DELETE FROM agent_failure_series WHERE agent_name=?1",
            params![agent],
        )?;
        Ok(())
    }

    /// Record that the threshold already parked this task on this series, so
    /// the same series never parks twice.
    pub fn mark_agent_failure_parked(
        &self,
        task_id: Id,
        agent: &str,
        at: Timestamp,
    ) -> StoreResult<()> {
        self.conn.execute(
            "UPDATE agent_failure_series SET parked_at=?1 WHERE task_id=?2 AND agent_name=?3",
            params![rows::ts(&at), task_id.to_string(), agent],
        )?;
        Ok(())
    }

    /// Resolves an execution by full id or short prefix. The prefix lookup
    /// fails LOUDLY on ambiguity (same discipline as `resolve_task`): UUIDv7
    /// leading bytes encode the creation timestamp, so display-shortened ids
    /// collide for rows created in the same instant, and `LIMIT 1` would
    /// silently finish the WRONG execution.
    pub fn get_execution(&self, id: &str) -> StoreResult<RoleExecution> {
        // Merged (rebase onto v6): the branch's loud ambiguity discipline —
        // UUIDv7 display-prefixes collide for same-instant rows, LIMIT 1
        // would finish the WRONG execution — over main's widened row (the
        // form-loop columns attempt/parent_execution/failure_class ride in).
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision,attempt,parent_execution,failure_class,owner_pid,owner_host,owner_process_start,driver,llm_provider,model,reasoning
             FROM role_executions WHERE id=?1 OR id LIKE ?1 || '%' LIMIT 2",
        )?;
        let mut rows = stmt.query(params![id.to_lowercase()])?;
        let mut found: Vec<RoleExecution> = Vec::new();
        while let Some(r) = rows.next()? {
            found.push(rows::execution_from_row(r)?);
        }
        match found.len() {
            0 => Err(StoreError::NotFound("execution".into(), id.to_string())),
            // invariant: found.len()==1 here — the match arm above guarantees
            // exactly one element, pop() cannot return None.
            1 => Ok(found.pop().unwrap()),
            _ => Err(StoreError::Ambiguous(format!(
                "execution id prefix `{id}` matches multiple executions"
            ))),
        }
    }

    pub fn list_executions(&self, task_id: Option<Id>) -> StoreResult<Vec<RoleExecution>> {
        let (sql, args): (&str, Vec<Option<String>>) = match task_id {
            Some(t) => (
                "SELECT id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision,attempt,parent_execution,failure_class,owner_pid,owner_host,owner_process_start,driver,llm_provider,model,reasoning FROM role_executions WHERE task_id=?1 ORDER BY started_at",
                vec![Some(t.to_string())],
            ),
            None => (
                "SELECT id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision,attempt,parent_execution,failure_class,owner_pid,owner_host,owner_process_start,driver,llm_provider,model,reasoning FROM role_executions ORDER BY started_at",
                vec![],
            ),
        };
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(args.iter()))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::execution_from_row(r)?);
        }
        Ok(out)
    }

    /// The RUNNING rows only — the snapshot's «идущие прогоны» list (R4
    /// quality return). Addressed by the `role_executions_status` index
    /// instead of reading the whole execution history and filtering in
    /// memory; the single-writer index allows at most one per task anyway.
    pub fn running_executions(&self) -> StoreResult<Vec<RoleExecution>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision,attempt,parent_execution,failure_class,owner_pid,owner_host,owner_process_start,driver,llm_provider,model,reasoning FROM role_executions WHERE status='RUNNING' ORDER BY started_at",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::execution_from_row(r)?);
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // decisions & questions
    // ------------------------------------------------------------------

    pub fn record_decision(&self, d: &Decision) -> StoreResult<()> {
        let payload = if d.payload.is_null() {
            None
        } else {
            Some(d.payload.to_string())
        };
        self.conn.execute(
            "INSERT INTO decisions(id,task_id,kind,verdict,actor,rationale,refs,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![d.id.to_string(), d.task_id.to_string(), d.kind, d.verdict.as_str(), d.actor, d.rationale, json_string(&d.refs), payload, rows::ts(&d.created_at)],
        )?;
        Ok(())
    }

    pub fn list_decisions(&self, task_id: Id) -> StoreResult<Vec<Decision>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,kind,verdict,actor,rationale,refs,created_at,payload FROM decisions WHERE task_id=?1 ORDER BY created_at",
        )?;
        let mut rows = stmt.query(params![task_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::decision_from_row(r)?);
        }
        Ok(out)
    }

    /// One decision row by full id or unique prefix — the same loud
    /// resolver rule as `resolve_task` (multiple prefix matches are an
    /// error, never a guess). The revocation door (`profile revoke`) needs
    /// to name the decision it takes out of force.
    pub fn get_decision(&self, id: &str) -> StoreResult<Decision> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,kind,verdict,actor,rationale,refs,created_at,payload FROM decisions WHERE id=?1 OR id LIKE ?1 || '%' LIMIT 2",
        )?;
        let mut rows = stmt.query(params![id.to_lowercase()])?;
        let mut found: Vec<Decision> = Vec::new();
        while let Some(r) = rows.next()? {
            found.push(rows::decision_from_row(r)?);
        }
        match found.len() {
            0 => Err(StoreError::NotFound(
                "decision".into(),
                format!("decision `{id}` does not exist"),
            )),
            1 => Ok(found.pop().unwrap()),
            _ => Err(StoreError::Ambiguous(format!(
                "decision id prefix `{id}` matches multiple decisions"
            ))),
        }
    }

    /// Every `profile_ratification` decision row — APPROVED ratifications
    /// AND their revocations — in stored chronology. The ratification's
    /// structured binding (project, file, edition, covered items, revokes)
    /// lives in `Decision::payload`, so the force cross-check reads the
    /// ratified EDITION from the row instead of re-deriving it (task
    /// 01a1bacf, RV2/RV6).
    pub fn profile_ratification_decisions(&self) -> StoreResult<Vec<Decision>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,kind,verdict,actor,rationale,refs,created_at,payload FROM decisions WHERE kind=?1 ORDER BY created_at",
        )?;
        let mut rows = stmt.query(params![decision_kind::PROFILE_RATIFICATION])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::decision_from_row(r)?);
        }
        Ok(out)
    }

    /// The ratification index for profile overlays (REQ-SP-106 layer (b)):
    /// every LIVE decisions row of kind `profile_ratification` with verdict
    /// APPROVED — a row taken out of force by a later revocation is absent —
    /// as decision-id -> refs. The delta loader cross-checks each item's
    /// `ratified_by` against this map AND against the recorded edition.
    pub fn ratified_profile_refs(&self) -> StoreResult<BTreeMap<String, Vec<String>>> {
        Ok(self
            .ratified_profile_decisions()?
            .into_iter()
            .map(|(id, (refs, _actor))| (id, refs))
            .collect())
    }

    /// The ONE read-side index of ratification rows: decision id -> typed
    /// row (payload, actor, verdict, live). `config::ratification_index`
    /// owns the revocation filter and the visible-on-corruption payload
    /// decode; this method is the store-side door to it, so a consumer never
    /// re-implements "which ratification is in force".
    pub fn ratification_index(&self) -> StoreResult<BTreeMap<String, config::RatificationRow>> {
        config::ratification_index(&self.profile_ratification_decisions()?)
    }

    /// The same index as decision id -> (refs, actor), LIVE rows only:
    /// REQ-QH-03/OQ-QH-2 need «чьим решением закреплено» on the human form
    /// of a report, and the author of the decision is a store fact — never
    /// something the presentation layer may invent.
    pub fn ratified_profile_decisions(
        &self,
    ) -> StoreResult<BTreeMap<String, (Vec<String>, String)>> {
        Ok(self
            .ratification_index()?
            .into_iter()
            .filter(|(_, row)| row.live)
            .map(|(id, row)| (id, (row.payload.items, row.actor)))
            .collect())
    }

    pub fn create_question(&self, q: &Question) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO questions(id,task_id,execution_id,text,context,options,blocking,status,answer,answered_by,answered_at,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL,NULL,NULL,?9)",
            params![
                q.id.to_string(),
                q.task_id.to_string(),
                q.execution_id.map(|i| i.to_string()),
                q.text,
                q.context,
                json_string(&q.options),
                q.blocking as i64,
                q.status.as_str(),
                rows::ts(&q.created_at),
            ],
        )?;
        Ok(())
    }

    /// Resolve a question by full id or unique prefix — same SINGLE
    /// resolver rule as `resolve_task`/`get_execution`/`get_artifact`
    /// (OQ-3, ratified): multiple prefix matches fail LOUDLY, the store
    /// never guesses which row the caller meant.
    pub fn get_question(&self, id: &str) -> StoreResult<Question> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,execution_id,text,context,options,blocking,status,answer,answered_by,answered_at,created_at
             FROM questions WHERE id=?1 OR id LIKE ?1 || '%' LIMIT 2",
        )?;
        let mut rows = stmt.query(params![id.to_lowercase()])?;
        let mut found: Vec<Question> = Vec::new();
        while let Some(r) = rows.next()? {
            found.push(rows::question_from_row(r)?);
        }
        match found.len() {
            0 => Err(StoreError::NotFound("question".into(), id.to_string())),
            // invariant: found.len()==1 here — the match arm above guarantees
            // exactly one element, pop() cannot return None.
            1 => Ok(found.pop().unwrap()),
            _ => Err(StoreError::Ambiguous(format!(
                "question id prefix `{id}` matches multiple questions"
            ))),
        }
    }

    pub fn answer_question(
        &self,
        id: Id,
        answer: &str,
        answered_by: &str,
        now: Timestamp,
    ) -> StoreResult<()> {
        self.conn.execute(
            "UPDATE questions SET status='ANSWERED', answer=?1, answered_by=?2, answered_at=?3 WHERE id=?4",
            params![answer, answered_by, rows::ts(&now), id.to_string()],
        )?;
        Ok(())
    }

    pub fn list_questions(&self, task_id: Id) -> StoreResult<Vec<Question>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,execution_id,text,context,options,blocking,status,answer,answered_by,answered_at,created_at FROM questions WHERE task_id=?1 ORDER BY created_at",
        )?;
        let mut rows = stmt.query(params![task_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::question_from_row(r)?);
        }
        Ok(out)
    }

    /// Every question row of the home in ONE snapshot, ordered like
    /// `list_questions` (task-scoped readers group it themselves). Added for
    /// the human-wait batch read (REQ-GATE-01..10): the section must not
    /// issue one `question.list` per task.
    pub fn questions_all(&self) -> StoreResult<Vec<Question>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,execution_id,text,context,options,blocking,status,answer,answered_by,answered_at,created_at FROM questions ORDER BY created_at",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::question_from_row(r)?);
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    pub fn insert_event(
        &self,
        task_id: Option<Id>,
        kind: &str,
        actor: &str,
        payload: serde_json::Value,
    ) -> StoreResult<()> {
        self.insert_event_with(new_id(), task_id, kind, actor, payload, Utc::now())
    }

    /// `insert_event` with a CALLER-CHOSEN id and time — the door the model
    /// surface uses to cite one probe event from the switch event that rests
    /// on it (REQ-MS-05: «ссылка на пробу»), and to keep the probe's own
    /// measurement time. The row is append-only like every event.
    pub fn insert_event_with(
        &self,
        id: Id,
        task_id: Option<Id>,
        kind: &str,
        actor: &str,
        payload: serde_json::Value,
        at: Timestamp,
    ) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![id.to_string(), task_id.map(|i| i.to_string()), kind, actor, payload.to_string(), rows::ts(&at)],
        )?;
        Ok(())
    }

    pub fn store_blob(&self, sha: &str, content: &[u8]) -> StoreResult<()> {
        let computed = hex_encode(&Sha256::digest(content));
        if computed != sha {
            return Err(StoreError::Corrupt(format!(
                "blob checksum mismatch: expected {sha}, content hashes to {computed}"
            )));
        }
        let path = self.blob_path(sha);
        // invariant: why unreachable — `path` is `blob_path(sha)` =
        // `<root>/blobs/<xx>/<yy>/<sha>`: at least three components are
        // appended to a rooted dir, so `parent()` is always `Some` by the
        // shape of the blob layout itself.
        std::fs::create_dir_all(path.parent().unwrap())?;
        if !path.exists() {
            std::fs::write(&path, content)?;
        }
        Ok(())
    }

    pub fn has_task(&self, id: &str) -> StoreResult<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM tasks WHERE id=?1", params![id], |_| Ok(()))
            .optional()?
            .is_some())
    }
}

/// The body of a pure-SQL rung: apply the shipped script verbatim. The
/// executor already wraps it in the rung's transaction.
fn apply_script(tx: &Transaction<'_>, step: &SchemaRung) -> StoreResult<()> {
    tx.execute_batch(step.script)?;
    Ok(())
}

/// True when a failed `role_executions` statement hit the v7 partial unique
/// index: the violation fires on the indexed column pair (`task_id` in the
/// SQLite message), not on the row id. Anything else — FK, NOT NULL, a
/// PRIMARY KEY collision — stays the opaque `Sql` it honestly is.
fn is_single_writer_violation(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(f, msg) => {
            f.code == rusqlite::ErrorCode::ConstraintViolation
                && msg
                    .as_deref()
                    .map_or(true, |m| m.contains("role_executions.task_id"))
        }
        _ => false,
    }
}

/// Rung v4→v5: the three additive `role_executions` columns of the
/// response-form loop. SQLite has no `ADD COLUMN IF NOT EXISTS`, so the
/// shipped script is executed STATEMENT BY STATEMENT, each `ADD COLUMN`
/// skipped when the live schema already carries that column. This is the
/// manifest's documented code/data boundary — idempotence pure SQL cannot
/// express. It matters in both directions: an interrupted earlier run that
/// applied part of the script, and a fully-shaped database whose version
/// marker was pushed back (the supersession tests rewrite history inside a
/// current-shape file) must both re-land at v5 without a duplicate-column
/// abort. An `ADD COLUMN` statement is recognized textually from the script
/// itself — the .sql file stays the single source of the target shape.
fn execution_form_columns(tx: &Transaction<'_>, step: &SchemaRung) -> StoreResult<()> {
    apply_script_with_column_skip(tx, step.script)
}

/// Statement-by-statement script application with `ADD COLUMN` presence
/// checks (SQLite has no `ADD COLUMN IF NOT EXISTS`) — the idempotence pure
/// SQL cannot express, shared by rung 006, rung 007 and rung 009. Everything
/// else (index DDL written `IF NOT EXISTS`) runs verbatim.
///
/// The presence check reads the table NAMED BY THE STATEMENT (rung 006/007
/// touched `role_executions`, rung 009 touches `tasks`): one generic scan,
/// never a per-rung table literal.
fn apply_script_with_column_skip(tx: &Transaction<'_>, script: &'static str) -> StoreResult<()> {
    // `--` commentary (Russian included, and its punctuation may contain
    // `;`) is cut from EVERY line BEFORE splitting on statements — comment
    // text must never reach the executor or the token parser.
    let code: String = script
        .lines()
        .map(|l| match l.find("--") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n");
    for stmt in code.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        if let Some((table, col)) = added_column_of(stmt) {
            let existing: Vec<String> = {
                let mut s = tx.prepare(&format!(
                    "SELECT name FROM pragma_table_info('{}')",
                    table.replace('\'', "''")
                ))?;
                let rows = s.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            if existing.iter().any(|c| c == &col) {
                continue;
            }
        }
        tx.execute(stmt, [])?;
    }
    Ok(())
}

/// The `(table, column)` pair an `ALTER TABLE <table> ADD [COLUMN] <name> …`
/// statement targets, by token scan; `None` for any other statement (those
/// run verbatim — only column-adding statements get the presence check).
fn added_column_of(code: &str) -> Option<(String, String)> {
    fn ident(tok: &str) -> String {
        tok.trim_matches(|c: char| c == '`' || c == '"' || c == '[')
            .trim_end_matches(']')
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    }
    let toks: Vec<&str> = code.split_whitespace().collect();
    let alter = |w: &&str| w.eq_ignore_ascii_case("ALTER");
    let table_kw = |w: &&str| w.eq_ignore_ascii_case("TABLE");
    let add = |w: &&str| w.eq_ignore_ascii_case("ADD");
    let column_kw = |w: &&str| w.eq_ignore_ascii_case("COLUMN");
    // `ALTER TABLE <table> ADD COLUMN <name> …` or the bare `ADD <name>`
    // SQLite form — token positions: [0]=ALTER [1]=TABLE [2]=<table>
    // [3]=ADD [4]=(COLUMN|[<name>]) [5]=(<name> after COLUMN).
    if toks.len() >= 5 && alter(&toks[0]) && table_kw(&toks[1]) && add(&toks[3]) {
        let name_tok = if column_kw(&toks[4]) {
            toks.get(5).copied().unwrap_or("")
        } else {
            toks[4]
        };
        if !name_tok.is_empty() {
            return Some((ident(toks[2]), ident(name_tok)));
        }
    }
    None
}

/// Rung v8→v9 (task 01a1ad1d, REQ-TN-01): one additive nullable column on
/// `tasks`; pure SQL plus the shared presence check for idempotence.
fn additive_human_name_column(tx: &Transaction<'_>, step: &SchemaRung) -> StoreResult<()> {
    apply_script_with_column_skip(tx, step.script)
}

/// Rung v9→v10 (task 01a1bacf, RV1/RV2/RV5): `decisions.payload` — the
/// kind-specific structured binding of a decision row (the profile
/// ratification's `<project, file, edition, items>`). Same additive-column
/// discipline as `009`: a re-run against a home that already carries the
/// column is a no-op, no row is rewritten.
///
/// Shape detection before any mutation, exactly like `decisions_backfill`:
/// a database predating the `decisions` table itself (the shipped v1
/// fixture shape) has no decision rows to bind, and `ALTER TABLE` on an
/// absent table would abort the whole `open()` for a legacy home that is
/// otherwise migratable forward. The rung is inert there — the same
/// absence the pre-v10 ladder already tolerated.
fn additive_decision_payload_column(tx: &Transaction<'_>, step: &SchemaRung) -> StoreResult<()> {
    let present: i64 = tx.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='decisions'",
        [],
        |r| r.get(0),
    )?;
    if present == 0 {
        return Ok(());
    }
    apply_script_with_column_skip(tx, step.script)
}

/// Rung v10→v11 (task 01a1b71f, REQ-MS-02/05): the active-bundle marker on
/// `agent_instances` and the three run-stamp columns on `role_executions`,
/// plus the partial unique index that keeps exactly one marker. Additive
/// only — no row is rewritten; the shared column-presence skip makes a
/// re-run over an already-shaped home a no-op.
fn additive_model_bundle_columns(tx: &Transaction<'_>, step: &SchemaRung) -> StoreResult<()> {
    apply_script_with_column_skip(tx, step.script)
}

/// Rung v11→v12 (task 01a1b71f, the ratified OQ-6 answer): the reasoning
/// level on the agent record and on the run stamp. Additive only — one defaulted column
/// and one nullable column; the shared column-presence skip keeps a re-run
/// over an already-shaped home a no-op.
fn additive_model_reasoning_column(tx: &Transaction<'_>, step: &SchemaRung) -> StoreResult<()> {
    apply_script_with_column_skip(tx, step.script)
}

/// Rung v2→v3 (REQ-003): the one-time repair of pre-existing multi-CURRENT
/// sets (the 2026-11-02 precedent: three CURRENT QUALITY_REPORT rows at
/// once). Runs INSIDE the rung transaction opened by `open()`, so
/// detection, the script's UPDATE + index DDL and the repair events commit
/// atomically with the `user_version` bump: a crash never leaves a
/// half-repaired group. The rows to flip are identified BEFORE the UPDATE
/// so each repaired (task_id, kind) group gets exactly one
/// `task.artifact.superseded` event citing its winner and every flipped id
/// — the repair lands in the event log, not silently. A group that was
/// already honest produces zero UPDATEs and zero events.
fn supersession_backfill(tx: &Transaction<'_>, step: &SchemaRung) -> StoreResult<()> {
    let losers: Vec<(String, String, String)> = {
        let mut stmt = tx.prepare(&format!(
            "SELECT task_id, kind, id FROM artifacts WHERE {MULTI_CURRENT_LOSER_PREDICATE} ORDER BY task_id, kind, seq"
        ))?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push((r.get(0)?, r.get(1)?, r.get(2)?));
        }
        out
    };
    tx.execute_batch(step.script)?;
    let mut by_group: Vec<((String, String), Vec<String>)> = Vec::new();
    for (task_id, kind, id) in losers {
        match by_group
            .iter_mut()
            .find(|(g, _)| *g == (task_id.clone(), kind.clone()))
        {
            Some((_, ids)) => ids.push(id),
            None => by_group.push(((task_id, kind), vec![id])),
        }
    }
    for ((task_id, kind), ids) in by_group {
        // The surviving CURRENT of the group after the UPDATE: exactly one
        // (the deterministic max of the (seq, created_at, id) order).
        let winner: String = tx
            .query_row(
                "SELECT id FROM artifacts WHERE task_id=?1 AND kind=?2 AND status='CURRENT'",
                params![task_id, kind],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::Corrupt(format!(
                "supersession backfill left group ({task_id}, {kind}) with no CURRENT row — aborted before commit"
            )))?;
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                task_id,
                event_kind::ARTIFACT_SUPERSEDED,
                format!("{MIGRATION_ACTOR_PREFIX}{}", step.id),
                json_string(&serde_json::json!({
                    (json_field::BACKFILL): step.id,
                    (json_field::SCRIPT_PATH): step.script_path,
                    (json_field::TASK_ID): task_id,
                    (json_field::KIND): kind,
                    (json_field::CURRENT_ARTIFACT_ID): winner,
                    (json_field::SUPERSEDED): ids,
                    (json_field::REASON): SUPERSESSION_BACKFILL_REASON,
                })),
                rows::ts(&Utc::now()),
            ],
        )?;
    }
    Ok(())
}

/// Fixed namespace for the deterministic ids of rung 004 (ratified spec
/// OQ2): every created decision row gets `UUIDv5(namespace, source event
/// id)` — a name-based id computed FROM the event id, so a repeated
/// extraction recomputes the same primary key and the same refs, and the
/// partial unique index `decisions_backfill_source_event` (predicated on
/// the v5 id space, which live rows — UUIDv7 — never enter) makes a second
/// insertion of the same event impossible at the database level. The value
/// is the backfill task id and is STABLE FOREVER: changing it would mint a
/// fresh id space that no longer dedups against the rows the first
/// application wrote.
pub const DECISIONS_BACKFILL_NAMESPACE: Uuid = Uuid::from_u128(0x01a1a14e2c0775a292ba905fe9886adc);

/// The deterministic decision id of the backfill (spec OQ2: "детерминированный
/// id decision из id исходного события"). Public so acceptance tests can
/// recompute the expected id instead of trusting the handler's bookkeeping.
pub fn backfill_decision_id(event_id: &str) -> Id {
    Uuid::new_v5(&DECISIONS_BACKFILL_NAMESPACE, event_id.as_bytes())
}

/// Rung v3→v4 (REQ-BF-*): the one-time extraction of historical
/// approve/reject/return acts into `decisions`. Runs INSIDE the rung
/// transaction opened by `open()`, so detection, the NEW decision rows, the
/// fact events and the `user_version` bump commit atomically — a failure
/// writing an event rolls the whole rung back (the 003 precedent, REQ-BF-003
/// AC3). This is CODE because SQL cannot parse the payload JSON, derive the
/// deterministic ids and emit one fact event per (task_id, source event
/// kind) group citing exactly the rows it created (the manifest's
/// documented code/data boundary); the shipped script's DDL still applies
/// verbatim inside the handler. Detection runs BEFORE any mutation in the
/// ratified "already disclosed" form (OQ2/OQ6, predicate style of 003,
/// REQ-BF-005): an event is extracted only when NO decision cites its id in
/// `refs` (re-application matches nothing its own rows already store) and —
/// when the payload carries a string rationale — no decision of the same
/// task already stores that exact text (today's `task approve` records
/// event AND decision beside each other, so its event is skipped). A
/// payload WITHOUT a rationale is still extracted as a NULL-rationale row
/// (ratified OQ4: the fact of the decision must not be lost — the reader
/// renders "(none stored)"). Beyond detection, the hard idempotence is the
/// database's: each insert carries the deterministic id and
/// `ON CONFLICT DO NOTHING` (never INSERT OR IGNORE — the silent-loss
/// precedent), and the affected-row count says whether THIS run created the
/// row, so fact events cite only rows truly created. One extraction NEVER
/// rewrites history: rows are only appended, events are only appended.
/// Verdict and decision kind are derived from the event kind — the payload
/// does not carry a verdict; the kind IS the act (REQ-BF-002); the reject/
/// return kinds are the producer-dictionary constants of ratified OQ3.
/// `actor` and `created_at` are copied from the event COLUMNS verbatim, and
/// every created row cites its source event id in `refs` (OQ6:
/// traceability and the dedup material).
fn decisions_backfill(tx: &Transaction<'_>, step: &SchemaRung) -> StoreResult<()> {
    // Shape detection before any mutation (the code-rung boundary the
    // RUNG_APPLIERS comment anticipates): a database predating the very
    // tables this extraction reads has nothing to backfill — inventing
    // domain rows from absent history is not repair, and a shipped v1
    // fixture shape stays migratable forward. Both tables must be present
    // or the rung is inert.
    let shapes: i64 = tx.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('workflow_events', 'decisions')", [], |r| r.get(0))?;
    if shapes < 2 {
        return Ok(());
    }
    let candidates: Vec<(String, String, String, String, String, String)> = {
        // Only events of a LIVE task are extractable: `decisions.task_id`
        // carries an FK, and an orphan row (task removed via CASCADE while
        // its events stay, e.g. bundle handling) must not brick `open()`.
        let mut stmt = tx.prepare(
            "SELECT id, task_id, kind, actor, payload, created_at FROM workflow_events \
             WHERE task_id IS NOT NULL \
               AND EXISTS (SELECT 1 FROM tasks tk WHERE tk.id = workflow_events.task_id) \
               AND kind IN (?1, ?2, ?3) ORDER BY seq",
        )?;
        let rows = stmt.query_map(
            params![
                event_kind::REQUIREMENTS_APPROVED,
                event_kind::REQUIREMENTS_REJECTED,
                event_kind::QUALITY_RETURNED
            ],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    // The shipped script (additive DDL) lands inside the same transaction.
    tx.execute_batch(step.script)?;

    // Grouped exactly like 003 (ratified OQ5): one fact event per
    // (task_id, source event kind) group that produced rows; a clean
    // database produces zero events.
    let mut groups: Vec<((String, String), Vec<String>, Vec<String>)> = Vec::new();
    for (event_id, task_id, kind, actor, payload, created_at) in candidates {
        let (dec_kind, verdict) = match kind.as_str() {
            k if k == event_kind::REQUIREMENTS_APPROVED => {
                (decision_kind::REQUIREMENTS_APPROVAL, Verdict::Approved)
            }
            k if k == event_kind::REQUIREMENTS_REJECTED => {
                (decision_kind::REQUIREMENTS_REJECTION, Verdict::Rejected)
            }
            k if k == event_kind::QUALITY_RETURNED => {
                (decision_kind::QUALITY_RETURN, Verdict::Returned)
            }
            _ => continue,
        };
        let parsed: serde_json::Value = serde_json::from_str(&payload).map_err(|e| {
            StoreError::Corrupt(format!("workflow_events id={event_id}: payload is not valid JSON ({e}) — refusing to build decisions on unreadable history"))
        })?;
        // Ratified OQ4: a payload WITHOUT a rationale (missing key or JSON
        // null) is still extracted, as a NULL-rationale row — skipping it
        // would lose the fact that the human acted; the reader renders
        // "(none stored)". Nothing is ever invented: the rationale column
        // stores the payload text verbatim, byte-identical, or NULL.
        let rationale: Option<String> = parsed
            .get(json_field::RATIONALE)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        // "Already disclosed" detection BEFORE any mutation (REQ-BF-005,
        // predicate style of 003), in the ratified OQ2/OQ6 order:
        // (a) a decision already citing THIS event id in refs — a
        //     re-applied rung matches nothing its own rows stored;
        let refs_json = json_string(&vec![event_id.clone()]);
        let cited: i64 = tx.query_row(
            "SELECT count(*) FROM decisions WHERE refs=?1",
            params![refs_json],
            |r| r.get(0),
        )?;
        if cited > 0 {
            continue;
        }
        // (b) for string rationales: a decision of the same task already
        //     storing that exact text — the modern write-path's event is
        //     never backfilled secondarily. NULL rationales have no text to
        //     match; (a) plus the deterministic id dedups them.
        if let Some(text) = &rationale {
            let disclosed: i64 = tx.query_row(
                "SELECT count(*) FROM decisions WHERE task_id=?1 AND rationale=?2",
                params![task_id, text],
                |r| r.get(0),
            )?;
            if disclosed > 0 {
                continue;
            }
        }
        // Deterministic id from the source event id (OQ2/OQ6) inserted with
        // ON CONFLICT DO NOTHING (never INSERT OR IGNORE — the silent-loss
        // precedent): the database pins the no-op, the affected-row count
        // tells whether THIS run created the row, so a re-applied rung
        // cites zero rows and writes zero fact events.
        let decision_id = backfill_decision_id(&event_id).to_string();
        let inserted = tx.execute(
            "INSERT INTO decisions(id,task_id,kind,verdict,actor,rationale,refs,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT DO NOTHING",
            params![decision_id, task_id, dec_kind, verdict.as_str(), actor, rationale, refs_json, created_at],
        )?;
        if inserted == 0 {
            continue;
        }
        match groups
            .iter_mut()
            .find(|((g_task, g_kind), _, _)| *g_task == task_id && *g_kind == kind)
        {
            Some((_, src, decs)) => {
                src.push(event_id);
                decs.push(decision_id);
            }
            None => groups.push(((task_id, kind), vec![event_id], vec![decision_id])),
        }
    }
    for ((task_id, kind), source_events, decision_ids) in groups {
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                task_id,
                event_kind::DECISIONS_BACKFILLED,
                format!("{MIGRATION_ACTOR_PREFIX}{}", step.id),
                json_string(&serde_json::json!({
                    (json_field::BACKFILL): step.id,
                    (json_field::SCRIPT_PATH): step.script_path,
                    (json_field::TASK_ID): task_id,
                    (json_field::KIND): kind,
                    (json_field::SOURCE_EVENTS): source_events,
                    (json_field::DECISION_IDS): decision_ids,
                    (json_field::REASON): DECISIONS_BACKFILL_REASON,
                })),
                rows::ts(&Utc::now()),
            ],
        )?;
    }
    Ok(())
}

/// Rung v6→v7 (REQ-SW-01/07, ratified OQ-1): the single-writer guard lands
/// on an EXISTING database whose history may already hold several RUNNING
/// rows for one task — nothing forbade that before v7, so a bare
/// `CREATE UNIQUE INDEX` would abort the ladder on a store that is merely
/// old, not corrupt. The repair, in the 003 observe-before-mutate shape:
/// detection runs FIRST inside the rung transaction (exactly the rows the
/// closure flips, cited by id in the event, never performed silently), the
/// losers close as FAILED carrying the 'single-writer migration' marker in
/// `error` — winner = newest `started_at`, ties on the UUIDv7 id order —
/// then the shipped script applies idempotently (columns + index), then one
/// fact event per duplicated task. Rerunning the rung against an already
/// honest database matches zero rows, closes zero rows, writes zero events.
/// A status/finished_at/error write only: no row or event is deleted, no
/// review chain is touched (the append-only store discipline).
fn single_writer_repair(tx: &Transaction<'_>, step: &SchemaRung) -> StoreResult<()> {
    let losers: Vec<(String, String)> = {
        let mut stmt = tx.prepare(&format!(
            "SELECT id, task_id FROM role_executions WHERE {RUNNING_DUPLICATE_LOSER_PREDICATE} ORDER BY task_id, started_at, id"
        ))?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push((r.get(0)?, r.get(1)?));
        }
        out
    };
    let now = rows::ts(&Utc::now());
    for (id, _) in &losers {
        tx.execute(
            "UPDATE role_executions SET status='FAILED', finished_at=?1, error=?2 WHERE id=?3",
            params![now, SINGLE_WRITER_REPAIR_REASON, id],
        )?;
    }
    // Columns + index DDL, idempotent (the 006 ADD COLUMN precedent): the
    // closure above already left every group with exactly one RUNNING row,
    // so the unique index builds.
    apply_script_with_column_skip(tx, step.script)?;
    // One fact event per duplicated task (grouping discipline of 003/005;
    // a clean database produces zero events).
    let mut by_task: Vec<(String, Vec<String>)> = Vec::new();
    for (id, task_id) in losers {
        match by_task.iter_mut().find(|(t, _)| *t == task_id) {
            Some((_, ids)) => ids.push(id),
            None => by_task.push((task_id, vec![id])),
        }
    }
    for (task_id, ids) in by_task {
        // The surviving RUNNING row of the group: exactly one — the strict
        // maximum of the (started_at, id) total order never matches its own
        // loser predicate.
        let winner: String = tx
            .query_row(
                "SELECT id FROM role_executions WHERE task_id=?1 AND status='RUNNING'",
                params![task_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::Corrupt(format!(
                "single-writer repair left task {task_id} with no RUNNING row — aborted before commit"
            )))?;
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                task_id,
                event_kind::EXECUTION_SINGLE_WRITER_REPAIR,
                format!("{MIGRATION_ACTOR_PREFIX}{}", step.id),
                json_string(&serde_json::json!({
                    (json_field::BACKFILL): step.id,
                    (json_field::SCRIPT_PATH): step.script_path,
                    (json_field::TASK_ID): task_id,
                    "kept_running": winner,
                    "closed_executions": ids,
                    (json_field::REASON): SINGLE_WRITER_REPAIR_REASON,
                })),
                rows::ts(&Utc::now()),
            ],
        )?;
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Input for publishing a new artifact version.
pub struct ArtifactDraft {
    pub task_id: Id,
    pub kind: ArtifactKind,
    pub name: String,
    pub category: DataCategory,
    pub content: Vec<u8>,
    pub media_type: String,
    pub contract: Option<ContractRef>,
    pub created_by: String,
    pub execution_id: Option<Id>,
    pub depends_on: Vec<Id>,
}

/// Best-effort media type from a file name.
pub fn guess_media_type(path: &Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("md" | "markdown") => "text/markdown".into(),
        Some("json") => "application/json".into(),
        Some("txt") => "text/plain".into(),
        Some("patch" | "diff") => "text/x-diff".into(),
        Some("html") => "text/html".into(),
        Some("rs" | "py" | "js" | "ts") => "text/x-source".into(),
        _ => "application/octet-stream".into(),
    }
}

#[cfg(test)]
mod trigger_tests {
    use super::*;

    #[test]
    fn event_log_rejects_update_and_delete() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t = Task::new("t", "d", None, Utc::now());
        store.create_task(&t).unwrap();

        let upd = store.conn.execute(
            "UPDATE workflow_events SET kind='forged' WHERE task_id=?1",
            params![t.id.to_string()],
        );
        assert!(upd.is_err(), "events must not be updatable");
        let del = store.conn.execute(
            "DELETE FROM workflow_events WHERE task_id=?1",
            params![t.id.to_string()],
        );
        assert!(del.is_err(), "events must not be deletable");
        assert_eq!(store.events(t.id, None).unwrap().len(), 1);
    }
}

#[cfg(test)]
mod dependency_atomicity_tests {
    use super::*;

    /// R1 QA (mt/dry): the atomic compound and the retroactive `task depend`
    /// path share ONE edge-insert helper — so the REFUSAL family must not
    /// drift either. For the shapes both paths can produce, the two errors
    /// are the same variant with byte-identical text, verified by direct
    /// string comparison (drift in either format string fails this test).
    #[test]
    fn compound_and_link_paths_refuse_with_identical_error_text() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let blocker = Task::new("blocker", "fixture", None, Utc::now());
        store.create_task(&blocker).unwrap();

        // (1) Self-reference: the compound refuses a task depending on
        // itself with the same text the link path produces.
        let mut self_dep = Task::new("selfish", "fixture", None, Utc::now());
        self_dep.id = blocker.id; // forces the `t.id == dep.id` guard
        let compound_self = store
            .create_task_with_dependencies(
                &self_dep,
                &[(&blocker, DependencyKind::Parent)],
                "op",
                Utc::now(),
                None,
            )
            .unwrap_err();
        let link_self = store
            .link_task_dependency(
                blocker.id,
                blocker.id,
                DependencyKind::Parent,
                "op",
                Utc::now(),
            )
            .unwrap_err();
        assert!(matches!(compound_self, StoreError::InvalidDependency(_)));
        assert_eq!(
            compound_self.to_string(),
            link_self.to_string(),
            "self-reference refusal must be byte-identical across paths"
        );

        // (2) Duplicate edge: within-request duplicate in the compound vs
        // a stored duplicate on the link path — same task pair, same kind.
        let fresh = Task::new("fresh", "fixture", None, Utc::now());
        let compound_dup = store
            .create_task_with_dependencies(
                &fresh,
                &[
                    (&blocker, DependencyKind::Parent),
                    (&blocker, DependencyKind::Parent),
                ],
                "op",
                Utc::now(),
                None,
            )
            .unwrap_err();
        assert_eq!(
            store
                .create_task_with_dependencies(
                    &fresh,
                    &[(&blocker, DependencyKind::Parent)],
                    "op",
                    Utc::now(),
                    None
                )
                .unwrap()
                .len(),
            1,
            "the refusal above left nothing behind — the compound create now lands exactly one edge"
        );
        let link_dup = store
            .link_task_dependency(
                fresh.id,
                blocker.id,
                DependencyKind::Parent,
                "op",
                Utc::now(),
            )
            .unwrap_err();
        assert!(matches!(link_dup, StoreError::InvalidDependency(_)));
        assert_eq!(
            compound_dup.to_string(),
            link_dup.to_string(),
            "duplicate-edge refusal must be byte-identical across paths"
        );
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    /// REQ-002 (of T1): a v1 database migrates forward ADDITIVELY — the old
    /// execution row survives untouched with a NULL revision, and nothing
    /// pre-existing is rewritten or deleted. The v2→v3 supersession rung
    /// walks the same ladder: the fixture carries a v1-shaped `artifacts`
    /// table (every real v1 schema had it), so the backfill detects a
    /// clean group and performs no flips at all.
    #[test]
    fn v1_database_migrates_to_v2_without_touching_history() {
        let dir = tempfile::TempDir::new().unwrap();
        let old_id = new_id().to_string();
        let new_id_s = new_id().to_string();
        let task_uuid = new_id().to_string();
        {
            // Hand-build a v1-shaped database: role_executions WITHOUT the
            // workspace_revision column, one historical row, plus the v1
            // artifacts table (empty — the v3 rung must find nothing).
            let conn = Connection::open(dir.path().join("devai.db")).unwrap();
            conn.execute_batch(
                "PRAGMA user_version=1;
                 CREATE TABLE tasks(
                   id TEXT PRIMARY KEY,
                   project_id TEXT NULL,
                   title TEXT NOT NULL,
                   business_description TEXT NOT NULL DEFAULT '',
                   state TEXT NOT NULL,
                   return_state TEXT NULL,
                   requirements_seq INTEGER NOT NULL DEFAULT 0,
                   lock_version INTEGER NOT NULL DEFAULT 1,
                   created_at TEXT NOT NULL,
                   updated_at TEXT NOT NULL);
                 CREATE TABLE role_executions(
                   id TEXT PRIMARY KEY, task_id TEXT NOT NULL, role_slug TEXT NOT NULL,
                   role_version INTEGER NOT NULL, agent_name TEXT NULL, status TEXT NOT NULL,
                   inputs TEXT NOT NULL DEFAULT '[]', summary TEXT NULL, error TEXT NULL,
                   started_at TEXT NOT NULL, finished_at TEXT NULL);
                 CREATE TABLE artifacts(
                   id TEXT PRIMARY KEY, task_id TEXT NOT NULL, kind TEXT NOT NULL,
                   name TEXT NOT NULL, seq INTEGER NOT NULL, category TEXT NOT NULL,
                   status TEXT NOT NULL, sha256 TEXT NOT NULL, size INTEGER NOT NULL,
                   media_type TEXT NOT NULL DEFAULT 'application/octet-stream',
                   contract_name TEXT NULL, contract_version INTEGER NULL,
                   created_by TEXT NOT NULL, execution_id TEXT NULL,
                   depends_on TEXT NOT NULL DEFAULT '[]', created_at TEXT NOT NULL);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO role_executions(id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at)
                 VALUES(?1,?2,'requirements-analyst',1,'mock','COMPLETED','[]','was old',NULL,'2026-01-01T00:00:00.000000Z',NULL)",
                params![old_id, task_uuid],
            )
            .unwrap();
        }

        let store = Store::open(dir.path()).unwrap();
        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION, "open must migrate v1 forward");

        let e = store.get_execution(&old_id).unwrap();
        assert!(
            e.workspace_revision.is_none(),
            "history rows keep a NULL revision — never guessed"
        );
        assert_eq!(
            e.summary.as_deref(),
            Some("was old"),
            "the historical row is byte-intact"
        );

        let count: i64 = store
            .conn
            .query_row("SELECT count(*) FROM evaluation_profiles", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "the new table exists and starts empty");

        // The new column accepts v2 writes (a NEW row — history is never
        // updated): append-only in the literal SQL sense.
        store
            .conn
            .execute(
                "INSERT INTO role_executions(id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision)
                 VALUES(?1,?2,'quality-auditor',2,'mock','COMPLETED','[]',NULL,NULL,'2026-01-02T00:00:00.000000Z',NULL,?3)",
                params![new_id_s, task_uuid, json_string(&serde_json::json!({"git_head":null,"dirty_digest":null,"profile":null,"roles":[]}))],
            )
            .unwrap();
        let e = store.get_execution(&new_id_s).unwrap();
        assert!(
            e.workspace_revision.is_some(),
            "v2 rows can carry a revision"
        );
        let old = store.get_execution(&old_id).unwrap();
        assert!(
            old.workspace_revision.is_none(),
            "the historical row was NOT rewritten by the v2 write"
        );
    }

    /// A structural fingerprint of one table as the DATABASE sees it:
    /// column shapes, every index (name, uniqueness, origin, partial
    /// predicate, indexed column order) and declared foreign keys — all
    /// read through PRAGMAs, never from the DDL text (schema.sql writes
    /// the statements with comments and `IF NOT EXISTS` variants that are
    /// not part of the stored structure). Deterministically ordered.
    fn structural_fingerprint(conn: &Connection, table: &str) -> Vec<String> {
        let pragma = |sql: String| -> Vec<String> {
            let mut stmt = conn.prepare(&sql).unwrap();
            let ncols = stmt.column_count();
            let mut rows = stmt.query([]).unwrap();
            let mut out = Vec::new();
            while let Some(r) = rows.next().unwrap() {
                let cols: Vec<String> = (0..ncols)
                    .map(|i| {
                        let v: rusqlite::types::Value = r.get(i).unwrap();
                        format!("{v:?}")
                    })
                    .collect();
                out.push(cols.join("|"));
            }
            out.sort();
            out
        };
        let mut lines = Vec::new();
        for row in pragma(format!("PRAGMA table_info({table})")) {
            lines.push(format!("column:{row}"));
        }
        for row in pragma(format!("PRAGMA foreign_key_list({table})")) {
            lines.push(format!("fk:{row}"));
        }
        for row in pragma(format!("PRAGMA index_list({table})")) {
            lines.push(format!("index:{row}"));
            // `row` is seq|name|unique|origin|partial — expand the columns
            // the index covers (2nd field is the index name).
            let name = row.split('|').nth(1).unwrap().trim_matches('"');
            for col in pragma(format!("PRAGMA index_info('{name}')")) {
                lines.push(format!("index:{name}:col:{col}"));
            }
        }
        lines
    }

    /// R2 QA (mt/change-isolation): a new rung requires THREE sites —
    /// manifest.json, migrations/*.sql and the schema.sql mirror — and a
    /// skipped mirror edit was only catchable indirectly. This test is the
    /// loud mirror check in the style of the guard-index precedent
    /// (`the_index_itself_rejects_a_second_running_row`), extended to the
    /// FULL object set of rung 008: the fresh home (schema.sql executed
    /// whole) and the migrated home (hand-built v1 shape walked by
    /// `open()` through every manifest rung to v8) must agree on the
    /// structure fingerprint of `task_creation_keys` and every index on
    /// it. Divergent DDL between the mirror and the ladder fails here,
    /// byte-for-byte visible in the diff of the two line sets.
    #[test]
    fn rung_008_object_set_is_identical_on_fresh_and_migrated_homes() {
        // Fresh home: the declarative mirror.
        let fresh_dir = tempfile::TempDir::new().unwrap();
        let fresh = Store::open(fresh_dir.path()).unwrap();
        assert_eq!(
            fresh
                .conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION,
            "a fresh home lands on the top rung"
        );

        // Migrated home: the v1 shape from the precedent above, carried to
        // v8 by the generic ladder executor — rung 008 runs from DATA.
        let mig_dir = tempfile::TempDir::new().unwrap();
        {
            let conn = Connection::open(mig_dir.path().join("devai.db")).unwrap();
            conn.execute_batch(
                "PRAGMA user_version=1;
                 CREATE TABLE tasks(
                   id TEXT PRIMARY KEY,
                   project_id TEXT NULL,
                   title TEXT NOT NULL,
                   business_description TEXT NOT NULL DEFAULT '',
                   state TEXT NOT NULL,
                   return_state TEXT NULL,
                   requirements_seq INTEGER NOT NULL DEFAULT 0,
                   lock_version INTEGER NOT NULL DEFAULT 1,
                   created_at TEXT NOT NULL,
                   updated_at TEXT NOT NULL);
                 CREATE TABLE role_executions(
                   id TEXT PRIMARY KEY, task_id TEXT NOT NULL, role_slug TEXT NOT NULL,
                   role_version INTEGER NOT NULL, agent_name TEXT NULL, status TEXT NOT NULL,
                   inputs TEXT NOT NULL DEFAULT '[]', summary TEXT NULL, error TEXT NULL,
                   started_at TEXT NOT NULL, finished_at TEXT NULL);
                 CREATE TABLE artifacts(
                   id TEXT PRIMARY KEY, task_id TEXT NOT NULL, kind TEXT NOT NULL,
                   name TEXT NOT NULL, seq INTEGER NOT NULL, category TEXT NOT NULL,
                   status TEXT NOT NULL, sha256 TEXT NOT NULL, size INTEGER NOT NULL,
                   media_type TEXT NOT NULL DEFAULT 'application/octet-stream',
                   contract_name TEXT NULL, contract_version INTEGER NULL,
                   created_by TEXT NOT NULL, execution_id TEXT NULL,
                   depends_on TEXT NOT NULL DEFAULT '[]', created_at TEXT NOT NULL);",
            )
            .unwrap();
        }
        let migrated = Store::open(mig_dir.path()).unwrap();
        assert_eq!(
            migrated
                .conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION,
            "the ladder carried the migrated home to the top rung"
        );

        let fp_fresh = structural_fingerprint(&fresh.conn, "task_creation_keys");
        let fp_migrated = structural_fingerprint(&migrated.conn, "task_creation_keys");
        assert!(
            fp_fresh.iter().any(|l| l.starts_with("column:"))
                && fp_fresh
                    .iter()
                    .any(|l| l.contains("task_creation_keys_task")),
            "the fingerprint is non-vacuous: the rung 008 table AND its named index are present on the fresh home — got {fp_fresh:?}"
        );
        assert_eq!(
            fp_fresh, fp_migrated,
            "schema.sql and the migration ladder must land the SAME rung-008 structure (mirror drift)"
        );
    }
}

#[cfg(test)]
mod supersession_tests {
    use super::*;

    fn task(store: &Store) -> Task {
        let t = Task::new("supersession", "fixture", None, Utc::now());
        store.create_task(&t).unwrap();
        t
    }

    fn publish(store: &Store, t: &Task, kind: ArtifactKind, bytes: &[u8]) -> Artifact {
        store
            .publish_artifact(ArtifactDraft {
                task_id: t.id,
                kind,
                name: format!("{kind:?}"),
                category: DataCategory::Artifact,
                content: bytes.to_vec(),
                media_type: "text/plain".into(),
                contract: None,
                created_by: "user".into(),
                execution_id: None,
                depends_on: vec![],
            })
            .unwrap()
    }

    fn status_of(store: &Store, id: Id) -> ArtifactStatus {
        store.get_artifact(&id.to_string()).unwrap().status
    }

    fn superseded_events(store: &Store, task_id: Id) -> Vec<WorkflowEvent> {
        store
            .events(task_id, None)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == event_kind::ARTIFACT_SUPERSEDED)
            .collect()
    }

    /// Only the events written by the v2→v3 rung (publish-path events share
    /// the same kind, so repair accounting must filter by actor).
    fn migration_events(store: &Store, task_id: Id) -> Vec<WorkflowEvent> {
        superseded_events(store, task_id)
            .into_iter()
            .filter(|e| e.actor == format!("{MIGRATION_ACTOR_PREFIX}003_artifact_supersession"))
            .collect()
    }

    /// REQ-001 atomicity: if the INSERT (or anything after the supersede
    /// UPDATE) fails, the flip is rolled back with it — never a
    /// zero-CURRENT intermediate, never an orphaned event.
    #[test]
    fn publish_rolls_back_the_supersede_flip_when_insert_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t = task(&store);
        let v1 = publish(&store, &t, ArtifactKind::Patch, b"v1");
        // Simulate a mid-transaction failure: every artifacts INSERT aborts.
        store
            .conn
            .execute_batch("CREATE TRIGGER test_fail_artifact_insert BEFORE INSERT ON artifacts BEGIN SELECT RAISE(ABORT,'boom'); END;")
            .unwrap();
        let draft = ArtifactDraft {
            task_id: t.id,
            kind: ArtifactKind::Patch,
            name: "Patch".into(),
            category: DataCategory::Artifact,
            content: b"v2".to_vec(),
            media_type: "text/plain".into(),
            contract: None,
            created_by: "user".into(),
            execution_id: None,
            depends_on: vec![],
        };
        assert!(
            store.publish_artifact(draft).is_err(),
            "the aborted INSERT must surface"
        );
        store
            .conn
            .execute_batch("DROP TRIGGER test_fail_artifact_insert")
            .unwrap();

        assert_eq!(
            status_of(&store, v1.id),
            ArtifactStatus::Current,
            "the rolled-back flip must leave exactly one CURRENT"
        );
        assert_eq!(
            store.artifacts(t.id).unwrap().len(),
            1,
            "no half-published row survived"
        );
        assert!(
            superseded_events(&store, t.id).is_empty(),
            "a rolled-back transaction writes no event"
        );
    }

    /// REQ-001 acceptance (AC2), verbatim: the invariant query itself —
    /// `SELECT task_id, kind, COUNT(*) FROM artifacts WHERE status='CURRENT'
    /// GROUP BY 1,2 HAVING COUNT(*)>1` — must be empty after an arbitrary
    /// (interleaved, multi-kind, multi-task) publication series. The DB-level
    /// pin (partial unique index, schema v3) makes a violation uncommittable;
    /// this test runs the literal detection the task text names.
    #[test]
    fn invariant_query_finds_no_multi_current_group_after_publish_series() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t1 = task(&store);
        let t2 = {
            let t = Task::new("supersession-2", "fixture", None, Utc::now());
            store.create_task(&t).unwrap();
            t
        };
        // Interleaved series: first versions AND supersessions of several
        // kinds across two tasks, in one mixed sequence.
        for bytes in [b"a", b"b", b"c"] {
            publish(&store, &t1, ArtifactKind::RequirementsSpec, bytes);
            publish(&store, &t1, ArtifactKind::ReviewReport, bytes);
            publish(&store, &t1, ArtifactKind::ImplementationReport, bytes);
            publish(&store, &t1, ArtifactKind::QualityReport, bytes);
            publish(&store, &t2, ArtifactKind::QualityReport, bytes);
            publish(&store, &t2, ArtifactKind::Patch, bytes);
        }
        let mut stmt = store
            .conn
            .prepare(
                "SELECT task_id, kind, COUNT(*) FROM artifacts WHERE status='CURRENT' GROUP BY 1,2 HAVING COUNT(*)>1",
            )
            .unwrap();
        let offenders: Vec<(String, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            offenders.is_empty(),
            "the single-CURRENT invariant query must be empty, found {offenders:?}"
        );
        // History is never deleted: all 18 rows remain readable.
        assert_eq!(store.artifacts(t1.id).unwrap().len(), 12);
        assert_eq!(store.artifacts(t2.id).unwrap().len(), 6);
    }

    /// REQ-003: the v2→v3 rung repairs the 2026-11-02 precedent shape (three
    /// CURRENT QUALITY_REPORT rows) to exactly one CURRENT per group, keeps
    /// history, and records one event per repaired group — "not silently".
    #[test]
    fn v2_multi_current_database_is_repaired_with_events() {
        let dir = tempfile::TempDir::new().unwrap();
        let (task_id, ids, patch_id): (Id, Vec<Id>, Id);
        {
            let store = Store::open(dir.path()).unwrap();
            let t = task(&store);
            let a1 = publish(&store, &t, ArtifactKind::QualityReport, b"q1");
            let a2 = publish(&store, &t, ArtifactKind::QualityReport, b"q2");
            let a3 = publish(&store, &t, ArtifactKind::QualityReport, b"q3");
            let p = publish(&store, &t, ArtifactKind::Patch, b"p1");
            // Recreate the pre-v3 defect: all report rows CURRENT at once,
            // the pin removed, version pushed back to v2.
            store
                .conn
                .execute_batch("DROP INDEX artifacts_single_current; UPDATE artifacts SET status='CURRENT'; PRAGMA user_version=2;")
                .unwrap();
            task_id = t.id;
            ids = vec![a1.id, a2.id, a3.id];
            patch_id = p.id;
        }
        let store = Store::open(dir.path()).unwrap();

        let arts = store.artifacts(task_id).unwrap();
        // Exactly one CURRENT per group; max-seq survives; nothing deleted.
        let current: Vec<Id> = arts
            .iter()
            .filter(|a| {
                a.status == ArtifactStatus::Current && a.kind == ArtifactKind::QualityReport
            })
            .map(|a| a.id)
            .collect();
        assert_eq!(current, vec![ids[2]], "the max-seq row is kept CURRENT");
        assert_eq!(
            arts.iter()
                .filter(|a| a.status == ArtifactStatus::Superseded
                    && a.kind == ArtifactKind::QualityReport)
                .count(),
            2
        );
        assert_eq!(
            status_of(&store, patch_id),
            ArtifactStatus::Current,
            "an honest group is untouched"
        );
        assert_eq!(arts.len(), 4, "backfill never deletes history");

        // One repair event citing winner + flipped ids, actor `migration:<id>`
        // (the two publish-path events of the seeding remain untouched).
        let evs = migration_events(&store, task_id);
        assert_eq!(evs.len(), 1, "one batched event per repaired group");
        assert_eq!(
            superseded_events(&store, task_id).len(),
            3,
            "history keeps the two publish-path events too"
        );
        let ev = &evs[0];
        assert_eq!(
            ev.actor,
            format!("{MIGRATION_ACTOR_PREFIX}003_artifact_supersession")
        );
        assert_eq!(
            ev.payload[json_field::BACKFILL],
            "003_artifact_supersession"
        );
        assert_eq!(
            ev.payload[json_field::SCRIPT_PATH],
            "migrations/003_artifact_supersession.sql"
        );
        assert_eq!(ev.payload[json_field::KIND], "QUALITY_REPORT");
        assert_eq!(
            ev.payload[json_field::CURRENT_ARTIFACT_ID],
            ids[2].to_string()
        );
        let flipped: Vec<String> = ev.payload[json_field::SUPERSEDED]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            flipped,
            vec![ids[0].to_string(), ids[1].to_string()],
            "both older CURRENT rows are cited, in seq order"
        );
        assert!(
            ev.payload[json_field::REASON]
                .as_str()
                .unwrap()
                .contains("nothing deleted"),
            "the why is recorded, not silent"
        );

        // Idempotence: a second open runs no rung (version already 3) and
        // re-opening on a forced-2 version repairs nothing / emits nothing.
        let v: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(
            migration_events(&store, task_id).len(),
            1,
            "re-open repeats no repair"
        );
    }

    /// REQ-003 edge (open question "equal-seq rungs"): the repair is
    /// deterministic — seq, then created_at, then id (UUIDv7 text order is
    /// time order). No silent coin flip, no operator abort.
    #[test]
    fn backfill_ties_on_seq_and_created_at_break_on_id() {
        let dir = tempfile::TempDir::new().unwrap();
        let (a1, a2);
        {
            let store = Store::open(dir.path()).unwrap();
            let t = task(&store);
            a1 = publish(&store, &t, ArtifactKind::TestReport, b"one");
            a2 = publish(&store, &t, ArtifactKind::TestReport, b"two");
            store
                .conn
                .execute_batch(
                    "DROP INDEX artifacts_single_current;
                     UPDATE artifacts SET status='CURRENT', seq=1, created_at='2026-01-01T00:00:00.000000Z';
                     PRAGMA user_version=2;",
                )
                .unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        // The winner is whichever id is lexicographically greater — the
        // same total order the SQL predicate uses, so the outcome is
        // deterministic per fixture and asserted as such, not guessed.
        let (winner, loser) = if a1.id.to_string() > a2.id.to_string() {
            (a1.id, a2.id)
        } else {
            (a2.id, a1.id)
        };
        assert_eq!(
            status_of(&store, winner),
            ArtifactStatus::Current,
            "equal (seq, created_at): the greater id wins"
        );
        assert_eq!(status_of(&store, loser), ArtifactStatus::Superseded);
    }

    /// REQ-003 acceptance "чистая база — ноль UPDATE и ноль событий": a
    /// pre-v3 database with honest statuses gets pushed through the rung
    /// with no flips and no events.
    #[test]
    fn backfill_on_clean_database_writes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        {
            let store = Store::open(dir.path()).unwrap();
            let t = task(&store);
            publish(&store, &t, ArtifactKind::DecisionRecord, b"one");
            publish(&store, &t, ArtifactKind::DecisionRecord, b"two");
            // Honest state (single CURRENT), but version pushed back to v2:
            // the rung must re-run and find nothing to repair.
            store.conn.execute_batch("PRAGMA user_version=2;").unwrap();
            assert_eq!(
                store
                    .events(t.id, None)
                    .unwrap()
                    .iter()
                    .filter(|e| e.kind == event_kind::ARTIFACT_SUPERSEDED)
                    .count(),
                1,
                "the publish-path event of the second version is the only one"
            );
        }
        let store = Store::open(dir.path()).unwrap();
        let t = store.list_tasks().unwrap().pop().unwrap();
        assert_eq!(
            superseded_events(&store, t.id).len(),
            1,
            "the backfill added no event"
        );
    }

    /// REQ-003 acceptance "сбой посреди ранга": a failure inside the rung
    /// (the repair events cannot be written) rolls back the flips, the
    /// index and the version bump together — no half-repaired group.
    #[test]
    fn backfill_is_atomic_when_the_event_write_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        {
            let store = Store::open(dir.path()).unwrap();
            let t = task(&store);
            publish(&store, &t, ArtifactKind::QualityReport, b"q1");
            publish(&store, &t, ArtifactKind::QualityReport, b"q2");
            store
                .conn
                .execute_batch("DROP INDEX artifacts_single_current; UPDATE artifacts SET status='CURRENT'; PRAGMA user_version=2;")
                .unwrap();
            // Hide the event table so the rung fails AFTER its UPDATE.
            store
                .conn
                .execute_batch("ALTER TABLE workflow_events RENAME TO workflow_events_hidden;")
                .unwrap();
        }
        let err = Store::open(dir.path());
        assert!(err.is_err(), "a mid-rung failure must surface");
        drop(err);
        {
            let conn = Connection::open(dir.path().join("devai.db")).unwrap();
            let cur: i64 = conn
                .query_row("SELECT count(*) FROM artifacts WHERE status='CURRENT' AND kind='QUALITY_REPORT'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(
                cur, 2,
                "the rolled-back rung left the group exactly as found — half-repair is impossible"
            );
            let v: i64 = conn
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
            assert_eq!(v, 2, "the version bump rolled back with the repair");
            let idx: i64 = conn
                .query_row("SELECT count(*) FROM sqlite_master WHERE type='index' AND name='artifacts_single_current'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(idx, 0, "the rung's DDL rolled back too");
            conn.execute_batch("ALTER TABLE workflow_events_hidden RENAME TO workflow_events;")
                .unwrap();
        }
        // With the table restored, the next open completes the repair.
        let store = Store::open(dir.path()).unwrap();
        let t = store.list_tasks().unwrap().pop().unwrap();
        assert_eq!(
            migration_events(&store, t.id).len(),
            1,
            "the retry performed exactly one recorded repair"
        );
        let arts = store.artifacts(t.id).unwrap();
        assert_eq!(
            arts.iter()
                .filter(|a| a.status == ArtifactStatus::Current)
                .count(),
            1,
            "exactly one CURRENT after the retry"
        );
        let v: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            v, SCHEMA_VERSION,
            "the version bump landed with the completed rung"
        );
    }

    /// QA return7 п.(г): a corrupt JSON array column is a read error, not
    /// an empty list (depends_on drives the stale closure).
    #[test]
    fn corrupt_array_column_fails_loud_on_read() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t = task(&store);
        let a = publish(&store, &t, ArtifactKind::Patch, b"p");
        store
            .conn
            .execute(
                "UPDATE artifacts SET depends_on='{broken' WHERE id=?1",
                params![a.id.to_string()],
            )
            .unwrap();
        assert!(
            store.get_artifact(&a.id.to_string()).is_err(),
            "corrupt depends_on must not read as `[]`"
        );
        assert!(store.artifacts(t.id).is_err());
    }

    /// REQ-004: the ladder is manifest DATA — the generated table mirrors
    /// `migrations/manifest.json` field-for-field (ids, ranges, script
    /// paths, apply tags), is continuous 1→SCHEMA_VERSION, and every tag
    /// resolves to a behavior. `SCHEMA_VERSION` itself is the manifest's
    /// top rung, not a hand-maintained const.
    #[test]
    fn schema_registry_is_continuous_data() {
        let rungs = schema_rungs();
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("../migrations/manifest.json")).unwrap();
        let records = doc["rungs"].as_array().unwrap();
        assert_eq!(
            rungs.len(),
            records.len(),
            "one generated rung per manifest record, in manifest order"
        );
        let mut prev_to = 1; // the ladder starts at the fresh-install version (schema.sql)
        for (rung, r) in rungs.iter().zip(records) {
            assert_eq!(rung.from, prev_to, "ladder continuous 1→N");
            assert_eq!(
                rung.to,
                rung.from + 1,
                "one rung advances exactly one version"
            );
            assert_eq!(
                rung.id,
                r["id"].as_str().unwrap(),
                "id is read from the manifest data"
            );
            assert_eq!(
                rung.script_path,
                r["script"].as_str().unwrap(),
                "script path is read from the manifest data"
            );
            assert_eq!(
                rung.apply,
                r["apply"].as_str().unwrap(),
                "apply tag is read from the manifest data"
            );
            assert!(!rung.script.is_empty() && !rung.id.is_empty());
            // Fail-loud contract: a tag the executor cannot run must never be reachable.
            assert!(
                RUNG_APPLIERS.iter().any(|(name, _)| *name == rung.apply),
                "apply tag `{}` of rung `{}` has no handler",
                rung.apply,
                rung.id
            );
            prev_to = rung.to;
        }
        assert_eq!(
            prev_to, SCHEMA_VERSION,
            "SCHEMA_VERSION is derived from the manifest top rung"
        );
    }
}

#[cfg(test)]
mod decisions_backfill_tests {
    use super::*;

    fn seed_event(
        store: &Store,
        id: &str,
        task_id: &str,
        kind: &str,
        actor: &str,
        payload: &str,
        created_at: &str,
    ) {
        store.conn.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![id, task_id, kind, actor, payload, created_at],
        )
        .unwrap();
    }

    fn payload_of(store: &Store, event_id: &str) -> String {
        store
            .conn
            .query_row(
                "SELECT payload FROM workflow_events WHERE id=?1",
                params![event_id],
                |r| r.get(0),
            )
            .unwrap()
    }

    /// REQ-BF-002/004/005 + REQ-BF-007 (store half): historical approve/
    /// reject/return acts are copied VERBATIM into NEW decision rows
    /// (actor/created_at from the event columns, verdict derived from the
    /// kind, deterministic ids from the event ids per ratified OQ2), one
    /// fact event per (task_id, kind) group (OQ5); a NULL-rationale event is
    /// extracted as a NULL row (ratified OQ4) while an already-disclosed
    /// modern approve is skipped; events stay byte-intact; a re-applied rung
    /// is a no-op.
    #[test]
    fn historical_events_backfill_decisions_once_and_idempotently() {
        let dir = tempfile::TempDir::new().unwrap();
        let task_id: Id;
        // Real UUID strings: the event reader parses ids as UUIDs.
        let (e_rej, e_appr, e_ret, e_null, e_modern) = (
            new_id().to_string(),
            new_id().to_string(),
            new_id().to_string(),
            new_id().to_string(),
            new_id().to_string(),
        );
        {
            let store = Store::open(dir.path()).unwrap();
            let t = Task::new("backfill", "fixture", None, Utc::now());
            store.create_task(&t).unwrap();
            task_id = t.id;
            let tid = t.id.to_string();
            // Pre-REQ-QAD-001 shape: the rationale lives only in the payload.
            seed_event(
                &store,
                &e_rej,
                &tid,
                event_kind::REQUIREMENTS_REJECTED,
                "human:legacy",
                r#"{"rationale":"split OQ1 before approving"}"#,
                "2026-09-01T00:00:00.000000Z",
            );
            seed_event(
                &store,
                &e_appr,
                &tid,
                event_kind::REQUIREMENTS_APPROVED,
                "human:legacy",
                r#"{"requirements_seq":2,"rationale":"OQ1: keep `task active`, do NOT ship `exec list --active`"}"#,
                "2026-10-01T00:00:00.000000Z",
            );
            seed_event(
                &store,
                &e_ret,
                &tid,
                event_kind::QUALITY_RETURNED,
                "human:legacy",
                r#"{"rationale":"ships the rejected alternative"}"#,
                "2026-10-05T00:00:00.000000Z",
            );
            // The NULL-rationale approve IS extracted (ratified OQ4: the
            // fact of the act must not be lost — NULL lands as a NULL row).
            seed_event(
                &store,
                &e_null,
                &tid,
                event_kind::REQUIREMENTS_APPROVED,
                "human:legacy",
                r#"{"requirements_seq":1,"rationale":null}"#,
                "2026-08-01T00:00:00.000000Z",
            );
            // The MODERN approve is the skip case: its decision row already
            // stands beside the event (the commands.rs write-path shape).
            seed_event(
                &store,
                &e_modern,
                &tid,
                event_kind::REQUIREMENTS_APPROVED,
                "human:now",
                r#"{"rationale":"already a stored decision"}"#,
                "2026-10-06T00:00:00.000000Z",
            );
            store
                .record_decision(&Decision {
                    id: new_id(),
                    task_id: t.id,
                    kind: decision_kind::REQUIREMENTS_APPROVAL.into(),
                    verdict: Verdict::Approved,
                    actor: "human:now".into(),
                    rationale: Some("already a stored decision".into()),
                    refs: vec!["spec-id".into()],
                    created_at: Utc::now(),
                    payload: serde_json::Value::Null,
                })
                .unwrap();
            // Pretend this database never saw v4: open() must run rung 004.
            store.conn.execute_batch("PRAGMA user_version=3;").unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(
            store
                .conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION,
            "open migrates v3 forward"
        );
        let idx: i64 = store
            .conn
            .query_row("SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN ('decisions_task','decisions_backfill_source_event')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            idx, 2,
            "the rung landed both indexes — the partial unique one pins the ratified OQ2 dedup at the database level"
        );

        let decisions = store.list_decisions(task_id).unwrap();
        // The one pre-existing row + all four undisclosed events, the
        // NULL-rationale one included (ratified OQ4); only the modern
        // approve is skipped — its rationale is already disclosed.
        assert_eq!(
            decisions.len(),
            5,
            "1 existing + 4 backfilled (NULL included), e-modern skipped: {decisions:?}"
        );
        assert!(
            decisions
                .iter()
                .all(|d| d.refs.iter().all(|r| *r != e_modern)),
            "the modern approve event is not extracted secondarily (REQ-BF-005 AC)"
        );
        // Stored chronology (ORDER BY created_at): Aug NULL-approve, Sep
        // reject, Oct approve, Oct return, then the pre-existing live row.
        for (event, row) in [
            (&e_null, &decisions[0]),
            (&e_rej, &decisions[1]),
            (&e_appr, &decisions[2]),
            (&e_ret, &decisions[3]),
        ] {
            assert_eq!(
                row.id,
                backfill_decision_id(event),
                "decision id is DETERMINISTICALLY derived from the source event id (ratified OQ2)"
            );
            assert_eq!(
                row.refs,
                vec![event.clone()],
                "the row cites its source event id in refs (ratified OQ6)"
            );
        }
        assert_eq!(decisions[0].kind, decision_kind::REQUIREMENTS_APPROVAL);
        assert_eq!(decisions[0].verdict, Verdict::Approved);
        assert!(
            decisions[0].rationale.is_none(),
            "OQ4: the NULL payload rationale lands as a NULL row, not a skip"
        );
        assert_eq!(decisions[1].kind, decision_kind::REQUIREMENTS_REJECTION);
        assert_eq!(decisions[1].verdict, Verdict::Rejected);
        let appr = &decisions[2];
        assert_eq!(appr.verdict, Verdict::Approved);
        assert_eq!(
            appr.rationale.as_deref().unwrap(),
            "OQ1: keep `task active`, do NOT ship `exec list --active`",
            "rationale copied byte-identical — never paraphrased"
        );
        assert!(
            appr.rationale.as_deref().unwrap().contains("task active"),
            "the acceptance string is visible to readers"
        );
        assert_eq!(
            appr.actor, "human:legacy",
            "actor comes from the event column"
        );
        let ret = &decisions[3];
        assert_eq!(ret.kind, decision_kind::QUALITY_RETURN);
        assert_eq!(ret.verdict, Verdict::Returned);
        assert_eq!(
            ret.created_at
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            "2026-10-05T00:00:00.000000Z",
            "created_at preserved from the event"
        );

        // created_at byte-identity at the column level too (the reader
        // reproduces the stored text, not a re-format).
        let raw: String = store
            .conn
            .query_row(
                "SELECT created_at FROM decisions WHERE id=?1",
                params![backfill_decision_id(&e_appr).to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(raw, "2026-10-01T00:00:00.000000Z");

        // One fact event per (task, source kind) group (ratified OQ5) —
        // actor migration:004, payload naming rung/script/reason and the
        // ids of both sides.
        let facts: Vec<WorkflowEvent> = store
            .events(task_id, None)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == event_kind::DECISIONS_BACKFILLED)
            .collect();
        assert_eq!(facts.len(), 3, "one event per group: {facts:?}");
        for f in &facts {
            assert_eq!(
                f.actor,
                format!("{MIGRATION_ACTOR_PREFIX}005_decisions_backfill")
            );
            assert_eq!(f.payload[json_field::BACKFILL], "005_decisions_backfill");
            assert_eq!(
                f.payload[json_field::SCRIPT_PATH],
                "migrations/005_decisions_backfill.sql"
            );
            assert!(
                f.payload[json_field::REASON]
                    .as_str()
                    .unwrap()
                    .contains("VERBATIM"),
                "the why is recorded, not silent"
            );
        }
        let appr_fact = facts
            .iter()
            .find(|f| {
                f.payload[json_field::KIND] == serde_json::json!(event_kind::REQUIREMENTS_APPROVED)
            })
            .unwrap();
        assert_eq!(
            appr_fact.payload[json_field::SOURCE_EVENTS]
                .as_array()
                .unwrap()
                .len(),
            2,
            "e-appr AND e-null were extracted for the approve group"
        );
        assert_eq!(
            appr_fact.payload[json_field::DECISION_IDS]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        // REQ-BF-003: the source events are untouched history.
        assert_eq!(
            payload_of(&store, &e_appr),
            r#"{"requirements_seq":2,"rationale":"OQ1: keep `task active`, do NOT ship `exec list --active`"}"#
        );
        assert_eq!(
            payload_of(&store, &e_modern),
            r#"{"rationale":"already a stored decision"}"#
        );

        // Idempotence (REQ-BF-005, ratified OQ2): force the version back,
        // re-open — the deterministic ids recompute identically, the ON
        // CONFLICT DO NOTHING inserts affect no row, no group produces a
        // fact event.
        drop(store);
        {
            let conn = Connection::open(dir.path().join("devai.db")).unwrap();
            conn.execute_batch("PRAGMA user_version=3;").unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(
            store.list_decisions(task_id).unwrap().len(),
            5,
            "a re-applied rung creates no decision row"
        );
        assert_eq!(
            store
                .events(task_id, None)
                .unwrap()
                .iter()
                .filter(|e| e.kind == event_kind::DECISIONS_BACKFILLED)
                .count(),
            3,
            "a re-applied rung creates no fact event"
        );
    }

    /// The ratified OQ2 mechanism, pinned: the backfill id is a pure
    /// function of the source event id, lands in the UUIDv5 version space
    /// the partial unique index predicates on, never aliases another
    /// event's id, and never collides with the live v7 id space.
    #[test]
    fn backfill_decision_id_is_deterministic_and_lives_in_the_indexed_space() {
        let a = new_id().to_string();
        let b = new_id().to_string();
        assert_eq!(
            backfill_decision_id(&a),
            backfill_decision_id(&a),
            "the same event recomputes the same id"
        );
        assert_ne!(
            backfill_decision_id(&a),
            backfill_decision_id(&b),
            "distinct events get distinct ids"
        );
        let id = backfill_decision_id(&a).to_string();
        assert_eq!(
            &id[14..15],
            "5",
            "backfilled ids enter the v5 space the index selects (substr(id,15,1)='5')"
        );
        assert_eq!(
            &new_id().to_string()[14..15],
            "7",
            "live ids stay OUTSIDE that space — the index never constrains the write-path"
        );
    }

    /// REQ-BF-007 (clean database): no candidate events → zero rows, zero
    /// fact events — the 003 "already honest" precedent.
    #[test]
    fn decisions_backfill_on_clean_database_writes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let task_id;
        {
            let store = Store::open(dir.path()).unwrap();
            let t = Task::new("clean", "fixture", None, Utc::now());
            store.create_task(&t).unwrap();
            task_id = t.id;
            store.conn.execute_batch("PRAGMA user_version=3;").unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        assert!(store.list_decisions(task_id).unwrap().is_empty());
        assert_eq!(
            store
                .events(task_id, None)
                .unwrap()
                .iter()
                .filter(|e| e.kind == event_kind::DECISIONS_BACKFILLED)
                .count(),
            0
        );
    }

    /// REQ-BF-003 AC3 (atomicity, the 003 precedent): if the fact-event
    /// write fails, the created decision rows, the DDL and the version bump
    /// roll back together — a half-backfilled database is impossible; a retry
    /// completes exactly once.
    #[test]
    fn decisions_backfill_is_atomic_when_the_event_write_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let tid: String;
        let e_a = new_id().to_string();
        {
            let store = Store::open(dir.path()).unwrap();
            let t = Task::new("atomic", "fixture", None, Utc::now());
            store.create_task(&t).unwrap();
            tid = t.id.to_string();
            seed_event(
                &store,
                &e_a,
                &tid,
                event_kind::REQUIREMENTS_APPROVED,
                "human:legacy",
                r#"{"rationale":"keep task active"}"#,
                "2026-10-01T00:00:00.000000Z",
            );
            // Block every event INSERT — the rung must fail AFTER its rows.
            store.conn.execute_batch("CREATE TRIGGER test_fail_event_insert BEFORE INSERT ON workflow_events BEGIN SELECT RAISE(ABORT,'boom'); END;").unwrap();
            // Drop the fresh-install copies of the rung's indexes so the
            // rollback check below observes the RUNG's DDL, not schema.sql's.
            store.conn.execute_batch("DROP INDEX decisions_task; DROP INDEX decisions_backfill_source_event; PRAGMA user_version=4;").unwrap();
        }
        assert!(
            Store::open(dir.path()).is_err(),
            "a mid-rung failure must surface"
        );
        {
            let conn = Connection::open(dir.path().join("devai.db")).unwrap();
            let n: i64 = conn
                .query_row("SELECT count(*) FROM decisions", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "the rolled-back rung left no decision row");
            let v: i64 = conn
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
            // After renumber the decisions rung is 005 (4→5): the ladder first
            // completes 004_task_dependencies, and the mid-rung failure rolls
            // back to the FAILED rung's `from` — 4, not 3.
            assert_eq!(v, 4, "the version bump rolled back with the rows");
            let idx: i64 = conn
                .query_row("SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN ('decisions_task','decisions_backfill_source_event')", [], |r| r.get(0))
                .unwrap();
            assert_eq!(idx, 0, "the rung's DDL rolled back too");
            conn.execute_batch("DROP TRIGGER test_fail_event_insert;")
                .unwrap();
        }
        // With writes restored, the next open completes the backfill once.
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT count(*) FROM decisions WHERE refs LIKE '%' || ?1 || '%'",
                    params![e_a],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1,
            "the retry created exactly one row"
        );
        assert_eq!(
            store
                .conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT count(*) FROM decisions WHERE id=?1",
                    params![backfill_decision_id(&e_a).to_string()],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1,
            "the retried row carries the SAME deterministic id — the retry cannot fork a second row for one event"
        );
        let idx: i64 = store
            .conn
            .query_row("SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN ('decisions_task','decisions_backfill_source_event')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(idx, 2, "the completed retry landed the rung's full DDL");
    }
}

#[cfg(test)]
mod single_writer_tests {
    use super::*;

    /// A full `RoleExecution` literal with overridable fields — every guard
    /// test starts from the same honest row shape.
    fn exec_for(task_id: Id, started_at: chrono::DateTime<Utc>) -> RoleExecution {
        RoleExecution {
            id: new_id(),
            task_id,
            role_slug: "implementation-agent".into(),
            role_version: 1,
            agent_name: Some("mock".into()),
            status: ExecutionStatus::Running,
            inputs: vec![],
            summary: None,
            error: None,
            started_at,
            finished_at: None,
            workspace_revision: None,
            attempt: 1,
            parent_execution: None,
            failure_class: None,
            owner_pid: None,
            owner_host: None,
            owner_process_start: None,
            bundle: None,
        }
    }

    fn task(store: &Store, name: &str) -> Id {
        let t = Task::new(name, "fixture", None, Utc::now());
        store.create_task(&t).unwrap();
        t.id
    }

    /// The v7 index as the database sees it (REQ-SW-01: the invariant lives
    /// in the schema, not only in code).
    fn index_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='index' AND name='role_executions_single_writer'", [], |r| r.get(0)).unwrap()
    }

    /// Pretend the store never saw rung 007: the index and the version
    /// stamp ARE the rung's state change, so dropping the index and walking
    /// `user_version` back makes duplicate RUNNING history storable exactly
    /// as it was pre-v7. The owner columns stay — their ADD on a genuinely
    /// old table shape is what the v1→v7 ladder of `migration_tests` walks
    /// end-to-end; dropping/re-adding them here would only re-test the
    /// column-skip path that re-application asserts directly below.
    fn downgrade_to_v6(store: &Store) {
        store
            .conn
            .execute_batch("DROP INDEX role_executions_single_writer; PRAGMA user_version=6;")
            .unwrap();
    }

    /// Seed a RUNNING row at an explicit started_at on an old-shape database
    /// (direct SQL — the v7 write path could not even address such a table).
    fn seed_running_v6(store: &Store, id: &str, task_id: &str, started_at: &str) {
        store
            .conn
            .execute(
                "INSERT INTO role_executions(id,task_id,role_slug,role_version,status,started_at) VALUES(?1,?2,'implementation-agent',1,'RUNNING',?3)",
                params![id, task_id, started_at],
            )
            .unwrap();
    }

    /// REQ-SW-01 AC1: the partial unique index exists on a fresh store and
    /// rejects a second RUNNING row for one task_id AT THE DATABASE LEVEL —
    /// raw SQL, bypassing every mapping in this crate.
    #[test]
    fn the_index_itself_rejects_a_second_running_row() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(
            index_count(&store.conn),
            1,
            "the guard index ships in schema.sql"
        );
        let t = task(&store, "index");
        store.start_execution(&exec_for(t, Utc::now())).unwrap();
        let raw = store.conn.execute(
            "INSERT INTO role_executions(id,task_id,role_slug,role_version,status,started_at) VALUES('x2',?1,'implementation-agent',1,'RUNNING','2026-11-01T00:00:00.000000Z')",
            params![t.to_string()],
        );
        let err = raw.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("UNIQUE constraint failed: role_executions.task_id"),
            "the DATABASE refuses, not the code: {msg}"
        );
    }

    /// REQ-SW-01/02 + OQ-2/REQ-SW-03: the named error cites the blocking
    /// row and the task; the owner columns round-trip through the reader;
    /// closing the row re-opens the slot (sequential starts stay legal —
    /// the form-loop shape, REQ-SW-01 AC4).
    #[test]
    fn second_running_start_fails_loud_naming_the_live_row() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t = task(&store, "busy");
        let mut first = exec_for(t, Utc::now());
        first.owner_pid = Some(4242);
        first.owner_host = Some("fixture-host".into());
        first.owner_process_start = Some(1_730_000_000);
        store.start_execution(&first).unwrap();

        // REQ-SW-03: owner identity travels through the reader unchanged.
        let read = store.get_execution(&first.id.to_string()).unwrap();
        assert_eq!(read.owner_pid, Some(4242));
        assert_eq!(read.owner_host.as_deref(), Some("fixture-host"));
        assert_eq!(read.owner_process_start, Some(1_730_000_000));

        // REQ-SW-02: the second start yields the NAMED variant, never
        // an opaque Sql error, and it names both ids.
        let err = store.start_execution(&exec_for(t, Utc::now())).unwrap_err();
        match &err {
            StoreError::SingleWriterBusy {
                task_id,
                existing_id,
                owner_pid,
                owner_host,
                ..
            } => {
                assert_eq!(*task_id, t.to_string(), "the busy task is named");
                assert_eq!(
                    *existing_id,
                    first.id.to_string(),
                    "the LIVE row is named by full id"
                );
                assert_eq!(
                    *owner_pid,
                    Some(4242),
                    "the blocker's owner is cited for triage"
                );
                assert_eq!(owner_host.as_deref(), Some("fixture-host"));
            }
            other => panic!("expected SingleWriterBusy, got {other:?}"),
        }
        assert!(
            err.to_string().contains("single-writer guard"),
            "the message is self-describing: {err}"
        );

        // Closing the live row re-opens the writer slot — one RUNNING per
        // task is the invariant, not one-ever (AC4: attempts are sequential).
        store
            .finish_execution(
                &first,
                ExecutionStatus::Completed,
                Some("done"),
                None,
                Utc::now(),
            )
            .unwrap();
        let second = exec_for(t, Utc::now());
        store.start_execution(&second).unwrap();
        assert!(
            store.start_execution(&exec_for(t, Utc::now())).is_err(),
            "and the new RUNNING row blocks again"
        );
    }

    /// REQ-SW-03 AC2 / OQ-2: rows written without owner columns (direct
    /// store users, pre-v7 history) read as ALL-NULL owner — «unknown»,
    /// never a fabricated identity.
    #[test]
    fn rows_without_an_owner_read_as_unknown_not_dead() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t = task(&store, "ownerless");
        store.start_execution(&exec_for(t, Utc::now())).unwrap();
        let e = store.list_executions(Some(t)).unwrap().pop().unwrap();
        assert_eq!(
            (e.owner_pid, e.owner_host.clone(), e.owner_process_start),
            (None, None, None)
        );
    }

    /// REQ-SW-07 + ratified OQ-1: opening a v6-shaped store that already
    /// holds duplicate RUNNING rows heals it atomically — newest
    /// started_at keeps the slot, the rest close FAILED carrying the
    /// 'single-writer migration' marker, one fact event names both sides,
    /// the index lands, the ladder completes. Untouched rows (a clean
    /// task, a COMPLETED history row) stay byte-stable.
    #[test]
    fn migration_closes_losers_keeps_winner_and_records_the_repair() {
        let dir = tempfile::TempDir::new().unwrap();
        let (ta, tb);
        let (old1, old2, winner, untouched, clean_row);
        {
            let store = Store::open(dir.path()).unwrap();
            ta = task(&store, "duplicated");
            tb = task(&store, "clean");
            let (ta_s, tb_s) = (ta.to_string(), tb.to_string());
            old1 = new_id().to_string();
            old2 = new_id().to_string();
            winner = new_id().to_string();
            untouched = new_id().to_string();
            clean_row = new_id().to_string();
            // First become the OLD store, then write the pre-v7 history:
            // duplicates are storable only where the guard does not yet
            // exist — nothing forbade these rows then, and the repair must
            // not treat them as corrupt.
            downgrade_to_v6(&store);
            seed_running_v6(&store, &old1, &ta_s, "2026-10-01T00:00:00.000000Z");
            seed_running_v6(&store, &old2, &ta_s, "2026-10-02T00:00:00.000000Z");
            seed_running_v6(&store, &winner, &ta_s, "2026-10-03T00:00:00.000000Z");
            seed_running_v6(&store, &untouched, &ta_s, "2026-09-30T00:00:00.000000Z");
            // A legitimately closed history row: terminal BEFORE the repair,
            // with its own close time — the repair must leave it byte-stable.
            store.conn.execute("UPDATE role_executions SET status='COMPLETED', finished_at='2026-10-01T00:00:00.000000Z' WHERE id=?1", params![&untouched]).unwrap();
            seed_running_v6(&store, &clean_row, &tb_s, "2026-10-01T00:00:00.000000Z");
        }
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(
            store
                .conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION,
            "open runs the ladder to v7"
        );
        assert_eq!(
            index_count(&store.conn),
            1,
            "the guard index landed after the closure"
        );

        let rows = store.list_executions(Some(ta)).unwrap();
        let by = |id: &str| {
            rows.iter()
                .find(|e| e.id.to_string() == id)
                .unwrap_or_else(|| panic!("row {id} missing from {rows:?}"))
        };
        assert_eq!(
            by(&winner).status,
            ExecutionStatus::Running,
            "the NEWEST started_at keeps the single-writer slot (OQ-1)"
        );
        for loser in [&old1, &old2] {
            let e = by(loser);
            assert_eq!(
                e.status,
                ExecutionStatus::Failed,
                "the older duplicate closes FAILED (OQ-1)"
            );
            assert!(
                e.error
                    .as_deref()
                    .unwrap()
                    .starts_with("single-writer migration"),
                "the explicit marker is in the row itself: {:?}",
                e.error
            );
            assert!(
                e.finished_at.is_some(),
                "a closed row carries its close time"
            );
        }
        let e = by(&untouched);
        assert_eq!(
            e.status,
            ExecutionStatus::Completed,
            "terminal history is never touched"
        );
        assert!(
            e.error.is_none(),
            "the repair wrote no marker into an unrelated row"
        );
        assert_eq!(
            e.finished_at
                .map(|f| f.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
                .as_deref(),
            Some("2026-10-01T00:00:00.000000Z"),
            "its close time is byte-stable — the repair rewrote nothing"
        );

        // The clean task keeps its RUNNING row and gets NO repair event.
        let clean = store.list_executions(Some(tb)).unwrap();
        assert_eq!(
            clean[0].status,
            ExecutionStatus::Running,
            "a task without duplicates is untouched"
        );
        // One fact event for the duplicated task, naming both sides (003's
        // observe-before-mutate discipline).
        let facts = store
            .events(ta, None)
            .unwrap()
            .into_iter()
            .filter(|ev| ev.kind == event_kind::EXECUTION_SINGLE_WRITER_REPAIR)
            .collect::<Vec<_>>();
        assert_eq!(
            facts.len(),
            1,
            "one repair event per duplicated task: {facts:?}"
        );
        let f = &facts[0];
        assert_eq!(
            f.actor,
            format!("{MIGRATION_ACTOR_PREFIX}007_single_writer_guard")
        );
        assert_eq!(
            f.payload[json_field::BACKFILL],
            serde_json::json!("007_single_writer_guard")
        );
        assert_eq!(
            f.payload[json_field::SCRIPT_PATH],
            serde_json::json!("migrations/007_single_writer_guard.sql")
        );
        assert_eq!(
            f.payload["kept_running"],
            serde_json::json!(winner),
            "the survivor is cited by id"
        );
        let closed: Vec<String> = f.payload["closed_executions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            closed,
            vec![old1.clone(), old2.clone()],
            "every closed row is cited, in loser order"
        );
        assert!(
            f.payload[json_field::REASON]
                .as_str()
                .unwrap()
                .contains("single-writer migration"),
            "the why is recorded, not silent"
        );
        assert_eq!(
            store
                .events(tb, None)
                .unwrap()
                .iter()
                .filter(|ev| ev.kind == event_kind::EXECUTION_SINGLE_WRITER_REPAIR)
                .count(),
            0,
            "a clean database produces zero events (the 003 precedent)"
        );
    }

    /// OQ-1 tie rule: equal started_at resolves on the UUIDv7 id order
    /// (lexicographic id IS time order) — same tie discipline as rung 003.
    #[test]
    fn equal_started_at_breaks_on_the_id_order() {
        let dir = tempfile::TempDir::new().unwrap();
        let ta;
        let (lo, hi);
        {
            let store = Store::open(dir.path()).unwrap();
            ta = task(&store, "tie");
            let ta_s = ta.to_string();
            lo = new_id().to_string();
            hi = new_id().to_string();
            let at = "2026-10-01T00:00:00.000000Z";
            downgrade_to_v6(&store);
            seed_running_v6(&store, &lo, &ta_s, at);
            seed_running_v6(&store, &hi, &ta_s, at);
        }
        let (winner, loser) = if lo > hi {
            (lo.clone(), hi.clone())
        } else {
            (hi.clone(), lo.clone())
        };
        let store = Store::open(dir.path()).unwrap();
        let rows = store.list_executions(Some(ta)).unwrap();
        let by = |id: &str| rows.iter().find(|e| e.id.to_string() == id).unwrap();
        assert_eq!(
            by(&winner).status,
            ExecutionStatus::Running,
            "the lexicographically greater id wins the tie"
        );
        assert_eq!(by(&loser).status, ExecutionStatus::Failed);
    }

    /// REQ-SW-07 idempotence: a re-applied rung 007 (version forced back)
    /// matches zero duplicates, closes zero rows, writes zero events — the
    /// repair cannot run twice on one history.
    #[test]
    fn reapplying_the_rung_on_a_healed_store_writes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let ta;
        {
            let store = Store::open(dir.path()).unwrap();
            ta = task(&store, "twice");
            downgrade_to_v6(&store);
            seed_running_v6(
                &store,
                &new_id().to_string(),
                &ta.to_string(),
                "2026-10-01T00:00:00.000000Z",
            );
        }
        let store = Store::open(dir.path()).unwrap(); // heals
        assert_eq!(
            store
                .events(ta, None)
                .unwrap()
                .iter()
                .filter(|e| e.kind == event_kind::EXECUTION_SINGLE_WRITER_REPAIR)
                .count(),
            0,
            "a single RUNNING row was never a duplicate"
        );
        store.conn.execute_batch("PRAGMA user_version=6;").unwrap();
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(
            store
                .conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION,
            "the re-applied rung still completes the ladder"
        );
        assert_eq!(
            store
                .events(ta, None)
                .unwrap()
                .iter()
                .filter(|e| e.kind == event_kind::EXECUTION_SINGLE_WRITER_REPAIR)
                .count(),
            0,
            "re-application writes no event"
        );
        assert_eq!(
            index_count(&store.conn),
            1,
            "the IF NOT EXISTS DDL stays idempotent"
        );
    }

    /// The full duplicate→repair→re-apply→verify loop on a store that also
    /// carries a live v7 row afterwards: once healed, the guard is ACTIVE —
    /// a second start on the kept row's task hits SingleWriterBusy.
    #[test]
    fn a_healed_store_enforces_the_guard_immediately() {
        let dir = tempfile::TempDir::new().unwrap();
        let ta;
        {
            let store = Store::open(dir.path()).unwrap();
            ta = task(&store, "healed-then-live");
            let winner = new_id().to_string();
            downgrade_to_v6(&store);
            seed_running_v6(
                &store,
                &new_id().to_string(),
                &ta.to_string(),
                "2026-10-01T00:00:00.000000Z",
            );
            seed_running_v6(
                &store,
                &winner,
                &ta.to_string(),
                "2026-10-02T00:00:00.000000Z",
            );
        }
        let store = Store::open(dir.path()).unwrap();
        let survivor = store
            .list_executions(Some(ta))
            .unwrap()
            .into_iter()
            .find(|e| e.status == ExecutionStatus::Running)
            .unwrap();
        let err = store
            .start_execution(&exec_for(ta, Utc::now()))
            .unwrap_err();
        match &err {
            StoreError::SingleWriterBusy { existing_id, .. } => assert_eq!(
                *existing_id,
                survivor.id.to_string(),
                "the kept winner is the named blocker"
            ),
            other => panic!("expected SingleWriterBusy, got {other:?}"),
        }
    }

    /// REQ-HOME-09: an observation open never migrates and never creates.
    /// A lagging store (here: rank 7, the shape the tripsales home really
    /// carries) opens as it is — rows readable, `user_version` untouched,
    /// no blobs directory materialised, the ladder never runs.
    #[test]
    fn read_only_open_never_migrates_and_never_creates() {
        let dir = tempfile::TempDir::new().unwrap();
        let lagging = dir.path().join("lagging");
        std::fs::create_dir_all(&lagging).unwrap();
        {
            let conn = Connection::open(lagging.join("devai.db")).unwrap();
            conn.execute_batch(
                "PRAGMA user_version=7;
                 CREATE TABLE projects(id TEXT PRIMARY KEY, name TEXT NOT NULL,
                   description TEXT NOT NULL DEFAULT '', repos TEXT NOT NULL DEFAULT '[]',
                   created_at TEXT NOT NULL, updated_at TEXT NOT NULL);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO projects(id,name,created_at,updated_at) VALUES('p1','lagging','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        }
        let store = Store::open_read_only(&lagging).unwrap();
        assert_eq!(store.schema_version().unwrap(), 7);
        let n: i64 = store
            .conn
            .query_row("SELECT count(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "rows of the lagging home are readable");
        drop(store);
        let conn =
            Connection::open_with_flags(lagging.join("devai.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            v, 7,
            "open_read_only never runs the ladder or bumps the rank"
        );
        assert!(
            !lagging.join("blobs").exists(),
            "the observer creates no directories inside the observed home"
        );
    }

    /// REQ-HOME-09: a store whose rank is AHEAD of this binary is
    /// observable — never a `SchemaVersion` startup failure. The observer
    /// reports the rank; stamping the cause is the caller's job.
    #[test]
    fn read_only_open_tolerates_a_newer_store() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("ahead");
        std::fs::create_dir_all(&root).unwrap();
        {
            // Store::open would refuse this rank (SchemaVersion); the
            // hand-built db proves the reader is gate-free.
            let conn = Connection::open(root.join("devai.db")).unwrap();
            conn.execute_batch(
                "PRAGMA user_version=9999; CREATE TABLE projects(id TEXT PRIMARY KEY);",
            )
            .unwrap();
        }
        let store = Store::open_read_only(&root).unwrap();
        assert_eq!(store.schema_version().unwrap(), 9999);
    }

    /// The observation open never creates a home: an absent store file is
    /// a typed `NotFound`, not a silent initialization (REQ-HOME-03 AC3
    /// feeds exactly this refusal to the add-dialog error path).
    #[test]
    fn read_only_open_refuses_missing_store_and_schemaless_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("nowhere");
        assert!(matches!(
            Store::open_read_only(&missing),
            Err(StoreError::NotFound(..))
        ));
        let naked = dir.path().join("naked");
        std::fs::create_dir_all(&naked).unwrap();
        {
            let conn = Connection::open(naked.join("devai.db")).unwrap();
            conn.execute_batch("CREATE TABLE unrelated(x);").unwrap();
            drop(conn);
        }
        assert!(matches!(
            Store::open_read_only(&naked),
            Err(StoreError::NotFound(..))
        ));
        assert!(
            std::fs::read_dir(&naked)
                .unwrap()
                .filter_map(|e| e.ok())
                .any(|e| e.file_name() == "devai.db"),
            "the probe ran while the file stayed exactly where it was"
        );
    }
}
