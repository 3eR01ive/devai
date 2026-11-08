// Rework R7: the refusal codes of the signature rights are spelled ONCE, in
// Rust (`crates/devai-api/src/signature/denial.rs` → DENIAL_CODES), and the
// section's word map `DENIAL_LABELS` must cover EXACTLY that set. A code
// added on one side only fails this test: the two halves cannot drift.
//
// This is a parity check over the real source, not a copy: it parses the
// Rust constants and the DENIAL_CODES array, then compares the key set.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { DENIAL_LABELS } from "../src/signature_labels.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const DENIAL_RS = resolve(HERE, "../../../devai-api/src/signature/denial.rs");

/** `pub const NAME: &str = "value";` pairs of the denial module. */
function stringConstants(source) {
  const consts = new Map();
  for (const m of source.matchAll(/pub const ([A-Z][A-Z0-9_]*): &str = "([^"]+)";/g)) {
    consts.set(m[1], m[2]);
  }
  return consts;
}

/** The codes named by `DENIAL_CODES`, resolved to their string values. */
function denialCodes(source) {
  const consts = stringConstants(source);
  const list = source.match(/pub const DENIAL_CODES: &\[&str\] = &\[([\s\S]*?)\];/);
  assert.ok(list, "denial.rs must declare the DENIAL_CODES array");
  return list[1]
    .split(",")
    .map((name) => name.trim())
    .filter(Boolean)
    .map((name) => {
      const value = consts.get(name);
      assert.ok(value, `DENIAL_CODES names ${name}, which has no string constant`);
      return value;
    });
}

test("every Rust denial code has a word, and no word exists without a code", () => {
  const codes = denialCodes(readFileSync(DENIAL_RS, "utf8"));
  assert.ok(codes.length > 0, "the denial dictionary must not be empty");
  assert.deepEqual(
    Object.keys(DENIAL_LABELS).sort(),
    [...codes].sort(),
    "DENIAL_LABELS and the Rust DENIAL_CODES disagree — add the code on BOTH sides",
  );
  for (const code of codes) {
    assert.ok(DENIAL_LABELS[code]?.trim(), `нет слова для кода отказа ${code}`);
  }
});
