//! TOKEN CONTROL of a run (task 01a1b851): the driver's usage report, the
//! cost of a run, and the budget check that happens BEFORE any model call.
//!
//! Three things live here, all of them platform-side and provider-agnostic:
//!
//! * [`parse_usage_report`] — the ONE reader of the driver's own usage file.
//!   The report format is the DRIVER's business (its name arrives as data),
//!   the store's `execution_usage` shape is the platform's: this module is the
//!   boundary where the first becomes the second.
//! * [`usage_record`] — one execution's spend record: the tokens, the identity
//!   stamp and the cost. The cost is computed from the CONFIG price table
//!   (ratified OQ-3); a model with no declared price falls back to the number
//!   the report itself carried, and with neither the cost stays UNKNOWN — a
//!   zero is never substituted for an unknown price.
//! * [`check_budget`] — «стоп до траты» (REQ-USG-05): the remaining budget of
//!   each level against the EXPECTED cost of this run (the historical average
//!   of the task+role, else of the role, else the run budget as the
//!   conservative upper bound — ratified OQ-8). The caller refuses the launch
//!   when this answers `Some`, so no execution row, no prompt and no money
//!   ever exist for a run the budget cannot pay.

use chrono::Utc;
use devai_core::types::{Id, RoleExecution, Timestamp};
use devai_store::config::UsagePolicy;
use devai_store::{Store, StoreResult, UsageGroup, UsageQuery, UsageRecord};

/// What one driver usage report says, in the platform's own words. Absent
/// numbers stay `None` (unknown), never 0.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageReport {
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub api_calls: Option<i64>,
}

/// Why a usage report could not be read. Both cases are FACTS about the run,
/// recorded as the honest «расход неизвестен» row — never as a silent zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageReportError(pub String);

impl std::fmt::Display for UsageReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Read the driver's usage report. The hermes report carries the token
/// counters at the top level (`input_tokens` / `output_tokens` /
/// `total_tokens`), the provider's own cost estimate and the model/provider
/// identity. A field that is missing is unknown — an absent report is the
/// CALLER's decision (it records `None`), not an error of this function.
pub fn parse_usage_report(text: &str) -> Result<UsageReport, UsageReportError> {
    let v: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| UsageReportError(format!("usage report is not JSON: {e}")))?;
    if !v.is_object() {
        return Err(UsageReportError(
            "usage report is not a JSON object".to_string(),
        ));
    }
    let num = |key: &str| v.get(key).and_then(|x| x.as_i64());
    let prompt = num("input_tokens");
    let completion = num("output_tokens");
    let total = num("total_tokens").or_else(|| match (prompt, completion) {
        // A report that carries both halves but no sum is not a lie: the sum
        // is arithmetic on two stored numbers, not a guess.
        (Some(p), Some(c)) => Some(p + c),
        _ => None,
    });
    Ok(UsageReport {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: total,
        cost_usd: v.get("estimated_cost_usd").and_then(|x| x.as_f64()),
        model: v.get("model").and_then(|x| x.as_str()).map(str::to_string),
        provider: v
            .get("provider")
            .and_then(|x| x.as_str())
            .map(str::to_string),
        api_calls: num("api_calls"),
    })
}

/// The cost of a run (ratified OQ-3 answer): the CONFIG price table first —
/// dollars per million tokens, split into prompt and completion. With no
/// declared price the driver's own estimate is the measured fact; with
/// neither, the cost is UNKNOWN (`None`) — never 0.0 as a placeholder.
pub fn cost_of(report: &UsageReport, model: Option<&str>, policy: &UsagePolicy) -> Option<f64> {
    if let Some(price) = model.and_then(|m| policy.price_of(m)) {
        let prompt = report.prompt_tokens.unwrap_or(0) as f64;
        let completion = report.completion_tokens.unwrap_or(0) as f64;
        return Some(
            (prompt * price.input_per_million + completion * price.output_per_million)
                / 1_000_000.0,
        );
    }
    report.cost_usd.filter(|c| c.is_finite() && *c >= 0.0)
}

