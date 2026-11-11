//! The task card's role/state graph (REQ-RG-01..06) — a PURE fold of stored
//! facts into the whole path the card lays out.
//!
//! The graph is never an "ideal process" drawing: a role node exists only
//! when that role actually executed on the task, a human gate exists because
//! the DECLARATION gives it (a state that declares a human act over a
//! document), and an edge exists only when a stored fact (an artifact, a
//! decision, an event) carries it. Everything here is a fold over slices the
//! caller loaded ONCE — no IO, no second store read per node or edge
//! (REQ-RG-01 AC4). The interface renders this object verbatim; it holds no
//! stage order, no state→node table and no label dictionary (REQ-RG-06).
//!
//! Closed vocabularies (REQ-RG-01 AC2) live in the `pub mod` blocks below —
//! a value outside them is never produced, so a consumer can switch on them
//! safely.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;

use crate::inbox::InboxWaitPolicy;
use crate::process::{Process, ProcessState, process_condition, process_executor};
use crate::types::*;

/// How the task's ONE position stands on the graph.
pub mod position_kind {
    /// The place is named by a node (role or human gate).
    pub const NODE: &str = "node";
    /// The place is named by a stage, and no node «сейчас» exists — the role
    /// of the stage has not executed yet (REQ-RG-03).
    pub const STAGE: &str = "stage";
    /// The task is parked in a custody hold whose return place exists.
    pub const HOLD: &str = "hold";
    /// The task is not on the path at all (draft / cancelled / no return
    /// state recorded).
    pub const OFF_GRAPH: &str = "off_graph";
    pub const ALL: &[&str] = &[NODE, STAGE, HOLD, OFF_GRAPH];
}

/// What the position points at.
pub mod anchor_kind {
    pub const ROLE: &str = "role";
    pub const GATE: &str = "gate";
    pub const STAGE: &str = "stage";
    pub const FINAL: &str = "final";
    pub const ALL: &[&str] = &[ROLE, GATE, STAGE, FINAL];
}

/// Why a task is outside the graph — the closed reason list.
pub mod off_graph_reason {
    pub const DRAFT: &str = "draft";
    pub const CANCELLED: &str = "cancelled";
    pub const NO_RETURN_STATE: &str = "no_return_state";
    pub const ALL: &[&str] = &[DRAFT, CANCELLED, NO_RETURN_STATE];
}

pub mod node_kind {
    pub const ROLE: &str = "role";
    pub const GATE: &str = "gate";
    pub const FINAL: &str = "final";
}

pub mod node_status {
    pub const PASSED: &str = "passed";
    pub const CURRENT: &str = "current";
    pub const EXPECTED: &str = "expected";
}

pub mod stage_status {
    pub const PASSED: &str = "passed";
    pub const CURRENT: &str = "current";
    pub const PENDING: &str = "pending";
}

pub mod edge_kind {
    /// An execution consumed an artifact — producer → consumer.
    pub const TRANSFER: &str = "transfer";
    /// A human signature carried by a stored decision.
    pub const SIGNATURE: &str = "signature";
    /// A refusal/return carried by a stored decision or event.
    pub const REJECTION: &str = "rejection";
    /// A rework return with its round number.
    pub const RETURN: &str = "return";
    /// The human acceptance into the final node.
    pub const ACCEPTANCE: &str = "acceptance";
}

pub mod carrier_kind {
    pub const ARTIFACT: &str = "artifact";
    pub const DECISION: &str = "decision";
    pub const EVENT: &str = "event";
}

/// The human words of the three stage/node statuses — the platform's one
/// wording, never re-spelled by a consumer.
pub fn stage_status_word(status: &str) -> &'static str {
    match status {
        stage_status::PASSED => "пройдено",
        stage_status::CURRENT => "сейчас",
        _ => "ещё не было",
    }
}

pub fn node_status_word(status: &str) -> &'static str {
    match status {
        node_status::PASSED => "пройдено",
        node_status::CURRENT => "сейчас",
        _ => "ожидается",
    }
}

/// The whole path the card receives in ONE read (REQ-RG-01).
#[derive(Serialize, Clone, Debug)]
pub struct RoleGraph {
    /// The declared role/human stages in DECLARED order — the axis. The
    /// interface prints this array in order and holds no order of its own.
    pub stages: Vec<GraphStage>,
    /// Role nodes (only executed roles), human gates (born from the
    /// declaration) and the final node (only for an accepted task).
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub position: GraphPosition,
    pub rework: ReworkLoop,
    /// RUNNING executions; one outside the declared path is reported here
    /// and in `notes`, and never becomes a node/edge (REQ-RG-02).
    pub running: Vec<GraphRunning>,
    /// One row per declared human gate: what it demands, the first declared
    /// command with real ids, whether it is the position and signable, and
    /// what the operator is shown (current document, else the last one).
    pub waits: Vec<GraphWait>,
    /// Open BLOCKING questions of the task — a fact of the question plane,
    /// never derived from the position or a gate (REQ-RG-05).
    pub blocking_questions: usize,
    pub notes: Vec<String>,
    /// The contract SCHEMAS referenced by the nodes (`name@v` -> schema), so
    /// the interface can show the rules a role reads and produces without a
    /// second round-trip. Empty when no role names a contract.
    pub contracts: Vec<GraphContract>,
}

/// One contract schema, verbatim as stored: the rule a role's input or output
/// must satisfy. `key` is `name@v` — the same spelling the node carries.
#[derive(Serialize, Clone, Debug)]
pub struct GraphContract {
    pub key: String,
    pub name: String,
    pub version: u32,
    pub schema: serde_json::Value,
}

