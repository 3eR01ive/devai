//! TOKEN SPEND of a run (task 01a1b851, REQ-USG-01/02/03) — the stored rows
//! and their aggregation.
//!
//! Two facts live here and nothing else:
//!
//! * ONE spend row per execution (`execution_usage`, schema v16), written by
//!   the PLATFORM at completion. The row is either MEASURED (numbers from the
//!   driver's usage report) or UNKNOWN (`measured = 0`, tokens/cost NULL and
//!   a stated reason) — there is no third, «zero» state, because a run
//!   without a report must never look free (REQ-USG-02 AC1).
//! * The READ of those rows (`usage_rollup`, `usage_runs`, `usage_average`)
//!   aggregated by task / role / model over a time window. The read counts
//!   executions, not usage rows, as the number of runs: a run whose row is
//!   missing (written before v16, or closed before the recorder could write)
//!   is an UNKNOWN run, so the number of runs and the number of accounted
//!   runs always agree (REQ-USG-02 AC4, REQ-USG-03 AC3).
//!
//! Nothing here reads the evidence tree, parses a driver report or
//! re-derives a number from logs: the store is the record, the runtime is the
//! writer (arch/no-read-time-rederivation, arch/store-is-cache).

use chrono::{DateTime, Utc};
use devai_core::types::{Decision, Id, Timestamp, decision_kind};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::rows;
use crate::{Store, StoreResult};

/// One stored spend row of one execution.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ExecutionUsage {
    pub execution_id: Id,
    pub task_id: Id,
    pub role_slug: String,
    pub run_id: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// `None` = unknown (never zero) — the same for the three token fields.
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub duration_ms: Option<i64>,
    /// `None` = the price of this model is unknown. Not `0.0` as a placeholder.
    pub cost_usd: Option<f64>,
    /// 1 = the numbers came from a real report; 0 = «расход неизвестен».
    pub measured: bool,
    pub unknown_reason: Option<String>,
    pub created_at: Timestamp,
}

/// The writer's input: what the platform observed about ONE execution.
#[derive(Clone, Debug)]
pub struct UsageRecord {
    pub execution_id: Id,
    pub task_id: Id,
    pub role_slug: String,
    pub run_id: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub duration_ms: Option<i64>,
    pub cost_usd: Option<f64>,
    pub measured: bool,
    pub unknown_reason: Option<String>,
}

impl UsageRecord {
    /// The honest «неизвестно» record for a run that produced no usage report
    /// (or whose report could not be read). Tokens and cost stay NULL.
    pub fn unknown(
        execution_id: Id,
        task_id: Id,
        role_slug: &str,
        run_id: Option<String>,
        provider: Option<String>,
        model: Option<String>,
        duration_ms: Option<i64>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            execution_id,
            task_id,
            role_slug: role_slug.to_string(),
            run_id,
            provider,
            model,
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            duration_ms,
            cost_usd: None,
            measured: false,
            unknown_reason: Some(reason.into()),
        }
    }
}

/// The dimension a rollup is grouped by. `None` = one totals bucket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageGroup {
    Task,
    Role,
    Model,
    Totals,
}

/// One aggregation bucket: the sums and the run counts of one key.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct UsageBucket {
    /// The group's key (task id, role slug, model name); `None` for totals.
    pub key: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    /// Sum of the KNOWN costs only. `cost_unknown` says how many measured
    /// runs carry no price, so the sum is never read as the whole truth.
    pub cost_usd: f64,
    pub runs: i64,
    pub measured: i64,
    pub unknown: i64,
    pub cost_unknown: i64,
}

/// What to aggregate: a half-open window `[from, to)`, an optional task
/// filter, and the dimension.
#[derive(Clone, Debug)]
pub struct UsageQuery {
    pub from: Option<Timestamp>,
    pub to: Option<Timestamp>,
    pub task_id: Option<Id>,
    pub group: UsageGroup,
}

impl UsageQuery {
    /// Everything, unfiltered, one totals bucket.
    pub fn all() -> Self {
        Self {
            from: None,
            to: None,
            task_id: None,
            group: UsageGroup::Totals,
        }
    }
}

/// One execution as the card's run table shows it: the run identity, its
/// status and — honestly — whether a spend number exists for it at all.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RunUsage {
    pub execution_id: Id,
    pub role_slug: String,
    pub started_at: Timestamp,
    pub status: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub measured: bool,
    pub unknown_reason: Option<String>,
    pub duration_ms: Option<i64>,
}