/// The spend record of one execution — measured when a report was read, the
/// honest unknown record when it was not.
#[allow(clippy::too_many_arguments)]
pub fn usage_record(
    execution: &RoleExecution,
    provider: Option<String>,
    model: Option<String>,
    duration_ms: Option<i64>,
    report: Option<&UsageReport>,
    policy: &UsagePolicy,
) -> UsageRecord {
    match report {
        None => UsageRecord::unknown(
            execution.id,
            execution.task_id,
            &execution.role_slug,
            execution.run_id.map(|r| r.to_string()),
            provider,
            model,
            duration_ms,
            "the run produced no usage report — spend unknown, not zero",
        ),
        Some(r) => {
            let model = model.or_else(|| r.model.clone());
            let provider = provider.or_else(|| r.provider.clone());
            UsageRecord {
                execution_id: execution.id,
                task_id: execution.task_id,
                role_slug: execution.role_slug.clone(),
                run_id: execution.run_id.map(|r| r.to_string()),
                provider,
                model: model.clone(),
                prompt_tokens: r.prompt_tokens,
                completion_tokens: r.completion_tokens,
                total_tokens: r.total_tokens,
                duration_ms,
                cost_usd: cost_of(r, model.as_deref(), policy),
                measured: true,
                unknown_reason: None,
            }
        }
    }
}

/// The budget LEVEL a refusal names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetLevel {
    Run,
    Task,
    Day,
}

impl BudgetLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Task => "task",
            Self::Day => "day",
        }
    }

    /// The human name of the level, used in the ONE refusal sentence.
    pub fn word(self) -> &'static str {
        match self {
            Self::Run => "прогона",
            Self::Task => "задачи",
            Self::Day => "суточный",
        }
    }
}

/// Where the EXPECTED cost of this run came from — printed in the refusal so
/// the operator knows whether the platform reasoned from history or from the
/// declared upper bound (REQ-USG-05 AC2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpectationBasis {
    TaskRole,
    Role,
    RunBudget,
}

impl ExpectationBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskRole => "task_role",
            Self::Role => "role",
            Self::RunBudget => "run_budget",
        }
    }

    pub fn words(self) -> &'static str {
        match self {
            Self::TaskRole => "историческое среднее по паре задача+роль",
            Self::Role => "историческое среднее по роли (по паре история не набралась)",
            Self::RunBudget => "бюджет прогона как оценка сверху (истории нет)",
        }
    }
}

/// A refusal to start a run: the numbers, the level and the ONE sentence the
/// operator reads on every surface (ack, event payload, report).
#[derive(Clone, Debug, PartialEq)]
pub struct BudgetRefusal {
    pub level: BudgetLevel,
    /// What this level already spent (0 for the run level — nothing ran yet).
    pub spent: i64,
    pub limit: u64,
    /// The expected cost of the run that was refused.
    pub expected: i64,
    pub basis: ExpectationBasis,
    pub line: String,
}

impl BudgetRefusal {
    /// The refusal sentence. It names the exhausted level, the numbers and
    /// the expectation — REQ-USG-05 AC4 («отказ называет цифры»).
    pub fn sentence(
        level: BudgetLevel,
        spent: i64,
        limit: u64,
        expected: i64,
        basis: ExpectationBasis,
    ) -> String {
        match level {
            BudgetLevel::Run => format!(
                "бюджет прогона исчерпан: прогон стоит ~{expected} токенов при лимите {limit} \
                 ({}) — нужен новый бюджет; прогон не начат, модель не вызывалась",
                basis.words()
            ),
            BudgetLevel::Task => format!(
                "бюджет задачи исчерпан: на задаче израсходовано {spent} из {limit}, прогон \
                 стоит ~{expected} ({}) — нужен новый бюджет; прогон не начат, модель не \
                 вызывалась",
                basis.words()
            ),
            BudgetLevel::Day => format!(
                "суточный бюджет исчерпан: за сутки израсходовано {spent} из {limit}, прогон \
                 стоит ~{expected} ({}) — новые прогоны не запускаются до новых суток; прогон \
                 не начат, модель не вызывалась",
                basis.words()
            ),
        }
    }
}

/// How much this run is expected to cost, and on what basis (ratified OQ-8:
/// the task+role average when the pair has history, else the role's average,
/// else the declared run budget as the conservative upper bound).
pub fn expected_run_cost(
    store: &Store,
    task_id: Id,
    role: &str,
    policy: &UsagePolicy,
) -> StoreResult<(i64, ExpectationBasis)> {
    if let Some((avg, n)) = store.usage_average(Some(task_id), role)? {
        if n > 0 {
            return Ok((avg, ExpectationBasis::TaskRole));
        }
    }
    if let Some((avg, n)) = store.usage_average(None, role)? {
        if n > 0 {
            return Ok((avg, ExpectationBasis::Role));
        }
    }
    Ok((
        policy.budgets.run.unwrap_or(0) as i64,
        ExpectationBasis::RunBudget,
    ))
}

