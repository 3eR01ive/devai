//! Portable task bundle: export/import of everything that constitutes a
//! task's domain state — workflow snapshot, event history, requirements
//! versions, role/contract definitions, executions, artifacts and their
//! dependency graph. Deliberately excludes caches, model context and
//! credentials: a task must continue from domain data alone.
//!
//! Bundle format v2 (schema v4): adds `dependencies.jsonl` — the task's
//! ACTIVE OUTGOING typed dependency edges (REQ-01 AC3: dependencies travel
//! with the bundle like every other table; SUPERSEDED rows are local
//! history and do not — the live graph restores, the audit trail stays on
//! its machine). A v1 bundle simply carries no such file and imports
//! unchanged. Edges reference the depended-on task BY ID,
//! so importing a bundle whose blocker/parent is not yet in the target
//! store fails LOUDLY on the FK and rolls the whole import back — import
//! the blocker's bundle first; a dangling graph is never silently truncated.
//!
//! Bundle layout (directory):
//! ```text
//! manifest.json      — format version + sha256/size of every file
//! task.json          — task snapshot
//! events.jsonl       — immutable workflow history (append-only)
//! artifacts.jsonl    — artifact metadata rows
//! blobs/<sha256>     — artifact contents, content-addressed
//! executions.jsonl   — role executions incl. their recorded workspace_revision
//! questions.jsonl
//! decisions.jsonl
//! dependencies.jsonl — outgoing task-dependency edges (v2; typed, attributed)
//! roles.jsonl        — role definitions referenced by executions
//! contracts.jsonl    — contracts referenced by artifacts/roles
//! profiles.jsonl     — evaluation profile definitions referenced by
//!                      recorded revisions (name@version -> raw YAML body)
//! ```

use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use devai_core::types::*;

use crate::error::{StoreError, StoreResult};
use crate::{SCHEMA_VERSION, Store, hex_encode, rows};

