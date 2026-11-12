use chrono::Utc;
use devai_core::types::*;
use devai_core::workflow::{self, Mutation};
use devai_store::bundle;
use devai_store::seed;
use devai_store::{ArtifactDraft, Store, StoreError};
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

fn publish(
    store: &Store,
    t: &Task,
    kind: ArtifactKind,
    content: &[u8],
    depends: Vec<Id>,
) -> Artifact {
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
    commit(store, &t, workflow::start(&proc(), &t, "u", now()).unwrap());
    t = store.get_task(&t.id.to_string()).unwrap();
    // process v4: the statement checker's verdict opens the analysis.
    commit(
        store,
        &t,
        workflow::statement_approved(&proc(), &t, "checker", now()).unwrap(),
    );
    t = store.get_task(&t.id.to_string()).unwrap();

    let spec = publish(
        store,
        &t,
        ArtifactKind::RequirementsSpec,
        b"reqs v1",
        vec![],
    );
    let arts = store.artifacts(t.id).unwrap();
    commit(
        store,
        &t,
        workflow::requirements_published(&proc(), &t, &arts, None, None, "analyst", now()).unwrap(),
    );
    t = store.get_task(&t.id.to_string()).unwrap();

    commit(
        store,
        &t,
        workflow::review_recorded(&proc(), &t, Verdict::Approved, None, "reviewer", now()).unwrap(),
    );
    t = store.get_task(&t.id.to_string()).unwrap();
    commit(
        store,
        &t,
        workflow::approve_requirements(&proc(), &t, Some("ok"), "u", now()).unwrap(),
    );
    t = store.get_task(&t.id.to_string()).unwrap();
    (t, spec.clone(), spec)
}

#[test]
fn create_resolve_commit_and_locking() {
    let (_d, store) = open();
    let t = draft(&store);
    let by_prefix = store.resolve_task(&t.id.to_string()[..8]).unwrap();
    assert_eq!(by_prefix.id, t.id);

    let m = workflow::start(&proc(), &by_prefix, "u", now()).unwrap();
    commit(&store, &by_prefix, m);
    let after_start = store.get_task(&t.id.to_string()).unwrap();
    assert_eq!(after_start.state, TaskState::Statement, "the checker goes first");
    let m = workflow::statement_approved(&proc(), &after_start, "checker", now()).unwrap();
    commit(&store, &after_start, m);
    let reloaded = store.get_task(&t.id.to_string()).unwrap();
    assert_eq!(reloaded.state, TaskState::Analysis);
    // start + the checker's verdict — two commits past the draft.
    assert_eq!(reloaded.lock_version, by_prefix.lock_version + 2);

    // stale optimistic write must fail, not silently overwrite
    let stale = by_prefix.clone(); // lock 1, task is now lock 2
    let m = workflow::pause(&proc(), &reloaded, Some("x"), "u", now()).unwrap();
    let err = store.commit(stale.lock_version, &m).unwrap_err();
    assert!(
        format!("{err}").contains("concurrent modification"),
        "{err}"
    );
    // the task is untouched after a rejected commit
    assert_eq!(
        store.get_task(&t.id.to_string()).unwrap().state,
        TaskState::Analysis
    );
}

#[test]
fn revision_marks_dependents_stale_persistently() {
    let (_d, store) = open();
    let (mut t, spec1, _) = to_implementation(&store);

    let plan = publish(
        &store,
        &t,
        ArtifactKind::ImplementationPlan,
        b"plan",
        vec![spec1.id],
    );
    let report = publish(
        &store,
        &t,
        ArtifactKind::ImplementationReport,
        b"report",
        vec![plan.id],
    );
    commit(
        &store,
        &t,
        workflow::implementation_reported(
            &proc(),
            &t,
            &workflow::ImplementationReportFacts::bare(new_id()),
            "impl",
            now(),
        )
        .unwrap(),
    );
    t = store.get_task(&t.id.to_string()).unwrap();
    assert_eq!(t.state, TaskState::Qa);

    // user revises requirements in QA: v2 supersedes v1, downstream goes stale
    let _spec2 = publish(
        &store,
        &t,
        ArtifactKind::RequirementsSpec,
        b"reqs v2",
        vec![],
    );
    let arts = store.artifacts(t.id).unwrap();
    let m = workflow::requirements_published(
        &proc(),
        &t,
        &arts,
        Some("scope change"),
        None,
        "u",
        now(),
    )
    .unwrap();
    commit(&store, &t, m);

    let arts = store.artifacts(t.id).unwrap();
    let st = |id: Id| arts.iter().find(|a| a.id == id).unwrap().status;
    assert_eq!(st(spec1.id), ArtifactStatus::Superseded);
    assert_eq!(st(plan.id), ArtifactStatus::Stale);
    assert_eq!(
        st(report.id),
        ArtifactStatus::Stale,
        "transitive dependents must be stale"
    );
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
    assert_eq!(store.read_artifact_body(&a1).unwrap(), b"same bytes");
}

