// Проведение приглашения в приложении (task 01a1bf78, REQ-QRAPP-02/04/06).
//
// Почему это работает именно здесь: машина отдаёт СВОЮ страницу, а у чужого
// origin нет моста в Rust (Tauri даёт IPC только своему). Поэтому сканирование
// сначала поднимает СВОЮ страницу приложения, она спрашивает Rust «что делать»
// (`pair_boot`), сама задаёт машине один дешёвый вопрос `GET /pair` (сеть из
// Rust запрещена гейтом `no-network`), докладывает статус (`pair_report`) — и
// ОКНО открывает готовый адрес (навигацией самого окна, не этой страницы:
// второй загрузки быть не должно, иначе одноразовый код сгорит дважды).
// Ни адресов, ни причин, ни решений этот модуль не изобретает: всё это
// приходит значением из Rust (единый источник).
import { invoke } from "./api";

/// Параметр адреса, которым Rust сообщает заметку (см. `pair::with_notice`).
export const NOTICE_PARAM = "notice";

/// Задача от Rust (`lan::deeplink::PairTask`). Поля — часть контракта,
/// закреплённого cargo-тестом `the_task_wire_shape_is_pinned_in_the_frontend`.
export interface PairTask {
  kind: "idle" | "probe" | "go";
  /// Адрес вопроса (`probe`) или адрес, который открыло окно (`go`).
  url?: string;
  /// Адрес машины, к которой относится вопрос — его возвращаем в ответе.
  addr?: string;
  /// Сколько ждать ответа машины (тот же бюджет, что объявлен в Rust).
  timeout_ms?: number;
}

/// Один вопрос машине: только статус, тело не читаем (ответ — три слова).
/// `null` — ответа не было: адрес молчит или не пускает по CORS.
export async function fetchPairStatus(
  url: string,
  timeoutMs: number,
  fetchImpl: typeof fetch = fetch,
): Promise<number | null> {
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), timeoutMs);
  try {
    const response = await fetchImpl(url, {
      mode: "cors",
      credentials: "omit",
      signal: abort.signal,
    });
    return response.status;
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

/// Швы, через которые проходит проводка: в приложении — настоящие Rust и
/// сеть, в тестах — подставные (сам модуль ничего не знает про Tauri).
export interface PairBridge {
  boot: () => Promise<PairTask>;
  report: (addr: string, status: number | null) => Promise<PairTask>;
  ask: (url: string, timeoutMs: number) => Promise<number | null>;
}

export function windowBridge(): PairBridge {
  return {
    boot: () => invoke<PairTask>("pair_boot"),
    report: (addr, status) => invoke<PairTask>("pair_report", { addr, status }),
    ask: (url, timeoutMs) => fetchPairStatus(url, timeoutMs),
  };
}

/// Один проход: спросить → (при необходимости) задать вопрос машине →
/// доложить. Возвращает задачу, которую выполнил Rust: по ней страница видит,
/// ушла ли она сама (тогда окно уже открывает дом) или делать больше нечего.
export async function runPairing(bridge: PairBridge): Promise<PairTask> {
  const first = await bridge.boot();
  if (first.kind !== "probe" || !first.url || !first.addr) {
    return first;
  }
  const status = await bridge.ask(first.url, first.timeout_ms ?? 0);
  return bridge.report(first.addr, status);
}
