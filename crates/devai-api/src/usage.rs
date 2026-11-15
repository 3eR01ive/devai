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

/// Short human form of a token count («9.9 млн», «241 тыс») — presentation
/// only; the exact number always travels beside it in the view fields.
pub fn token_text(n: i64) -> String {
    let v = n as f64;
    if v.abs() >= 1e9 {
        format!("{:.2} млрд", v / 1e9)
    } else if v.abs() >= 1e6 {
        format!("{:.1} млн", v / 1e6)
    } else if v.abs() >= 1e3 {
        format!("{:.0} тыс", v / 1e3)
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

fn group_of(name: &str) -> Option<(UsageGroup, &'static str)> {
    match name {
        "task" => Some((UsageGroup::Task, "task")),
        "role" => Some((UsageGroup::Role, "role")),
        "model" => Some((UsageGroup::Model, "model")),
        "totals" => Some((UsageGroup::Totals, "totals")),
        _ => None,
    }
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
        let totals = self
            .store
            .usage_rollup(&UsageQuery {
                from: None,
                to: None,
                task_id: Some(task.id),
                group: UsageGroup::Totals,
            })?
            .into_iter()
            .next()
            .unwrap_or_default();
        let runs_total = totals.runs;
        let measured = totals.measured;
        let unknown = totals.unknown;
        let spent = totals.total_tokens;
        let cost_usd = totals.cost_usd;
        let cost_unknown = totals.cost_unknown;
        let mut models: Vec<String> = Vec::new();
        for r in &runs {
            if let Some(m) = r.model.as_ref() {
                if !m.trim().is_empty() && !models.contains(m) {
                    models.push(m.clone());
                }
            }
        }
        let line = match limits.task {
            Some(limit) => format!("израсходовано {spent} из {limit}"),
            None => format!("израсходовано {spent} — без лимита"),
        };
        Ok(TaskUsageView {
            task: task.id.to_string(),
            task_name: task
                .human_name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| task.title.clone()),
            spent,
            limit: limits.task,
            limit_text: UsagePolicy::budget_text(limits.task),
            raised_to: limits.raised_to,
            line,
            models,
            cost_usd,
            cost_unknown,
            runs: runs_total,
            measured,
            unknown,
            rows: runs
                .into_iter()
                .map(|r| RunUsageView {
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
                })
                .collect(),
        })
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
        let Some((group, group_name)) = group_of(group_by) else {
            return Err(crate::ApiError::refused(
                "bad_request",
                format!("unknown usage grouping `{group_by}` — one of: task, role, model, totals"),
            ));
        };
        if !matches!(period, "day" | "week" | "month" | "all") {
            return Err(crate::ApiError::refused(
                "bad_request",
                format!("unknown usage period `{period}` — one of: day, week, month, all"),
            ));
        }
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
        let mut rows = self.store.usage_rollup(&q)?;
        let totals = self
            .store
            .usage_rollup(&UsageQuery {
                group: UsageGroup::Totals,
                ..q.clone()
            })?
            .into_iter()
            .next()
            .unwrap_or_default();
        let mut out = Vec::with_capacity(rows.len());
        for b in rows.drain(..) {
            let name = match group {
                UsageGroup::Task => b
                    .key
                    .as_deref()
                    .and_then(|k| self.store.get_task(k).ok())
                    .map(|t| {
                        t.human_name
                            .clone()
                            .filter(|n| !n.trim().is_empty())
                            .unwrap_or(t.title)
                    }),
                _ => None,
            };
            out.push(bucket_view(&b, name));
        }
        out.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens).then(a.key.cmp(&b.key)));
        Ok(UsageSummaryView {
            period: period.to_string(),
            from: from.map(|t| t.to_rfc3339()),
            to: to.map(|t| t.to_rfc3339()),
            group_by: group_name.to_string(),
            rows: out,
            totals: bucket_view(&totals, None),
            limits: BudgetLimits::from_policy(&policy),
        })
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
        // The ratified OQ-1 answer: an exhausted DAY budget stops NEW runs
        // until tomorrow and is shown as ONE line with the numbers — the
        // tasks themselves are not parked one by one.
        let day_gate = limits.day.and_then(|limit| {
            let spent = day.totals.total_tokens;
            if spent >= limit as i64 {
                Some(format!(
                    "суточный бюджет исчерпан: за сутки израсходовано {spent} из {limit} — новые \
                     прогоны не запускаются до новых суток (уже начатые досчитают)"
                ))
            } else {
                None
            }
        });
        Ok(UsageOverviewView {
            top_tasks: top.rows.into_iter().take(10).collect(),
            day,
            month,
            forecast,
            day_gate,
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
        let needed = policy.forecast_min_runs;
        let Some(limit) = policy.budgets.day else {
            return UsageForecastView {
                available: false,
                line:
                    "прогноз не строится: суточный лимит не задан (без лимита нечего исчерпывать)"
                        .to_string(),
                runs_used: day_totals.measured,
                needed_runs: needed,
            };
        };
        if day_totals.measured < needed as i64 {
            return UsageForecastView {
                available: false,
                line: format!(
                    "данных мало: за сутки {} прогонов с известным расходом, для прогноза нужно не \
                     меньше {needed}",
                    day_totals.measured
                ),
                runs_used: day_totals.measured,
                needed_runs: needed,
            };
        }
        let (start, _) = day_bounds(now);
        let elapsed = (now - start).num_seconds().max(1) as f64;
        let spent = day_totals.total_tokens as f64;
        if spent <= 0.0 {
            return UsageForecastView {
                available: false,
                line: "данных мало: известный расход за сутки нулевой — темп считать не из чего"
                    .to_string(),
                runs_used: day_totals.measured,
                needed_runs: needed,
            };
        }
        let rate = spent / elapsed;
        let remaining = (limit as f64 - spent).max(0.0);
        let until = now + chrono::Duration::seconds((remaining / rate) as i64);
        let local = until.with_timezone(&Local);
        let line = if remaining <= 0.0 {
            format!(
                "суточный бюджет исчерпан (израсходовано {} из {limit}); при текущем темпе \
                 ~{}/час следующая трата уйдёт в перерасход — оценка",
                token_text(day_totals.total_tokens),
                token_text((rate * 3600.0) as i64),
            )
        } else {
            format!(
                "хватит примерно до {} при текущем темпе ~{}/час — оценка, не гарантия",
                local.format("%H:%M"),
                token_text((rate * 3600.0) as i64),
            )
        };
        UsageForecastView {
            available: true,
            line,
            runs_used: day_totals.measured,
            needed_runs: needed,
        }
    }
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
        let mut parked = false;
        if refusal.level == devai_runtime::usage::BudgetLevel::Task {
            let process = self.process_policy()?;
            // The parking rule is the RUNTIME's — one implementation shared
            // with the engine's pre-stage gate, so both doors park the same
            // task with the same numbers (REQ-USG-06).
            parked = devai_runtime::usage::park_task_exhaustion(
                &self.store,
                &process,
                task,
                role,
                &refusal,
                policy.budgets.task,
                devai_core::types::PLATFORM_ACTOR,
                Utc::now(),
            )?;
        }
        if !parked {
            devai_runtime::usage::record_refusal(
                &self.store,
                task.id,
                role,
                &refusal,
                devai_core::types::PLATFORM_ACTOR,
            )?;
        }
        Ok(Some(refusal.line))
    }
}