#[derive(Serialize, Clone, Debug)]
pub struct GraphStage {
    pub state: String,
    /// Human label: the declared role's name, the gate's declared demand,
    /// else the state token.
    pub label: String,
    /// The declared executor: a role slug or `human`.
    pub executor: String,
    pub gate: bool,
    pub status: String,
    pub status_word: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct GraphNode {
    /// Stable id: `role:<slug>`, `gate:<STATE>` or `final`.
    pub id: String,
    pub kind: String,
    pub state: String,
    pub label: String,
    /// `label · vN` when a version is known — the active list prints the
    /// same string (REQ-NM-01 AC1).
    pub display: String,
    pub role_slug: Option<String>,
    pub role_version: Option<u32>,
    /// The role's declared INPUT rule (`name@v`): what it is allowed to read.
    /// Filled by the caller from the role definition; `None` for gates/final
    /// and for a role whose definition names no contract.
    pub input_contract: Option<String>,
    /// The role's declared OUTPUT rule (`name@v`): what its result must match.
    pub output_contract: Option<String>,
    pub status: String,
    pub status_word: String,
    /// A stored result of this node was retracted (STALE/SUPERSEDED) — shown
    /// as a separate mark; it never makes the node passed (REQ-RG-02 AC7).
    pub revoked: bool,
    pub executions: Vec<GraphExecution>,
    pub artifacts: Vec<GraphArtifact>,
}

#[derive(Serialize, Clone, Debug)]
pub struct GraphExecution {
    pub id: String,
    pub role_slug: String,
    pub role_display: String,
    pub role_version: u32,
    pub agent_name: Option<String>,
    pub status: String,
    pub attempt: u32,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

#[derive(Serialize, Clone, Debug)]
pub struct GraphArtifact {
    pub id: String,
    pub kind: String,
    pub seq: u32,
    pub status: String,
    pub name: String,
    /// `человеческое название вида · vN` — the platform's one label rule
    /// (the titles come from the declarative table loaded by the caller).
    pub display: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    /// 1-based rework round; present only on a return edge.
    pub round: Option<u32>,
    pub carrier: GraphCarrier,
}

#[derive(Serialize, Clone, Debug)]
pub struct GraphCarrier {
    pub kind: String,
    pub id: String,
    pub label: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct GraphPosition {
    pub kind: String,
    pub anchor: String,
    /// The declared stage the position belongs to (return stage for a hold).
    pub stage: Option<String>,
    /// The node the position stands on, when one exists.
    pub node: Option<String>,
    /// `off_graph_reason` value when `kind == off_graph`.
    pub reason: Option<String>,
    pub line: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ReworkLoop {
    /// Number of stored human returns (1..N, no gaps).
    pub rounds: u32,
    /// The task is in the rework stage right now.
    pub active: bool,
    pub note: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct GraphRunning {
    pub execution_id: String,
    pub role_slug: String,
    pub role_display: String,
    pub role_version: u32,
    pub agent_name: Option<String>,
    pub in_path: bool,
    pub note: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct GraphWait {
    pub node: String,
    pub state: String,
    pub requirement: String,
    pub hint: String,
    pub awaiting: bool,
    pub signable: bool,
    pub presentation: Option<GraphPresentation>,
}

#[derive(Serialize, Clone, Debug)]
pub struct GraphPresentation {
    pub artifact_id: String,
    pub display: String,
    pub status: String,
    pub current: bool,
    pub note: Option<String>,
}

/// Everything the fold reads — each slice is loaded ONCE by the caller.
pub struct GraphFacts<'a> {
    pub process: &'a Process,
    pub task: &'a Task,
    pub artifacts: &'a [Artifact],
    pub executions: &'a [RoleExecution],
    pub decisions: &'a [Decision],
    pub events: &'a [WorkflowEvent],
    pub questions: &'a [Question],
    /// `(slug, name)` of the latest stored definition of every role.
    pub roles: &'a [(String, String)],
    /// Artifact-kind token → human title (declarative table; complete by
    /// contract — the loader refuses an incomplete one).
    pub artifact_titles: &'a BTreeMap<String, String>,
    pub wait_policy: &'a InboxWaitPolicy,
}

/// A human gate is a DECLARED state that declares a human act over a
/// document. Starting a task and releasing a hold are not such acts, so the
/// gate count is never hard-coded (REQ-RG-02).
fn is_gate_state(st: &ProcessState) -> bool {
    st.outcomes.iter().any(|o| {
        matches!(
            o.as_str(),
            process_condition::HUMAN_APPROVED
                | process_condition::HUMAN_REJECTED
                | process_condition::HUMAN_ACCEPTED
                | process_condition::HUMAN_RETURNED
        )
    })
}

/// The axis: declared role stages (the rework stage is the separate loop,
/// not a point on the axis) plus the declared human gates.
fn axis_states(process: &Process) -> Vec<&ProcessState> {
    process
        .states
        .iter()
        .filter(|s| {
            (s.executor == process_executor::ROLE && s.state != TaskState::Rework.as_str())
                || is_gate_state(s)
        })
        .collect()
}

fn role_id(slug: &str) -> String {
    format!("role:{slug}")
}

fn gate_id(state: &str) -> String {
    format!("gate:{state}")
}

fn artifact_display(titles: &BTreeMap<String, String>, a: &Artifact) -> String {
    let title = titles
        .get(a.kind.as_str())
        .cloned()
        .unwrap_or_else(|| a.kind.as_str().to_string());
    artifact_label(&title, a.seq)
}

fn role_display(roles: &HashMap<String, String>, slug: &str, version: u32) -> String {
    let name = roles.get(slug).cloned().unwrap_or_else(|| slug.to_string());
    format!("{name} · v{version}")
}

fn role_name(roles: &HashMap<String, String>, slug: &str) -> String {
    roles.get(slug).cloned().unwrap_or_else(|| slug.to_string())
}

/// The fold's shared inputs and derived indexes: every slice was loaded ONCE
/// by the caller, so no part below reads the store again (REQ-RG-01 AC4).
struct Fold<'a> {
    facts: &'a GraphFacts<'a>,
    roles: HashMap<String, String>,
    axis: Vec<&'a ProcessState>,
    exec_by_role: BTreeMap<String, Vec<&'a RoleExecution>>,
    running: Vec<&'a RoleExecution>,
    running_version: HashMap<String, u32>,
}

/// The axis rows plus the slug/state → id maps the edges and the position
/// address them by.
struct NodeSet {
    rows: Vec<GraphNode>,
    role: HashMap<String, String>,
    gate: HashMap<String, String>,
}

impl NodeSet {
    /// The node of the role the DECLARATION assigns to a state, if it exists.
    fn role_for<'s>(&'s self, process: &Process, state: &str) -> Option<&'s String> {
        process.role_for(state).and_then(|slug| self.role.get(slug))
    }
}

/// The graph is laid out by these parts, one basis each: nodes, edges,
/// position, stages, waits, running rows, questions. `build` only assembles
/// their results.
impl<'a> Fold<'a> {
    fn new(f: &'a GraphFacts<'a>) -> Self {
        let mut exec_by_role: BTreeMap<String, Vec<&RoleExecution>> = BTreeMap::new();
        for e in f.executions {
            exec_by_role.entry(e.role_slug.clone()).or_default().push(e);
        }
        for list in exec_by_role.values_mut() {
            list.sort_by_key(|e| (e.started_at, e.id));
        }
        let running: Vec<&RoleExecution> = f
            .executions
            .iter()
            .filter(|e| e.status == ExecutionStatus::Running)
            .collect();
        // The running execution's version wins over the newest completed one.
        let running_version = running
            .iter()
            .map(|e| (e.role_slug.clone(), e.role_version))
            .collect();
        Fold {
            facts: f,
            roles: f.roles.iter().cloned().collect(),
            axis: axis_states(f.process),
            exec_by_role,
            running,
            running_version,
        }
    }

    /// The version a node's signboard prints: the running one, else the
    /// freshest stored run.
    fn role_version(&self, slug: &str) -> u32 {
        self.running_version
            .get(slug)
            .copied()
            .or_else(|| self.stored_version(slug))
            .unwrap_or(1)
    }

    fn stored_version(&self, slug: &str) -> Option<u32> {
        self.exec_by_role
            .get(slug)
            .and_then(|list| list.last().map(|e| e.role_version))
    }

    /// Nodes in declaration order, then the final of an accepted task. A role
    /// outside the declaration NEVER gets a node: its run is a note and a row
    /// in `running`, not a place on the path (REQ-RG-02 AC3).
    fn nodes(&self) -> NodeSet {
        let mut ns = NodeSet {
            rows: Vec::new(),
            role: HashMap::new(),
            gate: HashMap::new(),
        };
        for st in &self.axis {
            if is_gate_state(st) && st.executor != process_executor::ROLE {
                self.push_gate(&mut ns, st);
                continue;
            }
            let Some(slug) = st.role.as_deref() else {
                continue;
            };
            if self.exec_by_role.contains_key(slug) {
                self.push_role(&mut ns, st, slug);
            }
            // QA today: a role stage that also declares the human act, so it
            // carries BOTH a role node and a gate node.
            if is_gate_state(st) {
                self.push_gate(&mut ns, st);
            }
        }
        ns.rows.extend(final_node(self.facts.task));
        ns
    }

    /// One role node: it exists because the role EXECUTED on this task.
    fn push_role(&self, ns: &mut NodeSet, st: &ProcessState, slug: &str) {
        let version = self.role_version(slug);
        let id = role_id(slug);
        let (artifacts, revoked) = self.node_artifacts(slug);
        let status = if artifacts
            .iter()
            .any(|a| a.status == ArtifactStatus::Current.as_str())
        {
            node_status::PASSED
        } else {
            node_status::EXPECTED
        };
        let executions = self
            .exec_by_role
            .get(slug)
            .map(|list| execution_rows(&self.roles, list))
            .unwrap_or_default();
        ns.role.insert(slug.to_string(), id.clone());
        ns.rows.push(GraphNode {
            id,
            kind: node_kind::ROLE.to_string(),
            state: st.state.clone(),
            label: role_name(&self.roles, slug),
            display: role_display(&self.roles, slug, version),
            role_slug: Some(slug.to_string()),
            role_version: Some(version),
            input_contract: None,
            output_contract: None,
            status: status.to_string(),
            status_word: node_status_word(status).to_string(),
            revoked,
            executions,
            artifacts,
        });
    }

    /// One human gate: it exists because the DECLARATION declares a human act
    /// over a document here — the gate count is never hard-coded (REQ-RG-02).
    fn push_gate(&self, ns: &mut NodeSet, st: &ProcessState) {
        let id = gate_id(&st.state);
        let (label, status) = gate_facts(self.facts, &st.state, &id, node_status::EXPECTED);
        let word = node_status_word(status.as_str()).to_string();
        ns.gate.insert(st.state.clone(), id.clone());
        ns.rows.push(GraphNode {
            id,
            kind: node_kind::GATE.to_string(),
            state: st.state.clone(),
            label: label.clone(),
            display: label,
            role_slug: None,
            role_version: None,
            input_contract: None,
            output_contract: None,
            status,
            status_word: word,
            revoked: false,
            executions: Vec::new(),
            artifacts: Vec::new(),
        });
    }

    /// The ONE position of the task: `(kind, anchor, stage, node)` with a line
    /// a human reads, from the state and the stored facts (REQ-RG-03).
    fn position(&self, ns: &NodeSet) -> GraphPosition {
        let labels: HashMap<String, String> = ns
            .rows
            .iter()
            .map(|n| (n.id.clone(), n.display.clone()))
            .collect();
        position_of(self.facts, &self.axis, ns, &labels, &self.roles)
    }

