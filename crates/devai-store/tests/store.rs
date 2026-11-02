use chrono::Utc;
use devai_core::types::*;
use devai_core::workflow::{self, Mutation};
use devai_store::bundle;
use devai_store::seed;
use devai_store::{ArtifactDraft, Store};
use tempfile::TempDir;

fn open() -> (TempDir, Store) {
    let d = TempDir::new().unwrap();
    let s = Store::open(d.path()).unwrap();
    (d, s)
}

fn now() -> Timestamp {
    Utc::now()
}

fn draft(store: &Store) -> Task {
    let t = Task::new("test task", "business need", None, now());
    store.create_task(&t).unwrap();
    t
}

fn publish(store: &Store, t: &Task, kind: ArtifactKind, content: &[u8], depends: Vec<Id>) -> Artifact {
    store
        .publish_artifact(ArtifactDraft {
            task_id: t.id,
            kind,
            name: format!("{:?}", kind),
            category: DataCategory::Artifact,
            content: content.to_vec(),
            media_type: "text/plain".into(),
            contract: None,
            created_by: "user".into(),
            execution_id: None,
            depends_on: depends,
        })
        .unwrap()
}

fn commit(store: &Store, base: &Task, m: Mutation) {
    store.commit(base.lock_version, &m).unwrap();
}

/// Drive a task through the standard flow into IMPLEMENTATION.
fn to_implementation(store: &Store) -> (Task, Artifact, Artifact) {
    let mut t = draft(store);
    commit(store, &t, workflow::start(&t, "u", now()).unwrap());
    t = store.get_task(&t.id.to_string()).unwrap();

    let spec = publish(store, &t, ArtifactKind::RequirementsSpec, b"reqs v1", vec![]);
    let arts = store.artifacts(t.id).unwrap();
    commit(store, &t, workflow::requirements_published(&t, &arts, None, "analyst", now()).unwrap());
    t = store.get_task(&t.id.to_string()).unwrap();

    commit(store, &t, workflow::review_recorded(&t, Verdict::Approved, None, "reviewer", now()).unwrap());
    t = store.get_task(&t.id.to_string()).unwrap();
    commit(store, &t, workflow::approve_requirements(&t, Some("ok"), "u", now()).unwrap());
    t = store.get_task(&t.id.to_string()).unwrap();
    (t, spec.clone(), spec)
}

#[test]
fn create_resolve_commit_and_locking() {
    let (_d, store) = open();
    let t = draft(&store);
    let by_prefix = store.resolve_task(&t.id.to_string()[..8]).unwrap();
    assert_eq!(by_prefix.id, t.id);

    let m = workflow::start(&by_prefix, "u", now()).unwrap();
    commit(&store, &by_prefix, m);
    let reloaded = store.get_task(&t.id.to_string()).unwrap();
    assert_eq!(reloaded.state, TaskState::Analysis);
    assert_eq!(reloaded.lock_version, by_prefix.lock_version + 1);

    // stale optimistic write must fail, not silently overwrite
    let stale = by_prefix.clone(); // lock 1, task is now lock 2
    let m = workflow::pause(&reloaded, Some("x"), "u", now()).unwrap();
    let err = store.commit(stale.lock_version, &m).unwrap_err();
    assert!(format!("{err}").contains("concurrent modification"), "{err}");
    // the task is untouched after a rejected commit
    assert_eq!(store.get_task(&t.id.to_string()).unwrap().state, TaskState::Analysis);
}

#[test]
fn revision_marks_dependents_stale_persistently() {
    let (_d, store) = open();
    let (mut t, spec1, _) = to_implementation(&store);

    let plan = publish(&store, &t, ArtifactKind::ImplementationPlan, b"plan", vec![spec1.id]);
    let report = publish(&store, &t, ArtifactKind::ImplementationReport, b"report", vec![plan.id]);
    commit(&store, &t, workflow::implementation_reported(&t, None, "impl", now()).unwrap());
    t = store.get_task(&t.id.to_string()).unwrap();
    assert_eq!(t.state, TaskState::Qa);

    // user revises requirements in QA: v2 supersedes v1, downstream goes stale
    let _spec2 = publish(&store, &t, ArtifactKind::RequirementsSpec, b"reqs v2", vec![]);
    let arts = store.artifacts(t.id).unwrap();
    let m = workflow::requirements_published(&t, &arts, Some("scope change"), "u", now()).unwrap();
    commit(&store, &t, m);

    let arts = store.artifacts(t.id).unwrap();
    let st = |id: Id| arts.iter().find(|a| a.id == id).unwrap().status;
    assert_eq!(st(spec1.id), ArtifactStatus::Superseded);
    assert_eq!(st(plan.id), ArtifactStatus::Stale);
    assert_eq!(st(report.id), ArtifactStatus::Stale, "transitive dependents must be stale");
    let t2 = store.get_task(&t.id.to_string()).unwrap();
    assert_eq!(t2.state, TaskState::Review);
    assert_eq!(t2.requirements_seq, 2);
    // history preserved: nothing deleted
    assert_eq!(arts.len(), 4);
}

