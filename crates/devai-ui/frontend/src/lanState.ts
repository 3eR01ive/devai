// Состояния доступа с телефона — ОДНА декларация и ОДНА презентация для всей
// передней части (task 01a1be98, quality return R1). Список совпадает с
// Rust-перечислением `lan::state::LanState`; паритет проверяется
// cargo-тестом (`every_state_is_declared_across_the_wire_and_named_only_in_this_module`),
// поэтому добавить состояние на бэкенде и забыть здесь нельзя.
//
// Запись `Record<LanState, …>` вынуждает vue-tsc обработать каждое
// объявленное состояние, а `LanView.vue` получает готовые булевы факты и ни
// одного доменного литерала не содержит.
import type { LanStatus } from "./types";

export const LAN_STATES = ["off", "awaiting", "connected"] as const;

export type LanState = (typeof LAN_STATES)[number];

/// Всё, что решает вид панели. `null` — снимок ещё не пришёл.
export interface LanFacts {
  state: LanState | null;
  label: string;
  /// Кнопка «Включить доступ» вместо «Выключить».
  canEnable: boolean;
  /// Блок приглашения: QR, адрес и одноразовый код.
  showInvitation: boolean;
  /// Приглашение выпущено, но QR не построился: это НЕ «код использован».
  qrFailed: boolean;
  /// Доступ включён, а действующего приглашения нет (код уже использован).
  showInviteMissing: boolean;
  /// Таблица подключённых устройств (в том числе молчащих).
  showDevices: boolean;
}

type Facts = Omit<LanFacts, "state" | "label">;

const PRESENTATION: Record<LanState, (status: LanStatus) => Facts & { label: string }> = {
  off: () => ({
    label: "выключен",
    canEnable: true,
    showInvitation: false,
    qrFailed: false,
    showInviteMissing: false,
    showDevices: false,
  }),
  // Сервер поднят. Если телефон уже был подключён, но замолчал, это НЕ
  // «подключено»: окно честно говорит, что телефон не отвечает
  // (REQ-LAN-01 AC4, REQ-LAN-09 AC5).
  awaiting: (status) => ({
    label: status.devices.length > 0 ? "включён, телефон не отвечает" : "включён, ждёт подключения",
    canEnable: false,
    showInvitation: status.invite_active,
    qrFailed: status.qr_error !== null,
    showInviteMissing: !status.invite_active,
    showDevices: status.devices.length > 0,
  }),
  connected: (status) => ({
    label: "подключено",
    canEnable: false,
    showInvitation: status.invite_active,
    qrFailed: status.qr_error !== null,
    showInviteMissing: false,
    showDevices: true,
  }),
};

export function lanStateView(status: LanStatus | null): LanFacts {
  if (!status) {
    return {
      state: null,
      label: "загрузка…",
      canEnable: false,
      showInvitation: false,
      qrFailed: false,
      showInviteMissing: false,
      showDevices: false,
    };
  }
  return { state: status.state, ...PRESENTATION[status.state](status) };
}
