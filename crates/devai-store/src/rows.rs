//! Row <-> domain conversions.

use chrono::{DateTime, SecondsFormat, Utc};
use devai_core::types::*;
use rusqlite::Row;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::error::{StoreError, StoreResult};

pub fn ts(t: &Timestamp) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, true)
}

pub fn parse_ts(s: String) -> StoreResult<Timestamp> {
    DateTime::parse_from_rfc3339(&s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| StoreError::Corrupt(format!("bad timestamp: {e}")))
}

fn parse_id(s: String) -> StoreResult<Id> {
    Uuid::parse_str(&s).map_err(|e| StoreError::Corrupt(format!("bad uuid `{s}`: {e}")))
}

fn opt_id(s: Option<String>) -> StoreResult<Option<Id>> {
    s.map(|v| parse_id(v)).transpose()
}

fn opt_ts(s: Option<String>) -> StoreResult<Option<Timestamp>> {
    s.map(parse_ts).transpose()
}

fn get<'a, T: rusqlite::types::FromSql>(r: &'a Row, i: usize) -> StoreResult<T> {
    r.get::<usize, T>(i).map_err(|e| match e {
        rusqlite::Error::InvalidColumnType(..) | rusqlite::Error::InvalidQuery => StoreError::Corrupt(format!("column {i}: {e}")),
        other => StoreError::Sql(other),
    })
}