/// The effective TASK budget: the declared config value, raised only by a
/// stored human decision (REQ-USG-06 AC3 — «никаких автоматических подъёмов»).
pub fn effective_task_budget(
    store: &Store,
    task_id: Id,
    policy: &UsagePolicy,
) -> StoreResult<Option<u64>> {
    let raised = store.usage_budget_raise(task_id)?.map(|(amount, _)| amount);
    Ok(match (policy.budgets.task, raised) {
        (Some(config), Some(raised)) => Some(config.max(raised)),
        (None, Some(raised)) => Some(raised),
        (config, None) => config,
    })
}

/// CHECK BEFORE ANY SPEND (REQ-USG-05). `None` = the run may start; `Some` =
/// the run is refused with the numbers, before the model is called and before
/// any execution row exists.
///
/// Levels are checked from the narrowest: a run that cannot pay for itself is
/// refused on the run level even when the task still has room; a task that
/// cannot pay for one more run is refused (and the caller parks it with the
/// human question); the DAY budget refuses new runs for the rest of the day
/// (ratified OQ-1: already-started runs are allowed to finish).
pub fn check_budget(
    store: &Store,
    task_id: Id,
    role: &str,
    policy: &UsagePolicy,
    now: Timestamp,
) -> StoreResult<Option<BudgetRefusal>> {
    // No history and no run budget: there is nothing to estimate from, so the
    // check stays inert — the platform never invents an expectation.
    let (expected, basis) = expected_run_cost(store, task_id, role, policy)?;
    let _ = now;

    if let Some(run_budget) = policy.budgets.run {
        if expected > run_budget as i64 {
            return Ok(Some(BudgetRefusal {
                level: BudgetLevel::Run,
                spent: 0,
                limit: run_budget,
                expected,
                basis,
                line: BudgetRefusal::sentence(BudgetLevel::Run, 0, run_budget, expected, basis),
            }));
        }
    }

    if let Some(task_budget) = effective_task_budget(store, task_id, policy)? {
        let spent = task_spent(store, task_id)?;
        if spent.saturating_add(expected) > task_budget as i64 {
            return Ok(Some(BudgetRefusal {
                level: BudgetLevel::Task,
                spent,
                limit: task_budget,
                expected,
                basis,
                line: BudgetRefusal::sentence(
                    BudgetLevel::Task,
                    spent,
                    task_budget,
                    expected,
                    basis,
                ),
            }));
        }
    }

    if let Some(day_budget) = policy.budgets.day {
        let spent = day_spent(store, now)?;
        if spent.saturating_add(expected) > day_budget as i64 {
            return Ok(Some(BudgetRefusal {
                level: BudgetLevel::Day,
                spent,
                limit: day_budget,
                expected,
                basis,
                line: BudgetRefusal::sentence(BudgetLevel::Day, spent, day_budget, expected, basis),
            }));
        }
    }

    Ok(None)
}

/// The task's measured spend over its whole life, from the store (never from
/// the filesystem): the sum of the known token counts of its runs.
pub fn task_spent(store: &Store, task_id: Id) -> StoreResult<i64> {
    let q = UsageQuery {
        from: None,
        to: None,
        task_id: Some(task_id),
        group: UsageGroup::Totals,
    };
    Ok(store
        .usage_rollup(&q)?
        .first()
        .map(|b| b.total_tokens)
        .unwrap_or(0))
}

/// Today's spend in the operator's LOCAL calendar day (ratified OQ-4).
pub fn day_spent(store: &Store, now: Timestamp) -> StoreResult<i64> {
    let (from, to) = devai_store::calendar_window("day", now)
        .unwrap_or((now, now + chrono::Duration::seconds(1)));
    let q = UsageQuery {
        from: Some(from),
        to: Some(to),
        task_id: None,
        group: UsageGroup::Totals,
    };
    Ok(store
        .usage_rollup(&q)?
        .first()
        .map(|b| b.total_tokens)
        .unwrap_or(0))
}

/// The provider's «квота исчерпана» answer, as EVIDENCE (REQ-USG-02 AC2): the
/// matched text, if the failure carries a 429 or a quota word. The caller
/// records it as its own event — the run's spend stays whatever the report
/// said (or unknown), it is never turned into «потрачено ноль».
pub fn quota_exhausted_evidence(text: &str) -> Option<String> {
    let haystack = text.to_lowercase();
    const MARKERS: &[&str] = &[
        "429",
        "insufficient_quota",
        "quota exceeded",
        "exceeded your current quota",
        "quota exhausted",
        "квота исчерпана",
        "rate limit",
    ];
    MARKERS
        .iter()
        .find(|m| haystack.contains(**m))
        .map(|m| format!("matched `{m}` in the provider answer"))
}

