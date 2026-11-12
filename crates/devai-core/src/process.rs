//! THE DECLARED PROCESS (REQ-AR-09/11/14/15/16) — the state machine as DATA.
//!
//! This module owns the declaration and NOTHING else in the domain does: which
//! states exist, who executes them (`role` / `human` / `none`), which named
//! outcome leaves a state and WHERE it leads, where the custody holds park a
//! task, and which brakes stop it. `workflow::*` asks this type for every
//! target it walks to — a hardcoded target anywhere else would be a second
//! source of truth and the declaration would go back to being decoration (QA
//! R1 of task 01a1b7dc, `arch/registry-in-data`).
//!
//! Editing `config/workflow/process.yaml` therefore changes behaviour. A
//! declaration that cannot express what the code needs is refused at
//! initialisation, not at run time: [`Process::validate`] cross-checks the
//! declared edges against [`crate::workflow::emitted_edges`] and refuses
//! loudly. Nothing here reads or writes IO beyond the embedded factory bytes.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};
use crate::types::TaskState;

/// The declaration version THIS build speaks. A home whose
/// `config/workflow/process.yaml` declares another version is refused with a
/// typed config error at `devai init` (REQ-AR-16): the platform never runs a
/// home silently under rules it was not built for.
///
/// Version 3 replaced the silent round brake (`max_rounds_without_shift`)
/// with the declared attempt limit (`max_attempts_without_progress`).
pub const SUPPORTED_PROCESS_VERSION: u32 = 4;

/// The special transition address: «the remembered working stage» of a hold
/// (`return_state`), not a fixed state name. Spelled once.
pub const PROCESS_RETURN_STATE: &str = "RETURN_STATE";

/// Who executes a state (REQ-AR-09). Closed vocabulary, declared per state.
pub mod process_executor {
    pub const ROLE: &str = "role";
    pub const HUMAN: &str = "human";
    pub const NONE: &str = "none";
}

/// The closed vocabulary of condition names — the causal label of an edge.
/// Each name belongs to ONE command that moves the task, so `(from, on)` is
/// unambiguous: `spec_published` always means «a requirements version was
/// published», `implementation_reported` always means «the implementation
/// report landed». A shared `role_finished` would make REWORK ambiguous (a
/// published spec returns it to REVIEW, a report sends it to QA), and an
/// ambiguous declaration cannot be authoritative.
///
/// This table is the CODE's half of the vocabulary; the other half is the
/// declaration's `process.conditions` list (`config/workflow/process.yaml`).
/// The two are cross-checked BOTH ways in `Process::validate`
/// (`validate_condition_vocabulary`): a name demanded and not declared, or
/// declared and never emitted, refuses `devai init`. Neither side is a
/// silent source of truth for the other.
pub mod process_condition {
    /// A human started the task (DRAFT → the first role stage).
    pub const HUMAN_START: &str = "human_start";
    /// The statement checker accepted the task's OWN statement.
    pub const STATEMENT_APPROVED: &str = "statement_approved";
    /// A requirements version was published (role publication or `revise`).
    pub const SPEC_PUBLISHED: &str = "spec_published";
    /// The reviewer finished its report with a go verdict.
    pub const REVIEW_FINISHED: &str = "review_finished";
    /// The reviewer sent the requirements back for rework.
    pub const REVIEW_REJECTED: &str = "review_rejected";
    /// The operator signed the requirements gate.
    pub const HUMAN_APPROVED: &str = "human_approved";
    /// The operator rejected the requirements gate.
    pub const HUMAN_REJECTED: &str = "human_rejected";
    /// The implementation report landed (IMPLEMENTATION/REWORK → QA).
    pub const IMPLEMENTATION_REPORTED: &str = "implementation_reported";
    /// The operator accepted the result.
    pub const HUMAN_ACCEPTED: &str = "human_accepted";
    /// The operator returned the result for rework.
    pub const HUMAN_RETURNED: &str = "human_returned";
    /// The operator released a hold.
    pub const HUMAN_RESUMED: &str = "human_resumed";
    /// The role's launch failed (spawn/platform failure — never «the role
    /// worked», REQ-AR-13).
    pub const LAUNCH_FAILED: &str = "launch_failed";
    /// The execution produced no outcome at all (the vanished process,
    /// REQ-AR-05 AC2) — spelled out so «no outcome» has a transition too.
    ///
    /// Emission status, stated rather than implied: this build declares the
    /// name and every role state covers it (init refuses a gap, REQ-AR-14),
    /// but NO command walks the edge today. A provably vanished owner is
    /// recorded as a FAILED row with `process_vanished`/`host_restarted`
    /// (`reclaim.rs`) and the slot is freed; the task stays where the
    /// declaration left it for the next launch, because moving it would mean
    /// an automatic retry — forbidden by the human decision (REQ-AR-17). An
    /// emitting command would be a BEHAVIOUR change, not a comment fix, so it
    /// is not smuggled in here.
    pub const OUTCOME_MISSING: &str = "outcome_missing";

