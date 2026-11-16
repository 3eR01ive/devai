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

### Install script (no sudo)

```sh
curl -fsSL https://raw.githubusercontent.com/3eR01ive/devai/main/install.sh | sh
```

It fetches the latest linux-x86_64 release, verifies its sha256, and installs
**both** binaries — `devai` (CLI) and `devai-ui` (window) — into `~/.local/bin`.
Then it finishes the parts a release archive cannot do by itself:

- **creates the home** (`$DEVAI_HOME`, default `~/.devai`) by running `devai init`
  — config tree, roles, evaluation profiles and the store — unless a home
  already exists. An existing home is never touched: that is your data;
- **adds an application-menu entry** with the icon
  (`~/.local/share/applications/devai-ui.desktop`), so the window is launchable
  without a terminal;
- **checks the window's system libraries** (webkit2gtk 4.1, gtk3, rsvg) and
  names what is missing together with the package hint for your distribution —
  the CLI keeps working without them;
- prints the one `PATH` line you need; with `--add-path` it appends it to your
  shell profile once (guarded against duplicates).

Nothing is written outside `$HOME` and no sudo is used.

Flags (after `sh -s --` when piping):

```sh
curl -fsSL https://raw.githubusercontent.com/3eR01ive/devai/main/install.sh | sh -s -- --no-init
```

- `--no-init` — do not create the home (run `devai init` yourself later)
- `--no-desktop` — do not add the menu entry
- `--add-path` — append the `PATH` line to your shell profile

Overrides: `DEVAI_VERSION` (pin a tag), `DEVAI_INSTALL_DIR` (binaries
directory), `DEVAI_HOME` (which home to create).

### From source

Requires a stable Rust toolchain (edition 2021) and `git`:

```sh
git clone https://github.com/3eR01ive/devai.git
cd devai
cargo build --release --workspace
# binaries at target/release/devai and target/release/devai-ui
```

The repository's own helper puts them where the launchers expect them and builds
the frontend first (the window embeds it at compile time):

```sh
./build.sh            # debug build  -> ./bin/devai, ./bin/devai-ui
./build.sh --release  # release build
```

`build.sh` installs the frontend dependencies when `node_modules` is missing or
older than `package.json`, runs `vite build` **before** cargo (a stale `dist`
gives a window without assets), and copies the binaries into `./bin/` so a stale
`target/` binary can never masquerade as fresh after a schema bump.

## The desktop window (devai-ui)

The window is a Tauri app; its frontend is a Vue/Vite SPA in
`crates/devai-ui/frontend`. It talks to the same platform API as the CLI — every
click is a published operation, and the window keeps a mutation allowlist that is
enforced by tests.

### Run it (development)

```sh
devai init                 # once: materialize $DEVAI_HOME (config tree + store)
./run-ui.sh                # ~/.devai
./run-ui.sh --home /path   # another home
```

Why a launcher: the **debug** binary loads the SPA from `http://127.0.0.1:1420`,
so the window needs Vite running. `run-ui.sh` starts Vite, waits for the port,
launches the window and stops the server when the window closes.

Equivalent by hand:

```sh
# terminal 1 — frontend dev server
cd crates/devai-ui/frontend && npx vite --host 127.0.0.1 --port 1420 --strictPort

# terminal 2 — the window
DEVAI_HOME="$HOME/.devai" \
DEVAI_RUNNER="$PWD/bin/devai" \
WEBKIT_DISABLE_COMPOSITING_MODE=1 \
./bin/devai-ui
```

Environment the window honours:

- `DEVAI_HOME` — which home to show (default `~/.devai`).
- `DEVAI_RUNNER` — the `devai` binary the window calls to launch role runs. Without
  it the card honestly reports `no platform runner binary — set DEVAI_RUNNER`;
  the window never runs a role inside its own process.
- `WEBKIT_DISABLE_COMPOSITING_MODE=1` — turns compositing off; without it some
  compositors render a blank/black window.

### Run it (release build)

```sh
./build.sh --release
./run-ui.sh --release
```

Known upstream limitation: with webkit2gtk ≥ 2.46 the standalone (release) window
can come up blank because of the custom-scheme handling (wry#1725). The
debug + Vite path above is the one that works today.

### From the phone

The window itself serves LAN access (`Телефон` tab): it shows a QR invite, and the
phone then calls the same command plane — the same operations, the same
allowlist, recorded under the operator with a device suffix.

### What a release archive contains

`devai-<tag>-linux-x86_64.tar.gz` holds `devai` (CLI), `devai-ui` (window),
`run-ui.sh` and this README, plus a `.sha256` next to it. `install.sh` installs
both binaries into `~/.local/bin`.

**No APK in the release artifacts yet**: the Android build is
`./build.sh --android` (needs JDK 21 + Android SDK/NDK, see
`scripts/android-env.sh`) and is not published with the linux-x86_64 archive.

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
