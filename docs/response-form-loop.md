# Response-form correction loop (task 01a1a2d8, REQ-FG-01..08)

A role's failed reply is TWO different facts: the FORM (reply shape violates
the stored contract — cheap for the same role to fix) and the SUBSTANCE or
the run's ENVIRONMENT (provider down, process died, platform bug). Before
this feature a form refusal ate the whole cycle: task failed, operator
re-launched. Now the platform re-prompts the same role with the full
violation list, up to a declarative budget, inside one cycle.

## Failure taxonomy — every class, its handling, its owner

Stored on `role_executions.failure_class` (schema v5) as one of three values;
the engine and the adapter never infer it from error text.

| class | source (Rust) | handling | owner of the defect |
|---|---|---|---|
| `response_form` | `AdapterError::BadOutput { violations }` (adapters.rs) | the loop re-prompts the SAME role with the full list; attempts burn; on exhaustion a blocking question parks the task | the role's reply shape |
| `provider` | `AdapterError::Agent / Io / UnknownProvider` | NEVER replayed as a form repair; the stage fails immediately, workflow state untouched (a provider failure is not something re-typing the prompt fixes; provider-level retries are out of scope, ratified OQ-FG-04) | the agent process / provider |
| `platform` | `AdapterError::Platform`; worktree/evidence/prompt-write and post-loop stamping/assembly failures | the stage fails immediately; the role is not guilty of a shape the PLATFORM assembled — replaying it would spend role cycles on a platform bug | devai itself |

Pre-loop platform-side failures (worktree prepare, evidence dir, probes and
auditor-prompt storage) also stamp `platform` — an operator reading
`devai exec list` gets the class as data, not a text to parse.

## The budget is config, not code

`config/workflow/response-form-loop.yaml`:

```yaml
response_form:
  max_repair_attempts: 2   # RE-PROMPTS on top of the first call
```

- Total provider calls per stage = `1 + max_repair_attempts` (default 2 → at
  most 3 calls).
- `0` disables the loop — exactly the pre-feature behavior: one call, a form
  failure fails the execution, NO escalation question is invented.
- The key is `response_form.max_repair_attempts` in the platform-owned
  `config/workflow/` tree (ratified OQ-FG-01). Loading:
  `devai_store::config::load_response_form_policy` merges the last declared
  `response_form:` section found in sorted `workflow/*.yaml`; files without
  the section (e.g. `rework-context.yaml`) are inert. A malformed value is a
  LOUD config error at `devai init` (seed.rs) and at engine start — never a
  silently ignored typo. Editing the value needs no rebuild.
- The loop runs for EVERY role (ratified OQ-FG-09); there is no
  read/write-role asymmetry.

## Null policy, three layers (ratified OQ-FG-03)

1. **Schema authority** — the stored reply-shape contract declares every
   field an agent legitimately cannot know or fill: `requirements-specification@2`
   makes `task_id` (`["string","null"]`, platform-stamped) and
   `business_description` (declared + optional, platform-completed) skippable;
   echo stamping of `implementation-report@2` (`branch`/`worktree`/`commit`)
   is the same pattern, now paired with the analyst contract too.
   `null` is a legal type ONLY where the schema says so; a required key whose
   value is null still fails.
2. **Lenient read** — the validator (`validate_node`) skips null checks on
   OPTIONAL fields; an absent optional field and `null` are equivalent for
   older replies.
3. **Strict write** — the prompt contract (config/prompts/common.yaml rule)
   and the repair section forbid inventing nulls: an optional field you
   cannot fill is OMITTED, never written as `null`.

## One execution row per attempt (schema v5, ratified OQ-FG-06)

Columns `attempt` (1-based), `parent_execution` (lineage to the FIRST attempt
row of the stage), `failure_class`. Pre-loop rows read as attempt 1, NULL
parent, NULL class — exactly what they were. Migration rung
`005_role_execution_form_attempts` applies idempotently through the manifest
handler `execution_form_columns` (SQLite has no `ADD COLUMN IF NOT EXISTS`).

## Observability — loud by construction (REQ-FG-07)

- `execution.form_rejected` event per rejection, payload
  `{execution_id, role, attempt, violations[]}` — the FULL list, never the
  first item (the 2026-11-04 defect shape: `BadOutput` carried only
  summarize()'s one-line text, the list died in a local variable).
- `role_executions` rows as above; `devai exec list` shows
  `attempt#N [response_form]` from row data.
- On exhaustion: `execution.form_escalated` event + a blocking question
  (OQ-FG-08) whose text carries the full list and says to fix the FORM
  source; the task parks in `WAITING_FOR_INPUT`, never a bare FAILED.
  Answering does not replay the provider (no session continuation, ratified
  OQ-FG-10): re-run `devai task run` for a fresh full cycle.
- `run_with_form_loop` publishes nothing until an attempt passes the gate;
  rejected bodies never become artifacts.

## Manual publish parity (ratified OQ-FG-05)

`devai artifact publish` runs the same `contract_violations` gate as the
engine (commands.rs `publish_raw`): violations are printed one `warning:`
line each — the complete list — and the publish is REJECTED. The contract is
authority regardless of the publication path; choosing the CLI is not a way
around the gate.
