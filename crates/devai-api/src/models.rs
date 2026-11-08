//! The model-bundle SURFACE of the platform API (task 01a1b71f,
//! REQ-MS-04..07): one snapshot read, one probe, one switch.
//!
//! Everything the console and the window render comes out of these three
//! methods — the window never walks the store and never composes several
//! operations itself. The declared allow-list is config (`models.yaml`), the
//! active record is the store's ONE marked agent row, the history is the
//! append-only `model.probe` / `model.switched` events, and the run stamps on
//! `role_executions` answer «what was actually used, and when?».
//!
//! Security boundary (REQ-MS-08): the operations accept a bundle ID only.
//! There is no parameter for a driver, a model, an LLM-provider or a
//! base_url, and no read path copies a key/token into its result — the
//! catalogue type has no such field to begin with.

use chrono::{Duration, Utc};
use devai_core::types::{
    ExecutionBundle, Id, ProbeVerdict, SwitchReason, Timestamp, WorkflowEvent, event_kind,
    json_field, new_id,
};
use devai_store::{ModelBundle, ModelCatalog, ProbePolicy};
use serde::Serialize;

use crate::Api;
use crate::error::{ApiError, ApiResult};

/// One declared bundle, as the surface shows it.
#[derive(Serialize, Clone, Debug)]
pub struct ModelBundleView {
    pub id: String,
    pub label: String,
    pub agent: String,
    pub driver: String,
    pub llm_provider: String,
    pub model: String,
    /// REQ-MS-09: the level of thinking — part of the bundle.
    pub reasoning: String,
    pub probe_phrase: String,
    pub probe_timeout_ms: u64,
    pub fresh_secs: u64,
    /// True for the bundle the marked record currently matches.
    pub active: bool,
}

impl ModelBundleView {
    fn of(b: &ModelBundle, active: bool) -> Self {
        Self {
            id: b.id.clone(),
            label: b.label.clone(),
            agent: b.agent.clone(),
            driver: b.driver.clone(),
            llm_provider: b.llm_provider.clone(),
            model: b.model.clone(),
            reasoning: b.reasoning.clone(),
            probe_phrase: b.probe.phrase.clone(),
            probe_timeout_ms: b.probe.timeout_ms,
            fresh_secs: b.probe.fresh_secs,
            active,
        }
    }
}

/// The record in force. `id`/`label` are `None` when the marked agent record
/// is not one of the declared bundles — the surface says «не объявлена в
/// конфиге» instead of borrowing the nearest label.
#[derive(Serialize, Clone, Debug)]
pub struct ActiveBundleView {
    pub id: Option<String>,
    pub label: Option<String>,
    pub agent: String,
    pub driver: String,
    pub llm_provider: String,
    pub model: String,
    /// REQ-MS-09: the level of thinking. Empty on a record outside the
    /// declared list reads «связка неполна» — not a driver default.
    pub reasoning: String,
    pub declared: bool,
}

/// ONE status field (REQ-MS-06): «живая», «не работает», «не доказано»,
/// «неизвестно» — never two indicators at once.
#[derive(Serialize, Clone, Debug)]
pub struct BundleStatusView {
    /// `live` | `not_working` | `not_proven` | `unknown` | `not_selected`.
    pub state: String,
    /// The ONE operator word of `state` (core dictionary, R3): the window and
    /// the console render this field instead of keeping their own mapping.
    pub state_text: String,
    /// The probe's measured latency, when the evidence is a successful probe.
    pub latency_ms: Option<u64>,
    /// The closing verdict the state rests on (`ok`, `no_credits`, …).
    pub verdict: Option<String>,
    /// Failure type for `not_working`: a dead probe verdict, or the stored
    /// failure class of the execution path (`provider`), or the honest
    /// `причина не классифицирована` when the row carried no class.
    pub reason: Option<String>,
    /// The operator word of `reason` (core dictionary, R3).
    pub reason_text: Option<String>,
    /// Which input won: `probe` | `execution`.
    pub evidence: Option<String>,
    pub evidence_at: Option<Timestamp>,
    /// `fresh` | `stale` | `unknown` — derived from the declared window.
    pub freshness: String,
    /// The operator word of `freshness` — it names WHICH evidence is fresh
    /// («свежая проба» vs «свежий прогон»), so a run row is never mislabelled
    /// as a probe (R8 quality return).
    pub freshness_text: String,
    /// The probe exists but is OLDER than the failing execution row it lost to.
    pub probe_older: bool,
}

