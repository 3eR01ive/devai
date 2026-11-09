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
    ExecutionBundle, Id, ProbeVerdict, SwitchReason, Timestamp, WorkflowEvent, bundle_outcome,
    event_kind, evidence_kind, json_field, new_id, status_state, switch_outcome,
};
use devai_store::{ModelBundle, ModelCatalog, ProbePolicy};
use serde::{Deserialize, Serialize};

use crate::Api;
use crate::error::{ApiError, ApiResult};

/// The store step a switch can die in — named in BOTH the `failed` event and
/// the returned error, so the fact and the answer cannot drift (R7).
const SWITCH_STEP: &str = "apply_bundle_switch";

/// One declared bundle, as the surface shows it.
#[derive(Serialize, Clone, Debug)]
pub struct ModelBundleView {
    pub id: String,
    pub label: String,
    pub agent: String,
    pub driver: String,
    pub llm_provider: String,
    /// The operator word of `llm_provider` (R5 quality return: ONE source —
    /// the window and the console print this field, they do not re-derive it).
    pub llm_provider_text: String,
    pub model: String,
    /// The operator word of `model` («не передаётся» when empty).
    pub model_text: String,
    /// The level of thinking — part of the bundle (the ratified OQ-6 answer).
    pub reasoning: String,
    /// The operator word of `reasoning` («связка неполна» when empty).
    pub reasoning_text: String,
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
            llm_provider_text: devai_core::types::llm_provider_word(&b.llm_provider).to_string(),
            model: b.model.clone(),
            model_text: devai_core::types::model_word(&b.model).to_string(),
            reasoning: b.reasoning.clone(),
            reasoning_text: devai_core::types::reasoning_word(&b.reasoning).to_string(),
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
    /// The operator word of `llm_provider` — served, never re-derived.
    pub llm_provider_text: String,
    pub model: String,
    /// The operator word of `model` — served, never re-derived.
    pub model_text: String,
    /// The level of thinking (the ratified OQ-6 answer). Empty on a record
    /// outside the declared list reads «связка неполна» — not a driver default.
    pub reasoning: String,
    /// The operator word of `reasoning` — served, never re-derived.
    pub reasoning_text: String,
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
            state: status_state::UNKNOWN.into(),
            state_text: devai_core::types::status_word(status_state::UNKNOWN).into(),
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
        s.state = status_state::NOT_SELECTED.into();
        s.state_text = devai_core::types::status_word(status_state::NOT_SELECTED).into();
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
    /// The operator word of `outcome` for a bundle mutation (`создана` /
    /// `изменена` / `удалена` / `отказано`); `None` for a switch row, whose
    /// token the surface prints as stored (REQ-MC-07).
    pub outcome_text: Option<String>,
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
    /// The declared file edition this snapshot was read from (REQ-MC-06
    /// AC6): the form carries it back on save, so a stale form refuses.
    pub file_revision: String,
    /// The closed vocabulary the form may offer (drivers, factory policy).
    pub form: BundleFormView,
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

/// The operator's FORM input for one bundle (task 01a1be5c, REQ-MC-01/02/03).
/// Every field is a NAME the declaration may carry — there is no key, token,
/// credential or base-URL field to submit, and the wire shape is
/// deny-by-default: anything else fails deserialization NAMING the offending
/// key (REQ-MC-09 AC2). `editing` selects the edit rules (the model may stay
/// empty only where it already was); `file_revision` is the file edition the
/// form was opened on (REQ-MC-06 AC6). The probe policy is the SAME nested
/// shape the declaration carries, so the wire form and the file share ONE
/// field catalogue.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct BundleFormInput {
    /// The handle being edited. IGNORED on create: a new handle is derived
    /// from the name once (ASM-9) and never changes afterwards.
    #[serde(default)]
    pub id: String,
    pub label: String,
    pub agent: String,
    pub driver: String,
    #[serde(default)]
    pub llm_provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub reasoning: String,
    /// Required: the form always carries the operator's policy. A missing
    /// section is a wire error, never a silent substitution of factory
    /// values (ASM-5).
    pub probe: ProbePolicy,
    #[serde(default)]
    pub editing: bool,
    #[serde(default)]
    pub file_revision: String,
}

/// What the form needs to be truthful about its OWN vocabulary: the drivers
/// that may be declared (the diagnostic `mock` is absent by construction),
/// the driver that chooses its provider itself, and the factory probe policy
/// the form STARTS from (ASM-5: a starting value, never a substitute for an
/// empty input).
#[derive(Serialize, Clone, Debug)]
pub struct BundleFormView {
    pub drivers: Vec<String>,
    pub self_provider_driver: String,
    pub default_phrase: String,
    pub default_timeout_ms: u64,
    pub default_fresh_secs: u64,
}

