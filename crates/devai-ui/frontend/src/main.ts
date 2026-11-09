import { createApp } from "vue";
import App from "./App.vue";
import { router } from "./router";
import "./styles.css";

// Токен приглашения живёт в адресе РОВНО один раз: сервер обменял его на
// HttpOnly-cookie ещё при загрузке страницы (src/lan.rs). Убираем его из
// адресной строки и истории, чтобы он не оставался на виду.
const landing = new URL(window.location.href);
if (landing.searchParams.has("token")) {
  landing.searchParams.delete("token");
  const clean = landing.pathname + (landing.search ? landing.search : "") + landing.hash;
  window.history.replaceState(null, "", clean);
}

createApp(App).use(router).mount("#app");
