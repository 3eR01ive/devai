import { defineConfig } from "vitest/config";
import vue from "@vitejs/plugin-vue";

// Tauri expects a fixed dev port (devUrl in tauri.conf.json); production
// build output is served from `frontend/dist` through the custom protocol,
// so asset paths must stay relative.
export default defineConfig({
  plugins: [vue()],
  base: "./",
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2021",
  },
  // Компонентные тесты шаблонов (R4 QA-возврата): jsdom даёт localStorage
  // (настройки интервала) и DOM; IPC-слой в тестах замокан.
  test: {
    environment: "jsdom",
    setupFiles: ["src/__tests__/setup.ts"],
    include: ["src/__tests__/**/*.test.ts"],
  },
});
