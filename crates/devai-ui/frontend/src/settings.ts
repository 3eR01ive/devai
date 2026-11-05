// Локальные настройки интерфейса (A-UI-5): хранятся ВНЕ devai-store —
// это конфигурация потребителя, не доменное состояние платформы.
// Интервал поллинга живых панелей — настройка UI, не платформенное
// правило (REQ-UI-12 AC3); ручное обновление доступно всегда.

const KEY = "devai-ui.poll_ms";
const DEFAULT_MS = 5000;

export function pollMs(): number {
  const raw = localStorage.getItem(KEY);
  const n = raw ? Number.parseInt(raw, 10) : NaN;
  return Number.isFinite(n) && n >= 1000 ? n : DEFAULT_MS;
}

export function setPollMs(ms: number): void {
  localStorage.setItem(KEY, String(ms));
}

export const POLL_CHOICES = [2000, 5000, 10000, 30000];

export function fmtTime(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

export function fmtElapsed(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const rest = s % 60;
  return h > 0 ? `${h}ч ${m}м` : m > 0 ? `${m}м ${rest}с` : `${rest}с`;
}

export function shortId(id: string): string {
  // Display helper only; the platform's canonical width lives in
  // devai_core::types::short_id — the UI shows prefixes, never resolves
  // identities by them (task.get resolution stays API-side).
  return id.length > 12 ? id.slice(0, 12) : id;
}