impl BundleStatusView {
    fn unknown() -> Self {
        Self {
            state: "unknown".into(),
            state_text: devai_core::types::status_word("unknown").into(),
            latency_ms: None,
            verdict: None,
            reason: None,
            reason_text: None,
            evidence: None,
            evidence_at: None,
            freshness: "unknown".into(),
            freshness_text: devai_core::types::freshness_word("unknown", None).into(),
            probe_older: false,
        }
    }
    fn not_selected() -> Self {
        let mut s = Self::unknown();
        s.state = "not_selected".into();
        s.state_text = devai_core::types::status_word("not_selected").into();
        s
    }
}

/// One history row of the bundle surface — a whitelisted projection of the
/// stored event payload (keys, tokens and base_url cannot appear).
#[derive(Serialize, Clone, Debug)]
pub struct ModelEventView {
    pub at: Timestamp,
    pub kind: String,
    pub actor: String,
    pub bundle_id: Option<String>,
    pub verdict: Option<String>,
    pub outcome: Option<String>,
    pub reason: Option<String>,
    /// The operator word of `reason` (core dictionary, R3).
    pub reason_text: Option<String>,
    pub latency_ms: Option<u64>,
    pub from: serde_json::Value,
    pub to: serde_json::Value,
    pub agent_changed: bool,
}

/// A run in flight: task, agent, start. Running rows never decide status —
/// they are listed so the operator sees the switch's blast radius.
#[derive(Serialize, Clone, Debug)]
pub struct RunningBundleRow {
    pub execution_id: String,
    pub task_id: String,
    pub agent: Option<String>,
    pub started_at: Timestamp,
}

/// The refusal banner (REQ-MS-06): shown only when the status is
/// `not_working`; the suggestion is a bundle with the latest FRESH successful
/// probe — and it is never applied by itself.
#[derive(Serialize, Clone, Debug)]
pub struct FailuresBanner {
    pub code: String,
    pub text: String,
    pub suggestion: Option<String>,
}

/// THE snapshot — one call, one object (REQ-MS-07).
#[derive(Serialize, Clone, Debug)]
pub struct ModelSnapshot {
    pub snapshot_at: Timestamp,
    pub active: Option<ActiveBundleView>,
    pub status: BundleStatusView,
    pub declared: Vec<ModelBundleView>,
    pub history: Vec<ModelEventView>,
    pub failures: Option<FailuresBanner>,
    pub running: Vec<RunningBundleRow>,
}

/// One probe as the surface returns it.
#[derive(Serialize, Clone, Debug)]
pub struct ProbeView {
    pub bundle_id: String,
    pub verdict: String,
    /// The operator word of `verdict` (core dictionary, R3).
    pub verdict_text: String,
    pub latency_ms: u64,
    pub exit_code: Option<i32>,
    pub output_excerpt: String,
    pub at: Timestamp,
}

/// The switch answer. `outcome` is `applied` or `refused_probe`; every other
/// refusal is a typed `ApiError` raised BEFORE any event is written.
#[derive(Serialize, Clone, Debug)]
pub struct SwitchResultView {
    pub outcome: String,
    pub bundle_id: String,
    pub verdict: String,
    pub latency_ms: u64,
    pub code: String,
    /// The switch moved the agent NAME (REQ-MS-04 AC4): the UI names the
    /// runs in flight that stay on the previous agent. A refusal never
    /// changes it.
    pub agent_changed: bool,
}

/// The probe evidence stored on the event and returned to the caller.
struct ProbeRecord {
    event_id: Id,
    outcome: devai_runtime::probe::ProbeOutcome,
}

impl Api {
    /// The declared catalogue of this home (config, not store).
    fn model_catalog(&self) -> ApiResult<ModelCatalog> {
        Ok(devai_store::load_catalog(&self.model_config_dir())?)
    }

    /// Resolve a bundle id or refuse with the WHOLE declared list in the
    /// message (REQ-MS-02 AC7 / REQ-MS-08 AC2).
    fn declared_bundle(&self, id: &str) -> ApiResult<ModelBundle> {
        let catalog = self.model_catalog()?;
        catalog.get(id).cloned().ok_or_else(|| {
            ApiError::refused(
                "unknown_bundle",
                format!(
                    "model bundle `{id}` is not declared in {} — probe/switch accept a declared id only; declared: {}",
                    devai_store::MODEL_CONFIG_FILE,
                    if catalog.bundles.is_empty() {
                        "(none)".to_string()
                    } else {
                        catalog.ids().join(", ")
                    }
                ),
            )
        })
    }

