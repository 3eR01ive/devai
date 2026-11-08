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
//! * [`diff`] — the human field diff (its typed paragraph states and its
//!   typed refusal);
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
    DiffLine, DiffParagraphState, DiffRefusal, PreviousView, ReviewView, SignatureFact,
    SignatureKind, SignatureSource, SignatureView, SpecDiff, SpecView,
};

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use devai_core::types::{
        ArtifactKind, ArtifactStatus, DataCategory, Decision, Task, TaskState, Verdict,
        decision_kind, event_kind, new_id,
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
            parse_spec_body(Ok("не json [1, 2"), &task),
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

    /// The spec projection of a one-paragraph body, at a given status.
    fn spec_view(status: ArtifactStatus, current: bool) -> SpecView {
        parse_spec_body(Ok(r#"{"human_paragraph":"x"}"#), &task())
            .view(&artifact(1, status), current)
    }

    /// REQ-SIGN-07 AC1: the approval denial names the FIRST failed conjunct,
    /// in the ratified order.
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
        let stale = spec_view(ArtifactStatus::Stale, false);
        assert_eq!(
            approve_denied(TaskState::Approval, Some(&stale), 1),
            Some("spec_not_current")
        );
        let current = spec_view(ArtifactStatus::Current, true);
        assert_eq!(
            approve_denied(TaskState::Approval, Some(&current), 0),
            Some("no_requirements_version")
        );
        assert_eq!(approve_denied(TaskState::Approval, Some(&current), 1), None);
    }

    /// REQ-SIGN-07 AC2 and rework R7/R0: the two rights are independent, and
    /// every code those FOUR return sites can produce lives in the ONE const
    /// list `DENIAL_CODES` the UI parity test also reads.
    #[test]
    fn both_rights_are_independent_and_every_code_is_a_known_one() {
        assert_eq!(reject_denied(TaskState::Approval), None);
        assert_eq!(reject_denied(TaskState::Analysis), Some("not_approval"));
        let stale = spec_view(ArtifactStatus::Stale, false);
        let current = spec_view(ArtifactStatus::Current, true);
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

    /// REQ-SIGN-11 branch (2): @1 has no human_paragraph — the STORED summary
    /// becomes the paragraph, labelled as a summary.
    #[test]
    fn a_report_without_a_human_paragraph_shows_its_summary() {
        let parsed = parse_review_body(Ok(r#"{"verdict":"approved","summary":"Коротко"}"#));
        assert_eq!(parsed.verdict.as_deref(), Some("approved"));
        assert_eq!(parsed.paragraph_source, ReviewParagraphSource::Summary);
        assert_eq!(parsed.human_paragraph.as_deref(), Some("Коротко"));
        assert!(parsed.note.is_some());
    }

    /// REQ-SIGN-11: an EMPTY paragraph is the same as none; a readable body
    /// WITHOUT a verdict still shows the stored summary and names the gap.
    #[test]
    fn a_blank_paragraph_and_a_missing_verdict_are_named_but_not_faked() {
        let blank = parse_review_body(Ok(
            r#"{"verdict":"approved","human_paragraph":"  ","summary":"S"}"#,
        ));
        assert_eq!(blank.paragraph_source, ReviewParagraphSource::Summary);

        let no_verdict = parse_review_body(Ok(r#"{"summary":"S"}"#));
        assert!(no_verdict.verdict.is_none());
        assert_eq!(no_verdict.paragraph_source, ReviewParagraphSource::Summary);
        assert_eq!(no_verdict.human_paragraph.as_deref(), Some("S"));
        assert!(no_verdict.note.is_some());
    }

    /// REQ-SIGN-11 branch (3): @2 keeps the report's human paragraph as-is.
    #[test]
    fn a_report_paragraph_is_printed_as_stored() {
        let parsed = parse_review_body(Ok(
            r#"{"verdict":"needs_revision","human_paragraph":"Абзац"}"#,
        ));
        assert_eq!(parsed.paragraph_source, ReviewParagraphSource::ReviewReport);
        assert_eq!(parsed.human_paragraph.as_deref(), Some("Абзац"));
    }

    /// REQ-SIGN-11 branch (5): an unreadable body states its reason, never
    /// panics and never invents a verdict or a paragraph.
    #[test]
    fn an_unreadable_review_body_states_its_reason() {
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
    }

    /// REQ-SIGN-13: the diff compares PARSED fields — reformatting the JSON
    /// is not a spec change.
    #[test]
    fn reformatting_a_spec_is_not_a_change() {
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
    }

    /// REQ-SIGN-13: a changed statement shows as -/+ and an added requirement
    /// as a + line carrying its own id.
    #[test]
    fn changed_and_added_requirements_show_as_lines() {
        let task = task();
        let old = parse_spec_body(
            Ok(
                r#"{"requirements":[{"id":"REQ-1","statement":"Старое","acceptance_criteria":["A"]}]}"#,
            ),
            &task,
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

    /// The stored inputs of an approvable card: one CURRENT spec with
    /// structure, one reviewer report and one approval decision row.
    fn publish_approvable_inputs(store: &Store, task: &Task) {
        publish(
            store,
            task,
            ArtifactKind::RequirementsSpec,
            r#"{"human_paragraph":"Абзац спеки.\nВторая строка.","requirements":[{"id":"REQ-1","statement":"Сделать","acceptance_criteria":["AC1"]}],"open_questions":["OQ-1"]}"#,
        );
        publish(
            store,
            task,
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
    }

    /// R11: the assembled answer reads the stored spec (with structure) and
    /// the reviewer block in one pass.
    #[test]
    fn the_answer_reads_the_spec_and_the_review() {
        let (_dir, store, task) = store_with_task();
        publish_approvable_inputs(&store, &task);
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
    }

    /// R11: the same answer surfaces the stored decision row as the signature
    /// fact, and a CURRENT spec with a requirements version is signable.
    #[test]
    fn the_answer_surfaces_the_stored_signature_fact() {
        let (_dir, store, task) = store_with_task();
        publish_approvable_inputs(&store, &task);
        let view = signature_view(&store, task).unwrap();
        let fact = view
            .signature
            .expect("the decision row is the signature fact");
        assert_eq!(fact.kind, SignatureKind::Approval);
        assert_eq!(fact.source, SignatureSource::Decision);
        assert_eq!(fact.rationale.as_deref(), Some("принято"));
        assert_eq!(fact.spec_seq, None, "no ref → no guessed version");
        assert!(view.may_approve, "CURRENT spec, requirements seq > 0");
    }

    /// Rework R1 (the auditor's probe, reproduced on the shipped answer): a
    /// READABLE pre-protocol previous version has no paragraph of its own.
    /// The card falls back to the business description for its OWN slot, but
    /// the diff must not present that fallback as the previous version's
    /// paragraph — the state is typed, the sentence names the side, and the
    /// requirements of a readable body still diff.
    #[test]
    fn a_pre_protocol_previous_version_is_not_diffed_as_its_paragraph() {
        let (_dir, store, task) = store_with_task();
        publish(
            &store,
            &task,
            ArtifactKind::RequirementsSpec,
            r#"{"version":2,"requirements":[{"id":"REQ-1","statement":"Написано до протокола"}]}"#,
        );
        publish(
            &store,
            &task,
            ArtifactKind::RequirementsSpec,
            r#"{"human_paragraph":"Абзац текущей версии.","requirements":[{"id":"REQ-1","statement":"Написано до протокола"}]}"#,
        );
        let view = signature_view(&store, task).unwrap();
        assert_eq!(
            view.spec.as_ref().unwrap().paragraph_source,
            ParagraphSource::Spec,
            "the CURRENT version carries its own paragraph"
        );
        let previous = view.previous.expect("a previous version exists");
        let diff = &previous.diff;
        assert_eq!(
            diff.paragraph_previous,
            DiffParagraphState::PreProtocol,
            "a readable body without human_paragraph is pre-protocol, not empty"
        );
        assert_eq!(diff.paragraph_current, DiffParagraphState::Stored);
        assert!(diff.refusal.is_none(), "a readable body is not a refusal");
        assert!(
            diff.paragraph
                .iter()
                .all(|l| l.text != "бизнес-описание задачи"),
            "the business-description fallback is never a version's paragraph: {:?}",
            diff.paragraph
        );
        assert!(
            diff.paragraph
                .iter()
                .any(|l| l.mark == "+" && l.text == "Абзац текущей версии."),
            "the version that DOES have a paragraph is shown: {:?}",
            diff.paragraph
        );
        let note = diff
            .paragraph_note
            .as_deref()
            .expect("the missing paragraph is stated");
        assert!(note.contains("у прошлой версии"), "{note}");
        assert!(
            diff.requirements.iter().all(|l| l.mark == " "),
            "the readable structure still diffs: {:?}",
            diff.requirements
        );
    }

    /// R3/R8: an UNREADABLE previous body is a visible refusal with the side
    /// named — never a diff of the business-description fallback.
    #[test]
    fn an_unreadable_previous_version_is_named_not_diffed() {
        let (_dir, store, task) = store_with_task();
        publish(&store, &task, ArtifactKind::RequirementsSpec, "[broken");
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

    /// A stored approval decision row with no rationale and no refs.
    fn record_approval_row(store: &Store, task: &Task) {
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
    }

    /// REQ-SIGN-06: a later decision row wins over an older event; the fact
    /// is reported as a decision row and `rationale=null` is a valid stored
    /// fact, printed as such.
    #[test]
    fn the_later_decision_row_wins_over_an_older_event() {
        let (_dir, store, task) = store_with_task();
        store
            .insert_event(
                Some(task.id),
                event_kind::REQUIREMENTS_REJECTED,
                "guerrero",
                serde_json::json!({"rationale": "рано"}),
            )
            .unwrap();
        record_approval_row(&store, &task);
        let view = signature_view(&store, task).unwrap();
        let fact = view.signature.expect("a fact exists");
        assert_eq!(fact.kind, SignatureKind::Approval);
        assert_eq!(
            fact.source,
            SignatureSource::Decision,
            "the later decision row wins"
        );
        assert_eq!(
            fact.rationale, None,
            "rationale=null is a valid stored fact"
        );
    }

    /// REQ-SIGN-06 AC4: an event with no later row is reported as an event,
    /// and its version is never recovered from the payload.
    #[test]
    fn an_event_is_reported_with_its_own_source() {
        let (_dir, store, task) = store_with_task();
        store
            .insert_event(
                Some(task.id),
                event_kind::REQUIREMENTS_APPROVED,
                "guerrero",
                serde_json::json!({"rationale": "ок"}),
            )
            .unwrap();
        let view = signature_view(&store, task).unwrap();
        let fact = view.signature.expect("the event is a fact");
        assert_eq!(fact.kind, SignatureKind::Approval);
        assert_eq!(fact.source, SignatureSource::Event);
        assert_eq!(fact.spec_seq, None);
    }

    /// Rework R4: selection is FAIL-CLOSED over the kind vocabulary. An event
    /// or decision kind the platform does not know must not be skipped in
    /// silence — the read refuses and names the kind, so a future signature
    /// kind becomes visible instead of vanishing from the operator's card.
    #[test]
    fn an_unknown_kind_stops_the_fact_selection_loudly() {
        let (_dir, store, task) = store_with_task();
        store
            .insert_event(
                Some(task.id),
                "signature.brand_new",
                "platform",
                serde_json::json!({}),
            )
            .unwrap();
        let err = signature_view(&store, task).unwrap_err();
        assert_eq!(err.code().0, "signature_unknown_kind");
        assert!(format!("{err}").contains("signature.brand_new"), "{err}");

        let (_dir2, store2, task2) = store_with_task();
        store2
            .record_decision(&Decision {
                id: new_id(),
                task_id: task2.id,
                kind: "brand_new_decision".into(),
                verdict: Verdict::Approved,
                actor: "platform".into(),
                rationale: None,
                refs: vec![],
                created_at: Utc::now(),
            })
            .unwrap();
        let err2 = signature_view(&store2, task2).unwrap_err();
        assert_eq!(err2.code().0, "signature_unknown_kind");
        assert!(format!("{err2}").contains("brand_new_decision"), "{err2}");
    }

    /// REQ-SIGN-06 / rework R3: the CLI word and the wire token are the SAME
    /// string for both closed sets — the Display impl cannot drift from serde.
    #[test]
    fn the_display_words_are_the_wire_tokens() {
        let wire = |v: &serde_json::Value| v.as_str().unwrap().to_string();
        for kind in SignatureKind::ALL {
            assert_eq!(
                kind.to_string(),
                wire(&serde_json::to_value(kind).unwrap()),
                "SignatureKind::{} prints another word than it serializes",
                kind
            );
        }
        for source in SignatureSource::ALL {
            assert_eq!(
                source.to_string(),
                wire(&serde_json::to_value(source).unwrap()),
                "SignatureSource::{} prints another word than it serializes",
                source
            );
        }
        for state in DiffParagraphState::ALL {
            assert_eq!(
                state.to_string(),
                wire(&serde_json::to_value(state).unwrap()),
                "DiffParagraphState::{} prints another word than it serializes",
                state
            );
        }
    }

    /// Every KNOWN non-signature kind stays a silent skip — the explicit
    /// decision list and `event_kind::ALL` keep today's behaviour.
    #[test]
    fn known_non_signature_kinds_are_skipped_without_an_error() {
        let (_dir, store, task) = store_with_task();
        store
            .insert_event(
                Some(task.id),
                event_kind::STATE_CHANGED,
                "engine",
                serde_json::json!({}),
            )
            .unwrap();
        store
            .record_decision(&Decision {
                id: new_id(),
                task_id: task.id,
                kind: decision_kind::QUALITY_RETURN.into(),
                verdict: Verdict::Returned,
                actor: "guerrero".into(),
                rationale: Some("вернуть".into()),
                refs: vec![],
                created_at: Utc::now(),
            })
            .unwrap();
        let view = signature_view(&store, task).unwrap();
        assert!(
            view.signature.is_none(),
            "not every decision is a signature"
        );
    }
}
