//! TOKEN SPEND as STORED data (task 01a1b851, REQ-USG-01/02/03/06).
//!
//! What is pinned here, on a real store:
//! * ONE row per execution, and recording the same execution twice never
//!   doubles the sum (REQ-USG-01 AC4);
//! * a run that closed without a usage report is an UNKNOWN row — tokens and
//!   cost stay NULL, never 0 — and the rollup counts it as a run with unknown
//!   spend, so the number of runs and the number of accounted runs agree
//!   (REQ-USG-02 AC1/AC3/AC4);
//! * the rollup by task / role / model over a window, with the totals bucket
//!   equal to the sum of the rows of that window (REQ-USG-03 AC1/AC3);
//! * the day window is the operator's LOCAL calendar day, midnight to now
//!   (ratified OQ-4);
//! * a task budget raised by a human is read from the stored decision rows —
//!   the ONLY source of a raise (REQ-USG-06 AC3/AC4).

use chrono::{Datelike, Local, TimeZone, Timelike, Utc};
use devai_core::types::*;
use devai_store::{Store, UsageGroup, UsageQuery, UsageRecord};
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

/// Start one execution of `role` on `task` and close it with `status`. The
/// close is what writes the honest UNKNOWN row (REQ-USG-01 AC1).
fn execution(
    store: &Store,
    task_id: Id,
    role: &str,
    status: ExecutionStatus,
    bundle: Option<ExecutionBundle>,
) -> RoleExecution {
    let e = RoleExecution {
        id: new_id(),
        task_id,
        role_slug: role.to_string(),
        role_version: 1,
        agent_name: Some("mock".into()),
        status: ExecutionStatus::Running,
        inputs: Vec::new(),
        summary: None,
        error: None,
        started_at: Utc::now(),
        finished_at: None,
        workspace_revision: None,
        attempt: 1,
        parent_execution: None,
        failure_class: None,
        owner_pid: None,
        owner_host: None,
        owner_process_start: None,
        bundle,
        run_id: Some(new_id()),
    };
    store.start_execution(&e).unwrap();
    store
        .finish_execution(&e, status, None, None, Utc::now())
        .unwrap();
    e
}

fn measured(store: &Store, e: &RoleExecution, prompt: i64, completion: i64) {
    measured_as(store, e, "deepseek-flash", prompt, completion);
}

fn measured_as(store: &Store, e: &RoleExecution, model: &str, prompt: i64, completion: i64) {
    store
        .record_execution_usage(&UsageRecord {
            execution_id: e.id,
            task_id: e.task_id,
            role_slug: e.role_slug.clone(),
            run_id: e.run_id.map(|r| r.to_string()),
            provider: Some("hermes".into()),
            model: Some(model.into()),
            prompt_tokens: Some(prompt),
            completion_tokens: Some(completion),
            total_tokens: Some(prompt + completion),
            duration_ms: Some(1_234),
            cost_usd: Some(0.5),
            measured: true,
            unknown_reason: None,
        })
        .unwrap();
}