    /// `model.snapshot` (REQ-MS-07): active record, ONE status field, the
    /// declared list, the history, the refusal banner and the runs in flight
    /// — read in one pass, no network call. Each answer is its own step
    /// (R2 quality return: the body stays readable in one screen).
    pub fn model_snapshot(&self) -> ApiResult<ModelSnapshot> {
        let catalog = self.model_catalog()?;
        let active_record = self.store.active_agent()?;
        let now = Utc::now();
        let history = self.model_history_view()?;
        let status = self.status_of(active_record.as_ref(), &catalog, &history, now)?;
        Ok(ModelSnapshot {
            snapshot_at: now,
            active: active_record
                .as_ref()
                .map(|a| ActiveBundleView::of(&catalog, a)),
            status: status.clone(),
            declared: declared_views(&catalog, active_record.as_ref()),
            failures: self.failure_banner(&status, &catalog, &history, now, active_record.as_ref()),
            history,
            running: self.running_rows()?,
        })
    }

    /// Step 2 of the snapshot: the single status field of the record in force.
    fn status_of(
        &self,
        active: Option<&devai_core::types::AgentInstance>,
        catalog: &ModelCatalog,
        history: &[ModelEventView],
        now: Timestamp,
    ) -> ApiResult<BundleStatusView> {
        match active {
            None => Ok(BundleStatusView::not_selected()),
            Some(a) => {
                let policy =
                    catalog.policy_for(&a.name, &a.provider, &a.runtime, &a.model, &a.reasoning);
                self.bundle_status(a, &policy, history, now)
            }
        }
    }

    /// Step 3 of the snapshot: the refusal banner is part of the SAME answer
    /// and is present only for `not_working`. It never applies anything.
    fn failure_banner(
        &self,
        status: &BundleStatusView,
        catalog: &ModelCatalog,
        history: &[ModelEventView],
        now: Timestamp,
        active: Option<&devai_core::types::AgentInstance>,
    ) -> Option<FailuresBanner> {
        let active = active?;
        match status.state.as_str() {
            "not_working" => {}
            // Only a proven dead bundle glows; every other state is silent.
            _ => return None,
        }
        let code = status
            .reason
            .clone()
            .unwrap_or_else(|| "не доказано".into());
        Some(FailuresBanner {
            text: format!(
                "связка не работает: {} — переключитесь на живую",
                devai_core::types::failure_word(&code)
            ),
            code,
            suggestion: self.suggest_live_bundle(catalog, history, now, active),
        })
    }

    /// Step 4 of the snapshot: the runs in flight, read by STATUS (R4 quality
    /// return) — never by loading the whole execution history.
    fn running_rows(&self) -> ApiResult<Vec<RunningBundleRow>> {
        Ok(self
            .store
            .running_executions()?
            .into_iter()
            .map(|e| RunningBundleRow {
                execution_id: e.id.to_string(),
                task_id: e.task_id.to_string(),
                agent: e.agent_name.clone(),
                started_at: e.started_at,
            })
            .collect())
    }

    /// `model.probe` (REQ-MS-03): one real call, one event, no state change.
    pub fn model_probe(&self, id: &str) -> ApiResult<ProbeView> {
        let bundle = self.declared_bundle(id)?;
        let record = self.run_probe(&bundle)?;
        Ok(ProbeView {
            bundle_id: bundle.id.clone(),
            verdict: record.outcome.verdict.as_str().to_string(),
            verdict_text: devai_core::types::verdict_word(record.outcome.verdict.as_str()).into(),
            latency_ms: record.outcome.latency_ms,
            exit_code: record.outcome.exit_code,
            output_excerpt: record.outcome.output_excerpt,
            at: record.outcome.at,
        })
    }

    /// `model.switch` (REQ-MS-04): probe the target, decide, and — only on
    /// `ok` — move the record and the marker in ONE transaction with the
    /// event. A dead verdict refuses and writes `refused_probe`; unknown id,
    /// `no_op` and `not_probeable` refuse before any event exists. A write
    /// failure after the probe writes a `failed` fact with its STEP and
    /// leaves the rolled-back bundle in force (REQ-MS-05: «шаг отказа»).
    pub fn model_switch(&self, id: &str, reason: &str) -> ApiResult<SwitchResultView> {
        let rate = SwitchReason::parse(reason).ok_or_else(|| ApiError::InvalidInput {
            field: "reason",
            message: format!("`{reason}` is not a switch reason (manual|limits|error)"),
        })?;
        let bundle = self.declared_bundle(id)?;
        let previous = self.store.active_agent()?;
        refuse_no_op(previous.as_ref(), &bundle)?;
        refuse_unprobeable(&bundle)?;
        let probe = self.run_probe(&bundle)?;
        let attempt = SwitchAttempt {
            bundle: &bundle,
            probe: &probe,
            from: previous
                .as_ref()
                .map(record_identity)
                .unwrap_or(serde_json::Value::Null),
            to: bundle_json(&bundle),
            rate,
            agent_changed: previous
                .as_ref()
                .map(|p| p.name != bundle.agent)
                .unwrap_or(false),
        };
        if probe.outcome.verdict != ProbeVerdict::Ok {
            return self.refuse_probe(&attempt);
        }
        self.apply_switch(&attempt)
    }

