// Единственная дверь фронта в данные: invoke к Rust-командам devai-ui.
// Каждая команда — ровно одна опубликованная read-операция реестра API
// (REQ-UI-02); список команд зеркалит COMMAND_OPERATIONS в src/commands.rs.
//
// Два транспорта, ОДНА дверь (task 01a1be98, REQ-LAN-05): в окне вызов идёт
// через Tauri IPC, на телефоне — через POST /rpc того же процесса, который
// диспетчеризует РОВНО те же команды (src/lan.rs → dispatch_rpc). Телефон не
// получает отдельного набора прав и отдельной реализации чтения/записи.
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type {
  RulebookView,
  TaskUsageView,
  SourceSliceView,
  ActiveSnapshot,
  AgentStatusView,
  Artifact,
  ArtifactContent,
  ArtifactView,
  BundleFormInput,
  BundleHandleView,
  BundleMutationView,
  Decision,
  DependenciesView,
  ExecutionView,
  HomeRecord,
  HomeRecordsView,
  LanStatus,
  LivenessView,
  ModelSnapshot,
  OpDescriptor,
  RpcRequest,
  RpcResponse,
  ProbeView,
  ProgressView,
  Project,
  Question,
  QualityReportView,
  RunStartOutcome,
  SessionInfo,
  SignatureView,
  StatusView,
  SwitchResultView,
  Task,
  UiError,
  WaitSnapshot,
  WorkflowEvent,
  WorkspaceChangesView,
  WorkspaceDiffView,
} from "./types";

// Внутри Tauri webview есть мост; в браузере телефона его нет, и дверью
// становится HTTP-поверхность того же окна. Всё остальное (окно, а также
// node-харнесс фронтовых тестов, где нет ни window, ни document) идёт через
// IPC-транспорт: браузер — это именно страница с document и fetch.
const hasTauriBridge = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const isBrowserPage =
  typeof window !== "undefined" &&
  typeof document !== "undefined" &&
  typeof fetch === "function";
// Страницу отдал сам телефонный вход (`GET /` у lan::http) — она и в окне
// приложения обязана говорить по HTTP: в окне существуют ОБА моста, и догадка
// «по origin» тут ошибается (task 01a1bf78, REQ-QRAPP-02 AC5). Признак ставит
// сервер, который эту страницу отдал.
const lanShell =
  typeof window !== "undefined" &&
  (window as unknown as { __DEVAI_LAN__?: boolean }).__DEVAI_LAN__ === true;
const useRpc = lanShell || (isBrowserPage && !hasTauriBridge);

/// Сторож связи с домом нужен только странице, которую отдала машина: и в
/// браузере, и в окне приложения это ОДНА И ТА ЖЕ страница — она сама спрашивает
/// `/pair` (native-половине сеть запрещена гейтом `no-network`, задача
/// 01a1bf78). Локальная страница приложения (`pairingShell`) дома не показывает,
/// ей сторож не нужен.
export const pageWatchesPairing = useRpc;

/// Это СВОЯ страница приложения (tauri.localhost), а не отданная машиной: у
/// неё есть мост в Rust, и только она проводит приглашение (см. pairingNative).
/// Машинная страница — чужой origin, моста у неё нет по построению Tauri.
export const pairingShell = hasTauriBridge && !lanShell;

/// POST /rpc: сервер уже знает сессию по HttpOnly-cookie, поэтому токена в
/// JS нет вовсе (он живёт в адресе ровно один раз — при сканировании).
async function rpc<T>(command: string, params?: Record<string, unknown>): Promise<T> {
  const request: RpcRequest = { command, params: params ?? {} };
  let response: Response;
  try {
    response = await fetch("./rpc", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    });
  } catch {
    // Оборванная связь — это либо «отключить» на машине, либо смена сети
    // (REQ-LAN-08 AC2): честное сообщение вместо вечного ожидания.
    throw {
      code: "lan_unreachable",
      message: "связь с машиной потеряна — доступ выключен или сменилась сеть",
    };
  }
  if (response.status === 401 || response.status === 403) {
    throw {
      code: "lan_unauthorized",
      message: "доступ с телефона закрыт — откройте страницу по новому коду",
    };
  }
  const body = (await response.json()) as RpcResponse<T>;
  if (!body.ok || body.error) {
    throw body.error ?? { code: "ipc", message: "пустой ответ машины" };
  }
  return body.data as T;
}

