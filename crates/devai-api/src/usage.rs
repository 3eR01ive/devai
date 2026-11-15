//! TOKEN SPEND as the operator reads it (task 01a1b851, REQ-USG-03/07/08/09).
//!
//! Every view here is a READ of stored rows: the spend facts live in
//! `execution_usage` (written by the runtime at completion) and the runs are
//! counted from `role_executions`, so the number of runs and the number of
//! accounted runs always agree and a run without a report is an UNKNOWN run —
//! never a free one (REQ-USG-02 AC1/AC3/AC4).
//!
//! The ONE source of the numbers is [`devai_store::usage`]: the task card, the
//! summary operation, the overview screen and `devai usage` all call these
//! functions, so no surface re-adds a column of its own (REQ-USG-03 AC4,
//! REQ-USG-09 AC3).
//!
//! Limits are the DECLARED config budgets (REQ-USG-04) plus a human's stored
//! raise; «без лимита» is spelled out, never printed as 0.

use chrono::{Local, Utc};
use devai_core::types::Task;
use devai_store::config::UsagePolicy;
use devai_store::{Store, UsageGroup, UsageQuery};
use serde::{Deserialize, Serialize};

use crate::{Api, ApiResult};

/// The declared budgets of the home, as a read reports them. `None` is «без
/// лимита» — the surfaces spell that out (REQ-USG-04 AC3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetLimits {
    pub run: Option<u64>,
    pub task: Option<u64>,
    pub day: Option<u64>,
    /// The absolute task budget a human last raised a TASK to; `None` when the
    /// declaration stands. Filled for a task card, `None` for the home-wide
    /// reads.
    pub raised_to: Option<u64>,
}

impl BudgetLimits {
    fn from_policy(policy: &UsagePolicy) -> Self {
        Self {
            run: policy.budgets.run,
            task: policy.budgets.task,
            day: policy.budgets.day,
            raised_to: None,
        }
    }
}

/// Token display thresholds of [`token_text`]: presentation only, the exact
/// number always travels beside it in the view fields.
const BILLION: f64 = 1e9;
const MILLION: f64 = 1e6;
const THOUSAND: f64 = 1e3;
/// How many «топ-задачи по расходу» the overview screen lists (REQ-USG-08
/// AC2): a page-sized list, not the whole history.
const TOP_TASKS: usize = 10;
/// Seconds in an hour — the pace unit of the «на сколько хватит» estimate.
const SECONDS_PER_HOUR: f64 = 3600.0;

/// Short human form of a token count («9.9 млн», «241 тыс») — presentation
/// only; the exact number always travels beside it in the view fields.
pub fn token_text(n: i64) -> String {
    let v = n as f64;
    if v.abs() >= BILLION {
        format!("{:.2} млрд", v / BILLION)
    } else if v.abs() >= MILLION {
        format!("{:.1} млн", v / MILLION)
    } else if v.abs() >= THOUSAND {
        format!("{:.0} тыс", v / THOUSAND)
    } else {
        n.to_string()
    }
}

/// The LOCAL calendar boundaries of a read period (ratified OQ-4: calendar
/// days in the operator's zone, reset at 00:00), as UTC instants because the
/// stored timestamps are UTC.
pub fn period_bounds(
    period: &str,
    now: chrono::DateTime<Utc>,
) -> (Option<chrono::DateTime<Utc>>, Option<chrono::DateTime<Utc>>) {
    if period == "all" {
        return (None, Some(now));
    }
    match devai_store::calendar_window(period, now) {
        Some((from, to)) => (Some(from), Some(to)),
        None => (None, Some(now)),
    }
}

/// Midnight of today and the moment now, in the operator's local zone.
fn day_bounds(now: chrono::DateTime<Utc>) -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
    match devai_store::calendar_window("day", now) {
        Some(bounds) => bounds,
        None => (now, now),
    }
}

/// One run of a task with its spend — the card's run table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunUsageView {
    pub execution: String,
    pub role: String,
    pub started_at: String,
    pub status: String,
    pub model: Option<String>,
    /// `None` = unknown (the run reported no usage) — never 0.
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<i64>,
    pub measured: bool,
    /// Why the spend is unknown, when it is.
    pub unknown_reason: Option<String>,
}

