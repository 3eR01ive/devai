// Карточка задачи: панель пути (REQ-RG-06). Панель рисует ГОТОВЫЙ объект
// `progress.graph` — ось этапов в порядке ответа, узлы со статусами,
// рёбра с подписью носителя, ожидание человека, круги доработки и число
// блокирующих вопросов. Окно не хранит порядка этапов и не пересчитывает
// данные: здесь это фиксируется подменой порядка в ответе.
import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import RoleGraphPanel from "../components/RoleGraphPanel.vue";
import type { GraphStage, RoleGraph } from "../types";

function stage(
  state: string,
  label: string,
  status: GraphStage["status"],
  statusWord: string,
): GraphStage {
  return {
    state,
    label,
    executor: "role",
    gate: status === "current",
    status,
    status_word: statusWord,
  };
}

function graph(overrides: Partial<RoleGraph> = {}): RoleGraph {
  return {
    stages: [
      stage("ANALYSIS", "аналитик", "passed", "пройдено"),
      stage("APPROVAL", "утвердить требования", "current", "сейчас"),
      stage("IMPLEMENTATION", "имплементатор", "pending", "ещё не было"),
    ],
    nodes: [
      {
        id: "role:analyst",
        kind: "role",
        state: "ANALYSIS",
        label: "analyst",
        display: "analyst · v1",
        role_slug: "analyst",
        role_version: 1,
        input_contract: "requirements-specification@4",
        output_contract: "review-report@1",
        status: "passed",
        status_word: "пройдено",
        revoked: false,
        executions: [
          {
            id: "e-1",
            role_slug: "analyst",
            role_display: "analyst · v1",
            role_version: 1,
            agent_name: "hermes",
            status: "COMPLETED",
            attempt: 1,
            started_at: "2026-11-11T10:00:00Z",
            finished_at: "2026-11-11T10:05:00Z",
          },
          {
            id: "e-2",
            role_slug: "analyst",
            role_display: "analyst · v2",
            role_version: 2,
            agent_name: "hermes",
            status: "COMPLETED",
            attempt: 2,
            started_at: "2026-11-11T11:00:00Z",
            finished_at: "2026-11-11T11:05:00Z",
          },
        ],
        artifacts: [
          {
            id: "a-spec",
            kind: "REQUIREMENTS_SPEC",
            seq: 2,
            status: "CURRENT",
            name: "requirements-spec",
            display: "спека · v2",
          },
        ],
      },
      {
        id: "gate:APPROVAL",
        kind: "gate",
        state: "APPROVAL",
        label: "утвердить требования",
        display: "утвердить требования",
        role_slug: null,
        role_version: null,
    input_contract: null,
    output_contract: null,
        status: "current",
        status_word: "сейчас",
        revoked: false,
        executions: [],
        artifacts: [],
      },
    ],
    edges: [
      {
        from: "role:analyst",
        to: "gate:APPROVAL",
        kind: "transfer",
        round: null,
        carrier: { kind: "artifact", id: "a-spec", label: "спека · v2" },
      },
    ],
    position: {
      kind: "node",
      anchor: "gate",
      stage: "APPROVAL",
      node: "gate:APPROVAL",
      reason: null,
      line: "на утверждении требований",
    },
    rework: { rounds: 2, active: false, note: "кругов доработки: 2" },
    running: [
      {
        execution_id: "e-3",
        role_slug: "reviewer",
        role_display: "requirements-reviewer · v2",
        role_version: 2,
        agent_name: "hermes",
        in_path: true,
        note: null,
      },
    ],
    waits: [
      {
        node: "gate:APPROVAL",
        state: "APPROVAL",
        requirement: "подписать требования",
        hint: "devai task approve 01a1b5e1 --rationale \"…\"",
        awaiting: true,
        signable: true,
        presentation: {
          artifact_id: "a-spec",
          display: "спека · v2",
          status: "CURRENT",
          current: true,
          note: null,
        },
      },
    ],
    blocking_questions: 1,
    contracts: [],
    notes: ["прогон вне пути: reviewer"],
    ...overrides,
  };
}