    /// Every edge carries ONE stored fact and connects two EXISTING nodes
    /// (REQ-RG-04). Facts whose end is not on the graph leave a note.
    fn edges(&self, ns: &NodeSet, returns: &[&WorkflowEvent]) -> (Vec<GraphEdge>, Vec<String>) {
        let idx = EdgeIndex {
            exec_role: self
                .facts
                .executions
                .iter()
                .map(|e| (e.id, e.role_slug.as_str()))
                .collect(),
            artifacts: self.facts.artifacts.iter().map(|a| (a.id, a)).collect(),
        };
        let mut edges: Vec<GraphEdge> = Vec::new();
        let mut seen: SeenEdges = HashSet::new();
        let mut notes: Vec<String> = Vec::new();
        self.consumption_edges(ns, &idx, &mut edges, &mut seen);
        self.verdict_edges(ns, &mut edges, &mut seen, &mut notes);
        self.report_edges(ns, &idx, &mut edges, &mut seen);
        self.human_edges(ns, &mut edges, &mut seen);
        self.return_edges(ns, returns, &mut edges, &mut seen);
        self.acceptance_edges(ns, &mut edges, &mut seen, &mut notes);
        (edges, notes)
    }

    /// Consumption: an execution names an artifact → producer → consumer.
    fn consumption_edges(
        &self,
        ns: &NodeSet,
        idx: &EdgeIndex<'_>,
        edges: &mut Vec<GraphEdge>,
        seen: &mut SeenEdges,
    ) {
        for e in self.facts.executions {
            let Some(to) = ns.role.get(&e.role_slug) else {
                continue;
            };
            for input in &e.inputs {
                let Some((from, carrier)) = self.consumed_fact(ns, idx, input) else {
                    continue;
                };
                if &from == to {
                    continue;
                }
                push_edge(
                    edges,
                    seen,
                    GraphEdge {
                        from,
                        to: to.clone(),
                        kind: edge_kind::TRANSFER.to_string(),
                        round: None,
                        carrier,
                    },
                );
            }
        }
    }

    /// The producer node and the artifact a consumption edge carries; `None`
    /// while an end of the edge does not exist.
    fn consumed_fact(
        &self,
        ns: &NodeSet,
        idx: &EdgeIndex<'_>,
        input: &str,
    ) -> Option<(String, GraphCarrier)> {
        let id = input.parse::<Id>().ok()?;
        let a = idx.artifacts.get(&id)?;
        let slug = idx.exec_role.get(&a.execution_id?)?;
        let from = ns.role.get(*slug)?.clone();
        Some((from, artifact_carrier(self.facts.artifact_titles, a)))
    }

    /// Reviewer verdicts: «принято» to the approval gate, any other verdict
    /// back to the analyst. A verdict without its nodes is a NOTE.
    fn verdict_edges(
        &self,
        ns: &NodeSet,
        edges: &mut Vec<GraphEdge>,
        seen: &mut SeenEdges,
        notes: &mut Vec<String>,
    ) {
        let reviewer = ns.role_for(self.facts.process, TaskState::Review.as_str());
        let analyst = ns.role_for(self.facts.process, TaskState::Analysis.as_str());
        for ev in self
            .facts
            .events
            .iter()
            .filter(|e| e.kind == event_kind::REVIEW_RECORDED)
        {
            let approved = verdict_of(ev) == Verdict::Approved.as_str();
            let to = if approved {
                ns.gate.get(TaskState::Approval.as_str())
            } else {
                analyst
            };
            match (reviewer, to) {
                (Some(from), Some(to)) => {
                    push_edge(edges, seen, verdict_edge(from, to, ev, approved))
                }
                _ => notes.push(
                    "вердикт проверки без узла на графе — ребро не рисуется, факт остаётся в истории"
                        .to_string(),
                ),
            }
        }
    }

    /// Reports (implementation + quality): every publication gives its own
    /// edge from its role node to the quality gate.
    fn report_edges(
        &self,
        ns: &NodeSet,
        idx: &EdgeIndex<'_>,
        edges: &mut Vec<GraphEdge>,
        seen: &mut SeenEdges,
    ) {
        let Some(gate) = ns.gate.get(TaskState::Qa.as_str()) else {
            return;
        };
        let reports = self.facts.artifacts.iter().filter(|a| {
            matches!(
                a.kind,
                ArtifactKind::ImplementationReport | ArtifactKind::QualityReport
            )
        });
        for a in reports {
            let Some(slug) = a.execution_id.and_then(|id| idx.exec_role.get(&id)) else {
                continue;
            };
            let Some(from) = ns.role.get(*slug) else {
                continue;
            };
            push_edge(
                edges,
                seen,
                GraphEdge {
                    from: from.clone(),
                    to: gate.clone(),
                    kind: edge_kind::SIGNATURE.to_string(),
                    round: None,
                    carrier: artifact_carrier(self.facts.artifact_titles, a),
                },
            );
        }
    }

    /// Human acts carried by stored decisions: the requirements signature to
    /// the implementer, a rejection back to the analyst — only when BOTH ends
    /// exist (REQ-RG-04 AC3).
    fn human_edges(&self, ns: &NodeSet, edges: &mut Vec<GraphEdge>, seen: &mut SeenEdges) {
        let gate = ns.gate.get(TaskState::Approval.as_str());
        let implementer = ns.role_for(self.facts.process, TaskState::Implementation.as_str());
        let analyst = ns.role_for(self.facts.process, TaskState::Analysis.as_str());
        for d in self.facts.decisions {
            let (to, kind, label) = match d.kind.as_str() {
                decision_kind::REQUIREMENTS_APPROVAL if d.verdict == Verdict::Approved => {
                    (implementer, edge_kind::SIGNATURE, "подпись требований")
                }
                decision_kind::REQUIREMENTS_REJECTION => {
                    (analyst, edge_kind::REJECTION, "отклонение требований")
                }
                _ => continue,
            };
            if let Some(edge) = decision_edge(gate, to, d, kind, label) {
                push_edge(edges, seen, edge);
            }
        }
    }

    /// Returns: every stored quality return is one edge and one round.
    fn return_edges(
        &self,
        ns: &NodeSet,
        returns: &[&WorkflowEvent],
        edges: &mut Vec<GraphEdge>,
        seen: &mut SeenEdges,
    ) {
        let (Some(gate), Some(implementer)) = (
            ns.gate.get(TaskState::Qa.as_str()),
            ns.role_for(self.facts.process, TaskState::Implementation.as_str()),
        ) else {
            return;
        };
        for (i, ev) in returns.iter().enumerate() {
            let round = i as u32 + 1;
            push_edge(
                edges,
                seen,
                GraphEdge {
                    from: gate.clone(),
                    to: implementer.clone(),
                    kind: edge_kind::RETURN.to_string(),
                    round: Some(round),
                    carrier: GraphCarrier {
                        kind: carrier_kind::EVENT.to_string(),
                        id: ev.id.to_string(),
                        label: format!("возврат на доработку, круг {round}"),
                    },
                },
            );
        }
    }

    /// Acceptance into the final node, plus the data-mismatch note when an
    /// accepted task has no stored acceptance act.
    fn acceptance_edges(
        &self,
        ns: &NodeSet,
        edges: &mut Vec<GraphEdge>,
        seen: &mut SeenEdges,
        notes: &mut Vec<String>,
    ) {
        let accepted = self.facts.task.state == TaskState::Accepted;
        let act = self
            .facts
            .events
            .iter()
            .filter(|e| e.kind == event_kind::TASK_ACCEPTED)
            .next_back();
        if accepted && act.is_none() {
            notes.push(
                "у принятой задачи нет сохранённого акта приёмки — расхождение данных".to_string(),
            );
        }
        let (true, Some(ev), Some(gate)) = (accepted, act, ns.gate.get(TaskState::Qa.as_str()))
        else {
            return;
        };
        push_edge(
            edges,
            seen,
            GraphEdge {
                from: gate.clone(),
                to: "final".to_string(),
                kind: edge_kind::ACCEPTANCE.to_string(),
                round: None,
                carrier: GraphCarrier {
                    kind: carrier_kind::EVENT.to_string(),
                    id: ev.id.to_string(),
                    label: "приёмка".to_string(),
                },
            },
        );
    }

    /// Stages carry statuses positional to the ONE position; the rework stage
    /// is the separate loop, never a point on this axis (REQ-RG-05).
    fn stages(&self, position: &GraphPosition) -> Vec<GraphStage> {
        let idx = stage_index_for_position(&self.axis, position);
        let mut stages: Vec<GraphStage> = self
            .axis
            .iter()
            .enumerate()
            .map(|(i, st)| self.stage_row(st, i, idx))
            .collect();
        let on_rework = self.on_rework();
        // A task in REWORK (or a hold returning there) has passed the whole
        // axis: the loop is drawn separately, not as «ещё одна точка».
        if on_rework {
            for s in &mut stages {
                s.status = stage_status::PASSED.to_string();
                s.status_word = stage_status_word(stage_status::PASSED).to_string();
            }
        }
        // The current STAGE marker exists only when the position names a stage.
        if position.kind == position_kind::OFF_GRAPH || on_rework {
            for s in &mut stages {
                if s.status == stage_status::CURRENT {
                    s.status = stage_status::PENDING.to_string();
                    s.status_word = stage_status_word(stage_status::PENDING).to_string();
                }
            }
        }
        stages
    }