/// The task card's spend block (REQ-USG-07): «израсходовано X из Y» with the
/// model and the number of runs, the number of runs with an UNKNOWN spend,
/// and the ONE sentence the card prints. With no limit the text is «без
/// лимита» — never «0 из 0».
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskUsageView {
    pub task: String,
    pub task_name: String,
    /// Sum of the KNOWN token counts of the task's runs.
    pub spent: i64,
    /// The effective task budget; `None` = «без лимита».
    pub limit: Option<u64>,
    /// The budget's words: the number or «без лимита».
    pub limit_text: String,
    /// The absolute budget a human last raised the task to (a stored decision),
    /// if any — the card names it so a raise is visible.
    pub raised_to: Option<u64>,
    /// The ONE sentence: «израсходовано X из Y» / «израсходовано X — без лимита».
    pub line: String,
    /// Distinct models the task's runs used, in start order. Empty = unknown.
    pub models: Vec<String>,
    pub cost_usd: f64,
    /// Runs whose spend carries no price (measured, but the price is unknown).
    pub cost_unknown: i64,
    pub runs: i64,
    pub measured: i64,
    pub unknown: i64,
    pub rows: Vec<RunUsageView>,
}

/// One aggregation row of `usage.summary` (REQ-USG-03).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageBucketView {
    /// The group's key: the task id, the role slug or the model name. Empty
    /// for the totals row.
    pub key: String,
    /// A human name for the key where one exists (the task's name); `None`
    /// for roles, models and the totals row.
    pub name: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: f64,
    pub runs: i64,
    pub measured: i64,
    pub unknown: i64,
    pub cost_unknown: i64,
}

/// `usage.summary` — the ONE read operation of the spend (REQ-USG-03): sums
/// and run counts by task / role / model over a day / week / month window.
/// Read-only by construction: it touches no state and writes no row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummaryView {
    /// `day` | `week` | `month` | `all`.
    pub period: String,
    pub from: Option<String>,
    pub to: Option<String>,
    /// `task` | `role` | `model` | `totals`.
    pub group_by: String,
    pub rows: Vec<UsageBucketView>,
    /// The same window as ONE totals row, so a consumer never re-adds the
    /// rows to get a sum.
    pub totals: UsageBucketView,
    /// The declared budgets of the home, for the reader's context.
    pub limits: BudgetLimits,
}

/// The «на сколько хватит» forecast (REQ-USG-08 AC3/AC4): an ESTIMATE from
/// the current pace, shown only when the period holds enough runs with a
/// known spend. `available = false` carries the honest reason instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageForecastView {
    pub available: bool,
    /// The ONE sentence: the estimate with its marker, or «данных мало» with
    /// exactly what is missing.
    pub line: String,
    pub runs_used: i64,
    pub needed_runs: u32,
}

/// The overall spend screen's single read (REQ-USG-08): today, this month,
/// the top tasks by spend, the forecast and the day-gate note when the day
/// budget stands exhausted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageOverviewView {
    pub day: UsageSummaryView,
    pub month: UsageSummaryView,
    pub top_tasks: Vec<UsageBucketView>,
    pub forecast: UsageForecastView,
    /// The day budget's gate: `Some(sentence with the numbers)` when today
    /// cannot pay for one more run — ONE line for the operator, never a
    /// per-task question (ratified OQ-1).
    pub day_gate: Option<String>,
    pub limits: BudgetLimits,
}

fn bucket_view(b: &devai_store::UsageBucket, name: Option<String>) -> UsageBucketView {
    UsageBucketView {
        key: b.key.clone().unwrap_or_default(),
        name,
        prompt_tokens: b.prompt_tokens,
        completion_tokens: b.completion_tokens,
        total_tokens: b.total_tokens,
        cost_usd: b.cost_usd,
        runs: b.runs,
        measured: b.measured,
        unknown: b.unknown,
        cost_unknown: b.cost_unknown,
    }
}

fn group_of(name: &str) -> ApiResult<(UsageGroup, &'static str)> {
    match name {
        "task" => Ok((UsageGroup::Task, "task")),
        "role" => Ok((UsageGroup::Role, "role")),
        "model" => Ok((UsageGroup::Model, "model")),
        "totals" => Ok((UsageGroup::Totals, "totals")),
        _ => Err(crate::ApiError::refused(
            "bad_request",
            format!("unknown usage grouping `{name}` — one of: task, role, model, totals"),
        )),
    }
}

/// The periods the read accepts. One list for the check and the refusal text.
const PERIODS: &[&str] = &["day", "week", "month", "all"];

/// Reject an unknown period instead of silently reading «all time» — a typo
/// must never widen the window the operator asked about.
fn parse_period(period: &str) -> ApiResult<()> {
    if PERIODS.contains(&period) {
        return Ok(());
    }
    Err(crate::ApiError::refused(
        "bad_request",
        format!(
            "unknown usage period `{period}` — one of: {}",
            PERIODS.join(", ")
        ),
    ))
}

