//! TOKEN SPEND as the operator reads it (task 01a1b851, REQ-USG-03/04/06/07/08/09)
//! on a REAL home.
//!
//! What is pinned here:
//! * the card, `usage.summary` and the overview screen read ONE source and
//!   report the same numbers — no surface re-adds a column of its own;
//! * a run with an unknown spend is named beside the sum, never folded in as 0;
//! * «без лимита» is spoken, never printed as «0 из 0», and an edited config
//!   value applies to the very next read (no rebuild);
//! * a misspelled budget key is a LOUD read error, not a silently disabled
//!   limit;
//! * the budget question's two answers behave as ratified: «поднять до N»
//!   stores the decision (who/how much/why/when) and raises the effective
//!   limit, «остановиться» raises nothing and leaves the task to the human;
//! * the forecast refuses to invent a horizon when there is too little data.

use chrono::Utc;
use devai_api::{Api, TaskCreateReq};
use devai_core::types::*;
use devai_core::workflow;
use devai_store::{Store, UsageRecord};
use tempfile::TempDir;

fn home() -> (TempDir, Api) {
    let dir = TempDir::new().unwrap();
    // The handle reads THIS home's config tree, so the budget edits below land
    // on the next read whatever `DEVAI_CONFIG` the host exports.
    let api = Api::open(dir.path(), "usage-tester")
        .unwrap()
        .with_config_dir(dir.path().join("config"));
    api.seed_config(None).unwrap();
    (dir, api)
}

fn task(api: &Api, title: &str) -> String {
    api.task_create(TaskCreateReq {
        title: title.into(),
        project_id: None,
        description: "token spend".into(),
        parent: Vec::new(),
        blocked_by: Vec::new(),
        idempotency_key: None,
    })
    .unwrap()
    .task
    .id
    .to_string()
}

/// Start and close one execution, optionally recording its spend.
fn run(store: &Store, task_id: Id, role: &str, tokens: Option<(i64, i64)>) -> RoleExecution {
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
        bundle: Some(ExecutionBundle::new(
            "hermes",
            "hermes",
            "deepseek-flash",
            "high",
        )),
        run_id: Some(new_id()),
    };
    store.start_execution(&e).unwrap();
    store
        .finish_execution(&e, ExecutionStatus::Completed, None, None, Utc::now())
        .unwrap();
    if let Some((prompt, completion)) = tokens {
        store
            .record_execution_usage(&UsageRecord {
                execution_id: e.id,
                task_id,
                role_slug: role.to_string(),
                run_id: Some("1".into()),
                provider: Some("hermes".into()),
                model: Some("deepseek-flash".into()),
                prompt_tokens: Some(prompt),
                completion_tokens: Some(completion),
                total_tokens: Some(prompt + completion),
                duration_ms: Some(1_000),
                cost_usd: Some(0.25),
                measured: true,
                unknown_reason: None,
            })
            .unwrap();
    }
    e
}