    /// Every name above — the closed list a declaration must cover.
    pub const ALL: &[&str] = &[
        HUMAN_START,
        STATEMENT_APPROVED,
        SPEC_PUBLISHED,
        REVIEW_FINISHED,
        REVIEW_REJECTED,
        HUMAN_APPROVED,
        HUMAN_REJECTED,
        IMPLEMENTATION_REPORTED,
        HUMAN_ACCEPTED,
        HUMAN_RETURNED,
        HUMAN_RESUMED,
        LAUNCH_FAILED,
        OUTCOME_MISSING,
    ];

    /// The names that END a role's work normally. A role state must cover at
    /// least one of them, otherwise it can never leave on success.
    pub const ROLE_FINISHES: &[&str] = &[
        STATEMENT_APPROVED,
        SPEC_PUBLISHED,
        REVIEW_FINISHED,
        IMPLEMENTATION_REPORTED,
    ];
    /// Names a role state MUST cover (REQ-AR-14): a role can always fail to
    /// launch or vanish, and a state that cannot express those strands.
    pub const ROLE_REQUIRED: &[&str] = &[LAUNCH_FAILED, OUTCOME_MISSING];
    /// Names a human state must cover at least one of.
    pub const HUMAN_NAMES: &[&str] = &[
        HUMAN_START,
        HUMAN_APPROVED,
        HUMAN_REJECTED,
        HUMAN_ACCEPTED,
        HUMAN_RETURNED,
        HUMAN_RESUMED,
    ];
}

/// The custody holds a task can be parked in — platform acts, not process
/// outcomes. Their ADDRESS is declared data too (`process.holds`), so no
/// command assigns a hold state from code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldKind {
    Pause,
    Block,
    Cancel,
    Fail,
    Question,
}

impl HoldKind {
    /// The declaration key of this hold.
    pub fn key(self) -> &'static str {
        match self {
            HoldKind::Pause => "pause",
            HoldKind::Block => "block",
            HoldKind::Cancel => "cancel",
            HoldKind::Fail => "fail",
            HoldKind::Question => "question",
        }
    }
}

/// One state of the declared process.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessState {
    /// The `TaskState` wire name (e.g. `IMPLEMENTATION`).
    pub state: String,
    /// `role` | `human` | `none` (closed vocabulary).
    pub executor: String,
    /// The role slug for `executor: role`; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Every outcome this state can produce. Each must have a transition
    /// (REQ-AR-14); an unknown name is refused (REQ-AR-15).
    #[serde(default)]
    pub outcomes: Vec<String>,
}

/// One declared transition: `from` + condition name `on` → `to`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessTransition {
    pub from: String,
    pub on: String,
    pub to: String,
}

/// The declared brakes (REQ-AR-09/13). A tripped brake parks the task in
/// `park_state` with the reason and the counter; nothing auto-continues from
/// a park.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessBrakes {
    /// The attempt limit: how many COMPLETED executions of ONE role may pass
    /// WITHOUT a substantive shift before the platform stops the task with ONE
    /// human question. The counter lives on the (task, role) pair and is read
    /// from stored history: a converging round of the stage's own series (the
    /// score rises), a stored verdict change, or a move into a state the task
    /// had never reached — a repeated round of the same loop is NOT a shift.
    /// `5` in the shipped declaration; `None`, empty or `0` means «без
    /// предела». The number is DATA only — no copy lives in code.
    #[serde(default, deserialize_with = "attempt_limit_field")]
    pub max_attempts_without_progress: Option<u32>,
    /// Consecutive failed LAUNCHES (not role results): at this count the
    /// task parks (REQ-AR-13).
    pub max_consecutive_failures: u32,
    /// Failure CLASSES that park on their FIRST occurrence (REQ-AR-09: «классы
    /// отказов, паркующие сразу»). Names come from the closed `failure_class`
    /// vocabulary; an unknown name refuses init.
    #[serde(default)]
    pub park_immediately: Vec<String>,
    /// Where a tripped brake parks the task — a hold state.
    pub park_state: String,
}

