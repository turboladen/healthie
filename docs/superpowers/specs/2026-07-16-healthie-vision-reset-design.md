# Healthie Vision Reset — Design

**Date:** 2026-07-16
**Status:** Approved design, pre-implementation
**Supersedes:** the original Tauri/XML-parser vision (bd epics `apple-health-data-*`)

## Vision

Healthie is the system of record for Steve's body & soul — curated health data, concerns,
goals, and interventions — with an MCP surface that turns a recurring Claude conversation
into an effective, accountable health coach. Glovebox is to the cars what Healthie is to
the body: same pattern (`../personal-domain-pattern`), same stack, different domain.

The predecessor experiment (a Claude Cowork scheduled Sunday job: pull Apple Health data,
discuss the week, schedule workouts to iCal, log a summary to NotePlan) produced fluffy
guidance and mediocre plans because Claude had no durable, structured context. Healthie
fixes that: the data, the history, the commitments, and the outcomes all persist here, and
each checkin starts from them. **The accountability loop is the product.**

NotePlan is retired as the weekly memory; healthie replaces it. iCal remains an output.

## Decisions log

| Decision | Choice |
| --- | --- |
| Where intelligence lives | Hybrid: pure content store + semantic MCP context tools; small deterministic rules layer ONLY for safety-critical items (age/sex screening schedules, PT must-do rotation, supplement review dates). All other judgment is Claude's, at conversation time. |
| Data depth | Curated dailies + episodes. Daily/weekly aggregates for a curated metric list; raw high-resolution series only for flagged episodes. No full Apple Health mirror. |
| Checkin home | Claude conversation (any surface), scripted by healthie's MCP server; every answer persisted as structured checkin data. |
| Ingestion | Health Auto Export REST automation POSTs daily JSON to `/ingest/hae` on the odroid; HAE's MCP server is the gap-fill fallback. |
| Goal model | Concern → Goal → Protocol chain (see Domain model). |
| Whose health | Steve only, full model; family illness as lightweight `FamilyEvent` context, queryable but not per-person modeling. |
| Plan output | Healthie stores each checkin's Plan as source of truth; Claude pushes copies outward (iCal). |
| Build order | Checkin-loop first (see Roadmap). |
| Checkin cadence | Cadence-agnostic: a checkin covers "since the last checkin." No assumption of weekly. |

## The core loop

1. On whatever day the checkin happens, Claude calls `start_checkin`.
2. Healthie returns a **briefing**: the previous plan and its committed items, active
   concerns with last status and their *specialist lenses* (domain tags — the briefing
   prompts Claude to reason as GP / neurologist / psychiatrist / physical trainer /
   physiologist / nutritionist as each concern requires, including several at once),
   goal progress computed from ingested metrics, active protocols and adherence,
   AI/rules-origin observations awaiting review, due items from the rules layer, and
   data gaps.
3. Claude interviews like the right specialist(s) — "you were dealing with the back
   flare-up, how's that going?" — persisting answers via `record_checkin_response`.