    fn stage_row(&self, st: &ProcessState, i: usize, idx: Option<usize>) -> GraphStage {
        let status = match idx {
            None => stage_status::PENDING,
            Some(idx) if i < idx => stage_status::PASSED,
            Some(idx) if i == idx => stage_status::CURRENT,
            Some(_) => stage_status::PENDING,
        };
        GraphStage {
            state: st.state.clone(),
            label: stage_label(self.facts, st, &self.roles),
            executor: st
                .role
                .clone()
                .unwrap_or_else(|| process_executor::HUMAN.to_string()),
            gate: is_gate_state(st),
            status: status.to_string(),
            status_word: stage_status_word(status).to_string(),
        }
    }

    /// A task in REWORK — or parked with REWORK as its return place — is in
    /// the separate loop, off the axis.
    fn on_rework(&self) -> bool {
        self.facts.task.state == TaskState::Rework
            || (self.facts.task.state.is_hold()
                && self.facts.task.return_state == Some(TaskState::Rework))
    }

    /// One row per DECLARED gate: requirement, hint with real ids, whether it
    /// awaits NOW, whether it is signable NOW, what it presents (REQ-RG-02).
    fn waits(&self, position: &GraphPosition, gates: &HashMap<String, String>) -> Vec<GraphWait> {
        let parked = self.parked_question();
        self.axis
            .iter()
            .filter(|st| gates.contains_key(&st.state))
            .map(|st| self.wait_row(st, &gates[&st.state], position, parked))
            .collect()
    }

    fn wait_row(
        &self,
        st: &ProcessState,
        node: &str,
        position: &GraphPosition,
        parked: Option<&Question>,
    ) -> GraphWait {
        let doc = gate_document(self.facts, &st.state);
        let awaiting = position.node.as_deref() == Some(node);
        let signable = awaiting && doc.as_ref().is_some_and(|(_, current)| *current);
        let presentation = doc.map(|(a, current)| GraphPresentation {
            artifact_id: a.id.to_string(),
            display: artifact_display(self.facts.artifact_titles, a),
            status: a.status.as_str().to_string(),
            current,
            note: (!current).then(|| "последний сохранённый документ, не действующий".to_string()),
        });
        let (requirement, hint) =
            gate_demand(self.facts.wait_policy, self.facts.task, &st.state, parked);
        GraphWait {
            node: node.to_string(),
            state: st.state.clone(),
            requirement,
            hint,
            awaiting,
            signable,
            presentation,
        }
    }

    /// The blocking question a parked gate's hint refers to, if any.
    fn parked_question(&self) -> Option<&'a Question> {
        self.facts
            .questions
            .iter()
            .find(|q| q.status == QuestionStatus::Open && q.blocking)
    }

    /// Running roles: who works NOW, plus the out-of-path mark — a run
    /// outside the declaration is never a node (REQ-RG-02 AC3).
    fn running_rows(&self) -> (Vec<GraphRunning>, Vec<String>) {
        let mut rows: Vec<GraphRunning> =
            self.running.iter().map(|e| self.running_row(e)).collect();
        rows.sort_by(|a, b| a.execution_id.cmp(&b.execution_id));
        let notes = rows
            .iter()
            .filter(|r| !r.in_path)
            .map(|r| format!("прогон вне пути: {}", r.role_display))
            .collect();
        (rows, notes)
    }

    fn running_row(&self, e: &RoleExecution) -> GraphRunning {
        let in_path = self.declares_role(&e.role_slug);
        GraphRunning {
            execution_id: e.id.to_string(),
            role_slug: e.role_slug.clone(),
            role_display: role_display(&self.roles, &e.role_slug, e.role_version),
            role_version: e.role_version,
            agent_name: e.agent_name.clone(),
            in_path,
            note: (!in_path).then(|| "прогон вне пути".to_string()),
        }
    }

    /// A role is on the path when the DECLARATION gives it a role stage.
    fn declares_role(&self, slug: &str) -> bool {
        self.facts
            .process
            .states
            .iter()
            .any(|s| s.executor == process_executor::ROLE && s.role.as_deref() == Some(slug))
    }

    /// Blocking questions are a fact of the question plane, never of a gate or
    /// of the position (REQ-RG-05).
    fn blocking_count(&self) -> usize {
        self.facts
            .questions
            .iter()
            .filter(|q| q.status == QuestionStatus::Open && q.blocking)
            .count()
    }

    /// The stored quality returns — each one is a rework round.
    fn returns(&self) -> Vec<&'a WorkflowEvent> {
        self.facts
            .events
            .iter()
            .filter(|e| e.kind == event_kind::QUALITY_RETURNED)
            .collect()
    }

    fn node_artifacts(&self, slug: &str) -> (Vec<GraphArtifact>, bool) {
        node_artifacts(self.facts, slug, &self.exec_by_role, &self.roles)
    }

    fn rework(&self, rounds: u32) -> ReworkLoop {
        ReworkLoop {
            rounds,
            active: self.on_rework(),
            note: if rounds == 0 {
                "контура доработки не было".to_string()
            } else {
                format!("кругов доработки: {rounds}")
            },
        }
    }
}

/// The final node exists only for an accepted task.
fn final_node(task: &Task) -> Option<GraphNode> {
    (task.state == TaskState::Accepted).then(|| GraphNode {
        id: "final".to_string(),
        kind: node_kind::FINAL.to_string(),
        state: TaskState::Accepted.as_str().to_string(),
        label: "принято".to_string(),
        display: "принято".to_string(),
        role_slug: None,
        role_version: None,
        input_contract: None,
        output_contract: None,
        status: node_status::EXPECTED.to_string(),
        status_word: node_status_word(node_status::EXPECTED).to_string(),
        revoked: false,
        executions: Vec::new(),
        artifacts: Vec::new(),
    })
}

/// The two lookups every carrier edge needs, built once per fold.
struct EdgeIndex<'a> {
    exec_role: HashMap<Id, &'a str>,
    artifacts: HashMap<Id, &'a Artifact>,
}

type SeenEdges = HashSet<(String, String, String, String)>;

/// One execution row as the card prints it (name+version, agent, attempt).
fn execution_rows(roles: &HashMap<String, String>, list: &[&RoleExecution]) -> Vec<GraphExecution> {
    list.iter().map(|e| execution_row(roles, e)).collect()
}

fn execution_row(roles: &HashMap<String, String>, e: &RoleExecution) -> GraphExecution {
    GraphExecution {
        id: e.id.to_string(),
        role_slug: e.role_slug.clone(),
        role_display: role_display(roles, &e.role_slug, e.role_version),
        role_version: e.role_version,
        agent_name: e.agent_name.clone(),
        status: e.status.as_str().to_string(),
        attempt: e.attempt,
        started_at: e.started_at,
        finished_at: e.finished_at,
    }
}

fn artifact_carrier(titles: &BTreeMap<String, String>, a: &Artifact) -> GraphCarrier {
    GraphCarrier {
        kind: carrier_kind::ARTIFACT.to_string(),
        id: a.id.to_string(),
        label: artifact_display(titles, a),
    }
}

fn decision_edge(
    from: Option<&String>,
    to: Option<&String>,
    d: &Decision,
    kind: &str,
    label: &str,
) -> Option<GraphEdge> {
    Some(GraphEdge {
        from: from?.clone(),
        to: to?.clone(),
        kind: kind.to_string(),
        round: None,
        carrier: GraphCarrier {
            kind: carrier_kind::DECISION.to_string(),
            id: d.id.to_string(),
            label: label.to_string(),
        },
    })
}

