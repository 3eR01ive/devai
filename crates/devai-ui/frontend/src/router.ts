import { createRouter, createWebHashHistory } from "vue-router";
import ProjectsView from "./views/ProjectsView.vue";
import TasksView from "./views/TasksView.vue";
import TaskDetailView from "./views/TaskDetailView.vue";
import LiveView from "./views/LiveView.vue";
import InboxView from "./views/InboxView.vue";
import OpsView from "./views/OpsView.vue";

// Hash history: production assets are served through the Tauri custom
// protocol, где серверного SPA-fallback нет.
export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", redirect: "/projects" },
    { path: "/projects", name: "projects", component: ProjectsView },
    { path: "/tasks", name: "tasks", component: TasksView },
    { path: "/task/:id", name: "task", component: TaskDetailView, props: true },
    { path: "/live", name: "live", component: LiveView },
    { path: "/inbox", name: "inbox", component: InboxView },
    { path: "/ops", name: "ops", component: OpsView },
  ],
});
