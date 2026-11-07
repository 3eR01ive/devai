// R1 quality return: the homes machine vocabulary used to be hand-mirrored
// in `homes-view.ts` beside `crates/devai-api/src/codes.rs`. The old parity
// test compared the two copies value-by-value, so a NEW Rust code the UI did
// not branch on stayed invisible — the mirror silently lagged.
//
// Now there is one source: `crates/devai-api/src/codes.rs`. The TS module
// `src/generated/home-codes.ts` is a RENDER of it (scripts/gen-home-codes.mjs)
// and every code — branched on or not — lands in the TS dictionary by
// construction. This test therefore checks the render, not a mirror:
//   1. the checked-in module equals a fresh render of codes.rs (stale file
//      fails, naming the regen command);
//   2. no code of codes.rs is missing from the TS dictionary, and no TS key
//      exists without a Rust producer;
//   3. the mechanical key derivation is pinned (HOME_NO_STORE → noStore).

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { HOME_CODES } from "../src/homes-view.ts";
import {
  OUT_FILE,
  RUST_CODES,
  generate,
  parseCodes,
  renderModule,
  tsKey,
} from "../scripts/gen-home-codes.mjs";

const rustSource = (): string => readFileSync(RUST_CODES, "utf8");

test("the checked-in TS dictionary is a current render of codes.rs", () => {
  const expected = renderModule(parseCodes(rustSource()));
  assert.equal(
    readFileSync(OUT_FILE, "utf8"),
    expected,
    "src/generated/home-codes.ts is stale — run `node scripts/gen-home-codes.mjs`",
  );
});

test("generate() is idempotent — a clean tree needs no write", () => {
  // The build step runs on every `npm run build`/`test`; a no-op must stay
  // a no-op, or every build would dirty the working tree.
  assert.equal(generate(), false, "generate() rewrote an already-current file");
});

test("every Rust code reaches the TS dictionary, branched on or not", () => {
  const pairs = parseCodes(rustSource());
  assert.ok(pairs.length >= 4, `expected the homes vocabulary, got ${pairs.length}`);
  const tsValues = new Set<string>(Object.values(HOME_CODES));
  for (const { rustName, value } of pairs) {
    assert.ok(
      tsValues.has(value),
      `${rustName} (${value}) is emitted by Rust but absent from the TS dictionary`,
    );
  }
});

test("the TS dictionary does not grow a code the backend never emits", () => {
  const rustValues = new Set(parseCodes(rustSource()).map((p) => p.value));
  for (const [key, value] of Object.entries(HOME_CODES)) {
    assert.ok(rustValues.has(value), `TS-only code with no Rust producer: ${key} = ${value}`);
  }
});

test("keys derive mechanically from the Rust constant name", () => {
  assert.equal(tsKey("HOME_UNAVAILABLE"), "unavailable");
  assert.equal(tsKey("HOME_NO_STORE"), "noStore");
  assert.equal(tsKey("HOME_SCHEMA_VERSION"), "schemaVersion");
  assert.equal(tsKey("HOMES_REGISTRY_CORRUPT"), "registryCorrupt");
  assert.equal(tsKey("HOMES_REGISTRY_UNAVAILABLE"), "registryUnavailable");
  // The rendered dictionary carries exactly those keys.
  const expectedKeys = parseCodes(rustSource()).map((p) => tsKey(p.rustName));
  assert.deepEqual(Object.keys(HOME_CODES), expectedKeys);
});

test("two Rust constants never collapse into one TS key", () => {
  assert.throws(
    () =>
      renderModule([
        { rustName: "HOME_NO_STORE", value: "a" },
        { rustName: "HOMES_NO_STORE", value: "b" },
      ]),
    /two Rust constants render to the TS key/,
  );
});
