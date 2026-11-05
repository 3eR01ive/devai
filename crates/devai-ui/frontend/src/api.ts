// Единственная дверь фронта в данные: invoke к Rust-командам devai-ui.
// Каждая команда — ровно одна опубликованная read-операция реестра API
// (REQ-UI-02); список команд зеркалит COMMAND_OPERATIONS в src/commands.rs.
import { invoke } from "@tauri-apps/api/core";
import type {
  ActiveSnapshot,
  Artifact,
  ArtifactContent,
  Decision,
  DependenciesView,
  InboxEntry,
  LivenessView,
  OpDescriptor,
  ProgressView,
  Project,
  Question,
  QualityReportView,
  RoleExecution,
  SessionInfo,
  StatusView,
  Task,
  UiError,
  WorkflowEvent,
} from "./types";

export const api = {
  sessionInfo: () => invoke<SessionInfo>("session_info"),
  registryOperations: () => invoke<OpDescriptor[]>("registry_operations"),

  projectsList: () => invoke<Project[]>("projects_list"),
  projectGet: (id_or_name: string) =>
    invoke<Project | null>("projects_get", { id_or_name }),

  tasksList: () => invoke<Task[]>("tasks_list"),
  taskGet: (id_prefix: string) => invoke<Task>("task_get", { id_prefix }),
  taskStatus: (id_prefix: string) =>
    invoke<StatusView>("task_status", { id_prefix }),
  taskProgress: (id_prefix: string) =>
    invoke<ProgressView>("task_progress", { id_prefix }),
  taskDependencies: (id_prefix: string) =>
    invoke<DependenciesView>("task_dependencies", { id_prefix }),
  taskReport: (id_prefix: string) =>
    invoke<QualityReportView>("task_report", { id_prefix }),
  taskEvents: (id_prefix: string, limit: number | null) =>
    invoke<WorkflowEvent[]>("task_events", { id_prefix, limit }),
  taskDecisions: (id_prefix: string) =>
    invoke<Decision[]>("task_decisions", { id_prefix }),
  taskActive: () => invoke<ActiveSnapshot>("task_active"),

  artifactsList: (id_prefix: string) =>
    invoke<Artifact[]>("artifacts_list", { id_prefix }),
  artifactContent: (artifact_id: string) =>
    invoke<ArtifactContent>("artifact_content", { artifact_id }),

  questionsList: (id_prefix: string) =>
    invoke<Question[]>("questions_list", { id_prefix }),
  executionsList: (task_ref: string | null) =>
    invoke<RoleExecution[]>("executions_list", { task_ref }),
  executionLiveness: (exec_id: string) =>
    invoke<LivenessView>("execution_liveness", { exec_id }),

  inboxView: () => invoke<InboxEntry[]>("inbox_view"),
};

export function toUiError(e: unknown): UiError {
  if (e && typeof e === "object" && "code" in e && "message" in e) {
    return e as UiError;
  }
  return { code: "ipc", message: String(e) };
}