/// The answer of one applied bundle mutation.
#[derive(Serialize, Clone, Debug)]
pub struct BundleMutationView {
    /// `created` | `updated` | `deleted`.
    pub outcome: String,
    pub outcome_text: String,
    pub bundle_id: String,
    pub label: String,
    /// The probe the mutation rested on (absent for a deletion).
    pub verdict: Option<String>,
    pub latency_ms: Option<u64>,
    /// The edit moved the agent NAME of the record in force (runs in flight
    /// stay on the old one).
    pub agent_changed: bool,
    /// The file edition written — the next form opens on it.
    pub revision: String,
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
            file_revision: devai_store::catalog_revision(&self.model_config_dir())?,
            form: BundleFormView {
                drivers: devai_store::PROBEABLE_DRIVERS
                    .iter()
                    .map(|d| d.to_string())
                    .collect(),
                self_provider_driver: devai_store::SELF_PROVIDER_DRIVER.to_string(),
                default_phrase: devai_core::types::DEFAULT_PROBE_PHRASE.to_string(),
                default_timeout_ms: devai_core::types::DEFAULT_PROBE_TIMEOUT_MS,
                default_fresh_secs: devai_core::types::DEFAULT_PROBE_FRESH_SECS,
            },
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
            status_state::NOT_WORKING => {}
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
        self.probe_view(&bundle)
    }

    /// `model.bundle_check` (REQ-MC-02): the form's «Проверить» button — one
    /// real call over the CURRENT form values, one event, no file change.
    /// It does NOT reserve the answer for saving: the save probes again.
    pub fn model_bundle_check(&self, input: &BundleFormInput) -> ApiResult<ProbeView> {
        let previous = self.form_previous(input)?;
        let bundle = form_bundle(input, previous.as_ref())?;
        self.probe_view(&bundle)
    }

    /// `model.bundle_create` (REQ-MC-01): validate the form, derive the
    /// handle from the name (ASM-9), refuse a taken handle or a duplicate
    /// identity NAMING both records, PROBE the exact values, and only on a
    /// successful verdict write the file atomically. A refusal is recorded
    /// as an attempt; the file is not touched.
    pub fn model_bundle_create(&self, input: &BundleFormInput) -> ApiResult<BundleMutationView> {
        let catalog = self.model_catalog()?;
        let bundle = form_bundle(input, None)?;
        self.refuse_taken_handle(&catalog, &bundle, &input.file_revision)?;
        self.refuse_duplicate_identity(
            &catalog,
            &bundle,
            event_kind::MODEL_BUNDLE_CREATED,
            &input.file_revision,
        )?;
        let probe = self.probe_or_refuse(
            &bundle,
            event_kind::MODEL_BUNDLE_CREATED,
            &input.file_revision,
        )?;
        let (revision, _) = self.write_bundle(
            &bundle,
            None,
            &input.file_revision,
            event_kind::MODEL_BUNDLE_CREATED,
            &probe,
        )?;
        self.record_mutation(
            event_kind::MODEL_BUNDLE_CREATED,
            &bundle,
            Some(&probe),
            None,
            &revision,
            None,
            bundle_outcome::CREATED,
        )
    }

    /// `model.bundle_update` (REQ-MC-03): the same probe-and-write path, with
    /// the handle fixed. When the edited record is the ONE in force and its
    /// identity changes, the new identity is applied to the marked record
    /// (the marker stays); a name change marks the new record and leaves the
    /// old one unmarked. Runs in flight keep their own stored stamp.
    pub fn model_bundle_update(&self, input: &BundleFormInput) -> ApiResult<BundleMutationView> {
        let catalog = self.model_catalog()?;
        let previous = self.declared_bundle(&input.id)?;
        let bundle = form_bundle(input, Some(&previous))?;
        self.refuse_duplicate_identity(
            &catalog,
            &bundle,
            event_kind::MODEL_BUNDLE_UPDATED,
            &input.file_revision,
        )?;
        let probe = self.probe_or_refuse(
            &bundle,
            event_kind::MODEL_BUNDLE_UPDATED,
            &input.file_revision,
        )?;
        let (revision, retarget) = self.commit_update(input, &previous, &bundle, &probe)?;
        self.record_mutation(
            event_kind::MODEL_BUNDLE_UPDATED,
            &bundle,
            Some(&probe),
            Some(&previous),
            &revision,
            Some(retarget.agent_changed),
            bundle_outcome::UPDATED,
        )
    }

    /// The write + identity steps of an edit, with the retarget decision. The
    /// previous edition is read ONLY when the record in force is involved, so
    /// a failed identity step can put it back byte-for-byte.
    fn commit_update(
        &self,
        input: &BundleFormInput,
        previous: &ModelBundle,
        bundle: &ModelBundle,
        probe: &ProbeRecord,
    ) -> ApiResult<(String, Retarget)> {
        let active = self.store.active_agent()?;
        let retarget = self.retarget_of(active.as_ref(), previous, bundle);
        let (revision, old_text) = self.write_bundle(
            bundle,
            Some(previous),
            &input.file_revision,
            event_kind::MODEL_BUNDLE_UPDATED,
            probe,
        )?;
        if retarget.needed {
            self.apply_retarget(bundle, previous, &old_text, &input.file_revision, probe)?;
        }
        Ok((revision, retarget))
    }

    /// What an edit does to the record in force (REQ-MC-03): the marked
    /// record takes the new identity only when it IS the edited one and the
    /// identity actually changed; `agent_changed` says the NAME moved.
    fn retarget_of(
        &self,
        active: Option<&devai_core::types::AgentInstance>,
        previous: &ModelBundle,
        bundle: &ModelBundle,
    ) -> Retarget {
        let needed = active.is_some_and(|a| same_identity(a, previous))
            && previous.identity() != bundle.identity();
        Retarget {
            needed,
            agent_changed: needed && active.is_some_and(|a| a.name != bundle.agent),
        }
    }

    /// `model.bundle_delete` (REQ-MC-05): remove the declaration only. The
    /// bundle IN FORCE is never removed — the refusal is typed and says
    /// «сначала переключитесь»; agent records are untouched either way.
    pub fn model_bundle_delete(
        &self,
        id: &str,
        file_revision: &str,
    ) -> ApiResult<BundleMutationView> {
        let bundle = self.declared_bundle(id)?;
        let active = self.store.active_agent()?;
        if active.as_ref().is_some_and(|a| same_identity(a, &bundle)) {
            return Err(self.refuse_bundle(
                event_kind::MODEL_BUNDLE_DELETED,
                &bundle,
                "active_bundle",
                format!(
                    "связку «{}» удалить нельзя: она действующая — сначала переключитесь на другую, затем удаляйте",
                    bundle.label
                ),
                None,
                file_revision,
            ));
        }
        let revision = self.write_remove(&bundle, file_revision)?;
        self.record_mutation(
            event_kind::MODEL_BUNDLE_DELETED,
            &bundle,
            None,
            None,
            &revision,
            None,
            bundle_outcome::DELETED,
        )
    }

    /// The handle rule of a create (REQ-MC-04): the derived id is unique.
    fn refuse_taken_handle(
        &self,
        catalog: &ModelCatalog,
        bundle: &ModelBundle,
        revision: &str,
    ) -> ApiResult<()> {
        let Some(other) = catalog.get(&bundle.id) else {
            return Ok(());
        };
        Err(self.refuse_bundle(
            event_kind::MODEL_BUNDLE_CREATED,
            bundle,
            "duplicate_id",
            format!(
                "связку создать нельзя: идентификатор `{}` уже занят записью «{}» — выберите другое имя",
                bundle.id, other.label
            ),
            None,
            revision,
        ))
    }

    /// The probe-before-write step shared by create and edit (REQ-MC-01/03):
    /// one real call; a non-`ok` verdict records the refusal and aborts.
    fn probe_or_refuse(
        &self,
        bundle: &ModelBundle,
        kind: &'static str,
        revision: &str,
    ) -> ApiResult<ProbeRecord> {
        let probe = self.run_probe(bundle)?;
        if probe.outcome.verdict != ProbeVerdict::Ok {
            return Err(self.refuse_bundle(
                kind,
                bundle,
                "probe_refused",
                probe_refusal_text(bundle, &probe),
                Some(&probe),
                revision,
            ));
        }
        Ok(probe)
    }

    /// The ONE write step of a create/edit: upsert against the form's
    /// revision, with a typed failure fact if the write cannot land. The
    /// text it replaced comes back from the SAME read (R6) — an edit that
    /// must roll back does not read the file a second time.
    fn write_bundle(
        &self,
        bundle: &ModelBundle,
        previous: Option<&ModelBundle>,
        revision: &str,
        kind: &'static str,
        probe: &ProbeRecord,
    ) -> ApiResult<(String, String)> {
        let dir = self.model_config_dir();
        let edit = devai_store::CatalogEdit::Upsert {
            bundle,
            must_exist: previous.is_some(),
        };
        devai_store::write_catalog_with_previous(&dir, revision, &edit)
            .map_err(|e| self.fail_bundle(kind, bundle, previous, e, Some(probe), revision))
    }

    /// The delete write step (REQ-MC-05): the declaration goes, nothing else.
    fn write_remove(&self, bundle: &ModelBundle, revision: &str) -> ApiResult<String> {
        let dir = self.model_config_dir();
        let edit = devai_store::CatalogEdit::Remove { id: &bundle.id };
        devai_store::write_catalog(&dir, revision, &edit).map_err(|e| {
            self.fail_bundle(
                event_kind::MODEL_BUNDLE_DELETED,
                bundle,
                None,
                e,
                None,
                revision,
            )
        })
    }

    /// Apply the edited identity to the marked record (the marker stays). A
    /// failure AFTER the file was written puts the exact previous edition
    /// back (REQ-MC-06 AC5) and records the failure with both causes.
    fn apply_retarget(
        &self,
        bundle: &ModelBundle,
        previous: &ModelBundle,
        old_text: &str,
        file_revision: &str,
        probe: &ProbeRecord,
    ) -> ApiResult<()> {
        let switch = self.identity_switch(bundle);
        let Err(e) = self.store.apply_bundle_identity(&switch) else {
            return Ok(());
        };
        let cause = self.restore_after_failed_retarget(bundle, old_text, e);
        Err(self.record_failed_bundle(
            event_kind::MODEL_BUNDLE_UPDATED,
            bundle,
            Some(previous),
            "write_failed",
            Some(probe),
            file_revision,
            cause,
        ))
    }

    /// The store's identity-move request for an edited bundle in force.
    fn identity_switch(&self, bundle: &ModelBundle) -> devai_store::BundleSwitch {
        devai_store::BundleSwitch {
            agent: bundle.agent.clone(),
            driver: bundle.driver.clone(),
            llm_provider: bundle.llm_provider.clone(),
            model: bundle.model.clone(),
            reasoning: bundle.reasoning.clone(),
            actor: self.actor().to_string(),
            payload: serde_json::Value::Null,
            now: Utc::now(),
        }
    }

    /// Put the exact previous edition back after a failed identity step, and
    /// say BOTH causes — the file must never be newer than the record.
    fn restore_after_failed_retarget(
        &self,
        bundle: &ModelBundle,
        old_text: &str,
        e: devai_store::StoreError,
    ) -> ApiError {
        match devai_store::restore_catalog_text(&self.model_config_dir(), old_text) {
            Ok(()) => ApiError::at_step(BUNDLE_RECORD_STEP, e),
            Err(back) => ApiError::refused(
                "write_failed",
                format!(
                    "bundle `{}` edit failed at `{BUNDLE_RECORD_STEP}`: {e}; restoring the file also failed: {back}",
                    bundle.id
                ),
            ),
        }
    }

    /// The applied mutation's fact and view (REQ-MC-07): ONE event with the
    /// identity, the probe reference and the edition it wrote.
    #[allow(clippy::too_many_arguments)]
    fn record_mutation(
        &self,
        kind: &'static str,
        bundle: &ModelBundle,
        probe: Option<&ProbeRecord>,
        previous: Option<&ModelBundle>,
        revision: &str,
        agent_changed: Option<bool>,
        outcome: &str,
    ) -> ApiResult<BundleMutationView> {
        let payload = bundle_payload(bundle, outcome, probe, previous, revision, agent_changed);
        self.store
            .insert_event(None, kind, self.actor(), payload)
            .map_err(|e| ApiError::at_step(BUNDLE_RECORD_STEP, e))?;
        Ok(mutation_view(
            outcome,
            bundle,
            probe,
            agent_changed.unwrap_or(false),
            revision.to_string(),
        ))
    }

    /// The record a form is EDITING, if any (REQ-MC-02 AC1: a create form
    /// needs no declared record; an edit form is addressed by its handle).
    fn form_previous(&self, input: &BundleFormInput) -> ApiResult<Option<ModelBundle>> {
        if input.editing {
            Ok(Some(self.declared_bundle(&input.id)?))
        } else {
            Ok(None)
        }
    }

    /// One probe + its stored fact, as the surface returns it (REQ-MC-02:
    /// the draft probe and the declared-bundle probe share ONE path).
    fn probe_view(&self, bundle: &ModelBundle) -> ApiResult<ProbeView> {
        let record = self.run_probe(bundle)?;
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

    /// Refuse a create/edit whose identity another record already declares,
    /// naming BOTH records (REQ-MC-04). The attempt is recorded.
    fn refuse_duplicate_identity(
        &self,
        catalog: &ModelCatalog,
        bundle: &ModelBundle,
        kind: &'static str,
        revision: &str,
    ) -> ApiResult<()> {
        let Some(other) = catalog
            .bundles
            .iter()
            .find(|b| b.id != bundle.id && b.identity() == bundle.identity())
        else {
            return Ok(());
        };
        Err(self.refuse_bundle(
            kind,
            bundle,
            "duplicate_identity",
            format!(
                "сохранить нельзя: «{}» и «{}» объявляют одну и ту же связку ({}) — одна идентичность, одна запись",
                other.label,
                bundle.label,
                bundle.identity_text()
            ),
            None,
            revision,
        ))
    }

    /// A refused attempt: one history fact (`refused` + the reason token) and
    /// the typed error. The file is not touched. A failed fact write is NOT
    /// swallowed — it joins the answer (R3).
    fn refuse_bundle(
        &self,
        kind: &'static str,
        bundle: &ModelBundle,
        code: &'static str,
        message: String,
        probe: Option<&ProbeRecord>,
        revision: &str,
    ) -> ApiError {
        let mut payload =
            bundle_payload(bundle, bundle_outcome::REFUSED, probe, None, revision, None);
        if let serde_json::Value::Object(map) = &mut payload {
            map.insert(json_field::REASON.into(), serde_json::json!(code));
        }
        if let Err(e) = self.store.insert_event(None, kind, self.actor(), payload) {
            return ApiError::at_step(BUNDLE_RECORD_STEP, e);
        }
        ApiError::refused(code, message)
    }

    /// A write that failed: the file is unchanged (the store writer refuses
    /// before replacing anything), the failed fact is recorded, and the cause
    /// keeps its typed code (R7).
    fn fail_bundle(
        &self,
        kind: &'static str,
        bundle: &ModelBundle,
        previous: Option<&ModelBundle>,
        e: devai_store::StoreError,
        probe: Option<&ProbeRecord>,
        revision: &str,
    ) -> ApiError {
        let (code, message) = match &e {
            devai_store::StoreError::Stale { .. } => ("stale_form", e.to_string()),
            _ => (crate::error::store_code(&e).0, e.to_string()),
        };
        self.record_failed_bundle(
            kind,
            bundle,
            previous,
            code,
            probe,
            revision,
            ApiError::refused(code, message),
        )
    }

    /// A failed write/fact: the payload says `failed` with its reason, and a
    /// failure to RECORD even that is never swallowed — it joins the answer
    /// beside the original cause (R3, same rule as `record_failed_switch`).
    fn record_failed_bundle(
        &self,
        kind: &'static str,
        bundle: &ModelBundle,
        previous: Option<&ModelBundle>,
        code: &'static str,
        probe: Option<&ProbeRecord>,
        revision: &str,
        cause: ApiError,
    ) -> ApiError {
        let mut payload = bundle_payload(
            bundle,
            bundle_outcome::FAILED,
            probe,
            previous,
            revision,
            None,
        );
        if let serde_json::Value::Object(map) = &mut payload {
            map.insert(json_field::REASON.into(), serde_json::json!(code));
        }
        match self.store.insert_event(None, kind, self.actor(), payload) {
            Ok(()) => cause,
            Err(fact) => ApiError::refused(
                code,
                format!("{cause}; the failed attempt could not be recorded either: {fact}"),
            ),
        }
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
        self.store
            .insert_event(
                None,
                event_kind::MODEL_SWITCHED,
                self.actor(),
                switch_payload(a, switch_outcome::REFUSED_PROBE, false, None),
            )
            .map_err(|e| ApiError::at_step("record_refusal", e))?;
        Ok(SwitchResultView {
            outcome: switch_outcome::REFUSED_PROBE.into(),
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
            payload: switch_payload(a, switch_outcome::APPLIED, a.agent_changed, None),
            now: Utc::now(),
        };
        if let Err(e) = self.store.apply_bundle_switch(&switch) {
            return Err(self.record_failed_switch(a, e));
        }
        Ok(SwitchResultView {
            outcome: switch_outcome::APPLIED.into(),
            bundle_id: a.bundle.id.clone(),
            verdict: a.probe.outcome.verdict.as_str().into(),
            latency_ms: a.probe.outcome.latency_ms,
            code: switch_outcome::APPLIED.into(),
            agent_changed: a.agent_changed,
        })
    }

    /// The rolled-back switch: the `failed` fact is written AFTER the rollback
    /// (REQ-MS-05 names the step), and BOTH causes survive to the caller (R7)
    /// — the typed code is the switch's, the text names the step and, if the
    /// fact could not be written either, that failure too. Never swallowed.
    fn record_failed_switch(&self, a: &SwitchAttempt, e: devai_store::StoreError) -> ApiError {
        let code = crate::error::store_code(&e);
        let fact = self.store.insert_event(
            None,
            event_kind::MODEL_SWITCHED,
            self.actor(),
            switch_payload(a, switch_outcome::FAILED, false, Some(SWITCH_STEP)),
        );
        match fact {
            Ok(_) => ApiError::at_step(SWITCH_STEP, e),
            Err(fact) => ApiError::Refused {
                code,
                message: format!(
                    "switch to bundle `{}` failed at `{SWITCH_STEP}`: {e}; the failure fact could not be recorded either: {fact}",
                    a.bundle.id
                ),
            },
        }
    }

    /// Run one probe and store its fact event — exactly one event per call
    /// (REQ-MS-03 AC4). A failure to record the fact names the failing step:
    /// an answer without its stored proof would be a lie (R7).
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
        self.store
            .insert_event_with(
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
            )
            .map_err(|e| ApiError::at_step("record_probe", e))?;
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
    /// already in force (switching to it would be a `no_op`) — whose newest
    /// SUCCESSFUL probe is still FRESH. Only `ok` rows are candidates here
    /// (a later failing probe is not consulted by THIS offers-only-live
    /// rule; the switch into the suggested bundle probes it again anyway).
    /// None when nothing qualifies; the platform never applies a switch by
    /// itself.
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
                    // The NEWEST of that bundle's successful probes, by time
                    // (the history is append-only and may be imported out of
                    // order) — and that newest success must still be fresh.
                    .max_by_key(|e| e.at)
                    .map(|e| freshness(e.at, b.probe.fresh_secs, now) == "fresh")
                    .unwrap_or(false)
            })
            .map(|b| b.id.clone())
            .next()
    }
}