/// The CURRENT/body pairing has ONE body reader and ONE error policy:
/// absence is `NotFound` (the row outlived its content), an existing but
/// unreadable blob is `Corrupt` (a broken store must not read as an empty
/// one), and a healthy blob returns exactly the stored bytes.
#[test]
fn artifact_body_read_policy_is_one_place() {
    let (_d, store) = open();
    let t = draft(&store);
    let healthy = publish(&store, &t, ArtifactKind::Patch, b"body bytes", vec![]);
    assert_eq!(store.read_artifact_body(&healthy).unwrap(), b"body bytes");

    // The blob is gone, the row is not: NotFound naming the content address.
    let missing = publish(&store, &t, ArtifactKind::Patch, b"gone", vec![]);
    std::fs::remove_file(store.blob_path(&missing.sha256)).unwrap();
    match store.read_artifact_body(&missing).unwrap_err() {
        StoreError::NotFound(what, sha) => {
            assert_eq!(what, "artifact blob");
            assert_eq!(sha, missing.sha256);
        }
        other => panic!("absence must be NotFound, got {other:?}"),
    }

    // Present but unreadable (a directory sits at the content address): an
    // IO failure that is NOT absence — Corrupt, naming the artifact.
    let unreadable = publish(&store, &t, ArtifactKind::Patch, b"not a file", vec![]);
    let path = store.blob_path(&unreadable.sha256);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    match store.read_artifact_body(&unreadable).unwrap_err() {
        StoreError::Corrupt(message) => {
            assert!(
                message.contains(&unreadable.id.to_string()),
                "the corrupt fact must name the artifact: {message}"
            );
        }
        other => panic!("an unreadable blob must be Corrupt, got {other:?}"),
    }
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
        // v5 form-loop columns (REQ-FG-06): a manual row is root attempt 1.
        attempt: 1,
        parent_execution: None,
        failure_class: None,
        // v7 owner identity (REQ-SW-03): a fixture value — the roundtrip
        // below checks it travels into the bundle and back.
        owner_pid: Some(4242),
        owner_host: Some("fixture-host".into()),
        owner_process_start: Some(1_730_000_000),
        bundle: None,
        run_id: None,
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
    commit(
        &store,
        &t,
        workflow::raise_question(&proc(), &t, q.id, true, "impl", now()).unwrap(),
    );
    t = store.get_task(&t.id.to_string()).unwrap();
    store.answer_question(q.id, "worktree", "u", now()).unwrap();
    commit(
        &store,
        &t,
        workflow::answer_question(&proc(), &t, q.id, "u", now()).unwrap(),
    );
    t = store.get_task(&t.id.to_string()).unwrap();
    commit(
        &store,
        &t,
        workflow::implementation_reported(
            &proc(),
            &t,
            &workflow::ImplementationReportFacts::bare(new_id()),
            "impl",
            now(),
        )
        .unwrap(),
    );
    store
        .finish_execution(
            &store.get_execution(&e.id.to_string()).unwrap(),
            ExecutionStatus::Completed,
            Some("done"),
            None,
            now(),
        )
        .unwrap();
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
            payload: serde_json::Value::Null,
        })
        .unwrap();
    t = store.get_task(&t.id.to_string()).unwrap();

    // The profile identity the revision cites must be resolvable in the
    // SOURCE store — that is what export packages for the target store.
    store
        .put_evaluation_profile(
            "quality",
            3,
            "devai",
            "version: 3\nproject: devai\n",
            &now(),
        )
        .unwrap();

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
    assert_eq!(
        store2.artifacts(t2.id).unwrap().len(),
        store.artifacts(t.id).unwrap().len()
    );
    assert_eq!(
        store2.events(t2.id, None).unwrap().len(),
        store.events(t.id, None).unwrap().len()
    );
    let execs2 = store2.list_executions(Some(t2.id)).unwrap();
    assert_eq!(execs2.len(), 1);
    assert_eq!(
        execs2[0].workspace_revision.as_ref(),
        Some(&revision),
        "bundle roundtrip must preserve the workspace binding exactly"
    );
    // REQ-SW-03 AC3: the owner identity travels with the row (ratified
    // OQ-2 — columns, so bundle export/import carries them verbatim).
    assert_eq!(
        execs2[0].owner_pid,
        Some(4242),
        "owner pid survives the roundtrip: {:?}",
        execs2[0]
    );
    assert_eq!(
        execs2[0].owner_host.as_deref(),
        Some("fixture-host"),
        "owner host survives the roundtrip"
    );
    assert_eq!(
        execs2[0].owner_process_start,
        Some(1_730_000_000),
        "owner process start survives the roundtrip"
    );
    // And the cited profile identity resolves in the fresh store too.
    assert!(
        store2
            .get_evaluation_profile("quality", 3)
            .unwrap()
            .is_some(),
        "imported bundle re-seeds the profile history its revisions reference"
    );
    let q2 = store2.get_question(&q.id.to_string()).unwrap();
    assert_eq!(q2.status, QuestionStatus::Answered);
    // blobs carried over and verifiable
    let a = store2.artifacts(t2.id).unwrap().remove(0);
    store2.read_artifact_body(&a).unwrap();

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
    assert!(
        store2.list_tasks().unwrap().is_empty(),
        "failed import must leave no partial state"
    );
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
        .find(|(n, _, _)| n == "base")
        .cloned()
        .unwrap_or_else(|| panic!("factory base profile snapshotted on seed: {profiles:?}"));
    assert_eq!(project, "base");
    let (_p, original) = store
        .get_evaluation_profile(&name, factory_version)
        .unwrap()
        .expect("factory profile identity resolvable");
    assert!(
        original.contains("version:"),
        "body is the real profile YAML"
    );

    // A user edits the YAML in place WITHOUT bumping the version and re-inits:
    // the snapshot must NOT be clobbered — old revisions still resolve exactly.
    let path = dir
        .path()
        .join("config")
        .join("evaluation")
        .join("base.yaml");
    let edited = format!("{original}\n# edited in place, version kept\n");
    std::fs::write(&path, edited.as_bytes()).unwrap();
    seed::seed_from(&store, &dir.path().join("config"), now()).unwrap();
    let (_p, body_after) = store
        .get_evaluation_profile(&name, factory_version)
        .unwrap()
        .unwrap();
    assert_eq!(
        body_after, original,
        "in-place edit without a version bump never rewrites history"
    );

    // A proper version bump lands as a NEW row; both stay resolvable.
    let next = factory_version + 1;
    let bumped_body = format!("version: {next}\n# bumped definition\n");
    assert!(
        store
            .put_evaluation_profile(&name, next, "devai", &bumped_body, &now())
            .unwrap()
    );
    assert!(
        !store
            .put_evaluation_profile(&name, next, "devai", "tampered", &now())
            .unwrap(),
        "second put for the same identity is ignored"
    );
    assert_eq!(
        store
            .get_evaluation_profile(&name, next)
            .unwrap()
            .unwrap()
            .1,
        bumped_body
    );
    assert!(
        store
            .get_evaluation_profile(&name, factory_version)
            .unwrap()
            .is_some(),
        "the old identity still resolves"
    );
    assert!(
        store
            .get_evaluation_profile(&name, factory_version + 90)
            .unwrap()
            .is_none(),
        "unknown identities do not resolve — never fabricated"
    );
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
        assert_eq!(
            arts.iter()
                .filter(|x| x.kind == kind && x.status == ArtifactStatus::Current)
                .count(),
            1
        );
        assert!(
            supersession_events(&store, t.id).is_empty(),
            "the first version of a kind must produce zero SUPERSEDED rows and zero events"
        );
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
        assert_eq!(
            group
                .iter()
                .filter(|a| a.status == ArtifactStatus::Current)
                .count(),
            1,
            "exactly one CURRENT for this kind"
        );
        assert_eq!(
            group
                .iter()
                .filter(|a| a.status == ArtifactStatus::Current)
                .next()
                .unwrap()
                .id,
            v2.id
        );
        assert_eq!(
            store.get_artifact(&v1.id.to_string()).unwrap().status,
            ArtifactStatus::Superseded,
            "the previous version is marked, never deleted"
        );
        assert_eq!(
            v2.seq,
            v1.seq + 1,
            "per-group seq numbering is unchanged (REQ-001 constraint)"
        );

        // REQ-002: exactly one batched event naming new id, flipped ids, kind, task.
        let evs = supersession_events(&store, t.id);
        assert_eq!(
            evs.len(),
            before + 1,
            "one supersession event for this kind"
        );
        let ev = evs.last().unwrap();
        assert_eq!(ev.payload[json_field::ARTIFACT_ID], v2.id.to_string());
        assert_eq!(ev.payload[json_field::KIND], kind.as_str());
        assert_eq!(ev.payload[json_field::TASK_ID], t.id.to_string());
        assert_eq!(
            ev.payload[json_field::SUPERSEDED].as_array().unwrap().len(),
            1
        );
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

    assert_eq!(
        store.get_artifact(&spec1.id.to_string()).unwrap().status,
        ArtifactStatus::Superseded
    );
    assert_eq!(
        store.get_artifact(&patch1.id.to_string()).unwrap().status,
        ArtifactStatus::Current,
        "another task's publication never touches this group"
    );
    assert_eq!(
        store.get_artifact(&spec2.id.to_string()).unwrap().status,
        ArtifactStatus::Current
    );

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
    assert!(
        again.is_ok(),
        "publication supersedes inside its own transaction"
    );
}

