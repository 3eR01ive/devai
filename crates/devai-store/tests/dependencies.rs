//! The task dependency graph as STORED platform data (REQ-01/02/07/08):
//! many-to-many typed edges surviving a restart, loud reference integrity
//! at write, additive v4 migration, and bundle export/restore parity with
//! the other tables.

use chrono::Utc;
use devai_core::types::*;
use devai_store::bundle;
use devai_store::Store;
use rusqlite::{Connection, params};
use tempfile::TempDir;

fn open() -> (TempDir, Store) {
    let d = TempDir::new().unwrap();
    let s = Store::open(d.path()).unwrap();
    (d, s)
}

fn task(store: &Store, title: &str) -> Task {
    let t = Task::new(title, "business need", None, Utc::now());
    store.create_task(&t).unwrap();
    t
}

/// A raw second connection to the same database file (WAL allows this):
/// the v-rung tests shape old-schema states the public API cannot make.
fn raw(dir: &TempDir) -> Connection {
    Connection::open(dir.path().join("devai.db")).unwrap()
}

/// REQ-01 AC1/AC2 + REQ-02 AC1: one pair can hold BOTH kinds at once, one
/// task can carry TWO 'parent' edges, and be blocked by several tasks; the
/// whole graph survives a process restart (reopen of the same directory)
/// and reads without any operator chat.
#[test]
fn typed_edges_are_stored_many_to_many_and_survive_reopen() {
    let dir = TempDir::new().unwrap();
    let (a_id, b_id, c_id, d_id);
    {
        let store = Store::open(dir.path()).unwrap();
        let a = task(&store, "backfill (paused parent)");
        let b = task(&store, "derived gate task");
        let c = task(&store, "derived authority task");
        let d = task(&store, "second blocker");
        // The business case verbatim: one parent spawned two children.
        store.link_task_dependency(b.id, a.id, DependencyKind::Parent, "operator", Utc::now()).unwrap();
        let edge = store.link_task_dependency(c.id, a.id, DependencyKind::Parent, "operator", Utc::now()).unwrap();
        // Attribution and time on the edge itself (REQ-07 AC3, decisions.actor precedent).
        assert_eq!(edge.actor, "operator");
        assert!(edge.created_at <= Utc::now());
        // Both kinds on ONE pair, simultaneously.
        store.link_task_dependency(b.id, a.id, DependencyKind::Blocks, "operator", Utc::now()).unwrap();
        // And a second blocker for the same task.
        store.link_task_dependency(b.id, d.id, DependencyKind::Blocks, "operator", Utc::now()).unwrap();
        (a_id, b_id, c_id, d_id) = (a.id, b.id, c.id, d.id);
    }

    // Restart the "process": fresh Store handle over the same directory.
    let store = Store::open(dir.path()).unwrap();
    let out = store.task_dependencies(b_id).unwrap();
    assert_eq!(out.len(), 3, "parent + blocks(a) + blocks(d) all stored and distinguishable");
    let parents: Vec<Id> = out.iter().filter(|d| d.kind == DependencyKind::Parent).map(|d| d.depends_on_id).collect();
    assert_eq!(parents, vec![a_id]);
    let mut blockers: Vec<String> = out.iter().filter(|d| d.kind == DependencyKind::Blocks).map(|d| d.depends_on_id.to_string()).collect();
    blockers.sort();
    let mut expected = vec![a_id.to_string(), d_id.to_string()];
    expected.sort();
    assert_eq!(blockers, expected, "one task blocked by several tasks");
    // The edge projection carries the depended-on task's state (REQ-04 AC1).
    assert!(out.iter().all(|d| d.depends_on_state == TaskState::Draft));

    // Two children of ONE parent (the spawning case) read from the parent
    // side — incoming shows EVERY edge landing on it, of any kind.
    let incoming = store.incoming_dependencies(a_id).unwrap();
    assert_eq!(incoming.len(), 3, "two 'parent' edges + one 'blocks' edge land on the parent");
    assert_eq!(incoming.iter().filter(|d| d.kind == DependencyKind::Parent).count(), 2);
    assert!(incoming.iter().any(|d| d.task_id == b_id) && incoming.iter().any(|d| d.task_id == c_id));
}

