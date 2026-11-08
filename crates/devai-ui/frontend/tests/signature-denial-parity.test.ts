// Rework R5/R8: the signature section's words used to be hand-written TS maps
// beside `CLOSED_TOKEN_SETS`, and the refusal codes a Rust const table. Now
// there is ONE data file — `crates/devai-api/src/signature/labels.json` — and
// every closed set is wired to it mechanically:
//
//   * Rust tests (`crates/devai-api/src/signature/vocabulary.rs`) compare the
//     data's token sets with the closed enums and pin the ratified denial
//     codes;
//   * this test compares the checked-in TS render with a fresh render of the
//     data, per set — a token added to the data but not rendered, or a render
//     edited by hand, fails here;
//   * `DENIAL_LABELS` is checked against the data's `denial` set explicitly,
//     because that is the set the refusal rules return.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import {
  ARTIFACT_STATUS_LABELS,
  DENIAL_LABELS,
  PARAGRAPH_SOURCE_LABELS,
  REVIEW_PARAGRAPH_SOURCE_LABELS,
  REVIEW_VERDICT_LABELS,
  SIGNATURE_KIND_LABELS,
  SIGNATURE_SOURCE_LABELS,
} from "../src/signature_labels.ts";
import {
  LABELS_JSON,
  OUT_FILE,
  SETS,
  parseLabels,
  renderModule,
} from "../scripts/gen-signature-labels.mjs";

const data = parseLabels(readFileSync(LABELS_JSON, "utf8"));

const rendered: Record<string, Record<string, string>> = {
  ARTIFACT_STATUS_LABELS,
  REVIEW_VERDICT_LABELS,
  PARAGRAPH_SOURCE_LABELS,
  REVIEW_PARAGRAPH_SOURCE_LABELS,
  DENIAL_LABELS,
  SIGNATURE_KIND_LABELS,
  SIGNATURE_SOURCE_LABELS,
};

test("the checked-in TS render is a current render of labels.json", () => {
  assert.equal(
    readFileSync(OUT_FILE, "utf8"),
    renderModule(data),
    "src/generated/signature-labels.ts is stale — run `node scripts/gen-signature-labels.mjs`",
  );
});

test("every data set reaches the UI with exactly its tokens and words", () => {
  for (const [set, constant] of Object.entries(SETS)) {
    const pairs = data[set] as { token: string; word: string }[];
    const render = rendered[constant as string];
    assert.ok(render, `no render for set ${set} (${constant})`);
    assert.deepEqual(
      Object.keys(render).sort(),
      pairs.map((p) => p.token).sort(),
      `${constant} and labels.json «${set}» disagree — regenerate the module`,
    );
    for (const pair of pairs) {
      assert.equal(render[pair.token], pair.word, `${constant}:${pair.token} word differs`);
      assert.ok(pair.word.trim(), `${constant}:${pair.token} has a blank word`);
    }
  }
});

test("no UI dictionary exists that the data does not produce", () => {
  assert.deepEqual(
    Object.keys(rendered).sort(),
    Object.values(SETS).sort(),
    "a TS dictionary is rendered that labels.json does not define",
  );
});

test("the denial words come from the same data the refusal rules read", () => {
  assert.deepEqual(
    Object.keys(DENIAL_LABELS).sort(),
    data.denial.map((p: { token: string }) => p.token).sort(),
    "DENIAL_LABELS and labels.json «denial» disagree — the rules return data tokens",
  );
});

test("a set the data grows without a TS render fails loudly", () => {
  assert.throws(
    () => parseLabels(JSON.stringify({ ...data, unexpected_set: [{ token: "x", word: "y" }] })),
    /sets with no TS render/,
  );
  assert.throws(
    () => parseLabels(JSON.stringify({ ...data, denial: [] })),
    /no non-empty `denial` set/,
  );
});