#[test]
fn supersession_events_are_batched_per_publication() {
    let (_d, store) = open();
    let t = draft(&store);
    let v1 = publish(&store, &t, ArtifactKind::QualityReport, b"1", vec![]);
    let v2 = publish(&store, &t, ArtifactKind::QualityReport, b"2", vec![]);
    let _v3 = publish(&store, &t, ArtifactKind::QualityReport, b"3", vec![]);
    let evs = supersession_events(&store, t.id);
    assert_eq!(
        evs.len(),
        2,
        "one event per superseding publication: versions 2 and 3"
    );
    assert_eq!(
        evs[0].payload[json_field::SUPERSEDED].as_array().unwrap()[0],
        v1.id.to_string()
    );
    assert_eq!(
        evs[1].payload[json_field::SUPERSEDED].as_array().unwrap()[0],
        v2.id.to_string()
    );
    // Nothing deleted: all three rows still readable, two marked.
    assert_eq!(store.artifacts(t.id).unwrap().len(), 3);
}

/// UUIDv7 leading bytes are the creation timestamp, so two executions
/// started in the same millisecond share the 12-char display short id used
/// across the CLI. Prefix resolution must REFUSE to guess which row the
/// caller meant — silently taking the first match would finish the wrong
/// execution. Same loud discipline as `resolve_task`.
///
/// Since the v7 single-writer guard the two colliding rows live on
/// DIFFERENT tasks: at most one RUNNING execution per task_id is storable
/// now (partial unique index, REQ-SW-01), while same-millisecond prefix
/// collisions remain reachable across tasks — the class of ambiguity this
/// test pins is unchanged by the guard.
#[test]
fn execution_id_prefix_resolution_fails_loudly_on_ambiguity() {
    let (_d, store) = open();
    let ta = draft(&store);
    let tb = draft(&store);
    let exec = |s: &str, task_id: Id| RoleExecution {
        id: Id::parse_str(s).unwrap(),
        task_id,
        role_slug: "implementation-agent".into(),
        role_version: 1,
        agent_name: Some("mock".into()),
        status: ExecutionStatus::Running,
        inputs: vec![],
        summary: None,
        error: None,
        started_at: now(),
        finished_at: None,
        workspace_revision: None,
        attempt: 1,
        parent_execution: None,
        failure_class: None,
        owner_pid: None,
        owner_host: None,
        owner_process_start: None,
        bundle: None,
        run_id: None,
    };
    let a = exec("01a1a5db-3ba0-7a5b-8176-3f38ad4c1ea8", ta.id);
    let b = exec("01a1a5db-3ba1-7a5b-8176-3f38ad4c1ea9", tb.id);
    store.start_execution(&a).unwrap();
    store.start_execution(&b).unwrap();

    // The shared display short prefix matches BOTH rows: ambiguous, loud.
    let err = store.get_execution("01a1a5db-3ba").unwrap_err();
    assert!(matches!(err, StoreError::Ambiguous(_)), "{err}");
    assert!(format!("{err}").contains("01a1a5db-3ba"), "{err}");
    // A unique prefix still resolves — the human-friendly convenience survives.
    assert_eq!(store.get_execution("01a1a5db-3ba0").unwrap().id, a.id);
    assert_eq!(store.get_execution(&b.id.to_string()).unwrap().id, b.id);
}