/// REQ-USG-03 AC1/AC4 + REQ-USG-07 + REQ-USG-09 AC3: one source, one set of
/// numbers across the card, the summary and the screen.
#[test]
fn the_card_the_summary_and_the_screen_report_one_source() {
    let (dir, api) = home();
    let id = task(&api, "parity");
    let store = Store::open(dir.path()).unwrap();
    let t = store.resolve_task(&id).unwrap();
    run(&store, t.id, "implementation-agent", Some((60_000, 4_000)));
    run(&store, t.id, "quality-auditor", Some((20_000, 6_000)));
    // One run that closed without a report: counted, unknown — not free.
    run(&store, t.id, "quality-auditor", None);

    let card = api.task_usage(&id).unwrap();
    assert_eq!(card.line, "израсходовано 90000 из 50000000");
    assert_eq!(card.spent, 90_000);
    assert_eq!(card.limit, Some(50_000_000));
    assert_eq!(card.runs, 3);
    assert_eq!(card.measured, 2);
    assert_eq!(card.unknown, 1, "the unknown run is named, never zeroed");
    assert_eq!(card.cost_usd, 0.5);
    assert_eq!(card.cost_unknown, 0);
    assert!(card.models.contains(&"deepseek-flash".to_string()));
    assert_eq!(card.rows.len(), 3);
    // The card names the task by the platform's ONE signboard rule: a draft
    // with a business description shows the description's first sentence, not
    // a second reading of `human_name`/`title` (REQ-TN-03).
    assert_eq!(card.task_name, "token spend");
    assert_eq!(
        card.task_name,
        t.display_sign(),
        "the card and the read plane name the task the same way"
    );

    // `usage.summary` of the same window: the summary total IS the card's sum.
    let day = api.usage_summary("day", "totals", None).unwrap();
    assert_eq!(day.totals.total_tokens, card.spent);
    assert_eq!(day.totals.runs, card.runs);
    assert_eq!(day.totals.unknown, card.unknown);
    assert_eq!(day.totals.measured, card.measured);
    assert_eq!(day.totals.prompt_tokens, 80_000);
    assert_eq!(day.totals.completion_tokens, 10_000);
    assert_eq!(day.period, "day");
    assert!(day.from.is_some(), "the window is stated, not implied");

    // By task the same number is one row, and by role it splits into two.
    let by_task = api.usage_summary("day", "task", None).unwrap();
    let row = by_task
        .rows
        .iter()
        .find(|r| r.key == id)
        .expect("the task's own row");
    assert_eq!(row.total_tokens, card.spent);
    assert_eq!(
        row.name.as_deref(),
        Some(card.task_name.as_str()),
        "the bucket carries the same signboard as the card"
    );
    let by_role = api.usage_summary("day", "role", None).unwrap();
    assert_eq!(by_role.rows.len(), 2);
    assert_eq!(
        by_role.rows.iter().map(|r| r.total_tokens).sum::<i64>(),
        card.spent,
        "role rows add up to the same sum"
    );

    // The whole screen: day, month, top tasks, forecast, limits.
    let overview = api.usage_overview().unwrap();
    assert_eq!(overview.day.totals.total_tokens, card.spent);
    assert_eq!(overview.month.totals.total_tokens, card.spent);
    assert_eq!(overview.top_tasks.len(), 1);
    assert_eq!(overview.top_tasks[0].key, id);
    assert_eq!(overview.limits.task, Some(50_000_000));
    assert_eq!(overview.limits.run, Some(8_000_000));
    assert_eq!(
        overview.forecast.available, false,
        "two measured runs are below the declared threshold"
    );
    assert!(overview.forecast.line.contains("данных мало"));

    // An unknown period / grouping is a typed refusal, not an empty table.
    assert!(api.usage_summary("fortnight", "task", None).is_err());
    assert!(api.usage_summary("day", "colour", None).is_err());
    // An unknown task is explained, not silently empty.
    assert!(api.task_usage("00000000").is_err());
}

/// REQ-USG-04 AC3/AC4: «без лимита» is spoken; an edited value is honoured by
/// the NEXT read without a rebuild; a typo is LOUD.
#[test]
fn budgets_are_declarative_spoken_and_validated_loudly() {
    let (dir, api) = home();
    let id = task(&api, "limits");
    let yaml = dir.path().join("config/workflow/usage.yaml");

    // Zero means NO limit, and the surfaces say so in words.
    std::fs::write(
        &yaml,
        "usage:\n  budgets:\n    run: 0\n    task: 0\n    day: 0\n  prices: []\n",
    )
    .unwrap();
    let card = api.task_usage(&id).unwrap();
    assert_eq!(card.limit, None);
    assert_eq!(card.limit_text, "без лимита");
    assert!(
        card.line.contains("без лимита"),
        "«{}» must not read as «0 из 0»",
        card.line
    );
    let limits = api.usage_summary("day", "totals", None).unwrap().limits;
    assert_eq!((limits.run, limits.task, limits.day), (None, None, None));
    assert_eq!(
        api.usage_overview().unwrap().forecast.available,
        false,
        "no day limit → nothing to exhaust"
    );

    // An edited budget applies at once — same process, same Api value.
    std::fs::write(
        &yaml,
        "usage:\n  budgets:\n    run: 1000000\n    task: 2500000\n    day: 10000000\n",
    )
    .unwrap();
    let card = api.task_usage(&id).unwrap();
    assert_eq!(card.limit, Some(2_500_000));
    assert_eq!(card.line, "израсходовано 0 из 2500000");

    // A misspelled key is a loud error naming the section — never «без лимита».
    std::fs::write(
        &yaml,
        "usage:\n  budgets:\n    run: 1000000\n    taks: 2500000\n",
    )
    .unwrap();
    let err = api.task_usage(&id).unwrap_err();
    let text = format!("{err}");
    assert!(
        text.to_lowercase().contains("usage") || text.contains("taks"),
        "the refusal names what is wrong: {text}"
    );
    // A negative budget cannot be silently dropped either.
    std::fs::write(&yaml, "usage:\n  budgets:\n    run: -5\n").unwrap();
    assert!(api.task_usage(&id).is_err());
}