/// One measured convergence series, declared as DATA (rework R2): the stage
/// name the runtime's convergence slice publishes and the working states whose
/// work that series compares. A third series is a declaration edit, not a code
/// catalog; a state judged by two series is refused at init, so the reading is
/// unambiguous.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessSeries {
    /// The stage token the convergence slice publishes (`spec_review`, ...).
    pub stage: String,
    /// The working states whose work this series measures.
    pub states: Vec<String>,
}

/// The declared address of every custody hold.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessHolds {
    pub pause: String,
    pub block: String,
    pub cancel: String,
    pub fail: String,
    pub question: String,
}

impl ProcessHolds {
    /// The declared address of one hold.
    pub fn address(&self, kind: HoldKind) -> &str {
        match kind {
            HoldKind::Pause => &self.pause,
            HoldKind::Block => &self.block,
            HoldKind::Cancel => &self.cancel,
            HoldKind::Fail => &self.fail,
            HoldKind::Question => &self.question,
        }
    }
}

impl ProcessBrakes {
    /// The EFFECTIVE attempt limit: `None` when the declaration says «без
    /// предела» (key absent, empty, or `0`). The single reading of the number
    /// — the admission check, the status line and the stop question all ask
    /// this, so none of them can interpret zero on its own.
    pub fn attempt_limit(&self) -> Option<u32> {
        self.max_attempts_without_progress.filter(|n| *n > 0)
    }
}

/// The attempt-limit field's ONE reader (REQ-AL-04): a non-negative integer,
/// or empty for «без предела». Negative, fractional and non-numeric values are
/// a LOUD declaration error naming the key — a typo must never degrade into
/// «no limit» or into a stop on the first run.
fn attempt_limit_field<'de, D>(d: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let value = serde_yaml::Value::deserialize(d)?;
    match value {
        serde_yaml::Value::Null => Ok(None),
        serde_yaml::Value::Number(n) => match n.as_u64() {
            Some(u) => u32::try_from(u).map(Some).map_err(|_| {
                D::Error::custom(
                    "brakes.max_attempts_without_progress is out of range — it must be a \
                     non-negative integer (0 or empty means no limit)",
                )
            }),
            None => match n.as_i64() {
                Some(i) if i < 0 => Err(D::Error::custom(format!(
                    "brakes.max_attempts_without_progress must be a non-negative integer (got \
                     {i}); 0 or empty means no limit"
                ))),
                _ => Err(D::Error::custom(
                    "brakes.max_attempts_without_progress must be an integer or empty; 0 or \
                     empty means no limit",
                )),
            },
        },
        other => Err(D::Error::custom(format!(
            "brakes.max_attempts_without_progress must be an integer or empty (0 or empty means \
             no limit), got `{}`",
            serde_yaml::to_string(&other)
                .unwrap_or_default()
                .trim()
                .to_string()
        ))),
    }
}

/// Where a declared edge leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A fixed state name.
    State(TaskState),
    /// The remembered working stage of the hold being released.
    ReturnState,
}

/// The whole process declaration.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Process {
    pub version: u32,
    /// The closed list of condition names transitions may use.
    pub conditions: Vec<String>,
    pub states: Vec<ProcessState>,
    pub transitions: Vec<ProcessTransition>,
    pub brakes: ProcessBrakes,
    pub holds: ProcessHolds,
    /// The measured convergence series and the states each one judges. Empty
    /// is legitimate (REQ-AL-03 AC4): progress then rests on state moves and
    /// verdicts alone. `default` keeps a declaration written before the
    /// catalog existed loadable.
    #[serde(default)]
    pub series: Vec<ProcessSeries>,
}

impl Process {
    /// The role that executes `state`, when the declaration names one. This
    /// is the question the launcher and the engine ask — «запуск производен
    /// от состояния» (REQ-AR-11).
    pub fn role_for(&self, state: &str) -> Option<&str> {
        self.states
            .iter()
            .find(|s| s.state == state && s.executor == process_executor::ROLE)
            .and_then(|s| s.role.as_deref())
    }