/// OQ-3 (ratified): ONE rule across every entity resolver — a prefix that
/// matches several rows fails LOUDLY (Ambiguous), the store never guesses.
/// `get_artifact` and `get_question` used `LIMIT 1`: whichever colliding row
/// the scan met first was silently returned (the UUIDv7 same-millisecond
/// collision class of the QA 01a1acab-18b1 flake). The `get_execution`
/// discipline above is now law across the whole surface; a UNIQUE prefix
/// still resolves — the human-friendly convenience is preserved.
#[test]
fn artifact_and_question_resolvers_fail_loudly_on_ambiguity() {
    let (d, store) = open();
    let t = draft(&store);
    let a1 = publish(&store, &t, ArtifactKind::TestReport, b"one", vec![]);
    let a2 = publish(&store, &t, ArtifactKind::QualityReport, b"two", vec![]);
    // Hand-picked ids sharing the FULL canonical display prefix — the
    // collision is a fixture of the resolver rule, not of clock luck.
    {
        let raw = rusqlite::Connection::open(d.path().join("devai.db")).unwrap();
        raw.execute(
            "UPDATE artifacts SET id=?1 WHERE id=?2",
            rusqlite::params!["019e3333-0000-7000-8000-0000000000aa", a1.id.to_string()],
        )
        .unwrap();
        raw.execute(
            "UPDATE artifacts SET id=?1 WHERE id=?2",
            rusqlite::params!["019e3333-0000-7000-8000-0000000000ab", a2.id.to_string()],
        )
        .unwrap();
    }
    let err = store.get_artifact("019e3333-0000").unwrap_err();
    assert!(matches!(err, StoreError::Ambiguous(_)), "{err}");
    // A full or uniquely-prefixed lookup still resolves.
    assert_eq!(
        store
            .get_artifact("019e3333-0000-7000-8000-0000000000aa")
            .unwrap()
            .id
            .to_string(),
        "019e3333-0000-7000-8000-0000000000aa"
    );
    let unknown = store
        .get_artifact("019e3333-0000-7000-8000-0000000000b")
        .unwrap_err();
    assert!(
        unknown.to_string().contains("not found"),
        "an unknown prefix is the SAME NotFound sentence as before: {unknown}"
    );

    let q1 = Question {
        id: Id::parse_str("019e4444-0000-7000-8000-0000000000aa").unwrap(),
        task_id: t.id,
        execution_id: None,
        text: "question one".into(),
        context: None,
        options: vec![],
        blocking: false,
        status: QuestionStatus::Open,
        answer: None,
        answered_by: None,
        answered_at: None,
        created_at: now(),
    };
    let mut q2 = q1.clone();
    q2.id = Id::parse_str("019e4444-0000-7000-8000-0000000000ab").unwrap();
    q2.text = "question two".into();
    store.create_question(&q1).unwrap();
    store.create_question(&q2).unwrap();
    let err = store.get_question("019e4444-0000").unwrap_err();
    assert!(matches!(err, StoreError::Ambiguous(_)), "{err}");
    assert_eq!(
        store
            .get_question("019e4444-0000-7000-8000-0000000000ab")
            .unwrap()
            .id,
        q2.id
    );
    // Exact ids still resolve. (These fixtures differ only in the LAST
    // character, so every shorter prefix is shared — and must stay
    // ambiguous; a genuinely unique prefix still resolving is pinned by the
    // get_execution ambiguity test above.)
    assert_eq!(
        store
            .get_question("019e4444-0000-7000-8000-0000000000aa")
            .unwrap()
            .id,
        q1.id
    );
}