    /// A dead probe: the probe event is already stored; the switch fact says
    /// `refused_probe` with the SAME verdict, and the record/marker never
    /// move, so `agent_changed` is false (R8 quality return).
    fn refuse_probe(&self, a: &SwitchAttempt) -> ApiResult<SwitchResultView> {
        self.store.insert_event(
            None,
            event_kind::MODEL_SWITCHED,
            self.actor(),
            switch_payload(a, "refused_probe", false, None),
        )?;
        Ok(SwitchResultView {
            outcome: "refused_probe".into(),
            bundle_id: a.bundle.id.clone(),
            verdict: a.probe.outcome.verdict.as_str().into(),
            latency_ms: a.probe.outcome.latency_ms,
            code: "probe_refused".into(),
            agent_changed: false,
        })
    }

    /// The approved move: ONE store transaction. A failure rolls the whole
    /// bundle back; the `failed` fact is written AFTER the rollback so the
    /// history keeps the attempt and names the step that died.
    fn apply_switch(&self, a: &SwitchAttempt) -> ApiResult<SwitchResultView> {
        let switch = devai_store::BundleSwitch {
            agent: a.bundle.agent.clone(),
            driver: a.bundle.driver.clone(),
            llm_provider: a.bundle.llm_provider.clone(),
            model: a.bundle.model.clone(),
            reasoning: a.bundle.reasoning.clone(),
            actor: self.actor().to_string(),
            payload: switch_payload(a, "applied", a.agent_changed, None),
            now: Utc::now(),
        };
        if let Err(e) = self.store.apply_bundle_switch(&switch) {
            // REQ-MS-05: the failed outcome is a fact of its own with the
            // failing step; the transaction above already rolled back.
            self.store.insert_event(
                None,
                event_kind::MODEL_SWITCHED,
                self.actor(),
                switch_payload(a, "failed", false, Some("apply_bundle_switch")),
            )?;
            return Err(e.into());
        }
        Ok(SwitchResultView {
            outcome: "applied".into(),
            bundle_id: a.bundle.id.clone(),
            verdict: a.probe.outcome.verdict.as_str().into(),
            latency_ms: a.probe.outcome.latency_ms,
            code: "applied".into(),
            agent_changed: a.agent_changed,
        })
    }

    /// Run one probe and store its fact event — exactly one event per call
    /// (REQ-MS-03 AC4).
    fn run_probe(&self, bundle: &ModelBundle) -> ApiResult<ProbeRecord> {
        let request = devai_runtime::probe::ProbeRequest::new(
            &bundle.driver,
            &bundle.llm_provider,
            &bundle.model,
            &bundle.reasoning,
        )
        .with_policy(&bundle.probe.phrase, bundle.probe.timeout_ms);
        let outcome = (self.probe_runner)(&request);
        let event_id = new_id();
        self.store.insert_event_with(
            event_id,
            None,
            event_kind::MODEL_PROBE,
            self.actor(),
            serde_json::json!({
                (json_field::BUNDLE_ID): bundle.id,
                (json_field::BUNDLE): {
                    (json_field::DRIVER): bundle.driver,
                    (json_field::LLM_PROVIDER): bundle.llm_provider,
                    (json_field::MODEL): bundle.model,
                    (json_field::REASONING): bundle.reasoning,
                },
                (json_field::VERDICT): outcome.verdict.as_str(),
                (json_field::LATENCY_MS): outcome.latency_ms,
                (json_field::EXIT_CODE): outcome.exit_code,
                (json_field::OUTPUT_EXCERPT): outcome.output_excerpt,
            }),
            outcome.at,
        )?;
        Ok(ProbeRecord { event_id, outcome })
    }

    /// The stored history, oldest first, as the whitelisted projection.
    fn model_history_view(&self) -> ApiResult<Vec<ModelEventView>> {
        Ok(self
            .store
            .model_events()?
            .iter()
            .map(event_view)
            .collect::<Vec<_>>())
    }