/// The verdict token a stored review fact carries.
fn verdict_of(ev: &WorkflowEvent) -> &str {
    ev.payload
        .get(json_field::VERDICT)
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// One review verdict edge: «принято» is a signature to the approval gate,
/// any other verdict a rejection to the analyst; the verdict is the label.
fn verdict_edge(from: &String, to: &String, ev: &WorkflowEvent, approved: bool) -> GraphEdge {
    GraphEdge {
        from: from.clone(),
        to: to.clone(),
        kind: if approved {
            edge_kind::SIGNATURE
        } else {
            edge_kind::REJECTION
        }
        .to_string(),
        round: None,
        carrier: GraphCarrier {
            kind: carrier_kind::EVENT.to_string(),
            id: ev.id.to_string(),
            label: format!("отчёт проверки: {}", verdict_of(ev)),
        },
    }
}

/// Folds the stored facts into the path. Pure: no IO, no writes.
pub fn build(f: &GraphFacts<'_>) -> RoleGraph {
    let fold = Fold::new(f);
    let mut ns = fold.nodes();
    let position = fold.position(&ns);
    // The position's node is `current`; passed nodes stay passed.
    if let Some(node_id) = position.node.as_deref()
        && let Some(n) = ns.rows.iter_mut().find(|n| n.id == node_id)
    {
        n.status = node_status::CURRENT.to_string();
        n.status_word = node_status_word(node_status::CURRENT).to_string();
    }
    let returns = fold.returns();
    let (edges, mut notes) = fold.edges(&ns, &returns);
    let (running, running_notes) = fold.running_rows();
    notes.extend(running_notes);
    let stages = fold.stages(&position);
    let waits = fold.waits(&position, &ns.gate);
    RoleGraph {
        stages,
        nodes: ns.rows,
        edges,
        position,
        rework: fold.rework(returns.len() as u32),
        running,
        waits,
        blocking_questions: fold.blocking_count(),
        contracts: Vec::new(),
        notes,
    }
}

fn stage_label(f: &GraphFacts<'_>, st: &ProcessState, roles: &HashMap<String, String>) -> String {
    if let Some(slug) = st.role.as_deref()
        && !is_gate_state(st)
    {
        return role_name(roles, slug);
    }
    if let Some(w) = f.wait_policy.states.get(&st.state)
        && !w.demand.trim().is_empty()
    {
        return w.demand.clone();
    }
    if let Some(w) = f.wait_policy.states.get(&st.state)
        && let Some(v) = w.variants.first()
    {
        return v.demand.clone();
    }
    st.state.clone()
}

/// The gate's human requirement and the first declared command with REAL ids
/// substituted (never a placeholder in print).
fn gate_demand(
    policy: &InboxWaitPolicy,
    task: &Task,
    state: &str,
    parked: Option<&Question>,
) -> (String, String) {
    let short = short_id(&task.id);
    let qid = parked
        .map(|q| short_id(&q.id))
        .unwrap_or_else(|| short.clone());
    let fill = |s: &str| s.replace("{id}", &short).replace("{question_id}", &qid);
    let Some(row) = policy.states.get(state) else {
        return (state.to_string(), String::new());
    };
    let (demand, command) = if !row.demand.trim().is_empty() {
        (
            row.demand.clone(),
            row.commands.first().cloned().unwrap_or_default(),
        )
    } else if let Some(v) = row.variants.first() {
        (
            v.demand.clone(),
            v.commands.first().cloned().unwrap_or_default(),
        )
    } else {
        (state.to_string(), String::new())
    };
    (demand, fill(&command))
}

/// What the gate presents: the CURRENT document, else the last stored one
/// with `current: false`. Only document gates present a document.
fn gate_document<'a>(f: &'a GraphFacts<'a>, state: &str) -> Option<(&'a Artifact, bool)> {
    let kind = match state {
        s if s == TaskState::Approval.as_str() => ArtifactKind::RequirementsSpec,
        s if s == TaskState::Qa.as_str() => ArtifactKind::QualityReport,
        _ => return None,
    };
    let candidates: Vec<&Artifact> = f.artifacts.iter().filter(|a| a.kind == kind).collect();
    if let Some(current) = candidates
        .iter()
        .find(|a| a.status == ArtifactStatus::Current)
    {
        return Some((current, true));
    }
    candidates
        .into_iter()
        .max_by_key(|a| (a.seq, a.created_at))
        .map(|a| (a, false))
}

/// A gate node's label and its affirmative-act status. The status is FOLDED
/// here (`expected` or `passed`); the position pass later promotes the
/// anchor node to `current`.
fn gate_facts(f: &GraphFacts<'_>, state: &str, _id: &str, fallback: &str) -> (String, String) {
    let label = gate_demand(f.wait_policy, f.task, state, None).0;
    let passed = match state {
        s if s == TaskState::Approval.as_str() => f.decisions.iter().any(|d| {
            d.kind == decision_kind::REQUIREMENTS_APPROVAL && d.verdict == Verdict::Approved
        }),
        s if s == TaskState::Qa.as_str() => {
            f.events.iter().any(|e| e.kind == event_kind::TASK_ACCEPTED)
        }
        _ => false,
    };
    let status = if passed {
        node_status::PASSED
    } else {
        fallback
    };
    (label, status.to_string())
}

/// All artifacts produced by the role (every version, every status), plus
/// the retracted mark. Full lists — the card never hides rows (REQ-RG-06).
fn node_artifacts(
    f: &GraphFacts<'_>,
    slug: &str,
    exec_by_role: &BTreeMap<String, Vec<&RoleExecution>>,
    _roles: &HashMap<String, String>,
) -> (Vec<GraphArtifact>, bool) {
    let exec_ids: HashSet<Id> = exec_by_role
        .get(slug)
        .map(|list| list.iter().map(|e| e.id).collect())
        .unwrap_or_default();
    let mut out: Vec<GraphArtifact> = f
        .artifacts
        .iter()
        .filter(|a| {
            a.execution_id.is_some_and(|id| exec_ids.contains(&id))
                || exec_ids.iter().any(|id| id.to_string() == a.created_by)
        })
        .map(|a| GraphArtifact {
            id: a.id.to_string(),
            kind: a.kind.as_str().to_string(),
            seq: a.seq,
            status: a.status.as_str().to_string(),
            name: a.name.clone(),
            display: artifact_display(f.artifact_titles, a),
        })
        .collect();
    out.sort_by(|a, b| (a.seq, &a.id).cmp(&(b.seq, &b.id)));
    let revoked = out.iter().any(|a| {
        a.status == ArtifactStatus::Stale.as_str()
            || a.status == ArtifactStatus::Superseded.as_str()
    });
    (out, revoked)
}

/// Human signboards of the card: a node's display by id and a stage's
/// declared label — a position never heads with a raw id (REQ-NM-01).
struct Places<'a, 'b> {
    facts: &'b GraphFacts<'a>,
    axis: &'b [&'a ProcessState],
    node_labels: &'b HashMap<String, String>,
    roles: &'b HashMap<String, String>,
}

impl Places<'_, '_> {
    fn node(&self, id: &str) -> String {
        self.node_labels
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }

    fn stage(&self, state: &str) -> String {
        self.axis
            .iter()
            .find(|s| s.state == state)
            .map(|st| stage_label(self.facts, st, self.roles))
            .unwrap_or_else(|| state.to_string())
    }

    fn at_node(
        &self,
        kind: &str,
        anchor: &str,
        stage: Option<String>,
        id: &str,
        line: String,
    ) -> GraphPosition {
        GraphPosition {
            kind: kind.to_string(),
            anchor: anchor.to_string(),
            stage,
            node: Some(id.to_string()),
            reason: None,
            line,
        }
    }

    fn at_stage(&self, stage: &str, line: String) -> GraphPosition {
        GraphPosition {
            kind: position_kind::STAGE.to_string(),
            anchor: anchor_kind::STAGE.to_string(),
            stage: Some(stage.to_string()),
            node: None,
            reason: None,
            line,
        }
    }

    fn off(&self, reason: &str, line: &str) -> GraphPosition {
        GraphPosition {
            kind: position_kind::OFF_GRAPH.to_string(),
            anchor: anchor_kind::STAGE.to_string(),
            stage: None,
            node: None,
            reason: Some(reason.to_string()),
            line: line.to_string(),
        }
    }
}

/// The ONE position, from the state and stored facts: a dispatch over the
/// three cases, each named by its own unit below (REQ-RG-03).
fn position_of(
    f: &GraphFacts<'_>,
    axis: &[&ProcessState],
    ns: &NodeSet,
    node_labels: &HashMap<String, String>,
    roles: &HashMap<String, String>,
) -> GraphPosition {
    let places = Places {
        facts: f,
        axis,
        node_labels,
        roles,
    };
    match f.task.state {
        TaskState::Draft => places.off(off_graph_reason::DRAFT, "черновик — работа не начиналась"),
        TaskState::Cancelled => places.off(off_graph_reason::CANCELLED, "задача отменена"),
        TaskState::Accepted => places.at_node(
            position_kind::NODE,
            anchor_kind::FINAL,
            None,
            "final",
            "задача принята".to_string(),
        ),
        s if s.is_hold() => hold_position(f, &places, ns),
        _ => active_position(f, &places, ns),
    }
}

/// A parked task stands on its return place: that place's node when it HAS
/// one, else the stage without a «сейчас» node (REQ-RG-03).
fn hold_position(f: &GraphFacts<'_>, places: &Places<'_, '_>, ns: &NodeSet) -> GraphPosition {
    let Some(rs) = f.task.return_state else {
        return places.off(
            off_graph_reason::NO_RETURN_STATE,
            "задача припаркована, сохранённого возврата нет",
        );
    };
    let stage = Some(rs.as_str().to_string());
    if let Some(id) = f
        .process
        .role_for(rs.as_str())
        .and_then(|slug| ns.role.get(slug))
    {
        let line = format!("припаркована; место возврата — {}", places.node(id));
        return places.at_node(position_kind::HOLD, anchor_kind::ROLE, stage, id, line);
    }
    if is_gate_by_state(f.process, rs)
        && let Some(id) = ns.gate.get(rs.as_str())
    {
        let line = format!("припаркована; место возврата — гейт «{}»", places.node(id));
        return places.at_node(position_kind::HOLD, anchor_kind::GATE, stage, id, line);
    }
    places.at_stage(
        rs.as_str(),
        format!(
            "припаркована; место возврата — этап «{}»",
            places.stage(rs.as_str())
        ),
    )
}