function mountWith(g: RoleGraph) {
  return mount(RoleGraphPanel, {
    props: { graph: g, snapshotAt: "2026-11-11T12:00:00Z" },
  });
}

describe("RoleGraphPanel · путь задачи", () => {
  it("prints the stage axis in the order the response gives, with a word per status", () => {
    const w = mountWith(graph());
    const text = w.text();
    expect(text).toContain("аналитик");
    expect(text).toContain("пройдено");
    expect(text).toContain("сейчас");
    expect(text).toContain("ещё не было");
    // Порядок — массива ответа, а не зашитый в окне.
    expect(text.indexOf("аналитик")).toBeLessThan(text.indexOf("утвердить требования"));
    expect(text.indexOf("утвердить требования")).toBeLessThan(text.indexOf("имплементатор"));
    w.unmount();
  });

  it("renders another declared order without any change to the component", () => {
    const g = graph();
    g.stages = [
      stage("IMPLEMENTATION", "имплементатор", "current", "сейчас"),
      stage("ANALYSIS", "аналитик", "passed", "пройдено"),
    ];
    const w = mountWith(g);
    const text = w.text();
    expect(text.indexOf("имплементатор")).toBeLessThan(text.indexOf("аналитик"));
    w.unmount();
  });

  it("prints the position line, the rework rounds and the blocking count without expanding", () => {
    const w = mountWith(graph());
    const text = w.text();
    expect(text).toContain("на утверждении требований");
    expect(text).toContain("кругов доработки: 2");
    expect(text).toContain("открытых блокирующих вопросов: 1");
    expect(text).toContain("контур доработки");
    w.unmount();
  });

  it("names who executes now with the same «имя · vN» the node carries", () => {
    const w = mountWith(graph());
    const text = w.text();
    expect(text).toContain("кто исполняет сейчас");
    expect(text).toContain("requirements-reviewer · v2");
    expect(text).toContain("hermes");
    w.unmount();
  });

  it("prints every execution and artifact of a node, labelled by kind and version", () => {
    const w = mountWith(graph());
    const text = w.text();
    expect(text).toContain("исполнения (2)");
    expect(text).toContain("analyst · v1");
    expect(text).toContain("analyst · v2");
    expect(text).toContain("артефакты (1)");
    expect(text).toContain("спека · v2");
    // Ключ вида не занимает место заголовка: он в приглушённой второй позиции.
    expect(text).toContain("REQUIREMENTS_SPEC");
    expect(text.indexOf("спека · v2")).toBeLessThan(text.indexOf("REQUIREMENTS_SPEC"));
    w.unmount();
  });

  it("shows the human wait: requirement, hint, signability and the presentation", () => {
    const w = mountWith(graph());
    const text = w.text();
    expect(text).toContain("Требование: подписать требования");
    expect(text).toContain('devai task approve 01a1b5e1 --rationale "…"');
    expect(text).toContain("ждёт подписи");
    expect(text).toContain("подписываемо");
    expect(text).toContain("предъявлено: спека · v2");
    w.unmount();
  });

  it("names both ends of an edge and its carrier", () => {
    const w = mountWith(graph());
    const text = w.text();
    expect(text).toContain("переходы (1)");
    expect(text).toContain("analyst · v1 → утвердить требования");
    expect(text).toContain("спека · v2");
    w.unmount();
  });

  it("prints notes and the off-graph reason as data, never as a hidden hint", () => {
    const g = graph({
      position: {
        kind: "off_graph",
        anchor: "stage",
        stage: null,
        node: null,
        reason: "draft",
        line: "вне пути: черновик",
      },
    });
    const w = mountWith(g);
    const text = w.text();
    expect(text).toContain("вне пути: черновик");
    expect(text).toContain("причина: draft");
    expect(text).toContain("прогон вне пути: reviewer");
    w.unmount();
  });

  it("keeps the lists complete — no «+N ещё» counter", () => {
    const w = mountWith(graph());
    expect(w.text()).not.toMatch(/\+\s*\d+\s*ещё/);
    w.unmount();
  });
});