    /// The status rule (REQ-MS-06), ONE field with a full priority: the
    /// fresher of (last probe of the active identity, newest finished
    /// execution row with that stamp) wins; equal timestamps favour the
    /// probe. Running rows are not evidence. The decision and the two
    /// renderings are separate steps (R2 quality return).
    fn bundle_status(
        &self,
        active: &devai_core::types::AgentInstance,
        policy: &ProbePolicy,
        history: &[ModelEventView],
        now: Timestamp,
    ) -> ApiResult<BundleStatusView> {
        let probe = last_probe_of(history, active);
        let execution = self.store.latest_finished_execution_for_stamp(
            &active.provider,
            &active.runtime,
            &active.model,
            &active.reasoning,
        )?;
        let probe_at = probe.as_ref().map(|p| p.at);
        let exec_at = execution.as_ref().and_then(|e| e.finished_at);
        Ok(match (probe, execution) {
            // A strictly later finished row wins and the answer SAYS the
            // probe is older; an equal time is the probe's.
            (Some(_), Some(ex)) if exec_at > probe_at => execution_status(&ex, policy, now, true),
            (Some(p), _) => probe_status(&p, policy, now),
            (None, Some(ex)) => execution_status(&ex, policy, now, false),
            (None, None) => BundleStatusView::unknown(),
        })
    }

    /// The suggestion of the refusal banner: a DECLARED bundle — not the one
    /// already in force (switching to it would be a `no_op`) — whose LAST
    /// probe is `ok` and still FRESH. None when nothing qualifies; the
    /// platform never applies a switch by itself.
    fn suggest_live_bundle(
        &self,
        catalog: &ModelCatalog,
        history: &[ModelEventView],
        now: Timestamp,
        active: &devai_core::types::AgentInstance,
    ) -> Option<String> {
        catalog
            .bundles
            .iter()
            .filter(|b| !same_identity(active, b))
            .filter(|b| {
                history
                    .iter()
                    .filter(|e| {
                        e.kind == event_kind::MODEL_PROBE
                            && e.bundle_id.as_deref() == Some(b.id.as_str())
                            && e.verdict.as_deref() == Some(ProbeVerdict::Ok.as_str())
                    })
                    // The LAST probe of that bundle is the newest by time —
                    // the same rule the status field uses.
                    .max_by_key(|e| e.at)
                    .map(|e| freshness(e.at, b.probe.fresh_secs, now) == "fresh")
                    .unwrap_or(false)
            })
            .map(|b| b.id.clone())
            .next()
    }
}

/// The newest probe row of the record's identity, by TIME (the history is
/// append-only and may be imported out of order — REQ-MS-06).
fn last_probe_of(
    history: &[ModelEventView],
    active: &devai_core::types::AgentInstance,
) -> Option<ModelEventView> {
    history
        .iter()
        .filter(|e| {
            e.kind == event_kind::MODEL_PROBE
                && e.bundle_id.is_some()
                && e.bundle_matches(
                    &active.provider,
                    &active.runtime,
                    &active.model,
                    &active.reasoning,
                )
        })
        .max_by_key(|e| e.at)
        .cloned()
}

/// The probe branch of the status field: live/dead/not-proven comes from the
/// VERDICT, never from a re-read of a state string.
fn probe_status(probe: &ModelEventView, policy: &ProbePolicy, now: Timestamp) -> BundleStatusView {
    let verdict = probe
        .verdict
        .as_deref()
        .and_then(ProbeVerdict::parse)
        .unwrap_or(ProbeVerdict::Unknown);
    let live = verdict == ProbeVerdict::Ok;
    let dead = verdict.proves_dead();
    let state = if live {
        "live"
    } else if dead {
        "not_working"
    } else {
        "not_proven"
    };
    let reason = dead.then(|| verdict.as_str().to_string());
    let fresh = freshness(probe.at, policy.fresh_secs, now);
    BundleStatusView {
        state: state.into(),
        state_text: devai_core::types::status_word(state).into(),
        latency_ms: if live { probe.latency_ms } else { None },
        verdict: Some(verdict.as_str().into()),
        reason_text: reason
            .as_deref()
            .map(devai_core::types::failure_word)
            .map(str::to_string),
        reason,
        evidence: Some("probe".into()),
        evidence_at: Some(probe.at),
        freshness_text: devai_core::types::freshness_word(&fresh, Some("probe")).into(),
        freshness: fresh,
        probe_older: false,
    }
}