/// The store step a bundle mutation can die in, named in the `failed` fact
/// and in the caller's error (R7: the cause is never lost).
const BUNDLE_RECORD_STEP: &str = "record_bundle_mutation";

fn invalid(field: &'static str, message: impl Into<String>) -> ApiError {
    ApiError::InvalidInput {
        field,
        message: message.into(),
    }
}

/// Trimmed form values, validated by responsibility — the record is built
/// from THIS, so every refusal names the field it came from (REQ-MC-08).
struct FormFields {
    label: String,
    agent: String,
    driver: String,
    llm_provider: String,
    model: String,
    reasoning: String,
    probe: ProbePolicy,
}

/// The record the form describes, or a refusal NAMING the field that cannot
/// be accepted (REQ-MC-08). `previous` is `Some` for an edit: an existing
/// empty model may stay empty (ASM-3), a declared model may not be cleared,
/// and a CREATED bundle never lacks a model or a level. Values are trimmed —
/// the file carries what the operator sees in the form.
fn form_bundle(input: &BundleFormInput, previous: Option<&ModelBundle>) -> ApiResult<ModelBundle> {
    let f = form_fields(input)?;
    check_model_field(&f.model, previous)?;
    Ok(ModelBundle {
        id: form_id(previous, &f.label)?,
        label: f.label,
        agent: f.agent,
        driver: f.driver,
        llm_provider: f.llm_provider,
        model: f.model,
        reasoning: f.reasoning,
        probe: f.probe,
    })
}