4. Evidence review: did metrics move toward goals? Is each active protocol working?
   Verdicts recorded via `record_protocol_outcome` (e.g. "keto: weight ↓ but LDL ↑,
   abandoned" — permanently on file).
5. A **Plan** is committed (`commit_plan`): workouts drawn from the exercise library
   (PT must-dos rotated in by the rules layer), actions (book screening, refill
   magnesium), nutrition direction for the period, guidance summary. Claude pushes
   workouts to iCal; healthie's copy is canonical.
6. Between checkins, outcomes land via `record_plan_outcome`; the next briefing opens
   with "here's what you committed to — what actually happened?"

## Use cases

- **U1 — Silent ingestion.** HAE POSTs daily JSON; curated `DailyMetric`s land
  unattended. Flagged episodes stored at raw resolution.
- **U2 — Anytime logging.** "Log that my back spasmed getting out of the car" →
  `log_observation` / `log_symptom` any day. Future: quick-entry box in the SPA;
  eventually voice capture (near-term, voice-dictating to Claude on the phone already
  covers it).
- **U3 — Goal discovery.** Claude proposes goals the user didn't know to have, informed
  by the rules layer (screening table: 45 → colonoscopy) and by trends in real data
  ("resting HR drifted up 6bpm this year — want a cardio goal?").
- **U4 — Protocol history as guardrail.** Every intervention ever tried has a stored
  verdict; nothing gets re-suggested blind.
- **U5 — Exercise library.** PT-prescribed back exercises flagged *must-do lifetime*,
  plus general exercises with body-part/frequency metadata so plans rotate correctly.
- **U6 — Family context.** `FamilyEvent`s ("kids had HFMD, week 2") queryable against
  the user's own data (e.g. anxiety spikes vs. kids-sick periods).
- **U7 — Doctor records in.** Doctor printouts and MyHealth (GP iOS app) reports enter
  as Documents with extracted text; lab values lifted into structured `LabResult`s;
  `summarize_for_appointment` assembles relevant history before a visit.
- **U8 — Human browsing.** Svelte SPA for what conversation is bad at: trend charts,
  concern/goal/protocol timelines, checkin history, correcting mistakes.
- **U9 — Baseline intake.** One-time (occasionally re-run) comprehensive interview
  (age, sex, history, meds, surgeries, family history) seeding the system — the
  make/model/year → factory-service-schedule move. Backfill uses **claims with
  confidence**, not facts: each screening/immunization is `verified` (document on
  file) / `recalled` / `unknown` / `not-done`. `unknown` is a task to resolve ("ask GP
  next visit"), never a nag. History converges over time.
- **U10 — Supplement & diet review.** Supplements are Protocols with a *purpose* (linked
  concern) and a *review-by* date; "still needed?" surfaces at checkins. Plans include a
  nutrition section supporting active goals/protocols.

## Domain model

Pattern vocabulary mapping (typed specifically in code, per the discipline):

**Entities**

| Type | What it is | Key fields |
| --- | --- | --- |
| `Profile` | Singleton — the user | DOB, sex, height, blood/family-history notes |
| `Concern` | Ongoing tracked thing about body/soul | name, domain tags (musculoskeletal, neurological, mental-health, cardiovascular, metabolic, nutrition, preventive, …), status (active/monitoring/resolved), opened date, narrative |
| `Goal` | Target state, usually under a Concern | target (metric kind + range/direction, or qualitative), target date, status |
| `Protocol` | Intervention being run | kind (diet/exercise/supplement/therapy/screening/habit), Concern/Goal links, schedule, start/end, purpose, review-by date, **outcome verdict + rationale** |
| `Exercise` | Library item | body parts, PT-must-do flag, frequency guidance, instructions |

**Events (immutable)**

| Type | What it is |
| --- | --- |
| `DailyMetric` | Curated aggregate per metric kind per day (from ingest) |
| `Episode` | Raw high-res series worth zooming into, with reason |
| `Observation` | One-off noticing. `origin` = `self` (felt it) / `ai` (Claude spotted it in data; persists past the conversation, reviewable, can seed a Concern) / `rules` (deterministic flag). Severity, optional Concern link. |
| `Checkin` | Structured interview record: responses, concern status updates, family context. Cadence-agnostic. |
| `LabResult` | Analyte + value + unit + reference range + date, lifted from Documents |
| `PlanItemOutcome` | Did/didn't-do signal per Plan item (adherence) |
| `FamilyEvent` | Who-label + what + when; context only |

**Documents** — file + extracted text, attachable to Concerns, LabResults, Protocols.

**Goal+Plan slot** — `Plan`: starts-on + horizon (default ≈ a week), workout items
(Exercise refs), action items, nutrition direction, guidance summary.

**Rules layer** (deterministic code in `healthie-shared`, not schema): screening/
immunization recommendation table keyed by age/sex, compared against the
claims-with-confidence registry; PT must-do rotation constraints; supplement review-by
surfacing. Exposed via one tool: `get_due_items`.

## Architecture

```
healthie/
├── healthie-shared/     # SeaORM entities, migrations, services, validation,
│                        #   rules layer, FTS5
├── healthie-backend/    # Axum: JSON API for SPA + POST /ingest/hae
├── healthie-mcp/        # rmcp server: tools + resources over shared services
└── frontend/            # Svelte 5 + Vite + Bun SPA, baked into the binary
```

Rust edition 2024; single deployable binary → odroid n2+ (dietpi); one SQLite file;
documents on disk under `{DATA_DIR}/files/...`. Business logic in the library, never in
handlers or tools.

**`/ingest/hae`**: unattended machine-to-machine write path. Bearer-token auth,
idempotent upserts keyed on (metric kind, date) so HAE retries are harmless. Curation
config (which HK metric kinds to keep, at what aggregation) lives in the DB, editable
via UI/MCP later. Unknown metric kinds land in a quarantine table (inform curation
tuning), never silently dropped.

**Document upload — callback pattern (by design):** Claude Desktop cannot push PDFs
through MCP calls (file-size limits, as learned on glovebox). `store_document` therefore
creates a pending-attachment record bound to its target and returns an upload URL; the
user clicks it, a minimal web upload page receives the file, extraction runs
server-side.

## MCP surface

Semantic domain verbs, never CRUD — the LLM knows verbs, not schema.

- **The loop:** `start_checkin`, `record_checkin_response`, `commit_plan`,
  `record_plan_outcome`
- **Anytime:** `log_observation`, `log_symptom`, `record_lab_results`, `store_document`
- **Lifecycle:** `open_concern`, `update_concern_status`, `set_goal`, `start_protocol`,
  `record_protocol_outcome`
- **Interrogation:** `summarize_trends(metric, window)`, `get_due_items`,
  `list_active_experiments`, `get_protocol_history`, `summarize_for_appointment`,
  `search(query)` (FTS5 across everything), `get_data_gaps`
- **Intake:** `run_baseline_intake`, `record_intake_answers`

**Resources:** current briefing; per-Concern dossier; goal progress; recent activity
feed.

**Prompt:** `checkin` — the scripted conversation opener, identical from any Claude
surface.

## Data flow

```
daily     HAE ──POST /ingest/hae──▶ backend ──▶ curated DailyMetrics
anytime   user/Claude ──log_observation / log_symptom──▶ Observations
checkin   Claude ──start_checkin──▶ briefing (plan review, concerns + lenses,
            goal progress, due items, observations to review, data gaps)
          interview ──record_checkin_response──▶ Checkin rows
          evidence  ──record_protocol_outcome──▶ verdicts
          plan      ──commit_plan──▶ Plan ──Claude──▶ iCal
between   outcomes ──record_plan_outcome──▶ adherence for next briefing
```

## Error handling

- **Ingest is untrusted input:** malformed payloads → 400 + log; unknown metric kinds →
  quarantine table; idempotent upserts throughout.
- **Irregular checkins:** the briefing is date-aware; a multi-week gap widens its
  windows and says so. Nothing assumes a fixed cadence.
- **Odroid unreachable:** HAE queues and re-sends; `get_data_gaps` surfaces holes; the
  HAE MCP server couriers missing days during a conversation.
- **Dropped conversation mid-checkin:** checkin tools are append-only events, so a
  partial checkin is valid and resumable.

## Testing

- Unit tests on shared services (validation invariants: weight positive, verdicts
  require an ended protocol, …).
- Table-driven tests for the rules layer (age/sex → expected due items; rotation
  constraints).
- Golden-file tests for `/ingest/hae` against real HAE JSON exports.
- Integration tests for MCP tools against the real service layer on temp SQLite.
- The briefing assembler is the highest-value test target — it is the product.

## Roadmap

- **M1 — The loop, minimal.** `healthie-shared` + `healthie-mcp` only. Schema for
  Profile/Concern/Goal/Protocol/Checkin/Observation/Plan; checkin + logging + lifecycle
  tools; `checkin` prompt; baseline intake to seed. **First real checkin possible here**
  — before any UI or ingest.
- **M2 — Data flows in.** `healthie-backend` with `/ingest/hae`; curated DailyMetrics;
  `summarize_trends`; goal progress in briefings. Deploy to odroid.
- **M3 — Rules layer.** Screening table + claims-with-confidence registry; exercise
  library + PT rotation; supplement review-by. `get_due_items` live.
- **M4 — Documents & labs.** `store_document` with the callback-upload page (first web
  surface — a single route, does not wait for the SPA), text extraction, LabResults,
  `summarize_for_appointment`, MyHealth/printout import path.
- **M5 — The SPA.** Dashboards, timelines, checkin history, quick-entry observation box,
  data correction, episodes, `get_data_gaps` polish.

## Future hooks (explicitly deferred)

- **Checkin types:** monthly/quarterly/annual reviews with their own scripts (annual ≈
  partial intake re-run). Schema stays cadence-agnostic so this is additive.
- **fewd interconnection:** per the pattern's Phase 3 ruling, healthie never couples to
  fewd at the storage layer. When fewd has an MCP server, one Claude session connects to
  both: healthie provides goals/protocols/nutrition direction, fewd provides actual
  meals. Zero build cost now.
- **Voice quick-entry** in the SPA.
- **Phase 2 extraction** candidates (MCP scaffolding, document pipeline) per the pattern
  doc, once ≥3 apps share the shape.

## Housekeeping

The existing bd epics (`apple-health-data-*`: Tauri scaffold, XML parser, in-app
analysis engine, ECharts dashboards) describe the superseded vision. When M1 issues are
filed, supersede those epics (`bd supersede`) rather than deleting — the analysis ideas
(SpO2 desaturation, HRV correlation) may inform later `summarize_trends`/Episode work.