pub const BUNDLE_FORMAT: &str = "devai-bundle";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BundleManifest {
    pub format: String,
    pub bundle_version: u32,
    pub schema_version: i64,
    pub exported_at: String,
    pub task_id: Id,
    pub task_title: String,
    pub counts: BundleCounts,
    pub files: Vec<FileEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BundleCounts {
    pub events: usize,
    pub artifacts: usize,
    pub executions: usize,
    pub questions: usize,
    pub decisions: usize,
    pub roles: usize,
    pub contracts: usize,
    /// Evaluation profile definitions referenced by recorded revisions.
    /// `default` keeps pre-v2 manifests readable.
    #[serde(default)]
    pub profiles: usize,
    /// Outgoing dependency edges of the task (bundle v2 / schema v4).
    /// `default` keeps pre-v2 manifests readable.
    #[serde(default)]
    pub dependencies: usize,
}

#[derive(Clone, Debug)]
pub struct ImportOutcome {
    pub task_id: Id,
    pub imported_artifacts: usize,
    pub imported_events: usize,
    pub skipped_existing: bool,
}

pub fn export(store: &Store, task: &Task, out_dir: &Path) -> StoreResult<BundleManifest> {
    std::fs::create_dir_all(out_dir)?;
    let mut files: Vec<FileEntry> = Vec::new();

    write_json(store.root(), out_dir, "task.json", task, &mut files)?;

    let events = store.all_events_for_task(task.id)?;
    write_jsonl(out_dir, "events.jsonl", &events, &mut files)?;

    let artifacts = store.artifacts(task.id)?;
    write_jsonl(out_dir, "artifacts.jsonl", &artifacts, &mut files)?;

    let executions = store.list_executions(Some(task.id))?;
    write_jsonl(out_dir, "executions.jsonl", &executions, &mut files)?;

    let questions = store.list_questions(task.id)?;
    write_jsonl(out_dir, "questions.jsonl", &questions, &mut files)?;

    let decisions = store.list_decisions(task.id)?;
    write_jsonl(out_dir, "decisions.jsonl", &decisions, &mut files)?;

    // REQ-01 AC3: the dependency edges are domain data of this task —
    // exported like every other table. Outgoing edges only: each task's
    // bundle carries the edges IT declared; incoming ones travel inside
    // the other task's bundle, so no edge is exported twice.
    let dependencies = store.task_dependencies(task.id)?;
    write_jsonl(out_dir, "dependencies.jsonl", &dependencies, &mut files)?;

    // Blobs of all exported artifacts (content-addressed copies).
    let mut seen = std::collections::HashSet::new();
    for a in &artifacts {
        if !seen.insert(a.sha256.clone()) {
            continue;
        }
        let content = store.read_blob(&a.sha256).map_err(|e| match e {
            StoreError::Io(_) => StoreError::Corrupt(format!("artifact {} blob missing: {}", a.id, a.sha256)),
            other => other,
        })?;
        let rel = format!("blobs/{}", a.sha256);
        let abs = out_dir.join(&rel);
        std::fs::create_dir_all(abs.parent().unwrap())?;
        std::fs::write(&abs, &content)?;
        files.push(FileEntry { path: rel, sha256: hex_encode(&Sha256::digest(&content)), size: content.len() as u64 });
    }

    // Referenced role definitions (slug@version per execution).
    let mut roles: Vec<RoleDefinition> = Vec::new();
    let mut contract_refs: Vec<(String, u32)> = Vec::new();
    for a in &artifacts {
        if let Some(c) = &a.contract {
            contract_refs.push((c.name.clone(), c.version));
        }
    }
    for e in &executions {
        if let Some(r) = get_role(store, &e.role_slug, Some(e.role_version))? {
            push_role(&mut roles, r, &mut contract_refs);
        }
    }
    write_jsonl(out_dir, "roles.jsonl", &roles, &mut files)?;

    let mut contracts: Vec<ContractEntry> = Vec::new();
    for (name, version) in contract_refs.iter().cloned() {
        if contracts.iter().any(|c| c.name == name && c.version == version) {
            continue;
        }
        if let Some(schema) = store.get_contract(&name, version)? {
            contracts.push(ContractEntry { name, version, schema });
        }
    }
    write_jsonl(out_dir, "contracts.jsonl", &contracts, &mut files)?;

    // Evaluation profile definitions referenced by recorded workspace
    // revisions: a bundle must re-resolve every `name@version` its history
    // cites, even where local configs have since advanced.
    let mut profiles: Vec<ProfileEntry> = Vec::new();
    for e in &executions {
        let Some(ident) = e.workspace_revision.as_ref().and_then(|r| r.profile.clone()) else { continue };
        let Some((name, version)) = ident.rsplit_once('@') else { continue };
        let Ok(version) = version.parse::<u32>() else { continue };
        if profiles.iter().any(|p| p.name == name && p.version == version) {
            continue;
        }
        if let Some((project, body)) = store.get_evaluation_profile(name, version)? {
            profiles.push(ProfileEntry { name: name.into(), version, project, body });
        }
    }
    write_jsonl(out_dir, "profiles.jsonl", &profiles, &mut files)?;

    let manifest = BundleManifest {
        format: BUNDLE_FORMAT.into(),
        // v2: `dependencies.jsonl` added (schema v4). A v1 bundle (no such
        // file) still imports: the reader treats the missing file as an
        // empty edge set, exactly like the missing pre-v2 profiles file.
        bundle_version: 2,
        schema_version: SCHEMA_VERSION,
        exported_at: Utc::now().to_rfc3339(),
        task_id: task.id,
        task_title: task.title.clone(),
        counts: BundleCounts {
            events: events.len(),
            artifacts: artifacts.len(),
            executions: executions.len(),
            questions: questions.len(),
            decisions: decisions.len(),
            roles: roles.len(),
            contracts: contracts.len(),
            profiles: profiles.len(),
            dependencies: dependencies.len(),
        },
        files,
    };
    let abs = out_dir.join("manifest.json");
    let body = serde_json::to_vec_pretty(&manifest).map_err(|e| StoreError::Corrupt(e.to_string()))?;
    std::fs::write(&abs, &body)?;

    store.insert_event(
        Some(task.id),
        event_kind::EXPORTED,
        "user",
        serde_json::json!({ "bundle": out_dir.display().to_string() }),
    )?;
    Ok(manifest)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ContractEntry {
    pub name: String,
    pub version: u32,
    pub schema: String,
}

/// One evaluation profile definition, carried so a recorded `name@version`
/// stays resolvable after import on a machine whose local config advanced.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProfileEntry {
    pub name: String,
    pub version: u32,
    pub project: String,
    pub body: String,
}

fn push_role(roles: &mut Vec<RoleDefinition>, r: RoleDefinition, contract_refs: &mut Vec<(String, u32)>) {
    if roles.iter().any(|x| x.slug == r.slug && x.version == r.version) {
        return;
    }
    if let Some(c) = &r.input_contract {
        contract_refs.push((c.name.clone(), c.version));
    }
    if let Some(c) = &r.output_contract {
        contract_refs.push((c.name.clone(), c.version));
    }
    roles.push(r);
}

fn get_role(store: &Store, slug: &str, version: Option<u32>) -> StoreResult<Option<RoleDefinition>> {
    let sql = match version {
        Some(_) => "SELECT id,slug,version,name,purpose,instructions,tools,input_contract,output_contract,done_criteria,created_at FROM role_definitions WHERE slug=?1 AND version=?2",
        None => "SELECT id,slug,version,name,purpose,instructions,tools,input_contract,output_contract,done_criteria,created_at FROM role_definitions WHERE slug=?1 ORDER BY version DESC LIMIT 1",
    };
    let v = version.map(|x| x.to_string());
    store
        .conn
        .query_row(sql, rusqlite::params_from_iter([Some(slug.to_string()), v]), |r| {
            rows::role_from_row(r).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
        })
        .optional()
        .map_err(|e| match e {
            rusqlite::Error::ToSqlConversionFailure(b) => b.downcast::<StoreError>().map(|e| *e).unwrap_or(StoreError::Sql(rusqlite::Error::InvalidQuery)),
            other => StoreError::Sql(other),
        })
}

pub fn import(store: &Store, dir: &Path) -> StoreResult<ImportOutcome> {
    let manifest_raw = std::fs::read(dir.join("manifest.json"))?;
    let manifest: BundleManifest = serde_json::from_slice(&manifest_raw).map_err(|e| StoreError::Corrupt(format!("bad manifest: {e}")))?;
    if manifest.format != BUNDLE_FORMAT {
        return Err(StoreError::Corrupt(format!("not a devai bundle: format `{}`", manifest.format)));
    }
    // Forward/backward compatibility: a bundle from an OLDER store schema is
    // acceptable (its rows just lack the nullable workspace_revision; the
    // new table starts empty), but a bundle from a FUTURE schema may carry
    // columns we cannot store, so it is refused rather than silently truncated.
    if manifest.schema_version > SCHEMA_VERSION {
        return Err(StoreError::SchemaVersion { found: manifest.schema_version, expected: SCHEMA_VERSION });
    }

    // Verify every listed file against its recorded checksum.
    for f in &manifest.files {
        let content = std::fs::read(dir.join(&f.path))?;
        if content.len() as u64 != f.size {
            return Err(StoreError::Corrupt(format!("{} size mismatch", f.path)));
        }
        if hex_encode(&Sha256::digest(&content)) != f.sha256 {
            return Err(StoreError::Corrupt(format!("{} checksum mismatch", f.path)));
        }
    }

    let task: Task = read_json(dir, "task.json")?;
    if store.has_task(&task.id.to_string())? {
        return Ok(ImportOutcome { task_id: task.id, imported_artifacts: 0, imported_events: 0, skipped_existing: true });
    }

    let events: Vec<WorkflowEvent> = read_jsonl(dir, "events.jsonl")?;
    let artifacts: Vec<Artifact> = read_jsonl(dir, "artifacts.jsonl")?;
    let executions: Vec<RoleExecution> = read_jsonl(dir, "executions.jsonl")?;
    let questions: Vec<Question> = read_jsonl(dir, "questions.jsonl")?;
    let decisions: Vec<Decision> = read_jsonl(dir, "decisions.jsonl")?;
    let roles: Vec<RoleDefinition> = read_jsonl(dir, "roles.jsonl")?;
    let contracts: Vec<ContractEntry> = read_jsonl(dir, "contracts.jsonl")?;
    // profiles.jsonl appeared with schema v2: a pre-v2 bundle simply carries
    // no profile definitions, and its executions carry no recorded revisions.
    let profiles: Vec<ProfileEntry> = if dir.join("profiles.jsonl").exists() { read_jsonl(dir, "profiles.jsonl")? } else { Vec::new() };
    // dependencies.jsonl appeared with schema v4 / bundle v2: a pre-v4 bundle
    // carries no edges — that is an empty dependency set, not corruption
    // (REQ-08 AC2: absent means empty, never broken).
    let dependencies: Vec<TaskDependency> = if dir.join("dependencies.jsonl").exists() { read_jsonl(dir, "dependencies.jsonl")? } else { Vec::new() };

    let tx = store.conn.unchecked_transaction()?;

    tx.execute(
        "INSERT INTO tasks(id,project_id,title,business_description,state,return_state,requirements_seq,lock_version,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        rusqlite::params![
            task.id.to_string(),
            task.project_id.map(|i| i.to_string()),
            task.title,
            task.business_description,
            task.state.as_str(),
            task.return_state.map(|s| s.as_str()),
            task.requirements_seq,
            task.lock_version as i64,
            rows::ts(&task.created_at),
            rows::ts(&task.updated_at),
        ],
    )?;

    for e in &events {
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            rusqlite::params![e.id.to_string(), e.task_id.map(|i| i.to_string()), e.kind, e.actor, e.payload.to_string(), rows::ts(&e.created_at)],
        )?;
    }

    for a in &artifacts {
        let rel = format!("blobs/{}", a.sha256);
        let content = std::fs::read(dir.join(&rel))?;
        if hex_encode(&Sha256::digest(&content)) != a.sha256 {
            return Err(StoreError::Corrupt(format!("blob {} failed verification on import", a.sha256)));
        }
        store.store_blob(&a.sha256, &content)?;
        // No `INSERT OR IGNORE` on this table: since v3 a second CURRENT
        // row for one (task_id, kind) is rejected by the single-CURRENT
        // partial unique index, and OR IGNORE would let it swallow that
        // collision silently — the losing row would vanish without a trace,
        // exactly the defect class this repair removes. Detect the
        // collision explicitly instead (earlier bundle rows AND this
        // store's own CURRENT row are visible from inside the tx) and fail
        // loudly with task/kind/seq: the whole import transaction rolls
        // back, leaving no partial state. A pre-v3 bundle carrying the
        // multi-CURRENT defect must be re-exported from the migrated store.
        // Design source: wip/pre-t2-merge@5db7fb5 bundle.rs.
        if a.status == ArtifactStatus::Current {
            let clash: Option<(String, u32)> = tx
                .query_row(
                    "SELECT id, seq FROM artifacts WHERE task_id=?1 AND kind=?2 AND status='CURRENT'",
                    rusqlite::params![a.task_id.to_string(), a.kind.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((existing_id, existing_seq)) = clash {
                return Err(StoreError::SingleCurrentCollision {
                    task_id: a.task_id.to_string(),
                    kind: a.kind.as_str().to_string(),
                    seq: a.seq,
                    existing_id,
                    existing_seq,
                });
            }
        }
        tx.execute(
            "INSERT INTO artifacts(id,task_id,kind,name,seq,category,status,sha256,size,media_type,contract_name,contract_version,created_by,execution_id,depends_on,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            rusqlite::params![
                a.id.to_string(), a.task_id.to_string(), a.kind.as_str(), a.name, a.seq,
                a.category.as_str(), a.status.as_str(), a.sha256, a.size as i64, a.media_type,
                a.contract.as_ref().map(|c| c.name.clone()),
                a.contract.as_ref().map(|c| c.version),
                a.created_by, a.execution_id.map(|i| i.to_string()),
                json_string(&a.depends_on), rows::ts(&a.created_at),
            ],
        )?;
    }

    for r in &roles {
        tx.execute(
            "INSERT OR IGNORE INTO role_definitions(id,slug,version,name,purpose,instructions,tools,input_contract,output_contract,done_criteria,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            rusqlite::params![
                r.id.to_string(), r.slug, r.version, r.name, r.purpose, r.instructions,
                json_string(&r.tools),
                r.input_contract.as_ref().map(|c| c.to_string()),
                r.output_contract.as_ref().map(|c| c.to_string()),
                r.done_criteria, rows::ts(&r.created_at),
            ],
        )?;
    }

    for c in &contracts {
        tx.execute(
            "INSERT OR IGNORE INTO contracts(name,version,schema,created_at) VALUES(?1,?2,?3,?4)",
            rusqlite::params![c.name, c.version, c.schema, rows::ts(&Utc::now())],
        )?;
    }

    // Insert-only, exactly like the local seed: an existing (name, version)
    // profile definition on this machine is never clobbered by an import.
    for p in &profiles {
        tx.execute(
            "INSERT OR IGNORE INTO evaluation_profiles(name,version,project,body,created_at) VALUES(?1,?2,?3,?4,?5)",
            rusqlite::params![p.name, p.version, p.project, p.body, rows::ts(&Utc::now())],
        )?;
    }

    for e in &executions {
        tx.execute(
            "INSERT OR IGNORE INTO role_executions(id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision,attempt,parent_execution,failure_class)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            rusqlite::params![
                e.id.to_string(), e.task_id.to_string(), e.role_slug, e.role_version, e.agent_name,
                e.status.as_str(), json_string(&e.inputs), e.summary, e.error,
                rows::ts(&e.started_at), e.finished_at.as_ref().map(rows::ts),
                e.workspace_revision.as_ref().map(|r| json_string(r)),
                // The form-loop columns travel with the row: an exported loop
                // history must still be reconstructible after import (v5).
                e.attempt as i64, e.parent_execution.map(|p| p.to_string()), e.failure_class,
            ],
        )?;
    }

    for q in &questions {
        tx.execute(
            "INSERT OR IGNORE INTO questions(id,task_id,execution_id,text,context,options,blocking,status,answer,answered_by,answered_at,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            rusqlite::params![
                q.id.to_string(), q.task_id.to_string(), q.execution_id.map(|i| i.to_string()),
                q.text, q.context, json_string(&q.options), q.blocking as i64, q.status.as_str(),
                q.answer, q.answered_by, q.answered_at.as_ref().map(rows::ts), rows::ts(&q.created_at),
            ],
        )?;
    }

    for d in &decisions {
        tx.execute(
            "INSERT OR IGNORE INTO decisions(id,task_id,kind,verdict,actor,rationale,refs,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                d.id.to_string(), d.task_id.to_string(), d.kind, d.verdict.as_str(), d.actor,
                d.rationale, json_string(&d.refs), rows::ts(&d.created_at),
            ],
        )?;
    }

    // REQ-01 AC3: the dependency edges restore inside the same transaction.
    // Plain INSERT (no OR IGNORE): the FK check is the loud integrity gate —
    // an edge whose depended-on task is not in the target store fails the
    // whole import and rolls it back, instead of silently truncating the
    // graph (a dangling dependency chain is exactly the defect class of
    // corrupt-on-read data loss; REQ-07 AC2 discipline at the bundle seam).
    for d in &dependencies {
        tx.execute(
            "INSERT INTO task_dependencies(id,task_id,depends_on_id,kind,actor,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            rusqlite::params![
                d.id.to_string(), d.task_id.to_string(), d.depends_on_id.to_string(),
                d.kind.as_str(), d.actor, rows::ts(&d.created_at),
            ],
        )?;
    }

    tx.execute(
        "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
        rusqlite::params![
            new_id().to_string(),
            task.id.to_string(),
            event_kind::IMPORTED,
            "user",
            serde_json::json!({ "bundle": dir.display().to_string() }).to_string(),
            rows::ts(&Utc::now()),
        ],
    )?;

    tx.commit()?;
    Ok(ImportOutcome {
        task_id: task.id,
        imported_artifacts: artifacts.len(),
        imported_events: events.len() + 1,
        skipped_existing: false,
    })
}

