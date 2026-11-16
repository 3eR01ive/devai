import { createRouter, createWebHashHistory } from "vue-router";
import ProjectsView from "./views/ProjectsView.vue";
import OpsView from "./views/OpsView.vue";
import HomesView from "./views/HomesView.vue";
import ModelsView from "./views/ModelsView.vue";
import LanView from "./views/LanView.vue";
import UsageView from "./views/UsageView.vue";
import PairView from "./views/PairView.vue";
import ProtoTasks from "./proto/ProtoTasks.vue";
import ProtoTask from "./proto/ProtoTask.vue";

// Hash history: production assets are served through the Tauri custom
// protocol, где серверного SPA-fallback нет.
export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", redirect: "/projects" },
    { path: "/projects", name: "projects", component: ProjectsView },
    // Новый экран по макету — единственный список задач и карточка.
    // Старые экраны (список, ожидание, активные) из интерфейса убраны
    // решением оператора 14.11: активные — группой в списке, остальное не нужно.
    { path: "/proto/tasks", name: "proto-tasks", component: ProtoTasks },
    { path: "/proto/task/:id", name: "proto-task", component: ProtoTask, props: true },
    { path: "/tasks", redirect: "/proto/tasks" },
    { path: "/task/:id", redirect: (to: any) => `/proto/task/${to.params.id}` },
    { path: "/live", redirect: "/proto/tasks" },
    { path: "/inbox", redirect: "/proto/tasks" },
    { path: "/homes", name: "homes", component: HomesView },
    { path: "/models", name: "models", component: ModelsView },
    { path: "/lan", name: "lan", component: LanView },
    // Отказ в приглашении (task 01a1bf78): экран открывает нативная половина
    // приложения по `#/pair?reason=…`; на десктопе маршрут никому не показан.
    { path: "/pair", name: "pair", component: PairView },
    { path: "/ops", name: "ops", component: OpsView },
    // Экран расхода токенов (REQ-USG-08): сутки/месяц, топ-задачи, прогноз.
    { path: "/usage", name: "usage", component: UsageView },
  ],
});