impl Store {
    /// Record ONE execution's spend. Idempotent by the primary key: repeating
    /// the same execution REPLACES its row, so no amount is ever counted
    /// twice (REQ-USG-01 AC4). A run whose close already inserted the honest
    /// UNKNOWN row is completed here with the numbers the report carried.
    pub fn record_execution_usage(&self, rec: &UsageRecord) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO execution_usage(execution_id,task_id,role_slug,run_id,provider,model,\
             prompt_tokens,completion_tokens,total_tokens,duration_ms,cost_usd,measured,\
             unknown_reason,created_at) \
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) \
             ON CONFLICT(execution_id) DO UPDATE SET \
             prompt_tokens=excluded.prompt_tokens, completion_tokens=excluded.completion_tokens, \
             total_tokens=excluded.total_tokens, duration_ms=excluded.duration_ms, \
             cost_usd=excluded.cost_usd, measured=excluded.measured, \
             unknown_reason=excluded.unknown_reason, provider=excluded.provider, \
             model=excluded.model, run_id=excluded.run_id",
            params![
                rec.execution_id.to_string(),
                rec.task_id.to_string(),
                rec.role_slug,
                rec.run_id,
                rec.provider,
                rec.model,
                rec.prompt_tokens,
                rec.completion_tokens,
                rec.total_tokens,
                rec.duration_ms,
                rec.cost_usd,
                i64::from(rec.measured),
                rec.unknown_reason,
                rows::ts(&Utc::now()),
            ],
        )?;
        Ok(())
    }

    /// The honest default row of an execution that closed without a spend
    /// report: `INSERT OR IGNORE`, so it never overwrites a measured row and
    /// never doubles one. Called INSIDE the close transaction of every
    /// execution (`finish_execution_classified`), which is what makes «нет
    /// прогона без строки расхода» a storage property instead of a promise
    /// (REQ-USG-01 AC1, REQ-USG-02 AC1).
    pub(crate) fn ensure_usage_row_in(
        tx: &rusqlite::Transaction<'_>,
        e: &devai_core::types::RoleExecution,
        now: Timestamp,
    ) -> StoreResult<()> {
        tx.execute(
            "INSERT OR IGNORE INTO execution_usage(execution_id,task_id,role_slug,run_id,\
             provider,model,prompt_tokens,completion_tokens,total_tokens,duration_ms,cost_usd,\
             measured,unknown_reason,created_at) \
             VALUES(?1,?2,?3,?4,?5,?6,NULL,NULL,NULL,NULL,NULL,0,?7,?8)",
            params![
                e.id.to_string(),
                e.task_id.to_string(),
                e.role_slug,
                e.run_id.map(|r| r.to_string()),
                e.bundle.as_ref().map(|b| b.llm_provider.clone()),
                e.bundle.as_ref().map(|b| b.model.clone()),
                "the run closed without a usage report — spend unknown, not zero",
                rows::ts(&now),
            ],
        )?;
        Ok(())
    }

    /// The spend row of one execution, if any was recorded.
    pub fn execution_usage(&self, execution_id: Id) -> StoreResult<Option<ExecutionUsage>> {
        let row = self
            .conn
            .query_row(
                "SELECT execution_id,task_id,role_slug,run_id,provider,model,prompt_tokens,\
                 completion_tokens,total_tokens,duration_ms,cost_usd,measured,unknown_reason,\
                 created_at FROM execution_usage WHERE execution_id=?1",
                params![execution_id.to_string()],
                usage_from_row,
            )
            .optional()?;
        Ok(row)
    }

    /// Aggregate spend over a window. Every EXECUTION in the window counts as
    /// a run — a run without a usage row (or with `measured = 0`) is counted
    /// as unknown, never dropped and never zeroed (REQ-USG-02 AC3/AC4).
    pub fn usage_rollup(&self, q: &UsageQuery) -> StoreResult<Vec<UsageBucket>> {
        let key = match q.group {
            UsageGroup::Totals => "''".to_string(),
            UsageGroup::Task => "e.task_id".to_string(),
            UsageGroup::Role => "e.role_slug".to_string(),
            // The model stamp of the execution is the run's own identity
            // (REQ-MS-05). A run written before the stamp has no model — it
            // is grouped under the EMPTY key and the reader words it, rather
            // than being folded into another model's row.
            UsageGroup::Model => "COALESCE(u.model, e.model, '')".to_string(),
        };
        let sql = format!(
            "SELECT {key} AS k, \
             COALESCE(SUM(CASE WHEN u.measured=1 THEN u.prompt_tokens ELSE 0 END),0), \
             COALESCE(SUM(CASE WHEN u.measured=1 THEN u.completion_tokens ELSE 0 END),0), \
             COALESCE(SUM(CASE WHEN u.measured=1 THEN u.total_tokens ELSE 0 END),0), \
             COALESCE(SUM(CASE WHEN u.measured=1 THEN u.cost_usd END),0.0), \
             COUNT(*), \
             COALESCE(SUM(CASE WHEN u.measured=1 THEN 1 ELSE 0 END),0), \
             COALESCE(SUM(CASE WHEN u.measured=1 THEN 0 ELSE 1 END),0), \
             COALESCE(SUM(CASE WHEN u.measured=1 AND u.cost_usd IS NULL THEN 1 ELSE 0 END),0) \
             FROM role_executions e LEFT JOIN execution_usage u ON u.execution_id=e.id \
             WHERE (?1 IS NULL OR e.started_at >= ?1) \
               AND (?2 IS NULL OR e.started_at < ?2) \
               AND (?3 IS NULL OR e.task_id = ?3) \
             GROUP BY k"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows_out = stmt.query(params![
            q.from.as_ref().map(rows::ts),
            q.to.as_ref().map(rows::ts),
            q.task_id.map(|t| t.to_string()),
        ])?;
        let mut out = Vec::new();
        while let Some(r) = rows_out.next()? {
            let raw: String = rows::get(r, 0)?;
            out.push(UsageBucket {
                key: match q.group {
                    UsageGroup::Totals => None,
                    _ => Some(raw),
                },
                prompt_tokens: rows::get(r, 1)?,
                completion_tokens: rows::get(r, 2)?,
                total_tokens: rows::get(r, 3)?,
                cost_usd: rows::get(r, 4)?,
                runs: rows::get(r, 5)?,
                measured: rows::get(r, 6)?,
                unknown: rows::get(r, 7)?,
                cost_unknown: rows::get(r, 8)?,
            });
        }
        Ok(out)
    }

    /// The runs of one task with their spend, in start order — the card's
    /// run table. A run without a row is present with `measured = false`.
    pub fn usage_runs(&self, task_id: Id) -> StoreResult<Vec<RunUsage>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.id, e.role_slug, e.started_at, e.status, \
             COALESCE(u.provider, e.llm_provider), COALESCE(u.model, e.model), \
             u.prompt_tokens, u.completion_tokens, u.total_tokens, u.cost_usd, \
             COALESCE(u.measured,0), u.unknown_reason, u.duration_ms \
             FROM role_executions e LEFT JOIN execution_usage u ON u.execution_id=e.id \
             WHERE e.task_id=?1 ORDER BY e.started_at, e.id",
        )?;
        let mut rows_in = stmt.query(params![task_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(r) = rows_in.next()? {
            out.push(RunUsage {
                execution_id: rows::parse_id(rows::get(r, 0)?)?,
                role_slug: rows::get(r, 1)?,
                started_at: rows::parse_ts(rows::get(r, 2)?)?,
                status: rows::get(r, 3)?,
                provider: rows::get(r, 4)?,
                model: rows::get(r, 5)?,
                prompt_tokens: rows::get(r, 6)?,
                completion_tokens: rows::get(r, 7)?,
                total_tokens: rows::get(r, 8)?,
                cost_usd: rows::get(r, 9)?,
                measured: rows::get::<i64>(r, 10)? == 1,
                unknown_reason: rows::get(r, 11)?,
                duration_ms: rows::get(r, 12)?,
            });
        }
        Ok(out)
    }

    /// The historical average spend of completed MEASURED runs of one
    /// (task, role) pair, else of the role alone (ratified OQ-8 answer), with
    /// the number of runs the average rests on. `None` = no history at all —
    /// the caller falls back to the run budget (REQ-USG-05 AC2).
    pub fn usage_average(
        &self,
        task_id: Option<Id>,
        role: &str,
    ) -> StoreResult<Option<(i64, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT CAST(ROUND(AVG(u.total_tokens)) AS INTEGER), COUNT(*) \
             FROM execution_usage u JOIN role_executions e ON e.id=u.execution_id \
             WHERE u.measured=1 AND u.total_tokens IS NOT NULL AND e.role_slug=?1 \
               AND (?2 IS NULL OR e.task_id=?2)",
        )?;
        let row = stmt
            .query_row(params![role, task_id.map(|t| t.to_string())], |r| {
                Ok((r.get::<usize, Option<i64>>(0)?, r.get::<usize, i64>(1)?))
            })
            .optional()?;
        Ok(match row {
            Some((Some(avg), n)) if n > 0 => Some((avg, n)),
            _ => None,
        })
    }

    /// The number of executions of one task — the denominator the card and
    /// the refusal sentence use, so «число прогонов» never means «число
    /// строк расхода» (REQ-USG-02 AC4).
    pub fn execution_count(&self, task_id: Id) -> StoreResult<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM role_executions WHERE task_id=?1",
            params![task_id.to_string()],
            |r| r.get(0),
        )?)
    }

    /// The ABSOLUTE task budget a human last raised this task to, from the
    /// stored decision rows (REQ-USG-06 AC3/AC4). `None` = the config value
    /// stands: the platform never raises a budget by itself.
    pub fn usage_budget_raise(&self, task_id: Id) -> StoreResult<Option<(u64, Decision)>> {
        let decisions = self.list_decisions(task_id)?;
        Ok(decisions
            .into_iter()
            .filter(|d| {
                d.kind == decision_kind::USAGE_BUDGET
                    && d.verdict == devai_core::types::Verdict::Approved
            })
            .filter_map(|d| {
                let amount = d
                    .payload
                    .get(devai_core::types::json_field::TOKEN_BUDGET)
                    .and_then(|v| v.as_u64());
                amount.map(|n| (n, d))
            })
            .max_by_key(|(_, d)| d.created_at))
    }

    /// The runs of one task that FAILED while the provider said «квота
    /// исчерпана» — the 429 fact, stored as a distinct event, never folded
    /// into «потрачено ноль» (REQ-USG-02 AC2). The reader is `events`.
    pub fn has_event(&self, task_id: Id, kind: &str) -> StoreResult<bool> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM workflow_events WHERE task_id=?1 AND kind=?2",
            params![task_id.to_string(), kind],
            |r| r.get::<usize, i64>(0),
        )? > 0)
    }
}