/// The effective task budgets, the ONE place the card and the admission check
/// read them from: the declared config value raised ONLY by a stored human
/// decision (REQ-USG-06 AC3).
pub(crate) fn task_limits(
    store: &Store,
    task: &Task,
    policy: &UsagePolicy,
) -> ApiResult<BudgetLimits> {
    let raised_to = store.usage_budget_raise(task.id)?.map(|(amount, _)| amount);
    let limit = devai_runtime::usage::effective_task_budget(store, task.id, policy)?;
    Ok(BudgetLimits {
        run: policy.budgets.run,
        task: limit,
        day: policy.budgets.day,
        raised_to,
    })
}

impl Api {
    /// The token-control policy in force for this home (REQ-USG-04): the same
    /// loader `devai init` validates and the engine enforces. A broken
    /// declaration is a LOUD read error here — never a default that would
    /// print limits the operator did not declare.
    pub(crate) fn usage_policy(&self) -> ApiResult<UsagePolicy> {
        Ok(devai_store::config::load_usage_policy(
            &self.config_dir().join("workflow"),
        )?)
    }

    /// [`Api::task_usage`] — see [`TaskUsageView`].
    pub fn task_usage(&self, id_prefix: &str) -> ApiResult<TaskUsageView> {
        let policy = self.usage_policy()?;
        let task = self.store.resolve_task(id_prefix)?;
        let limits = task_limits(&self.store, &task, &policy)?;
        let runs = self.store.usage_runs(task.id)?;
        // The numbers come from the SAME rollup the summary operation and the
        // screen read (REQ-USG-03 AC4, REQ-USG-09 AC3): the card performs no
        // arithmetic of its own, so its «израсходовано X» cannot disagree with
        // `usage.summary` or `devai usage`.
        let totals = self.task_totals(task.id)?;
        Ok(TaskUsageView {
            task: task.id.to_string(),
            task_name: task.display_sign(),
            spent: totals.total_tokens,
            limit: limits.task,
            limit_text: UsagePolicy::budget_text(limits.task),
            raised_to: limits.raised_to,
            line: spent_line(totals.total_tokens, limits.task),
            models: run_models(&runs),
            cost_usd: totals.cost_usd,
            cost_unknown: totals.cost_unknown,
            runs: totals.runs,
            measured: totals.measured,
            unknown: totals.unknown,
            rows: runs.into_iter().map(run_row).collect(),
        })
    }

    /// The totals bucket of ONE task over its whole life — the card's numbers,
    /// read from the same rollup every other surface uses.
    fn task_totals(&self, task_id: devai_core::types::Id) -> ApiResult<devai_store::UsageBucket> {
        let buckets = self.store.usage_rollup(&UsageQuery {
            from: None,
            to: None,
            task_id: Some(task_id),
            group: UsageGroup::Totals,
        })?;
        Ok(buckets.into_iter().next().unwrap_or_default())
    }

    /// `usage.summary` (REQ-USG-03): the spend by task / role / model over a
    /// period, plus the same window as one totals row. Read-only.
    pub fn usage_summary(
        &self,
        period: &str,
        group_by: &str,
        task_ref: Option<&str>,
    ) -> ApiResult<UsageSummaryView> {
        let policy = self.usage_policy()?;
        let (group, group_name) = group_of(group_by)?;
        parse_period(period)?;
        let task_id = match task_ref {
            Some(r) => Some(self.store.resolve_task(r)?.id),
            None => None,
        };
        let (from, to) = period_bounds(period, Utc::now());
        let q = UsageQuery {
            from,
            to,
            task_id,
            group,
        };
        let rows = self.store.usage_rollup(&q)?;
        let totals = self
            .store
            .usage_rollup(&UsageQuery {
                group: UsageGroup::Totals,
                ..q.clone()
            })?
            .into_iter()
            .next()
            .unwrap_or_default();
        Ok(UsageSummaryView {
            period: period.to_string(),
            from: from.map(|t| t.to_rfc3339()),
            to: to.map(|t| t.to_rfc3339()),
            group_by: group_name.to_string(),
            rows: self.bucket_views(group, rows)?,
            totals: bucket_view(&totals, None),
            limits: BudgetLimits::from_policy(&policy),
        })
    }