/// The duration of one execution in whole milliseconds, from stored times —
/// `None` when the row has no finish time (it never closed).
pub fn duration_ms(started: Timestamp, finished: Option<Timestamp>) -> Option<i64> {
    let finished = finished?;
    Some((finished - started).num_milliseconds().max(0))
}

/// «Сейчас» in one place, so the writer and the tests agree.
pub fn now() -> Timestamp {
    Utc::now()
}

/// The task's budget is exhausted: park it with ONE human question and record
/// the refusal (REQ-USG-06). This is the SINGLE implementation of that rule —
/// the API admission gate and the engine's pre-stage gate both call it, so the
/// two doors cannot drift apart in wording, numbers or parking behaviour.
///
/// Parking happens only when it is answerable (the state has a role) and no
/// human question is already open: one question per exhaustion FACT, never a
/// stack of them (ratified assumption). `declared` is the DECLARED task budget
/// the raise is proposed from — data, never a code literal. Returns `true`
/// when this call parked the task.
#[allow(clippy::too_many_arguments)]
pub fn park_task_exhaustion(
    store: &Store,
    process: &devai_core::process::Process,
    task: &devai_core::types::Task,
    role: &str,
    refusal: &BudgetRefusal,
    declared: Option<u64>,
    actor: &str,
    at: Timestamp,
) -> crate::RuntimeResult<bool> {
    if refusal.level != BudgetLevel::Task
        || process.role_for(task.state.as_str()).is_none()
        || task.state.is_hold()
        || store
            .list_questions(task.id)?
            .iter()
            .any(|q| q.status == devai_core::types::QuestionStatus::Open)
    {
        return Ok(false);
    }
    let declared = declared.unwrap_or(refusal.limit);
    let raise_to = (refusal.spent.max(0) as u64).saturating_add(declared);
    let m = devai_core::workflow::park_usage_budget(
        process,
        task,
        devai_core::types::new_id(),
        role,
        refusal.spent.max(0) as u64,
        refusal.limit,
        refusal.expected.max(0) as u64,
        raise_to,
        actor,
        at,
    )?;
    store.commit(task.lock_version, &m)?;
    Ok(true)
}

