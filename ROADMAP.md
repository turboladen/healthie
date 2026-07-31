# Roadmap

The milestones **M1–M5** are the durable plan, decided in
[ADR-0002](docs/adr/0002-personal-domain-pattern-rebuild.md#roadmap) — that ADR
is the immutable origin. This file is the **living** breakdown: as each
milestone is sliced into shippable chunks, the lettered sub-milestones
(M1a, M1b, …) are recorded here. They are working labels for grouping PRs and
beads, not separate ADR-level decisions. The authoritative tracking is always
the beads; the labels are convenience.

## M1 — The checkin loop (minimal)

The shared domain lib + MCP server; first real checkin before any UI or ingest.

- **M1a** — `healthie-shared` domain library (entities, migrations, services,
  briefing assembler). — PR #3, bead healthie-2hu ✅
- **M1b** — `healthie-mcp` server: 15 tools, `healthie://briefing` resource,
  `checkin` prompt, bearer auth. — PR #8, bead healthie-1ci ✅
- **M1c** — baseline intake: claims-with-confidence registry, 4 intake tools,
  `baseline_intake` prompt ([ADR-0004](docs/adr/0004-claims-registry.md)). —
  PR #9, bead healthie-26g ✅

## M2 — Data flows in

`healthie-backend` (the single deployed binary), `/ingest/hae`, curated
DailyMetrics; deploy to the odroid.

- **M2** (this slice) — backend crate, `daily_metric` store, `ingest_hae`,
  kinded `auth_token`, `/mcp` mount
  ([ADR-0005](docs/adr/0005-metrics-ingest-and-backend.md)). — PR #10,
  bead healthie-267 ✅
- **M2b** — the _reading_ layer deferred out of M2 (better designed once real
  data has landed): `summarize_trends` MCP tool, goal-progress in briefings,
  metric-tracking recommendations, sleep start/end timestamps. —
  bead healthie-bx9
- Supporting work in the M2 arc: Apple Health history backfill
  (healthie-4lf), odroid deploy kit (healthie-93n), and the real data-first
  baseline on the deployed box (healthie-sj9).

## M3 — Rules layer

Screening table + claims-with-confidence registry, exercise library + PT
rotation, supplement review-by; `get_due_items` live. — not yet subdivided.

## M4 — Documents & labs

`store_document` callback-upload page, text extraction, LabResults,
`summarize_for_appointment`. — not yet subdivided.

## M5 — The SPA

Dashboards, timelines, checkin history, quick-entry, data correction. —
bead healthie-hs1, not yet subdivided.
