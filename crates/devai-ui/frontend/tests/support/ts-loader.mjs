// Node 20 test runner + TypeScript sources: Node 20 has no native type
// stripping (that landed in 22.6). The frontend tests under `tests/` import
// `.ts` modules and `.vue` SFCs (the latter only through Vite SSR inside the
// harness), so the runner needs a loader that turns `.ts` into plain ESM.
//
// esbuild is already a dependency of vite (no new package); `transformSync`
// with the `ts` loader is exactly the strip step — type annotations vanish,
// runtime code is unchanged. `.vue` never reaches this loader: the harness
// loads components through a Vite dev server's `ssrLoadModule`.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { transformSync } from "esbuild";

export async function load(url, context, nextLoad) {
  if (url.endsWith(".ts")) {
    const source = readFileSync(fileURLToPath(url), "utf8");
    const { code } = transformSync(source, {
      loader: "ts",
      format: "esm",
      target: "es2022",
      sourcemap: "inline",
    });
    return { format: "module", source: code, shortCircuit: true };
  }
  return nextLoad(url, context);
}
