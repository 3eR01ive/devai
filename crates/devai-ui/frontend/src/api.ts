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
  HomeRecord,
  HomeRecordsView,
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
  WaitSnapshot,
  WorkflowEvent,
  WorkspaceChangesView,
  WorkspaceDiffView,
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
  taskWorkspaceChanges: (id_prefix: string) =>
    invoke<WorkspaceChangesView>("task_workspace_changes", { id_prefix }),
  taskWorkspaceDiff: (id_prefix: string, path: string) =>
    invoke<WorkspaceDiffView>("task_workspace_diff", { id_prefix, path }),

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

  // Секция ожидания человека (REQ-GATE-01..10): один снимок — строки
  // гейтов, остановленные, справочные заметки и бейдж на нём же.
  inboxView: () => invoke<WaitSnapshot>("inbox_view"),

  // --- homes registry (REQ-HOME-01…09) ---
  // Чтение: home.list / home.projects_all / home.tasks_all (Read-плоскость).
  // Управление реестром: home.add / home.set_enabled / home.activate
  // (Registry-плоскость, пишет только homes.json контрольного дома).
  homesList: () => invoke<HomeRecord[]>("homes_list"),
  homesAdd: (path: string) => invoke<HomeRecord>("homes_add", { path }),
  homesSetEnabled: (path: string, enabled: boolean) =>
    invoke<HomeRecord[]>("homes_set_enabled", { path, enabled }),
  homesActivate: (path: string) => invoke<SessionInfo>("homes_activate", { path }),
  projectsAll: (home: string | null) =>
    invoke<HomeRecordsView<Project>[]>("projects_all", { home }),
  tasksAll: (home: string | null) =>
    invoke<HomeRecordsView<Task>[]>("tasks_all", { home }),
};

export function toUiError(e: unknown): UiError {
  if (e && typeof e === "object" && "code" in e && "message" in e) {
    return e as UiError;
  }
  return { code: "ipc", message: String(e) };
}