    /// The declared executor of `state` (`role`/`human`/`none`), if declared.
    pub fn executor_for(&self, state: &str) -> Option<&str> {
        self.states
            .iter()
            .find(|s| s.state == state)
            .map(|s| s.executor.as_str())
    }

    /// The roles the declaration CONFINES to `state`: named by that state's
    /// row and by no other row. This is the declaration's answer to «which
    /// roles are the executors of this stage only?» — the QA gate and the
    /// active-executions view ask it for the audit roster, so the roster is
    /// derived from data instead of a second list in code (rework R1/R4 of
    /// task 01a1b7dc).
    pub fn roles_confined_to(&self, state: &str) -> Vec<&str> {
        self.states
            .iter()
            .filter(|s| s.state == state && s.executor == process_executor::ROLE)
            .filter_map(|s| s.role.as_deref())
            .filter(|role| {
                !self
                    .states
                    .iter()
                    .any(|other| other.state != state && other.role.as_deref() == Some(*role))
            })
            .collect()
    }

    /// The states ONE declared convergence series judges (rework R2): the
    /// declaration's answer, never a code catalog. No declared series → an
    /// empty slice, and progress falls back to state moves and verdicts.
    pub fn series_states(&self, stage: &str) -> &[String] {
        self.series
            .iter()
            .find(|s| s.stage == stage)
            .map(|s| s.states.as_slice())
            .unwrap_or(&[])
    }

    /// The measured series that judges `state`, if the declaration names one.
    /// A state lives in at most one series (checked at init), so the answer is
    /// unambiguous.
    pub fn series_of(&self, state: &str) -> Option<&str> {
        self.series
            .iter()
            .find(|s| s.states.iter().any(|st| st == state))
            .map(|s| s.stage.as_str())
    }

    /// Does the measured series `stage` speak for `role` — is any state it
    /// judges executed by that role? ONE reading shared by the series rounds
    /// and the verdicts, so neither can attribute a shift to another role.
    pub fn series_judges(&self, stage: &str, role: &str) -> bool {
        self.series_states(stage)
            .iter()
            .any(|state| self.role_for(state) == Some(role))
    }

    /// The raw declared target of one edge (`RETURN_STATE` included).
    pub fn transition_target(&self, from: &str, on: &str) -> Option<&str> {
        self.transitions
            .iter()
            .find(|t| t.from == from && t.on == on)
            .map(|t| t.to.as_str())
    }

    /// THE resolution every workflow command uses: the target of `(from, on)`
    /// as the declaration states it. No edge — a typed refusal naming both
    /// sides, never a code-side default.
    pub fn target(&self, from: &str, on: &str) -> CoreResult<Target> {
        match self.transition_target(from, on) {
            Some(PROCESS_RETURN_STATE) => Ok(Target::ReturnState),
            Some(to) => {
                TaskState::parse(to)
                    .map(Target::State)
                    .map_err(|_| CoreError::UndeclaredEdge {
                        from: from.to_string(),
                        on: format!("{on} → `{to}` (not a state name)"),
                    })
            }
            None => Err(CoreError::UndeclaredEdge {
                from: from.to_string(),
                on: on.to_string(),
            }),
        }
    }

    /// The declared address of one custody hold.
    pub fn hold_target(&self, kind: HoldKind) -> CoreResult<TaskState> {
        let address = self.holds.address(kind);
        TaskState::parse(address).map_err(|_| CoreError::UndeclaredEdge {
            from: kind.key().to_string(),
            on: format!("hold address `{address}` is not a state name"),
        })
    }

    /// The declared park address of a tripped brake.
    pub fn park_state(&self) -> CoreResult<TaskState> {
        TaskState::parse(&self.brakes.park_state).map_err(|_| CoreError::UndeclaredEdge {
            from: "brakes".to_string(),
            on: format!(
                "park_state `{}` is not a state name",
                self.brakes.park_state
            ),
        })
    }

    /// True when the declaration says a failure of `class` parks the task on
    /// its FIRST occurrence (REQ-AR-09: «классы отказов, паркующие сразу»).
    pub fn parks_immediately(&self, class: &str) -> bool {
        self.brakes.park_immediately.iter().any(|c| c == class)
    }

