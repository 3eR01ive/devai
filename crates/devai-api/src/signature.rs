//! The signature read-plane (REQ-SIGN-01..13): everything the operator's
//! APPROVAL card needs to sign exactly what is stored, in ONE read.
//!
//! The parse rules used to live in the WINDOW (`devai-ui/src/commands.rs`:
//! `SpecPresentation`, `ParagraphSource`, `spec_presentation` and friends),
//! which made the window the only place able to present a spec body. The
//! `task.signature` operation is a `devai-api` read, so the rules move HERE
//! (REQ-SIGN-12): ONE function parses a stored spec body into ONE
//! intermediate value ([`ParsedSpec`]), from which two projections are
//! built — the existing [`SpecPresentation`] for the human-wait row (shape
//! unchanged, REQ-SIGN-10 AC7) and [`SpecView`] for the signature card.
//!
//! Module layout (rework R5 — one responsibility per module, the file name
//! says what is inside):
//! * [`body`] — reading a stored body as text, with a TYPED refusal;
//! * [`spec`] — parsing a spec body and its two projections;
//! * [`review`] — parsing a reviewer report (the five branches);
//! * [`selectors`] — which report / which previous spec;
//! * [`diff`] — the human field diff (and its typed refusal);
//! * [`denial`] — the refusal codes of the two signature rights (one
//!   dictionary, pinned against the UI labels by a parity test);
//! * [`fact`] — the newest stored signature act;
//! * [`answer`] — assembling the `SignatureView` from the above.
//!
//! Nothing here rewrites stored data: the body is read, parsed, projected.
//! `artifact.diff` is not introduced (registry.rs:386-405) — the human diff
//! is a projection over the SAME parsed fields (REQ-SIGN-13).

mod answer;
mod body;
mod denial;
mod diff;
mod fact;
mod review;
mod selectors;
mod spec;
mod types;

