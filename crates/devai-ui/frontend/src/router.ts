import { createRouter, createWebHashHistory } from "vue-router";
import ProjectsView from "./views/ProjectsView.vue";
import TasksView from "./views/TasksView.vue";
import TaskDetailView from "./views/TaskDetailView.vue";
import LiveView from "./views/LiveView.vue";
import InboxView from "./views/InboxView.vue";
import OpsView from "./views/OpsView.vue";
import HomesView from "./views/HomesView.vue";
import ModelsView from "./views/ModelsView.vue";
import LanView from "./views/LanView.vue";
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
    { path: "/tasks", name: "tasks", component: TasksView },
    // Новый экран по макету: список и карточка. Старые экраны остаются,
    // пока человек не скажет, что из них забрать.
    { path: "/proto/tasks", name: "proto-tasks", component: ProtoTasks },
    { path: "/proto/task/:id", name: "proto-task", component: ProtoTask, props: true },
    { path: "/task/:id", name: "task", component: TaskDetailView, props: true },
    { path: "/live", name: "live", component: LiveView },
    { path: "/inbox", name: "inbox", component: InboxView },
    { path: "/homes", name: "homes", component: HomesView },
    { path: "/models", name: "models", component: ModelsView },
    { path: "/lan", name: "lan", component: LanView },
    // Отказ в приглашении (task 01a1bf78): экран открывает нативная половина
    // приложения по `#/pair?reason=…`; на десктопе маршрут никому не показан.
    { path: "/pair", name: "pair", component: PairView },
    { path: "/ops", name: "ops", component: OpsView },
  ],
});