/// The execution branch: a finished row is evidence; ONLY its stored failure
/// class may name a reason (REQ-MS-06 AC2/AC3), and its freshness is a RUN's,
/// never «свежая проба» (R8 quality return). A finished row outside
/// COMPLETED/FAILED is not evidence for either verdict: «неизвестно».
fn execution_status(
    exec: &devai_core::types::RoleExecution,
    policy: &ProbePolicy,
    now: Timestamp,
    probe_older: bool,
) -> BundleStatusView {
    let (state, reason) = match exec.status.as_str() {
        "COMPLETED" => ("live", None),
        "FAILED" => (
            "not_working",
            Some(
                exec.failure_class
                    .clone()
                    .unwrap_or_else(|| "причина не классифицирована".into()),
            ),
        ),
        _ => ("unknown", None),
    };
    let fresh = match exec.finished_at {
        Some(at) => freshness(at, policy.fresh_secs, now),
        None => "unknown".into(),
    };
    BundleStatusView {
        state: state.into(),
        state_text: devai_core::types::status_word(state).into(),
        latency_ms: None,
        verdict: None,
        reason_text: reason
            .as_deref()
            .map(devai_core::types::failure_word)
            .map(str::to_string),
        reason,
        evidence: Some("execution".into()),
        evidence_at: exec.finished_at,
        freshness_text: devai_core::types::freshness_word(&fresh, Some("execution")).into(),
        freshness: fresh,
        probe_older,
    }
}

/// The immutable facts of one approved-candidate switch: the declared bundle,
/// the probe it rests on and the two identity snapshots. Grouping them keeps
/// each step's signature short (R2 quality return).
struct SwitchAttempt<'a> {
    bundle: &'a ModelBundle,
    probe: &'a ProbeRecord,
    from: serde_json::Value,
    to: serde_json::Value,
    rate: SwitchReason,
    /// True only when an APPLIED switch moved the agent NAME; a refusal or a
    /// rolled-back failure never did, so it stays false there.
    agent_changed: bool,
}

/// The `to` identity snapshot of a declared bundle — the four fields only
/// (the id is the event's separate `bundle_id`).
fn bundle_json(b: &ModelBundle) -> serde_json::Value {
    serde_json::json!({
        (json_field::DRIVER): b.driver,
        (json_field::LLM_PROVIDER): b.llm_provider,
        (json_field::MODEL): b.model,
        (json_field::REASONING): b.reasoning,
    })
}

/// The complete `model.switched` payload in ONE place, so every outcome
/// (`applied` / `refused_probe` / `failed`) carries the same keys; only
/// `failed` carries `failure_step` (REQ-MS-05).
fn switch_payload(
    a: &SwitchAttempt,
    outcome: &str,
    agent_changed: bool,
    failure_step: Option<&str>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        (json_field::BUNDLE_ID): a.bundle.id,
        (json_field::BUNDLE): a.to.clone(),
        (json_field::FROM): a.from.clone(),
        (json_field::VERDICT): a.probe.outcome.verdict.as_str(),
        (json_field::LATENCY_MS): a.probe.outcome.latency_ms,
        (json_field::OUTCOME): outcome,
        (json_field::REASON): a.rate.as_str(),
        (json_field::PROBE_EVENT): a.probe.event_id.to_string(),
        (json_field::AGENT_CHANGED): agent_changed,
    });
    if let (Some(step), serde_json::Value::Object(map)) = (failure_step, &mut payload) {
        map.insert(json_field::FAILURE_STEP.into(), serde_json::json!(step));
    }
    payload
}

/// `no_op`: the target identity already is the one in force — nothing is
/// probed, nothing is written (REQ-MS-04 AC3).
fn refuse_no_op(
    previous: Option<&devai_core::types::AgentInstance>,
    bundle: &ModelBundle,
) -> ApiResult<()> {
    if previous.map(|p| same_identity(p, bundle)).unwrap_or(false) {
        return Err(ApiError::refused(
            "no_op",
            format!(
                "bundle `{}` is already in force — no probe, no event, no change",
                bundle.id
            ),
        ));
    }
    Ok(())
}

/// A declared `mock` bundle has no network leg: refuse before any process or
/// event (REQ-MS-04 AC3). The loader already rejects it; this is the runtime
/// guard for a hand-built catalogue.
fn refuse_unprobeable(bundle: &ModelBundle) -> ApiResult<()> {
    if bundle.driver == "mock" {
        return Err(ApiError::refused(
            "not_probeable",
            format!(
                "bundle `{}` declares driver `mock` — it has no network leg to probe; nothing was written",
                bundle.id
            ),
        ));
    }
    Ok(())
}

