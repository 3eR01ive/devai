//! Declarative demand table of the human-wait section (task 01a1b0ea).
//!
//! The section lists every point where the platform (or the operator's own
//! stop) needs the operator. WHICH stored state belongs to which group and
//! WHAT the operator is asked to do is project-tunable DATA, not code: the
//! table lives in `config/workflow/inbox-waits.yaml` (the `inbox_waits:`
//! section, the same merge-scan idiom as `humanize:` / `response_form:`)
//! and is loaded by `devai_store::config::load_inbox_wait_policy`. The
//! renderer (`devai-ui`) is generic over this table, so adding a wording or
//! pointing a state at another group is an edit of the file, never a Rust
//! change (R1 of the QA return: the table was 62 lines of match arms).
//!
//! Types live in the domain core because BOTH the loader (`devai-store`) and
//! the consumers (`devai-api`, `devai-ui`) must name them; the core performs
//! no IO and owns no filesystem default — the factory bytes stay in
//! `config/workflow/inbox-waits.yaml`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::types::TaskState;

/// Which group of the wait section a stored state belongs to — the three
/// tokens the section renders, declared in the table per state.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum WaitGroup {
    /// The platform itself stands on the human (APPROVAL / WAITING_FOR_INPUT
    /// / QA / FAILED — ratified OQ-GATE-2).
    Platform,
    /// The operator stopped the task (PAUSED / BLOCKED) — a separate bottom
    /// group: «там стоит не платформа, а выбор оператора» (OQ-GATE-2).
    HumanStopped,
    /// Not a wait state at all (DRAFT, ANALYSIS, … TERMINAL).
    #[serde(rename = "none")]
    NotWait,
}

impl Default for WaitGroup {
    fn default() -> Self {
        Self::NotWait
    }
}

/// One conditional wording of a state's demand: the renderer picks the
/// variant whose `when` predicate the STORED data satisfies (today the
/// parked-question / no-question split of WAITING_FOR_INPUT), never a code
/// branch per case.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WaitVariant {
    /// Machine predicate token, one of [`WHEN_ALL`].
    pub when: String,
    /// What is required FROM the operator, in human words.
    pub demand: String,
    /// Literal CLI templates; `{id}` and `{question_id}` are substituted by
    /// the renderer with stored ids (never a placeholder left in print).
    pub commands: Vec<String>,
}

/// One stored state's row in the table.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields, default)]
pub struct WaitStatePolicy {
    pub group: WaitGroup,
    /// Unconditional demand wording (used when `variants` is empty).
    pub demand: String,
    pub commands: Vec<String>,
    /// Conditional wordings matched by stored data.
    pub variants: Vec<WaitVariant>,
}

/// The whole table: one entry per stored `TaskState` token.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields, default)]
pub struct InboxWaitPolicy {
    pub states: BTreeMap<String, WaitStatePolicy>,
}

/// The variant predicates the renderer knows. A `when` token outside this
/// list is a LOUD config error at load time — an unknown predicate would
/// otherwise silently never match and render an empty demand.
pub const WHEN_PARKED_QUESTION: &str = "parked_question";
pub const WHEN_NO_QUESTION: &str = "no_question";
pub const WHEN_ALL: &[&str] = &[WHEN_PARKED_QUESTION, WHEN_NO_QUESTION];

impl InboxWaitPolicy {
    /// The row of one stored state, keyed by its canonical token.
    pub fn state(&self, state: TaskState) -> Option<&WaitStatePolicy> {
        self.states.get(state.as_str())
    }
}