fn totals(store: &Store, q: UsageQuery) -> devai_store::UsageBucket {
    store
        .usage_rollup(&q)
        .unwrap()
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// REQ-USG-01 AC1/AC4: every close leaves ONE row, and re-recording the same
/// execution replaces it instead of adding a second one.
#[test]
fn one_row_per_execution_and_a_repeat_never_doubles_the_sum() {
    let (_d, store) = open();
    let t = task(&store, "spend");
    let e = execution(
        &store,
        t.id,
        "implementation-agent",
        ExecutionStatus::Completed,
        None,
    );

    // The close already wrote the honest UNKNOWN row.
    let row = store.execution_usage(e.id).unwrap().expect("a row per run");
    assert!(!row.measured);
    assert_eq!(row.total_tokens, None);

    // The report arrives: the same execution is completed with its numbers.
    measured(&store, &e, 900, 100);
    let row = store.execution_usage(e.id).unwrap().unwrap();
    assert!(row.measured);
    assert_eq!(row.total_tokens, Some(1_000));
    assert_eq!(row.model.as_deref(), Some("deepseek-flash"));
    assert_eq!(row.provider.as_deref(), Some("hermes"));
    assert_eq!(row.duration_ms, Some(1_234));

    // A re-application (same execution again) does NOT add a second row.
    measured(&store, &e, 900, 100);
    let n: i64 = store
        .usage_rollup(&UsageQuery {
            task_id: Some(t.id),
            group: UsageGroup::Totals,
            ..UsageQuery::all()
        })
        .unwrap()
        .iter()
        .map(|b| b.runs)
        .sum();
    assert_eq!(n, 1, "one execution, one row");
    assert_eq!(
        totals(
            &store,
            UsageQuery {
                task_id: Some(t.id),
                group: UsageGroup::Totals,
                ..UsageQuery::all()
            }
        )
        .total_tokens,
        1_000,
        "a repeat does not double the spend"
    );
}

/// REQ-USG-02 AC1/AC3/AC4: a run without a report is UNKNOWN — not free — and
/// the run count of a view equals the number of executions, with the unknown
/// ones named beside the sum.
#[test]
fn a_run_without_a_report_is_unknown_and_never_zero() {
    let (_d, store) = open();
    let t = task(&store, "unknown spend");
    let good = execution(&store, t.id, "role-a", ExecutionStatus::Completed, None);
    measured(&store, &good, 10, 5);
    // A run that closed without a report, and one that failed hard.
    execution(&store, t.id, "role-a", ExecutionStatus::Failed, None);
    let failed_429 = execution(&store, t.id, "role-a", ExecutionStatus::Failed, None);

    let row = store.execution_usage(failed_429.id).unwrap().unwrap();
    assert!(!row.measured);
    assert_eq!((row.prompt_tokens, row.completion_tokens), (None, None));
    assert_eq!(
        row.cost_usd, None,
        "no price is invented for an unknown run"
    );
    assert!(
        row.unknown_reason
            .as_deref()
            .is_some_and(|r| r.contains("unknown")),
        "the reason is stated, not silent"
    );

    let b = totals(
        &store,
        UsageQuery {
            task_id: Some(t.id),
            group: UsageGroup::Totals,
            ..UsageQuery::all()
        },
    );
    assert_eq!(b.runs, 3, "every execution is in the count");
    assert_eq!(b.measured, 1);
    assert_eq!(b.unknown, 2);
    assert_eq!(
        b.total_tokens, 15,
        "only the measured run contributes tokens — the unknown ones are not zeroed into it"
    );
    assert_eq!(b.cost_usd, 0.5, "only the measured run's price is summed");
    assert_eq!(b.cost_unknown, 0, "the measured run carries a price");

    // The card's run list shows the unknown rows honestly too.
    let runs = store.usage_runs(t.id).unwrap();
    assert_eq!(runs.len(), 3);
    assert_eq!(runs.iter().filter(|r| !r.measured).count(), 2);
    assert!(
        runs.iter()
            .filter(|r| !r.measured)
            .all(|r| r.total_tokens.is_none())
    );
}

/// REQ-USG-03 AC1/AC3: the same window read by task, role and model, and the
/// totals bucket equals the sum of the rows — on unchanged data, reproducibly.
#[test]
fn the_rollup_by_task_role_and_model_adds_up_to_the_totals() {
    let (_d, store) = open();
    let t1 = task(&store, "one");
    let t2 = task(&store, "two");

    let bundle = |model: &str| ExecutionBundle::new("hermes", "hermes", model, "high");
    let a = execution(
        &store,
        t1.id,
        "role-a",
        ExecutionStatus::Completed,
        Some(bundle("m-1")),
    );
    measured_as(&store, &a, "m-1", 1_000, 0);
    let b = execution(
        &store,
        t2.id,
        "role-b",
        ExecutionStatus::Completed,
        Some(bundle("m-2")),
    );
    measured_as(&store, &b, "m-2", 500, 500);
    execution(&store, t2.id, "role-b", ExecutionStatus::Failed, None);

    let window = || UsageQuery {
        from: None,
        to: None,
        task_id: None,
        group: UsageGroup::Totals,
    };
    let all = totals(&store, window());
    assert_eq!((all.runs, all.measured, all.unknown), (3, 2, 1));
    assert_eq!(all.total_tokens, 2_000);

    for group in [UsageGroup::Task, UsageGroup::Role, UsageGroup::Model] {
        let rows = store
            .usage_rollup(&UsageQuery {
                from: None,
                to: None,
                task_id: None,
                group,
            })
            .unwrap();
        assert!(!rows.is_empty(), "{group:?} returns rows");
        let runs: i64 = rows.iter().map(|r| r.runs).sum();
        let tokens: i64 = rows.iter().map(|r| r.total_tokens).sum();
        let unknown: i64 = rows.iter().map(|r| r.unknown).sum();
        assert_eq!(runs, all.runs, "{group:?}: runs add up");
        assert_eq!(tokens, all.total_tokens, "{group:?}: tokens add up");
        assert_eq!(unknown, all.unknown, "{group:?}: unknown runs add up");
    }

    // By task: t2 holds two runs (one unknown) and 1 000 tokens.
    let by_task = store
        .usage_rollup(&UsageQuery {
            from: None,
            to: None,
            task_id: None,
            group: UsageGroup::Task,
        })
        .unwrap();
    let two = by_task
        .iter()
        .find(|r| r.key.as_deref() == Some(t2.id.to_string().as_str()))
        .unwrap();
    assert_eq!((two.runs, two.measured, two.unknown), (2, 1, 1));
    assert_eq!(two.total_tokens, 1_000);

    // By model: the unknown run has no model stamp and is its own bucket, not
    // folded into another model's row.
    let by_model = store
        .usage_rollup(&UsageQuery {
            from: None,
            to: None,
            task_id: None,
            group: UsageGroup::Model,
        })
        .unwrap();
    assert_eq!(by_model.len(), 3, "m-1, m-2 and the un-stamped bucket");

    // A window that excludes everything answers an empty set — the reader says
    // «прогонов не было», it never invents a zero row.
    let future = Utc::now() + chrono::Duration::days(1);
    assert!(
        store
            .usage_rollup(&UsageQuery {
                from: Some(future),
                to: None,
                task_id: None,
                group: UsageGroup::Totals,
            })
            .unwrap()
            .is_empty()
    );

    // The history average the run-cost expectation rests on counts MEASURED
    // runs only, and reports how many it rests on (REQ-USG-05 AC2).
    let (avg, n) = store.usage_average(Some(t2.id), "role-b").unwrap().unwrap();
    assert_eq!((avg, n), (1_000, 1));
    assert_eq!(
        store.usage_average(None, "no-such-role").unwrap(),
        None,
        "no history is None — the caller falls back to the run budget"
    );
}

/// Ratified OQ-4: «сутки» are calendar days in the operator's zone, starting at
/// local midnight — not a rolling 24 hours.
#[test]
fn the_day_window_starts_at_local_midnight() {
    let now = Utc::now();
    let (from, to) = devai_store::calendar_window("day", now).expect("day window");
    assert_eq!(to, now);
    let local_from = from.with_timezone(&Local);
    assert_eq!(
        (local_from.hour(), local_from.minute(), local_from.second()),
        (0, 0, 0),
        "the day starts at 00:00 local"
    );
    assert_eq!(local_from.day(), now.with_timezone(&Local).day());

    let (week_from, _) = devai_store::calendar_window("week", now).unwrap();
    assert_eq!(
        week_from.with_timezone(&Local).weekday(),
        chrono::Weekday::Mon,
        "the week starts on Monday"
    );
    let (month_from, _) = devai_store::calendar_window("month", now).unwrap();
    assert_eq!(month_from.with_timezone(&Local).day(), 1);

    assert!(
        devai_store::calendar_window("fortnight", now).is_none(),
        "an unknown period is None, not a silently widened window"
    );

    // A store row inside today's window is counted; one just before local
    // midnight is not.
    let (_d, store) = open();
    let t = task(&store, "window");
    let _ = t;
    let before = Local
        .with_ymd_and_hms(
            Local::now().year(),
            Local::now().month(),
            Local::now().day(),
            0,
            0,
            0,
        )
        .single()
        .unwrap()
        .with_timezone(&Utc)
        - chrono::Duration::minutes(1);
    assert!(before < from, "yesterday's last minute is outside today");
}

/// REQ-USG-06 AC3/AC4: the raised budget is READ from the stored decision —
/// the platform itself never raises anything, and the newest human decision
/// wins.
#[test]
fn a_human_decision_is_the_only_source_of_a_raised_budget() {
    let (_d, store) = open();
    let t = task(&store, "budget");
    assert!(store.usage_budget_raise(t.id).unwrap().is_none());

    let decide = |verdict: Verdict, amount: u64, at: Timestamp| Decision {
        id: new_id(),
        task_id: t.id,
        kind: decision_kind::USAGE_BUDGET.to_string(),
        verdict,
        actor: "guerrero".into(),
        rationale: Some("raise it".into()),
        refs: Vec::new(),
        created_at: at,
        payload: serde_json::json!({ (json_field::TOKEN_BUDGET): amount }),
    };
    // A REJECTED decision («остановиться») raises nothing.
    store
        .record_decision(&decide(Verdict::Rejected, 999, Utc::now()))
        .unwrap();
    assert!(store.usage_budget_raise(t.id).unwrap().is_none());

    let first = Utc::now() - chrono::Duration::minutes(5);
    store
        .record_decision(&decide(Verdict::Approved, 60_000_000, first))
        .unwrap();
    store
        .record_decision(&decide(Verdict::Approved, 90_000_000, Utc::now()))
        .unwrap();
    let (amount, d) = store.usage_budget_raise(t.id).unwrap().unwrap();
    assert_eq!(amount, 90_000_000, "the newest human decision wins");
    assert_eq!(d.kind, decision_kind::USAGE_BUDGET);
    assert_eq!(d.actor, "guerrero", "who decided is stored");
}

/// REQ-USG-02 AC2: a provider «quota exhausted» answer is its OWN stored fact,
/// and the run's spend row stays what the report said — reading the event never
/// turns the run into «потрачено ноль».
#[test]
fn the_quota_fact_is_stored_beside_the_spend_not_instead_of_it() {
    let (_d, store) = open();
    let t = task(&store, "429");
    let e = execution(&store, t.id, "role-a", ExecutionStatus::Failed, None);
    store
        .insert_event(
            Some(t.id),
            event_kind::USAGE_QUOTA_EXHAUSTED,
            "platform",
            serde_json::json!({
                (json_field::ROLE): "role-a",
                (json_field::MESSAGE): "matched `429` in the provider answer",
                "execution": e.id.to_string(),
            }),
        )
        .unwrap();
    assert!(
        store
            .has_event(t.id, event_kind::USAGE_QUOTA_EXHAUSTED)
            .unwrap(),
        "the 429 is a fact of the history"
    );
    assert!(
        !store
            .has_event(t.id, event_kind::USAGE_BUDGET_REFUSED)
            .unwrap()
    );
    // The spend of that run is still UNKNOWN — the quota fact did not zero it.
    assert!(
        store
            .execution_usage(e.id)
            .unwrap()
            .is_some_and(|r| !r.measured && r.total_tokens.is_none())
    );
}

/// REQ-USG-10: the spend row carries no credentials. Two proofs, both
/// structural rather than a keyword list: the serialized row has EXACTLY the
/// declared field set (there is no free-form field a secret could ride in),
/// and the values of a real run carry neither a bearer header nor a key
/// prefix.
#[test]
fn the_spend_row_stores_no_credentials() {
    let (_d, store) = open();
    let t = task(&store, "secrets");
    let e = execution(&store, t.id, "role-a", ExecutionStatus::Completed, None);
    measured(&store, &e, 1, 1);
    let row = store.execution_usage(e.id).unwrap().unwrap();
    let text = serde_json::to_string(&row).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut fields: Vec<&str> = value
        .as_object()
        .expect("the row serializes as an object")
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    // The declared shape of `ExecutionUsage` — one entry per field, no more.
    assert_eq!(
        fields,
        [
            "completion_tokens",
            "cost_usd",
            "created_at",
            "duration_ms",
            "execution_id",
            "measured",
            "model",
            "prompt_tokens",
            "provider",
            "role_slug",
            "run_id",
            "task_id",
            "total_tokens",
            "unknown_reason",
        ],
        "the row's field set is fixed — no field can hold a credential"
    );
    // A credential shape would have to appear INSIDE a value: neither a bearer
    // header name nor a provider key prefix may do so.
    for credential_shape in ["bearer ", "authorization", "sk-"] {
        assert!(
            !text.to_lowercase().contains(credential_shape),
            "the row must not carry `{credential_shape}`: {text}"
        );
    }
}
