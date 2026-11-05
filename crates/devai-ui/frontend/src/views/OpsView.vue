<script setup lang="ts">
// Опубликованный реестр операций (REQ-UI-02 AC2): панель, на которой видно,
// что каждая граница UI = запись api.registry класса Read. Оранжевой точкой
// помечены операции, которые этот клиент реально вызывает; Mutation-класс
// недоступен UI по построению (read-only база, OQ-UI-6).
import { onMounted, ref } from "vue";
import { api, toUiError } from "../api";
import StateChip from "../components/StateChip.vue";
import type { OpDescriptor, UiError } from "../types";

// Источник истины — COMMAND_OPERATIONS в src/commands.rs (там же тест, что
// каждая из них — опубликованная Read-операция реестра).
const used = [
  "api.registry",
  "project.list", "project.get",
  "task.list", "task.get", "task.status", "task.progress", "task.dependencies",
  "task.report", "task.events", "task.decisions", "task.active",
  "artifact.list", "artifact.cat", "artifact.cat_text",
  "question.list",
  "exec.list", "exec.liveness",
];

const ops = ref<OpDescriptor[]>([]);
const error = ref<UiError | null>(null);

async function load() {
  error.value = null;
  try {
    ops.value = await api.registryOperations();
  } catch (e) {
    error.value = toUiError(e);
  }
}
onMounted(load);
</script>

<template>
  <section>
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">Операции API (api.registry)</h2>
      <div class="spacer" />
      <button @click="load">Обновить</button>
    </div>
    <div v-if="error" class="errorbox">
      <span class="code">{{ error.code }}</span>{{ error.message }}
    </div>
    <table class="data">
      <thead>
        <tr><th>Операция</th><th>Класс</th><th>Вход</th><th>Выход</th><th>Семантика</th></tr>
      </thead>
      <tbody>
        <tr v-for="o in ops" :key="o.name">
          <td>
            <span class="mono">{{ o.name }}</span>
            <span v-if="used.includes(o.name)" title="вызывается devai-ui" style="color: var(--orange); font-weight: 700"> ●</span>
          </td>
          <td><StateChip :token="o.class.toUpperCase()" /></td>
          <td class="mono muted">{{ o.input }}</td>
          <td class="mono muted">{{ o.output }}</td>
          <td class="muted">{{ o.description }}</td>
        </tr>
      </tbody>
    </table>
  </section>
</template>
