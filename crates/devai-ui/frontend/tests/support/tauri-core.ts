// Mock of the Tauri invoke transport for the component harness
// (`tests/views-activation.test.ts`). The real `@tauri-apps/api/core` needs
// a Tauri window; the harness aliases this module in its Vite config and
// routes every IPC call to the handler the test installs on `globalThis`.
// The mock is deliberately dumb: it records the command and delegates, so
// the production `src/api.ts` table is exercised unchanged.

export const calls: { cmd: string; args: unknown }[] = [];

export async function invoke<T>(cmd: string, args?: unknown): Promise<T> {
  calls.push({ cmd, args });
  const handler = (
    globalThis as unknown as {
      __DEVAI_TEST_INVOKE__?: (cmd: string, args: unknown) => unknown;
    }
  ).__DEVAI_TEST_INVOKE__;
  if (!handler) {
    throw new Error(`no test invoke handler installed (command ${cmd})`);
  }
  return (await handler(cmd, args)) as T;
}