#[test]
fn content_addressing_and_seq() {
    let (_d, store) = open();
    let t = draft(&store);
    let a1 = publish(&store, &t, ArtifactKind::Patch, b"same bytes", vec![]);
    let a2 = publish(&store, &t, ArtifactKind::Patch, b"same bytes", vec![]);
    assert_eq!(a1.sha256, a2.sha256);
    assert_ne!(a1.seq, a2.seq);
    assert_eq!(store.read_blob(&a1.sha256).unwrap(), b"same bytes");
}

#[test]
fn export_import_roundtrip_restores_everything() {
    let (_d1, store) = open();
    let (mut t, spec1, _) = to_implementation(&store);

    // an execution (bound to a workspace revision), a blocking question +
    // answer, a decision
    let revision = WorkspaceRevision {
        git_head: Some("f".repeat(40)),
        dirty_digest: Some("e".repeat(64)),
        profile: Some("quality@3".into()),
        roles: vec!["implementation-agent@1".into(), "quality-auditor@1".into()],
    };
    let e = RoleExecution {
        id: new_id(),
        task_id: t.id,
        role_slug: "implementation-agent".into(),
        role_version: 1,
        agent_name: Some("claude".into()),
        status: ExecutionStatus::Running,
        inputs: vec![spec1.id.to_string()],
        summary: None,
        error: None,
        started_at: now(),
        finished_at: None,
        workspace_revision: Some(revision.clone()),
    };
    store.start_execution(&e).unwrap();
    let q = Question {
        id: new_id(),
        task_id: t.id,
        execution_id: Some(e.id),
        text: "which branch strategy?".into(),
        context: None,
        options: vec!["worktree".into(), "branch".into()],
        blocking: true,
        status: QuestionStatus::Open,
        answer: None,
        answered_by: None,
        answered_at: None,
        created_at: now(),
    };
    store.create_question(&q).unwrap();
    commit(&store, &t, workflow::raise_question(&t, q.id, true, "impl", now()).unwrap());
    t = store.get_task(&t.id.to_string()).unwrap();
    store.answer_question(q.id, "worktree", "u", now()).unwrap();
    commit(&store, &t, workflow::answer_question(&t, q.id, "u", now()).unwrap());
    t = store.get_task(&t.id.to_string()).unwrap();
    commit(&store, &t, workflow::implementation_reported(&t, None, "impl", now()).unwrap());
    store.finish_execution(&store.get_execution(&e.id.to_string()).unwrap(), ExecutionStatus::Completed, Some("done"), None, now()).unwrap();
    store
        .record_decision(&Decision {
            id: new_id(),
            task_id: t.id,
            kind: "requirements_approval".into(),
            verdict: Verdict::Approved,
            actor: "u".into(),
            rationale: Some("ok".into()),
            refs: vec![spec1.id.to_string()],
            created_at: now(),
        })
        .unwrap();
    t = store.get_task(&t.id.to_string()).unwrap();

    // The profile identity the revision cites must be resolvable in the
    // SOURCE store — that is what export packages for the target store.
    store.put_evaluation_profile("quality", 3, "devai", "version: 3\nproject: devai\n", &now()).unwrap();

    // export captures the current events, THEN appends its own EXPORTED event,
    // so compare the manifest against a pre-export snapshot.
    let events_before_export = store.events(t.id, None).unwrap().len();
    let bundle_dir = TempDir::new().unwrap();
    let manifest = bundle::export(&store, &t, bundle_dir.path()).unwrap();
    assert_eq!(manifest.counts.events, events_before_export);
    assert!(manifest.counts.artifacts >= 1);
    // The revision cites quality@3 (the factory profile identity): the bundle
    // must carry that definition so it stays resolvable on import elsewhere.
    assert!(
        bundle_dir.path().join("profiles.jsonl").exists() && manifest.counts.profiles >= 1,
        "bundle carries the profile definitions its revisions cite: {:?}",
        manifest.counts
    );

    // fresh store (client B): import must restore full domain state
    let (_d2, store2) = open();
    let outcome = bundle::import(&store2, bundle_dir.path()).unwrap();
    assert!(!outcome.skipped_existing);
    let t2 = store2.get_task(&t.id.to_string()).unwrap();
    assert_eq!(t2.state, t.state);
    assert_eq!(t2.requirements_seq, t.requirements_seq);
    assert_eq!(t2.lock_version, t.lock_version);
    assert_eq!(store2.artifacts(t2.id).unwrap().len(), store.artifacts(t.id).unwrap().len());
    assert_eq!(store2.events(t2.id, None).unwrap().len(), store.events(t.id, None).unwrap().len());
    let execs2 = store2.list_executions(Some(t2.id)).unwrap();
    assert_eq!(execs2.len(), 1);
    assert_eq!(
        execs2[0].workspace_revision.as_ref(),
        Some(&revision),
        "bundle roundtrip must preserve the workspace binding exactly"
    );
    // And the cited profile identity resolves in the fresh store too.
    assert!(store2.get_evaluation_profile("quality", 3).unwrap().is_some(), "imported bundle re-seeds the profile history its revisions reference");
    let q2 = store2.get_question(&q.id.to_string()).unwrap();
    assert_eq!(q2.status, QuestionStatus::Answered);
    // blobs carried over and verifiable
    let a = store2.artifacts(t2.id).unwrap().remove(0);
    store2.read_blob(&a.sha256).unwrap();

    // re-import is a no-op, not a duplicate
    let again = bundle::import(&store2, bundle_dir.path()).unwrap();
    assert!(again.skipped_existing);
    assert_eq!(store2.list_tasks().unwrap().len(), 1);
}

