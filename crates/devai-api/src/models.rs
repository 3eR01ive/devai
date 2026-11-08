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
    event_kind, json_field, new_id, ExecutionBundle, Id, ProbeVerdict, SwitchReason, Timestamp,
    WorkflowEvent,
};
use devai_store::{ModelBundle, ModelCatalog, ProbePolicy};
use serde::Serialize;

use crate::error::{ApiError, ApiResult};
use crate::Api;

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
    /// The probe's measured latency, when the evidence is a successful probe.
    pub latency_ms: Option<u64>,
    /// The closing verdict the state rests on (`ok`, `no_credits`, …).
    pub verdict: Option<String>,
    /// Failure type for `not_working`: a dead probe verdict, or the stored
    /// failure class of the execution path (`provider`), or the honest
    /// `причина не классифицирована` when the row carried no class.
    pub reason: Option<String>,
    /// Which input won: `probe` | `execution`.
    pub evidence: Option<String>,
    pub evidence_at: Option<Timestamp>,
    /// `fresh` | `stale` | `unknown` — derived from the declared window.
    pub freshness: String,
    /// The probe exists but is OLDER than the failing execution row it lost to.
    pub probe_older: bool,
}

impl BundleStatusView {
    fn unknown() -> Self {
        Self {
            state: "unknown".into(),
            latency_ms: None,
            verdict: None,
            reason: None,
            evidence: None,
            evidence_at: None,
            freshness: "unknown".into(),
            probe_older: false,
        }
    }
    fn not_selected() -> Self {
        let mut s = Self::unknown();
        s.state = "not_selected".into();
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
    /// — read in one pass, no network call.
    pub fn model_snapshot(&self) -> ApiResult<ModelSnapshot> {
        let catalog = self.model_catalog()?;
        let active_record = self.store.active_agent()?;
        let now = Utc::now();
        let active = active_record.as_ref().map(|a| {
            let declared = catalog.for_record(
                &a.name,
                &a.provider,
                &a.runtime,
                &a.model,
                &a.reasoning,
            );
            ActiveBundleView {
                id: declared.map(|b| b.id.clone()),
                label: declared.map(|b| b.label.clone()),
                agent: a.name.clone(),
                driver: a.provider.clone(),
                llm_provider: a.runtime.clone(),
                model: a.model.clone(),
                reasoning: a.reasoning.clone(),
                declared: declared.is_some(),
            }
        });
        let policy = active_record
            .as_ref()
            .map(|a| {
                catalog.policy_for(&a.name, &a.provider, &a.runtime, &a.model, &a.reasoning)
            })
            .unwrap_or_default();
        let history = self.model_history_view()?;
        let status = match &active_record {
            None => BundleStatusView::not_selected(),
            Some(a) => self.bundle_status(a, &policy, &history, now)?,
        };
        let declared = catalog
            .bundles
            .iter()
            .map(|b| {
                let is_active = active_record
                    .as_ref()
                    .map(|a| {
                        a.name == b.agent
                            && a.provider == b.driver
                            && a.runtime == b.llm_provider
                            && a.model == b.model
                            && a.reasoning == b.reasoning
                    })
                    .unwrap_or(false);
                ModelBundleView::of(b, is_active)
            })
            .collect();
        let failures = match (&status.state, active_record.as_ref()) {
            (state, Some(active)) if state == "not_working" => {
                let code = status.reason.clone().unwrap_or_else(|| "не доказано".into());
                Some(FailuresBanner {
                    code: code.clone(),
                    text: format!(
                        "связка не работает: {} — переключитесь на живую",
                        human_failure(&code)
                    ),
                    suggestion: self.suggest_live_bundle(&catalog, &history, now, active),
                })
            }
            _ => None,
        };
        let running = self
            .store
            .list_executions(None)?
            .into_iter()
            .filter(|e| e.status.as_str() == "RUNNING")
            .map(|e| RunningBundleRow {
                execution_id: e.id.to_string(),
                task_id: e.task_id.to_string(),
                agent: e.agent_name.clone(),
                started_at: e.started_at,
            })
            .collect();
        Ok(ModelSnapshot {
            snapshot_at: now,
            active,
            status,
            declared,
            history,
            failures,
            running,
        })
    }

    /// `model.probe` (REQ-MS-03): one real call, one event, no state change.
    pub fn model_probe(&self, id: &str) -> ApiResult<ProbeView> {
        let bundle = self.declared_bundle(id)?;
        let record = self.run_probe(&bundle)?;
        Ok(ProbeView {
            bundle_id: bundle.id.clone(),
            verdict: record.outcome.verdict.as_str().to_string(),
            latency_ms: record.outcome.latency_ms,
            exit_code: record.outcome.exit_code,
            output_excerpt: record.outcome.output_excerpt,
            at: record.outcome.at,
        })
    }

    /// `model.switch` (REQ-MS-04): probe the target, decide, and — only on
    /// `ok` — move the record and the marker in ONE transaction with the
    /// event. A dead verdict refuses and writes `refused_probe`; unknown id,
    /// `no_op` and `not_probeable` refuse before any event exists.
    pub fn model_switch(&self, id: &str, reason: &str) -> ApiResult<SwitchResultView> {
        let rate = SwitchReason::parse(reason).ok_or_else(|| {
            ApiError::InvalidInput {
                field: "reason",
                message: format!("`{reason}` is not a switch reason (manual|limits|error)"),
            }
        })?;
        let bundle = self.declared_bundle(id)?;
        let previous = self.store.active_agent()?;
        // `no_op`: the target identity already is the one in force — nothing
        // is probed, nothing is written (REQ-MS-04 AC3).
        if let Some(p) = &previous {
            if p.name == bundle.agent
                && p.provider == bundle.driver
                && p.runtime == bundle.llm_provider
                && p.model == bundle.model
                && p.reasoning == bundle.reasoning
            {
                return Err(ApiError::refused(
                    "no_op",
                    format!(
                        "bundle `{}` is already in force — no probe, no event, no change",
                        bundle.id
                    ),
                ));
            }
        }
        if bundle.driver == "mock" {
            return Err(ApiError::refused(
                "not_probeable",
                format!(
                    "bundle `{}` declares driver `mock` — it has no network leg to probe; nothing was written",
                    bundle.id
                ),
            ));
        }
        let probe = self.run_probe(&bundle)?;
        let verdict = probe.outcome.verdict;
        let from = previous.as_ref().map(record_identity);
        let to = serde_json::json!({
            (json_field::BUNDLE_ID): bundle.id,
            (json_field::DRIVER): bundle.driver,
            (json_field::LLM_PROVIDER): bundle.llm_provider,
            (json_field::MODEL): bundle.model,
            (json_field::REASONING): bundle.reasoning,
        });
        if verdict != ProbeVerdict::Ok {
            // Probe written, record and marker untouched, refusal recorded
            // with the SAME verdict (REQ-MS-04 AC2).
            self.store.insert_event(
                None,
                event_kind::MODEL_SWITCHED,
                self.actor(),
                serde_json::json!({
                    (json_field::BUNDLE_ID): bundle.id,
                    (json_field::BUNDLE): to.clone(),
                    (json_field::FROM): from,
                    (json_field::VERDICT): verdict.as_str(),
                    (json_field::LATENCY_MS): probe.outcome.latency_ms,
                    (json_field::OUTCOME): "refused_probe",
                    (json_field::REASON): rate.as_str(),
                    (json_field::PROBE_EVENT): probe.event_id.to_string(),
                    (json_field::AGENT_CHANGED): previous.as_ref().map(|p| p.name != bundle.agent).unwrap_or(false),
                }),
            )?;
            return Ok(SwitchResultView {
                outcome: "refused_probe".into(),
                bundle_id: bundle.id.clone(),
                verdict: verdict.as_str().into(),
                latency_ms: probe.outcome.latency_ms,
                code: "probe_refused".into(),
                agent_changed: false,
            });
        }
        // ok: ONE transaction — upsert by name, marker move, event, verify.
        self.store.apply_bundle_switch(&devai_store::BundleSwitch {
            agent: bundle.agent.clone(),
            driver: bundle.driver.clone(),
            llm_provider: bundle.llm_provider.clone(),
            model: bundle.model.clone(),
            reasoning: bundle.reasoning.clone(),
            actor: self.actor().to_string(),
            payload: serde_json::json!({
                (json_field::BUNDLE_ID): bundle.id,
                (json_field::BUNDLE): to,
                (json_field::VERDICT): verdict.as_str(),
                (json_field::LATENCY_MS): probe.outcome.latency_ms,
                (json_field::OUTCOME): "applied",
                (json_field::REASON): rate.as_str(),
                (json_field::PROBE_EVENT): probe.event_id.to_string(),
            }),
            now: Utc::now(),
        })?;
        Ok(SwitchResultView {
            outcome: "applied".into(),
            bundle_id: bundle.id.clone(),
            verdict: verdict.as_str().into(),
            latency_ms: probe.outcome.latency_ms,
            code: "applied".into(),
            agent_changed: previous
                .as_ref()
                .map(|p| p.name != bundle.agent)
                .unwrap_or(false),
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
        let outcome = devai_runtime::probe::probe(&request);
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
    /// probe. Running rows are not evidence.
    fn bundle_status(
        &self,
        active: &devai_core::types::AgentInstance,
        policy: &ProbePolicy,
        history: &[ModelEventView],
        now: Timestamp,
    ) -> ApiResult<BundleStatusView> {
        let probe = history
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
            // «Последняя проба» is the newest by TIME, not by insertion
            // order: the history is append-only, and a test/import may add a
            // row out of order (REQ-MS-06: the fresher evidence wins).
            .max_by_key(|e| e.at)
            .cloned();
        let probe_at = probe.as_ref().map(|p| p.at);
        let probe_freshness = match probe_at {
            None => "unknown".to_string(),
            Some(at) => freshness(at, policy.fresh_secs, now),
        };
        let execution = self.store.latest_finished_execution_for_stamp(
            &active.provider,
            &active.runtime,
            &active.model,
            &active.reasoning,
        )?;
        let exec_time = execution.as_ref().and_then(|e| e.finished_at);
        let (use_probe, probe_older) = match (probe_at, exec_time) {
            (Some(p), Some(e)) => (p >= e, p < e),
            (Some(_), None) => (true, false),
            (None, Some(_)) => (false, false),
            (None, None) => (true, false),
        };
        if use_probe {
            if let Some(probe) = probe {
                let verdict = probe
                    .verdict
                    .as_deref()
                    .and_then(ProbeVerdict::parse)
                    .unwrap_or(ProbeVerdict::Unknown);
                let mut view = BundleStatusView {
                    state: "unknown".into(),
                    latency_ms: probe.latency_ms,
                    verdict: Some(verdict.as_str().into()),
                    reason: None,
                    evidence: Some("probe".into()),
                    evidence_at: probe_at,
                    freshness: probe_freshness,
                    probe_older: false,
                };
                // The live/dead decision comes from the VERDICT, never from a
                // re-read of the state string (the vocabulary's single use at
                // this site is the assignment just below).
                let live = verdict == ProbeVerdict::Ok;
                view.state = if live {
                    "live"
                } else if verdict.proves_dead() {
                    view.reason = Some(verdict.as_str().into());
                    "not_working"
                } else {
                    "not_proven"
                }
                .into();
                if !live {
                    view.latency_ms = None;
                }
                return Ok(view);
            }
        }
        if let Some(exec) = execution {
            let mut view = BundleStatusView {
                state: "unknown".into(),
                latency_ms: None,
                verdict: None,
                reason: None,
                evidence: Some("execution".into()),
                evidence_at: exec.finished_at,
                freshness: match exec.finished_at {
                    Some(at) => freshness(at, policy.fresh_secs, now),
                    None => "unknown".into(),
                },
                probe_older,
            };
            if exec.status.as_str() == "COMPLETED" {
                view.state = "live".into();
            } else if exec.status.as_str() == "FAILED" {
                view.state = "not_working".into();
                // ONLY the stored class may name a failure type — never the
                // error text (REQ-MS-06 AC2/AC3).
                view.reason = Some(
                    exec.failure_class
                        .clone()
                        .unwrap_or_else(|| "причина не классифицирована".into()),
                );
            }
            return Ok(view);
        }
        Ok(BundleStatusView {
            freshness: probe_freshness,
            ..BundleStatusView::unknown()
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
            .filter(|b| {
                !(b.agent == active.name
                    && b.driver == active.provider
                    && b.llm_provider == active.runtime
                    && b.model == active.model
                    && b.reasoning == active.reasoning)
            })
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
        latency_ms: p.get(json_field::LATENCY_MS).and_then(|v| v.as_u64()),
        from: p.get(json_field::FROM).cloned().unwrap_or(serde_json::Value::Null),
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

/// The human words of a failure code; unknown codes are shown verbatim
/// instead of being flattened into one sentence.
fn human_failure(code: &str) -> &str {
    match code {
        "no_credits" => "нет кредитов",
        "model_not_found" => "модель не существует",
        "no_key" => "нет ключа провайдера",
        "network" => "сеть недоступна",
        "provider" => "отказ провайдера",
        other => other,
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