impl ActiveBundleView {
    /// Step 1 of the snapshot: the record in force as the surface shows it.
    fn of(catalog: &ModelCatalog, a: &devai_core::types::AgentInstance) -> Self {
        let declared = catalog.for_record(&a.name, &a.provider, &a.runtime, &a.model, &a.reasoning);
        Self {
            id: declared.map(|b| b.id.clone()),
            label: declared.map(|b| b.label.clone()),
            agent: a.name.clone(),
            driver: a.provider.clone(),
            llm_provider: a.runtime.clone(),
            model: a.model.clone(),
            reasoning: a.reasoning.clone(),
            declared: declared.is_some(),
        }
    }
}

/// The declared list with the ONE `active` flag: the record's full identity
/// (all five fields) must equal the bundle's — never a partial match.
fn declared_views(
    catalog: &ModelCatalog,
    active: Option<&devai_core::types::AgentInstance>,
) -> Vec<ModelBundleView> {
    catalog
        .bundles
        .iter()
        .map(|b| ModelBundleView::of(b, active.map(|a| same_identity(a, b)).unwrap_or(false)))
        .collect()
}

/// The declared identity vs the stored record — one predicate, used by the
/// snapshot's `active` flag, the `no_op` pre-check and the suggestion filter.
fn same_identity(a: &devai_core::types::AgentInstance, b: &ModelBundle) -> bool {
    a.name == b.agent
        && a.provider == b.driver
        && a.runtime == b.llm_provider
        && a.model == b.model
        && a.reasoning == b.reasoning
}

/// The identity snapshot of a stored agent record, for the «from» half.
fn record_identity(a: &devai_core::types::AgentInstance) -> serde_json::Value {
    serde_json::json!({
        (json_field::DRIVER): a.provider,
        (json_field::LLM_PROVIDER): a.runtime,
        (json_field::MODEL): a.model,
        (json_field::REASONING): a.reasoning,
    })
}