/// An active state: the role while its run is the place; where BOTH a role and
/// a human work (QA today), the signature gate unless the run is going; else
/// the stage itself, with no «сейчас» node (REQ-RG-03).
fn active_position(f: &GraphFacts<'_>, places: &Places<'_, '_>, ns: &NodeSet) -> GraphPosition {
    let state = f.task.state;
    let gate_state = is_gate_by_state(f.process, state);
    if let Some(slug) = f.process.role_for(state.as_str()) {
        let stage = Some(state.as_str().to_string());
        let running = f
            .executions
            .iter()
            .any(|e| e.role_slug == slug && e.status == ExecutionStatus::Running);
        if let Some(id) = ns.role.get(slug).filter(|_| !gate_state || running) {
            let line = format!("сейчас: {}", places.node(id));
            return places.at_node(position_kind::NODE, anchor_kind::ROLE, stage, id, line);
        }
        if gate_state && let Some(id) = ns.gate.get(state.as_str()) {
            let line = format!("сейчас: гейт «{}»", places.node(id));
            return places.at_node(position_kind::NODE, anchor_kind::GATE, stage, id, line);
        }
        return places.at_stage(
            state.as_str(),
            format!("сейчас: этап «{}»", places.stage(state.as_str())),
        );
    }
    if gate_state && let Some(id) = ns.gate.get(state.as_str()) {
        let stage = Some(state.as_str().to_string());
        let line = format!("сейчас: гейт «{}»", places.node(id));
        return places.at_node(position_kind::NODE, anchor_kind::GATE, stage, id, line);
    }
    places.at_stage(
        state.as_str(),
        format!("этап «{}»", places.stage(state.as_str())),
    )
}

fn is_gate_by_state(process: &Process, state: TaskState) -> bool {
    process
        .states
        .iter()
        .any(|s| s.state == state.as_str() && is_gate_state(s))
}

fn stage_index_for_position(axis: &[&ProcessState], p: &GraphPosition) -> Option<usize> {
    if p.kind == position_kind::OFF_GRAPH {
        return None;
    }
    if p.anchor == anchor_kind::FINAL {
        return Some(axis.len());
    }
    let stage = p.stage.as_deref()?;
    if stage == TaskState::Rework.as_str() {
        return Some(axis.len());
    }
    axis.iter().position(|s| s.state == stage)
}

