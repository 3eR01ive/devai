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
    commit(&store, &t, workflow::implementation_reported(&t, &workflow::ImplementationReportFacts::bare(new_id()), "impl", now()).unwrap());
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
    commit(&store, &t, workflow::implementation_reported(&t, &workflow::ImplementationReportFacts::bare(new_id()), "impl", now()).unwrap());
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

// ---------------------------------------------------------------------------
// REQ-001/REQ-002/REQ-006: artifact supersession — exactly one CURRENT per
// (task_id, kind), one batched task.artifact.superseded event per publication
// that superseded something. Explicitly covers the four artifact kinds named
// by the task (spec/review/implementation/quality-report); the store rule
// itself is generic over all ArtifactKind.
// ---------------------------------------------------------------------------

fn supersession_events(store: &Store, task_id: Id) -> Vec<WorkflowEvent> {
    store
        .events(task_id, None)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "task.artifact.superseded")
        .collect()
}

#[test]
fn first_version_of_each_kind_supersedes_nothing() {
    let (_d, store) = open();
    let t = draft(&store);
    for kind in [
        ArtifactKind::RequirementsSpec,
        ArtifactKind::ReviewReport,
        ArtifactKind::ImplementationReport,
        ArtifactKind::QualityReport,
    ] {
        let a = publish(&store, &t, kind, b"v1", vec![]);
        assert_eq!(a.status, ArtifactStatus::Current);
        let arts = store.artifacts(t.id).unwrap();
        assert_eq!(arts.iter().filter(|x| x.kind == kind && x.status == ArtifactStatus::Current).count(), 1);
        assert!(supersession_events(&store, t.id).is_empty(), "the first version of a kind must produce zero SUPERSEDED rows and zero events");
    }
}

#[test]
fn second_version_supersedes_the_first_for_each_named_kind() {
    let (_d, store) = open();
    let t = draft(&store);
    for kind in [
        ArtifactKind::RequirementsSpec,
        ArtifactKind::ReviewReport,
        ArtifactKind::ImplementationReport,
        ArtifactKind::QualityReport,
    ] {
        let before = supersession_events(&store, t.id).len();
        let v1 = publish(&store, &t, kind, b"one", vec![]);
        let v2 = publish(&store, &t, kind, b"two", vec![]);

        let arts = store.artifacts(t.id).unwrap();
        let group: Vec<_> = arts.iter().filter(|a| a.kind == kind).collect();
        assert_eq!(group.iter().filter(|a| a.status == ArtifactStatus::Current).count(), 1, "exactly one CURRENT for this kind");
        assert_eq!(group.iter().filter(|a| a.status == ArtifactStatus::Current).next().unwrap().id, v2.id);
        assert_eq!(store.get_artifact(&v1.id.to_string()).unwrap().status, ArtifactStatus::Superseded, "the previous version is marked, never deleted");
        assert_eq!(v2.seq, v1.seq + 1, "per-group seq numbering is unchanged (REQ-001 constraint)");

        // REQ-002: exactly one batched event naming new id, flipped ids, kind, task.
        let evs = supersession_events(&store, t.id);
        assert_eq!(evs.len(), before + 1, "one supersession event for this kind");
        let ev = evs.last().unwrap();
        assert_eq!(ev.payload[json_field::ARTIFACT_ID], v2.id.to_string());
        assert_eq!(ev.payload[json_field::KIND], kind.as_str());
        assert_eq!(ev.payload[json_field::TASK_ID], t.id.to_string());
        assert_eq!(ev.payload[json_field::SUPERSEDED].as_array().unwrap().len(), 1);
        assert_eq!(ev.payload[json_field::SUPERSEDED][0], v1.id.to_string());
    }
}

#[test]
fn supersession_is_per_group_and_survives_a_long_series() {
    let (_d, store) = open();
    let t1 = draft(&store);
    let t2 = draft(&store);
    // Interleave kinds and tasks: each (task, kind) group keeps its own CURRENT.
    let spec1 = publish(&store, &t1, ArtifactKind::RequirementsSpec, b"s1", vec![]);
    let patch1 = publish(&store, &t1, ArtifactKind::Patch, b"p1", vec![]);
    let spec2 = publish(&store, &t1, ArtifactKind::RequirementsSpec, b"s2", vec![]);
    let _patch2 = publish(&store, &t2, ArtifactKind::Patch, b"p2", vec![]);

    assert_eq!(store.get_artifact(&spec1.id.to_string()).unwrap().status, ArtifactStatus::Superseded);
    assert_eq!(store.get_artifact(&patch1.id.to_string()).unwrap().status, ArtifactStatus::Current, "another task's publication never touches this group");
    assert_eq!(store.get_artifact(&spec2.id.to_string()).unwrap().status, ArtifactStatus::Current);

    // REQ-001 AC2: no (task, kind) holds more than one CURRENT row.
    for task in [t1.id, t2.id] {
        let arts = store.artifacts(task).unwrap();
        let mut groups: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for a in arts.iter().filter(|a| a.status == ArtifactStatus::Current) {
            *groups.entry(a.kind.as_str()).or_default() += 1;
        }
        for (kind, n) in groups {
            assert_eq!(n, 1, "task {task} kind {kind} holds {n} CURRENT rows");
        }
    }
    // The publish path remains the only writer and keeps the invariant.
    let again = store.publish_artifact(ArtifactDraft {
        task_id: t1.id,
        kind: ArtifactKind::RequirementsSpec,
        name: "again".into(),
        category: DataCategory::Artifact,
        content: b"s3".to_vec(),
        media_type: "text/plain".into(),
        contract: None,
        created_by: "user".into(),
        execution_id: None,
        depends_on: vec![],
    });
    assert!(again.is_ok(), "publication supersedes inside its own transaction");
}

#[test]
fn supersession_events_are_batched_per_publication() {
    let (_d, store) = open();
    let t = draft(&store);
    let v1 = publish(&store, &t, ArtifactKind::QualityReport, b"1", vec![]);
    let v2 = publish(&store, &t, ArtifactKind::QualityReport, b"2", vec![]);
    let _v3 = publish(&store, &t, ArtifactKind::QualityReport, b"3", vec![]);
    let evs = supersession_events(&store, t.id);
    assert_eq!(evs.len(), 2, "one event per superseding publication: versions 2 and 3");
    assert_eq!(evs[0].payload[json_field::SUPERSEDED].as_array().unwrap()[0], v1.id.to_string());
    assert_eq!(evs[1].payload[json_field::SUPERSEDED].as_array().unwrap()[0], v2.id.to_string());
    // Nothing deleted: all three rows still readable, two marked.
    assert_eq!(store.artifacts(t.id).unwrap().len(), 3);
}