/// Whitelisted projection of one stored event. Every field is copied BY NAME
/// — a payload key that is not in this list never reaches the surface, so a
/// producer bug cannot leak a secret through history.
fn event_view(e: &WorkflowEvent) -> ModelEventView {
    let p = &e.payload;
    let bundle = p.get(json_field::BUNDLE);
    ModelEventView {
        at: e.created_at,
        kind: e.kind.clone(),
        actor: e.actor.clone(),
        bundle_id: p
            .get(json_field::BUNDLE_ID)
            .and_then(|v| v.as_str())
            .map(str::to_string),
        verdict: p
            .get(json_field::VERDICT)
            .and_then(|v| v.as_str())
            .map(str::to_string),
        outcome: p
            .get(json_field::OUTCOME)
            .and_then(|v| v.as_str())
            .map(str::to_string),
        reason: p
            .get(json_field::REASON)
            .and_then(|v| v.as_str())
            .map(str::to_string),
        reason_text: p
            .get(json_field::REASON)
            .and_then(|v| v.as_str())
            .map(devai_core::types::failure_word)
            .map(str::to_string),
        latency_ms: p.get(json_field::LATENCY_MS).and_then(|v| v.as_u64()),
        from: p
            .get(json_field::FROM)
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        to: bundle.cloned().unwrap_or(serde_json::Value::Null),
        agent_changed: p
            .get(json_field::AGENT_CHANGED)
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
}

impl ModelEventView {
    /// Does this history row's bundle identity equal the active record's?
    fn bundle_matches(
        &self,
        driver: &str,
        llm_provider: &str,
        model: &str,
        reasoning: &str,
    ) -> bool {
        self.to.get(json_field::DRIVER).and_then(|v| v.as_str()) == Some(driver)
            && self
                .to
                .get(json_field::LLM_PROVIDER)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                == llm_provider
            && self
                .to
                .get(json_field::MODEL)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                == model
            && self
                .to
                .get(json_field::REASONING)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                == reasoning
    }
}

/// `fresh` while the evidence is inside the DECLARED window, `stale` after
/// it, `unknown` when there is no evidence at all.
fn freshness(at: Timestamp, fresh_secs: u64, now: Timestamp) -> String {
    let age = now.signed_duration_since(at);
    if age < Duration::zero() {
        // A clock skew forward is still "seen just now", never "from the
        // future" — the status must not go dark because of it.
        return "fresh".into();
    }
    if age.num_seconds() <= fresh_secs as i64 {
        "fresh".into()
    } else {
        "stale".into()
    }
}

/// The LLM-provider as humans read it: empty AND the neutral `local`
/// default both mean «the driver chooses» — the value is never shown as a
/// provider name (REQ-MS-01 AC2).
fn llm_provider_word(value: &str) -> &str {
    if value.trim().is_empty() || value == "local" {
        "не задан — выбирает драйвер"
    } else {
        value
    }
}

/// The declared bundle a probe/switch id resolved to, for the CLI's
/// rendering parity. Kept public so the console shows the same words.
pub fn bundle_identity_text(b: &ModelBundle) -> String {
    format!(
        "driver `{}` · LLM-провайдер `{}` · модель `{}` · уровень размышления `{}` · агент `{}`",
        b.driver,
        llm_provider_word(&b.llm_provider),
        if b.model.is_empty() {
            "не передаётся"
        } else {
            &b.model
        },
        b.reasoning,
        b.agent
    )
}

/// Read a stored run stamp as the config vocabulary (a row without one reads
/// «связка не записана» — REQ-MS-05).
pub fn stamp_text(bundle: &Option<ExecutionBundle>) -> String {
    match bundle {
        None => "связка не записана".into(),
        Some(b) => format!(
            "driver `{}` · LLM-провайдер `{}` · модель `{}` · уровень размышления `{}`",
            b.driver,
            llm_provider_word(&b.llm_provider),
            if b.model.is_empty() {
                "не передаётся"
            } else {
                &b.model
            },
            if b.reasoning.is_empty() {
                "не задан — связка неполна"
            } else {
                &b.reasoning
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devai_runtime::probe::ProbeOutcome;

    /// REQ-MS-05 «шаг отказа» (R5 quality return): the `failed` outcome is a
    /// real event shape — it names the step that died and never claims an
    /// agent-name change (the transaction rolled back). Applied/refused
    /// carry no step.
    fn attempt() -> SwitchAttempt<'static> {
        let bundle = Box::leak(Box::new(ModelBundle {
            id: "hermes-live".into(),
            label: "Hermes Live".into(),
            agent: "hermes".into(),
            driver: "hermes".into(),
            llm_provider: "deepseek".into(),
            model: "live-1".into(),
            reasoning: "high".into(),
            probe: ProbePolicy::default(),
        }));
        let probe = Box::leak(Box::new(ProbeRecord {
            event_id: new_id(),
            outcome: ProbeOutcome {
                verdict: ProbeVerdict::Ok,
                latency_ms: 7,
                exit_code: Some(0),
                output_excerpt: "pong".into(),
                at: Utc::now(),
            },
        }));
        SwitchAttempt {
            bundle,
            probe,
            from: serde_json::json!({"driver": "claude"}),
            to: bundle_json(bundle),
            rate: SwitchReason::Manual,
            agent_changed: true,
        }
    }

    #[test]
    fn a_failed_switch_names_its_step_and_never_claims_an_agent_change() {
        let a = attempt();
        let failed = switch_payload(&a, "failed", false, Some("apply_bundle_switch"));
        assert_eq!(failed[json_field::OUTCOME], "failed");
        assert_eq!(failed[json_field::FAILURE_STEP], "apply_bundle_switch");
        assert_eq!(failed[json_field::AGENT_CHANGED], false);
        assert_eq!(failed[json_field::VERDICT], "ok");

        let applied = switch_payload(&a, "applied", true, None);
        assert!(applied.get(json_field::FAILURE_STEP).is_none());
        assert_eq!(applied[json_field::AGENT_CHANGED], true);

        let refused = switch_payload(&a, "refused_probe", false, None);
        assert!(refused.get(json_field::FAILURE_STEP).is_none());
        assert_eq!(refused[json_field::AGENT_CHANGED], false);
    }

    /// The event-count contract of REQ-MS-05 AC4: every outcome carries the
    /// same keys; only `failed` adds the step.
    #[test]
    fn every_outcome_carries_the_same_identity_keys() {
        let a = attempt();
        for outcome in ["applied", "refused_probe", "failed"] {
            let p = switch_payload(&a, outcome, false, None);
            for key in [
                json_field::BUNDLE_ID,
                json_field::BUNDLE,
                json_field::FROM,
                json_field::VERDICT,
                json_field::LATENCY_MS,
                json_field::OUTCOME,
                json_field::REASON,
                json_field::PROBE_EVENT,
                json_field::AGENT_CHANGED,
            ] {
                assert!(p.get(key).is_some(), "outcome {outcome} lacks `{key}`");
            }
        }
    }
}