fn form_fields(input: &BundleFormInput) -> ApiResult<FormFields> {
    let f = FormFields {
        label: input.label.trim().to_string(),
        agent: input.agent.trim().to_string(),
        driver: input.driver.trim().to_string(),
        llm_provider: input.llm_provider.trim().to_string(),
        model: input.model.trim().to_string(),
        reasoning: input.reasoning.trim().to_string(),
        probe: ProbePolicy {
            phrase: input.probe.phrase.trim().to_string(),
            timeout_ms: input.probe.timeout_ms,
            fresh_secs: input.probe.fresh_secs,
        },
    };
    check_identity_fields(&f)?;
    check_probe_fields(&f.probe)?;
    Ok(f)
}

/// The identity fields: name, agent, driver (the closed list, `mock`
/// excluded) and the provider rule of a self-providing driver.
fn check_identity_fields(f: &FormFields) -> ApiResult<()> {
    if f.label.is_empty() {
        return Err(invalid(
            "label",
            "поле «имя связки» не заполнено — из него выводится идентификатор",
        ));
    }
    if f.agent.is_empty() {
        return Err(invalid(
            "agent",
            "поле «агент» не заполнено — связка закрепляется за записью агента",
        ));
    }
    check_driver_field(&f.driver, &f.llm_provider)?;
    if f.reasoning.is_empty() {
        return Err(invalid(
            "reasoning",
            "поле «уровень размышления» не заполнено — уровень — часть связки, без него прогон не начинается",
        ));
    }
    Ok(())
}