/// Adapts a `StoreResult` row conversion for rusqlite closures.
pub fn r2sql<T>(v: StoreResult<T>) -> rusqlite::Result<T> {
    v.map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

/// Unwraps a query_row(..).optional() whose closure may embed a StoreError.
pub fn unwrap_row<T>(res: Result<Option<T>, rusqlite::Error>) -> StoreResult<Option<T>> {
    match res {
        Ok(o) => Ok(o),
        Err(rusqlite::Error::ToSqlConversionFailure(b)) => match b.downcast::<StoreError>() {
            Ok(e) => Err(*e),
            Err(other) => Err(StoreError::Corrupt(other.to_string())),
        },
        Err(other) => Err(StoreError::Sql(other)),
    }
}

fn arr<T: DeserializeOwned>(s: Option<String>) -> Vec<T> {
    json_array(s.as_deref())
}

pub fn enum_col<T: Copy>(ty: &'static str, s: String, f: impl Fn(&str) -> Result<T, devai_core::error::CoreError>) -> StoreResult<T> {
    f(&s).map_err(|_| StoreError::Corrupt(format!("bad {ty} variant `{s}`")))
}

fn opt_enum<T: Copy>(s: Option<String>, ty: &'static str, f: impl Fn(&str) -> Result<T, devai_core::error::CoreError>) -> StoreResult<Option<T>> {
    s.map(|v| enum_col(ty, v, &f)).transpose()
}

pub fn task_from_row(r: &Row) -> StoreResult<Task> {
    Ok(Task {
        id: parse_id(get(r, 0)?)?,
        project_id: opt_id(get(r, 1)?)?,
        title: get(r, 2)?,
        business_description: get(r, 3)?,
        state: enum_col("TaskState", get(r, 4)?, TaskState::parse)?,
        return_state: opt_enum(get(r, 5)?, "TaskState", TaskState::parse)?,
        requirements_seq: get::<i64>(r, 6)? as u32,
        lock_version: get::<i64>(r, 7)? as u64,
        created_at: parse_ts(get(r, 8)?)?,
        updated_at: parse_ts(get(r, 9)?)?,
    })
}

pub fn project_from_row(r: &Row) -> StoreResult<Project> {
    Ok(Project {
        id: parse_id(get(r, 0)?)?,
        name: get(r, 1)?,
        description: get(r, 2)?,
        repos: arr(get(r, 3)?),
        created_at: parse_ts(get(r, 4)?)?,
        updated_at: parse_ts(get(r, 5)?)?,
    })
}

pub fn artifact_from_row(r: &Row) -> StoreResult<Artifact> {
    let contract: Option<String> = get(r, 10)?;
    let version: Option<i64> = get(r, 11)?;
    Ok(Artifact {
        id: parse_id(get(r, 0)?)?,
        task_id: parse_id(get(r, 1)?)?,
        kind: enum_col("ArtifactKind", get(r, 2)?, ArtifactKind::parse)?,
        name: get(r, 3)?,
        seq: get::<i64>(r, 4)? as u32,
        category: enum_col("DataCategory", get(r, 5)?, DataCategory::parse)?,
        status: enum_col("ArtifactStatus", get(r, 6)?, ArtifactStatus::parse)?,
        sha256: get(r, 7)?,
        size: get::<i64>(r, 8)? as u64,
        media_type: get(r, 9)?,
        contract: match (contract, version) {
            (Some(name), Some(v)) => Some(ContractRef { name, version: v as u32 }),
            _ => None,
        },
        created_by: get(r, 12)?,
        execution_id: opt_id(get(r, 13)?)?,
        depends_on: arr(get(r, 14)?),
        created_at: parse_ts(get(r, 15)?)?,
    })
}

pub fn event_from_row(r: &Row) -> StoreResult<WorkflowEvent> {
    Ok(WorkflowEvent {
        id: parse_id(get(r, 0)?)?,
        task_id: opt_id(get(r, 1)?)?,
        kind: get(r, 2)?,
        actor: get(r, 3)?,
        payload: serde_json::from_str(&get::<String>(r, 4)?)
            .map_err(|e| StoreError::Corrupt(format!("bad event payload: {e}")))?,
        created_at: parse_ts(get(r, 5)?)?,
    })
}

pub fn role_from_row(r: &Row) -> StoreResult<RoleDefinition> {
    let c = |s: Option<String>| -> StoreResult<Option<ContractRef>> {
        match s.as_deref() {
            None | Some("") => Ok(None),
            Some(v) => ContractRef::parse(v).map(Some).map_err(|e| StoreError::Corrupt(e.to_string())),
        }
    };
    Ok(RoleDefinition {
        id: parse_id(get(r, 0)?)?,
        slug: get(r, 1)?,
        version: get::<i64>(r, 2)? as u32,
        name: get(r, 3)?,
        purpose: get(r, 4)?,
        instructions: get(r, 5)?,
        tools: arr(get(r, 6)?),
        input_contract: c(get(r, 7)?)?,
        output_contract: c(get(r, 8)?)?,
        done_criteria: get(r, 9)?,
        created_at: parse_ts(get(r, 10)?)?,
    })
}

pub fn agent_from_row(r: &Row) -> StoreResult<AgentInstance> {
    Ok(AgentInstance {
        id: parse_id(get(r, 0)?)?,
        name: get(r, 1)?,
        provider: get(r, 2)?,
        model: get(r, 3)?,
        runtime: get(r, 4)?,
        created_at: parse_ts(get(r, 5)?)?,
    })
}

pub fn execution_from_row(r: &Row) -> StoreResult<RoleExecution> {
    let revision: Option<String> = get(r, 11)?;
    Ok(RoleExecution {
        id: parse_id(get(r, 0)?)?,
        task_id: parse_id(get(r, 1)?)?,
        role_slug: get(r, 2)?,
        role_version: get::<i64>(r, 3)? as u32,
        agent_name: get(r, 4)?,
        status: enum_col("ExecutionStatus", get(r, 5)?, ExecutionStatus::parse)?,
        inputs: arr(get(r, 6)?),
        summary: get(r, 7)?,
        error: get(r, 8)?,
        started_at: parse_ts(get(r, 9)?)?,
        finished_at: opt_ts(get(r, 10)?)?,
        // Pre-v2 rows carry NULL: the revision is absent, never guessed.
        workspace_revision: revision
            .as_deref()
            .map(|s| serde_json::from_str::<WorkspaceRevision>(s).map_err(|e| StoreError::Corrupt(format!("bad workspace_revision: {e}"))))
            .transpose()?,
    })
}

pub fn decision_from_row(r: &Row) -> StoreResult<Decision> {
    Ok(Decision {
        id: parse_id(get(r, 0)?)?,
        task_id: parse_id(get(r, 1)?)?,
        kind: get(r, 2)?,
        verdict: enum_col("Verdict", get(r, 3)?, Verdict::parse)?,
        actor: get(r, 4)?,
        rationale: get(r, 5)?,
        refs: arr(get(r, 6)?),
        created_at: parse_ts(get(r, 7)?)?,
    })
}

pub fn question_from_row(r: &Row) -> StoreResult<Question> {
    Ok(Question {
        id: parse_id(get(r, 0)?)?,
        task_id: parse_id(get(r, 1)?)?,
        execution_id: opt_id(get(r, 2)?)?,
        text: get(r, 3)?,
        context: get(r, 4)?,
        options: arr(get(r, 5)?),
        blocking: get::<i64>(r, 6)? != 0,
        status: enum_col("QuestionStatus", get(r, 7)?, QuestionStatus::parse)?,
        answer: get(r, 8)?,
        answered_by: get(r, 9)?,
        answered_at: opt_ts(get(r, 10)?)?,
        created_at: parse_ts(get(r, 11)?)?,
    })
}