// ---------------------------------------------------------------------------
// tiny IO helpers
// ---------------------------------------------------------------------------

fn write_json<T: Serialize>(
    _root: &Path,
    dir: &Path,
    rel: &str,
    value: &T,
    files: &mut Vec<FileEntry>,
) -> StoreResult<()> {
    let body = serde_json::to_vec_pretty(value).map_err(|e| StoreError::Corrupt(e.to_string()))?;
    std::fs::write(dir.join(rel), &body)?;
    files.push(FileEntry { path: rel.into(), sha256: hex_encode(&Sha256::digest(&body)), size: body.len() as u64 });
    Ok(())
}

fn write_jsonl<T: Serialize>(dir: &Path, rel: &str, values: &[T], files: &mut Vec<FileEntry>) -> StoreResult<()> {
    let mut buf = Vec::new();
    for v in values {
        buf.extend(serde_json::to_vec(v).map_err(|e| StoreError::Corrupt(e.to_string()))?);
        buf.push(b'\n');
    }
    let parent: PathBuf = dir.join(rel);
    std::fs::create_dir_all(parent.parent().unwrap())?;
    std::fs::write(&parent, &buf)?;
    files.push(FileEntry { path: rel.into(), sha256: hex_encode(&Sha256::digest(&buf)), size: buf.len() as u64 });
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(dir: &Path, rel: &str) -> StoreResult<T> {
    let body = std::fs::read(dir.join(rel))?;
    serde_json::from_slice(&body).map_err(|e| StoreError::Corrupt(format!("{rel}: {e}")))
}

fn read_jsonl<T: serde::de::DeserializeOwned>(dir: &Path, rel: &str) -> StoreResult<Vec<T>> {
    let body = std::fs::read(dir.join(rel))?;
    let mut out = Vec::new();
    for line in body.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        out.push(serde_json::from_slice(line).map_err(|e| StoreError::Corrupt(format!("{rel}: {e}")))?);
    }
    Ok(out)
}