/// The driver is one of the closed list (`mock` is never a bundle), and the
/// driver that picks its own provider must leave the field empty.
fn check_driver_field(driver: &str, llm_provider: &str) -> ApiResult<()> {
    let drivers = devai_store::PROBEABLE_DRIVERS.join(", ");
    if driver.is_empty() {
        return Err(invalid(
            "driver",
            format!("поле «драйвер» не заполнено — допустимые драйверы: {drivers}"),
        ));
    }
    if driver == "mock" {
        return Err(invalid(
            "driver",
            "диагностический драйвер `mock` не может быть связкой: у него нет сетевой проверки — допустимые драйверы: claude, hermes",
        ));
    }
    if !devai_store::PROBEABLE_DRIVERS.contains(&driver) {
        return Err(invalid(
            "driver",
            format!("недопустимый драйвер `{driver}` — допустимые: {drivers}"),
        ));
    }
    if driver == devai_store::SELF_PROVIDER_DRIVER && !llm_provider.is_empty() {
        return Err(invalid(
            "llm_provider",
            format!(
                "драйвер `{driver}` сам выбирает LLM-провайдера — поле провайдера должно быть пустым"
            ),
        ));
    }
    Ok(())
}

/// The model rule (ASM-3): required on create, may stay empty only where it
/// already was empty, never cleared once declared.
fn check_model_field(model: &str, previous: Option<&ModelBundle>) -> ApiResult<()> {
    match previous {
        None if model.is_empty() => Err(invalid(
            "model",
            "поле «модель» не заполнено — связка без модели не создаётся: выберите модель провайдера",
        )),
        Some(prev) if !prev.model.trim().is_empty() && model.is_empty() => Err(invalid(
            "model",
            "поле «модель» нельзя очистить у связки, где модель объявлена — укажите другую модель",
        )),
        _ => Ok(()),
    }
}

