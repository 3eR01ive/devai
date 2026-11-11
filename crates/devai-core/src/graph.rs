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

/// Folds the stored facts into the path. Pure: no IO, no writes.
pub fn build(f: &GraphFacts<'_>) -> RoleGraph {
    let roles: HashMap<String, String> = f.roles.iter().cloned().collect();
    let axis = axis_states(f.process);
    let mut notes: Vec<String> = Vec::new();

    // --- executions by role, freshest version wins ---------------------
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
    let mut running_version: HashMap<String, u32> = HashMap::new();
    for e in &running {
        running_version.insert(e.role_slug.clone(), e.role_version);
    }

    // --- nodes: declared order, then out-of-path executions -------------
    let mut nodes: Vec<GraphNode> = Vec::new();
    let mut role_nodes: HashMap<String, String> = HashMap::new();
    let mut gate_nodes: HashMap<String, String> = HashMap::new();

    for st in &axis {
        if is_gate_state(st) && st.executor != process_executor::ROLE {
            // A human-only gate (APPROVAL today). QA is role-executed and
            // declares human acts too; its node is also a gate — the branch
            // below covers it because a gate is a gate regardless of who
            // works there.
            let (label, status) =
                gate_facts(f, &st.state, &gate_id(&st.state), &node_status::EXPECTED);
            gate_nodes.insert(st.state.clone(), gate_id(&st.state));
            nodes.push(GraphNode {
                id: gate_id(&st.state),
                kind: node_kind::GATE.to_string(),
                state: st.state.clone(),
                label: label.clone(),
                display: label,
                role_slug: None,
                role_version: None,
                status: status.clone(),
                status_word: node_status_word(status.as_str()).to_string(),
                revoked: false,
                executions: Vec::new(),
                artifacts: Vec::new(),
            });
            continue;
        }
        if let Some(slug) = st.role.as_deref() {
            if let Some(list) = exec_by_role.get(slug) {
                let version = running_version
                    .get(slug)
                    .copied()
                    .or_else(|| list.last().map(|e| e.role_version))
                    .unwrap_or(1);
                let id = role_id(slug);
                let (artifacts, revoked) = node_artifacts(f, slug, &exec_by_role, &roles);
                let passed = artifacts
                    .iter()
                    .any(|a| a.status == ArtifactStatus::Current.as_str());
                let status = if passed {
                    node_status::PASSED
                } else {
                    node_status::EXPECTED
                };
                let executions = list
                    .iter()
                    .map(|e| GraphExecution {
                        id: e.id.to_string(),
                        role_slug: e.role_slug.clone(),
                        role_display: role_display(&roles, &e.role_slug, e.role_version),
                        role_version: e.role_version,
                        agent_name: e.agent_name.clone(),
                        status: e.status.as_str().to_string(),
                        attempt: e.attempt,
                        started_at: e.started_at,
                        finished_at: e.finished_at,
                    })
                    .collect();
                role_nodes.insert(slug.to_string(), id.clone());
                nodes.push(GraphNode {
                    id,
                    kind: node_kind::ROLE.to_string(),
                    state: st.state.clone(),
                    label: role_name(&roles, slug),
                    display: role_display(&roles, slug, version),
                    role_slug: Some(slug.to_string()),
                    role_version: Some(version),
                    status: status.to_string(),
                    status_word: node_status_word(status).to_string(),
                    revoked,
                    executions,
                    artifacts,
                });
            }
            if is_gate_state(st) {
                // QA today: a role stage that also declares the human act.
                // The gate keeps the affirmative-act fact (`passed` once the
                // acceptance is stored) — the position pass promotes it to
                // `current` while the signature is still awaited.
                let (label, status) =
                    gate_facts(f, &st.state, &gate_id(&st.state), &node_status::EXPECTED);
                gate_nodes.insert(st.state.clone(), gate_id(&st.state));
                nodes.push(GraphNode {
                    id: gate_id(&st.state),
                    kind: node_kind::GATE.to_string(),
                    state: st.state.clone(),
                    label: label.clone(),
                    display: label,
                    role_slug: None,
                    role_version: None,
                    status: status.clone(),
                    status_word: node_status_word(status.as_str()).to_string(),
                    revoked: false,
                    executions: Vec::new(),
                    artifacts: Vec::new(),
                });
            }
        }
    }

    // Executed roles outside the declared path: they exist as nodes (the
    // fact is stored) and are named as out-of-path by a note; they never
    // create edges to the declared path.
    let declared: HashSet<String> = role_nodes.keys().cloned().collect();
    for (slug, list) in &exec_by_role {
        if declared.contains(slug) {
            continue;
        }
        let version = running_version
            .get(slug)
            .copied()
            .or_else(|| list.last().map(|e| e.role_version))
            .unwrap_or(1);
        let (artifacts, revoked) = node_artifacts(f, slug, &exec_by_role, &roles);
        let passed = artifacts
            .iter()
            .any(|a| a.status == ArtifactStatus::Current.as_str());
        let status = if passed {
            node_status::PASSED
        } else {
            node_status::EXPECTED
        };
        let executions = list
            .iter()
            .map(|e| GraphExecution {
                id: e.id.to_string(),
                role_slug: e.role_slug.clone(),
                role_display: role_display(&roles, &e.role_slug, e.role_version),
                role_version: e.role_version,
                agent_name: e.agent_name.clone(),
                status: e.status.as_str().to_string(),
                attempt: e.attempt,
                started_at: e.started_at,
                finished_at: e.finished_at,
            })
            .collect();
        role_nodes.insert(slug.clone(), role_id(slug));
        nodes.push(GraphNode {
            id: role_id(slug),
            kind: node_kind::ROLE.to_string(),
            state: String::new(),
            label: role_name(&roles, slug),
            display: role_display(&roles, slug, version),
            role_slug: Some(slug.clone()),
            role_version: Some(version),
            status: status.to_string(),
            status_word: node_status_word(status).to_string(),
            revoked,
            executions,
            artifacts,
        });
    }

    // --- the final node exists only for an accepted task ----------------
    if f.task.state == TaskState::Accepted {
        nodes.push(GraphNode {
            id: "final".to_string(),
            kind: node_kind::FINAL.to_string(),
            state: TaskState::Accepted.as_str().to_string(),
            label: "принято".to_string(),
            display: "принято".to_string(),
            role_slug: None,
            role_version: None,
            status: node_status::EXPECTED.to_string(),
            status_word: node_status_word(node_status::EXPECTED).to_string(),
            revoked: false,
            executions: Vec::new(),
            artifacts: Vec::new(),
        });
    }

    // --- position -------------------------------------------------------
    // The position names a PLACE in human words: node display («имя · vN» /
    // the gate's declared demand) or the declared stage's label — never a raw
    // id/token as the heading (REQ-NM-01).
    let node_labels: HashMap<String, String> = nodes
        .iter()
        .map(|n| (n.id.clone(), n.display.clone()))
        .collect();
    let mut position = position_of(f, &axis, &role_nodes, &gate_nodes, &node_labels, &roles);
    // The position's node is `current`; passed nodes stay passed.
    if let Some(node_id) = position.node.as_deref()
        && let Some(n) = nodes.iter_mut().find(|n| n.id == node_id)
    {
        n.status = node_status::CURRENT.to_string();
        n.status_word = node_status_word(node_status::CURRENT).to_string();
    }

    // --- edges: only stored facts, only between existing nodes ----------
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut seen: HashSet<(String, String, String, String)> = HashSet::new();
    let exec_role: HashMap<Id, &str> = f
        .executions
        .iter()
        .map(|e| (e.id, e.role_slug.as_str()))
        .collect();
    let artifact_by_id: HashMap<Id, &Artifact> = f.artifacts.iter().map(|a| (a.id, a)).collect();

    // Consumption: an execution names an artifact → producer → consumer.
    for e in f.executions {
        let Some(to) = role_nodes.get(&e.role_slug) else {
            continue;
        };
        for input in &e.inputs {
            let Ok(id) = input.parse::<Id>() else {
                continue;
            };
            let Some(a) = artifact_by_id.get(&id) else {
                continue;
            };
            let Some(producer_exec) = a.execution_id else {
                continue;
            };
            let Some(producer_slug) = exec_role.get(&producer_exec) else {
                continue;
            };
            let Some(from) = role_nodes.get(*producer_slug) else {
                continue;
            };
            if from == to {
                continue;
            }
            push_edge(
                &mut edges,
                &mut seen,
                GraphEdge {
                    from: from.clone(),
                    to: to.clone(),
                    kind: edge_kind::TRANSFER.to_string(),
                    round: None,
                    carrier: GraphCarrier {
                        kind: carrier_kind::ARTIFACT.to_string(),
                        id: a.id.to_string(),
                        label: artifact_display(f.artifact_titles, a),
                    },
                },
            );
        }
    }

    // Reviewer verdicts: a go verdict goes to the requirements gate, any
    // other verdict back to the analyst node. A verdict without its node
    // gives a NOTE and zero edges.
    let reviewer_node = f
        .process
        .role_for(TaskState::Review.as_str())
        .and_then(|slug| role_nodes.get(slug));
    let analyst_node = f
        .process
        .role_for(TaskState::Analysis.as_str())
        .and_then(|slug| role_nodes.get(slug));
    for ev in f
        .events
        .iter()
        .filter(|e| e.kind == event_kind::REVIEW_RECORDED)
    {
        let verdict = ev
            .payload
            .get(json_field::VERDICT)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let approved = verdict == Verdict::Approved.as_str();
        let from = reviewer_node;
        let to = if approved {
            gate_nodes.get(TaskState::Approval.as_str())
        } else {
            analyst_node
        };
        match (from, to) {
            (Some(from), Some(to)) => push_edge(
                &mut edges,
                &mut seen,
                GraphEdge {
                    from: from.clone(),
                    to: to.clone(),
                    kind: if approved {
                        edge_kind::SIGNATURE.to_string()
                    } else {
                        edge_kind::REJECTION.to_string()
                    },
                    round: None,
                    carrier: GraphCarrier {
                        kind: carrier_kind::EVENT.to_string(),
                        id: ev.id.to_string(),
                        label: format!("отчёт проверки: {verdict}"),
                    },
                },
            ),
            _ => notes.push(
                "вердикт проверки без узла на графе — ребро не рисуется, факт остаётся в истории"
                    .to_string(),
            ),
        }
    }

    // Reports (implementation + quality): every publication gives its own
    // edge from its role node to the quality gate. A report without an
    // execution gives no node and no edge.
    let qa_gate = gate_nodes.get(TaskState::Qa.as_str()).cloned();
    if let Some(gate) = qa_gate.as_deref() {
        for a in f.artifacts.iter().filter(|a| {
            matches!(
                a.kind,
                ArtifactKind::ImplementationReport | ArtifactKind::QualityReport
            )
        }) {
            let Some(exec_id) = a.execution_id else {
                continue;
            };
            let Some(slug) = exec_role.get(&exec_id) else {
                continue;
            };
            let Some(from) = role_nodes.get(*slug) else {
                continue;
            };
            push_edge(
                &mut edges,
                &mut seen,
                GraphEdge {
                    from: from.clone(),
                    to: gate.to_string(),
                    kind: edge_kind::SIGNATURE.to_string(),
                    round: None,
                    carrier: GraphCarrier {
                        kind: carrier_kind::ARTIFACT.to_string(),
                        id: a.id.to_string(),
                        label: artifact_display(f.artifact_titles, a),
                    },
                },
            );
        }
    }

    // Human acts: signature, rejection, returns, acceptance — each from the
    // stored decision/event and only when BOTH ends exist.
    let implementer_node = f
        .process
        .role_for(TaskState::Implementation.as_str())
        .and_then(|slug| role_nodes.get(slug));
    let approval_gate = gate_nodes.get(TaskState::Approval.as_str());
    for d in f.decisions {
        match d.kind.as_str() {
            decision_kind::REQUIREMENTS_APPROVAL if d.verdict == Verdict::Approved => {
                if let (Some(from), Some(to)) = (approval_gate, implementer_node) {
                    push_edge(
                        &mut edges,
                        &mut seen,
                        GraphEdge {
                            from: from.clone(),
                            to: to.clone(),
                            kind: edge_kind::SIGNATURE.to_string(),
                            round: None,
                            carrier: GraphCarrier {
                                kind: carrier_kind::DECISION.to_string(),
                                id: d.id.to_string(),
                                label: "подпись требований".to_string(),
                            },
                        },
                    );
                }
            }
            decision_kind::REQUIREMENTS_REJECTION => {
                if let (Some(from), Some(to)) = (approval_gate, analyst_node) {
                    push_edge(
                        &mut edges,
                        &mut seen,
                        GraphEdge {
                            from: from.clone(),
                            to: to.clone(),
                            kind: edge_kind::REJECTION.to_string(),
                            round: None,
                            carrier: GraphCarrier {
                                kind: carrier_kind::DECISION.to_string(),
                                id: d.id.to_string(),
                                label: "отклонение требований".to_string(),
                            },
                        },
                    );
                }
            }
            _ => {}
        }
    }

    // Returns: every stored quality return is one edge and one round.
    let mut rounds = 0u32;
    for ev in f
        .events
        .iter()
        .filter(|e| e.kind == event_kind::QUALITY_RETURNED)
    {
        rounds += 1;
        if let (Some(from), Some(to)) = (qa_gate.as_deref(), implementer_node) {
            push_edge(
                &mut edges,
                &mut seen,
                GraphEdge {
                    from: from.to_string(),
                    to: to.clone(),
                    kind: edge_kind::RETURN.to_string(),
                    round: Some(rounds),
                    carrier: GraphCarrier {
                        kind: carrier_kind::EVENT.to_string(),
                        id: ev.id.to_string(),
                        label: format!("возврат на доработку, круг {rounds}"),
                    },
                },
            );
        }
    }

    // Acceptance into the final node.
    let accepted_events: Vec<&WorkflowEvent> = f
        .events
        .iter()
        .filter(|e| e.kind == event_kind::TASK_ACCEPTED)
        .collect();
    if let (Some(gate), true) = (qa_gate.as_deref(), !accepted_events.is_empty())
        && f.task.state == TaskState::Accepted
    {
        if let Some(ev) = accepted_events.last() {
            push_edge(
                &mut edges,
                &mut seen,
                GraphEdge {
                    from: gate.to_string(),
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
    }
    if f.task.state == TaskState::Accepted && accepted_events.is_empty() {
        notes.push(
            "у принятой задачи нет сохранённого акта приёмки — расхождение данных".to_string(),
        );
    }

    // --- stages: statuses positional to the ONE position ----------------
    let stage_idx = stage_index_for_position(&axis, &position);
    let mut stages: Vec<GraphStage> = axis
        .iter()
        .enumerate()
        .map(|(i, st)| {
            let status = match stage_idx {
                None => stage_status::PENDING,
                Some(idx) if i < idx => stage_status::PASSED,
                Some(idx) if i == idx => stage_status::CURRENT,
                Some(_) => stage_status::PENDING,
            };
            let label = stage_label(f, st, &roles);
            GraphStage {
                state: st.state.clone(),
                label,
                executor: st
                    .role
                    .clone()
                    .unwrap_or_else(|| process_executor::HUMAN.to_string()),
                gate: is_gate_state(st),
                status: status.to_string(),
                status_word: stage_status_word(status).to_string(),
            }
        })
        .collect();
    // A task in REWORK (or a hold returning there) has passed the whole
    // axis: the loop is drawn separately, not as «ещё одна точка».
    let on_rework = f.task.state == TaskState::Rework
        || (f.task.state.is_hold() && f.task.return_state == Some(TaskState::Rework));
    if on_rework {
        for s in &mut stages {
            s.status = stage_status::PASSED.to_string();
            s.status_word = stage_status_word(stage_status::PASSED).to_string();
        }
    }
    // The current STAGE marker exists only when the position names a stage
    // (kind `stage` or `hold`), never for the rework loop.
    if position.kind == position_kind::OFF_GRAPH || on_rework {
        for s in &mut stages {
            if s.status == stage_status::CURRENT {
                s.status = stage_status::PENDING.to_string();
                s.status_word = stage_status_word(stage_status::PENDING).to_string();
            }
        }
    }
    position.line = position_line(&position);

    // --- waits: one row per declared gate -------------------------------
    let parked_question = f
        .questions
        .iter()
        .find(|q| q.status == QuestionStatus::Open && q.blocking);
    let waits: Vec<GraphWait> = axis
        .iter()
        .filter(|st| gate_nodes.contains_key(&st.state))
        .map(|st| {
            let node = &gate_nodes[&st.state];
            let state = &st.state;
            let doc = gate_document(f, state);
            let awaiting = position.node.as_deref() == Some(node.as_str());
            let signable = awaiting && doc.as_ref().is_some_and(|(_, current)| *current);
            let presentation = doc.map(|(a, current)| GraphPresentation {
                artifact_id: a.id.to_string(),
                display: artifact_display(f.artifact_titles, a),
                status: a.status.as_str().to_string(),
                current,
                note: (!current)
                    .then(|| "последний сохранённый документ, не действующий".to_string()),
            });
            let (requirement, hint) = gate_demand(f.wait_policy, f.task, state, parked_question);
            GraphWait {
                node: node.clone(),
                state: state.clone(),
                requirement,
                hint,
                awaiting,
                signable,
                presentation,
            }
        })
        .collect();

    // --- running roles ---------------------------------------------------
    let mut running_views: Vec<GraphRunning> = running
        .iter()
        .map(|e| {
            let in_path = f.process.states.iter().any(|s| {
                s.executor == process_executor::ROLE && s.role.as_deref() == Some(&e.role_slug)
            });
            GraphRunning {
                execution_id: e.id.to_string(),
                role_slug: e.role_slug.clone(),
                role_display: role_display(&roles, &e.role_slug, e.role_version),
                role_version: e.role_version,
                agent_name: e.agent_name.clone(),
                in_path,
                note: (!in_path).then(|| "прогон вне пути".to_string()),
            }
        })
        .collect();
    running_views.sort_by(|a, b| a.execution_id.cmp(&b.execution_id));
    for r in &running_views {
        if !r.in_path {
            notes.push(format!("прогон вне пути: {}", r.role_display));
        }
    }

    let blocking_questions = f
        .questions
        .iter()
        .filter(|q| q.status == QuestionStatus::Open && q.blocking)
        .count();

    let rework = ReworkLoop {
        rounds,
        active: on_rework,
        note: if rounds == 0 {
            "контура доработки не было".to_string()
        } else {
            format!("кругов доработки: {rounds}")
        },
    };

    RoleGraph {
        stages,
        nodes,
        edges,
        position,
        rework,
        running: running_views,
        waits,
        blocking_questions,
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

fn position_of(
    f: &GraphFacts<'_>,
    axis: &[&ProcessState],
    role_nodes: &HashMap<String, String>,
    gate_nodes: &HashMap<String, String>,
    node_labels: &HashMap<String, String>,
    roles: &HashMap<String, String>,
) -> GraphPosition {
    // A node's human signboard; falls back to the id only when the node is
    // somehow absent from `node_labels` (never produced by this fold).
    let place = |id: &str| -> String {
        node_labels
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    };
    // The declared stage's human label (a role name or the gate's demand).
    let stage_place = |state: &str| -> String {
        axis.iter()
            .find(|s| s.state == state)
            .map(|st| stage_label(f, st, roles))
            .unwrap_or_else(|| state.to_string())
    };
    let off = |reason: &str, line: &str| GraphPosition {
        kind: position_kind::OFF_GRAPH.to_string(),
        anchor: anchor_kind::STAGE.to_string(),
        stage: None,
        node: None,
        reason: Some(reason.to_string()),
        line: line.to_string(),
    };
    match f.task.state {
        TaskState::Draft => return off(off_graph_reason::DRAFT, "черновик — работа не начиналась"),
        TaskState::Cancelled => return off(off_graph_reason::CANCELLED, "задача отменена"),
        TaskState::Accepted => {
            return GraphPosition {
                kind: position_kind::NODE.to_string(),
                anchor: anchor_kind::FINAL.to_string(),
                stage: None,
                node: Some("final".to_string()),
                reason: None,
                line: "задача принята".to_string(),
            };
        }
        s if s.is_hold() => {
            let Some(rs) = f.task.return_state else {
                return off(
                    off_graph_reason::NO_RETURN_STATE,
                    "задача припаркована, сохранённого возврата нет",
                );
            };
            let stage = Some(rs.as_str().to_string());
            if let Some(slug) = f.process.role_for(rs.as_str()) {
                if let Some(id) = role_nodes.get(slug) {
                    return GraphPosition {
                        kind: position_kind::HOLD.to_string(),
                        anchor: anchor_kind::ROLE.to_string(),
                        stage,
                        node: Some(id.clone()),
                        reason: None,
                        line: format!("припаркована; место возврата — {}", place(id)),
                    };
                }
            }
            if is_gate_by_state(f.process, rs) {
                if let Some(id) = gate_nodes.get(rs.as_str()) {
                    return GraphPosition {
                        kind: position_kind::HOLD.to_string(),
                        anchor: anchor_kind::GATE.to_string(),
                        stage,
                        node: Some(id.clone()),
                        reason: None,
                        line: format!("припаркована; место возврата — гейт «{}»", place(id)),
                    };
                }
            }
            return GraphPosition {
                kind: position_kind::STAGE.to_string(),
                anchor: anchor_kind::STAGE.to_string(),
                stage: Some(rs.as_str().to_string()),
                node: None,
                reason: None,
                line: format!(
                    "припаркована; место возврата — этап «{}»",
                    stage_place(rs.as_str())
                ),
            };
        }
        _ => {}
    }
    // Active working states. Where BOTH a role and a human work (QA today),
    // the role is the position only while its run is going; otherwise the
    // human signature gate is (REQ-RG-03).
    let gate_state = is_gate_by_state(f.process, f.task.state);
    if let Some(slug) = f.process.role_for(f.task.state.as_str()) {
        let stage = Some(f.task.state.as_str().to_string());
        let audit_running = f
            .executions
            .iter()
            .any(|e| e.role_slug == slug && e.status == ExecutionStatus::Running);
        let prefer_role = !gate_state || audit_running;
        if prefer_role && let Some(id) = role_nodes.get(slug) {
            return GraphPosition {
                kind: position_kind::NODE.to_string(),
                anchor: anchor_kind::ROLE.to_string(),
                stage,
                node: Some(id.clone()),
                reason: None,
                line: format!("сейчас: {}", place(id)),
            };
        }
        if gate_state && let Some(id) = gate_nodes.get(f.task.state.as_str()) {
            return GraphPosition {
                kind: position_kind::NODE.to_string(),
                anchor: anchor_kind::GATE.to_string(),
                stage,
                node: Some(id.clone()),
                reason: None,
                line: format!("сейчас: гейт «{}»", place(id)),
            };
        }
        return GraphPosition {
            kind: position_kind::STAGE.to_string(),
            anchor: anchor_kind::STAGE.to_string(),
            stage,
            node: None,
            reason: None,
            line: format!("сейчас: этап «{}»", stage_place(f.task.state.as_str())),
        };
    }
    if is_gate_by_state(f.process, f.task.state) {
        let stage = Some(f.task.state.as_str().to_string());
        if let Some(id) = gate_nodes.get(f.task.state.as_str()) {
            return GraphPosition {
                kind: position_kind::NODE.to_string(),
                anchor: anchor_kind::GATE.to_string(),
                stage,
                node: Some(id.clone()),
                reason: None,
                line: format!("сейчас: гейт «{}»", place(id)),
            };
        }
    }
    // A declared state that is neither role, gate nor hold: the axis still
    // names the place (`_ = axis` keeps the unused-parameter contract).
    let _ = axis;
    GraphPosition {
        kind: position_kind::STAGE.to_string(),
        anchor: anchor_kind::STAGE.to_string(),
        stage: Some(f.task.state.as_str().to_string()),
        node: None,
        reason: None,
        line: format!("этап «{}»", stage_place(f.task.state.as_str())),
    }
}

fn position_line(p: &GraphPosition) -> String {
    p.line.clone()
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

    fn build_graph(f: &GraphFacts<'_>) -> RoleGraph {
        build(f)
    }

    fn facts<'a>(
        process: &'a Process,
        task: &'a Task,
        artifacts: &'a [Artifact],
        executions: &'a [RoleExecution],
        decisions: &'a [Decision],
        events: &'a [WorkflowEvent],
        questions: &'a [Question],
        roles: &'a [(String, String)],
        titles: &'a BTreeMap<String, String>,
        policy: &'a InboxWaitPolicy,
    ) -> GraphFacts<'a> {
        GraphFacts {
            process,
            task,
            artifacts,
            executions,
            decisions,
            events,
            questions,
            roles,
            artifact_titles: titles,
            wait_policy: policy,
        }
    }

    /// One current spec, no reviewer run: the analyst node is passed, both
    /// declared gates exist and are expected, nothing else is drawn. The
    /// position is the REVIEW stage (a stage, not a node) — REQ-RG-02/03.
    #[test]
    fn current_spec_makes_only_the_analyst_node() {
        let process = Process::factory();
        let t = task(TaskState::Review);
        let ex = execution(t.id, "requirements-analyst", 6, ExecutionStatus::Completed);
        let spec = artifact(
            t.id,
            ArtifactKind::RequirementsSpec,
            1,
            ArtifactStatus::Current,
            Some(ex.id),
        );
        let arts = vec![spec];
        let execs = vec![ex];
        let roles = roles();
        let titles = titles();
        let policy = policy();
        let f = facts(
            process,
            &t,
            &arts,
            &execs,
            &[],
            &[],
            &[],
            &roles,
            &titles,
            &policy,
        );
        let g = build_graph(&f);

        let role_nodes: Vec<&GraphNode> = g
            .nodes
            .iter()
            .filter(|n| n.kind == node_kind::ROLE)
            .collect();
        assert_eq!(role_nodes.len(), 1);
        assert_eq!(
            role_nodes[0].role_slug.as_deref(),
            Some("requirements-analyst")
        );
        assert_eq!(role_nodes[0].status, node_status::PASSED);
        let gates: Vec<&GraphNode> = g
            .nodes
            .iter()
            .filter(|n| matches!(n.kind.as_str(), node_kind::GATE))
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
        let process = Process::factory();
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
        let review1 = artifact(
            t.id,
            ArtifactKind::ReviewReport,
            1,
            ArtifactStatus::Stale,
            Some(rev1.id),
        );
        let review2 = artifact(
            t.id,
            ArtifactKind::ReviewReport,
            2,
            ArtifactStatus::Current,
            Some(rev2.id),
        );
        let execs = vec![analyst, rev1, rev2];
        let arts = vec![spec, review1, review2];
        let roles = roles();
        let titles = titles();
        let policy = policy();
        let f = facts(
            process,
            &t,
            &arts,
            &execs,
            &[],
            &[],
            &[],
            &roles,
            &titles,
            &policy,
        );
        let g = build_graph(&f);
        let reviewer = g
            .nodes
            .iter()
            .find(|n| n.role_slug.as_deref() == Some("requirements-reviewer"))
            .unwrap();
        assert_eq!(reviewer.executions.len(), 2);
        assert_eq!(reviewer.role_version, Some(4));
        assert!(reviewer.revoked);
        assert_eq!(reviewer.artifacts.len(), 2);
        // Approval gate is the position and pending its signature.
        assert_eq!(g.position.node.as_deref(), Some("gate:APPROVAL"));
        assert!(g.waits.iter().any(|w| w.awaiting && w.signable));
    }

    /// A running role outside the declared process is a node with the note
    /// «прогон вне пути» and never an edge; an artifact without an execution
    /// creates no node at all (REQ-RG-02 AC3).
    #[test]
    fn out_of_path_run_is_a_note_and_an_artifact_alone_is_no_node() {
        let process = Process::factory();
        let t = task(TaskState::Analysis);
        let mut out = execution(t.id, "project-intelligence", 1, ExecutionStatus::Running);
        out.inputs = vec![];
        let loose = artifact(t.id, ArtifactKind::Patch, 1, ArtifactStatus::Current, None);
        let execs = vec![out];
        let arts = vec![loose];
        let roles = roles();
        let titles = titles();
        let policy = policy();
        let f = facts(
            process,
            &t,
            &arts,
            &execs,
            &[],
            &[],
            &[],
            &roles,
            &titles,
            &policy,
        );
        let g = build_graph(&f);
        assert!(g.running.iter().any(|r| !r.in_path && r.note.is_some()));
        assert!(g.notes.iter().any(|n| n.contains("прогон вне пути")));
        let out_node = g
            .nodes
            .iter()
            .find(|n| n.role_slug.as_deref() == Some("project-intelligence"));
        assert!(out_node.is_some());
        assert!(
            g.edges
                .iter()
                .all(|e| e.from != out_node.unwrap().id && e.to != out_node.unwrap().id)
        );
        // The loose artifact produced no node: only the out-of-path one exists.
        assert_eq!(
            g.nodes.iter().filter(|n| n.kind == node_kind::ROLE).count(),
            1
        );
    }

    /// Signed requirements with no implementation run yet: the position is
    /// the IMPLEMENTATION stage (no «сейчас» node) and NO signature edge is
    /// drawn into a role that has not started (REQ-RG-04 AC3, REQ-RG-03 AC2).
    #[test]
    fn signature_without_implementer_gives_stage_position_and_no_edge() {
        let process = Process::factory();
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
        let execs = vec![analyst];
        let arts = vec![spec];
        let decs = vec![dec];
        let roles = roles();
        let titles = titles();
        let policy = policy();
        let f = facts(
            process,
            &t,
            &arts,
            &execs,
            &decs,
            &[],
            &[],
            &roles,
            &titles,
            &policy,
        );
        let g = build_graph(&f);
        assert_eq!(g.position.kind, position_kind::STAGE);
        assert_eq!(g.position.stage.as_deref(), Some("IMPLEMENTATION"));
        assert_eq!(g.position.node, None);
        assert!(
            g.edges
                .iter()
                .all(|e| e.to != role_id("implementation-agent"))
        );
        let gate = g.nodes.iter().find(|n| n.id == "gate:APPROVAL").unwrap();
        assert_eq!(gate.status, node_status::PASSED);
    }

    /// Two stored returns: two edges with rounds 1 and 2, the loop survives
    /// into QA (not active there) — REQ-RG-05.
    #[test]
    fn rework_rounds_are_counted_from_stored_returns() {
        let process = Process::factory();
        let t = task(TaskState::Qa);
        let analyst = execution(t.id, "requirements-analyst", 6, ExecutionStatus::Completed);
        let impl_exec = execution(t.id, "implementation-agent", 8, ExecutionStatus::Completed);
        let audit = execution(t.id, "quality-auditor", 7, ExecutionStatus::Completed);
        let spec = artifact(
            t.id,
            ArtifactKind::RequirementsSpec,
            1,
            ArtifactStatus::Current,
            Some(analyst.id),
        );
        let report = artifact(
            t.id,
            ArtifactKind::ImplementationReport,
            1,
            ArtifactStatus::Current,
            Some(impl_exec.id),
        );
        let qa = artifact(
            t.id,
            ArtifactKind::QualityReport,
            1,
            ArtifactStatus::Current,
            Some(audit.id),
        );
        let events = vec![
            event(t.id, event_kind::QUALITY_RETURNED, serde_json::json!({})),
            event(t.id, event_kind::QUALITY_RETURNED, serde_json::json!({})),
        ];
        let execs = vec![analyst, impl_exec, audit];
        let arts = vec![spec, report, qa];
        let roles = roles();
        let titles = titles();
        let policy = policy();
        let f = facts(
            process,
            &t,
            &arts,
            &execs,
            &[],
            &events,
            &[],
            &roles,
            &titles,
            &policy,
        );
        let g = build_graph(&f);
        assert_eq!(g.rework.rounds, 2);
        assert!(!g.rework.active);
        let mut rounds: Vec<u32> = g
            .edges
            .iter()
            .filter(|e| e.kind == edge_kind::RETURN)
            .filter_map(|e| e.round)
            .collect();
        rounds.sort();
        assert_eq!(rounds, vec![1, 2]);
        assert!(g.edges.iter().any(|e| e.kind == edge_kind::RETURN
            && e.from == "gate:QA"
            && e.to == role_id("implementation-agent")));
    }

    /// The blocking-question count is the question plane's fact, independent
    /// of the position; a draft has no position current and all stages pending.
    #[test]
    fn blocking_questions_are_counted_from_stored_open_rows() {
        let process = Process::factory();
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
        let execs = vec![analyst];
        let arts = vec![spec];
        let roles = roles();
        let titles = titles();
        let policy = policy();
        let f = facts(
            process,
            &t,
            &arts,
            &execs,
            &[],
            &[],
            &qs,
            &roles,
            &titles,
            &policy,
        );
        let g = build_graph(&f);
        assert_eq!(g.blocking_questions, 1);

        let draft = task(TaskState::Draft);
        let f = facts(
            process,
            &draft,
            &[],
            &[],
            &[],
            &[],
            &[],
            &roles,
            &titles,
            &policy,
        );
        let g = build_graph(&f);
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
        let process = Process::factory();
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
        let execs = vec![audit];
        let arts = vec![qa];
        let events = vec![accepted.clone()];
        let roles = roles();
        let titles = titles();
        let policy = policy();
        let f = facts(
            process,
            &t,
            &arts,
            &execs,
            &[],
            &events,
            &[],
            &roles,
            &titles,
            &policy,
        );
        let g = build_graph(&f);
        assert_eq!(g.position.node.as_deref(), Some("final"));
        let current: Vec<&str> = g
            .nodes
            .iter()
            .filter(|n| n.status == node_status::CURRENT)
            .map(|n| n.id.as_str())
            .collect();
        assert_eq!(current, vec!["final"]);
        let final_node = g.nodes.iter().find(|n| n.id == "final").unwrap();
        assert_eq!(final_node.kind, node_kind::FINAL);
        assert!(final_node.executions.is_empty());
        assert_eq!(final_node.role_slug, None);
        // Gates: declaration-born, never a role carrier.
        for n in g.nodes.iter().filter(|n| n.kind == node_kind::GATE) {
            assert_eq!(n.role_slug, None);
            assert_eq!(n.role_version, None);
            assert!(n.executions.is_empty());
        }
        // REQ-RG-02: acceptance is the affirmative human act that PASSES the
        // quality gate — it is not left «ожидается» on a finished task.
        let qa_gate = g.nodes.iter().find(|n| n.id == "gate:QA").unwrap();
        assert_eq!(qa_gate.status, node_status::PASSED);
        let approval_gate = g.nodes.iter().find(|n| n.id == "gate:APPROVAL").unwrap();
        assert_eq!(approval_gate.status, node_status::EXPECTED);
        // REQ-RG-01 AC1: the waits follow the DECLARED axis order (never a
        // hash-map iteration), so two reads of unchanged facts agree.
        let order: Vec<&str> = g.waits.iter().map(|w| w.state.as_str()).collect();
        assert_eq!(order, vec!["APPROVAL", "QA"]);
        let again = build_graph(&facts(
            f.process,
            f.task,
            &arts,
            &execs,
            &[],
            &events,
            &[],
            &roles,
            &titles,
            &policy,
        ));
        let order_again: Vec<&str> = again.waits.iter().map(|w| w.state.as_str()).collect();
        assert_eq!(order, order_again);
        assert!(g.edges.iter().any(|e| e.kind == edge_kind::ACCEPTANCE
            && e.from == "gate:QA"
            && e.to == "final"
            && e.carrier.id == accepted.id.to_string()));
    }

    /// REQ-RG-03 AC4: an accepted task without a stored acceptance act says so
    /// in a note instead of drawing an edge to nowhere.
    #[test]
    fn accepted_task_without_stored_act_reports_a_data_mismatch() {
        let process = Process::factory();
        let t = task(TaskState::Accepted);
        let roles = roles();
        let titles = titles();
        let policy = policy();
        let f = facts(
            process,
            &t,
            &[],
            &[],
            &[],
            &[],
            &[],
            &roles,
            &titles,
            &policy,
        );
        let g = build_graph(&f);
        assert!(g.notes.iter().any(|n| n.contains("расхождение данных")));
        assert!(g.edges.iter().all(|e| e.kind != edge_kind::ACCEPTANCE));
    }

    /// REQ-RG-03 AC3: a hold returning to the approval gate stands ON that
    /// gate, and the gate keeps both its signature state and its presentation.
    #[test]
    fn hold_on_the_approval_gate_keeps_signature_and_presentation() {
        let process = Process::factory();
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
        let execs = vec![analyst];
        let arts = vec![spec];
        let roles = roles();
        let titles = titles();
        let policy = policy();
        let f = facts(
            process,
            &t,
            &arts,
            &execs,
            &[],
            &[],
            &[],
            &roles,
            &titles,
            &policy,
        );
        let g = build_graph(&f);
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
        let process = Process::factory();
        let mut t = task(TaskState::WaitingForInput);
        t.return_state = Some(TaskState::Analysis);
        let roles = roles();
        let titles = titles();
        let policy = policy();
        let f = facts(
            process,
            &t,
            &[],
            &[],
            &[],
            &[],
            &[],
            &roles,
            &titles,
            &policy,
        );
        let g = build_graph(&f);
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
        let process = Process::factory();
        let t = task(TaskState::Review);
        let analyst = execution(t.id, "requirements-analyst", 6, ExecutionStatus::Completed);
        let spec = artifact(
            t.id,
            ArtifactKind::RequirementsSpec,
            1,
            ArtifactStatus::Superseded,
            Some(analyst.id),
        );
        let execs = vec![analyst];
        let arts = vec![spec];
        let roles = roles();
        let titles = titles();
        let policy = policy();
        let f = facts(
            process,
            &t,
            &arts,
            &execs,
            &[],
            &[],
            &[],
            &roles,
            &titles,
            &policy,
        );
        let g = build_graph(&f);
        let node = g
            .nodes
            .iter()
            .find(|n| n.role_slug.as_deref() == Some("requirements-analyst"))
            .unwrap();
        assert_eq!(node.status, node_status::EXPECTED);
        assert!(node.revoked);
        assert_eq!(node.artifacts.len(), 1);
    }

    /// REQ-NM-01 AC4: the ONE heading rule is «название вида · vN», checked on
    /// versions 1..3 of one kind; the stored name never becomes the heading.
    #[test]
    fn artifact_heading_is_the_declared_title_and_version() {
        let process = Process::factory();
        let t = task(TaskState::Review);
        let analyst = execution(t.id, "requirements-analyst", 6, ExecutionStatus::Completed);
        let arts = vec![
            artifact(
                t.id,
                ArtifactKind::RequirementsSpec,
                1,
                ArtifactStatus::Superseded,
                Some(analyst.id),
            ),
            artifact(
                t.id,
                ArtifactKind::RequirementsSpec,
                2,
                ArtifactStatus::Stale,
                Some(analyst.id),
            ),
            artifact(
                t.id,
                ArtifactKind::RequirementsSpec,
                3,
                ArtifactStatus::Current,
                Some(analyst.id),
            ),
        ];
        let execs = vec![analyst];
        let roles = roles();
        let titles = titles();
        let policy = policy();
        let f = facts(
            process,
            &t,
            &arts,
            &execs,
            &[],
            &[],
            &[],
            &roles,
            &titles,
            &policy,
        );
        let g = build_graph(&f);
        let node = g
            .nodes
            .iter()
            .find(|n| n.role_slug.as_deref() == Some("requirements-analyst"))
            .unwrap();
        let displays: Vec<&str> = node.artifacts.iter().map(|a| a.display.as_str()).collect();
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