/// REQ-USG-08: the day gate and the forecast speak only with numbers. The
/// gate appears when today's spend reached the declared day budget, the
/// forecast appears only with enough measured runs, and the model rollup
/// groups by the run's own model stamp.
#[test]
fn the_day_gate_and_the_forecast_are_numbers_not_inventions() {
    let (dir, api) = home();
    let id = task(&api, "screen");
    let store = Store::open(dir.path()).unwrap();
    let t = store.resolve_task(&id).unwrap();
    // Three measured runs — the factory threshold — of 6000 tokens in total.
    run(&store, t.id, "implementation-agent", Some((1_000, 500)));
    run(&store, t.id, "quality-auditor", Some((2_000, 500)));
    run(&store, t.id, "quality-auditor", Some((1_500, 500)));

    let overview = api.usage_overview().unwrap();
    assert!(overview.day_gate.is_none(), "a day with room has no gate");
    assert!(
        overview.forecast.available,
        "three measured runs are enough for the estimate"
    );
    assert!(overview.forecast.line.contains("оценка"));
    assert_eq!(overview.forecast.runs_used, 3);

    // By model: the spend of each model stamp, and the totals agree with it.
    let by_model = api.usage_summary("day", "model", None).unwrap();
    assert_eq!(by_model.rows.len(), 1);
    assert_eq!(by_model.rows[0].key, "deepseek-flash");
    assert_eq!(
        by_model.rows[0].total_tokens,
        overview.day.totals.total_tokens
    );
    assert_eq!(by_model.totals.total_tokens, 6_000);

    // A day budget already reached turns the gate ON with the numbers, and the
    // forecast says the budget is exhausted instead of inventing a horizon.
    let yaml = dir.path().join("config/workflow/usage.yaml");
    std::fs::write(
        &yaml,
        "usage:\n  budgets:\n    run: 8000000\n    task: 50000000\n    day: 6000\n  \
         forecast_min_runs: 3\n",
    )
    .unwrap();
    let overview = api.usage_overview().unwrap();
    let gate = overview.day_gate.expect("the gate names the exhausted day");
    assert!(gate.contains("6000"), "the gate names the numbers: {gate}");
    assert!(overview.forecast.available);
    assert!(
        overview.forecast.line.contains("исчерпан"),
        "an exhausted day is stated, not given a fake horizon: {}",
        overview.forecast.line
    );
}