fn push_edge(
    edges: &mut Vec<GraphEdge>,
    seen: &mut HashSet<(String, String, String, String)>,
    e: GraphEdge,
) {
    let key = (
        e.from.clone(),
        e.to.clone(),
        e.kind.clone(),
        e.carrier.id.clone(),
    );
    if seen.insert(key) {
        edges.push(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::{WaitGroup, WaitStatePolicy};
    use chrono::{TimeZone, Utc};

    fn now() -> Timestamp {
        Utc.with_ymd_and_hms(2026, 11, 11, 12, 0, 0).unwrap()
    }

    fn task(state: TaskState) -> Task {
        let mut t = Task::new("формальный заголовок", "бизнес-описание", None, now());
        t.state = state;
        t
    }

    fn titles() -> BTreeMap<String, String> {
        ArtifactKind::ALL
            .iter()
            .map(|k| (k.as_str().to_string(), format!("название {}", k.as_str())))
            .collect()
    }

    fn roles() -> Vec<(String, String)> {
        vec![
            ("requirements-analyst".into(), "Requirements Analyst".into()),
            (
                "requirements-reviewer".into(),
                "Requirements Reviewer".into(),
            ),
            ("implementation-agent".into(), "Implementation Agent".into()),
            ("quality-auditor".into(), "Quality Auditor".into()),
        ]
    }

    fn policy() -> InboxWaitPolicy {
        let mut states = std::collections::BTreeMap::new();
        states.insert(
            "APPROVAL".to_string(),
            WaitStatePolicy {
                group: WaitGroup::Platform,
                demand: "Утвердить требования".into(),
                commands: vec!["devai task approve {id}".into()],
                variants: vec![],
            },
        );
        states.insert(
            "QA".to_string(),
            WaitStatePolicy {
                group: WaitGroup::Platform,
                demand: "Подписать качество".into(),
                commands: vec!["devai task accept {id}".into()],
                variants: vec![],
            },
        );
        InboxWaitPolicy { states }
    }

    fn execution(task_id: Id, slug: &str, version: u32, status: ExecutionStatus) -> RoleExecution {
        RoleExecution {
            id: new_id(),
            task_id,
            role_slug: slug.into(),
            role_version: version,
            agent_name: Some("mock".into()),
            status,
            inputs: vec![],
            summary: None,
            error: None,
            started_at: now(),
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
        }
    }

    fn artifact(
        task_id: Id,
        kind: ArtifactKind,
        seq: u32,
        status: ArtifactStatus,
        execution_id: Option<Id>,
    ) -> Artifact {
        Artifact {
            id: new_id(),
            task_id,
            kind,
            name: format!("{} v{seq}", kind.as_str()),
            seq,
            category: DataCategory::Artifact,
            status,
            sha256: "0".repeat(64),
            size: 2,
            media_type: "application/json".into(),
            contract: None,
            created_by: execution_id
                .map(|e| e.to_string())
                .unwrap_or_else(|| "user".into()),
            execution_id,
            depends_on: vec![],
            created_at: now(),
        }
    }

    fn event(task_id: Id, kind: &str, payload: serde_json::Value) -> WorkflowEvent {
        WorkflowEvent {
            id: new_id(),
            task_id: Some(task_id),
            kind: kind.into(),
            actor: "guerrero".into(),
            payload,
            created_at: now(),
        }
    }

    fn decision(task_id: Id, kind: &str, verdict: Verdict) -> Decision {
        Decision {
            id: new_id(),
            task_id,
            kind: kind.into(),
            verdict,
            actor: "guerrero".into(),
            rationale: None,
            refs: vec![],
            created_at: now(),
            payload: serde_json::Value::Null,
        }
    }

    fn question(task_id: Id, blocking: bool, open: bool) -> Question {
        Question {
            id: new_id(),
            task_id,
            execution_id: None,
            text: "вопрос".into(),
            context: None,
            options: vec![],
            blocking,
            status: if open {
                QuestionStatus::Open
            } else {
                QuestionStatus::Answered
            },
            answer: None,
            answered_by: None,
            answered_at: None,
            created_at: now(),
        }
    }

    /// The declaration-wide fixture: process, role names, artifact titles and
    /// wait policy every test folds with, built once per test.
    struct Fx {
        process: &'static Process,
        roles: Vec<(String, String)>,
        titles: BTreeMap<String, String>,
        policy: InboxWaitPolicy,
    }

    impl Fx {
        fn new() -> Self {
            Fx {
                process: Process::factory(),
                roles: roles(),
                titles: titles(),
                policy: policy(),
            }
        }

        /// Folds the facts under test; the caller names only the slices it is
        /// about, the declaration-wide ones come from the fixture.
        fn graph(
            &self,
            task: &Task,
            artifacts: &[Artifact],
            executions: &[RoleExecution],
            decisions: &[Decision],
            events: &[WorkflowEvent],
            questions: &[Question],
        ) -> RoleGraph {
            build(&GraphFacts {
                process: self.process,
                task,
                artifacts,
                executions,
                decisions,
                events,
                questions,
                roles: &self.roles,
                artifact_titles: &self.titles,
                wait_policy: &self.policy,
            })
        }
    }

    /// The node with this id — the tests name the node they are about.
    fn node<'a>(g: &'a RoleGraph, id: &str) -> &'a GraphNode {
        g.nodes.iter().find(|n| n.id == id).unwrap()
    }

    /// The node of this role.
    fn role_node<'a>(g: &'a RoleGraph, slug: &str) -> &'a GraphNode {
        g.nodes
            .iter()
            .find(|n| n.role_slug.as_deref() == Some(slug))
            .unwrap()
    }

    /// The ids of the nodes the fold marked `current`.
    fn current_ids(g: &RoleGraph) -> Vec<&str> {
        g.nodes
            .iter()
            .filter(|n| n.status == node_status::CURRENT)
            .map(|n| n.id.as_str())
            .collect()
    }

    /// One current spec, no reviewer run: the analyst node is passed, both
    /// declared gates exist and are expected, nothing else is drawn. The
    /// position is the REVIEW stage (a stage, not a node) — REQ-RG-02/03.
    #[test]
    fn current_spec_makes_only_the_analyst_node() {
        let fx = Fx::new();
        let t = task(TaskState::Review);
        let ex = execution(t.id, "requirements-analyst", 6, ExecutionStatus::Completed);
        let spec = artifact(
            t.id,
            ArtifactKind::RequirementsSpec,
            1,
            ArtifactStatus::Current,
            Some(ex.id),
        );
        let g = fx.graph(&t, &[spec], &[ex], &[], &[], &[]);

        let roles: Vec<&GraphNode> = g
            .nodes
            .iter()
            .filter(|n| n.kind == node_kind::ROLE)
            .collect();
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].role_slug.as_deref(), Some("requirements-analyst"));
        assert_eq!(roles[0].status, node_status::PASSED);
        let gates: Vec<&GraphNode> = g
            .nodes
            .iter()
            .filter(|n| n.kind == node_kind::GATE)
            .collect();
        assert_eq!(gates.len(), 2);
        assert!(gates.iter().all(|n| n.status == node_status::EXPECTED));
        assert_eq!(g.position.kind, position_kind::STAGE);
        assert_eq!(g.position.stage.as_deref(), Some("REVIEW"));
        assert_eq!(g.position.node, None);
        assert_eq!(g.stages[0].status, stage_status::PASSED); // ANALYSIS
        assert_eq!(g.stages[1].status, stage_status::CURRENT); // REVIEW
        assert_eq!(g.stages[2].status, stage_status::PENDING); // APPROVAL
    }

    /// Two completed reviewer versions: the node's version is the freshest and
    /// the execution list keeps BOTH rows (REQ-RG-02 AC2).
    #[test]
    fn two_versions_keep_both_executions_and_latest_version() {
        let fx = Fx::new();
        let t = task(TaskState::Approval);
        let analyst = execution(t.id, "requirements-analyst", 6, ExecutionStatus::Completed);
        let mut rev1 = execution(t.id, "requirements-reviewer", 3, ExecutionStatus::Completed);
        rev1.started_at = now() - chrono::Duration::minutes(10);
        let rev2 = execution(t.id, "requirements-reviewer", 4, ExecutionStatus::Completed);
        let spec = artifact(
            t.id,
            ArtifactKind::RequirementsSpec,
            1,
            ArtifactStatus::Current,
            Some(analyst.id),
        );
        let r1 = artifact(
            t.id,
            ArtifactKind::ReviewReport,
            1,
            ArtifactStatus::Stale,
            Some(rev1.id),
        );
        let r2 = artifact(
            t.id,
            ArtifactKind::ReviewReport,
            2,
            ArtifactStatus::Current,
            Some(rev2.id),
        );
        let g = fx.graph(&t, &[spec, r1, r2], &[analyst, rev1, rev2], &[], &[], &[]);

        let reviewer = role_node(&g, "requirements-reviewer");
        assert_eq!(reviewer.executions.len(), 2);
        assert_eq!(reviewer.role_version, Some(4));
        assert!(reviewer.revoked);
        assert_eq!(reviewer.artifacts.len(), 2);
        // Approval gate is the position and pending its signature.
        assert_eq!(g.position.node.as_deref(), Some("gate:APPROVAL"));
        assert!(g.waits.iter().any(|w| w.awaiting && w.signable));
    }

    /// A running role outside the declared process is ONLY a running row and
    /// the note «прогон вне пути» — it never gets a node (REQ-RG-02 AC3: the
    /// graph is the declared process plus confirmed facts). An artifact
    /// without an execution creates no node and no edge either.
    #[test]
    fn out_of_path_run_is_a_note_and_never_a_node() {
        let fx = Fx::new();
        let t = task(TaskState::Analysis);
        let mut out = execution(t.id, "project-intelligence", 1, ExecutionStatus::Running);
        out.inputs = vec![];
        let loose = artifact(t.id, ArtifactKind::Patch, 1, ArtifactStatus::Current, None);
        let g = fx.graph(&t, &[loose], &[out], &[], &[], &[]);

        // The run stays a fact of the «активные» plane with its mark…
        assert!(g.running.iter().any(|r| !r.in_path && r.note.is_some()));
        assert!(g.notes.iter().any(|n| n.contains("прогон вне пути")));
        // …but an undeclared role NEVER becomes a node and never carries an
        // edge: the graph is the declared process (REQ-RG-02 AC3).
        assert!(
            !g.nodes
                .iter()
                .any(|n| n.role_slug.as_deref() == Some("project-intelligence"))
        );
        assert!(
            g.edges
                .iter()
                .all(|e| !e.from.contains("project-intelligence")
                    && !e.to.contains("project-intelligence"))
        );
        // The loose artifact produced no node either: the axis has no role
        // node at all, so the position is the STAGE itself.
        assert_eq!(
            g.nodes.iter().filter(|n| n.kind == node_kind::ROLE).count(),
            0
        );
        assert_eq!(g.position.kind, position_kind::STAGE);
        assert_eq!(g.position.node, None);
    }

    /// Signed requirements with no implementation run yet: the position is
    /// the IMPLEMENTATION stage (no «сейчас» node) and NO signature edge is
    /// drawn into a role that has not started (REQ-RG-04 AC3, REQ-RG-03 AC2).
    #[test]
    fn signature_without_implementer_gives_stage_position_and_no_edge() {
        let fx = Fx::new();
        let t = task(TaskState::Implementation);
        let analyst = execution(t.id, "requirements-analyst", 6, ExecutionStatus::Completed);
        let spec = artifact(
            t.id,
            ArtifactKind::RequirementsSpec,
            1,
            ArtifactStatus::Current,
            Some(analyst.id),
        );
        let dec = decision(
            t.id,
            decision_kind::REQUIREMENTS_APPROVAL,
            Verdict::Approved,
        );
        let g = fx.graph(&t, &[spec], &[analyst], &[dec], &[], &[]);

        assert_eq!(g.position.kind, position_kind::STAGE);
        assert_eq!(g.position.stage.as_deref(), Some("IMPLEMENTATION"));
        assert_eq!(g.position.node, None);
        assert!(
            g.edges
                .iter()
                .all(|e| e.to != role_id("implementation-agent"))
        );
        assert_eq!(node(&g, "gate:APPROVAL").status, node_status::PASSED);
    }

    /// A completed run and the current artifact it produced at version 1.
    fn done(
        task_id: Id,
        slug: &str,
        version: u32,
        kind: ArtifactKind,
    ) -> (RoleExecution, Artifact) {
        let e = execution(task_id, slug, version, ExecutionStatus::Completed);
        let a = artifact(task_id, kind, 1, ArtifactStatus::Current, Some(e.id));
        (e, a)
    }

    /// The rounds carried by the stored return edges, in order.
    fn return_rounds(g: &RoleGraph) -> Vec<u32> {
        let mut rounds: Vec<u32> = g
            .edges
            .iter()
            .filter(|e| e.kind == edge_kind::RETURN)
            .filter_map(|e| e.round)
            .collect();
        rounds.sort();
        rounds
    }

    /// Two stored returns: two edges with rounds 1 and 2, the loop survives
    /// into QA (not active there) — REQ-RG-05.
    #[test]
    fn rework_rounds_are_counted_from_stored_returns() {
        let fx = Fx::new();
        let t = task(TaskState::Qa);
        let (analyst, spec) = done(
            t.id,
            "requirements-analyst",
            6,
            ArtifactKind::RequirementsSpec,
        );
        let (impl_exec, report) = done(
            t.id,
            "implementation-agent",
            8,
            ArtifactKind::ImplementationReport,
        );
        let (audit, qa) = done(t.id, "quality-auditor", 7, ArtifactKind::QualityReport);
        let events = vec![
            event(t.id, event_kind::QUALITY_RETURNED, serde_json::json!({})),
            event(t.id, event_kind::QUALITY_RETURNED, serde_json::json!({})),
        ];
        let g = fx.graph(
            &t,
            &[spec, report, qa],
            &[analyst, impl_exec, audit],
            &[],
            &events,
            &[],
        );

        assert_eq!(g.rework.rounds, 2);
        assert!(!g.rework.active);
        assert_eq!(return_rounds(&g), vec![1, 2]);
        assert!(g.edges.iter().any(|e| e.kind == edge_kind::RETURN
            && e.from == "gate:QA"
            && e.to == role_id("implementation-agent")));
    }

    /// The blocking-question count is the question plane's fact, independent
    /// of the position; a draft has no position current and all stages pending.
    #[test]
    fn blocking_questions_are_counted_from_stored_open_rows() {
        let fx = Fx::new();
        let t = task(TaskState::Analysis);
        let analyst = execution(t.id, "requirements-analyst", 6, ExecutionStatus::Completed);
        let spec = artifact(
            t.id,
            ArtifactKind::RequirementsSpec,
            1,
            ArtifactStatus::Current,
            Some(analyst.id),
        );
        let qs = vec![
            question(t.id, true, true),
            question(t.id, true, false),
            question(t.id, false, true),
        ];
        let g = fx.graph(&t, &[spec], &[analyst], &[], &[], &qs);
        assert_eq!(g.blocking_questions, 1);

        let draft = task(TaskState::Draft);
        let g = fx.graph(&draft, &[], &[], &[], &[], &[]);
        assert_eq!(g.position.kind, position_kind::OFF_GRAPH);
        assert_eq!(g.position.reason.as_deref(), Some(off_graph_reason::DRAFT));
        assert!(g.stages.iter().all(|s| s.status == stage_status::PENDING));
        // The two declared human gates exist by declaration; no ROLE node
        // exists (nobody has executed) and no edge is drawn.
        assert!(g.nodes.iter().all(|n| n.kind == node_kind::GATE));
        assert!(g.edges.is_empty());
    }

    /// REQ-RG-03 AC4 / REQ-RG-02 AC4: an accepted task has ONE current node —
    /// the final one — and the acceptance edge from the quality gate carries
    /// the stored event. Gates and the final node carry no role, no version
    /// and no executions.
    #[test]
    fn accepted_task_has_final_current_and_an_acceptance_edge() {
        let fx = Fx::new();
        let t = task(TaskState::Accepted);
        let audit = execution(t.id, "quality-auditor", 7, ExecutionStatus::Completed);
        let qa = artifact(
            t.id,
            ArtifactKind::QualityReport,
            1,
            ArtifactStatus::Current,
            Some(audit.id),
        );
        let accepted = event(t.id, event_kind::TASK_ACCEPTED, serde_json::json!({}));
        let g = fx.graph(&t, &[qa], &[audit], &[], &[accepted.clone()], &[]);

        assert_eq!(g.position.node.as_deref(), Some("final"));
        assert_eq!(current_ids(&g), vec!["final"]);
        let last = node(&g, "final");
        assert_eq!(last.kind, node_kind::FINAL);
        assert!(last.executions.is_empty());
        assert_eq!(last.role_slug, None);
        // Gates: declaration-born, never a role carrier.
        for n in g.nodes.iter().filter(|n| n.kind == node_kind::GATE) {
            assert_eq!(n.role_slug, None);
            assert_eq!(n.role_version, None);
            assert!(n.executions.is_empty());
        }
        // REQ-RG-02: acceptance is the affirmative human act that PASSES the
        // quality gate — it is not left «ожидается» on a finished task.
        assert_eq!(node(&g, "gate:QA").status, node_status::PASSED);
        assert_eq!(node(&g, "gate:APPROVAL").status, node_status::EXPECTED);
        // REQ-RG-01 AC1: the waits follow the DECLARED axis order (never a
        // hash-map iteration), so two reads of unchanged facts agree.
        let order: Vec<&str> = g.waits.iter().map(|w| w.state.as_str()).collect();
        assert_eq!(order, vec!["APPROVAL", "QA"]);
        assert!(g.edges.iter().any(|e| e.kind == edge_kind::ACCEPTANCE
            && e.from == "gate:QA"
            && e.to == "final"
            && e.carrier.id == accepted.id.to_string()));
    }

    /// REQ-RG-01 AC1: a second read of the SAME facts differs in nothing — the
    /// fold is pure and every list follows the declaration, not a hash map.
    #[test]
    fn a_repeated_read_of_the_same_facts_is_identical() {
        let fx = Fx::new();
        let t = task(TaskState::Qa);
        let audit = execution(t.id, "quality-auditor", 7, ExecutionStatus::Completed);
        let qa = artifact(
            t.id,
            ArtifactKind::QualityReport,
            1,
            ArtifactStatus::Current,
            Some(audit.id),
        );
        let returns = vec![
            event(t.id, event_kind::QUALITY_RETURNED, serde_json::json!({})),
            event(t.id, event_kind::QUALITY_RETURNED, serde_json::json!({})),
        ];
        let first = fx.graph(&t, &[qa.clone()], &[audit.clone()], &[], &returns, &[]);
        let again = fx.graph(&t, &[qa], &[audit], &[], &returns, &[]);
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&again).unwrap()
        );
    }

    /// REQ-RG-03 AC4: an accepted task without a stored acceptance act says so
    /// in a note instead of drawing an edge to nowhere.
    #[test]
    fn accepted_task_without_stored_act_reports_a_data_mismatch() {
        let fx = Fx::new();
        let t = task(TaskState::Accepted);
        let g = fx.graph(&t, &[], &[], &[], &[], &[]);
        assert!(g.notes.iter().any(|n| n.contains("расхождение данных")));
        assert!(g.edges.iter().all(|e| e.kind != edge_kind::ACCEPTANCE));
    }

    /// REQ-RG-03 AC3: a hold returning to the approval gate stands ON that
    /// gate, and the gate keeps both its signature state and its presentation.
    #[test]
    fn hold_on_the_approval_gate_keeps_signature_and_presentation() {
        let fx = Fx::new();
        let mut t = task(TaskState::Paused);
        t.return_state = Some(TaskState::Approval);
        let analyst = execution(t.id, "requirements-analyst", 6, ExecutionStatus::Completed);
        let spec = artifact(
            t.id,
            ArtifactKind::RequirementsSpec,
            2,
            ArtifactStatus::Current,
            Some(analyst.id),
        );
        let g = fx.graph(&t, &[spec], &[analyst], &[], &[], &[]);

        assert_eq!(g.position.kind, position_kind::HOLD);
        assert_eq!(g.position.anchor, anchor_kind::GATE);
        assert_eq!(g.position.node.as_deref(), Some("gate:APPROVAL"));
        let approval = g
            .waits
            .iter()
            .find(|w| w.state == TaskState::Approval.as_str())
            .expect("гейт утверждения объявлен");
        assert!(approval.awaiting && approval.signable);
        assert_eq!(
            approval.presentation.as_ref().map(|p| p.display.as_str()),
            Some("название REQUIREMENTS_SPEC · v2")
        );
        assert!(approval.presentation.as_ref().is_some_and(|p| p.current));
    }

    /// REQ-RG-03 AC1/AC3: a hold whose return stage has no node is a STAGE
    /// position — the stage is marked current and no node is «сейчас».
    #[test]
    fn hold_returning_to_a_stage_without_a_node_is_a_stage_position() {
        let fx = Fx::new();
        let mut t = task(TaskState::WaitingForInput);
        t.return_state = Some(TaskState::Analysis);
        let g = fx.graph(&t, &[], &[], &[], &[], &[]);

        assert_eq!(g.position.kind, position_kind::STAGE);
        assert_eq!(g.position.anchor, anchor_kind::STAGE);
        assert_eq!(g.position.stage.as_deref(), Some("ANALYSIS"));
        assert_eq!(g.position.node, None);
        assert!(g.nodes.iter().all(|n| n.status != node_status::CURRENT));
        let analysis = g
            .stages
            .iter()
            .find(|s| s.state == TaskState::Analysis.as_str())
            .unwrap();
        assert_eq!(analysis.status, stage_status::CURRENT);
    }

    /// REQ-RG-02 AC7: a retracted result is a separate mark and never makes
    /// the node passed.
    #[test]
    fn a_revoked_result_does_not_make_the_node_passed() {
        let fx = Fx::new();
        let t = task(TaskState::Review);
        let analyst = execution(t.id, "requirements-analyst", 6, ExecutionStatus::Completed);
        let spec = artifact(
            t.id,
            ArtifactKind::RequirementsSpec,
            1,
            ArtifactStatus::Superseded,
            Some(analyst.id),
        );
        let g = fx.graph(&t, &[spec], &[analyst], &[], &[], &[]);

        let node = role_node(&g, "requirements-analyst");
        assert_eq!(node.status, node_status::EXPECTED);
        assert!(node.revoked);
        assert_eq!(node.artifacts.len(), 1);
    }

    /// REQ-NM-01 AC4: the ONE heading rule is «название вида · vN», checked on
    /// versions 1..3 of one kind; the stored name never becomes the heading.
    #[test]
    fn artifact_heading_is_the_declared_title_and_version() {
        let fx = Fx::new();
        let t = task(TaskState::Review);
        let analyst = execution(t.id, "requirements-analyst", 6, ExecutionStatus::Completed);
        let arts: Vec<Artifact> = [1, 2, 3]
            .into_iter()
            .map(|seq| {
                artifact(
                    t.id,
                    ArtifactKind::RequirementsSpec,
                    seq,
                    ArtifactStatus::Superseded,
                    Some(analyst.id),
                )
            })
            .collect();
        let g = fx.graph(&t, &arts, &[analyst], &[], &[], &[]);

        let displays: Vec<&str> = role_node(&g, "requirements-analyst")
            .artifacts
            .iter()
            .map(|a| a.display.as_str())
            .collect();
        assert_eq!(
            displays,
            vec![
                "название REQUIREMENTS_SPEC · v1",
                "название REQUIREMENTS_SPEC · v2",
                "название REQUIREMENTS_SPEC · v3",
            ]
        );
    }
}