/// REQ-BIND-02 (C5 D1): the project-binding writer at store level. An
/// effective change bumps `lock_version`, moves `project_id`, and appends
/// ONE `task.project.set` event carrying the vocabulary payload keys — all
/// in one transaction. A stale optimistic base refuses with `Conflict` and
/// leaves the row exactly as it was. `workflow_events` stays append-only
/// throughout: the `wf_events_no_update`/`wf_events_no_delete` triggers are
/// never exercised by this path (it only INSERTs).
#[test]
fn set_task_project_binds_bumps_lock_appends_event_refuses_stale() {
    let (_d, store) = open();
    let t = draft(&store);
    let p = Project::new("binding", "", vec![], now());
    store.create_project(&p).unwrap();

    let before = store.get_task(&t.id.to_string()).unwrap();
    let updated = store
        .set_task_project(&before, p.id, "operator", now())
        .expect("binding lands");
    assert_eq!(updated.project_id, Some(p.id));
    assert_eq!(updated.lock_version, before.lock_version + 1);

    let stored = store.get_task(&t.id.to_string()).unwrap();
    assert_eq!(stored.project_id, Some(p.id));
    assert_eq!(stored.lock_version, before.lock_version + 1);
    assert_eq!(stored.state, before.state);
    assert_eq!(stored.requirements_seq, before.requirements_seq);

    // Exactly one new event of the binding kind, payload speaks the
    // json_field vocabulary (project_id / previous_project_id).
    let events = store.all_events_for_task(t.id).unwrap();
    let sets: Vec<_> = events
        .iter()
        .filter(|e| e.kind == event_kind::TASK_PROJECT_SET)
        .collect();
    assert_eq!(sets.len(), 1, "one append-only event per effective change");
    assert_eq!(sets[0].actor, "operator");
    assert_eq!(
        sets[0].payload[json_field::PROJECT_ID].as_str(),
        Some(p.id.to_string().as_str())
    );
    assert!(sets[0].payload[json_field::PREVIOUS_PROJECT_ID].is_null());

    // Stale base (someone else moved the row since `before` was loaded):
    // typed Conflict, the row keeps the FIRST binding, no second event.
    let err = store
        .set_task_project(&before, p.id, "late-writer", now())
        .expect_err("a stale lock_version must refuse");
    assert!(
        matches!(&err, StoreError::Conflict { expected, found, .. }
            if *expected == before.lock_version && *found == stored.lock_version),
        "{err}"
    );
    assert_eq!(
        store.get_task(&t.id.to_string()).unwrap().lock_version,
        stored.lock_version
    );
    let events = store.all_events_for_task(t.id).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == event_kind::TASK_PROJECT_SET)
            .count(),
        1,
        "the refused write left no event"
    );
}