/// The probe policy is a REAL call: an empty phrase or a zero budget is
/// refused before any process starts (ASM-5: no on-the-fly substitution).
fn check_probe_fields(probe: &ProbePolicy) -> ApiResult<()> {
    if probe.phrase.is_empty() {
        return Err(invalid(
            "probe.phrase",
            "фраза пробы пуста — проба делает один короткий вызов и не может быть пустой",
        ));
    }
    if probe.timeout_ms == 0 {
        return Err(invalid(
            "probe.timeout_ms",
            "таймаут пробы должен быть больше нуля",
        ));
    }
    if probe.fresh_secs == 0 {
        return Err(invalid(
            "probe.fresh_secs",
            "срок свежести пробы должен быть больше нуля",
        ));
    }
    Ok(())
}

/// The handle (ASM-9): fixed on an edit, derived ONCE from the name on a
/// create; a name with no usable latin characters refuses.
fn form_id(previous: Option<&ModelBundle>, label: &str) -> ApiResult<String> {
    if let Some(prev) = previous {
        return Ok(prev.id.clone());
    }
    let id = devai_store::bundle_id_from_label(label);
    if id.is_empty() {
        return Err(invalid(
            "label",
            "из имени не выводится идентификатор: нужны латинские буквы или цифры — выберите другое имя",
        ));
    }
    Ok(id)
}

fn probe_refusal_text(bundle: &ModelBundle, probe: &ProbeRecord) -> String {
    format!(
        "связку «{}» сохранить нельзя: проба {} ({}) — файл не изменён",
        bundle.label,
        probe.outcome.verdict.as_str(),
        devai_core::types::verdict_word(probe.outcome.verdict.as_str())
    )
}

