// Компонентный харнесс SFC-слоя (R6 quality return): SFC-темплейты
// монтируются по-настоящему (Vue-рендерер + клик по кнопке), а IPC
// подменяется mock-транспортом. Харнесс — мини-DOM: базовые host-операции
// Vue, которых хватает таблицам/панелям этих вью (без jsdom и без новых
// зависимостей).
//
// Почему так: R1 (активация из чужого гейта не публиковала сессию) прошёл
// мимо тестов именно потому, что .vue никто не исполнял — тестировались
// только чистые модули. Здесь исполняется тот же код, что и в окне:
// Vite SSR-загрузка .vue (plugin-vue), router на memory-history,
// `createRenderer` из vue и клик по найденной кнопке.

import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { createRenderer, nextTick, type Component } from "vue";
import { createMemoryHistory, createRouter, type Router } from "vue-router";
import { createServer, type ViteDevServer } from "vite";
import vue from "@vitejs/plugin-vue";

const HERE = dirname(fileURLToPath(import.meta.url));
const FRONTEND = resolve(HERE, "../..");

// Vue's `v-model` beforeUpdate hook names the DOM constructors directly
// (`rootNode instanceof Document`), and plain Node has no such globals.
// Inert stubs keep that branch skipped — our fake nodes are instances of
// neither, which is exactly the unfocused-input case.
const globals = globalThis as Record<string, unknown>;
if (typeof globals.Document === "undefined") globals.Document = class Document {};
if (typeof globals.ShadowRoot === "undefined") globals.ShadowRoot = class ShadowRoot {};

export interface FakeNode {
  type: string;
  props: Record<string, unknown>;
  children: FakeNode[];
  parent: FakeNode | null;
  text?: string;
  /** Minimal DOM surface used by Vue's built-in directives (v-model). */
  value?: string;
  style?: Record<string, string>;
  /** Handlers registered through `addEventListener` (v-model binds there). */
  listeners: Map<string, unknown[]>;
  addEventListener: (type: string, handler: unknown) => void;
  removeEventListener: (type: string, handler: unknown) => void;
  /** v-model's beforeUpdate checks focus through the root node. */
  getRootNode: () => FakeNode;
}

function makeNode(type: string): FakeNode {
  const listeners = new Map<string, unknown[]>();
  const node: FakeNode = {
    type,
    props: {},
    children: [],
    parent: null,
    listeners,
    addEventListener: (type, handler) => {
      const list = listeners.get(type) ?? [];
      list.push(handler);
      listeners.set(type, list);
    },
    removeEventListener: (type, handler) => {
      const list = listeners.get(type);
      const i = list ? list.indexOf(handler) : -1;
      if (list && i >= 0) list.splice(i, 1);
    },
    getRootNode: () => node,
  };
  return node;
}

function detach(node: FakeNode) {
  const parent = node.parent;
  if (!parent) return;
  const i = parent.children.indexOf(node);
  if (i >= 0) parent.children.splice(i, 1);
  node.parent = null;
}

function insert(child: FakeNode, parent: FakeNode, anchor?: FakeNode | null) {
  detach(child);
  const at = anchor ? parent.children.indexOf(anchor) : -1;
  if (at >= 0) parent.children.splice(at, 0, child);
  else parent.children.push(child);
  child.parent = parent;
}

const nodeOps = {
  createElement: (type: string) => makeNode(type),
  createText: (text: string) => ({ ...makeNode("#text"), text }),
  createComment: (text: string) => ({ ...makeNode("#comment"), text }),
  setText: (node: FakeNode, text: string) => {
    node.text = text;
  },
  setElementText: (node: FakeNode, text: string) => {
    for (const child of node.children) child.parent = null;
    node.children = [];
    if (text !== "") insert({ ...makeNode("#text"), text }, node);
  },
  insert,
  remove: detach,
  parentNode: (node: FakeNode) => node.parent,
  nextSibling: (node: FakeNode) => {
    const parent = node.parent;
    if (!parent) return null;
    const i = parent.children.indexOf(node);
    return parent.children[i + 1] ?? null;
  },
  querySelector: () => null,
  setScopeId: () => {},
  patchProp: (el: FakeNode, key: string, _prev: unknown, next: unknown) => {
    el.props[key] = next;
  },
  insertStaticContent: () => {
    throw new Error(
      "static hoisted content needs hoistStatic:false in the test Vite config",
    );
  },
};

export const { createApp } = createRenderer<FakeNode, FakeNode>(nodeOps);

/** Visible text of a subtree (what the operator reads). */
export function textOf(node: FakeNode): string {
  if (node.type === "#text") return node.text ?? "";
  return node.children.map(textOf).join(" ");
}

/** Depth-first collection of element nodes. */
export function elements(root: FakeNode): FakeNode[] {
  const out: FakeNode[] = [];
  const walk = (n: FakeNode) => {
    if (!n.type.startsWith("#")) out.push(n);
    for (const c of n.children) walk(c);
  };
  walk(root);
  return out;
}

/** The one visible button whose label contains `label`. */
export function buttonByLabel(root: FakeNode, label: string): FakeNode {
  const hit = elements(root).find(
    (n) => n.type === "button" && typeof n.props.onClick === "function" && textOf(n).includes(label),
  );
  assert.ok(hit, `no button labelled «${label}»; rendered: ${textOf(root)}`);
  return hit;
}

/**
 * Type into a v-model input: Vue binds `vModelText` through
 * `addEventListener("input")`, so the stored handler is fired with the
 * element as the event target — the same path a keystroke takes.
 */