// ---------------------------------------------------------------------------
// Task 01a1ad1d (REQ-TN-01/03, ratified OQ-TN-1/2/5): the human signboard
// is task DATA — write path, event trail, read-plane signboards on edges —
// and the formal title is never rewritten.
// ---------------------------------------------------------------------------

#[test]
fn human_name_roundtrip_event_and_title_immutable() {
    let (_d, store) = open();
    let t = draft(&store);
    // A fresh task carries no name (legacy shape — ratified OQ-TN-2).
    assert_eq!(t.human_name, None);

    let updated = store
        .set_task_human_name(&t, Some("Понятные имена"), "guerrero", now())
        .unwrap();
    assert_eq!(updated.human_name.as_deref(), Some("Понятные имена"));
    assert_eq!(updated.lock_version, t.lock_version + 1);

    let reloaded = store.get_task(&t.id.to_string()).unwrap();
    assert_eq!(reloaded.human_name.as_deref(), Some("Понятные имена"));
    // REQ-TN-01 AC2: the rename never touches the creator's verbatim title.
    assert_eq!(reloaded.title, "test task");

    // REQ-TN-01 AC3: ONE current name — the second write REPLACES the value
    // (no accumulation), and each EFFECTIVE change appends one event.
    let second = store
        .set_task_human_name(&reloaded, Some("Второе имя"), "guerrero", now())
        .unwrap();
    assert_eq!(second.human_name.as_deref(), Some("Второе имя"));
    assert_eq!(
        store
            .get_task(&t.id.to_string())
            .unwrap()
            .human_name
            .as_deref(),
        Some("Второе имя")
    );
    let names: Vec<_> = store
        .events(t.id, None)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == event_kind::TASK_HUMAN_NAME_SET)
        .collect();
    assert_eq!(names.len(), 2, "one append-only event per effective change");
    assert_eq!(names[0].payload[json_field::HUMAN_NAME], "Понятные имена");

    // Optimistic lock: a stale base refuses typed Conflict, writes nothing.
    let err = store
        .set_task_human_name(&t, Some("race"), "x", now())
        .expect_err("stale lock must refuse");
    assert!(matches!(err, StoreError::Conflict { .. }), "{err}");
}