fn usage_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ExecutionUsage> {
    rows::r2sql((|| {
        Ok(ExecutionUsage {
            execution_id: rows::parse_id(rows::get(r, 0)?)?,
            task_id: rows::parse_id(rows::get(r, 1)?)?,
            role_slug: rows::get(r, 2)?,
            run_id: rows::get(r, 3)?,
            provider: rows::get(r, 4)?,
            model: rows::get(r, 5)?,
            prompt_tokens: rows::get(r, 6)?,
            completion_tokens: rows::get(r, 7)?,
            total_tokens: rows::get(r, 8)?,
            duration_ms: rows::get(r, 9)?,
            cost_usd: rows::get(r, 10)?,
            measured: rows::get::<i64>(r, 11)? == 1,
            unknown_reason: rows::get(r, 12)?,
            created_at: rows::parse_ts(rows::get(r, 13)?)?,
        })
    })())
}

/// The LOCAL calendar boundaries of the read periods (ratified OQ-4 answer:
/// calendar days in the operator's zone, reset at 00:00). Returned as UTC
/// instants, because the store's timestamps are UTC.
pub fn calendar_window(period: &str, now: DateTime<Utc>) -> Option<(Timestamp, Timestamp)> {
    use chrono::{Datelike, Local, TimeZone};
    let local = now.with_timezone(&Local);
    let midnight = Local
        .with_ymd_and_hms(local.year(), local.month(), local.day(), 0, 0, 0)
        .single()?;
    let day_start = midnight.with_timezone(&Utc);
    let (from, to) = match period {
        "day" => (day_start, now),
        "week" => {
            let back = local.weekday().num_days_from_monday() as i64;
            (day_start - chrono::Duration::days(back), now)
        }
        "month" => (
            Local
                .with_ymd_and_hms(local.year(), local.month(), 1, 0, 0, 0)
                .single()?
                .with_timezone(&Utc),
            now,
        ),
        _ => return None,
    };
    Some((from, to))
}