    /// The view rows of one rollup, biggest spend first. Task buckets are
    /// named in ONE batch read; an unnamed bucket keeps its key, never a blank.
    fn bucket_views(
        &self,
        group: UsageGroup,
        rows: Vec<devai_store::UsageBucket>,
    ) -> ApiResult<Vec<UsageBucketView>> {
        let names = self.bucket_names(group, &rows)?;
        let mut out: Vec<UsageBucketView> = rows
            .iter()
            .map(|b| bucket_view(b, names.get(b.key.as_deref().unwrap_or_default()).cloned()))
            .collect();
        out.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens).then(a.key.cmp(&b.key)));
        Ok(out)
    }

    /// The display names of the task buckets — one query for the whole page
    /// (pf/io-churn), and a store failure is an error, never a nameless row
    /// standing in for an unreadable one.
    fn bucket_names(
        &self,
        group: UsageGroup,
        rows: &[devai_store::UsageBucket],
    ) -> ApiResult<std::collections::HashMap<String, String>> {
        if group != UsageGroup::Task {
            return Ok(std::collections::HashMap::new());
        }
        let keys: Vec<String> = rows.iter().filter_map(|b| b.key.clone()).collect();
        Ok(self.store.task_display_names(&keys)?.into_iter().collect())
    }

    /// `usage.overview` (REQ-USG-08): ONE read for the whole spend screen —
    /// today, this month, the top tasks, the forecast and the day gate.
    pub fn usage_overview(&self) -> ApiResult<UsageOverviewView> {
        let policy = self.usage_policy()?;
        let now = Utc::now();
        let day = self.usage_summary("day", "totals", None)?;
        let month = self.usage_summary("month", "totals", None)?;
        let top = self.usage_summary("month", "task", None)?;
        let limits = BudgetLimits::from_policy(&policy);
        let forecast = self.usage_forecast(&policy, &day.totals, now);
        Ok(UsageOverviewView {
            top_tasks: top.rows.into_iter().take(TOP_TASKS).collect(),
            day_gate: day_gate(&limits, day.totals.total_tokens),
            day,
            month,
            forecast,
            limits,
        })
    }

    /// The forecast of the DAY budget (REQ-USG-08 AC3/AC4): available only
    /// with a day limit and at least `forecast_min_runs` measured runs today.
    fn usage_forecast(
        &self,
        policy: &UsagePolicy,
        day_totals: &UsageBucketView,
        now: chrono::DateTime<Utc>,
    ) -> UsageForecastView {
        let (limit, elapsed) = match forecast_inputs(policy, day_totals, now) {
            ForecastInputs::Ready { limit, elapsed } => (limit, elapsed),
            // Not enough to estimate from: the view carries WHY, never a
            // horizon built on nothing.
            ForecastInputs::NotEnough(told) => return told,
        };
        UsageForecastView {
            available: true,
            line: forecast_line(now, limit, day_totals.total_tokens as f64, elapsed),
            runs_used: day_totals.measured,
            needed_runs: policy.forecast_min_runs,
        }
    }
}

/// What a forecast read found: enough to estimate from, or the view that says
/// honestly why there is no estimate.
enum ForecastInputs {
    Ready { limit: u64, elapsed: f64 },
    NotEnough(UsageForecastView),
}

/// The two inputs the estimate needs — the day limit and how much of the day
/// has elapsed — or the honest «данных мало» view.
fn forecast_inputs(
    policy: &UsagePolicy,
    day_totals: &UsageBucketView,
    now: chrono::DateTime<Utc>,
) -> ForecastInputs {
    let needed = policy.forecast_min_runs;
    let Some(limit) = policy.budgets.day else {
        return ForecastInputs::NotEnough(forecast_unavailable(
            "прогноз не строится: суточный лимит не задан (без лимита нечего исчерпывать)",
            day_totals,
            needed,
        ));
    };
    if day_totals.measured < needed as i64 {
        return ForecastInputs::NotEnough(forecast_unavailable(
            &format!(
                "данных мало: за сутки {} прогонов с известным расходом, для прогноза нужно не \
                 меньше {needed}",
                day_totals.measured
            ),
            day_totals,
            needed,
        ));
    }
    if day_totals.total_tokens <= 0 {
        return ForecastInputs::NotEnough(forecast_unavailable(
            "данных мало: известный расход за сутки нулевой — темп считать не из чего",
            day_totals,
            needed,
        ));
    }
    let (start, _) = day_bounds(now);
    ForecastInputs::Ready {
        limit,
        // `max(1)` keeps a same-instant read from dividing by zero: one
        // second is the smallest pace the estimate admits, never an infinity.
        elapsed: (now - start).num_seconds().max(1) as f64,
    }
}