#[test]
fn read_plane_hands_finished_signboards_on_edges_and_graph() {
    let (_d, store) = open();

    // a: a named task — the stamp travels the REAL publication-shaped path
    // (manual rename door writes the same column the publication stamps).
    let mut a = draft(&store);
    a = store
        .set_task_human_name(&a, Some("Имя по смыслу спека"), "guerrero", now())
        .unwrap();

    // b: DRAFT with a multi-sentence, multi-line statement (REQ-TN-03 showcase).
    let b = {
        let t = Task::new(
            "formal title b",
            "Первое предложение боли.\nВторое — уже детали запуска.",
            None,
            now(),
        );
        store.create_task(&t).unwrap();
        store.get_task(&t.id.to_string()).unwrap()
    };

    // c: legacy task past analysis WITHOUT a name (OQ-TN-2: keeps the title).
    let mut c = draft(&store);
    commit(
        &store,
        &c,
        workflow::start(&proc(), &c, "u", now()).unwrap(),
    );
    c = store.get_task(&c.id.to_string()).unwrap();
    commit(
        &store,
        &c,
        workflow::statement_approved(&proc(), &c, "checker", now()).unwrap(),
    );
    c = store.get_task(&c.id.to_string()).unwrap();
    publish(
        &store,
        &c,
        ArtifactKind::RequirementsSpec,
        b"legacy spec",
        vec![],
    );
    let arts = store.artifacts(c.id).unwrap();
    commit(
        &store,
        &c,
        workflow::requirements_published(&proc(), &c, &arts, None, None, "analyst", now()).unwrap(),
    );
    let c = store.get_task(&c.id.to_string()).unwrap();
    assert_eq!(c.human_name, None);
    assert_eq!(c.state, TaskState::Review);

    // a depends on b and on c: the EDGE rows must carry finished signboards
    // for BOTH ends (ratified OQ-TN-5), titles stay machine-readable.
    let e1 = store
        .link_task_dependency(a.id, b.id, DependencyKind::Blocks, "tester", now())
        .unwrap();
    assert_eq!(e1.task_display, "Имя по смыслу спека");
    assert_eq!(e1.depends_on_display, "Первое предложение боли.");
    assert_eq!(e1.task_title, "test task");
    assert_eq!(e1.depends_on_title, "formal title b");

    let e2 = store
        .link_task_dependency(a.id, c.id, DependencyKind::Parent, "tester", now())
        .unwrap();
    assert_eq!(e2.depends_on_display, "test task"); // REVIEW без имени → title

    // The same rule arrives through the graph read (dependency_graph).
    let graph = store.dependency_graph().unwrap();
    let node_a = graph.iter().find(|n| n.task.id == a.id).unwrap();
    assert_eq!(node_a.task.display_sign(), "Имя по смыслу спека");
    let outs: Vec<_> = node_a
        .dependencies
        .iter()
        .map(|d| (d.depends_on_id, d.depends_on_display.clone()))
        .collect();
    assert!(outs.contains(&(b.id, "Первое предложение боли.".into())));
    assert!(outs.contains(&(c.id, "test task".into())));

    // DRAFT without a name reads its FIRST SENTENCE through task.get/list.
    let listed = store.list_tasks().unwrap();
    let lb = listed.iter().find(|t| t.id == b.id).unwrap();
    assert_eq!(lb.display_sign(), "Первое предложение боли.");
    // A DRAFT without any statement falls to the title (AC3: вывеска не пуста).
    let bare = {
        let t = Task::new("bare title", "", None, now());
        store.create_task(&t).unwrap();
        store.get_task(&t.id.to_string()).unwrap()
    };
    assert_eq!(bare.display_sign(), "bare title");
    // Legacy REVIEW keeps the title as its signboard (OQ-TN-2, no forced rename).
    assert_eq!(c.display_sign(), "test task");
}