/// Record a refusal that did NOT park the task (the run and day levels, or a
/// task that was already held / already asking) as its own event, with the
/// same numbers the operator's sentence carries (REQ-USG-05 AC4).
pub fn record_refusal(
    store: &Store,
    task_id: Id,
    role: &str,
    refusal: &BudgetRefusal,
    actor: &str,
) -> StoreResult<()> {
    store.insert_event(
        Some(task_id),
        devai_core::types::event_kind::USAGE_BUDGET_REFUSED,
        actor,
        serde_json::json!({
            (devai_core::types::json_field::ROLE): role,
            (devai_core::types::json_field::LEVEL): refusal.level.as_str(),
            (devai_core::types::json_field::SPENT): refusal.spent,
            (devai_core::types::json_field::LIMIT): refusal.limit,
            (devai_core::types::json_field::EXPECTED): refusal.expected,
            (devai_core::types::json_field::BASIS): refusal.basis.as_str(),
            (devai_core::types::json_field::REASON): refusal.line,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use devai_store::config::{ModelPrice, TokenBudgets};

    fn report(json: &str) -> UsageReport {
        parse_usage_report(json).expect("report parses")
    }

    /// The real hermes report shape (an observed file): the tokens, the cost
    /// estimate and the identity are read; absent fields are unknown.
    #[test]
    fn the_driver_report_shape_is_read_without_inventing_numbers() {
        let r = report(
            r#"{"estimated_cost_usd":0.08,"input_tokens":9872773,"output_tokens":49670,
                "total_tokens":9922443,"api_calls":90,"model":"deepseek-flash",
                "provider":"deepseek"}"#,
        );
        assert_eq!(r.prompt_tokens, Some(9_872_773));
        assert_eq!(r.completion_tokens, Some(49_670));
        assert_eq!(r.total_tokens, Some(9_922_443));
        assert_eq!(r.cost_usd, Some(0.08));
        assert_eq!(r.model.as_deref(), Some("deepseek-flash"));
        assert_eq!(r.provider.as_deref(), Some("deepseek"));

        // A report with no token fields: every number is UNKNOWN, not zero.
        let empty = report(r#"{"model":"m"}"#);
        assert_eq!(empty.prompt_tokens, None);
        assert_eq!(empty.total_tokens, None);
        assert_eq!(empty.cost_usd, None);

        // A sum can be computed from the two halves — that is arithmetic on
        // stored numbers, not a guess.
        let halves = report(r#"{"input_tokens":10,"output_tokens":5}"#);
        assert_eq!(halves.total_tokens, Some(15));

        // Not JSON at all is a stated error, never an empty report.
        assert!(parse_usage_report("not json").is_err());
        assert!(parse_usage_report("[]").is_err());
    }

    /// Ratified OQ-3 answer: the config table prices the run when the model is
    /// declared; without a declared price the report's own estimate is used;
    /// with neither the cost is UNKNOWN — never 0.
    #[test]
    fn cost_prefers_the_config_table_then_the_report_and_never_invents_zero() {
        let mut policy = UsagePolicy::default();
        policy.prices = vec![ModelPrice {
            model: "priced".into(),
            input_per_million: 1.0,
            output_per_million: 2.0,
        }];
        let r = UsageReport {
            prompt_tokens: Some(2_000_000),
            completion_tokens: Some(1_000_000),
            total_tokens: Some(3_000_000),
            cost_usd: Some(9.99),
            ..UsageReport::default()
        };
        // 2M * 1.0 + 1M * 2.0, per million => 4.0 (the report's 9.99 is not used).
        assert_eq!(cost_of(&r, Some("priced"), &policy), Some(4.0));
        // Not in the table: the report's own cost is the measured fact.
        assert_eq!(cost_of(&r, Some("unknown-model"), &policy), Some(9.99));
        // Neither: UNKNOWN, not zero.
        let no_cost = UsageReport {
            cost_usd: None,
            ..r
        };
        assert_eq!(cost_of(&no_cost, Some("unknown-model"), &policy), None);
    }

    /// The refusal sentence names the level and the numbers — the ONE text the
    /// operator reads on every surface (REQ-USG-05 AC4).
    #[test]
    fn the_refusal_sentence_carries_the_numbers_and_the_level() {
        let line = BudgetRefusal::sentence(
            BudgetLevel::Task,
            18_000_000,
            20_000_000,
            3_000_000,
            ExpectationBasis::Role,
        );
        assert!(line.contains("бюджет задачи"));
        assert!(line.contains("18000000"));
        assert!(line.contains("20000000"));
        assert!(line.contains("~3000000"));
        assert!(line.contains("модель не вызывалась"));

        let run = BudgetRefusal::sentence(
            BudgetLevel::Run,
            0,
            1_000,
            5_000,
            ExpectationBasis::RunBudget,
        );
        assert!(run.contains("бюджет прогона"));
        assert!(run.contains("лимите 1000"));
        assert!(run.contains("~5000"));

        let day = BudgetRefusal::sentence(
            BudgetLevel::Day,
            199_000_000,
            200_000_000,
            4_000_000,
            ExpectationBasis::TaskRole,
        );
        assert!(day.contains("суточный бюджет"));
        assert!(day.contains("до новых суток"));
    }

    /// The 429/quota answer is recognised as its own evidence, and a plain
    /// failure is not mistaken for it.
    #[test]
    fn the_provider_quota_answer_is_recognised_as_its_own_fact() {
        assert!(quota_exhausted_evidence("HTTP 429 Too Many Requests").is_some());
        assert!(
            quota_exhausted_evidence("{\"error\":\"insufficient_quota\"}").is_some(),
            "the OpenAI-style quota code"
        );
        assert!(quota_exhausted_evidence("exit code 1: syntax error").is_none());
    }

    /// The expectation walks the ratified ladder: the pair's history, then the
    /// role's, then the declared run budget; and the budget check refuses
    /// BEFORE any spend, naming the level (REQ-USG-05).
    #[test]
    fn expectation_falls_back_from_history_to_the_run_budget_and_the_check_refuses() {
        let dir = std::env::temp_dir().join(format!("devai-usage-x-{}", uuid::Uuid::now_v7()));
        let store = Store::open(&dir).unwrap();
        let policy = UsagePolicy::default();
        let task = devai_core::types::Task {
            id: devai_core::types::new_id(),
            project_id: None,
            title: "t".into(),
            business_description: String::new(),
            human_name: None,
            state: devai_core::types::TaskState::Draft,
            return_state: None,
            requirements_seq: 0,
            lock_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        store.create_task(&task).unwrap();

        // No history at all: the run budget is the conservative expectation.
        let (expected, basis) = expected_run_cost(&store, task.id, "role-a", &policy).unwrap();
        assert_eq!(basis, ExpectationBasis::RunBudget);
        assert_eq!(expected, policy.budgets.run.unwrap() as i64);

        // One MEASURED run of the role gives that role a history.
        let exec = devai_core::types::RoleExecution {
            id: devai_core::types::new_id(),
            task_id: task.id,
            role_slug: "role-a".into(),
            role_version: 1,
            agent_name: None,
            status: devai_core::types::ExecutionStatus::Running,
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
            bundle: None,
            run_id: None,
        };
        store.start_execution(&exec).unwrap();
        store
            .finish_execution(
                &exec,
                devai_core::types::ExecutionStatus::Completed,
                None,
                None,
                Utc::now(),
            )
            .unwrap();
        store
            .record_execution_usage(&UsageRecord {
                execution_id: exec.id,
                task_id: task.id,
                role_slug: "role-a".into(),
                run_id: None,
                provider: Some("hermes".into()),
                model: Some("m".into()),
                prompt_tokens: Some(900),
                completion_tokens: Some(100),
                total_tokens: Some(1_000),
                duration_ms: Some(5),
                cost_usd: None,
                measured: true,
                unknown_reason: None,
            })
            .unwrap();
        let (expected, basis) = expected_run_cost(&store, task.id, "role-a", &policy).unwrap();
        assert_eq!((expected, basis), (1_000, ExpectationBasis::TaskRole));

        // The declared task budget (50M in the factory policy) is far above
        // 1M: nothing is refused.
        assert_eq!(
            check_budget(&store, task.id, "role-a", &policy, Utc::now()).unwrap(),
            None
        );

        // A task budget of 1.5k cannot pay for a 1k run after 1k is already
        // spent: refused on the TASK level, with the numbers, before any row.
        let tight = UsagePolicy {
            budgets: TokenBudgets {
                run: Some(10_000),
                task: Some(1_500),
                day: Some(1_000_000),
            },
            ..policy.clone()
        };
        let refusal = check_budget(&store, task.id, "role-a", &tight, Utc::now())
            .unwrap()
            .expect("refused");
        assert_eq!(refusal.level, BudgetLevel::Task);
        assert_eq!(refusal.spent, 1_000);
        assert_eq!(refusal.limit, 1_500);
        assert_eq!(refusal.expected, 1_000);
        assert!(refusal.line.contains("бюджет задачи исчерпан"));

        // The run level refuses a run whose expectation exceeds the run limit.
        let run_tight = UsagePolicy {
            budgets: TokenBudgets {
                run: Some(500),
                task: None,
                day: None,
            },
            ..policy.clone()
        };
        let refusal = check_budget(&store, task.id, "role-a", &run_tight, Utc::now())
            .unwrap()
            .expect("refused");
        assert_eq!(refusal.level, BudgetLevel::Run);

        // The day level refuses new runs when today's spend cannot cover one
        // more run, and only names the day.
        let day_tight = UsagePolicy {
            budgets: TokenBudgets {
                run: Some(10_000),
                task: None,
                day: Some(1_200),
            },
            ..policy.clone()
        };
        let refusal = check_budget(&store, task.id, "role-a", &day_tight, Utc::now())
            .unwrap()
            .expect("refused");
        assert_eq!(refusal.level, BudgetLevel::Day);
        assert!(refusal.line.contains("до новых суток"));

        // The honest row of a run that closed without a report is UNKNOWN, and
        // it is never summed into the spend (REQ-USG-02 AC1).
        let bare = devai_core::types::RoleExecution {
            id: devai_core::types::new_id(),
            task_id: task.id,
            role_slug: "role-b".into(),
            role_version: 1,
            agent_name: None,
            status: devai_core::types::ExecutionStatus::Running,
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
            bundle: None,
            run_id: None,
        };
        store.start_execution(&bare).unwrap();
        store
            .finish_execution(
                &bare,
                devai_core::types::ExecutionStatus::Failed,
                None,
                Some("boom"),
                Utc::now(),
            )
            .unwrap();
        let row = store.execution_usage(bare.id).unwrap().expect("row exists");
        assert!(!row.measured);
        assert_eq!(row.total_tokens, None);
        assert_eq!(row.cost_usd, None);
        assert_eq!(task_spent(&store, task.id).unwrap(), 1_000);
    }
}