export async function typeInto(node: FakeNode, value: string) {
  node.value = value;
  const handlers = node.listeners.get("input") ?? [];
  assert.ok(handlers.length > 0, `no input handler bound on <${node.type}>`);
  for (const handler of handlers) {
    (handler as (e: unknown) => void)({ target: node });
  }
  await nextTick();
}

/** Let Vue flush effects and the awaited async handlers settle. */
export async function flush(times = 6) {
  for (let i = 0; i < times; i += 1) {
    await nextTick();
    await new Promise((r) => setTimeout(r, 0));
  }
}

export interface Harness {
  server: ViteDevServer;
  router: Router;
  close: () => Promise<void>;
}

/** Vite in SSR mode: loads .vue/.ts exactly as the window's build does. */
export async function startHarness(): Promise<Harness> {
  // The harness mounts for the CLIENT (custom renderer), but Vite's
  // `ssrLoadModule` asks plugin-vue for the SSR shape (ssrRender +
  // useSSRContext), which cannot be clicked. This wrapper compiles .vue in
  // client mode while the module graph is still loaded through the SSR
  // runner; the emitted code is plain client-render ESM.
  const vuePlugin = vue({
    template: { compilerOptions: { hoistStatic: false } },
  });
  const clientVue = {
    name: "test-client-vue",
    enforce: "pre" as const,
    async transform(
      this: unknown,
      code: string,
      id: string,
      options?: { ssr?: boolean },
    ) {
      if (!id.endsWith(".vue")) return null;
      const transform = vuePlugin.transform as unknown as (
        this: unknown,
        code: string,
        id: string,
        options: { ssr?: boolean },
      ) => unknown;
      // Scoped-style injection belongs to the browser pipeline (it imports
      // `/@vite/client` for HMR). Styles are irrelevant to behaviour under
      // test, so they are dropped before the client-mode compile.
      const noStyles = code.replace(/<style[\s>][\s\S]*?<\/style>/g, "");
      const result = (await transform.call(this, noStyles, id, {
        ...options,
        ssr: false,
      })) as string | { code: string } | null;
      if (result === null) return null;
      // Client-mode compile emits HMR wiring; the SSR module runner has no
      // `import.meta.hot`. A no-op stub keeps the rest of the module — and
      // the component itself — untouched.
      const stub = "({ on() {}, accept() {}, prune() {} })";
      const patchHot = (s: string) => s.replaceAll("import.meta.hot", stub);
      if (typeof result === "string") return patchHot(result);
      return { ...result, code: patchHot(result.code) };
    },
  };
  const server = await createServer({
    root: FRONTEND,
    configFile: false,
    logLevel: "silent",
    appType: "custom",
    server: { middlewareMode: true },
    // The Tauri packages are aliased to mocks above; externalizing them
    // would let the real module load and reach for `window`.
    ssr: { noExternal: [/^@tauri-apps\//] },
    plugins: [
      // The same plugin instance configured by Vite (its `configResolved`
      // sets the SFC compiler), but its own transform is suppressed: the
      // wrapper above calls it with `ssr:false`.
      { ...vuePlugin, name: "test-vue-config", transform: undefined },
      clientVue,
    ],
    resolve: {
      alias: [
        {
          find: /^@tauri-apps\/api\/core$/,
          replacement: resolve(HERE, "tauri-core.ts"),
        },
        {
          find: /^@tauri-apps\/plugin-dialog$/,
          replacement: resolve(HERE, "plugin-dialog.ts"),
        },
      ],
    },
  });
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/", component: { render: () => null } },
      { path: "/projects", component: { render: () => null } },
      { path: "/tasks", component: { render: () => null } },
      { path: "/task/:id", component: { render: () => null } },
      { path: "/homes", component: { render: () => null } },
    ],
  });
  return {
    server,
    router,
    close: async () => {
      await server.close();
    },
  };
}

/** Load a view module through Vite's SSR pipeline. */
export async function loadView(
  harness: Harness,
  file: string,
): Promise<Component> {
  const mod = (await harness.server.ssrLoadModule(absolute(file))) as {
    default: Component;
  };
  return mod.default;
}

/** `/src/…` → absolute path inside this frontend package. */
function absolute(file: string): string {
  return file.startsWith("/src/") ? resolve(FRONTEND, file.slice(1)) : file;
}

/** Install the fake IPC: command → handler. */
export function installInvoke(
  handler: (cmd: string, args: unknown) => unknown,
) {
  (globalThis as Record<string, unknown>).__DEVAI_TEST_INVOKE__ = handler;
}

/** Mount a component with the shared session store and router. */
export async function mount(
  harness: Harness,
  component: Component,
  props: Record<string, unknown> = {},
  initialPath = "/",
): Promise<{
  root: FakeNode;
  sessionStore: {
    get: () => SessionInfoLike | null;
    set: (session: SessionInfoLike) => void;
  };
}> {
  const session = (await harness.server.ssrLoadModule(
    resolve(FRONTEND, "src/session.ts"),
  )) as {
    sessionStore: {
      get: () => SessionInfoLike | null;
      set: (session: SessionInfoLike) => void;
    };
  };
  await harness.router.push(initialPath);
  await harness.router.isReady();
  const root = makeNode("root");
  const app = createApp(component, props);
  app.use(harness.router);
  app.mount(root);
  await flush();
  return { root, sessionStore: session.sessionStore };
}

/** Only the fields the activation tests assert on. */
export interface SessionInfoLike {
  home: string;
}