/// A bundle identity snapshot for the history (all five fields; the handle
/// stays the event's separate `bundle_id`).
fn bundle_identity(b: &ModelBundle) -> serde_json::Value {
    serde_json::json!({
        (json_field::AGENT): b.agent,
        (json_field::DRIVER): b.driver,
        (json_field::LLM_PROVIDER): b.llm_provider,
        (json_field::MODEL): b.model,
        (json_field::REASONING): b.reasoning,
    })
}

/// The complete bundle-mutation payload in ONE place (REQ-MC-07): identity,
/// `from` on an edit, outcome, the file revision, and — when a probe stood
/// behind it — the verdict, latency and the probe event's id.
fn bundle_payload(
    b: &ModelBundle,
    outcome: &str,
    probe: Option<&ProbeRecord>,
    from: Option<&ModelBundle>,
    revision: &str,
    agent_changed: Option<bool>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        (json_field::BUNDLE_ID): b.id,
        (json_field::BUNDLE): bundle_identity(b),
        (json_field::FROM): from.map(bundle_identity).unwrap_or(serde_json::Value::Null),
        (json_field::OUTCOME): outcome,
        (json_field::REVISION): revision,
    });
    let serde_json::Value::Object(map) = &mut payload else {
        return payload;
    };
    if let Some(p) = probe {
        map.insert(
            json_field::VERDICT.into(),
            serde_json::json!(p.outcome.verdict.as_str()),
        );
        map.insert(
            json_field::LATENCY_MS.into(),
            serde_json::json!(p.outcome.latency_ms),
        );
        map.insert(
            json_field::PROBE_EVENT.into(),
            serde_json::json!(p.event_id.to_string()),
        );
    }
    if let Some(changed) = agent_changed {
        map.insert(json_field::AGENT_CHANGED.into(), serde_json::json!(changed));
    }
    payload
}