// Один и тот же вызов на обоих транспортах. Имя объявлено без generic-скобки:
// тест src/commands.rs сканирует api.ts по вызовам-точкам и должен видеть
// ровно реальные вызовы команд, а не объявление обёртки.
//
// Экспортируется для `pairingNative.ts`: приглашение — не чтение данных, но
// IPC-дверь остаётся одной (выбор транспорта и адрес машины решаются здесь).
export const invoke = <T,>(command: string, params?: Record<string, unknown>): Promise<T> =>
  useRpc ? rpc<T>(command, params) : tauriInvoke<T>(command, params);

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
  // Секция подписи спеки на гейте APPROVAL (REQ-SIGN-07): один read на всю
  // карточку и две мутации — те же акты платформы, что `devai task
  // approve/reject` (REQ-SIGN-05).
  taskSignature: (id_prefix: string) =>
    invoke<SignatureView>("task_signature", { id_prefix }),
  taskApprove: (id_prefix: string, rationale: string | null) =>
    invoke<SignatureView>("task_approve", { id_prefix, rationale }),
  taskReject: (id_prefix: string, rationale: string | null) =>
    invoke<SignatureView>("task_reject", { id_prefix, rationale }),
  // Гейт качества (REQ-QG-01): те же операции, что `devai task accept|return`;
  // ответ — ПЕРЕЧИТАННЫЙ статус с публичным `quality_gate`, локального патча нет.
  taskAccept: (id_prefix: string, rationale: string | null) =>
    invoke<StatusView>("task_accept", { id_prefix, rationale }),
  taskReturn: (id_prefix: string, rationale: string | null) =>
    invoke<StatusView>("task_return", { id_prefix, rationale }),
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
  // тот же diff с полным контекстом файла: по нему окно разворачивает
  // скрытые строки, не пересчитывая git
  taskWorkspaceDiffFull: (id_prefix: string, path: string) =>
    invoke<WorkspaceDiffView>("task_workspace_diff_full", { id_prefix, path }),

  artifactsList: (id_prefix: string) =>
    invoke<ArtifactView[]>("artifacts_list", { id_prefix }),
  artifactContent: (artifact_id: string) =>
    invoke<ArtifactContent>("artifact_content", { artifact_id }),

  questionsList: (id_prefix: string) =>
    invoke<Question[]>("questions_list", { id_prefix }),
  // Ответ на вопрос платформы — тот же акт, что `devai question answer`.
  // Возвращается ПЕРЕЧИТАННЫЙ статус: карточка не собирает состояние сама.
  questionAnswer: (id_prefix: string, question_id: string, text: string) =>
    invoke<StatusView>("question_answer", { id_prefix, question_id, text }),
  // Отмена задачи — тот же акт, что `devai task cancel`: причина и автор
  // остаются в истории, строка не исчезает.
  taskCancel: (id_prefix: string, reason: string | null) =>
    invoke<StatusView>("task_cancel", { id_prefix, reason }),
  // Возврат в работу: тот же акт, что `devai task resume` — платформа помнит
  // стадию, на которой дело закрыли.
  taskResume: (id_prefix: string) =>
    invoke<StatusView>("task_resume", { id_prefix }),
  // Пройти путь заново с шага и переписать постановку: акты человека, причина
  // обязательна — журнал должен объяснять, зачем путь пройден дважды.
  taskRewind: (id_prefix: string, to: string, reason: string) =>
    invoke<StatusView>("task_rewind", { id_prefix, to, reason }),
  taskRestate: (id_prefix: string, title: string | null, description: string | null, reason: string) =>
    invoke<StatusView>("task_restate", { id_prefix, title, description, reason }),
  // Строки статуса агентов (задача 01a1b60c, REQ-AF-07): одна фраза платформы
  // на агента — окно ничего не собирает и не перефразирует.
  agentStatuses: () => invoke<AgentStatusView[]>("agent_statuses"),
  executionsList: (task_ref: string | null) =>
    invoke<ExecutionView[]>("executions_list", { task_ref }),
  // Перезапуск мёртвого исполнения (REQ-LV-05 AC3): та же операция платформы,
  // что `devai task run` — она сама снимает доказанно мёртвого владельца и
  // запускает по обычным правилам. Ответ несёт готовую фразу `line`.
  taskRun: (id_prefix: string, agent: string | null, force_reaudit: boolean) =>
    invoke<RunStartOutcome>("task_run", { id_prefix, agent, force_reaudit }),
  // Остановка живого прогона: платформа закрывает исполнение (exec.cancel),
  // статус и причина остаются в истории.
  // Правила оценки дома: то, чем судит качество (профиль оценки как данные).
  evaluationRules: () => invoke<RulebookView>("evaluation_rules"),
  // Расход токенов задачи по прогонам (файлы улик драйвера).
  taskUsage: (id_prefix: string) => invoke<TaskUsageView>("task_usage", { id_prefix }),
  // Кусок исходника на коммите по ссылке «путь:строки».
  sourceSlice: (id_prefix: string, rev: string, path: string, from: number, to: number) =>
    invoke<SourceSliceView>("source_slice", { id_prefix, rev, path, from, to }),
  execCancel: (exec_id: string) =>
    invoke<ExecutionView>("execution_cancel", { exec_id }),
  executionLiveness: (exec_id: string) =>
    invoke<LivenessView>("execution_liveness", { exec_id }),

  // Секция ожидания человека (REQ-GATE-01..10): один снимок — строки
  // гейтов, остановленные, справочные заметки и бейдж на нём же.
  inboxView: () => invoke<WaitSnapshot>("inbox_view"),

  // Раздел связок модели (REQ-MS-07): ОДИН read на весь экран и две
  // записи — проба и переключение. Те же операции, что `devai model …`.
  modelSnapshot: () => invoke<ModelSnapshot>("model_snapshot"),
  modelProbe: (bundle_id: string) =>
    invoke<ProbeView>("model_probe", { bundle_id }),
  modelSwitch: (bundle_id: string, reason: string) =>
    invoke<SwitchResultView>("model_switch", { bundle_id, reason }),
  // Авторская поверхность связок (REQ-MC-01..09): идентификатор новой
  // связки считает СЕРВЕР (ASM-9), проверка черновика и три записи в тот
  // же файл, который читает консоль. Payload — те же имена полей
  // объявления, никаких ключей/адресов сервиса.
  modelBundleHandle: (label: string) =>
    invoke<BundleHandleView>("model_bundle_handle", { label }),
  modelBundleCheck: (input: BundleFormInput) =>
    invoke<ProbeView>("model_bundle_check", { input }),
  modelBundleCreate: (input: BundleFormInput) =>
    invoke<BundleMutationView>("model_bundle_create", { input }),
  modelBundleUpdate: (input: BundleFormInput) =>
    invoke<BundleMutationView>("model_bundle_update", { input }),
  modelBundleDelete: (bundle_id: string, file_revision: string) =>
    invoke<BundleMutationView>("model_bundle_delete", { bundle_id, file_revision }),

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

  // --- доступ с телефона (task 01a1be98, REQ-LAN-01…09) ---
  // Один снимок состояния + управляющие действия. Те же команды
  // диспетчеризуются по HTTP, поэтому панель работает и на самом телефоне.
  lanStatus: () => invoke<LanStatus>("lan_status"),
  lanEnable: () => invoke<LanStatus>("lan_enable"),
  lanDisable: () => invoke<LanStatus>("lan_disable"),
  lanNewInvite: () => invoke<LanStatus>("lan_new_invite"),
  lanDisconnect: (id: string) => invoke<LanStatus>("lan_disconnect", { id }),
};

export function toUiError(e: unknown): UiError {
  if (e && typeof e === "object" && "code" in e && "message" in e) {
    return e as UiError;
  }
  return { code: "ipc", message: String(e) };
}