    /// The factory declaration — the embedded `config/workflow/process.yaml`,
    /// parsed ONCE. ONE source for the default: a home without the file and a
    /// home with the untouched factory file behave identically.
    pub fn factory() -> &'static Process {
        static FACTORY: OnceLock<Process> = OnceLock::new();
        FACTORY.get_or_init(|| {
            // invariant: the panic below is unreachable — `FACTORY_YAML` is
            // compiled into this binary by `include_str!`, and the build
            // refuses to ship a declaration that does not parse: the loader
            // tests (`config.rs`) parse these very bytes through `parse()`,
            // and `devai init` validates the same file on every home. So an
            // invalid embedded declaration is a broken build, not a runtime
            // state; returning a `Result` here would push that impossibility
            // onto every caller of `factory()`.
            parse(FACTORY_YAML, "embedded config/workflow/process.yaml")
                .unwrap_or_else(|e| panic!("the embedded process declaration is invalid: {e}"))
        })
    }
}

const FACTORY_YAML: &str = include_str!("../../../config/workflow/process.yaml");

/// Parse and VALIDATE one declaration document (`process:` section). The
/// `where_` prefix names the file in every message, so a loud refusal always
/// says what to edit.
pub fn parse(text: &str, where_: &str) -> Result<Process, String> {
    let doc: serde_yaml::Value = serde_yaml::from_str(text)
        .map_err(|e| format!("{where_}: parse process declaration: {e}"))?;
    let section = doc
        .get("process")
        .ok_or_else(|| format!("{where_}: no `process:` section"))?;
    let policy: Process = serde_yaml::from_value(section.clone()).map_err(|e| {
        format!(
            "{where_}: process section is invalid (keys: version, conditions, states, \
             transitions, holds, series, brakes): {e}"
        )
    })?;
    policy.validate(where_)?;
    Ok(policy)
}

impl Process {
    /// Loud validation (init + stage time). Split by subject so each check
    /// says ONE thing about ONE part of the declaration.
    pub fn validate(&self, where_: &str) -> Result<(), String> {
        self.validate_header(where_)?;
        self.validate_states(where_)?;
        self.validate_series(where_)?;
        self.validate_transitions(where_)?;
        self.validate_holds(where_)?;
        self.validate_brakes(where_)
    }

    /// Version and condition vocabulary: both halves of the header, each in
    /// its own check so a failure names ONE thing.
    fn validate_header(&self, where_: &str) -> Result<(), String> {
        self.validate_version(where_)?;
        self.validate_condition_vocabulary(where_)
    }

    /// The declaration version this build speaks (REQ-AR-16): a home on
    /// another version is refused, never run silently under new rules.
    fn validate_version(&self, where_: &str) -> Result<(), String> {
        if self.version != SUPPORTED_PROCESS_VERSION {
            return Err(format!(
                "{where_}: process.version {} is not supported by this build (expected {}) — \
                 a home on another declaration version must not run silently under these rules; \
                 update the declaration (or the platform) explicitly",
                self.version, SUPPORTED_PROCESS_VERSION
            ));
        }
        Ok(())
    }

    /// The closed condition vocabulary, checked BOTH ways: every name this
    /// build emits is declared, and every declared name is one it emits.
    fn validate_condition_vocabulary(&self, where_: &str) -> Result<(), String> {
        if self.conditions.is_empty() {
            return Err(format!(
                "{where_}: process.conditions is empty — transitions need a closed vocabulary of names"
            ));
        }
        let mut seen: Vec<&str> = Vec::new();
        for c in &self.conditions {
            if c.trim().is_empty() {
                return Err(format!(
                    "{where_}: process.conditions contains a blank name"
                ));
            }
            if seen.contains(&c.as_str()) {
                return Err(format!("{where_}: process.conditions declares `{c}` twice"));
            }
            seen.push(c);
        }
        for name in process_condition::ALL {
            if !seen.contains(name) {
                return Err(format!(
                    "{where_}: process.conditions does not declare `{name}` — this build emits it, \
                     so a transition on it must be expressible"
                ));
            }
        }
        // The check runs BOTH ways (rework R10): the table above is not the
        // source of the list, so a name the declaration invents would be
        // silently accepted here while no command can ever emit it — an
        // inert edge. The declaration and the build must agree exactly, and a
        // disagreement is an init refusal, never a warning.
        self.reject_invented_conditions(&seen, where_)
    }