/// REQ-07 AC2: integrity is LOUD AT WRITE. Self-dependency, a dangling
/// reference and a duplicate edge are refused, never stored-and-ignored.
#[test]
fn bad_edges_are_rejected_at_write() {
    let (_d, store) = open();
    let a = task(&store, "a");
    let b = task(&store, "b");

    let err = store.link_task_dependency(a.id, a.id, DependencyKind::Parent, "op", Utc::now()).unwrap_err();
    assert!(format!("{err}").contains("itself"), "self-dependency rejected: {err}");

    let ghost = new_id();
    let err = store.link_task_dependency(a.id, ghost, DependencyKind::Blocks, "op", Utc::now()).unwrap_err();
    assert!(format!("{err}").contains("not found"), "dangling reference rejected: {err}");
    let err = store.link_task_dependency(ghost, b.id, DependencyKind::Parent, "op", Utc::now()).unwrap_err();
    assert!(format!("{err}").contains("not found"), "dangling dependent rejected: {err}");

    store.link_task_dependency(b.id, a.id, DependencyKind::Parent, "op", Utc::now()).unwrap();
    let err = store.link_task_dependency(b.id, a.id, DependencyKind::Parent, "op", Utc::now()).unwrap_err();
    assert!(format!("{err}").contains("already exists"), "duplicate edge rejected: {err}");
    // The other kind of the same pair is NOT a duplicate (REQ-02 AC1).
    store.link_task_dependency(b.id, a.id, DependencyKind::Blocks, "op", Utc::now()).unwrap();

    // Nothing bad landed: exactly the two good edges remain.
    assert_eq!(store.task_dependencies(b.id).unwrap().len(), 2);
    assert_eq!(store.task_dependencies(a.id).unwrap().len(), 0);
}

/// REQ-04 AC1/REQ-05 AC1: the WHOLE graph with task states in ONE query;
/// every task appears, edges typed in both readings. REQ-06: no epic
/// entity exists — the epic role falls out of the same query (incoming
/// 'parent' edges), no separate table was added.
#[test]
fn one_query_yields_edges_and_states_epic_role_included() {
    let (d, store) = open();
    let epic = task(&store, "epic: visible progress");
    let child1 = task(&store, "child one");
    let child2 = task(&store, "child two");
    store.link_task_dependency(child1.id, epic.id, DependencyKind::Parent, "op", Utc::now()).unwrap();
    store.link_task_dependency(child2.id, epic.id, DependencyKind::Parent, "op", Utc::now()).unwrap();
    store.link_task_dependency(child1.id, child2.id, DependencyKind::Blocks, "op", Utc::now()).unwrap();

    let graph = store.dependency_graph().unwrap();
    assert_eq!(graph.len(), 3, "every task is a node — even with zero edges");
    let epic_node = graph.iter().find(|n| n.task.id == epic.id).unwrap();
    assert!(epic_node.dependencies.is_empty(), "the epic itself declared no dependencies");
    let c1 = graph.iter().find(|n| n.task.id == child1.id).unwrap();
    assert_eq!(c1.dependencies.len(), 2);
    assert_eq!(c1.task.state, TaskState::Draft, "node state is in the same query");
    assert!(c1.dependencies.iter().all(|x| x.task_id == child1.id && x.task_state == TaskState::Draft));
    assert!(c1.dependencies.iter().any(|x| x.kind == DependencyKind::Blocks && x.depends_on_id == child2.id));

    // REQ-06 AC1/AC2: no epic table — and `epic` is named as a vertex by the
    // very same dependency query used for any other edge.
    let has_epic_table: i64 = raw(&d)
        .query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name LIKE '%epic%'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(has_epic_table, 0);
    assert_eq!(store.incoming_dependencies(epic.id).unwrap().len(), 2, "epic = vertex with incoming edges");
}

/// REQ-08: the v4 rung is ADDITIVE over a real v3 database with existing
/// tasks — no row rewritten, and pre-v4 tasks read as having an EMPTY
/// dependency set, never as corrupt (and behave identically to an explicitly
/// empty one).
#[test]
fn v3_database_gains_the_graph_additively() {
    let dir = TempDir::new().unwrap();
    let (t_id, title);
    {
        let store = Store::open(dir.path()).unwrap();
        let t = task(&store, "pre-v4 work");
        t_id = t.id;
        title = t.title.clone();
        drop(store);
        // Recreate a real v3 database with live rows: drop the v4 table and
        // push the version marker back (the technique of the v2/v3 rung tests).
        raw(&dir)
            .execute_batch("DROP TABLE task_dependencies; PRAGMA user_version=3;")
            .unwrap();
    }
    let store = Store::open(dir.path()).unwrap();
    let version: i64 = raw(&dir).query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, devai_store::SCHEMA_VERSION, "open migrated v3 forward to v4");
    assert_eq!(devai_store::SCHEMA_VERSION, 4, "the ladder top is now v4");

    let t = store.get_task(&t_id.to_string()).unwrap();
    assert_eq!(t.title, title, "the pre-existing task row is byte-intact");
    assert_eq!(store.task_dependencies(t_id).unwrap().len(), 0, "absent edges read as the empty set");
    // REQ-08 AC2: identical behavior to a task with an explicit empty set —
    // a graph node with zero edges, no error, no special-casing.
    let node = store.dependency_graph().unwrap().into_iter().find(|n| n.task.id == t_id).unwrap();
    assert!(node.dependencies.is_empty());

    // And the new surface works on the migrated database.
    let other = task(&store, "pre-v4 too");
    store.link_task_dependency(t_id, other.id, DependencyKind::Blocks, "op", Utc::now()).unwrap();
    assert_eq!(store.task_dependencies(t_id).unwrap().len(), 1);
}