fn mutation_view(
    outcome: &str,
    b: &ModelBundle,
    probe: Option<&ProbeRecord>,
    agent_changed: bool,
    revision: String,
) -> BundleMutationView {
    BundleMutationView {
        outcome: outcome.to_string(),
        outcome_text: devai_core::types::bundle_outcome_word(outcome).to_string(),
        bundle_id: b.id.clone(),
        label: b.label.clone(),
        verdict: probe.map(|p| p.outcome.verdict.as_str().to_string()),
        latency_ms: probe.map(|p| p.outcome.latency_ms),
        agent_changed,
        revision,
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
    let state = if live {
        status_state::LIVE
    } else if verdict.proves_dead() {
        status_state::NOT_WORKING
    } else {
        status_state::NOT_PROVEN
    };
    let reason = (state == status_state::NOT_WORKING).then(|| verdict.as_str().to_string());
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
        evidence: Some(evidence_kind::PROBE.into()),
        evidence_at: Some(probe.at),
        freshness_text: devai_core::types::freshness_word(&fresh, Some(evidence_kind::PROBE))
            .into(),
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
        "COMPLETED" => (status_state::LIVE, None),
        "FAILED" => (
            status_state::NOT_WORKING,
            Some(
                exec.failure_class
                    .clone()
                    .unwrap_or_else(|| "причина не классифицирована".into()),
            ),
        ),
        _ => (status_state::UNKNOWN, None),
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
        evidence: Some(evidence_kind::EXECUTION.into()),
        evidence_at: exec.finished_at,
        freshness_text: devai_core::types::freshness_word(&fresh, Some(evidence_kind::EXECUTION))
            .into(),
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

/// What an edit does to the record in force (REQ-MC-03): whether the marked
/// record takes the new identity, and whether the agent NAME moves with it.
struct Retarget {
    needed: bool,
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
            llm_provider_text: devai_core::types::llm_provider_word(&a.runtime).to_string(),
            model: a.model.clone(),
            model_text: devai_core::types::model_word(&a.model).to_string(),
            reasoning: a.reasoning.clone(),
            reasoning_text: devai_core::types::reasoning_word(&a.reasoning).to_string(),
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
    let bundle_event = is_bundle_event(&e.kind);
    let reason = text_field(p, json_field::REASON);
    ModelEventView {
        at: e.created_at,
        kind: e.kind.clone(),
        actor: e.actor.clone(),
        bundle_id: text_field(p, json_field::BUNDLE_ID),
        verdict: text_field(p, json_field::VERDICT),
        outcome: text_field(p, json_field::OUTCOME),
        outcome_text: text_field(p, json_field::OUTCOME)
            .as_deref()
            .filter(|_| bundle_event)
            .map(devai_core::types::bundle_outcome_word)
            .map(str::to_string),
        reason_text: reason.as_deref().map(|r| reason_word(r, bundle_event)),
        reason,
        latency_ms: p.get(json_field::LATENCY_MS).and_then(|v| v.as_u64()),
        from: p
            .get(json_field::FROM)
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        to: p
            .get(json_field::BUNDLE)
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        agent_changed: p
            .get(json_field::AGENT_CHANGED)
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
}

fn text_field(payload: &serde_json::Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// A create/edit/delete row carries its OWN reason vocabulary (REQ-MC-04/05/
/// 07); a switch reason stays the human-chosen one.
fn reason_word(reason: &str, bundle_event: bool) -> String {
    if bundle_event {
        devai_core::types::bundle_reason_word(reason)
    } else {
        devai_core::types::failure_word(reason)
    }
    .to_string()
}

/// The create/edit/delete rows carry their OWN reason vocabulary
/// (REQ-MC-04/05/07); a switch reason stays the human-chosen one.
fn is_bundle_event(kind: &str) -> bool {
    matches!(
        kind,
        event_kind::MODEL_BUNDLE_CREATED
            | event_kind::MODEL_BUNDLE_UPDATED
            | event_kind::MODEL_BUNDLE_DELETED
    )
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

/// The declared bundle a probe/switch id resolved to, for the CLI's
/// rendering parity. Kept public so the console shows the same words.
pub fn bundle_identity_text(b: &ModelBundle) -> String {
    format!(
        "driver `{}` · LLM-провайдер `{}` · модель `{}` · уровень размышления `{}` · агент `{}`",
        b.driver,
        devai_core::types::llm_provider_word(&b.llm_provider),
        devai_core::types::model_word(&b.model),
        devai_core::types::reasoning_word(&b.reasoning),
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
            devai_core::types::llm_provider_word(&b.llm_provider),
            devai_core::types::model_word(&b.model),
            devai_core::types::reasoning_word(&b.reasoning),
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
        let failed = switch_payload(&a, switch_outcome::FAILED, false, Some(SWITCH_STEP));
        assert_eq!(failed[json_field::OUTCOME], switch_outcome::FAILED);
        assert_eq!(failed[json_field::FAILURE_STEP], SWITCH_STEP);
        assert_eq!(failed[json_field::AGENT_CHANGED], false);
        assert_eq!(failed[json_field::VERDICT], "ok");

        let applied = switch_payload(&a, switch_outcome::APPLIED, true, None);
        assert!(applied.get(json_field::FAILURE_STEP).is_none());
        assert_eq!(applied[json_field::AGENT_CHANGED], true);

        let refused = switch_payload(&a, switch_outcome::REFUSED_PROBE, false, None);
        assert!(refused.get(json_field::FAILURE_STEP).is_none());
        assert_eq!(refused[json_field::AGENT_CHANGED], false);
    }

    /// The event-count contract of REQ-MS-05 AC4: every outcome carries the
    /// same keys; only `failed` adds the step. The outcome set is the CORE
    /// dictionary — an outcome outside it is a vocabulary break (R2).
    #[test]
    fn every_outcome_carries_the_same_identity_keys() {
        let a = attempt();
        for outcome in switch_outcome::ALL {
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

    /// R8 quality return: the freshness window has exactly three outcomes,
    /// and a clock skew FORWARD reads «seen just now», never «from the
    /// future» (the status must not go dark because of it).
    #[test]
    fn freshness_covers_fresh_stale_and_the_clock_skew() {
        let now = Utc::now();
        assert_eq!(freshness(now + Duration::seconds(60), 900, now), "fresh");
        assert_eq!(freshness(now - Duration::seconds(900), 900, now), "fresh");
        assert_eq!(freshness(now - Duration::seconds(901), 900, now), "stale");
    }

    /// R8 quality return: the runtime guard refuses a `mock` bundle BEFORE any
    /// process or event (the loader rejects it too — this is the hand-built
    /// catalogue path).
    #[test]
    fn a_mock_bundle_is_refused_as_not_probeable() {
        let mock = ModelBundle {
            id: "mock".into(),
            label: "Mock".into(),
            agent: "mock".into(),
            driver: "mock".into(),
            llm_provider: String::new(),
            model: String::new(),
            reasoning: "high".into(),
            probe: ProbePolicy::default(),
        };
        let err = refuse_unprobeable(&mock).unwrap_err();
        assert_eq!(err.code(), crate::ErrorCode("not_probeable"));
        assert!(err.to_string().contains("no network leg"), "{err}");

        let mut real = mock.clone();
        real.driver = "hermes".into();
        assert!(refuse_unprobeable(&real).is_ok());
    }

    /// R5 quality return: the surface SERVES the empty-field words from the
    /// core dictionary instead of each consumer re-deriving them. Both views
    /// (declared row and active record) must carry the same words.
    #[test]
    fn the_views_serve_the_core_words_for_empty_fields() {
        let bundle = ModelBundle {
            id: "claude-default".into(),
            label: "Claude".into(),
            agent: "claude".into(),
            driver: "claude".into(),
            llm_provider: String::new(),
            model: String::new(),
            reasoning: "medium".into(),
            probe: ProbePolicy::default(),
        };
        let view = ModelBundleView::of(&bundle, false);
        assert_eq!(view.llm_provider_text, "не задан — выбирает драйвер");
        assert_eq!(view.model_text, "не передаётся");
        assert_eq!(view.reasoning_text, "medium");

        // A record outside the declared list (empty level, `local` runtime)
        // is worded the same way — the window prints these fields verbatim.
        let record = devai_core::types::AgentInstance {
            id: new_id(),
            name: "manual".into(),
            provider: "hermes".into(),
            model: String::new(),
            runtime: "local".into(),
            reasoning: String::new(),
            created_at: Utc::now(),
            active: true,
        };
        let active = ActiveBundleView::of(&ModelCatalog::default(), &record);
        assert_eq!(active.llm_provider_text, "не задан — выбирает драйвер");
        assert_eq!(active.model_text, "не передаётся");
        assert_eq!(active.reasoning_text, "не задан — связка неполна");
        assert!(!active.declared);
    }
}