/// REQ-MS-02 AC5 (task 01a1b71f): the «one marked record» rule is held by the
/// DATABASE, not only by the switch code — the partial unique index refuses a
/// second `active=1` row written by any path.
#[test]
fn the_partial_index_refuses_a_second_marked_agent_record() {
    let (d, store) = open();
    for name in ["first", "second"] {
        store
            .put_agent(&AgentInstance {
                id: new_id(),
                name: name.into(),
                provider: "hermes".into(),
                model: String::new(),
                runtime: String::new(),
                reasoning: String::new(),
                created_at: now(),
                active: false,
            })
            .unwrap();
    }
    let raw = rusqlite::Connection::open(d.path().join("devai.db")).unwrap();
    raw.execute("UPDATE agent_instances SET active=1 WHERE name='first'", [])
        .unwrap();
    let err = raw
        .execute(
            "UPDATE agent_instances SET active=1 WHERE name='second'",
            [],
        )
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("unique"),
        "the second marker must violate the partial unique index: {err}"
    );
    // The read stays single-valued: the FIRST marked record.
    assert_eq!(store.active_agent().unwrap().unwrap().name, "first");
}

/// R4 quality return: the snapshot's «идущие прогоны» is an ADDRESSED read —
/// only RUNNING rows come back, never the whole execution history.
#[test]
fn running_executions_reads_only_running_rows() {
    let (_d, store) = open();
    let t1 = draft(&store);
    let t2 = draft(&store);
    let row = |task: Id, status: ExecutionStatus| RoleExecution {
        id: new_id(),
        task_id: task,
        role_slug: "implementation-agent".into(),
        role_version: 1,
        agent_name: Some("hermes".into()),
        status,
        inputs: vec![],
        summary: None,
        error: None,
        started_at: now(),
        finished_at: None,
        workspace_revision: None,
        attempt: 1,
        parent_execution: None,
        failure_class: None,
        owner_pid: None,
        owner_host: None,
        owner_process_start: None,
        bundle: None,
        run_id: None,
    };
    let live = row(t1.id, ExecutionStatus::Running);
    let done = row(t2.id, ExecutionStatus::Running);
    store.start_execution(&live).unwrap();
    store.start_execution(&done).unwrap();
    store
        .finish_execution_classified(&done, ExecutionStatus::Completed, None, None, None, now())
        .unwrap();

    let running = store.running_executions().unwrap();
    assert_eq!(running.len(), 1, "only the RUNNING row is listed");
    assert_eq!(running[0].id, live.id);
    assert_eq!(running[0].status.as_str(), "RUNNING");
}

/// The process declaration every workflow command resolves its target from
/// (REQ-AR-09): the factory bytes, parsed once. A test that changes the
/// declaration changes the walk — that is the point of the data.
fn proc() -> &'static devai_core::process::Process {
    devai_core::process::Process::factory()
}