/// The ONE sentence of the day budget's gate (ratified OQ-1: an exhausted DAY
/// budget stops NEW runs until tomorrow and is shown as ONE line with the
/// numbers — the tasks themselves are not parked one by one).
fn day_gate(limits: &BudgetLimits, spent: i64) -> Option<String> {
    let limit = limits.day?;
    if spent < limit as i64 {
        return None;
    }
    Some(format!(
        "суточный бюджет исчерпан: за сутки израсходовано {spent} из {limit} — новые прогоны не \
         запускаются до новых суток (уже начатые досчитают)"
    ))
}

/// The card's ONE sentence: «израсходовано X из Y» or «израсходовано X — без
/// лимита», never «0 из 0» (REQ-USG-04 AC3, REQ-USG-07 AC3).
fn spent_line(spent: i64, limit: Option<u64>) -> String {
    match limit {
        Some(limit) => format!("израсходовано {spent} из {limit}"),
        None => format!("израсходовано {spent} — без лимита"),
    }
}

/// The distinct models the task's runs used, in start order. An empty list
/// means «модель неизвестна» — the card words it, it is not a blank.
fn run_models(runs: &[devai_store::RunUsage]) -> Vec<String> {
    let mut models: Vec<String> = Vec::new();
    for model in runs.iter().filter_map(|r| r.model.as_ref()) {
        if !model.trim().is_empty() && !models.contains(model) {
            models.push(model.clone());
        }
    }
    models
}

/// One run row of the card. `None` numbers stay `None` — unknown, not zero.
fn run_row(r: devai_store::RunUsage) -> RunUsageView {
    RunUsageView {
        execution: r.execution_id.to_string(),
        role: r.role_slug,
        started_at: r.started_at.to_rfc3339(),
        status: r.status,
        model: r.model,
        prompt_tokens: r.prompt_tokens,
        completion_tokens: r.completion_tokens,
        total_tokens: r.total_tokens,
        cost_usd: r.cost_usd,
        duration_ms: r.duration_ms,
        measured: r.measured,
        unknown_reason: r.unknown_reason,
    }
}

/// A forecast that is NOT shown: the honest reason, the runs it had and the
/// threshold it lacked — never an invented horizon (REQ-USG-08 AC4).
fn forecast_unavailable(
    line: &str,
    day_totals: &UsageBucketView,
    needed_runs: u32,
) -> UsageForecastView {
    UsageForecastView {
        available: false,
        line: line.to_string(),
        runs_used: day_totals.measured,
        needed_runs,
    }
}

/// The «на сколько хватит» sentence at the current pace — marked as an
/// ESTIMATE on both limbs (room left, or already over the day budget).
fn forecast_line(now: chrono::DateTime<Utc>, limit: u64, spent: f64, elapsed: f64) -> String {
    let rate = spent / elapsed;
    let per_hour = token_text((rate * SECONDS_PER_HOUR) as i64);
    let remaining = (limit as f64 - spent).max(0.0);
    if remaining <= 0.0 {
        return format!(
            "суточный бюджет исчерпан (израсходовано {} из {limit}); при текущем темпе ~{per_hour}\
             /час следующая трата уйдёт в перерасход — оценка",
            token_text(spent as i64),
        );
    }
    let until = now + chrono::Duration::seconds((remaining / rate) as i64);
    let local = until.with_timezone(&Local);
    format!(
        "хватит примерно до {} при текущем темпе ~{per_hour}/час — оценка, не гарантия",
        local.format("%H:%M"),
    )
}

impl Api {
    /// ADMISSION, early half (REQ-USG-05): the same pure check the engine runs
    /// before a stage, asked BEFORE any process is spawned — so an exhausted
    /// budget costs neither a runner nor an execution row. On the TASK level
    /// the task is parked with ONE human question (REQ-USG-06); the refusal
    /// text is exactly the engine's sentence, so both doors say the same
    /// numbers.
    pub(crate) fn usage_gate(&self, task: &Task, role: &str) -> ApiResult<Option<String>> {
        let policy = self.usage_policy()?;
        let Some(refusal) =
            devai_runtime::usage::check_budget(&self.store, task.id, role, &policy, Utc::now())?
        else {
            return Ok(None);
        };
        // The parking rule is the RUNTIME's — one implementation shared with
        // the engine's pre-stage gate, so both doors park the same task with
        // the same numbers (REQ-USG-06).
        let process = self.process_policy()?;
        devai_runtime::usage::stop_task(
            &self.store,
            &process,
            task,
            &refusal,
            policy.budgets.task,
            devai_core::types::PLATFORM_ACTOR,
            Utc::now(),
        )?;
        Ok(Some(refusal.line))
    }
}
