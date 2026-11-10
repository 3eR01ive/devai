// Фикстуры секции ожидания для компонентных тестов:
// ровно те поля, что публикует `inbox_view` (WaitSnapshot, types.ts).
import type {
  AgentFailureView,
  AgentStatusView,
  Artifact,
  Question,
  QualityReportView,
  SpecPresentation,
  Task,
  WaitNote,
  WaitRow,
  WaitSnapshot,
} from "../types";

export function task(id: string, state: string, title = `task ${id}`): Task {
  return {
    id,
    project_id: null,
    title,
    // Человеческое имя после аналитика и его витрина (REQ-TN): в фикстуре
    // имя не задано — задача ещё без подписи, витрина совпадает с title.
    human_name: null,
    display: title,
    business_description: "бизнес-описание",
    state,
    return_state: null,
    requirements_seq: 2,
    lock_version: 1,
    created_at: "2026-11-06T10:00:00Z",
    updated_at: "2026-11-06T11:00:00Z",
  };
}

export function artifact(id: string, seq = 2): Artifact {
  return {
    id,
    task_id: "t1",
    kind: "REQUIREMENTS_SPEC",
    name: "requirements-spec",
    seq,
    category: "requirements",
    status: "SUPERSEDED",
    sha256: "0".repeat(64),
    size: 100,
    media_type: "application/json",
    contract: { name: "requirements-spec", version: 3 },
    created_by: "analyst",
    execution_id: null,
    depends_on: [],
    created_at: "2026-11-06T10:30:00Z",
  };
}

export function question(id: string, overrides: Partial<Question> = {}): Question {
  return {
    id,
    task_id: "t1",
    execution_id: null,
    text: `вопрос ${id}`,
    context: "контекст вопроса",
    options: ["да", "нет"],
    blocking: true,
    status: "OPEN",
    answer: null,
    answered_by: null,
    answered_at: null,
    created_at: "2026-11-06T11:00:00Z",
    ...overrides,
  };
}

export function spec(overrides: Partial<SpecPresentation> = {}): SpecPresentation {
  return {
    artifact: artifact("a-1"),
    current: true,
    human_paragraph: "человеческий абзац спеки",
    paragraph_source: "spec",
    open_questions: ["OQ-1: первый вопрос документа", "OQ-2: второй вопрос документа"],
    unavailable_note: null,
    ...overrides,
  };
}

export function report(overrides: Partial<QualityReportView> = {}): QualityReportView {
  return {
    task: "t1",
    artifact: "a-qa",
    seq: 1,
    contract: "quality-report@1",
    created_at: "2026-11-06T11:00:00Z",
    execution: null,
    // @5: статус строки, источник человеческой формы и факт свежести —
    // фикстура изображает свежий CURRENT-отчёт с плоским текстом.
    status: "CURRENT",
    source: "flat_text",
    freshness: null,
    report: { score: 0.7 },
    report_text: "человеческий отчёт качества",
    ...overrides,
  };
}

export function row(task: Task, overrides: Partial<WaitRow> = {}): WaitRow {
  return {
    task,
    demand: `demand for ${task.state}`,
    cli_hint: `devai task ... ${task.id}`,
    open_questions: [],
    spec: null,
    report: null,
    report_note: null,
    convergence: null,
    // Отказ агента отсутствует в фикстуре по умолчанию (задача 01a1b60c).
    agent_failure: null,
    ...overrides,
  };
}

export function note(task: Task, q: Question): WaitNote {
  return { task, question: q };
}

/// Строка статуса агента (задача 01a1b60c, REQ-AF-07): одна фраза платформы
/// приходит целиком в `line`, окно её не собирает.
export function agentStatus(overrides: Partial<AgentStatusView> = {}): AgentStatusView {
  return {
    name: "hermes",
    driver: "hermes",
    llm_provider: "deepseek",
    model: "deepseek-v4",
    availability: "unavailable",
    availability_word: "недоступен",
    last_class: "provider_model",
    last_class_word: "модель исчезла у провайдера",
    last_failure_at: "2026-11-10T12:07:00Z",
    last_outcome_at: "2026-11-10T12:07:00Z",
    last_seen_at: "2026-11-10T12:07:00Z",
    failures: 5,
    series: 3,
    series_first_at: "2026-11-10T12:00:00Z",
    line: "hermes: недоступен, модель исчезла у провайдера, 5 отказов с 10.11 12:00",
    ...overrides,
  };
}

/// Факт отказа агента (задача 01a1b60c, REQ-AF-08): хвост ограничен, полный
/// текст остаётся в строке выполнения.
export function agentFailure(
  overrides: Partial<AgentFailureView> = {},
): AgentFailureView {
  return {
    agent: "hermes",
    class: "provider_model",
    class_word: "модель исчезла у провайдера",
    action: "перевыпустить агента",
    series_count: 3,
    threshold: 3,
    window_count: 1,
    total_failures: 3,
    first_at: "2026-11-10T12:00:00Z",
    last_at: "2026-11-10T12:07:00Z",
    parked: true,
    exit_code: 2,
    duration_ms: 900,
    tail: "404 Model not exist",
    output_empty: false,
    driver_log_tail: null,
    driver_log_note: "журнал драйвера за прогон пуст",
    message: "`hermes` exited with exit status: 2",
    basis: "text:model not exist",
    ...overrides,
  };
}

export function snapshot(overrides: Partial<WaitSnapshot> = {}): WaitSnapshot {
  return { waiting: [], human_stopped: [], notes: [], badge: 0, ...overrides };
}