/// REQ-USG-06: the two answers of the budget question. «Поднять до N» stores
/// the decision and raises the effective limit; «остановиться» raises nothing
/// and leaves the task stopped with the decision recorded.
#[test]
fn the_budget_question_answers_raise_or_stop_and_leave_a_decision() {
    let (dir, api) = home();
    let id = task(&api, "gate");
    api.task_start(&id).unwrap();
    api.statement_approve_for_tests(&id).unwrap();
    let store = Store::open(dir.path()).unwrap();
    let process = devai_core::process::Process::factory();

    // Park exactly the way the platform does: spent 60M of a 60M task budget,
    // one more run ~5M — the question proposes a raise to 120M.
    let t = store.resolve_task(&id).unwrap();
    let qid = new_id();
    let stop = workflow::UsageStop::new(
        "requirements-analyst",
        60_000_000,
        60_000_000,
        5_000_000,
        120_000_000,
    );
    let m =
        workflow::park_usage_budget(&process, &t, qid, &stop, PLATFORM_ACTOR, Utc::now()).unwrap();
    store.commit(t.lock_version, &m).unwrap();

    let parked = store.get_task(&id).unwrap();
    assert!(parked.state.is_hold());
    let q = store.list_questions(parked.id).unwrap().remove(0);
    assert_eq!(q.id, qid);
    assert!(q.options.contains(&"поднять до 120000000".to_string()));
    assert!(q.options.contains(&"остановиться".to_string()));
    assert!(q.text.contains("израсходовано 60000000 из 60000000"));

    // A free-form answer is refused: the platform does not guess a budget.
    assert!(
        api.question_answer(&qid.to_string(), "давай побольше")
            .is_err()
    );
    assert_eq!(
        api.task_usage(&id).unwrap().limit,
        Some(50_000_000),
        "nothing was raised by the refused answer"
    );

    // «Поднять до N»: decision stored (who, how much, why, when) and the
    // effective limit follows the decision.
    let answered = api
        .question_answer(&qid.to_string(), "поднять до 120000000")
        .unwrap();
    assert!(!answered.to_state.is_hold(), "the task moves again");
    let decisions = store.list_decisions(parked.id).unwrap();
    let d = decisions
        .iter()
        .find(|d| d.kind == decision_kind::USAGE_BUDGET)
        .expect("the raise is a stored decision");
    assert_eq!(d.verdict, Verdict::Approved);
    assert_eq!(d.actor, "usage-tester");
    assert_eq!(d.rationale.as_deref(), Some("поднять до 120000000"));
    assert_eq!(d.payload[json_field::TOKEN_BUDGET], 120_000_000_i64);
    assert_eq!(d.payload[json_field::SPENT], 60_000_000_i64);
    let card = api.task_usage(&id).unwrap();
    assert_eq!(card.limit, Some(120_000_000), "the human raise is in force");
    assert_eq!(card.raised_to, Some(120_000_000));

    // The platform never raises by itself: with no decision there is no raise.
    let other = task(&api, "stop");
    api.task_start(&other).unwrap();
    api.statement_approve_for_tests(&other).unwrap();
    let t2 = store.resolve_task(&other).unwrap();
    let q2 = new_id();
    let stop2 = workflow::UsageStop::new("requirements-analyst", 1_000, 1_000, 500, 2_000);
    let m =
        workflow::park_usage_budget(&process, &t2, q2, &stop2, PLATFORM_ACTOR, Utc::now()).unwrap();
    store.commit(t2.lock_version, &m).unwrap();
    let answered = api
        .question_answer(&q2.to_string(), "остановиться")
        .unwrap();
    assert!(
        answered.to_state.is_hold(),
        "a stop leaves the task stopped"
    );
    let stopped = store.get_task(&other).unwrap();
    assert!(stopped.state.is_hold());
    // The stop releases the task to its DECLARED human-custody hold and
    // remembers the stage it was stopped in — a held task with no open question
    // and no remembered stage would be a dead end (no question to answer, no
    // way back to work). The platform resumed nothing.
    assert_ne!(
        stopped.state, parked.state,
        "not the question hold any more"
    );
    assert!(
        stopped.return_state.is_some(),
        "the stopped stage is remembered for a later human release"
    );
    assert!(
        store
            .list_questions(stopped.id)
            .unwrap()
            .iter()
            .all(|q| q.status != QuestionStatus::Open),
        "the question is answered — nothing waits in the queue"
    );
    assert!(
        store
            .has_event(stopped.id, event_kind::USAGE_BUDGET_STOPPED)
            .unwrap(),
        "the stop is its own fact of the history"
    );
    assert_eq!(
        api.task_usage(&other).unwrap().limit,
        Some(50_000_000),
        "«остановиться» raises nothing"
    );
    let d = store
        .list_decisions(stopped.id)
        .unwrap()
        .into_iter()
        .find(|d| d.kind == decision_kind::USAGE_BUDGET)
        .expect("the stop is a stored decision");
    assert_eq!(d.verdict, Verdict::Rejected);
    assert_eq!(d.actor, "usage-tester");
}
