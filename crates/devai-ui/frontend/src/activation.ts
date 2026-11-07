// Единственный путь активации дома (R1 quality return). Два сайта
// активируют дом: вкладка «Дома» (HomesView) и гейт чужого дома в деталях
// задачи (TaskDetailView). Раньше каждый звал api.homesActivate сам, и
// TaskDetailView не публиковала свежий SessionInfo — статус-бар держал
// старый активный дом до перезагрузки окна. Теперь публикация живёт здесь:
// оба сайта зовут activateHome, и ни один не может «забыть» store.
//
// Модуль чистый (никаких vue-импортов): node-харнесс подменяет api и
// publish и исполняет ровно тот код, что и окно.
import { api } from "./api";
import { sessionStore } from "./session";
import type { SessionInfo } from "./types";

export interface ActivationPorts {
  /** home.activate — переоткрытие bootstrap-операции выбранного дома. */
  activate: (path: string) => Promise<SessionInfo>;
  /** Публикация свежей сессии всем потребителям окна. */
  publish: (info: SessionInfo) => void;
}

/** Продакшн-порты: Tauri-команда и стор сессии окна. */
export const activationPorts: ActivationPorts = {
  activate: (path) => api.homesActivate(path),
  publish: (info) => sessionStore.set(info),
};

/**
 * Активировать дом и опубликовать сессию. Возвращает свежий SessionInfo,
 * чтобы вызывающий мог обновить свои данные; ошибка активации летит
 * наружу — реестр не тронут, сессия не опубликована (в отличие от
 * скрытого переключения ради одного действия, ратифицированный OQ-HOME-2
 * требует именно явного клика оператора).
 */
export async function activateHome(
  path: string,
  ports: ActivationPorts = activationPorts,
): Promise<SessionInfo> {
  const info = await ports.activate(path);
  ports.publish(info);
  return info;
}
