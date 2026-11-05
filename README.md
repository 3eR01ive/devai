<div align="center">

<img src="assets/logo.svg" alt="devai" width="128" />

## devai

**Local-first multi-agent development platform with a declarative workflow kernel.**

Tasks move through an explicit state machine; roles (analyst, reviewer, implementer,
auditor) are executed by your local coding agents; code quality is judged by a
declarative evaluation profile with machine gates — not by vibes.

</div>

## Why

A coding agent without a process produces commits, not a history you can audit.
devai keeps every step — requirements, reviews, decisions, patches, quality reports,
the provenance of the exact profile and revision that judged the code — in an
append-only store. You get `git log` for the *process*, plus a hard QA gate before
anything merges.

- **Sovereign task queue** — DRAFT → ANALYSIS → REVIEW → APPROVAL → EXECUTE → QA → ACCEPTED, with human gates where the decision is substance, not ceremony.
- **Declarative evaluation** — the QA profile (gates, checks, anchors, floors, weights, verdict) is a YAML file. The platform runs gates and mechanical probes; the auditor model only scores rubric checks and must attach evidence.
- **Typed API, CLI as its first consumer** — every read *and* write goes through `devai-api` (typed Req/Outcome, machine-readable error codes); nothing touches the store directly.
- **Append-only SQLite** — events are never rewritten, artifacts are content-addressed, rework is a versioned lineage, not an overwrite.
- **Local-first** — no network calls in product code; coding agents are integrated as local subprocess adapters.

## Install (Linux)

### Release binary

Grab the `devai-vX.Y.Z-linux-x86_64.tar.gz` asset (plus its `.sha256`) from the
[latest release](https://github.com/3eR01ive/devai/releases/latest):

```sh
curl -LO https://github.com/3eR01ive/devai/releases/latest/download/devai-v0.1.0-linux-x86_64.tar.gz
echo "$(cat devai-v0.1.0-linux-x86_64.tar.gz.sha256)  devai-v0.1.0-linux-x86_64.tar.gz" | sha256sum -c -
tar xzf devai-v0.1.0-linux-x86_64.tar.gz
sudo install devai-v0.1.0-linux-x86_64/devai /usr/local/bin/devai
```

### From source

Requires a stable Rust toolchain (edition 2021) and `git`:

```sh
git clone https://github.com/3eR01ive/devai.git
cd devai
cargo build --release --workspace
# binary at target/release/devai
```

## Getting started

All state lives in `$DEVAI_HOME` (default `~/.devai`): the SQLite store, the seeded
config tree (roles, criteria, evaluation profiles), content-addressed blobs and a
git worktree per task.

```sh
devai init        # materialize and validate the config tree
```

Agents that execute roles are registered providers (`claude`, `hermes`) plus an
offline `mock` for dry runs — see `devai agent list`.

Run a task through the full loop:

```sh
cat > task.txt <<'TXT'
Add --json to `task status`.
Why: scripts consuming the CLI need a machine-readable contract.
TXT

devai task new "status --json" --description-file task.txt   # -> DRAFT
devai task start 01a1a…                                       # -> ANALYSIS
devai task run  01a1a… --agent hermes
```

`task run` walks the loop until the first human gate: the requirements-analyst
publishes a spec, the reviewer judges it, and you **approve or reject** before any
code is written. After the implementer commits into an isolated worktree, the
quality-auditor scores the diff against the evaluation profile and the platform
computes the verdict deterministically:

```sh
devai task report 01a1a…                       # verdict: S, floor breaches, findings with file:line
devai task approve 01a1a… --rationale "…"      # the human gate before code
devai task run     01a1a… --agent hermes       # implementation + QA cycle
devai task accept  01a1a… --rationale "…"      # your signature on the verdict
devai task merge   01a1a…                      # fast-forwards only a clean-ancestry branch
```

Rejected work returns via `devai task return <id> --rationale "…"`; the QA findings
are delivered verbatim into the next implementation prompt, so the loop closes
without manual copying.

Watching the queue:

```sh
devai task active       # where everything is right now
devai task progress <id>
devai exec list         # role executions, live and historical
devai artifact list <id>
devai artifact cat <artifact-id>
```

## The evaluation profile

`config/evaluation/quality.yaml` is the court by which code is judged:
an open `error_types` taxonomy linked many-to-many to `checks`; binary `gates`
(build, tests, no-secrets, no-network — a failed gate zeroes everything);
rubric/hybrid checks with mandatory evidence and anchor scales; severity-derived
`floors` (a critical finding fails the verdict no matter how nice the rest is);
weights summing to exactly 1.0; and a deterministic verdict function
PASS / FAIL / INCONCLUSIVE — a missing human signature is *inconclusive*, never
a silent pass. Edit the YAML, run `devai init`, and the next execution is judged
by the new version; old reports resolve the exact profile they were computed with.
Grammar of the schema: `docs/evaluation-schema.md`.

## Repository layout

| Path | Role |
|---|---|
| `crates/devai-core` | domain types + workflow transition rules (sole owner of state-machine semantics) |
| `crates/devai-store` | append-only SQLite store, content-addressed artifacts, config seeding |
| `crates/devai-api` | the typed public boundary: Req/Outcome, machine error codes, operation registry |
| `crates/devai-runtime` | execution engine, role loops, gates, evaluation assembly |
| `crates/devai-cli` | input/render only — the first consumer of the API |
| `docs/` | evaluation schema, CLI contract snapshots, design notes |

## Notes

- Linux x86_64 release assets are built by GitHub Actions on every `v*` tag;
  CI (fmt gate, build, tests) runs on every push.
- `devai` itself never phones home; the agent CLIs you configure may.
- The whole state is one SQLite file plus blobs — back up `$DEVAI_HOME`.
