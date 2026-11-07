// Mock of `@tauri-apps/plugin-dialog` for the component harness: HomesView
// imports `open` at module scope, and the native dialog needs a Tauri
// window. The answer comes from a global the test installs
// (`__DEVAI_TEST_DIALOG__`): a string is the picked path, `null` is the
// operator's cancel, an Error models a dialog failing outside Tauri. With
// nothing installed the call fails loudly — an honest setup failure.

export async function open(): Promise<string | null> {
  const answer = (globalThis as { __DEVAI_TEST_DIALOG__?: unknown })
    .__DEVAI_TEST_DIALOG__;
  if (answer === undefined) {
    throw new Error("native dialog is not available in the component harness");
  }
  if (answer instanceof Error) throw answer;
  return answer as string | null;
}
