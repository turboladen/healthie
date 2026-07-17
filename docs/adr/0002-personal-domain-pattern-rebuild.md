# ADR-0002: Rebuild on the personal-domain pattern — the checkin loop is the product

- **Status:** Accepted
- **Date:** 2026-07-16
- **Related:** healthie-hf2 (M1 epic), healthie-19k, PR #3 (M1a domain library),
  `CLAUDE.md` (architecture + conventions).
  Full spec text: `docs/superpowers/specs/2026-07-16-healthie-vision-reset-design.md`
  and `docs/superpowers/plans/2026-07-16-m1a-healthie-shared.md`, removed in this
  PR — retrieve via git history.

## Context

Healthie began as a Tauri desktop app with an Apple Health XML parser, an in-app
analysis engine, and ECharts dashboards (the `apple-health-data-*` bd epics). Before
that vision shipped, a predecessor experiment ran: a Claude Cowork scheduled Sunday
job that pulled Apple Health data, discussed the week, scheduled workouts to iCal,
and logged a summary to NotePlan. It produced fluffy guidance and mediocre plans
because Claude had no durable, structured context — no persisted history,
commitments, or outcomes to start from.

Meanwhile glovebox proved the **personal-domain pattern**
(`../personal-domain-pattern`): a Rust system-of-record with a semantic MCP surface
that turns a recurring Claude conversation into an effective domain assistant.
Glovebox is to the cars what healthie should be to the body — same pattern, same
stack, different domain.

## Decision

Rebuild healthie as the **system of record for body & soul** — curated health data,
concerns, goals, and interventions — with an MCP surface that scripts an
accountable health-coach checkin conversation. **The accountability loop is the
product.** NotePlan is retired as the weekly memory; healthie replaces it (iCal
remains an output). The Tauri/XML-parser vision is superseded.

### Decisions log

| Decision | Choice |
| --- | --- |
| Where intelligence lives | Hybrid: pure content store + semantic MCP context tools; a small deterministic rules layer ONLY for safety-critical items (age/sex screening schedules, PT must-do rotation, supplement review dates). All other judgment is Claude's, at conversation time. |
| Data depth | Curated dailies + episodes. Daily/weekly aggregates for a curated metric list; raw high-resolution series only for flagged episodes. No full Apple Health mirror. |
| Checkin home | Claude conversation (any surface), scripted by healthie's MCP server; every answer persisted as structured checkin data. |
| Ingestion | Health Auto Export REST automation POSTs daily JSON to `/ingest/hae`; HAE's MCP server is the gap-fill fallback. Bearer-token auth, idempotent upserts keyed on (metric kind, date), unknown metric kinds quarantined — never silently dropped. |
| Goal model | Concern → Goal → Protocol chain. Protocols carry a purpose, review-by date, and a permanent outcome verdict + rationale — nothing gets re-suggested blind. |
| Whose health | Steve only, full model; family illness as lightweight `FamilyEvent` context, queryable but not per-person modeling. |
| Plan output | Healthie stores each checkin's Plan as source of truth. Plan items are typed; each kind maps to a natural external destination Claude pushes to at the conversation layer: time-bound items → calendar (iCal), discrete actions → a task system, guidance/nutrition direction → stays in healthie. Destinations are pluggable without schema changes. |
| Build order | Checkin-loop first (see Roadmap below). |
| Checkin cadence | Cadence-agnostic: a checkin covers "since the last checkin." No assumption of weekly. |

### Architecture

Three crates plus a SPA, shipping as **one binary** to the odroid n2+ (dietpi) with
a single SQLite file (documents on disk under `{DATA_DIR}/files/`):

- `healthie-shared` — SeaORM entities, migrations, services, validation, rules
  layer, FTS5. **Business logic lives here, never in handlers or tools.**
- `healthie-backend` — axum: JSON API for the SPA + `POST /ingest/hae`.
- `healthie-mcp` — rmcp server: tools + resources over the shared services.
- `frontend` — Svelte 5 + Vite + Bun SPA, baked into the binary.

Rust edition 2024. Dependency policy: pin the latest stable of everything at
kickoff and verify APIs against current documentation at implementation time — not
training-data memory.

### MCP surface discipline

Tools are **semantic domain verbs, never CRUD** — the LLM knows verbs, not schema:

- The loop: `start_checkin`, `record_checkin_response`, `commit_plan`,
  `record_plan_outcome`
- Anytime: `log_observation`, `log_symptom`, `record_lab_results`, `store_document`
- Lifecycle: `open_concern`, `update_concern_status`, `set_goal`,
  `start_protocol`, `record_protocol_outcome`
- Interrogation: `summarize_trends`, `get_due_items`, `list_active_experiments`,
  `get_protocol_history`, `summarize_for_appointment`, `search`, `get_data_gaps`
- Intake: `run_baseline_intake`, `record_intake_answers`

Plus resources (current briefing, per-Concern dossier, goal progress, recent
activity) and a `checkin` prompt — the scripted conversation opener, identical from
any Claude surface.

### Robustness calls baked into the shape

- Checkin tools are append-only events: a dropped conversation mid-checkin leaves a
  valid, resumable partial checkin.
- The briefing is date-aware; a multi-week gap widens its windows and says so.
- Baseline intake records **claims with confidence** (`verified` / `recalled` /
  `unknown` / `not-done`), not facts; `unknown` is a task to resolve, never a nag.
- Document upload uses a callback pattern: `store_document` returns an upload URL
  because Claude clients cannot push files through MCP calls (size limits, learned
  on glovebox).

### Roadmap

- **M1 — The loop, minimal.** `healthie-shared` + `healthie-mcp` only: schema,
  checkin/logging/lifecycle tools, `checkin` prompt, baseline intake. First real
  checkin possible here — before any UI or ingest. (M1a = the domain library.)
- **M2 — Data flows in.** `healthie-backend` with `/ingest/hae`, curated
  DailyMetrics, `summarize_trends`, goal progress in briefings. Deploy to odroid.
- **M3 — Rules layer.** Screening table + claims-with-confidence registry,
  exercise library + PT rotation, supplement review-by. `get_due_items` live.
- **M4 — Documents & labs.** `store_document` callback-upload page, text
  extraction, LabResults, `summarize_for_appointment`.
- **M5 — The SPA.** Dashboards, timelines, checkin history, quick-entry, data
  correction.

### Explicitly deferred

Typed checkin cadences (monthly/quarterly/annual scripts — schema stays
cadence-agnostic so this is additive); fewd interconnection (MCP-level only, one
Claude session connecting to both servers — never storage-layer coupling); voice
quick-entry in the SPA; Phase-2 pattern extraction (MCP scaffolding, document
pipeline) once ≥3 apps share the shape.

## Consequences

- **Positive:** every checkin starts from persisted data, history, commitments, and
  outcomes — fixing precisely what made the predecessor experiment fluffy. Protocol
  verdicts are permanently on file; the next briefing opens with "here's what you
  committed to — what actually happened?"
- **Positive:** the briefing assembler is the highest-value test target — it *is*
  the product — alongside table-driven rules tests, golden-file ingest tests, and
  MCP integration tests on temp SQLite.
- **Negative / limits:** single-user by design; the curated-metrics choice means
  raw history outside flagged episodes is not retained locally.
- This ADR replaces the vision-reset spec as the durable record; the spec and its
  M1a plan are retired to git history. Re-scoping any decision above means a
  superseding ADR, not an edit.
- The superseded `apple-health-data-*` epics' analysis ideas (SpO2 desaturation,
  HRV correlation) may still inform later `summarize_trends`/Episode work.