#[test]
fn corrupted_bundle_is_rejected() {
    let (_d1, store) = open();
    let (t, _s, _) = to_implementation(&store);
    let bundle_dir = TempDir::new().unwrap();
    bundle::export(&store, &t, bundle_dir.path()).unwrap();
    let path = bundle_dir.path().join("artifacts.jsonl");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[0] ^= 0xff; // same length, different checksum
    std::fs::write(&path, &bytes).unwrap();

    let (_d2, store2) = open();
    let err = bundle::import(&store2, bundle_dir.path()).unwrap_err();
    assert!(format!("{err}").contains("checksum mismatch"), "{err}");
    assert!(store2.list_tasks().unwrap().is_empty(), "failed import must leave no partial state");
}

/// REQ-007: evaluation profile identities recorded in old executions stay
/// resolvable to the definition that was in force at execution time, even
/// after the YAML is edited in place. The store's history is insert-only:
/// the FIRST stored text for a (name, version) IS that version's definition.
#[test]
fn profile_history_is_insert_only_and_survives_in_place_edits() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path()).unwrap();
    seed::seed_defaults(&store, now()).unwrap();

    // `devai init` snapshotted the factory profile under its declared version.
    let profiles = store.list_evaluation_profiles().unwrap();
    let (name, factory_version, project) = profiles
        .iter()
        .find(|(n, _, _)| n == "quality")
        .cloned()
        .unwrap_or_else(|| panic!("quality profile snapshotted on seed: {profiles:?}"));
    assert_eq!(project, "devai");
    let (_p, original) = store
        .get_evaluation_profile(&name, factory_version)
        .unwrap()
        .expect("factory profile identity resolvable");
    assert!(original.contains("version:"), "body is the real profile YAML");

    // A user edits the YAML in place WITHOUT bumping the version and re-inits:
    // the snapshot must NOT be clobbered — old revisions still resolve exactly.
    let path = dir.path().join("config").join("evaluation").join("quality.yaml");
    let edited = format!("{original}\n# edited in place, version kept\n");
    std::fs::write(&path, edited.as_bytes()).unwrap();
    seed::seed_from(&store, &dir.path().join("config"), now()).unwrap();
    let (_p, body_after) = store.get_evaluation_profile(&name, factory_version).unwrap().unwrap();
    assert_eq!(body_after, original, "in-place edit without a version bump never rewrites history");

    // A proper version bump lands as a NEW row; both stay resolvable.
    let next = factory_version + 1;
    let bumped_body = format!("version: {next}\n# bumped definition\n");
    assert!(store.put_evaluation_profile(&name, next, "devai", &bumped_body, &now()).unwrap());
    assert!(!store.put_evaluation_profile(&name, next, "devai", "tampered", &now()).unwrap(), "second put for the same identity is ignored");
    assert_eq!(store.get_evaluation_profile(&name, next).unwrap().unwrap().1, bumped_body);
    assert!(store.get_evaluation_profile(&name, factory_version).unwrap().is_some(), "the old identity still resolves");
    assert!(store.get_evaluation_profile(&name, factory_version + 90).unwrap().is_none(), "unknown identities do not resolve — never fabricated");
}