pub use answer::signature_view;
pub use body::{BodyError, BodyProblem, BodyProblemKind};
pub use denial::{DENIAL_CODES, approve_denied, reject_denied};
pub use diff::diff_specs;
pub use review::{ReviewParagraphSource, parse_review_body};
pub use selectors::{last_review_artifact, pick_previous_spec};
pub use spec::{
    ParagraphSource, RequirementView, SpecPresentation, parse_spec_body, spec_presentation,
};
pub use types::{
    DiffLine, DiffRefusal, PreviousView, ReviewView, SignatureFact, SignatureView, SpecDiff,
    SpecView,
};

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use devai_core::types::{
        ArtifactKind, DataCategory, Decision, Task, TaskState, Verdict, decision_kind, event_kind,
        new_id,
    };
    use devai_store::{ArtifactDraft, Store};

    fn task() -> Task {
        Task {
            id: new_id(),
            project_id: None,
            title: "дело".into(),
            business_description: "бизнес-описание задачи".into(),
            human_name: None,
            state: TaskState::Approval,
            return_state: None,
            requirements_seq: 2,
            lock_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn store_with_task() -> (tempfile::TempDir, Store, Task) {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let task = task();
        store.create_task(&task).unwrap();
        (dir, store, task)
    }

    fn publish(
        store: &Store,
        task: &Task,
        kind: ArtifactKind,
        body: &str,
    ) -> devai_core::types::Artifact {
        store
            .publish_artifact(ArtifactDraft {
                task_id: task.id,
                kind,
                name: format!(
                    "{kind:?}-{}.json",
                    Utc::now().timestamp_nanos_opt().unwrap()
                ),
                category: DataCategory::Artifact,
                content: body.as_bytes().to_vec(),
                media_type: "application/json".into(),
                contract: None,
                created_by: "analyst".into(),
                execution_id: None,
                depends_on: vec![],
            })
            .unwrap()
    }

    // ---- parsing rules -----------------------------------------------------

    /// REQ-SIGN-02 AC1/AC2: the paragraph is printed verbatim (with its line
    /// breaks), the requirements keep array order and their criteria; a body
    /// without structure keeps the paragraph and says so.
    #[test]
    fn spec_parsing_prints_stored_text_and_structure_verbatim() {
        let task = task();
        let body = r#"{
          "human_paragraph": "Строка один.\nСтрока два.",
          "requirements": [
            {"id": "REQ-1", "statement": "Сделать X", "acceptance_criteria": ["AC1", "AC2"], "constraints": ["C1"]},
            {"id": "REQ-2", "statement": "Сделать Y", "acceptance_criteria": []}
          ],
          "open_questions": ["OQ-1: вопрос?"]
        }"#;
        let parsed = parse_spec_body(Ok(body), &task);
        assert_eq!(parsed.paragraph_source, ParagraphSource::Spec);
        assert_eq!(
            parsed.human_paragraph.as_deref(),
            Some("Строка один.\nСтрока два.")
        );
        assert_eq!(parsed.requirements.len(), 2);
        assert_eq!(parsed.requirements[0].id, "REQ-1");
        assert_eq!(
            parsed.requirements[0].acceptance_criteria,
            vec!["AC1", "AC2"]
        );
        assert_eq!(parsed.requirements[1].id, "REQ-2");
        assert!(parsed.requirements_note.is_none());
        assert_eq!(parsed.open_questions, vec!["OQ-1: вопрос?"]);
        assert!(parsed.body_problem.is_none());

        // No structure: paragraph survives, the note is honest, nothing invented.
        let no_structure = r#"{"human_paragraph": "Абзац."}"#;
        let parsed = parse_spec_body(Ok(no_structure), &task);
        assert!(parsed.requirements.is_empty());
        assert!(parsed.requirements_note.is_some());
        assert_eq!(parsed.human_paragraph.as_deref(), Some("Абзац."));
    }

    /// Ratified OQ-GATE-4 carried over: unreadable body / pre-protocol spec
    /// fall back to the business description, labelled — never empty. The
    /// unreadable ones ALSO carry the typed problem (R3/R8), the readable
    /// pre-protocol one does not (its structure still diffs).
    #[test]
    fn unreadable_bodies_fall_back_to_the_labelled_business_description() {
        let task = task();
        let not_text = BodyError::NotText { size: 12 };
        for parsed in [
            parse_spec_body(Err(&not_text), &task),
            parse_spec_body(Ok("не json {{{"), &task),
        ] {
            assert_eq!(
                parsed.paragraph_source,
                ParagraphSource::BusinessDescription
            );
            assert!(parsed.unavailable_note.is_some());
            assert!(parsed.body_problem.is_some());
            assert_eq!(
                parsed.human_paragraph.as_deref(),
                Some("бизнес-описание задачи")
            );
        }
        let pre_protocol = parse_spec_body(Ok(r#"{"version":2}"#), &task);
        assert_eq!(
            pre_protocol.paragraph_source,
            ParagraphSource::BusinessDescription
        );
        assert!(pre_protocol.body_problem.is_none());
    }

    /// R10: a non-string array item is a VISIBLE anomaly, never a silent
    /// `to_string()` that looks like stored text.
    #[test]
    fn non_string_items_are_printed_as_anomalies() {
        let task = task();
        let body = r#"{
          "human_paragraph": "Абзац.",
          "open_questions": ["нормальный вопрос", {"ошибка": 1}],
          "requirements": [
            {"id": "REQ-1", "statement": "X", "acceptance_criteria": ["ok", 42]}
          ]
        }"#;
        let parsed = parse_spec_body(Ok(body), &task);
        assert_eq!(parsed.open_questions.len(), 2);
        assert!(parsed.open_questions[1].contains("аномальный элемент"));
        assert!(parsed.requirements[0].acceptance_criteria[1].contains("аномальный элемент"));
        assert!(
            parsed
                .requirements_note
                .as_deref()
                .is_some_and(|n| n.contains("нестрокового")),
            "the anomaly is named in the structure note: {:?}",
            parsed.requirements_note
        );
    }

    /// REQ-SIGN-07 AC1: the denial names the FIRST failed conjunct, in order.
    #[test]
    fn approve_denial_names_the_first_failed_conjunct() {
        assert_eq!(
            approve_denied(TaskState::Review, None, 0),
            Some("not_approval")
        );
        assert_eq!(
            approve_denied(TaskState::Approval, None, 0),
            Some("no_spec")
        );
        let mut view = parse_spec_body(Ok(r#"{"human_paragraph":"x"}"#), &task()).view(
            &artifact(1, devai_core::types::ArtifactStatus::Stale),
            false,
        );
        assert_eq!(
            approve_denied(TaskState::Approval, Some(&view), 1),
            Some("spec_not_current")
        );
        view.current = true;
        assert_eq!(
            approve_denied(TaskState::Approval, Some(&view), 0),
            Some("no_requirements_version")
        );
        assert_eq!(approve_denied(TaskState::Approval, Some(&view), 1), None);
        assert_eq!(reject_denied(TaskState::Approval), None);
        assert_eq!(reject_denied(TaskState::Analysis), Some("not_approval"));

        // R7: every code those FOUR return sites can produce lives in the ONE
        // dictionary the frontend parity test reads.
        let stale = parse_spec_body(Ok(r#"{"human_paragraph":"x"}"#), &task()).view(
            &artifact(1, devai_core::types::ArtifactStatus::Stale),
            false,
        );
        let current = view.clone();
        for code in [
            approve_denied(TaskState::Review, None, 0),
            approve_denied(TaskState::Approval, None, 0),
            approve_denied(TaskState::Approval, Some(&stale), 1),
            approve_denied(TaskState::Approval, Some(&current), 0),
            reject_denied(TaskState::Review),
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                DENIAL_CODES.contains(&code),
                "{code} is not in DENIAL_CODES"
            );
        }
    }

    fn artifact(
        seq: u32,
        status: devai_core::types::ArtifactStatus,
    ) -> devai_core::types::Artifact {
        devai_core::types::Artifact {
            id: new_id(),
            task_id: new_id(),
            kind: ArtifactKind::RequirementsSpec,
            name: format!("spec-v{seq}.json"),
            seq,
            category: DataCategory::Artifact,
            status,
            sha256: "0".repeat(64),
            size: 2,
            media_type: "application/json".into(),
            contract: None,
            created_by: "analyst".into(),
            execution_id: None,
            depends_on: vec![],
            created_at: Utc::now(),
        }
    }

    /// REQ-SIGN-11: the five review branches, each with a visible text.
    #[test]
    fn review_branches_are_all_visible() {
        // (2) @1: no human_paragraph → summary as the paragraph, labelled.
        let parsed = parse_review_body(Ok(r#"{"verdict":"approved","summary":"Коротко"}"#));
        assert_eq!(parsed.verdict.as_deref(), Some("approved"));
        assert_eq!(parsed.paragraph_source, ReviewParagraphSource::Summary);
        assert_eq!(parsed.human_paragraph.as_deref(), Some("Коротко"));
        assert!(parsed.note.is_some());

        // (3) @2: human_paragraph as stored.
        let parsed = parse_review_body(Ok(
            r#"{"verdict":"needs_revision","human_paragraph":"Абзац"}"#,
        ));
        assert_eq!(parsed.paragraph_source, ReviewParagraphSource::ReviewReport);
        assert_eq!(parsed.human_paragraph.as_deref(), Some("Абзац"));

        // Empty paragraph = absent → summary branch.
        let parsed = parse_review_body(Ok(
            r#"{"verdict":"approved","human_paragraph":"  ","summary":"S"}"#,
        ));
        assert_eq!(parsed.paragraph_source, ReviewParagraphSource::Summary);

        // (5) unreadable body: no verdict, a reason, no panic.
        for parsed in [
            parse_review_body(Err(&BodyError::NotText { size: 3 })),
            parse_review_body(Ok("не json")),
        ] {
            assert_eq!(parsed.paragraph_source, ReviewParagraphSource::None);
            assert!(
                parsed.note.is_some(),
                "every unreadable branch states its reason"
            );
            assert!(parsed.verdict.is_none());
        }
        // A readable body WITHOUT a verdict: the summary branch still shows
        // the stored summary, the missing verdict is named in a note.
        let parsed = parse_review_body(Ok(r#"{"summary":"S"}"#));
        assert!(parsed.verdict.is_none());
        assert_eq!(parsed.paragraph_source, ReviewParagraphSource::Summary);
        assert_eq!(parsed.human_paragraph.as_deref(), Some("S"));
        assert!(parsed.note.is_some());
    }

    /// REQ-SIGN-13: the diff compares PARSED fields — reformatting is not a
    /// change; a changed statement shows as -/+; an added requirement as +.
    #[test]
    fn spec_diff_is_human_and_by_parsed_fields() {
        let task = task();
        let old = parse_spec_body(
            Ok(
                r#"{"requirements":[{"id":"REQ-1","statement":"Старое","acceptance_criteria":["A"]}]}"#,
            ),
            &task,
        );
        let reformatted = parse_spec_body(
            Ok(
                "{\n  \"requirements\": [\n    {\"id\": \"REQ-1\", \"statement\": \"Старое\", \"acceptance_criteria\": [\"A\"]}\n  ]\n}\n",
            ),
            &task,
        );
        let same = diff_specs(&old, &reformatted);
        assert!(
            same.requirements.iter().all(|l| l.mark == " "),
            "reformatting must not read as a spec change"
        );

        let new = parse_spec_body(
            Ok(r#"{"requirements":[
              {"id":"REQ-1","statement":"Новое","acceptance_criteria":["A"]},
              {"id":"REQ-2","statement":"Добавлено","acceptance_criteria":["B"]}
            ]}"#),
            &task,
        );
        let diff = diff_specs(&old, &new);
        assert!(
            diff.requirements
                .iter()
                .any(|l| l.mark == "-" && l.text == "Старое")
        );
        assert!(
            diff.requirements
                .iter()
                .any(|l| l.mark == "+" && l.text == "Новое")
        );
        assert!(
            diff.requirements
                .iter()
                .any(|l| l.mark == "+" && l.label == "REQ-2")
        );
        assert!(diff.note.is_none());
    }

    // ---- the assembled answer (R11) ---------------------------------------

    /// R11 / REQ-SIGN-07: a task with NO spec still gets a complete answer —
    /// null spec, the honest reviewer note, no invented previous version and
    /// the two rights judged independently.
    #[test]
    fn the_answer_of_a_task_without_any_spec_is_honest() {
        let (_dir, store, task) = store_with_task();
        let view = signature_view(&store, task).unwrap();
        assert!(view.spec.is_none());
        assert_eq!(view.may_approve_denied.as_deref(), Some("no_spec"));
        assert!(!view.may_approve);
        assert!(view.may_reject, "reject is the state alone (ASM-2)");
        assert!(view.review.is_none());
        assert_eq!(view.review_note.as_deref(), Some("отчёта ревьювера нет"));
        assert!(view.previous.is_none());
        assert_eq!(view.previous_note.as_deref(), Some("прошлой версии нет"));
        assert!(view.signature.is_none());
    }

    /// R11: the assembled answer reads the stored spec (with structure), the
    /// reviewer block and the stored decision row in one pass.
    #[test]
    fn the_answer_reads_the_spec_the_review_and_the_signature_fact() {
        let (_dir, store, task) = store_with_task();
        publish(
            &store,
            &task,
            ArtifactKind::RequirementsSpec,
            r#"{"human_paragraph":"Абзац спеки.\nВторая строка.","requirements":[{"id":"REQ-1","statement":"Сделать","acceptance_criteria":["AC1"]}],"open_questions":["OQ-1"]}"#,
        );
        publish(
            &store,
            &task,
            ArtifactKind::ReviewReport,
            r#"{"verdict":"approved","human_paragraph":"Одобряю."}"#,
        );
        store
            .record_decision(&Decision {
                id: new_id(),
                task_id: task.id,
                kind: decision_kind::REQUIREMENTS_APPROVAL.into(),
                verdict: Verdict::Approved,
                actor: "guerrero".into(),
                rationale: Some("принято".into()),
                refs: vec![],
                created_at: Utc::now(),
            })
            .unwrap();

        let view = signature_view(&store, task).unwrap();
        let spec = view.spec.expect("spec is presented");
        assert!(spec.current);
        assert_eq!(spec.requirements.len(), 1);
        assert_eq!(spec.open_questions, vec!["OQ-1"]);
        let review = view
            .review
            .expect("review block is present whenever a report exists");
        assert_eq!(review.verdict.as_deref(), Some("approved"));
        assert_eq!(review.human_paragraph.as_deref(), Some("Одобряю."));
        let fact = view
            .signature
            .expect("the decision row is the signature fact");
        assert_eq!(fact.kind, "approval");
        assert_eq!(fact.source, "decision");
        assert_eq!(fact.rationale.as_deref(), Some("принято"));
        assert_eq!(fact.spec_seq, None, "no ref → no guessed version");
        assert!(view.may_approve, "CURRENT spec, requirements seq > 0");
    }

    /// R3/R8: an UNREADABLE previous body is a visible refusal with the side
    /// named — never a diff of the business-description fallback.
    #[test]
    fn an_unreadable_previous_version_is_named_not_diffed() {
        let (_dir, store, task) = store_with_task();
        publish(&store, &task, ArtifactKind::RequirementsSpec, "{broken");
        publish(
            &store,
            &task,
            ArtifactKind::RequirementsSpec,
            r#"{"human_paragraph":"Абзац.","requirements":[]}"#,
        );
        let view = signature_view(&store, task).unwrap();
        let previous = view.previous.expect("a previous version exists");
        assert!(
            previous.diff.paragraph.is_empty() && previous.diff.requirements.is_empty(),
            "no invented diff lines on a refused comparison"
        );
        let note = previous.diff.note.expect("the refusal reaches the answer");
        assert!(note.contains("прошлая версия не читается"), "{note}");
        let refusal = previous.diff.refusal.expect("typed refusal");
        assert!(refusal.previous.is_some());
        assert!(refusal.current.is_none());
    }

    /// REQ-SIGN-06: the newest act wins; the decision row is preferred over an
    /// older event, and an event is reported with its own source marker.
    #[test]
    fn the_newest_signature_fact_wins_and_the_source_is_named() {
        let (_dir, store, task) = store_with_task();
        store
            .insert_event(
                Some(task.id),
                event_kind::REQUIREMENTS_REJECTED,
                "guerrero",
                serde_json::json!({"rationale": "рано"}),
            )
            .unwrap();
        store
            .record_decision(&Decision {
                id: new_id(),
                task_id: task.id,
                kind: decision_kind::REQUIREMENTS_APPROVAL.into(),
                verdict: Verdict::Approved,
                actor: "guerrero".into(),
                rationale: None,
                refs: vec![],
                created_at: Utc::now(),
            })
            .unwrap();
        let view = signature_view(&store, task).unwrap();
        let fact = view.signature.expect("a fact exists");
        assert_eq!(fact.kind, "approval");
        assert_eq!(fact.source, "decision", "the later decision row wins");
        assert_eq!(
            fact.rationale, None,
            "rationale=null is a valid stored fact"
        );

        // An event with no later row is reported as an event.
        let (_dir2, store2, task2) = store_with_task();
        store2
            .insert_event(
                Some(task2.id),
                event_kind::REQUIREMENTS_APPROVED,
                "guerrero",
                serde_json::json!({"rationale": "ок"}),
            )
            .unwrap();
        let view2 = signature_view(&store2, task2).unwrap();
        let fact2 = view2.signature.expect("the event is a fact");
        assert_eq!(fact2.source, "event");
        assert_eq!(fact2.spec_seq, None);
    }
}