    /// The reverse half of the vocabulary check: every DECLARED name must be
    /// one this build can emit. A declared name no command produces would be
    /// an edge that can never fire.
    fn reject_invented_conditions(&self, seen: &[&str], where_: &str) -> Result<(), String> {
        if let Some(unknown) = seen.iter().find(|c| !process_condition::ALL.contains(c)) {
            return Err(format!(
                "{where_}: process.conditions declares `{unknown}`, which this build never emits — \
                 the condition vocabulary is closed on both sides (known names: {}); a declared \
                 name no command produces is an edge that can never fire",
                process_condition::ALL.join(", ")
            ));
        }
        Ok(())
    }

    /// Every `TaskState` of this build has a row, no unknown token, and each
    /// row is internally consistent. A new domain state without a declaration
    /// row refuses init (the code and the declaration disagree).
    fn validate_states(&self, where_: &str) -> Result<(), String> {
        if self.states.is_empty() {
            return Err(format!(
                "{where_}: process.states is empty — there is no process to run"
            ));
        }
        let mut seen: Vec<String> = Vec::new();
        for st in &self.states {
            TaskState::parse(&st.state).map_err(|_| {
                format!(
                    "{where_}: process.states names unknown state `{}` (this build knows: {})",
                    st.state,
                    TaskState::ALL
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            if seen.contains(&st.state) {
                return Err(format!(
                    "{where_}: process.states declares `{}` twice",
                    st.state
                ));
            }
            seen.push(st.state.clone());
            self.validate_state(st, where_)?;
        }
        for state in TaskState::ALL {
            if !seen.iter().any(|s| s == state.as_str()) {
                return Err(format!(
                    "{where_}: process.states has no row for `{}` — the code can be in that state, \
                     so the declaration must say who executes it (REQ-AR-16)",
                    state.as_str()
                ));
            }
        }
        Ok(())
    }

    /// The measured series catalog is DATA (rework R2): every stage name is
    /// non-blank and unique, every judged state is a declared role state, and
    /// no state is judged by two series. An empty catalog is allowed — the
    /// declaration then measures nothing and progress rests on moves/verdicts.
    fn validate_series(&self, where_: &str) -> Result<(), String> {
        let mut stages: Vec<&str> = Vec::new();
        let mut judged: Vec<&str> = Vec::new();
        for series in &self.series {
            if series.stage.trim().is_empty() {
                return Err(format!("{where_}: series contains a blank stage name"));
            }
            if stages.contains(&series.stage.as_str()) {
                return Err(format!(
                    "{where_}: series declares stage `{}` twice",
                    series.stage
                ));
            }
            stages.push(&series.stage);
            self.validate_series_states(series, &mut judged, where_)?;
        }
        Ok(())
    }

    /// The states of ONE declared series: known, role-executed, not repeated.
    fn validate_series_states<'a>(
        &self,
        series: &'a ProcessSeries,
        judged: &mut Vec<&'a str>,
        where_: &str,
    ) -> Result<(), String> {
        if series.states.is_empty() {
            return Err(format!(
                "{where_}: series stage `{}` names no state — a measure of nothing is not a series",
                series.stage
            ));
        }
        for state in &series.states {
            TaskState::parse(state).map_err(|_| {
                format!(
                    "{where_}: series stage `{}` names unknown state `{state}`",
                    series.stage
                )
            })?;
            if self.role_for(state).is_none() {
                return Err(format!(
                    "{where_}: series stage `{}` judges state `{state}`, which no role executes — \
                     a series measures role work",
                    series.stage
                ));
            }
            if judged.contains(&state.as_str()) {
                return Err(format!(
                    "{where_}: state `{state}` is judged by two series — the reading would be \
                     ambiguous"
                ));
            }
            judged.push(state);
        }
        Ok(())
    }

    /// One state row: executor/role agreement, a stage the platform can drive,
    /// closed outcome names, edges for every outcome, and the mandatory
    /// coverage per executor kind.
    fn validate_state(&self, st: &ProcessState, where_: &str) -> Result<(), String> {
        self.validate_executor(st, where_)?;
        self.validate_role_stage(st, where_)?;
        self.validate_outcomes(st, where_)?;
        self.validate_coverage(st, where_)
    }

    /// A role state must be a stage this build can drive: the code owns what a
    /// stage publishes, the declaration owns who runs it (REQ-AR-17). A
    /// declaration that elects a role for a state with no stage artifact would
    /// die MID-WALK, so it is refused here — the code⇄declaration disagreement
    /// is an init refusal, never a surprise in battle (REQ-AR-09 AC1).
    fn validate_role_stage(&self, st: &ProcessState, where_: &str) -> Result<(), String> {
        if st.executor != process_executor::ROLE {
            return Ok(());
        }
        let state = TaskState::parse(&st.state).map_err(|_| {
            format!(
                "{where_}: state {} is not a state name — cannot check its stage artifact",
                st.state
            )
        })?;
        if crate::workflow::stage_artifact_kind(state).is_none() {
            return Err(format!(
                "{where_}: state {} is declared as executed by role `{}`, but the platform has \
                 no stage artifact for that state — the declaration and the stage semantics \
                 disagree, so this process cannot be walked (REQ-AR-17)",
                st.state,
                st.role.as_deref().unwrap_or("")
            ));
        }
        Ok(())
    }

    /// Every declared outcome is a CLOSED name and has an edge — a state that
    /// announces an outcome nobody transitions on is a dead end (REQ-AR-14).
    fn validate_outcomes(&self, st: &ProcessState, where_: &str) -> Result<(), String> {
        for outcome in &st.outcomes {
            if !self.conditions.iter().any(|c| c == outcome) {
                return Err(format!(
                    "{where_}: state {} declares outcome `{outcome}` which is not in \
                     process.conditions — the transition vocabulary is closed (REQ-AR-15)",
                    st.state
                ));
            }
            if self.transition_target(&st.state, outcome).is_none() {
                return Err(format!(
                    "{where_}: DEAD END — state {} declares outcome `{outcome}` but no transition \
                     covers it; init refuses a process that can strand the task (REQ-AR-14)",
                    st.state
                ));
            }
        }
        Ok(())
    }

    /// The outcomes each executor kind MUST cover: a role can always fail to
    /// launch or vanish and must have a way to finish; a human gate must be
    /// decidable.
    fn validate_coverage(&self, st: &ProcessState, where_: &str) -> Result<(), String> {
        if st.executor == process_executor::ROLE {
            for required in process_condition::ROLE_REQUIRED {
                if !st.outcomes.iter().any(|o| o == required) {
                    return Err(format!(
                        "{where_}: DEAD END — role state {} does not cover the `{required}` \
                         outcome (REQ-AR-14)",
                        st.state
                    ));
                }
            }
            if !st
                .outcomes
                .iter()
                .any(|o| process_condition::ROLE_FINISHES.contains(&o.as_str()))
            {
                return Err(format!(
                    "{where_}: DEAD END — role state {} covers no successful finish (one of: {})",
                    st.state,
                    process_condition::ROLE_FINISHES.join(", ")
                ));
            }
        }
        if st.executor == process_executor::HUMAN
            && !st
                .outcomes
                .iter()
                .any(|o| process_condition::HUMAN_NAMES.contains(&o.as_str()))
        {
            return Err(format!(
                "{where_}: human state {} covers no human outcome — a gate that can never be \
                 decided is a dead end",
                st.state
            ));
        }
        Ok(())
    }

    /// The executor vocabulary and the role/executor agreement.
    fn validate_executor(&self, st: &ProcessState, where_: &str) -> Result<(), String> {
        match st.executor.as_str() {
            process_executor::ROLE => {
                if st
                    .role
                    .as_deref()
                    .map(|r| r.trim().is_empty())
                    .unwrap_or(true)
                {
                    return Err(format!(
                        "{where_}: state {} has executor `role` but names no role",
                        st.state
                    ));
                }
                Ok(())
            }
            process_executor::HUMAN | process_executor::NONE => {
                if st.role.as_deref().is_some_and(|r| !r.trim().is_empty()) {
                    return Err(format!(
                        "{where_}: state {} has executor `{}` but names a role — only `role` states \
                         are executed by a role",
                        st.state, st.executor
                    ));
                }
                Ok(())
            }
            other => Err(format!(
                "{where_}: state {} declares unknown executor `{other}` (expected `role`, `human` \
                 or `none`)",
                st.state
            )),
        }
    }

    /// Every edge names declared conditions and declared states, and every
    /// edge the CODE needs is present — this is the init-time cross-check
    /// that turns a code⇄declaration divergence into a refusal instead of a
    /// silent preference for one side.
    fn validate_transitions(&self, where_: &str) -> Result<(), String> {
        for t in &self.transitions {
            if !self.conditions.iter().any(|c| c == &t.on) {
                return Err(format!(
                    "{where_}: transition {}.on = `{}` is not a declared condition name — \
                     conditions are named, never expressions (REQ-AR-15)",
                    t.from, t.on
                ));
            }
            if !self.states.iter().any(|s| s.state == t.from) {
                return Err(format!(
                    "{where_}: transition from unknown state `{}`",
                    t.from
                ));
            }
            if t.to != PROCESS_RETURN_STATE && !self.states.iter().any(|s| s.state == t.to) {
                return Err(format!(
                    "{where_}: transition `{}` → `{}` targets unknown state `{}` (or the special \
                     address `{PROCESS_RETURN_STATE}`)",
                    t.from, t.on, t.to
                ));
            }
        }
        for (state, condition) in crate::workflow::emitted_edges() {
            if self.transition_target(state.as_str(), condition).is_none() {
                return Err(format!(
                    "{where_}: the platform code moves `{}` on `{condition}`, and the declaration \
                     has no such transition — a declared process that cannot express what the code \
                     does is refused at init, never silently overridden (REQ-AR-09)",
                    state.as_str()
                ));
            }
        }
        Ok(())
    }

    /// Hold addresses are HOLD states, and the cancel address is a terminal
    /// one: a park must be releasable by a human, a cancellation must end the
    /// cycle (the two are different promises, so each is checked for what it
    /// is).
    fn validate_holds(&self, where_: &str) -> Result<(), String> {
        for kind in [
            HoldKind::Pause,
            HoldKind::Block,
            HoldKind::Fail,
            HoldKind::Question,
        ] {
            let address = self.holds.address(kind);
            let parsed = TaskState::parse(address).map_err(|_| {
                format!(
                    "{where_}: holds.{} `{address}` is not a state name",
                    kind.key()
                )
            })?;
            if !parsed.is_hold() {
                return Err(format!(
                    "{where_}: holds.{} `{address}` is not a hold state — a held task must be \
                     releasable by a human",
                    kind.key()
                ));
            }
        }
        let cancel = TaskState::parse(&self.holds.cancel).map_err(|_| {
            format!(
                "{where_}: holds.cancel `{}` is not a state name",
                self.holds.cancel
            )
        })?;
        if !cancel.is_terminal() {
            return Err(format!(
                "{where_}: holds.cancel `{}` is not a terminal state — a cancelled task is over, \
                 it is not parked",
                self.holds.cancel
            ));
        }
        Ok(())
    }

    /// The brakes: a known failure-class vocabulary and a hold park address.
    /// The attempt limit is NOT range-checked here: `0`/empty is the declared
    /// «без предела», and a negative or non-numeric value never reaches this
    /// point — the field's own reader refuses it with the key name.
    fn validate_brakes(&self, where_: &str) -> Result<(), String> {
        if self.brakes.max_consecutive_failures == 0 {
            return Err(format!(
                "{where_}: brakes.max_consecutive_failures must be >= 1 — a zero budget parks on \
                 the first failed launch"
            ));
        }
        for class in &self.brakes.park_immediately {
            if !crate::types::failure_class::KNOWN.contains(&class.as_str()) {
                return Err(format!(
                    "{where_}: brakes.park_immediately names `{class}`, which is not a failure \
                     class the platform records (expected one of: {})",
                    crate::types::failure_class::KNOWN.join(", ")
                ));
            }
        }
        let park = TaskState::parse(&self.brakes.park_state).map_err(|_| {
            format!(
                "{where_}: brakes.park_state `{}` is not a state name",
                self.brakes.park_state
            )
        })?;
        if !park.is_hold() {
            return Err(format!(
                "{where_}: brakes.park_state `{}` is not a hold state — a park must be releasable \
                 by a human",
                self.brakes.park_state
            ));
        }
        Ok(())
    }
}