/// REQ-01 AC3: dependencies enter bundle export/restore like the other
/// tables; a bundle whose depended-on task is absent fails LOUDLY and
/// restores nothing partial, and the ordered import brings the edges back.
#[test]
fn bundles_carry_dependency_edges() {
    let (_d1, store) = open();
    let a = task(&store, "parent/backfill");
    let b = task(&store, "derived gate");
    store.link_task_dependency(b.id, a.id, DependencyKind::Parent, "operator", Utc::now()).unwrap();
    store.link_task_dependency(b.id, a.id, DependencyKind::Blocks, "operator", Utc::now()).unwrap();

    let bundle_dir = TempDir::new().unwrap();
    let manifest = bundle::export(&store, &b, &bundle_dir.path().join("b")).unwrap();
    assert_eq!(manifest.counts.dependencies, 2, "the bundle carries the task's declared edges");
    assert!(bundle_dir.path().join("b").join("dependencies.jsonl").exists());

    // Fresh store: importing ONLY the child would leave the edge dangling —
    // refused loudly, transaction rolled back, no partial state.
    let (_d2, store2) = open();
    let err = bundle::import(&store2, &bundle_dir.path().join("b")).unwrap_err();
    let msg = format!("{err}").to_lowercase();
    assert!(msg.contains("foreign key") || msg.contains("constraint"), "dangling edge import fails loud: {err}");
    assert!(store2.list_tasks().unwrap().is_empty(), "failed import left nothing behind");

    // Import the parent bundle first, then the child: the graph lands intact.
    let m_a = bundle::export(&store, &a, &bundle_dir.path().join("a")).unwrap();
    assert_eq!(m_a.counts.dependencies, 0, "a task that declared none exports an empty set");
    bundle::import(&store2, &bundle_dir.path().join("a")).unwrap();
    let outcome = bundle::import(&store2, &bundle_dir.path().join("b")).unwrap();
    assert!(!outcome.skipped_existing);
    let edges = store2.task_dependencies(b.id).unwrap();
    assert_eq!(edges.len(), 2, "both kinds restored on the other machine");
    assert!(edges.iter().any(|d| d.kind == DependencyKind::Parent) && edges.iter().any(|d| d.kind == DependencyKind::Blocks));
    assert_eq!(edges[0].actor, "operator", "attribution restored");

    // The linked event history is part of the child's task and restored too.
    let evs = store2.events(b.id, None).unwrap();
    assert!(evs.iter().any(|e| e.kind == event_kind::TASK_DEPENDENCY_LINKED), "link provenance travels with the task");
}

/// REQ-07: the act of linking is itself an attributed platform event, so
/// "this exists because" is readable as history, not only as a current edge.
#[test]
fn linking_records_an_attributable_event() {
    let (_d, store) = open();
    let a = task(&store, "a");
    let b = task(&store, "b");
    let edge = store.link_task_dependency(b.id, a.id, DependencyKind::Parent, "operator", Utc::now()).unwrap();
    let evs = store.events(b.id, None).unwrap();
    let linked = evs.iter().find(|e| e.kind == event_kind::TASK_DEPENDENCY_LINKED).expect("link event recorded");
    assert_eq!(linked.actor, "operator");
    assert_eq!(linked.payload[json_field::DEPENDENCY_ID], edge.id.to_string());
    assert_eq!(linked.payload[json_field::DEPENDS_ON_ID], a.id.to_string());
    assert_eq!(linked.payload[json_field::DEPENDENCY_KIND], "parent");
}

/// The FK cascade mirrors the artifacts precedent: deleting a task removes
/// its edges, never leaving a half-graph behind. (Deletion policy of edges
/// themselves is an open question — task deletion is the only path built.)
#[test]
fn edges_follow_their_tasks_on_delete() {
    let (d, store) = open();
    let a = task(&store, "a");
    let b = task(&store, "b");
    store.link_task_dependency(b.id, a.id, DependencyKind::Blocks, "op", Utc::now()).unwrap();
    // Delete the parent through a connection with FK enforcement ON (the
    // platform has no task-delete API; this pins the storage invariant).
    let c = raw(&d);
    c.pragma_update(None, "foreign_keys", "ON").unwrap();
    c.execute("DELETE FROM tasks WHERE id=?1", params![a.id.to_string()]).unwrap();
    assert!(store.task_dependencies(b.id).unwrap().is_empty(), "edge to a deleted task is gone, not dangling");
}
